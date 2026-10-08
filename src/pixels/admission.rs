//! What a decode reserves of the decode memory budget before it starts
//! (`docs/design/image-formats.md` section 2.3, "Byte-based decode
//! admission").
//!
//! The pixel service starts no decode, render or frame-sized copy without a
//! [`DecodePermit`] from [`admit`], which reserves [`decode_estimate`] bytes
//! with the scheduler for as long as the work runs. The estimate is computed
//! from the catalog entry alone, before the file is opened: a file cannot
//! lower its own estimate by what it declares, because the decoders check
//! what they find against the same entry before they size anything from it
//! (`raster::decode_raster_frame`, "The entry is checked against the file").

use super::error::{PixelError, PixelResult};
use super::schedule::{DecodeClass, DecodeLimits, DecodePermit, DecodeRefusal, DecodeScheduler};
use crate::types::FileEntry;
use std::sync::Arc;

/// The part of a DICOM decode's estimate that does not depend on the frame:
/// the parsed data set without its pixel data, decoder state and tables.
pub const DICOM_DECODE_BASE_BYTES: u64 = 16 * 1024 * 1024;

/// The part of a display frame's estimate that does not depend on the
/// frame: the window's lookup table and histogram and the encoder's state.
pub const DISPLAY_BASE_BYTES: u64 = 1024 * 1024;

/// The part of a thumbnail's estimate that does not depend on the frame: the
/// resampled image (at most 1024 x 1024 RGB), its JPEG and the encoder.
pub const THUMBNAIL_BASE_BYTES: u64 = 8 * 1024 * 1024;

/// One piece of work the pixel service does under a permit. Each is admitted
/// once, for all of its steps: no step waits for a second permit while the
/// first is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecodeWork {
    /// Decoding one frame to its raw samples (the raw tier).
    RawFrame,
    /// One display frame from a cold cache: the decode (through the raw tier
    /// or the codec's display decoder), the window, the shutter and overlay
    /// planes, the PNG, and the redaction boxes painted on it. A preview is
    /// the same work.
    DisplayFrame,
    /// One thumbnail: the decode and render, the redaction boxes, the
    /// resampling and the JPEG.
    Thumbnail,
    /// One presentation layer: an RGBA image of the frame's size and its PNG.
    PresentationLayer,
    /// The copy of a raw frame in which redaction boxes are filled for one
    /// response.
    RawRedaction,
}

/// The bytes `work` on one frame of `file` reserves, from the catalog entry
/// alone. Nothing is read: two calls for one entry give one answer, whatever
/// has happened to the file.
///
/// With, from the entry,
///
/// ```text
/// P = rows * columns                                   pixels
/// S = P * samples_per_pixel                            samples
/// B = max(1, ceil(bits_allocated / 8))                 bytes a raw sample is served in
/// F = S * B                                            the raw frame
/// D = P       when samples_per_pixel < 3               the display buffer
///     3 * P   when samples_per_pixel >= 3 and B == 1
///     6 * P   when samples_per_pixel >= 3 and B >= 2
/// V = 32 * S  when B >= 4                              windowing wide samples
///     0       otherwise
/// ```
///
/// the decode of one frame is estimated as `decode`:
///
/// | Entry | `decode` |
/// |---|---|
/// | raster (`file.format.is_raster()`) | [`raster_decode_heap_limit`]`(file, raster.file_length)`: 32 MiB and one read buffer, `6 * F`, and four times the file as discovery measured it or the read budget if that is less |
/// | DICOM: native, RLE, deflated frame | [`DICOM_DECODE_BASE_BYTES`]` + 3 * F` |
/// | DICOM: JPEG Baseline, JPEG Lossless, JPEG-LS | [`DICOM_DECODE_BASE_BYTES`]` + 5 * F` |
/// | DICOM: JPEG 2000, JPEG XL | [`DICOM_DECODE_BASE_BYTES`]` + 16 * S + 2 * F` |
/// | DICOM with no codec (`codec_for_file` is `None`) | as native |
///
/// and each piece of work as:
///
/// | `work` | Bytes |
/// |---|---|
/// | [`DecodeWork::RawFrame`] | `decode` |
/// | [`DecodeWork::DisplayFrame`] | `decode + V + 3 * D + `[`DISPLAY_BASE_BYTES`] |
/// | [`DecodeWork::Thumbnail`] | `decode + V + D + `[`THUMBNAIL_BASE_BYTES`] |
/// | [`DecodeWork::PresentationLayer`] | `9 * P + `[`DISPLAY_BASE_BYTES`] |
/// | [`DecodeWork::RawRedaction`] | `F` |
///
/// Every sum saturates at `u64::MAX`. A raster for which
/// [`raster_decode_heap_limit`] is `None` (more pixels than the viewer
/// decodes) estimates `u64::MAX` for every kind of work; the service refuses
/// such a file by its `support_reason` before it asks.
///
/// # Why these numbers
///
/// - A raster's `decode` is the limit `decode_raster_frame` is held to for
///   every file, hostile ones included, so the reservation is never less
///   than what the decode holds (`tests/raster_cost` measures both).
/// - `3 * D` is the display buffer, its PNG (never larger than the buffer
///   beside a fixed overhead) and one more buffer while redaction boxes are
///   painted on the decoded PNG. The steps after the decode run while the
///   raw frame may still be held, which `decode` already counts.
/// - `V` is for samples of 32 or 64 bits, which have no lookup table: the
///   frame is converted to 64-bit values, rescaled and sorted for its
///   percentiles, each in an array of its own, three arrays of eight bytes
///   a sample, with a quarter more beside. Samples of 16 bits or fewer are
///   windowed through a table and need none of it.
/// - `9 * P` is the layer's four bytes a pixel, a PNG no larger than it, and
///   one byte a pixel for the shutter's visibility.
/// - The DICOM rows are rules, not measurements of every codec: the data
///   set's frame, the decoder's own copy and the samples it is converted
///   to. A DICOM decoder is not yet held to its entry the way the raster
///   decoder is (an encapsulated codestream states its own size), so for a
///   damaged or hostile DICOM file the reservation is the catalog's claim.
///
/// [`raster_decode_heap_limit`]: super::raster::raster_decode_heap_limit
pub fn decode_estimate(file: &FileEntry, work: DecodeWork) -> u64 {
    let _ = (file, work);
    todo!("FMT4: estimate the bytes a piece of decode work reserves")
}

/// A permit of `class` for `work` on one frame of `file`, reserving
/// [`decode_estimate`] bytes. A scheduler without limits reserves nothing
/// and the estimate is not computed.
///
/// Errors are the scheduler's refusals as the API reports them:
/// [`PixelError::DecodeMemoryExceeded`] and [`PixelError::DecodeBusy`].
pub(super) async fn admit(
    scheduler: &Arc<DecodeScheduler>,
    class: DecodeClass,
    file: &FileEntry,
    work: DecodeWork,
) -> PixelResult<DecodePermit> {
    let bytes = if scheduler.limits() == DecodeLimits::UNLIMITED {
        0
    } else {
        decode_estimate(file, work)
    };
    scheduler
        .admit(class, bytes)
        .await
        .map_err(|refusal| match refusal {
            DecodeRefusal::TooLarge {
                needed_bytes,
                limit_bytes,
                class,
            } => PixelError::DecodeMemoryExceeded {
                needed_bytes,
                limit_bytes,
                background: class == DecodeClass::Background,
            },
            DecodeRefusal::Busy => PixelError::DecodeBusy,
        })
}
