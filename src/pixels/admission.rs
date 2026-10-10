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
use crate::types::{FileEntry, OverlayEncoding};
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
    /// One presentation layer: an RGBA image of the frame's size while it
    /// is encoded as a PNG.
    PresentationLayer,
    /// The copy of a raw frame in which redaction boxes are filled for one
    /// response.
    RawRedaction,
    /// The value range of an RT Dose or Parametric Map, which its legend
    /// spans: every frame of the object, one after another, each decoded
    /// and held once more as real-world values while its smallest and
    /// largest are found. The file is the object.
    ValueLegend,
    /// One SEG frame painted on the displayed frame it segments: the SEG
    /// frame's decode, an RGBA image of the displayed frame's size and its
    /// PNG. The file is the SEG object.
    SegmentationOverlay {
        /// Rows of the displayed frame the overlay is drawn on.
        target_rows: u32,
        /// Columns of the displayed frame the overlay is drawn on.
        target_columns: u32,
    },
    /// An RT Dose or Parametric Map resampled onto one displayed frame: the
    /// decode of each plane the frame is sampled from, those planes held
    /// together as real-world values, the resampled values of the displayed
    /// frame, and what they are sent as. The file is the dose or map.
    ValueOverlay {
        /// Rows of the displayed frame the overlay is drawn on.
        target_rows: u32,
        /// Columns of the displayed frame the overlay is drawn on.
        target_columns: u32,
        /// How many of the object's frames the work decodes and holds at
        /// once. The work decodes no frame beyond this count.
        planes: u32,
        /// What the resampled values are sent as: an RGBA image and its
        /// PNG, or the values as 32-bit floats.
        encoding: OverlayEncoding,
    },
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
/// D = 3 * P   when samples_per_pixel >= 3 and B == 1    the display buffer
///     6 * P   when samples_per_pixel >= 3 and B >= 2
///     3 * P   for one sample a pixel that is PALETTE COLOR
///     P       otherwise
/// V = 32 * S  when B >= 4                              windowing wide samples
///     0       otherwise
/// ```
///
/// and, from an overlay's [`DecodeWork`],
///
/// ```text
/// T = target_rows * target_columns                     pixels of the displayed frame
/// N = planes                                           frames held at once
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
/// | [`DecodeWork::DisplayFrame`] | `V + max(decode, F + 7 * D) + `[`DISPLAY_BASE_BYTES`] |
/// | [`DecodeWork::Thumbnail`] | `decode + V + D + `[`THUMBNAIL_BASE_BYTES`] |
/// | [`DecodeWork::PresentationLayer`] | `25 * P + `[`DISPLAY_BASE_BYTES`] |
/// | [`DecodeWork::RawRedaction`] | `F` |
/// | [`DecodeWork::ValueLegend`] | `decode + 8 * P` |
/// | [`DecodeWork::SegmentationOverlay`] | `decode + 24 * T + `[`DISPLAY_BASE_BYTES`] |
/// | [`DecodeWork::ValueOverlay`] as [`OverlayEncoding::Png`] | `8 * N * P + max(decode, 32 * T + `[`DISPLAY_BASE_BYTES`]`)` |
/// | [`DecodeWork::ValueOverlay`] as [`OverlayEncoding::Values`] | `8 * N * P + max(decode, 12 * T + `[`DISPLAY_BASE_BYTES`]`)` |
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
/// - A display frame has two stages, and reserves the larger. The decode
///   is over, and what it held given back, before the frame is encoded;
///   only the raw frame (`F`) may still be held then. `7 * D` is the
///   display buffer while it is encoded: the buffer, the attempt to
///   compress it, which for an image that does not compress is up to one
///   and a half times the buffer in a buffer grown to at most twice that,
///   and the stored copy written in its place, held twice over while it
///   grows. `tests/raster_cost` measures such frames against it.
///   Redaction boxes are painted on a decoded copy of the PNG, which is
///   given up before the copy is encoded, so painting them holds no more.
/// - A PALETTE COLOR frame is one sample a pixel and is displayed as RGB,
///   so its display buffer is three bytes a pixel.
/// - `V` is for samples of 32 or 64 bits, which have no lookup table: the
///   frame is converted to 64-bit values, rescaled and sorted for its
///   percentiles, each in an array of its own, three arrays of eight bytes
///   a sample, with a quarter more beside. Samples of 16 bits or fewer are
///   windowed through a table and need none of it.
/// - `25 * P` is a presentation layer, an image of four bytes a pixel,
///   while it is encoded: a shutter that hides every other pixel makes a
///   layer that does not compress, and such a layer measures just over 24
///   bytes a pixel at the sizes where it holds the most. The byte a pixel
///   that marks what a shutter hides is given up before the layer is
///   encoded.
/// - `8 * P` is one frame as real-world values, eight bytes a pixel. A
///   legend holds one frame's values at a time, beside the raw frame they
///   were read from, which `decode` counts.
/// - `24 * T` is an overlay image while it is encoded: four bytes a pixel
///   of RGBA, the compressed stream in a buffer that has grown to at most
///   twice its length, and the PNG it is copied into, which while it grows
///   is held twice over. An image that does not compress is the bound, and
///   `tests/raster_cost` measures it.
/// - A value overlay holds its `N` planes as real-world values from the
///   first decode to the end. Beside them it holds first a decode and then,
///   with every decode over, the displayed frame's resampled values (eight
///   bytes a pixel) and either the image (`24 * T`) or the 32-bit values
///   that are sent (`4 * T`): whichever of the two stages is larger.
/// - The DICOM rows are rules, not measurements of every codec: the data
///   set's frame, the decoder's own copy and the samples it is converted
///   to. A DICOM decoder is not yet held to its entry the way the raster
///   decoder is (an encapsulated codestream states its own size), so for a
///   damaged or hostile DICOM file the reservation is the catalog's claim.
///
/// [`raster_decode_heap_limit`]: super::raster::raster_decode_heap_limit
pub fn decode_estimate(file: &FileEntry, work: DecodeWork) -> u64 {
    use super::syntax::Codec;

    let pixels = u64::from(file.rows).saturating_mul(u64::from(file.columns));
    let samples = pixels.saturating_mul(u64::from(file.samples_per_pixel));
    let bytes_per_sample = u64::from(file.bits_allocated).saturating_add(7) / 8;
    let bytes_per_sample = bytes_per_sample.max(1);
    let frame = samples.saturating_mul(bytes_per_sample);
    let palette = file.samples_per_pixel == 1
        && file
            .photometric_interpretation
            .trim()
            .eq_ignore_ascii_case("PALETTE COLOR");
    let display = pixels.saturating_mul(match (file.samples_per_pixel, bytes_per_sample) {
        (3.., 1) => 3,
        (3.., _) => 6,
        _ if palette => 3,
        _ => 1,
    });
    let wide = if bytes_per_sample >= 4 {
        samples.saturating_mul(32)
    } else {
        0
    };

    let decode = if file.format.is_raster() {
        let length = file.raster.as_ref().map_or(0, |raster| raster.file_length);
        let Some(limit) = super::raster::raster_decode_heap_limit(file, length) else {
            return u64::MAX;
        };
        limit
    } else {
        let buffers = match super::syntax::codec_for_file(file) {
            Some(Codec::JpegBaseline | Codec::JpegLossless | Codec::JpegLs) => {
                frame.saturating_mul(5)
            }
            Some(Codec::Jpeg2000 | Codec::JpegXl) => samples
                .saturating_mul(16)
                .saturating_add(frame.saturating_mul(2)),
            _ => frame.saturating_mul(3),
        };
        DICOM_DECODE_BASE_BYTES.saturating_add(buffers)
    };

    match work {
        DecodeWork::RawFrame => decode,
        DecodeWork::DisplayFrame => decode
            .max(frame.saturating_add(display.saturating_mul(7)))
            .saturating_add(wide)
            .saturating_add(DISPLAY_BASE_BYTES),
        DecodeWork::Thumbnail => decode
            .saturating_add(wide)
            .saturating_add(display)
            .saturating_add(THUMBNAIL_BASE_BYTES),
        DecodeWork::PresentationLayer => {
            pixels.saturating_mul(25).saturating_add(DISPLAY_BASE_BYTES)
        }
        DecodeWork::RawRedaction => frame,
        DecodeWork::ValueLegend => decode.saturating_add(pixels.saturating_mul(8)),
        DecodeWork::SegmentationOverlay {
            target_rows,
            target_columns,
        } => {
            let target = u64::from(target_rows).saturating_mul(u64::from(target_columns));
            decode
                .saturating_add(target.saturating_mul(24))
                .saturating_add(DISPLAY_BASE_BYTES)
        }
        DecodeWork::ValueOverlay {
            target_rows,
            target_columns,
            planes,
            encoding,
        } => {
            let target = u64::from(target_rows).saturating_mul(u64::from(target_columns));
            let encoded = target
                .saturating_mul(match encoding {
                    OverlayEncoding::Png => 32,
                    OverlayEncoding::Values => 12,
                })
                .saturating_add(DISPLAY_BASE_BYTES);
            pixels
                .saturating_mul(u64::from(planes))
                .saturating_mul(8)
                .saturating_add(decode.max(encoded))
        }
    }
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
