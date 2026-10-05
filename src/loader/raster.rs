//! Header-only inspection of raster image files into a `FileEntry`
//! (`docs/design/image-formats.md` sections 2.4, 4 and 5.1).
//!
//! Discovery never decodes pixel data and never reads a file to its end to
//! describe it: each format is read only as far as the table in section 4
//! says. Decoding belongs to `pixels/` and is not part of this module.

use super::{discovery::DiscoveryReason, entry::EntryInspection};
use crate::api::contracts::{RasterColorType, RasterSampleFormat};
use crate::types::{FileEntry, NativePixelDataKind, RasterMetadata, SeriesMetadata};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

mod headers;
mod tiff;
use crate::types::FileFormat;
use anyhow::Result;
use std::path::Path;

/// Inspects `path`, whose first bytes carry the signature of `format`, into a
/// catalog entry without decoding any pixel data.
///
/// `format` is a raster format (never [`FileFormat::Dicom`]) and has already
/// been matched against the file's signature and the `--formats` selection.
///
/// # Outcomes
///
/// - `Ok(EntryInspection::Selected(entry))` when the header describes an
///   image: positive width and height and a layout the table below maps.
/// - `Ok(EntryInspection::Skipped(DiscoveryReason::RasterHeaderInvalid))`
///   when the signature matched but the header does not parse: truncated
///   before the fields below, a corrupt chunk or marker structure, zero
///   width or height, or (TIFF) a first page that cannot be read.
/// - `Err(_)` only for an I/O error other than reaching the end of the file
///   (the caller reports `DiscoveryReason::InspectionFailed`). A file that
///   ends early is `RasterHeaderInvalid`, not an error.
///
/// This function must not panic on any input, must not allocate from a
/// length the header declares without bounding it by the file's length, and
/// must not read pixel data.
///
/// # What is read
///
/// | Format | Read | Not read |
/// |---|---|---|
/// | PNG | chunks up to, not including, the first `IDAT`: `IHDR`, `PLTE`, `tRNS`, `sBIT`, `iCCP` (presence), `eXIf`, `acTL` | any `IDAT` |
/// | JPEG | markers up to and including the first `SOFn`: `APP1` EXIF (orientation), `APP2` ICC (presence), `APP14` Adobe | entropy-coded data; the file is never read whole |
/// | TIFF | the IFD chain: every page's tags | strips and tiles |
/// | WebP | the RIFF chunk headers: `VP8 `/`VP8L`/`VP8X`, `ICCP` (presence), `EXIF`, `ANIM` | image data |
///
/// `image`'s `JpegDecoder::new` reads the whole file into memory and so must
/// not be used here.
///
/// # The entry
///
/// - `format` is `format`; `raster` is `Some`; `path` is `path` as given;
///   `index` is 0 (the registry assigns it).
/// - `label` is the file name.
/// - Every DICOM identity string (`patient_id` through `sop_class_uid`) and
///   `transfer_syntax_uid` is empty. `series_metadata` is the default except
///   `native_pixel.pixel_data_kind`: `Integer`, or `Float32`/`Float64` for
///   32- and 64-bit float samples, so `value_mapping::stored_value_type`
///   answers for a raster as it does for DICOM.
/// - `has_pixels` is `true`; `rescale_slope` is 1, `rescale_intercept` 0;
///   `default_window` is `None`.
/// - `rows` and `columns` are the stored pixel grid. Orientation is recorded
///   in `raster.orientation` and never applied here.
/// - `frame_count` is `raster.frame_pages.len()`: 1 for PNG, JPEG and WebP
///   (the first frame of an animation), and the pages that pass the page
///   rule for TIFF.
/// - The sample layout describes the raw frames a decoder will serve
///   (section 5.2), from what the file stores:
///
/// | Stored | `raster.color_type` | `samples_per_pixel` | `photometric_interpretation` |
/// |---|---|---|---|
/// | gray | `gray` | 1 | `MONOCHROME2`; `MONOCHROME1` for TIFF WhiteIsZero |
/// | gray + alpha | `gray_alpha` | 2 | `MONOCHROME2` |
/// | RGB; JPEG YCbCr | `rgb` | 3 | `RGB` |
/// | RGB + alpha | `rgba` | 4 | `RGBA` |
/// | palette | `palette` | 3, or 4 with transparency | `RGB`, or `RGBA` |
/// | CMYK, YCCK (JPEG) | `cmyk` | 3 | `RGB` |
///
/// - `raster.bit_depth` is the stored bits per sample. `bits_allocated` is
///   the width a raw sample is served in: 8 for depths up to 8, else the
///   depth (16, 32 or 64). A palette file's `bits_allocated` is 8 whatever
///   its index depth. `pixel_representation` is 1 for signed integer samples
///   and 0 otherwise.
/// - `raster.orientation` is the EXIF orientation (JPEG `APP1`, PNG `eXIf`,
///   WebP `EXIF`) or TIFF tag 274; 1 when absent or outside 1 to 8.
/// - `raster.has_alpha` is true for an alpha channel, PNG `tRNS` (on any
///   colour type), WebP alpha, or TIFF `ExtraSamples` 1 or 2;
///   `alpha_associated` only for TIFF `ExtraSamples` 1.
/// - `raster.animated` is true for a PNG with `acTL` and an animated WebP.
/// - `raster.significant_bits` is the PNG `sBIT` payload; `None` otherwise.
///
/// # TIFF pages (section 2.4)
///
/// Page 0 is always frame 0. A later page is a frame when it is not flagged
/// reduced-resolution (`NewSubfileType` bit 0) or mask (bit 2) and equals
/// page 0 in width, height, samples per pixel, sample format, bits per
/// sample, photometric interpretation, extra-sample type and orientation.
/// Any other page goes in `raster.excluded_pages` with the first
/// `RasterPageDifference` that applies, in that enum's declaration order.
/// `frame_pages` and `excluded_pages` are in page order and together hold
/// every page once; `pages_total` is their combined length. A page whose ICC
/// profile presence differs from page 0's is still a frame and adds a
/// warning. If the chain cannot be read past some page, the pages read so
/// far stand and a warning says where the walk stopped. Classic TIFF and
/// BigTIFF, in either byte order, are read alike.
///
/// A first page whose compression or photometric interpretation the reader
/// cannot describe may be reported as `RasterHeaderInvalid`; listing such
/// files as unsupported is the decoder work's decision.
pub(super) fn inspect_raster(_path: &Path, _format: FileFormat) -> Result<EntryInspection> {
    let file = File::open(_path)?;
    let len = file.metadata()?.len();
    let mut input = HeaderReader { file, len };
    let parsed = match _format {
        FileFormat::Png => headers::png(&mut input),
        FileFormat::Jpeg => headers::jpeg(&mut input),
        FileFormat::Tiff => tiff::inspect(&mut input),
        FileFormat::Webp => headers::webp(&mut input),
        FileFormat::Dicom => anyhow::bail!("expected a raster format"),
    };
    let header = match parsed {
        Ok(header) if header.width > 0 && header.height > 0 => header,
        Err(error) if is_io_failure(&error) => return Err(error),
        _ => {
            return Ok(EntryInspection::Skipped(
                DiscoveryReason::RasterHeaderInvalid,
            ))
        }
    };
    let raster = header.metadata;
    let (samples_per_pixel, photometric) = match raster.color_type {
        RasterColorType::Gray => (
            1,
            if header.white_is_zero {
                "MONOCHROME1"
            } else {
                "MONOCHROME2"
            },
        ),
        RasterColorType::GrayAlpha => (2, "MONOCHROME2"),
        RasterColorType::Rgb | RasterColorType::Cmyk => (3, "RGB"),
        RasterColorType::Rgba => (4, "RGBA"),
        RasterColorType::Palette if raster.has_alpha => (4, "RGBA"),
        RasterColorType::Palette => (3, "RGB"),
    };
    let mut series_metadata = SeriesMetadata::default();
    series_metadata.native_pixel.pixel_data_kind =
        Some(match (raster.sample_format, raster.bit_depth) {
            (RasterSampleFormat::Float, 32) => NativePixelDataKind::Float32,
            (RasterSampleFormat::Float, 64) => NativePixelDataKind::Float64,
            _ => NativePixelDataKind::Integer,
        });
    let file_name = _path.file_name().unwrap_or_default().to_string_lossy();
    Ok(EntryInspection::Selected(Box::new(FileEntry {
        index: 0,
        path: _path.to_path_buf(),
        format: _format,
        label: super::build_label("", "", "", &file_name),
        patient_id: String::new(),
        patient_name: String::new(),
        study_instance_uid: String::new(),
        study_date: String::new(),
        study_description: String::new(),
        series_instance_uid: String::new(),
        series_number: String::new(),
        series_description: String::new(),
        modality: String::new(),
        instance_number: String::new(),
        sop_instance_uid: String::new(),
        sop_class_uid: String::new(),
        transfer_syntax_uid: String::new(),
        series_metadata: Box::new(series_metadata),
        has_pixels: true,
        frame_count: raster.frame_pages.len() as u32,
        rows: header.height,
        columns: header.width,
        bits_allocated: if raster.color_type == RasterColorType::Palette {
            8
        } else {
            raster.bit_depth.next_power_of_two().max(8)
        },
        pixel_representation: u32::from(raster.sample_format == RasterSampleFormat::Int),
        samples_per_pixel,
        photometric_interpretation: photometric.into(),
        rescale_slope: 1.0,
        rescale_intercept: 0.0,
        default_window: None,
        raster: Some(Box::new(raster)),
    })))
}

struct Header {
    width: u32,
    height: u32,
    white_is_zero: bool,
    metadata: RasterMetadata,
}

impl Header {
    fn new(width: u32, height: u32, color_type: RasterColorType, bit_depth: u32) -> Self {
        Self {
            width,
            height,
            white_is_zero: false,
            metadata: RasterMetadata {
                color_type,
                bit_depth,
                sample_format: RasterSampleFormat::Uint,
                has_alpha: matches!(
                    color_type,
                    RasterColorType::GrayAlpha | RasterColorType::Rgba
                ),
                alpha_associated: false,
                orientation: 1,
                has_icc: false,
                pages_total: 1,
                frame_pages: vec![0],
                excluded_pages: Vec::new(),
                animated: false,
                significant_bits: None,
                warnings: Vec::new(),
            },
        }
    }
}

// Seek past payloads and validate ranges before allocating metadata buffers.
struct HeaderReader {
    file: File,
    len: u64,
}

impl HeaderReader {
    fn check_range(&self, offset: u64, len: u64) -> Result<()> {
        anyhow::ensure!(
            offset <= self.len && len <= self.len - offset,
            "truncated raster header"
        );
        Ok(())
    }

    fn read<const N: usize>(&mut self, offset: u64) -> Result<[u8; N]> {
        self.check_range(offset, N as u64)?;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0; N];
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn bytes(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.check_range(offset, len)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(usize::try_from(len)?)?;
        bytes.resize(len as usize, 0);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

fn is_io_failure(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() != io::ErrorKind::UnexpectedEof)
    })
}

fn orientation(value: u64) -> u8 {
    if (1..=8).contains(&value) {
        value as u8
    } else {
        1
    }
}

/// Only IFD0 of the TIFF block, shared by JPEG, PNG and WebP EXIF.
fn exif_orientation(bytes: &[u8]) -> u8 {
    fn read(bytes: &[u8]) -> Option<u64> {
        let little = match bytes.get(..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let number = |offset: usize, len: usize| -> Option<u64> {
            let slice = bytes.get(offset..offset.checked_add(len)?)?;
            Some(if little {
                slice.iter().rev().fold(0, |n, b| (n << 8) | u64::from(*b))
            } else {
                slice.iter().fold(0, |n, b| (n << 8) | u64::from(*b))
            })
        };
        if number(2, 2)? != 42 {
            return None;
        }
        let ifd = usize::try_from(number(4, 4)?).ok()?;
        let count = usize::try_from(number(ifd, 2)?).ok()?;
        for i in 0..count {
            let offset = ifd.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
            if number(offset, 2)? == 0x0112
                && number(offset + 2, 2)? == 3
                && number(offset + 4, 4)? == 1
            {
                return number(offset + 8, 2);
            }
        }
        None
    }
    orientation(read(bytes).unwrap_or(1))
}
