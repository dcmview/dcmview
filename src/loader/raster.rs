//! Header-only inspection of raster image files into a `FileEntry`
//! (`docs/design/image-formats.md` sections 2.4, 4 and 5.1).
//!
//! Discovery never decodes pixel data and never reads a file to its end to
//! describe it: each format is read only as far as the table in section 4
//! says. Decoding belongs to `pixels/` and is not part of this module.

use super::entry::EntryInspection;
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
    todo!("FMT1: read the raster header into a FileEntry")
}
