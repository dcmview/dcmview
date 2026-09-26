//! Colorwash overlays: real-world values already resampled onto a displayed
//! frame (see `plane_stack`), colorized with a fixed perceptual colormap.

use crate::api::contracts::RawFrameMetadata;
use crate::types::NativePixelDataKind;
use bytes::Bytes;
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};

use super::native::decode_numeric_samples;
use super::{PixelError, PixelResult};

pub const COLORMAP_NAME: &str = "viridis";

/// Viridis at nine evenly spaced positions; colors between stops are
/// interpolated linearly in RGB.
pub const COLORMAP_STOPS: [[u8; 3]; 9] = [
    [68, 1, 84],
    [71, 44, 122],
    [59, 81, 139],
    [44, 113, 142],
    [33, 144, 141],
    [39, 173, 129],
    [92, 200, 99],
    [170, 220, 50],
    [253, 231, 37],
];

/// The value range the colormap spans. Values at or below
/// `transparent_at_or_below` and non-finite values (outside the volume or
/// without a real-world mapping) are transparent; every other pixel is
/// opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorScale {
    pub min: f64,
    pub max: f64,
    pub transparent_at_or_below: Option<f64>,
}

pub struct ColorwashRequest<'a> {
    /// Real-world values of the displayed frame in row-major order.
    pub values: &'a [f64],
    pub target_rows: u32,
    pub target_columns: u32,
    pub scale: ColorScale,
}

/// The stored samples of one single-sample raw frame as numbers.
pub fn raw_frame_values(
    body: &[u8],
    metadata: &RawFrameMetadata,
    kind: NativePixelDataKind,
) -> PixelResult<Vec<f64>> {
    if metadata.samples_per_pixel != 1 {
        return Err(PixelError::UnsupportedLayout(
            "value overlays require one sample per pixel".to_string(),
        ));
    }
    let values = decode_numeric_samples(
        body,
        metadata.bits_allocated,
        metadata.pixel_representation == 1,
        false,
        kind,
    )
    .map_err(PixelError::raw_decode)?;
    let expected = u64::from(metadata.rows) * u64::from(metadata.columns);
    if values.len() as u64 != expected {
        return Err(PixelError::UnsupportedLayout(format!(
            "raw frame has {} samples for {expected} pixels",
            values.len()
        )));
    }
    Ok(values)
}

/// Color of a position in `0..=1` along the colormap.
pub fn colormap(position: f64) -> [u8; 3] {
    let last = COLORMAP_STOPS.len() - 1;
    let scaled = if position.is_finite() {
        position.clamp(0.0, 1.0) * last as f64
    } else {
        0.0
    };
    let low = (scaled.floor() as usize).min(last);
    let high = (low + 1).min(last);
    let fraction = scaled - low as f64;
    std::array::from_fn(|channel| {
        let from = f64::from(COLORMAP_STOPS[low][channel]);
        let to = f64::from(COLORMAP_STOPS[high][channel]);
        (from + (to - from) * fraction).round() as u8
    })
}

pub fn encode_colorwash_png(request: ColorwashRequest<'_>) -> PixelResult<Bytes> {
    let output_len = usize::try_from(
        u64::from(request.target_rows)
            .checked_mul(u64::from(request.target_columns))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                PixelError::UnsupportedLayout("overlay dimensions overflow".to_string())
            })?,
    )
    .map_err(|_| PixelError::UnsupportedLayout("overlay dimensions overflow".to_string()))?;
    if request.values.len() * 4 != output_len {
        return Err(PixelError::UnsupportedLayout(
            "overlay values do not match the displayed frame".to_string(),
        ));
    }
    let mut rgba = vec![0_u8; output_len];
    let ColorScale {
        min,
        max,
        transparent_at_or_below,
    } = request.scale;
    let span = max - min;

    for (value, pixel) in request.values.iter().zip(rgba.chunks_exact_mut(4)) {
        if !value.is_finite() || transparent_at_or_below.is_some_and(|floor| *value <= floor) {
            continue;
        }
        let position = if span > 0.0 {
            (value - min) / span
        } else {
            0.0
        };
        pixel[..3].copy_from_slice(&colormap(position));
        pixel[3] = 255;
    }

    let mut encoded = Vec::new();
    PngEncoder::new(&mut encoded)
        .write_image(
            &rgba,
            request.target_columns,
            request.target_rows,
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| PixelError::frame_decode(error.into()))?;
    Ok(Bytes::from(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colormap_interpolates_between_stops() {
        assert_eq!(colormap(0.0), COLORMAP_STOPS[0]);
        assert_eq!(colormap(1.0), COLORMAP_STOPS[8]);
        assert_eq!(colormap(0.0625), [70, 23, 103]);
        assert_eq!(colormap(f64::NAN), COLORMAP_STOPS[0]);
    }

    #[test]
    fn colors_values_and_leaves_the_floor_and_missing_values_transparent() {
        let png = encode_colorwash_png(ColorwashRequest {
            values: &[0.0, 4.0, 8.0, f64::NAN, 12.0, -1.0],
            target_rows: 2,
            target_columns: 3,
            scale: ColorScale {
                min: 0.0,
                max: 8.0,
                transparent_at_or_below: Some(0.0),
            },
        })
        .expect("colorwash PNG");
        let image = image::load_from_memory(&png).expect("decode").to_rgba8();

        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        let [r, g, b] = colormap(0.5);
        assert_eq!(image.get_pixel(1, 0).0, [r, g, b, 255]);
        let [r, g, b] = colormap(1.0);
        assert_eq!(image.get_pixel(2, 0).0, [r, g, b, 255]);
        // Values beyond the scale clamp to its top color.
        assert_eq!(image.get_pixel(1, 1).0, [r, g, b, 255]);
        assert_eq!(image.get_pixel(0, 1).0[3], 0);
        assert_eq!(image.get_pixel(2, 1).0[3], 0);
    }

    #[test]
    fn rejects_values_that_do_not_match_the_frame() {
        assert!(encode_colorwash_png(ColorwashRequest {
            values: &[1.0; 5],
            target_rows: 2,
            target_columns: 3,
            scale: ColorScale {
                min: 0.0,
                max: 1.0,
                transparent_at_or_below: None,
            },
        })
        .is_err());
    }
}
