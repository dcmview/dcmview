use crate::types::OverlayPlane;

const DEFAULT_OVERLAY_PRESENTATION_VALUE: u8 = 255;

pub(crate) fn apply_overlay_planes(
    samples: &mut [u8],
    image_rows: u32,
    image_columns: u32,
    frame: u32,
    planes: &[OverlayPlane],
) {
    for plane in planes {
        for_each_set_pixel(plane, image_rows, image_columns, frame, |index| {
            if let Some(sample) = samples.get_mut(index) {
                *sample = DEFAULT_OVERLAY_PRESENTATION_VALUE;
            }
        });
    }
}

/// Calls `visit` with the row-major image sample index of every set bit of
/// `plane`'s overlay frame for zero-based image `frame` that lands inside the
/// image. Nothing is visited when the plane has no frame for this image frame.
pub(crate) fn for_each_set_pixel(
    plane: &OverlayPlane,
    image_rows: u32,
    image_columns: u32,
    frame: u32,
    mut visit: impl FnMut(usize),
) {
    let Some(source_frame) = frame.checked_add(1) else {
        return;
    };
    let Some(overlay_frame) = source_frame.checked_sub(plane.image_frame_origin) else {
        return;
    };
    if overlay_frame >= plane.number_of_frames {
        return;
    }
    let Some(plane_pixels) = u64::from(plane.rows).checked_mul(u64::from(plane.columns)) else {
        return;
    };
    let frame_bit_offset = u64::from(overlay_frame).saturating_mul(plane_pixels);
    let image_rows = i64::from(image_rows);
    let image_columns = i64::from(image_columns);
    let row_origin = i64::from(plane.origin[0]) - 1;
    let column_origin = i64::from(plane.origin[1]) - 1;

    for overlay_row in 0..plane.rows {
        for overlay_column in 0..plane.columns {
            let overlay_offset =
                u64::from(overlay_row) * u64::from(plane.columns) + u64::from(overlay_column);
            let bit_index = frame_bit_offset + overlay_offset;
            let Some(word) = plane.data.get((bit_index / 16) as usize) else {
                continue;
            };
            if word & (1_u16 << (bit_index % 16)) == 0 {
                continue;
            }

            let image_row = row_origin + i64::from(overlay_row);
            let image_column = column_origin + i64::from(overlay_column);
            if image_row < 0
                || image_row >= image_rows
                || image_column < 0
                || image_column >= image_columns
            {
                continue;
            }
            visit(image_row as usize * image_columns as usize + image_column as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_overlay_planes;
    use crate::types::OverlayPlane;

    fn plane(origin: [i32; 2], data: Vec<u16>) -> OverlayPlane {
        OverlayPlane {
            group: 0x6000,
            rows: 2,
            columns: 2,
            origin,
            overlay_type: "G".to_string(),
            number_of_frames: 1,
            image_frame_origin: 1,
            data,
        }
    }

    #[test]
    fn composites_prepared_lsb_first_diagonal_after_luminance_rendering() {
        let mut samples = vec![10, 20, 30, 40];
        apply_overlay_planes(&mut samples, 2, 2, 0, &[plane([1, 1], vec![0x0009])]);
        assert_eq!(samples, [255, 20, 30, 255]);
    }

    #[test]
    fn clips_signed_origins_and_respects_overlay_frame_placement() {
        let mut clipped = vec![10, 20, 30, 40];
        apply_overlay_planes(&mut clipped, 2, 2, 0, &[plane([0, 0], vec![0x0009])]);
        assert_eq!(clipped, [255, 20, 30, 40]);

        let mut future = plane([1, 1], vec![0x000f]);
        future.image_frame_origin = 2;
        let mut samples = vec![10, 20, 30, 40];
        apply_overlay_planes(&mut samples, 2, 2, 0, &[future.clone()]);
        assert_eq!(samples, [10, 20, 30, 40]);
        apply_overlay_planes(&mut samples, 2, 2, 1, &[future]);
        assert_eq!(samples, [255, 255, 255, 255]);
    }
}
