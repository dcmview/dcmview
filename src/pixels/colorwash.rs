//! Colorwash overlays: real-world values resampled onto a displayed frame
//! and colorized with a fixed perceptual colormap.

use crate::api::contracts::RawFrameMetadata;
use crate::geometry::PixelAffineTransform;
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
/// `transparent_at_or_below`, values outside the planes' grid, and values
/// without a real-world mapping are transparent; every other pixel is opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorScale {
    pub min: f64,
    pub max: f64,
    pub transparent_at_or_below: Option<f64>,
}

/// One stack plane's real-world values in row-major order, with its
/// interpolation weight.
#[derive(Debug, Clone)]
pub struct WeightedPlane {
    pub values: Vec<f64>,
    pub weight: f64,
}

pub struct ColorwashRequest<'a> {
    pub planes: &'a [WeightedPlane],
    pub plane_rows: u32,
    pub plane_columns: u32,
    /// Displayed-frame pixel to plane pixel.
    pub transform: PixelAffineTransform,
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
    let mut color = [0_u8; 3];
    for channel in 0..3 {
        let from = f64::from(COLORMAP_STOPS[low][channel]);
        let to = f64::from(COLORMAP_STOPS[high][channel]);
        color[channel] = (from + (to - from) * fraction).round() as u8;
    }
    color
}

pub fn encode_colorwash_png(request: ColorwashRequest<'_>) -> PixelResult<Bytes> {
    let plane_len = u64::from(request.plane_rows) * u64::from(request.plane_columns);
    if request
        .planes
        .iter()
        .any(|plane| plane.values.len() as u64 != plane_len)
    {
        return Err(PixelError::UnsupportedLayout(
            "overlay plane values do not match the plane grid".to_string(),
        ));
    }
    let output_len = usize::try_from(
        u64::from(request.target_rows)
            .checked_mul(u64::from(request.target_columns))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                PixelError::UnsupportedLayout("overlay dimensions overflow".to_string())
            })?,
    )
    .map_err(|_| PixelError::UnsupportedLayout("overlay dimensions overflow".to_string()))?;
    let mut rgba = vec![0_u8; output_len];
    let ColorScale {
        min,
        max,
        transparent_at_or_below,
    } = request.scale;
    let span = max - min;

    for target_row in 0..request.target_rows {
        for target_column in 0..request.target_columns {
            let [row, column] = request
                .transform
                .map(f64::from(target_row), f64::from(target_column));
            let mut value = 0.0;
            for plane in request.planes {
                let Some(sample) = bilinear(
                    &plane.values,
                    request.plane_rows,
                    request.plane_columns,
                    row,
                    column,
                ) else {
                    value = f64::NAN;
                    break;
                };
                value += sample * plane.weight;
            }
            if !value.is_finite() || transparent_at_or_below.is_some_and(|floor| value <= floor) {
                continue;
            }
            let position = if span > 0.0 {
                (value - min) / span
            } else {
                0.0
            };
            let index = (target_row as usize * request.target_columns as usize
                + target_column as usize)
                * 4;
            rgba[index..index + 3].copy_from_slice(&colormap(position));
            rgba[index + 3] = 255;
        }
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

/// Bilinear sample at a pixel-center coordinate, or `None` outside the grid
/// (more than half a pixel beyond an edge center) or at a non-finite value.
fn bilinear(values: &[f64], rows: u32, columns: u32, row: f64, column: f64) -> Option<f64> {
    let max_row = f64::from(rows) - 1.0;
    let max_column = f64::from(columns) - 1.0;
    if !(row >= -0.5 && row <= max_row + 0.5 && column >= -0.5 && column <= max_column + 0.5) {
        return None;
    }
    let row = row.clamp(0.0, max_row);
    let column = column.clamp(0.0, max_column);
    let (row0, column0) = (row.floor() as usize, column.floor() as usize);
    let row1 = (row0 + 1).min(rows as usize - 1);
    let column1 = (column0 + 1).min(columns as usize - 1);
    let (row_fraction, column_fraction) = (row - row0 as f64, column - column0 as f64);
    let at = |r: usize, c: usize| values[r * columns as usize + c];
    let top = at(row0, column0) * (1.0 - column_fraction) + at(row0, column1) * column_fraction;
    let bottom = at(row1, column0) * (1.0 - column_fraction) + at(row1, column1) * column_fraction;
    let value = top * (1.0 - row_fraction) + bottom * row_fraction;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> PixelAffineTransform {
        PixelAffineTransform {
            source_origin: [0.0, 0.0],
            source_step_for_target_row: [0.5, 0.0],
            source_step_for_target_column: [0.0, 0.5],
        }
    }

    #[test]
    fn colormap_interpolates_between_stops() {
        assert_eq!(colormap(0.0), COLORMAP_STOPS[0]);
        assert_eq!(colormap(1.0), COLORMAP_STOPS[8]);
        assert_eq!(colormap(0.0625), [70, 23, 103]);
        assert_eq!(colormap(f64::NAN), COLORMAP_STOPS[0]);
    }

    #[test]
    fn interpolates_planes_and_leaves_zero_and_outside_transparent() {
        // Two 2x2 planes sampled at half-pixel steps onto a 4x4 frame.
        let planes = [
            WeightedPlane {
                values: vec![0.0, 2.0, 4.0, 6.0],
                weight: 0.5,
            },
            WeightedPlane {
                values: vec![0.0, 6.0, 8.0, 10.0],
                weight: 0.5,
            },
        ];
        let png = encode_colorwash_png(ColorwashRequest {
            planes: &planes,
            plane_rows: 2,
            plane_columns: 2,
            transform: identity(),
            target_rows: 4,
            target_columns: 4,
            scale: ColorScale {
                min: 0.0,
                max: 8.0,
                transparent_at_or_below: Some(0.0),
            },
        })
        .expect("colorwash PNG");
        let image = image::load_from_memory(&png).expect("decode").to_rgba8();

        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        // (row 0, column 1): plane values 2 and 6 average to 4, mid-scale.
        let [r, g, b] = colormap(0.5);
        assert_eq!(image.get_pixel(2, 0).0, [r, g, b, 255]);
        // (row 1, column 1): 6 and 10 average to the top of the scale.
        let [r, g, b] = colormap(1.0);
        assert_eq!(image.get_pixel(2, 2).0, [r, g, b, 255]);
        // Target column 3 maps to plane column 1.5, still within half a pixel.
        assert_eq!(image.get_pixel(3, 2).0[3], 255);
    }

    #[test]
    fn samples_outside_the_plane_grid_are_transparent() {
        assert_eq!(bilinear(&[1.0; 4], 2, 2, -0.6, 0.0), None);
        assert_eq!(bilinear(&[1.0; 4], 2, 2, 1.5, 1.5), Some(1.0));
        assert_eq!(bilinear(&[1.0; 4], 2, 2, 1.0, 1.6), None);
    }
}
