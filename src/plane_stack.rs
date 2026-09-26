//! Stacks of parallel value planes, such as an RT Dose grid, and how one
//! displayed image frame samples them.
//!
//! A displayed frame is covered when it is parallel to the planes, lies
//! within the stack along the normal (or at most half a plane spacing beyond
//! an end plane), and overlaps the planes' in-plane grid. Its values are then
//! a bilinear sample of the one or two planes that bracket it, weighted
//! linearly by distance along the normal.

use crate::api::contracts::DoseGridGeometry;
use crate::geometry::{
    dot, grids_overlap, in_plane_transform, plane_normal, subtract, valid_geometry,
    GeometryTolerances, PatientFrameGeometry, PixelAffineTransform,
};

/// A stored plane closer than this weight to a displayed frame is used alone.
const SINGLE_PLANE_WEIGHT: f64 = 1.0e-6;

#[derive(Debug, Clone, PartialEq)]
pub struct PlaneStack {
    /// Geometry of the plane at offset zero. Every plane shares its in-plane
    /// grid and lies at `offset` millimeters along `normal` from it.
    base: PatientFrameGeometry,
    normal: [f64; 3],
    /// Ascending, distinct offsets.
    planes: Vec<StackPlane>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StackPlane {
    offset: f64,
    frame: u32,
}

/// How one displayed frame samples a [`PlaneStack`].
#[derive(Debug, Clone, PartialEq)]
pub struct StackSample {
    /// One stack frame, or two bracketing frames whose weights sum to one.
    pub planes: Vec<(u32, f64)>,
    /// Displayed-frame pixel to stack-plane pixel.
    pub transform: PixelAffineTransform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StackSampleError {
    #[error("the displayed frame is not parallel to the overlay planes")]
    NotParallel,
    #[error("the overlay does not cover the displayed frame")]
    NotCovered,
}

impl PlaneStack {
    /// The planes of an RT Dose grid. Grid Frame Offset Vector values are
    /// relative to Image Position (Patient) when the first is zero and
    /// absolute plane coordinates otherwise; either way frame `k` lies at
    /// `offset[k] - offset[0]` along the normal from the first plane.
    pub fn from_dose_grid(
        rows: u32,
        columns: u32,
        frame_count: u32,
        geometry: &DoseGridGeometry,
    ) -> Result<Self, String> {
        let base = PatientFrameGeometry {
            position: geometry
                .image_position_patient
                .ok_or("dose grid Image Position (Patient) is missing")?,
            orientation: geometry
                .image_orientation_patient
                .ok_or("dose grid Image Orientation (Patient) is missing")?,
            pixel_spacing: geometry
                .pixel_spacing
                .ok_or("dose grid Pixel Spacing is missing")?,
            rows,
            columns,
        };
        let offsets = match geometry.grid_frame_offsets.as_slice() {
            [] if frame_count == 1 => &[0.0][..],
            offsets => offsets,
        };
        if offsets.len() != frame_count as usize {
            return Err(format!(
                "Grid Frame Offset Vector lists {} offsets for {frame_count} frames",
                offsets.len()
            ));
        }
        let first = offsets[0];
        Self::new(
            base,
            offsets
                .iter()
                .zip(0_u32..)
                .map(|(offset, frame)| StackPlane {
                    offset: offset - first,
                    frame,
                })
                .collect(),
        )
    }

    fn new(base: PatientFrameGeometry, mut planes: Vec<StackPlane>) -> Result<Self, String> {
        if !valid_geometry(base) {
            return Err("overlay plane geometry is incomplete or degenerate".to_string());
        }
        let normal = plane_normal(base.orientation)
            .ok_or("overlay plane orientation is degenerate".to_string())?;
        if planes.is_empty() || planes.iter().any(|plane| !plane.offset.is_finite()) {
            return Err("overlay plane positions are missing or not finite".to_string());
        }
        planes.sort_by(|left, right| left.offset.total_cmp(&right.offset));
        if planes.windows(2).any(|pair| {
            pair[1].offset - pair[0].offset <= GeometryTolerances::default().plane_distance_mm
        }) {
            return Err("two overlay frames share one plane".to_string());
        }
        Ok(Self {
            base,
            normal,
            planes,
        })
    }

    pub fn plane_rows(&self) -> u32 {
        self.base.rows
    }

    pub fn plane_columns(&self) -> u32 {
        self.base.columns
    }

    pub fn sample(&self, target: PatientFrameGeometry) -> Result<StackSample, StackSampleError> {
        let target_normal =
            plane_normal(target.orientation).ok_or(StackSampleError::NotParallel)?;
        if 1.0 - dot(self.normal, target_normal).abs() > GeometryTolerances::default().orientation {
            return Err(StackSampleError::NotParallel);
        }
        let distance = dot(subtract(target.position, self.base.position), self.normal);
        let planes = self.bracket(distance).ok_or(StackSampleError::NotCovered)?;
        let transform =
            in_plane_transform(self.base, target).ok_or(StackSampleError::NotParallel)?;
        if !grids_overlap(self.base, target, transform) {
            return Err(StackSampleError::NotCovered);
        }
        Ok(StackSample { planes, transform })
    }

    fn bracket(&self, distance: f64) -> Option<Vec<(u32, f64)>> {
        let first = self.planes[0];
        let last = self.planes[self.planes.len() - 1];
        let (below, above) = match self.planes.as_slice() {
            [_] => {
                let tolerance = GeometryTolerances::default().plane_distance_mm;
                (tolerance, tolerance)
            }
            planes => (
                (planes[1].offset - first.offset) / 2.0,
                (last.offset - planes[planes.len() - 2].offset) / 2.0,
            ),
        };
        if !distance.is_finite()
            || distance < first.offset - below
            || distance > last.offset + above
        {
            return None;
        }
        if distance <= first.offset {
            return Some(vec![(first.frame, 1.0)]);
        }
        if distance >= last.offset {
            return Some(vec![(last.frame, 1.0)]);
        }
        let upper = self
            .planes
            .partition_point(|plane| plane.offset <= distance);
        let (low, high) = (self.planes[upper - 1], self.planes[upper]);
        let weight = (distance - low.offset) / (high.offset - low.offset);
        Some(if weight <= SINGLE_PLANE_WEIGHT {
            vec![(low.frame, 1.0)]
        } else if weight >= 1.0 - SINGLE_PLANE_WEIGHT {
            vec![(high.frame, 1.0)]
        } else {
            vec![(low.frame, 1.0 - weight), (high.frame, weight)]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dose_geometry(offsets: Vec<f64>) -> DoseGridGeometry {
        DoseGridGeometry {
            frame_of_reference_uid: None,
            image_position_patient: Some([0.0, 0.0, 10.0]),
            image_orientation_patient: Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            pixel_spacing: Some([4.0, 4.0]),
            grid_frame_offsets: offsets,
        }
    }

    fn slice(z: f64) -> PatientFrameGeometry {
        PatientFrameGeometry {
            position: [0.0, 0.0, z],
            orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            pixel_spacing: [2.0, 2.0],
            rows: 10,
            columns: 10,
        }
    }

    fn stack(offsets: Vec<f64>) -> PlaneStack {
        let frames = offsets.len() as u32;
        PlaneStack::from_dose_grid(4, 4, frames, &dose_geometry(offsets)).expect("dose stack")
    }

    #[test]
    fn interpolates_between_bracketing_planes_and_maps_in_plane_pixels() {
        let sample = stack(vec![0.0, 4.0, 8.0])
            .sample(slice(16.0))
            .expect("covered");
        assert_eq!(sample.planes, vec![(1, 0.5), (2, 0.5)]);
        assert_eq!(sample.transform.map(2.0, 4.0), [1.0, 2.0]);

        let exact = stack(vec![0.0, 4.0, 8.0])
            .sample(slice(14.0))
            .expect("covered");
        assert_eq!(exact.planes, vec![(1, 1.0)]);
    }

    #[test]
    fn absolute_and_descending_offsets_place_the_same_planes() {
        // Absolute offsets: the first equals the grid's own z.
        let absolute = stack(vec![10.0, 14.0, 18.0]);
        assert_eq!(absolute, stack(vec![0.0, 4.0, 8.0]));
        let descending = stack(vec![0.0, -4.0]).sample(slice(7.0)).expect("covered");
        assert_eq!(descending.planes, vec![(1, 0.75), (0, 0.25)]);
    }

    #[test]
    fn covers_half_a_spacing_beyond_the_end_planes_only() {
        let stack = stack(vec![0.0, 4.0, 8.0]);
        assert_eq!(stack.sample(slice(20.0)).unwrap().planes, vec![(2, 1.0)]);
        assert_eq!(stack.sample(slice(8.0)).unwrap().planes, vec![(0, 1.0)]);
        assert_eq!(stack.sample(slice(20.5)), Err(StackSampleError::NotCovered));
        assert_eq!(stack.sample(slice(7.5)), Err(StackSampleError::NotCovered));
    }

    #[test]
    fn rejects_frames_without_in_plane_overlap_or_parallel_planes() {
        let stack = stack(vec![0.0, 4.0]);
        let mut beside = slice(12.0);
        beside.position[0] = 100.0;
        assert_eq!(stack.sample(beside), Err(StackSampleError::NotCovered));
        let mut sagittal = slice(12.0);
        sagittal.orientation = [0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
        assert_eq!(stack.sample(sagittal), Err(StackSampleError::NotParallel));
    }

    #[test]
    fn rejects_offset_vectors_that_do_not_describe_one_plane_per_frame() {
        assert!(PlaneStack::from_dose_grid(4, 4, 3, &dose_geometry(vec![0.0, 4.0])).is_err());
        assert!(PlaneStack::from_dose_grid(4, 4, 2, &dose_geometry(vec![0.0, 0.0])).is_err());
        assert!(PlaneStack::from_dose_grid(4, 4, 1, &dose_geometry(Vec::new())).is_ok());
    }
}
