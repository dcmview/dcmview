use crate::api::contracts::RawFrameMetadata;
use crate::semantic::SegmentationOverlayPlan;
use bytes::Bytes;
use image::{ExtendedColorType, ImageEncoder};

use super::color::png_encoder;

use super::{PixelError, PixelResult};

pub fn encode_segmentation_overlay_png(
    samples: &Bytes,
    metadata: &RawFrameMetadata,
    plan: &SegmentationOverlayPlan,
    target_rows: u32,
    target_columns: u32,
) -> PixelResult<Bytes> {
    if metadata.samples_per_pixel != 1 || metadata.bits_allocated > 8 {
        return Err(PixelError::UnsupportedLayout(
            "SEG overlay requires one expanded 8-bit-or-smaller sample per pixel".to_string(),
        ));
    }
    let sample_count = usize::try_from(u64::from(metadata.rows) * u64::from(metadata.columns))
        .map_err(|_| PixelError::UnsupportedLayout("SEG frame dimensions overflow".to_string()))?;
    if samples.len() != sample_count {
        return Err(PixelError::UnsupportedLayout(format!(
            "SEG raw frame has {} bytes for {sample_count} pixels",
            samples.len()
        )));
    }
    let output_len = usize::try_from(
        u64::from(target_rows)
            .checked_mul(u64::from(target_columns))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                PixelError::UnsupportedLayout("overlay dimensions overflow".to_string())
            })?,
    )
    .map_err(|_| PixelError::UnsupportedLayout("overlay dimensions overflow".to_string()))?;
    let mut rgba = vec![0_u8; output_len];
    let maximum_fractional = match plan.segmentation_type.as_str() {
        "BINARY" => None,
        "FRACTIONAL" => Some(
            plan.maximum_fractional_value
                .filter(|value| *value > 0 && *value <= 255)
                .ok_or_else(|| {
                    PixelError::UnsupportedLayout(
                        "fractional SEG overlay requires Maximum Fractional Value in 1..=255"
                            .to_string(),
                    )
                })?,
        ),
        other => {
            return Err(PixelError::UnsupportedLayout(format!(
                "SEG overlay does not support segmentation type {other}"
            )))
        }
    };

    for target_row in 0..target_rows {
        for target_column in 0..target_columns {
            let [source_row, source_column] = plan
                .target_to_segmentation
                .map(f64::from(target_row), f64::from(target_column));
            let source_row = source_row.round();
            let source_column = source_column.round();
            if source_row < 0.0
                || source_column < 0.0
                || source_row >= f64::from(metadata.rows)
                || source_column >= f64::from(metadata.columns)
            {
                continue;
            }
            let source_index =
                source_row as usize * metadata.columns as usize + source_column as usize;
            let value = samples[source_index];
            let alpha = maximum_fractional.map_or_else(
                || if value == 0 { 0 } else { 178 },
                |maximum| ((u32::from(value).min(maximum) * 204) / maximum) as u8,
            );
            if alpha == 0 {
                continue;
            }
            let target_index =
                (target_row as usize * target_columns as usize + target_column as usize) * 4;
            rgba[target_index..target_index + 3].copy_from_slice(&plan.color);
            rgba[target_index + 3] = alpha;
        }
    }

    let mut encoded = Vec::new();
    png_encoder(&mut encoded)
        .write_image(&rgba, target_columns, target_rows, ExtendedColorType::Rgba8)
        .map_err(|error| PixelError::frame_decode(error.into()))?;
    Ok(Bytes::from(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PixelAffineTransform;

    fn metadata(rows: u32, columns: u32) -> RawFrameMetadata {
        RawFrameMetadata {
            rows,
            columns,
            bits_allocated: 1,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            default_wc: None,
            default_ww: None,
            padding_low: None,
            padding_high: None,
        }
    }

    fn plan(transform: PixelAffineTransform) -> SegmentationOverlayPlan {
        SegmentationOverlayPlan {
            segmentation_file_index: 0,
            segmentation_frame_index: 0,
            source_file_index: 1,
            source_frame_index: 0,
            target_to_segmentation: transform,
            segmentation_type: "BINARY".to_string(),
            maximum_fractional_value: None,
            color: [255, 79, 132],
        }
    }

    #[test]
    fn resamples_binary_mask_into_target_grid_with_transparency() {
        let transform = PixelAffineTransform {
            source_origin: [0.0, 0.0],
            source_step_for_target_row: [1.0, 0.0],
            source_step_for_target_column: [0.0, 1.0],
        };
        let png = encode_segmentation_overlay_png(
            &Bytes::from_static(&[0, 1, 1, 0]),
            &metadata(2, 2),
            &plan(transform),
            2,
            2,
        )
        .expect("overlay PNG");
        let decoded = image::load_from_memory(&png)
            .expect("decode PNG")
            .to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(decoded.get_pixel(1, 0).0, [255, 79, 132, 178]);
        assert_eq!(decoded.get_pixel(0, 1).0, [255, 79, 132, 178]);
    }
}
