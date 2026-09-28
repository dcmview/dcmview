use super::color::cielab_to_srgb8;
use super::overlay::for_each_set_pixel;
use crate::types::{DisplayShutter, FileEntry, ShutterShape};

/// Where the display shutter is drawn: one rendered frame and the physical
/// shape of its pixels.
pub(crate) struct ShutterFrame {
    pub(crate) rows: u32,
    pub(crate) columns: u32,
    /// Zero-based image frame, which selects a bitmap shutter's overlay frame.
    pub(crate) frame: u32,
    /// Row spacing over column spacing; 1.0 for square pixels.
    pub(crate) pixel_aspect_ratio: f64,
}

/// A file's shutter for one rendered frame, if it declares one.
fn file_shutter(
    file: &FileEntry,
    frame: u32,
    rows: u32,
    columns: u32,
) -> Option<(&DisplayShutter, ShutterFrame)> {
    let shutter = file
        .series_metadata
        .presentation
        .display_shutter_for_frame(frame)?;
    let target = ShutterFrame {
        rows,
        columns,
        frame,
        pixel_aspect_ratio: file
            .series_metadata
            .native_pixel
            .effective_pixel_aspect_ratio()
            .unwrap_or(1.0),
    };
    Some((shutter, target))
}

/// Applies the file's shutter to a grayscale display frame with the Shutter
/// Presentation Value, drawn as `pixel(gray)` (`N` interleaved bytes).
pub(crate) fn apply_to_luminance<const N: usize>(
    pixels: &mut [u8],
    pixel: impl Fn(u8) -> [u8; N],
    file: &FileEntry,
    frame: u32,
    rows: u32,
    columns: u32,
) {
    if let Some((shutter, target)) = file_shutter(file, frame, rows, columns) {
        fill_outside_opening(pixels, &pixel(luminance_fill(shutter)), target, shutter);
    }
}

/// Applies the file's shutter to an interleaved 8-bit RGB display frame.
pub(crate) fn apply_to_rgb8(rgb: &mut [u8], file: &FileEntry, frame: u32, rows: u32, columns: u32) {
    if let Some((shutter, target)) = file_shutter(file, frame, rows, columns) {
        fill_outside_opening(rgb, &color_fill(shutter), target, shutter);
    }
}

/// Applies the file's shutter to interleaved RGB samples whose full scale
/// is `max`.
pub(crate) fn apply_to_rgb16(
    rgb: &mut [u16],
    max: u16,
    file: &FileEntry,
    frame: u32,
    rows: u32,
    columns: u32,
) {
    if let Some((shutter, target)) = file_shutter(file, frame, rows, columns) {
        let fill = color_fill(shutter)
            .map(|channel| ((u32::from(channel) * u32::from(max) + 127) / 255) as u16);
        fill_outside_opening(rgb, &fill, target, shutter);
    }
}

fn luminance_fill(shutter: &DisplayShutter) -> u8 {
    p_value_to_u8(shutter.presentation_value)
}

/// Shutter Presentation Color CIELab Value as sRGB, else the gray
/// presentation value.
fn color_fill(shutter: &DisplayShutter) -> [u8; 3] {
    shutter
        .presentation_color_cielab
        .map(cielab_to_srgb8)
        .unwrap_or([luminance_fill(shutter); 3])
}

/// Replaces every pixel (`fill.len()` interleaved samples) outside the
/// shutter opening with `fill`. The opening is the intersection of every
/// shape's opening (PS3.3 C.7.6.11.1.1).
fn fill_outside_opening<T: Copy>(
    samples: &mut [T],
    fill: &[T],
    target: ShutterFrame,
    shutter: &DisplayShutter,
) {
    let columns = target.columns as usize;
    let pixel_count = (target.rows as usize)
        .saturating_mul(columns)
        .min(samples.len() / fill.len().max(1));
    if pixel_count == 0 {
        return;
    }

    let mut visible = vec![true; pixel_count];
    for shape in &shutter.shapes {
        if let ShutterShape::Bitmap(plane) = shape {
            for_each_set_pixel(plane, target.rows, target.columns, target.frame, |index| {
                if let Some(visible) = visible.get_mut(index) {
                    *visible = false;
                }
            });
            continue;
        }
        for (index, visible) in visible.iter_mut().enumerate() {
            if *visible {
                let row = (index / columns) as i64 + 1;
                let column = (index % columns) as i64 + 1;
                *visible = opening_contains(shape, row, column, target.pixel_aspect_ratio);
            }
        }
    }

    for (pixel, visible) in samples.chunks_exact_mut(fill.len()).zip(visible) {
        if !visible {
            pixel.copy_from_slice(fill);
        }
    }
}

/// Whether the one-based pixel `(row, column)` lies in a geometric shape's
/// opening, edges included.
fn opening_contains(shape: &ShutterShape, row: i64, column: i64, pixel_aspect_ratio: f64) -> bool {
    match shape {
        ShutterShape::Rectangular {
            left_vertical_edge,
            right_vertical_edge,
            upper_horizontal_edge,
            lower_horizontal_edge,
        } => {
            (i64::from(*left_vertical_edge)..=i64::from(*right_vertical_edge)).contains(&column)
                && (i64::from(*upper_horizontal_edge)..=i64::from(*lower_horizontal_edge))
                    .contains(&row)
        }
        ShutterShape::Circular { center, radius } => {
            // The radius counts pixels along a row, so scale row offsets into
            // column-pixel units.
            let row_offset = (row - i64::from(center[0])) as f64 * pixel_aspect_ratio;
            let column_offset = (column - i64::from(center[1])) as f64;
            let radius = f64::from(*radius);
            row_offset * row_offset + column_offset * column_offset <= radius * radius
        }
        ShutterShape::Polygonal { vertices } => polygon_contains(vertices, row, column),
        ShutterShape::Bitmap(_) => true,
    }
}

/// Even-odd point-in-polygon test in exact integer arithmetic; a point on an
/// edge or vertex is inside.
fn polygon_contains(vertices: &[[i32; 2]], row: i64, column: i64) -> bool {
    let mut inside = false;
    for (index, start) in vertices.iter().enumerate() {
        let end = vertices[(index + 1) % vertices.len()];
        let (start_row, start_column) = (i64::from(start[0]), i64::from(start[1]));
        let (end_row, end_column) = (i64::from(end[0]), i64::from(end[1]));

        let cross = (end_row - start_row) * (column - start_column)
            - (end_column - start_column) * (row - start_row);
        if cross == 0
            && row >= start_row.min(end_row)
            && row <= start_row.max(end_row)
            && column >= start_column.min(end_column)
            && column <= start_column.max(end_column)
        {
            return true;
        }

        if (start_row > row) != (end_row > row) {
            // The edge crosses this row; toggle when the crossing lies to the
            // right of the point: column < start_column + t * (end_column -
            // start_column) with t = (row - start_row) / (end_row - start_row).
            let row_span = end_row - start_row;
            let lhs = (column - start_column) * row_span;
            let rhs = (row - start_row) * (end_column - start_column);
            if (row_span > 0 && lhs < rhs) || (row_span < 0 && lhs > rhs) {
                inside = !inside;
            }
        }
    }
    inside
}

fn p_value_to_u8(value: u16) -> u8 {
    ((u32::from(value) * 255 + 32_767) / 65_535) as u8
}

#[cfg(test)]
mod tests {
    use super::{color_fill, fill_outside_opening, p_value_to_u8, ShutterFrame};
    use crate::types::{DisplayShutter, OverlayPlane, ShutterShape};

    fn frame(rows: u32, columns: u32) -> ShutterFrame {
        ShutterFrame {
            rows,
            columns,
            frame: 0,
            pixel_aspect_ratio: 1.0,
        }
    }

    fn shutter(shapes: Vec<ShutterShape>) -> DisplayShutter {
        DisplayShutter {
            shapes,
            presentation_value: 0,
            presentation_color_cielab: None,
        }
    }

    fn rectangle(left: i32, right: i32, upper: i32, lower: i32) -> ShutterShape {
        ShutterShape::Rectangular {
            left_vertical_edge: left,
            right_vertical_edge: right,
            upper_horizontal_edge: upper,
            lower_horizontal_edge: lower,
        }
    }

    /// Renders a 255-filled frame through the shutter as rows of 0/1 flags,
    /// 1 marking a visible pixel.
    fn visibility(target: ShutterFrame, shutter: &DisplayShutter) -> Vec<Vec<u8>> {
        let columns = target.columns as usize;
        let mut samples = vec![255_u8; target.rows as usize * columns];
        fill_outside_opening(&mut samples, &[0], target, shutter);
        samples
            .chunks(columns)
            .map(|row| row.iter().map(|value| u8::from(*value == 255)).collect())
            .collect()
    }

    #[test]
    fn prepared_full_frame_rectangle_preserves_display_pixels() {
        let mut samples = vec![0, 64, 128, 255];
        fill_outside_opening(
            &mut samples,
            &[0],
            frame(2, 2),
            &shutter(vec![rectangle(1, 2, 1, 2)]),
        );
        assert_eq!(samples, [0, 64, 128, 255]);
    }

    #[test]
    fn non_degenerate_rectangle_replaces_every_pixel_outside_inclusive_edges() {
        let mut samples = (1_u8..=9).collect::<Vec<_>>();
        fill_outside_opening(
            &mut samples,
            &[0],
            frame(3, 3),
            &shutter(vec![rectangle(2, 2, 2, 2)]),
        );
        assert_eq!(samples, [0, 0, 0, 0, 5, 0, 0, 0, 0]);
    }

    #[test]
    fn rectangle_edges_are_columns_then_rows() {
        // Columns 2-3, row 1 only: a wide opening, not a tall one.
        assert_eq!(
            visibility(frame(3, 4), &shutter(vec![rectangle(2, 3, 1, 1)])),
            [[0, 1, 1, 0], [0, 0, 0, 0], [0, 0, 0, 0]]
        );
    }

    #[test]
    fn circle_keeps_pixels_within_the_radius_edge_included() {
        let circle = ShutterShape::Circular {
            center: [3, 3],
            radius: 2,
        };
        assert_eq!(
            visibility(frame(5, 5), &shutter(vec![circle])),
            [
                [0, 0, 1, 0, 0],
                [0, 1, 1, 1, 0],
                [1, 1, 1, 1, 1],
                [0, 1, 1, 1, 0],
                [0, 0, 1, 0, 0],
            ]
        );
    }

    #[test]
    fn circle_center_is_row_then_column() {
        let circle = ShutterShape::Circular {
            center: [1, 3],
            radius: 0,
        };
        assert_eq!(
            visibility(frame(2, 4), &shutter(vec![circle])),
            [[0, 0, 1, 0], [0, 0, 0, 0]]
        );
    }

    #[test]
    fn circle_radius_counts_pixels_along_a_row() {
        // Rows twice as tall as columns are wide: a radius of two columns
        // spans only one row above and below the center.
        let circle = ShutterShape::Circular {
            center: [3, 3],
            radius: 2,
        };
        let target = ShutterFrame {
            pixel_aspect_ratio: 2.0,
            ..frame(5, 5)
        };
        assert_eq!(
            visibility(target, &shutter(vec![circle])),
            [
                [0, 0, 0, 0, 0],
                [0, 0, 1, 0, 0],
                [1, 1, 1, 1, 1],
                [0, 0, 1, 0, 0],
                [0, 0, 0, 0, 0],
            ]
        );
    }

    #[test]
    fn polygon_keeps_interior_and_boundary_pixels() {
        // Triangle with vertices (row, column) (1,1), (1,5), (5,1).
        let triangle = ShutterShape::Polygonal {
            vertices: vec![[1, 1], [1, 5], [5, 1]],
        };
        assert_eq!(
            visibility(frame(5, 5), &shutter(vec![triangle])),
            [
                [1, 1, 1, 1, 1],
                [1, 1, 1, 1, 0],
                [1, 1, 1, 0, 0],
                [1, 1, 0, 0, 0],
                [1, 0, 0, 0, 0],
            ]
        );
    }

    #[test]
    fn rectangular_polygon_matches_the_rectangle_shape() {
        let polygon = ShutterShape::Polygonal {
            vertices: vec![[2, 2], [2, 4], [3, 4], [3, 2]],
        };
        assert_eq!(
            visibility(frame(4, 5), &shutter(vec![polygon])),
            visibility(frame(4, 5), &shutter(vec![rectangle(2, 4, 2, 3)]))
        );
    }

    #[test]
    fn concave_polygon_excludes_its_notch() {
        // A "U": the notch at rows 1-2, column 3 is outside.
        let u_shape = ShutterShape::Polygonal {
            vertices: vec![
                [1, 1],
                [1, 2],
                [2, 2],
                [2, 4],
                [1, 4],
                [1, 5],
                [3, 5],
                [3, 1],
            ],
        };
        assert_eq!(
            visibility(frame(3, 5), &shutter(vec![u_shape])),
            [[1, 1, 0, 1, 1], [1, 1, 1, 1, 1], [1, 1, 1, 1, 1]]
        );
    }

    #[test]
    fn combined_shapes_keep_only_the_intersection_of_their_openings() {
        let shapes = vec![
            rectangle(1, 3, 1, 5),
            ShutterShape::Circular {
                center: [3, 3],
                radius: 2,
            },
        ];
        assert_eq!(
            visibility(frame(5, 5), &shutter(shapes)),
            [
                [0, 0, 1, 0, 0],
                [0, 1, 1, 0, 0],
                [1, 1, 1, 0, 0],
                [0, 1, 1, 0, 0],
                [0, 0, 1, 0, 0],
            ]
        );
    }

    #[test]
    fn bitmap_occludes_set_overlay_bits_for_the_matching_frame() {
        // 2x2 plane at image row 2, column 2 with bits 0 and 3 set.
        let plane = OverlayPlane {
            group: 0x6000,
            rows: 2,
            columns: 2,
            origin: [2, 2],
            overlay_type: "G".to_string(),
            number_of_frames: 1,
            image_frame_origin: 2,
            data: vec![0b1001],
        };
        let bitmap = shutter(vec![ShutterShape::Bitmap(plane)]);
        assert_eq!(visibility(frame(3, 3), &bitmap), [[1, 1, 1]; 3]);
        assert_eq!(
            visibility(
                ShutterFrame {
                    frame: 1,
                    ..frame(3, 3)
                },
                &bitmap
            ),
            [[1, 1, 1], [1, 0, 1], [1, 1, 0]]
        );
    }

    #[test]
    fn scales_unsigned_p_values_to_display_luminance() {
        assert_eq!(p_value_to_u8(0), 0);
        assert_eq!(p_value_to_u8(32_768), 128);
        assert_eq!(p_value_to_u8(65_535), 255);
    }

    #[test]
    fn interleaved_color_pixels_take_the_whole_fill() {
        let mut rgb = vec![9_u8; 3 * 3];
        fill_outside_opening(
            &mut rgb,
            &[10, 20, 30],
            frame(1, 3),
            &shutter(vec![rectangle(2, 2, 1, 1)]),
        );
        assert_eq!(rgb, [10, 20, 30, 9, 9, 9, 10, 20, 30]);
    }

    #[test]
    fn color_fill_prefers_the_cielab_color_over_the_gray_value() {
        let mut gray = shutter(Vec::new());
        gray.presentation_value = 0xFFFF;
        assert_eq!(color_fill(&gray), [255, 255, 255]);

        let mut colored = gray.clone();
        colored.presentation_color_cielab = Some([0, 0x8080, 0x8080]);
        assert_eq!(color_fill(&colored), [0, 0, 0]);
    }
}
