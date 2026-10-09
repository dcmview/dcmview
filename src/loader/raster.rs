//! Header-only inspection of raster image files into a `FileEntry`
//! (`docs/design/image-formats.md` sections 2.4, 4 and 5.1).
//!
//! Discovery never decodes pixel data and never reads a file to its end to
//! describe it: each format is read only as far as the table in section 4
//! says. Decoding belongs to `pixels/` and is not part of this module.

use super::{discovery::DiscoveryReason, entry::EntryInspection};
use crate::api::contracts::{RasterColorType, RasterSampleFormat, WindowPreset};
use crate::types::{
    FileEntry, NativePixelDataKind, RasterMetadata, RasterUnsupported, SeriesMetadata,
    RASTER_MAX_FRAME_PIXELS,
};
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
/// A cancelled discovery is the other `Err`: `check_active` is called at
/// least once for every segment, chunk and page visited, and once more for
/// every buffer of JPEG fill bytes read inside one segment, and its error is
/// returned as it is. Only the first kind is a structural step: a check
/// between two buffers of fill spends none of the step limit below.
///
/// This function must not panic on any input and must not read pixel data.
///
/// # The scan budget
///
/// What one file may cost is fixed, whatever its size and whatever its
/// header declares. All four formats read through one reader that enforces:
///
/// - at most [`HEADER_SCAN_MAX_BYTES`] bytes obtained from the file, counting
///   every byte a read returns, read-ahead included;
/// - at most [`HEADER_SCAN_MAX_READS`] reads issued to the file;
/// - at most [`HEADER_SCAN_MAX_STEPS`] structural steps: JPEG segments, PNG
///   chunks, WebP chunks, TIFF pages.
///
/// A read that would pass a limit is not issued. A file whose header is not
/// complete when a limit is reached is
/// `Skipped(DiscoveryReason::RasterHeaderInvalid)`, and one line on stderr
/// says why in words a user can act on:
/// `dcmview: warning — <path>: <what ran out>; not loaded`, for example
/// "more than 65535 TIFF pages". There are no wall-clock limits.
///
/// How the bytes are read is part of the contract, because it is what keeps
/// a crafted or merely large file cheap:
///
/// - JPEG, PNG and WebP are scanned front to back through a buffer of
///   [`HEADER_SCAN_BUFFER_BYTES`]. A forward skip that lands inside the
///   buffer costs no read; a longer skip is a seek, not a read. The read
///   after a skip of [`HEADER_SCAN_READ_AHEAD_SKIP_BYTES`] or more returns
///   only the bytes asked for (a chunk header is eight), so walking past
///   payloads is not charged a buffer for each; a read that continues from
///   the last byte used, or nearly, reads ahead. Scanning `n` contiguous
///   bytes costs at most `n / HEADER_SCAN_BUFFER_BYTES + 2` reads.
///   The walk ends at the last thing it needs, not at the end of the file.
/// - TIFF pages are scattered through the file between pixel data, so an IFD
///   is read with reads of exactly its own length (its entry count, then its
///   entry table) and never with read-ahead, which would spend the budget on
///   pixel data.
/// - Nothing is read or allocated in proportion to a length the file
///   declares. A payload that is not needed is skipped. Of an EXIF block
///   only its first [`EXIF_SCAN_MAX_BYTES`] are read, and the orientation is
///   taken only from an IFD0 that lies within them. A PNG chunk before the
///   image data is never buffered whole: `IHDR`, the presence of `PLTE`,
///   `tRNS`, `iCCP` and `acTL`, the at most four bytes of `sBIT`, and the
///   capped `eXIf` are read from the chunk walk itself, and no limit is
///   derived from the file's length (a blank 512 x 512 mask is 334 bytes).
///   Chunk checksums are not verified.
///
/// # What is read
///
/// | Format | Read | Not read |
/// |---|---|---|
/// | PNG | chunk headers up to the first `IDAT`, and of the chunks: `IHDR`, `sBIT`, the start of `eXIf`; `PLTE`, `tRNS`, `iCCP`, `acTL` by presence | any `IDAT`; text and profile payloads |
/// | JPEG | markers up to and including the first `SOFn`: `APP1` EXIF (orientation), `APP2` ICC (presence), `APP14` Adobe | entropy-coded data; the file is never read whole |
/// | TIFF | the IFD chain, up to the page limit: each page's layout tags | strips and tiles; other tag values |
/// | WebP | the first `VP8 `/`VP8L` header, or the `VP8X` header with its alpha, ICC, EXIF and animation flags; only when that flag announces EXIF, the chunk headers up to `EXIF` and its start | image data and animation frames; any chunk after the last one needed |
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
/// - `has_pixels` is `true`; `rescale_slope` is 1, `rescale_intercept` 0.
///   `default_window` is the whole stored range of gray samples of 8 bits
///   or fewer, a TIFF's declared sample range, and `None` otherwise
///   (`default_window` below has the rule).
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
/// | anything else (TIFF) | `other` | the stored count | empty |
///
/// - A file the viewer does not decode is still listed, with
///   `raster.unsupported` set, so the catalog reports it as unsupported and
///   says why rather than dropping the file. The reasons, and the order in
///   which one is chosen when several apply, are `RasterUnsupported`'s:
///   - `Color` (TIFF): CIELab, more than four samples, or an extra sample
///     that is not alpha (`ExtraSamples` 0), reported as `color_type`
///     `other` with `has_alpha` false; palette; CMYK; gray with alpha;
///     YCbCr unless JPEG-compressed; more than one sample with
///     `PlanarConfiguration` 2.
///   - `SampleFormat` (TIFF): gray that is not 8-, 16- or 32-bit integer or
///     32- or 64-bit float; colour that is not 8- or 16-bit unsigned. The
///     `color_type`, `bit_depth` and `sample_format` are reported as stored.
///   - `Compression` (TIFF): `Compression` other than 1 (none), 5 (LZW), 8
///     or 32946 (Deflate) or 32773 (PackBits), read from page 0.
///   - `JpegProcess`: a frame header other than `SOF0`, `SOF1` or `SOF2`, or
///     a sample precision other than 8.
///   - `TooLarge`: more than `RASTER_MAX_FRAME_PIXELS` pixels in a frame.
///   - `FileLength` (PNG, JPEG, WebP): a file longer than
///     `pixels::raster_read_budget` of its entry, which no decode reads.
///
///   `RasterHeaderInvalid` is for a header that cannot be described at all:
///   no dimensions, a depth that is not 1, 2, 4, 8, 16, 32 or 64, samples of
///   mixed depth or format.
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
/// `frame_pages` and `excluded_pages` are in page order.
/// `excluded_pages_total` counts every excluded page and `excluded_pages`
/// lists the first `RASTER_EXCLUDED_PAGES_LISTED` of them; `pages_total` is
/// `frame_pages.len()` plus `excluded_pages_total`. A page whose ICC profile
/// presence differs from page 0's is still a frame and adds a warning. If
/// the chain cannot be read past some page (a bad offset, a cycle), the
/// pages read so far stand and a warning says where the walk stopped.
/// `warnings` never exceeds `RASTER_WARNINGS_MAX` entries: when more arise,
/// the last entry says how many are not shown. Each entry is also printed
/// once to stderr as it is found here, in the style of the scan budget's
/// line: `dcmview: warning — <path>: <note>`. Classic TIFF and BigTIFF, in
/// either byte order, are read alike.
///
/// A file may have at most [`HEADER_SCAN_MAX_STEPS`] pages. One with more is
/// not truncated to the limit: it is skipped as `RasterHeaderInvalid` with
/// the stderr line above, since a frame map that silently stops would shift
/// what "the last frame" means.
///
/// A page whose IFD holds any tag twice cannot be read: TIFF readers
/// disagree on which entry counts, so the layout listed here would not be
/// the one a decoder is given. Page 0 with a repeated tag makes the file
/// `RasterHeaderInvalid`; a later page ends the walk there, like any page
/// that cannot be read. Entries need not be in ascending tag order.
///
/// `raster.frame_offsets` holds the file offset of each frame's IFD, in
/// frame order, so a frame is decoded from its own page. Later pages may be
/// compressed differently from page 0; the decoder checks each page it
/// reads.
///
/// A CMYK or YCCK JPEG adds the warning that it is converted approximately
/// to sRGB and its profile not used.
pub(super) fn inspect_raster(
    _path: &Path,
    _format: FileFormat,
    _check_active: &dyn Fn() -> Result<()>,
) -> Result<EntryInspection> {
    let mut file = File::open(_path)?;
    let metadata = file.metadata()?;
    let mut inspection =
        inspect_raster_source(_path, &mut file, metadata.len(), _format, _check_active)?;
    if let EntryInspection::Selected(entry) = &mut inspection {
        // From the same `stat` as the length, on the opened file.
        entry.modified = metadata.modified().ok();
    }
    Ok(inspection)
}

/// The most bytes header inspection obtains from one file. A TIFF at the
/// page limit needs about 12 MiB as classic TIFF (a 15-entry IFD is 186
/// bytes) and about 27 MiB as BigTIFF (20 entries, 416 bytes), plus small
/// out-of-line values; the other formats need a few kilobytes. 64 MiB admits
/// all of them twice over and is tens of milliseconds of buffered reading.
pub(super) const HEADER_SCAN_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The most reads header inspection issues to one file: eight per page at
/// the page limit (a TIFF page needs two, and up to four with out-of-line
/// values). A reader that fetched one byte per read would stop after half a
/// megabyte instead of walking the file.
pub(super) const HEADER_SCAN_MAX_READS: u64 = 8 * HEADER_SCAN_MAX_STEPS as u64;

/// The most JPEG segments, PNG chunks, WebP chunks or TIFF pages visited in
/// one file. Real JPEG, PNG and WebP headers hold tens of them and real
/// multi-page TIFFs thousands; 65,535 is also the most pages a TIFF may have
/// and still be listed.
pub(super) const HEADER_SCAN_MAX_STEPS: u32 = 65_535;

/// The buffer JPEG, PNG and WebP are scanned through.
pub(super) const HEADER_SCAN_BUFFER_BYTES: usize = 8 * 1024;

/// A read after a skip shorter than this still fills the whole buffer.
/// Chunks that small sit several to a buffer, so reading ahead costs about
/// the bytes walked; past a longer payload the next chunk is as likely to
/// be skipped as read.
pub(super) const HEADER_SCAN_READ_AHEAD_SKIP_BYTES: u64 = 1024;

/// How much of an EXIF block is read for its orientation. A JPEG `APP1`
/// segment cannot be longer; IFD0 starts 8 bytes in.
pub(super) const EXIF_SCAN_MAX_BYTES: u64 = 64 * 1024;

/// What header inspection reads from: the file, or a test's counted bytes.
pub(super) trait HeaderSource: Read + Seek {}

impl<T: Read + Seek> HeaderSource for T {}

/// [`inspect_raster`] over an open source of `length` bytes positioned at
/// its start. `inspect_raster` opens the file and delegates here, so this is
/// the only path a raster header is read through, and the scan budget is
/// counted against what `source` is asked for: its `read` calls and the
/// bytes they return. `path` only names the file in the entry and in
/// messages; it is never opened here.
pub(super) fn inspect_raster_source(
    _path: &Path,
    _source: &mut dyn HeaderSource,
    _length: u64,
    _format: FileFormat,
    _check_active: &dyn Fn() -> Result<()>,
) -> Result<EntryInspection> {
    let mut input = HeaderReader::new(_source, _length, _format, _check_active);
    let parsed = match _format {
        FileFormat::Png => headers::png(&mut input),
        FileFormat::Jpeg => headers::jpeg(&mut input),
        FileFormat::Tiff => tiff::inspect(&mut input),
        FileFormat::Webp => headers::webp(&mut input),
        FileFormat::Dicom => anyhow::bail!("expected a raster format"),
    };
    let header = match parsed {
        Ok(header) if header.width > 0 && header.height > 0 => header,
        Err(error) if input.cancelled || is_io_failure(&error) => return Err(error),
        _ => {
            if let Some(what) = input.exhausted {
                eprintln!("dcmview: warning — {}: {what}; not loaded", _path.display());
            }
            return Ok(EntryInspection::Skipped(
                DiscoveryReason::RasterHeaderInvalid,
            ));
        }
    };
    let mut raster = header.metadata;
    raster.file_length = _length;
    if raster.unsupported.is_none()
        && u64::from(header.width) * u64::from(header.height) > RASTER_MAX_FRAME_PIXELS
    {
        raster.unsupported = Some(RasterUnsupported::TooLarge);
    }
    for note in &raster.warnings {
        eprintln!("dcmview: warning — {}: {note}", _path.display());
    }
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
        RasterColorType::Other => (u32::try_from(header.stored_samples).unwrap_or(u32::MAX), ""),
    };
    let mut series_metadata = SeriesMetadata::default();
    series_metadata.native_pixel.pixel_data_kind =
        Some(match (raster.sample_format, raster.bit_depth) {
            (RasterSampleFormat::Float, 32) => NativePixelDataKind::Float32,
            (RasterSampleFormat::Float, 64) => NativePixelDataKind::Float64,
            _ => NativePixelDataKind::Integer,
        });
    let file_name = _path.file_name().unwrap_or_default().to_string_lossy();
    let mut entry = FileEntry {
        index: 0,
        path: _path.to_path_buf(),
        size_bytes: _length,
        modified: None,
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
        default_window: default_window(&raster, header.sample_range),
        raster: Some(Box::new(raster)),
    };
    // The read budget is a property of the entry, so it is compared with
    // the length once the entry exists. A TIFF is read a page at a time and
    // may be any length.
    if _format != FileFormat::Tiff {
        let budget = crate::pixels::raster_read_budget(&entry);
        if let (Some(budget), Some(raster)) = (budget, entry.raster.as_mut()) {
            if raster.unsupported.is_none() && _length > budget {
                raster.unsupported = Some(RasterUnsupported::FileLength);
            }
        }
    }
    Ok(EntryInspection::Selected(Box::new(entry)))
}

/// The default window of a gray raster
/// (`docs/design/image-formats.md` section 5.3), as the linear DICOM window
/// that maps `low..=high` onto the display range: center `(low + high + 1) /
/// 2`, width `high - low + 1`.
///
/// - TIFF `MinSampleValue`/`MaxSampleValue` on unsigned samples: that range.
/// - Otherwise integer samples of 8 bits or fewer: the whole stored range,
///   `0..=2^depth - 1`, or `-128..=127` when signed. An 8-bit image is shown
///   as it is, a one-bit image black and white.
/// - Otherwise `None`: wider and floating-point samples take the percentile
///   window of each frame. PNG `sBIT` never narrows a window.
///
/// Colour frames have no window.
fn default_window(
    raster: &RasterMetadata,
    sample_range: Option<(u64, u64)>,
) -> Option<WindowPreset> {
    if !matches!(
        raster.color_type,
        RasterColorType::Gray | RasterColorType::GrayAlpha
    ) {
        return None;
    }
    let (low, high) = match (sample_range, raster.sample_format, raster.bit_depth) {
        (Some((low, high)), _, _) => (low as f64, high as f64),
        (None, RasterSampleFormat::Uint, depth @ 1..=8) => (0.0, f64::from((1_u32 << depth) - 1)),
        (None, RasterSampleFormat::Int, 8) => (-128.0, 127.0),
        _ => return None,
    };
    Some(WindowPreset {
        center: (low + high + 1.0) / 2.0,
        width: high - low + 1.0,
    })
}

struct Header {
    width: u32,
    height: u32,
    white_is_zero: bool,
    // TIFF retains its channel count even when its layout has no decoder.
    stored_samples: u64,
    /// TIFF `MinSampleValue..=MaxSampleValue` of unsigned gray samples.
    sample_range: Option<(u64, u64)>,
    metadata: RasterMetadata,
}

impl Header {
    fn new(width: u32, height: u32, color_type: RasterColorType, bit_depth: u32) -> Self {
        Self {
            width,
            height,
            white_is_zero: false,
            stored_samples: 1,
            sample_range: None,
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
                file_length: 0,
                frame_offsets: Vec::new(),
                excluded_pages: Vec::new(),
                excluded_pages_total: 0,
                unsupported: None,
                animated: false,
                significant_bits: None,
                warnings: Vec::new(),
            },
        }
    }
}

// All source reads pass here, including read-ahead. Sequential formats keep
// skipped bytes already in the buffer; TIFF reads only the requested ranges.
struct HeaderReader<'a> {
    source: &'a mut dyn HeaderSource,
    len: u64,
    buffer: Vec<u8>,
    buffer_start: u64,
    buffer_len: usize,
    // One past the last byte handed to a format walk.
    used_to: u64,
    bytes_read: u64,
    reads: u64,
    steps: u32,
    format: FileFormat,
    check_active: &'a dyn Fn() -> Result<()>,
    cancelled: bool,
    exhausted: Option<&'static str>,
}

impl<'a> HeaderReader<'a> {
    fn new(
        source: &'a mut dyn HeaderSource,
        len: u64,
        format: FileFormat,
        check_active: &'a dyn Fn() -> Result<()>,
    ) -> Self {
        Self {
            source,
            len,
            buffer: if format == FileFormat::Tiff {
                Vec::new()
            } else {
                vec![0; HEADER_SCAN_BUFFER_BYTES]
            },
            buffer_start: 0,
            buffer_len: 0,
            used_to: 0,
            bytes_read: 0,
            reads: 0,
            steps: 0,
            format,
            check_active,
            cancelled: false,
            exhausted: None,
        }
    }

    fn active(&mut self) -> Result<()> {
        (self.check_active)().inspect_err(|_| self.cancelled = true)
    }

    fn step(&mut self) -> Result<()> {
        self.active()?;
        if self.steps == HEADER_SCAN_MAX_STEPS {
            let what = match self.format {
                FileFormat::Tiff => "more than 65535 TIFF pages",
                FileFormat::Jpeg => "more than 65535 JPEG segments before the image",
                FileFormat::Png => "more than 65535 PNG chunks before the image",
                FileFormat::Webp => "more than 65535 WebP chunks",
                FileFormat::Dicom => unreachable!("raster inspection only"),
            };
            self.exhausted = Some(what);
            anyhow::bail!(what);
        }
        self.steps += 1;
        Ok(())
    }

    fn check_read(&mut self, len: u64) -> Result<()> {
        if len > HEADER_SCAN_MAX_BYTES - self.bytes_read || self.reads == HEADER_SCAN_MAX_READS {
            let what = "the image header is larger than 64 MiB";
            self.exhausted = Some(what);
            anyhow::bail!(what);
        }
        Ok(())
    }

    fn read_source(&mut self, bytes: &mut [u8]) -> Result<usize> {
        self.check_read(bytes.len() as u64)?;
        self.reads += 1;
        let read = self.source.read(bytes)?;
        self.bytes_read += read as u64;
        anyhow::ensure!(read > 0, "truncated raster header");
        Ok(read)
    }

    fn check_range(&self, offset: u64, len: u64) -> Result<()> {
        anyhow::ensure!(
            offset <= self.len && len <= self.len - offset,
            "truncated raster header"
        );
        Ok(())
    }

    fn read_into(&mut self, mut offset: u64, mut bytes: &mut [u8]) -> Result<()> {
        self.check_range(offset, bytes.len() as u64)?;
        if self.buffer.is_empty() {
            self.source.seek(SeekFrom::Start(offset))?;
            while !bytes.is_empty() {
                let read = self.read_source(bytes)?;
                bytes = &mut bytes[read..];
            }
        } else {
            while !bytes.is_empty() {
                if !self.buffered(offset) {
                    self.refill(offset, bytes.len())?;
                }
                let start = (offset - self.buffer_start) as usize;
                let count = bytes.len().min(self.buffer_len - start);
                bytes[..count].copy_from_slice(&self.buffer[start..start + count]);
                bytes = &mut bytes[count..];
                offset += count as u64;
                self.used_to = offset;
            }
        }
        Ok(())
    }

    fn buffered(&self, offset: u64) -> bool {
        offset >= self.buffer_start && offset - self.buffer_start < self.buffer_len as u64
    }

    /// Buffers from `offset`: a whole buffer when that is where the walk
    /// already was, or nearly, and only the `wanted` bytes after a longer
    /// skip, where what follows is most often the next payload to skip.
    fn refill(&mut self, offset: u64, wanted: usize) -> Result<()> {
        let continues =
            offset >= self.used_to && offset - self.used_to < HEADER_SCAN_READ_AHEAD_SKIP_BYTES;
        self.source.seek(SeekFrom::Start(offset))?;
        let mut buffer = std::mem::take(&mut self.buffer);
        let limit = if continues {
            buffer.len()
        } else {
            wanted.min(buffer.len())
        };
        let read = self.read_source(&mut buffer[..limit]);
        self.buffer = buffer;
        // A failed read leaves nothing buffered rather than stale bytes.
        self.buffer_len = 0;
        self.buffer_start = offset;
        self.buffer_len = read?;
        Ok(())
    }

    /// The offset of the first byte at or after `offset` that is not `byte`,
    /// found a buffer at a time. A long run is the one place a sequential
    /// walk stays inside a single step, so cancellation is checked at every
    /// refill.
    fn skip_run(&mut self, mut offset: u64, byte: u8) -> Result<u64> {
        loop {
            self.check_range(offset, 1)?;
            if !self.buffered(offset) {
                self.active()?;
                self.refill(offset, self.buffer.len())?;
            }
            let run = &self.buffer[(offset - self.buffer_start) as usize..self.buffer_len];
            match run.iter().position(|found| *found != byte) {
                Some(index) => {
                    self.used_to = offset + index as u64;
                    return Ok(self.used_to);
                }
                None => offset += run.len() as u64,
            }
            self.used_to = offset;
        }
    }

    fn read<const N: usize>(&mut self, offset: u64) -> Result<[u8; N]> {
        let mut bytes = [0; N];
        self.read_into(offset, &mut bytes)?;
        Ok(bytes)
    }

    fn bytes(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.check_range(offset, len)?;
        // Bound even a TIFF's declared table size before allocating it.
        if self.buffer.is_empty() {
            self.check_read(len)?;
        }
        let mut bytes = vec![0; usize::try_from(len)?];
        self.read_into(offset, &mut bytes)?;
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

#[cfg(test)]
mod tests {
    //! What one raster header may cost, counted at the source: the reads
    //! issued and the bytes they return. Nothing here measures time.

    use super::{
        inspect_raster_source, EntryInspection, EXIF_SCAN_MAX_BYTES, HEADER_SCAN_BUFFER_BYTES,
        HEADER_SCAN_MAX_BYTES, HEADER_SCAN_MAX_READS, HEADER_SCAN_MAX_STEPS,
    };
    use crate::loader::DiscoveryReason;
    use crate::types::{FileEntry, FileFormat};
    use std::cell::Cell;
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use std::path::Path;

    const BUFFER: u64 = HEADER_SCAN_BUFFER_BYTES as u64;
    const STEPS: u64 = HEADER_SCAN_MAX_STEPS as u64;

    /// A file in memory that counts what is read from it.
    struct Counted<'a> {
        bytes: Cursor<Vec<u8>>,
        reads: &'a Cell<u64>,
        read: &'a Cell<u64>,
    }

    impl Read for Counted<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.bytes.read(buffer)?;
            self.reads.set(self.reads.get() + 1);
            self.read.set(self.read.get() + count as u64);
            Ok(count)
        }
    }

    impl Seek for Counted<'_> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.bytes.seek(position)
        }
    }

    /// A classic little-endian TIFF of `pages` 2x2 one-bit pages, each IFD
    /// (42 bytes) followed by `gap` bytes standing for its pixel data.
    fn tiff_chain(pages: usize, gap: usize) -> Vec<u8> {
        const IFD: usize = 2 + 3 * 12 + 4;
        let mut out = b"II\x2a\0\x08\0\0\0".to_vec();
        for page in 0..pages {
            out.extend_from_slice(&3_u16.to_le_bytes());
            for (tag, value) in [(256_u16, 2_u16), (257, 2), (262, 1)] {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&3_u16.to_le_bytes());
                out.extend_from_slice(&1_u32.to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            }
            let next = if page + 1 == pages {
                0
            } else {
                out.len() + 4 + gap
            };
            out.extend_from_slice(&(next as u32).to_le_bytes());
            out.resize(out.len() + gap, 0);
            debug_assert_eq!(out.len(), 8 + (page + 1) * (IFD + gap));
        }
        out
    }

    fn chunk(name: &[u8; 4], payload: &[u8], big_endian: bool) -> Vec<u8> {
        let length = payload.len() as u32;
        let mut out = Vec::new();
        if big_endian {
            // PNG: length, type, data, CRC (not verified by the reader).
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(payload);
            out.extend_from_slice(&[0; 4]);
        } else {
            // RIFF: type, length, data, padded to an even length.
            out.extend_from_slice(name);
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(payload);
            out.resize(out.len() + payload.len() % 2, 0);
        }
        out
    }

    fn png_start(width: u32, height: u32) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        // 8-bit grayscale, deflate, adaptive filtering, not interlaced.
        header.extend_from_slice(&[8, 0, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &header, true));
        out
    }

    fn webp(chunks: Vec<u8>) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(chunks.len() as u32 + 4).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend(chunks);
        out
    }

    /// An EXIF block of `length` bytes whose IFD0, at offset 8, holds the
    /// orientation.
    fn exif(orientation: u16, length: usize) -> Vec<u8> {
        let mut out = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        out.extend_from_slice(&orientation.to_le_bytes());
        out.extend_from_slice(&[0; 6]);
        out.resize(length, 0);
        out
    }

    enum Outcome {
        /// Skipped as `raster_header_invalid`.
        Invalid,
        /// Listed, with this holding of the entry.
        Listed(fn(&FileEntry) -> bool),
    }

    struct Case {
        name: &'static str,
        format: FileFormat,
        bytes: Vec<u8>,
        outcome: Outcome,
        /// The most bytes and reads the source may be asked for.
        most: (u64, u64),
    }

    fn cases() -> Vec<Case> {
        const MIB: usize = 1024 * 1024;
        let sequential = |consumed: u64| (consumed + 2 * BUFFER, consumed / BUFFER + 8);

        let mut fill = vec![0xff, 0xd8];
        fill.resize(2 * MIB, 0xff);
        let fill_length = fill.len() as u64;

        let mut segments = vec![0xff, 0xd8];
        for _ in 0..4 * STEPS {
            segments.extend_from_slice(&[0xff, 0xe0, 0x00, 0x02]);
        }

        let mut empty_png_chunks = png_start(1, 1);
        for _ in 0..2 * STEPS {
            empty_png_chunks.extend(chunk(b"tEXt", &[], true));
        }

        let mut empty_webp_chunks = Vec::new();
        for _ in 0..3 * STEPS {
            empty_webp_chunks.extend(chunk(b"JUNK", &[], false));
        }

        let mut text_png = png_start(4, 4);
        text_png.extend(chunk(b"tEXt", &vec![b'a'; 4 * MIB], true));
        text_png.extend(chunk(b"eXIf", &exif(8, 2 * MIB), true));
        text_png.extend(chunk(b"IDAT", &[0; 16], true));

        // VP8X announcing EXIF, a 3x2 lossless image header, then the block.
        let mut exif_webp = chunk(b"VP8X", &[0x08, 0, 0, 0, 2, 0, 0, 1, 0, 0], false);
        exif_webp.extend(chunk(b"VP8L", &[0x2f, 0x02, 0x40, 0, 0], false));
        exif_webp.extend(chunk(b"EXIF", &exif(6, 4 * MIB), false));

        // 90 MB of animation frames, each far longer than the read buffer.
        const FRAMES: u64 = 9_000;
        let animation = |flags: u8, exif_block: &[u8]| {
            let mut chunks = chunk(b"VP8X", &[flags, 0, 0, 0, 63, 0, 0, 47, 0, 0], false);
            chunks.extend(chunk(b"ANIM", &[0; 6], false));
            let frame = chunk(b"ANMF", &[0; 10_000], false);
            for _ in 0..FRAMES {
                chunks.extend_from_slice(&frame);
            }
            if !exif_block.is_empty() {
                chunks.extend(chunk(b"EXIF", exif_block, false));
            }
            webp(chunks)
        };

        // Every reserved bit set around an alpha flag, then a 3x2 image.
        let mut reserved_webp = chunk(b"VP8X", &[0xd1, 0xff, 0xff, 0xff, 2, 0, 0, 1, 0, 0], false);
        reserved_webp.extend(chunk(b"VP8L", &[0x2f, 0x02, 0x40, 0, 0x10], false));

        vec![
            // Fill bytes before a marker are legal, in any number.
            Case {
                name: "JPEG of fill bytes",
                format: FileFormat::Jpeg,
                bytes: fill,
                outcome: Outcome::Invalid,
                // One read per buffer of fill and nothing per byte.
                most: (fill_length, fill_length / BUFFER),
            },
            Case {
                name: "JPEG of empty segments",
                format: FileFormat::Jpeg,
                bytes: segments,
                outcome: Outcome::Invalid,
                most: sequential(4 * (STEPS + 1) + 2),
            },
            Case {
                name: "PNG of empty chunks",
                format: FileFormat::Png,
                bytes: empty_png_chunks,
                outcome: Outcome::Invalid,
                most: sequential(12 * (STEPS + 1) + 33),
            },
            Case {
                name: "WebP of empty chunks",
                format: FileFormat::Webp,
                bytes: webp(empty_webp_chunks),
                outcome: Outcome::Invalid,
                most: sequential(8 * (STEPS + 1) + 12),
            },
            // The walk stops at the page limit, not at the end of the chain.
            Case {
                name: "TIFF of three times the page limit",
                format: FileFormat::Tiff,
                bytes: tiff_chain(3 * STEPS as usize, 0),
                outcome: Outcome::Invalid,
                most: (42 * (STEPS + 1) + 8 + 2 * BUFFER, HEADER_SCAN_MAX_READS),
            },
            // Pages between pixel data cost their own bytes, not the gaps.
            Case {
                name: "TIFF of scattered pages",
                format: FileFormat::Tiff,
                bytes: tiff_chain(2_000, 16 * 1024),
                outcome: Outcome::Listed(|entry| entry.frame_count == 2_000),
                most: (2_000 * 128, 2_000 * 4 + 8),
            },
            // Text is skipped and of EXIF only the start is read.
            Case {
                name: "PNG with megabytes of text and EXIF before its image data",
                format: FileFormat::Png,
                bytes: text_png,
                outcome: Outcome::Listed(|entry| {
                    (entry.rows, entry.columns) == (4, 4)
                        && entry
                            .raster
                            .as_ref()
                            .is_some_and(|raster| raster.orientation == 8)
                }),
                most: (
                    EXIF_SCAN_MAX_BYTES + 6 * BUFFER,
                    EXIF_SCAN_MAX_BYTES / BUFFER + 16,
                ),
            },
            Case {
                name: "WebP with megabytes of EXIF",
                format: FileFormat::Webp,
                bytes: webp(exif_webp),
                outcome: Outcome::Listed(|entry| {
                    (entry.rows, entry.columns) == (2, 3)
                        && entry
                            .raster
                            .as_ref()
                            .is_some_and(|raster| raster.orientation == 6)
                }),
                most: (
                    EXIF_SCAN_MAX_BYTES + 6 * BUFFER,
                    EXIF_SCAN_MAX_BYTES / BUFFER + 16,
                ),
            },
            // Readers ignore reserved bits; a later writer may set them.
            Case {
                name: "WebP with reserved VP8X bits set",
                format: FileFormat::Webp,
                bytes: webp(reserved_webp),
                outcome: Outcome::Listed(|entry| {
                    (entry.rows, entry.columns) == (2, 3)
                        && entry.raster.as_ref().is_some_and(|raster| {
                            raster.has_alpha && !raster.animated && !raster.has_icc
                        })
                }),
                most: (BUFFER, 1),
            },
            // Everything but the orientation is in the first chunk.
            Case {
                name: "WebP animation of 90 MB",
                format: FileFormat::Webp,
                bytes: animation(0x02, &[]),
                outcome: Outcome::Listed(|entry| {
                    (entry.rows, entry.columns) == (48, 64)
                        && entry.raster.as_ref().is_some_and(|raster| raster.animated)
                }),
                most: (BUFFER, 1),
            },
            // The orientation follows the frames: their headers are read,
            // eight bytes each, and nothing of their payloads.
            Case {
                name: "WebP animation of 90 MB with EXIF after its frames",
                format: FileFormat::Webp,
                bytes: animation(0x0a, &exif(6, 64)),
                outcome: Outcome::Listed(|entry| {
                    entry
                        .raster
                        .as_ref()
                        .is_some_and(|raster| raster.animated && raster.orientation == 6)
                }),
                most: (8 * FRAMES + 3 * BUFFER, FRAMES + 4),
            },
        ]
    }

    #[test]
    fn a_header_costs_a_bounded_number_of_reads_and_bytes_whatever_the_file_declares() {
        for case in cases() {
            let name = case.name;
            let (reads, read) = (Cell::new(0), Cell::new(0));
            let length = case.bytes.len() as u64;
            let mut source = Counted {
                bytes: Cursor::new(case.bytes),
                reads: &reads,
                read: &read,
            };

            let inspected = inspect_raster_source(
                Path::new("crafted"),
                &mut source,
                length,
                case.format,
                &|| Ok(()),
            )
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));

            match (inspected, case.outcome) {
                (EntryInspection::Skipped(reason), Outcome::Invalid) => {
                    assert_eq!(reason, DiscoveryReason::RasterHeaderInvalid, "{name}");
                }
                (EntryInspection::Selected(entry), Outcome::Listed(holds)) => {
                    assert!(holds(&entry), "{name}: {entry:?}");
                }
                (EntryInspection::Selected(_), Outcome::Invalid) => {
                    panic!("{name} was listed")
                }
                (EntryInspection::Skipped(reason), Outcome::Listed(_)) => {
                    panic!("{name} was skipped: {}", reason.code())
                }
            }
            let (most_bytes, most_reads) = case.most;
            assert!(
                read.get() <= most_bytes.min(HEADER_SCAN_MAX_BYTES),
                "{name}: {} bytes read of {length}, at most {most_bytes} expected",
                read.get()
            );
            assert!(
                reads.get() <= most_reads.min(HEADER_SCAN_MAX_READS),
                "{name}: {} reads, at most {most_reads} expected",
                reads.get()
            );
        }
    }

    #[test]
    fn a_cancelled_discovery_stops_a_header_walk_at_its_next_check() {
        let mut segments = vec![0xff, 0xd8];
        for _ in 0..STEPS {
            segments.extend_from_slice(&[0xff, 0xe0, 0x00, 0x02]);
        }
        // One segment whose marker never comes: megabytes of fill.
        let mut fill = vec![0xff, 0xd8];
        fill.resize(4 * 1024 * 1024, 0xff);

        // Each walk is cancelled at its tenth check, with the most bytes it
        // may have read by then.
        let cases = [
            ("JPEG of empty segments", segments, 2 * BUFFER),
            ("JPEG of fill bytes", fill, 10 * BUFFER),
        ];
        for (name, bytes, most_bytes) in cases {
            let (reads, read, checks) = (Cell::new(0), Cell::new(0), Cell::new(0));
            let length = bytes.len() as u64;
            let mut source = Counted {
                bytes: Cursor::new(bytes),
                reads: &reads,
                read: &read,
            };

            let result = inspect_raster_source(
                Path::new("crafted"),
                &mut source,
                length,
                FileFormat::Jpeg,
                &|| {
                    checks.set(checks.get() + 1);
                    anyhow::ensure!(checks.get() < 10, "discovery cancelled");
                    Ok(())
                },
            );

            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("{name}: the walk outlived its cancellation"),
            };
            assert!(
                error.to_string().contains("discovery cancelled"),
                "{name}: {error:#}"
            );
            assert_eq!(checks.get(), 10, "{name}: the walk stops when told to");
            assert!(
                read.get() <= most_bytes,
                "{name}: {} bytes read",
                read.get()
            );
        }
    }
}
