//! Reduced-resolution decodes for thumbnails
//! (`docs/design/gallery-views.md` section 3.3).
//!
//! A thumbnail is at most 1,024 pixels along its longest edge, and two codecs
//! can decode a frame at a fraction of its size for a fraction of the work:
//! JPEG 2000 stops at a lower resolution level of its wavelet transform, and
//! the JPEG DCT processes invert each block at 1/2, 1/4 or 1/8 of its size.
//! A thumbnail of such a frame is decoded at the smallest of those sizes that
//! is still at least twice the thumbnail along both axes, and then resampled
//! like any other, so every pixel of the thumbnail is still an average of
//! four decoded pixels or more.
//!
//! The rules, each stated where it is implemented:
//!
//! - **How far.** [`thumbnail_reduction`] gives the reduction a thumbnail
//!   asks for, from the catalog entry and the request alone. Nothing a frame
//!   declares raises it.
//! - **Same presentation.** A reduction is asked for only when the frame's
//!   presentation does not depend on its samples as a whole (a stored window
//!   or VOI LUT, or colour), so a thumbnail has the same window whichever
//!   path decoded it. A frame windowed from its own histogram or its own
//!   extremes is decoded in full.
//! - **Checked first.** A reduced decode is handed the frame only after
//!   `codestream::checked` has accepted it for the entry, exactly as a full
//!   decode is (`jpeg2000::render_jp2_thumbnail`,
//!   `pixeldata_frame::decode_frame_for_thumbnail`).
//! - **Held to the entry.** What a reduced decode returns must be the
//!   entry's image at the reduction that was applied
//!   ([`Reduction::accepts`]); anything else is discarded, and the frame is
//!   decoded in full.
//! - **Never worse than a full decode.** A frame that cannot be reduced as
//!   asked (its codestream has fewer resolution levels, its JPEG process is
//!   lossless, the decoder refuses) is reduced less or decoded in full under
//!   the same permit, after the reduced attempt has given back what it held.
//!   No frame that has a thumbnail without this module lacks one with it.
//! - **Redaction.** A frame with redaction boxes is decoded in full, so the
//!   boxes are painted on stored pixels before anything is resampled and no
//!   redacted pixel reaches the thumbnail through a decoder's filter.
//!
//! The reservation does not change: [`DecodeWork::Thumbnail`] reserves what a
//! full decode holds whichever path is taken, because whether a frame can be
//! reduced is known only once its codestream has been read, and the full
//! decode is the fallback under the same permit.
//!
//! [`DecodeWork::Thumbnail`]: super::admission::DecodeWork::Thumbnail

use super::render::DisplayBuffer;
use super::syntax::{codec_for_file, Codec};
use super::thumbnail::thumbnail_dimensions;
use super::window::selected_voi_lut;
use crate::api::contracts::WindowMode;
use crate::types::FileEntry;

/// The most a JPEG frame is reduced: the decoder inverts a block at 1/2, 1/4
/// or 1/8 of its size and no smaller.
pub const JPEG_MAX_REDUCTION_SHIFT: u8 = 3;

/// The most a JPEG 2000 frame is reduced. A frame's rows and columns are
/// 32-bit numbers, so no larger shift leaves a pixel.
pub const JPEG_2000_MAX_REDUCTION_SHIFT: u8 = 31;

/// How far a decode is reduced: each axis of the frame is divided by
/// `2^shift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reduction {
    /// At least 1: a decode that is not reduced has no `Reduction`.
    pub shift: u8,
}

impl Reduction {
    /// Whether an image of `columns` by `rows` is `file`'s frame at this
    /// reduction: along each axis, the entry's extent divided by `2^shift`
    /// and rounded up, which is what both decoders return for an image that
    /// starts at the origin of its grid. A reduced decode that returns any
    /// other size is not the entry's image and is discarded.
    pub fn accepts(self, file: &FileEntry, columns: u32, rows: u32) -> bool {
        let reduced = |extent: u32| u64::from(extent).div_ceil(1_u64 << self.shift.min(63));
        self.shift >= 1
            && u64::from(columns) == reduced(file.columns)
            && u64::from(rows) == reduced(file.rows)
            && columns >= 1
            && rows >= 1
    }
}

/// One frame rendered for a thumbnail, and how it was decoded.
#[derive(Debug)]
pub(crate) struct ThumbnailRender {
    /// The frame's default presentation: in the stored grid when `shift` is
    /// 0, otherwise that grid with each axis divided by `2^shift`.
    pub(crate) buffer: DisplayBuffer,
    /// The reduction the decode applied; 0 for a full decode.
    pub(crate) shift: u8,
}

impl ThumbnailRender {
    /// A full decode.
    pub(crate) fn full(buffer: DisplayBuffer) -> Self {
        Self { buffer, shift: 0 }
    }
}

/// Whether the default presentation of `file` in `window_mode` is the same
/// function of each sample whatever the other samples of the frame are, so
/// that a frame decoded at a lower resolution is presented exactly as the
/// full frame is.
///
/// True for three samples a pixel (no window), and for MONOCHROME1 and
/// MONOCHROME2 in [`WindowMode::Default`] when the entry has a stored window
/// or a VOI LUT that the default presentation selects. False otherwise:
/// [`WindowMode::FullDynamic`] spans the frame's own extremes, and a frame
/// with neither a window nor a VOI LUT is windowed from its own histogram,
/// both of which a reduced image would move.
pub(crate) fn presentation_ignores_resolution(file: &FileEntry, window_mode: WindowMode) -> bool {
    if file.samples_per_pixel == 3 {
        return true;
    }
    let photometric = file.photometric_interpretation.trim();
    if file.samples_per_pixel != 1
        || !(photometric.eq_ignore_ascii_case("MONOCHROME1")
            || photometric.eq_ignore_ascii_case("MONOCHROME2"))
        || window_mode != WindowMode::Default
    {
        return false;
    }
    file.default_window.is_some()
        || selected_voi_lut(
            window_mode,
            None,
            None,
            file.default_window,
            file.series_metadata.native_pixel.voi_lut.as_ref(),
        )
        .is_some()
}

/// The reduction the thumbnail of `file` at `bucket` asks its decoder for,
/// or `None` for a full decode. A function of the catalog entry and the
/// request alone: two calls give one answer whatever the file holds.
///
/// `None` unless all of these hold:
///
/// 1. `codec_for_file(file)` is [`Codec::Jpeg2000`] or
///    [`Codec::JpegBaseline`]. No other codec has a reduced decode.
/// 2. [`presentation_ignores_resolution`]`(file, window_mode)`.
/// 3. Some `shift` from 1 to the codec's limit
///    ([`JPEG_2000_MAX_REDUCTION_SHIFT`], [`JPEG_MAX_REDUCTION_SHIFT`]) keeps
///    the frame at least twice as large as its thumbnail along both axes:
///    `file.columns >> shift >= 2 * width` and
///    `file.rows >> shift >= 2 * height`, where `(width, height)` is
///    [`thumbnail_dimensions`]`(file.rows, file.columns, ratio, bucket)` with
///    the entry's effective pixel aspect ratio
///    (`file.series_metadata.native_pixel.effective_pixel_aspect_ratio()`).
///    Twice, so the area filter that makes the thumbnail still averages
///    four decoded pixels or more for each of its own, as it does for a
///    frame decoded in full.
///
/// The result is the largest such `shift`. It is what is asked for: a
/// decoder applies it or less (`jpeg2000::render_jp2_thumbnail`,
/// `pixeldata_frame::decode_frame_for_thumbnail`), never more.
///
/// Never panics, for any entry: rows, columns and `bucket` may each be 0 or
/// `u32::MAX`.
pub fn thumbnail_reduction(
    file: &FileEntry,
    bucket: u32,
    window_mode: WindowMode,
) -> Option<Reduction> {
    let limit = match codec_for_file(file)? {
        Codec::Jpeg2000 => JPEG_2000_MAX_REDUCTION_SHIFT,
        Codec::JpegBaseline => JPEG_MAX_REDUCTION_SHIFT,
        _ => return None,
    };
    if !presentation_ignores_resolution(file, window_mode) {
        return None;
    }
    let thumbnail = thumbnail_dimensions(
        file.rows,
        file.columns,
        file.series_metadata
            .native_pixel
            .effective_pixel_aspect_ratio(),
        bucket,
    );
    largest_shift(file.columns, file.rows, thumbnail, limit).map(|shift| Reduction { shift })
}

/// Step 3 of [`thumbnail_reduction`]: the largest `shift` from 1 to `limit`
/// with `columns >> shift >= 2 * thumbnail.0` and `rows >> shift >= 2 *
/// thumbnail.1`, or `None` when there is none. `thumbnail` is `(width,
/// height)`, each at least 1. Never panics, whatever the numbers; `limit`
/// is at most 31.
fn largest_shift(columns: u32, rows: u32, thumbnail: (u32, u32), limit: u8) -> Option<u8> {
    (1..=limit.min(31)).rev().find(|&shift| {
        u64::from(columns >> shift) >= 2 * u64::from(thumbnail.0)
            && u64::from(rows >> shift) >= 2 * u64::from(thumbnail.1)
    })
}
