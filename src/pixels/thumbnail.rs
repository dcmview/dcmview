//! Gallery thumbnails: the geometry of one and its JPEG encoding. The
//! service entry point is `service::load_thumbnail`.

use super::render::DisplayBuffer;
use crate::api::contracts::{
    ThumbnailSource, WindowMode, THUMBNAIL_DEFAULT_SIZE, THUMBNAIL_SIZE_BUCKETS,
};
use anyhow::Result;
use bytes::Bytes;

/// JPEG quality of a thumbnail. Thumbnails are previews; the viewer's own
/// frames stay lossless.
pub const THUMBNAIL_JPEG_QUALITY: u8 = 85;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailRequest {
    pub frame: u32,
    /// One of [`THUMBNAIL_SIZE_BUCKETS`]; see [`thumbnail_bucket`].
    pub bucket: u32,
    /// `Default` for the frame's default presentation (DICOM window, else
    /// VOI LUT, else the automatic window), or `FullDynamic`.
    pub window_mode: WindowMode,
}

#[derive(Debug, Clone)]
pub struct ThumbnailResponse {
    /// The encoded JPEG.
    pub body: Bytes,
    /// The step that produced the image.
    pub source: ThumbnailSource,
    /// Whether the thumbnail cache held it (or a render of it was already
    /// under way).
    pub cache_hit: bool,
}

/// The bucket a requested `size` is rendered at: the smallest of
/// [`THUMBNAIL_SIZE_BUCKETS`] that is at least `size`, or
/// [`THUMBNAIL_DEFAULT_SIZE`] when no size is named. `None` for a size of
/// zero or above the largest bucket.
pub fn thumbnail_bucket(size: Option<u32>) -> Option<u32> {
    let size = size.unwrap_or(THUMBNAIL_DEFAULT_SIZE);
    if size == 0 {
        return None;
    }
    THUMBNAIL_SIZE_BUCKETS
        .into_iter()
        .find(|bucket| *bucket >= size)
}

/// The `(width, height)` in pixels of the thumbnail of a `rows` x `columns`
/// frame at `bucket`.
///
/// The frame's physical shape is `columns` wide and `rows *
/// pixel_aspect_ratio` high (`FileSummary.pixel_aspect_ratio`, the height of
/// a pixel over its width; absent or invalid means square). The thumbnail has
/// that shape, with the longest edge at most `bucket`, and never has more
/// pixels along an axis than the stored frame has, so nothing is enlarged:
///
/// ```text
/// width  = columns * min(1, 1 / ratio)
/// height = rows    * min(1, ratio)
/// scale  = min(1, bucket / max(width, height))
/// thumbnail = (max(1, round(width * scale)), max(1, round(height * scale)))
/// ```
///
/// A stored-grid position `(row, column)` is therefore at `(row *
/// thumbnail_height / rows, column * thumbnail_width / columns)`: one scale
/// per axis, no offset.
pub fn thumbnail_dimensions(
    rows: u32,
    columns: u32,
    pixel_aspect_ratio: Option<f64>,
    bucket: u32,
) -> (u32, u32) {
    let ratio = pixel_aspect_ratio
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .unwrap_or(1.0);
    let width = f64::from(columns) * (1.0 / ratio).min(1.0);
    let height = f64::from(rows) * ratio.min(1.0);
    let scale = (f64::from(bucket) / width.max(height)).min(1.0);
    let fit = |extent: f64| ((extent * scale).round() as u32).clamp(1, bucket.max(1));
    (fit(width), fit(height))
}

/// Resamples `buffer` to [`thumbnail_dimensions`] and encodes it as a
/// baseline JPEG of quality [`THUMBNAIL_JPEG_QUALITY`]: grayscale for
/// `Gray8`, RGB otherwise, without an ICC profile.
///
/// The buffer is shrunk in display space with an area (box) filter,
/// `image::imageops::thumbnail`, so every stored pixel under a thumbnail
/// pixel contributes to it and no other does. `Rgb16` samples are first
/// reduced to 8 bits the way `image::DynamicImage::to_rgb8` reduces them. A
/// buffer that already has the thumbnail's dimensions is encoded as it is.
///
/// Whatever must not be shown (redaction boxes) is painted on the buffer
/// before this call, so a redacted pixel never contributes to a thumbnail
/// pixel.
pub(crate) fn encode_thumbnail_jpeg(
    buffer: DisplayBuffer,
    bucket: u32,
    pixel_aspect_ratio: Option<f64>,
) -> Result<Bytes> {
    let _ = (buffer, bucket, pixel_aspect_ratio);
    todo!("GAL1: resample the buffer and encode the thumbnail JPEG")
}
