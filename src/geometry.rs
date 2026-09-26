//! Patient-coordinate image geometry shared by series and semantic features.

use crate::types::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatientFrameGeometry {
    pub position: [f64; 3],
    pub orientation: [f64; 6],
    pub pixel_spacing: [f64; 2],
    pub rows: u32,
    pub columns: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryTolerances {
    pub plane_distance_mm: f64,
    pub orientation: f64,
}

impl Default for GeometryTolerances {
    fn default() -> Self {
        Self {
            plane_distance_mm: 1.0e-3,
            orientation: 1.0e-5,
        }
    }
}

/// Affine mapping from a target image pixel coordinate to a source image pixel
/// coordinate. Pixel coordinates are expressed as `(row, column)` at pixel
/// centers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelAffineTransform {
    pub source_origin: [f64; 2],
    pub source_step_for_target_row: [f64; 2],
    pub source_step_for_target_column: [f64; 2],
}

impl PixelAffineTransform {
    pub fn map(&self, target_row: f64, target_column: f64) -> [f64; 2] {
        [
            self.source_origin[0]
                + target_row * self.source_step_for_target_row[0]
                + target_column * self.source_step_for_target_column[0],
            self.source_origin[1]
                + target_row * self.source_step_for_target_row[1]
                + target_column * self.source_step_for_target_column[1],
        ]
    }
}

pub fn frame_geometry(file: &FileEntry, frame: u32) -> Option<PatientFrameGeometry> {
    let geometry = PatientFrameGeometry {
        position: file.frame_image_position_patient(frame)?,
        orientation: file.frame_image_orientation_patient(frame)?,
        pixel_spacing: file.frame_pixel_spacing(frame)?,
        rows: file.rows,
        columns: file.columns,
    };
    valid_geometry(geometry).then_some(geometry)
}

/// Build a patient-coordinate mapping from `target` pixels into `source`
/// pixels when both grids are coplanar and consistently oriented.
pub fn target_to_source_transform(
    source: PatientFrameGeometry,
    target: PatientFrameGeometry,
    tolerances: GeometryTolerances,
) -> Option<PixelAffineTransform> {
    if !valid_geometry(source) || !valid_geometry(target) {
        return None;
    }
    let [source_row, source_column] = orientation_axes(source.orientation)?;
    let [target_row, target_column] = orientation_axes(target.orientation)?;
    if 1.0 - dot(source_row, target_row) > tolerances.orientation
        || 1.0 - dot(source_column, target_column) > tolerances.orientation
    {
        return None;
    }
    let source_normal = normalized(cross(source_row, source_column))?;
    let target_normal = normalized(cross(target_row, target_column))?;
    if 1.0 - dot(source_normal, target_normal) > tolerances.orientation {
        return None;
    }
    if dot(subtract(target.position, source.position), source_normal).abs()
        > tolerances.plane_distance_mm
    {
        return None;
    }
    in_plane_transform(source, target)
}

/// Project `target` pixels onto the plane of `source` and express them as
/// `source` pixel coordinates. The distance between the planes and any
/// in-plane rotation or flip between the grids are not checked; callers
/// decide which geometric relationship they accept.
pub fn in_plane_transform(
    source: PatientFrameGeometry,
    target: PatientFrameGeometry,
) -> Option<PixelAffineTransform> {
    if !valid_geometry(source) || !valid_geometry(target) {
        return None;
    }
    let [source_row, source_column] = orientation_axes(source.orientation)?;
    let [target_row, target_column] = orientation_axes(target.orientation)?;
    let origin_delta = subtract(target.position, source.position);

    // DICOM Pixel Spacing is [row spacing, column spacing]. Image
    // Orientation's first triplet advances with columns; its second triplet
    // advances with rows.
    let source_origin = [
        dot(origin_delta, source_column) / source.pixel_spacing[0],
        dot(origin_delta, source_row) / source.pixel_spacing[1],
    ];
    let target_row_vector = scale(target_column, target.pixel_spacing[0]);
    let target_column_vector = scale(target_row, target.pixel_spacing[1]);
    Some(PixelAffineTransform {
        source_origin,
        source_step_for_target_row: [
            dot(target_row_vector, source_column) / source.pixel_spacing[0],
            dot(target_row_vector, source_row) / source.pixel_spacing[1],
        ],
        source_step_for_target_column: [
            dot(target_column_vector, source_column) / source.pixel_spacing[0],
            dot(target_column_vector, source_row) / source.pixel_spacing[1],
        ],
    })
}

pub fn grids_overlap(
    source: PatientFrameGeometry,
    target: PatientFrameGeometry,
    transform: PixelAffineTransform,
) -> bool {
    let target_corners = [
        transform.map(0.0, 0.0),
        transform.map(f64::from(target.rows.saturating_sub(1)), 0.0),
        transform.map(0.0, f64::from(target.columns.saturating_sub(1))),
        transform.map(
            f64::from(target.rows.saturating_sub(1)),
            f64::from(target.columns.saturating_sub(1)),
        ),
    ];
    let min_row = target_corners
        .iter()
        .map(|p| p[0])
        .fold(f64::INFINITY, f64::min);
    let max_row = target_corners
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let min_column = target_corners
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min);
    let max_column = target_corners
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    max_row >= -0.5
        && min_row <= f64::from(source.rows) - 0.5
        && max_column >= -0.5
        && min_column <= f64::from(source.columns) - 0.5
}

/// The unit row and column direction cosines of an Image Orientation
/// (Patient): the directions of increasing column and increasing row.
pub(crate) fn orientation_axes(orientation: [f64; 6]) -> Option<[[f64; 3]; 2]> {
    Some([
        normalized([orientation[0], orientation[1], orientation[2]])?,
        normalized([orientation[3], orientation[4], orientation[5]])?,
    ])
}

fn valid_geometry(geometry: PatientFrameGeometry) -> bool {
    geometry.rows > 0
        && geometry.columns > 0
        && geometry.position.iter().all(|value| value.is_finite())
        && geometry.orientation.iter().all(|value| value.is_finite())
        && geometry
            .pixel_spacing
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
        && normalized([
            geometry.orientation[0],
            geometry.orientation[1],
            geometry.orientation[2],
        ])
        .is_some()
        && normalized([
            geometry.orientation[3],
            geometry.orientation[4],
            geometry.orientation[5],
        ])
        .is_some()
}

/// The unit vector along `vector`, or `None` when its length is degenerate or
/// not finite.
pub(crate) fn normalized(vector: [f64; 3]) -> Option<[f64; 3]> {
    let length = magnitude(vector);
    (length.is_finite() && length > f64::EPSILON)
        .then(|| [vector[0] / length, vector[1] / length, vector[2] / length])
}

pub(crate) fn magnitude(vector: [f64; 3]) -> f64 {
    dot(vector, vector).sqrt()
}

pub(crate) fn subtract(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

pub(crate) fn scale(vector: [f64; 3], factor: f64) -> [f64; 3] {
    [vector[0] * factor, vector[1] * factor, vector[2] * factor]
}

pub(crate) fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

pub(crate) fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(rows: u32, columns: u32) -> PatientFrameGeometry {
        PatientFrameGeometry {
            position: [0.0, 0.0, 10.0],
            orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            pixel_spacing: [2.0, 3.0],
            rows,
            columns,
        }
    }

    #[test]
    fn maps_differently_sized_coplanar_grids_in_patient_space() {
        let source = geometry(190, 192);
        let mut target = geometry(192, 190);
        target.position = [3.0, 2.0, 10.0];
        let transform = target_to_source_transform(source, target, GeometryTolerances::default())
            .expect("coplanar transform");
        assert_eq!(transform.map(0.0, 0.0), [1.0, 1.0]);
        assert_eq!(transform.map(2.0, 3.0), [3.0, 4.0]);
        assert!(grids_overlap(source, target, transform));
    }

    #[test]
    fn in_plane_transform_projects_parallel_planes_and_in_plane_rotations() {
        let source = geometry(16, 16);
        let mut target = geometry(8, 8);
        target.position = [3.0, 2.0, 25.0];
        // Target rows advance along patient +x and columns along +y.
        target.orientation = [0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
        target.pixel_spacing = [3.0, 2.0];
        assert!(
            target_to_source_transform(source, target, GeometryTolerances::default()).is_none()
        );
        let transform = in_plane_transform(source, target).expect("parallel projection");
        assert_eq!(transform.map(0.0, 0.0), [1.0, 1.0]);
        assert_eq!(transform.map(1.0, 0.0), [1.0, 2.0]);
        assert_eq!(transform.map(0.0, 1.0), [2.0, 1.0]);
    }

    #[test]
    fn rejects_non_coplanar_or_reoriented_grids() {
        let source = geometry(16, 16);
        let mut displaced = source;
        displaced.position[2] += 0.1;
        assert!(
            target_to_source_transform(source, displaced, GeometryTolerances::default()).is_none()
        );

        let mut reoriented = source;
        reoriented.orientation = [0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
        assert!(
            target_to_source_transform(source, reoriented, GeometryTolerances::default()).is_none()
        );
    }
}
