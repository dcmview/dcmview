//! Stacks of parallel value planes, such as an RT Dose grid, and how one
//! displayed image frame samples them.
//!
//! The stack is a volume: each displayed pixel center has a position along
//! the planes' normal and a row and column in their shared in-plane grid. The
//! pixel is inside the volume when it lies within the grid (at most half a
//! pixel beyond an edge pixel center) and within the stack along the normal
//! (at most half a plane spacing beyond an end plane). Its value is then a
//! bilinear sample of the one or two planes that bracket it, weighted
//! linearly by distance along the normal: trilinear resampling. A displayed
//! frame is covered when any of its pixels is inside the volume.
//!
//! A frame parallel to the planes has one normal position for every pixel,
//! so it brackets the same planes everywhere; one that is not parallel
//! (oblique) brackets planes pixel by pixel.

use crate::api::contracts::DoseGridGeometry;
use crate::geometry::{
    dot, frame_geometry, in_plane_transform, magnitude, orientation_axes, plane_normal, scale,
    subtract, valid_geometry, GeometryTolerances, PatientFrameGeometry, PixelAffineTransform,
};
use crate::types::FileEntry;
use std::ops::Range;

/// A stored plane closer than this weight to a displayed pixel is used alone.
const SINGLE_PLANE_WEIGHT: f64 = 1.0e-6;
/// How far frame origins may drift within the plane and still share a grid.
const IN_PLANE_ORIGIN_TOLERANCE_MM: f64 = 1.0e-2;
/// Slack when choosing the planes an oblique frame may reach, so rounding
/// in a pixel's normal position never needs a plane that was not decoded.
const NORMAL_RANGE_SLACK_MM: f64 = 1.0e-9;

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

/// The planes one normal position samples: `low` alone when `weight` is
/// zero, else `low * (1 - weight) + high * weight`. Positions index the
/// stack's planes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bracket {
    low: usize,
    high: usize,
    weight: f64,
}

/// How one displayed frame samples a [`PlaneStack`].
#[derive(Debug, Clone, PartialEq)]
pub struct StackSample {
    stack: PlaneStack,
    /// Displayed-frame pixel to stack-plane pixel.
    transform: PixelAffineTransform,
    /// Displayed-frame pixel to position along the stack normal:
    /// `normal_origin + row * normal_step[0] + column * normal_step[1]`.
    normal_origin: f64,
    normal_step: [f64; 2],
    /// A frame parallel to the planes: both normal steps are zero and every
    /// pixel brackets the same planes.
    parallel: bool,
    /// The planes, by stack position, that some displayed pixel may sample.
    planes: Range<usize>,
    target_rows: u32,
    target_columns: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StackSampleError {
    #[error("the displayed frame geometry is degenerate")]
    InvalidGeometry,
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

    /// The frames of a multi-frame object whose per-frame geometry forms a
    /// stack: one orientation, pixel spacing, and in-plane origin, with each
    /// frame on its own plane.
    pub fn from_frames(file: &FileEntry) -> Result<Self, String> {
        let geometries = (0..file.frame_count)
            .map(|frame| frame_geometry(file, frame))
            .collect::<Option<Vec<_>>>()
            .ok_or("a frame lacks patient position, orientation, or pixel spacing")?;
        let base = *geometries.first().ok_or("the object has no frames")?;
        let normal = plane_normal(base.orientation)
            .ok_or("overlay plane orientation is degenerate".to_string())?;
        let [base_row, base_column] = orientation_axes(base.orientation)
            .ok_or("overlay plane orientation is degenerate".to_string())?;
        let tolerances = GeometryTolerances::default();
        let mut planes = Vec::with_capacity(geometries.len());
        for (geometry, frame) in geometries.iter().zip(0_u32..) {
            let same_axes = orientation_axes(geometry.orientation).is_some_and(|[row, column]| {
                1.0 - dot(row, base_row) <= tolerances.orientation
                    && 1.0 - dot(column, base_column) <= tolerances.orientation
            });
            let same_spacing = geometry
                .pixel_spacing
                .iter()
                .zip(base.pixel_spacing)
                .all(|(spacing, base)| (spacing - base).abs() <= base * 1.0e-4);
            if !same_axes || !same_spacing {
                return Err("frames differ in orientation or pixel spacing".to_string());
            }
            let delta = subtract(geometry.position, base.position);
            let offset = dot(delta, normal);
            if magnitude(subtract(delta, scale(normal, offset))) > IN_PLANE_ORIGIN_TOLERANCE_MM {
                return Err("frames are shifted within their plane".to_string());
            }
            planes.push(StackPlane { offset, frame });
        }
        Self::new(base, planes)
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

    /// How the displayed frame `target` samples the stack, or `NotCovered`
    /// when none of its pixels lies inside the volume.
    pub fn sample(&self, target: PatientFrameGeometry) -> Result<StackSample, StackSampleError> {
        self.sample_frame(target, true)
    }

    /// With `snap_parallel`, a frame parallel to the planes within the
    /// orientation tolerance gets one normal position for every pixel (its
    /// origin's), which also keeps it to the one or two planes that bracket
    /// that position. Without it every frame is sampled pixel by pixel.
    fn sample_frame(
        &self,
        target: PatientFrameGeometry,
        snap_parallel: bool,
    ) -> Result<StackSample, StackSampleError> {
        let target_normal =
            plane_normal(target.orientation).ok_or(StackSampleError::InvalidGeometry)?;
        let [target_row, target_column] =
            orientation_axes(target.orientation).ok_or(StackSampleError::InvalidGeometry)?;
        let transform =
            in_plane_transform(self.base, target).ok_or(StackSampleError::InvalidGeometry)?;
        let parallel = snap_parallel
            && 1.0 - dot(self.normal, target_normal).abs()
                <= GeometryTolerances::default().orientation;
        let normal_step = if parallel {
            [0.0, 0.0]
        } else {
            // Displayed rows advance along the column direction cosines and
            // displayed columns along the row direction cosines.
            [
                dot(scale(target_column, target.pixel_spacing[0]), self.normal),
                dot(scale(target_row, target.pixel_spacing[1]), self.normal),
            ]
        };
        let mut sample = StackSample {
            stack: self.clone(),
            transform,
            normal_origin: dot(subtract(target.position, self.base.position), self.normal),
            normal_step,
            parallel,
            planes: 0..0,
            target_rows: target.rows,
            target_columns: target.columns,
        };
        sample.planes = sample.plane_range().ok_or(StackSampleError::NotCovered)?;
        if !sample.covers_a_pixel() {
            return Err(StackSampleError::NotCovered);
        }
        Ok(sample)
    }

    /// The normal positions the volume spans: its end planes, extended by
    /// half the neighboring plane spacing (or the plane tolerance for a
    /// single plane).
    fn extent(&self) -> (f64, f64) {
        let first = self.planes[0].offset;
        let last = self.planes[self.planes.len() - 1].offset;
        match self.planes.as_slice() {
            [_] => {
                let tolerance = GeometryTolerances::default().plane_distance_mm;
                (first - tolerance, last + tolerance)
            }
            planes => (
                first - (planes[1].offset - first) / 2.0,
                last + (last - planes[planes.len() - 2].offset) / 2.0,
            ),
        }
    }

    /// The planes a normal position samples, or `None` outside the volume.
    /// Beyond an end plane that plane is used alone.
    fn bracket(&self, distance: f64) -> Option<Bracket> {
        let (lowest, highest) = self.extent();
        if !distance.is_finite() || distance < lowest || distance > highest {
            return None;
        }
        let last = self.planes.len() - 1;
        let single = |position| Bracket {
            low: position,
            high: position,
            weight: 0.0,
        };
        if distance <= self.planes[0].offset {
            return Some(single(0));
        }
        if distance >= self.planes[last].offset {
            return Some(single(last));
        }
        let upper = self
            .planes
            .partition_point(|plane| plane.offset <= distance);
        let (low, high) = (self.planes[upper - 1].offset, self.planes[upper].offset);
        let weight = (distance - low) / (high - low);
        Some(if weight <= SINGLE_PLANE_WEIGHT {
            single(upper - 1)
        } else if weight >= 1.0 - SINGLE_PLANE_WEIGHT {
            single(upper)
        } else {
            Bracket {
                low: upper - 1,
                high: upper,
                weight,
            }
        })
    }
}

impl StackSample {
    /// The stack frames `resample` needs, in the order it takes their values.
    pub fn frames(&self) -> Vec<u32> {
        self.stack.planes[self.planes.clone()]
            .iter()
            .map(|plane| plane.frame)
            .collect()
    }

    pub fn plane_rows(&self) -> u32 {
        self.stack.plane_rows()
    }

    pub fn plane_columns(&self) -> u32 {
        self.stack.plane_columns()
    }

    /// The displayed frame's values in row-major order. `values` holds the
    /// planes of [`Self::frames`] in that order, each row-major in the plane
    /// grid. Pixels outside the volume, or whose sample meets a non-finite
    /// value, are NaN. `None` when a plane does not match the plane grid.
    pub fn resample(&self, values: &[Vec<f64>]) -> Option<Vec<f64>> {
        let plane_len = self.plane_rows() as usize * self.plane_columns() as usize;
        if values.len() != self.planes.len() || values.iter().any(|plane| plane.len() != plane_len)
        {
            return None;
        }
        let plane = |position: usize| {
            position
                .checked_sub(self.planes.start)
                .and_then(|index| values.get(index))
        };
        // A parallel frame brackets the same planes at every pixel.
        let fixed = self
            .parallel
            .then(|| self.stack.bracket(self.normal_origin))
            .flatten();
        let mut output =
            Vec::with_capacity(self.target_rows as usize * self.target_columns as usize);
        for row in 0..self.target_rows {
            for column in 0..self.target_columns {
                let (row, column) = (f64::from(row), f64::from(column));
                let value = self.locate(row, column, fixed).and_then(
                    |([plane_row, plane_column], bracket)| {
                        let sample = |position| {
                            bilinear(
                                plane(position)?,
                                self.plane_rows(),
                                self.plane_columns(),
                                plane_row,
                                plane_column,
                            )
                        };
                        let low = sample(bracket.low)?;
                        if bracket.weight == 0.0 {
                            return Some(low);
                        }
                        Some(low * (1.0 - bracket.weight) + sample(bracket.high)? * bracket.weight)
                    },
                );
                output.push(value.unwrap_or(f64::NAN));
            }
        }
        Some(output)
    }

    fn normal_at(&self, row: f64, column: f64) -> f64 {
        self.normal_origin + row * self.normal_step[0] + column * self.normal_step[1]
    }

    /// A displayed pixel's plane-grid position and bracketing planes, or
    /// `None` outside the volume. `fixed` is the bracket of a parallel frame.
    fn locate(&self, row: f64, column: f64, fixed: Option<Bracket>) -> Option<([f64; 2], Bracket)> {
        let [plane_row, plane_column] = self.transform.map(row, column);
        let max_row = f64::from(self.plane_rows()) - 0.5;
        let max_column = f64::from(self.plane_columns()) - 0.5;
        if !(plane_row >= -0.5
            && plane_row <= max_row
            && plane_column >= -0.5
            && plane_column <= max_column)
        {
            return None;
        }
        let bracket = match fixed {
            Some(bracket) => bracket,
            None => self.stack.bracket(self.normal_at(row, column))?,
        };
        Some(([plane_row, plane_column], bracket))
    }

    /// The stack planes, by position, that the frame's normal positions can
    /// reach; `None` when every position lies beyond the volume. Normal
    /// position is affine in the pixel, so the corners bound it.
    fn plane_range(&self) -> Option<Range<usize>> {
        let (lowest, highest) = self.stack.extent();
        let last_row = f64::from(self.target_rows.saturating_sub(1));
        let last_column = f64::from(self.target_columns.saturating_sub(1));
        let corners = [
            self.normal_at(0.0, 0.0),
            self.normal_at(last_row, 0.0),
            self.normal_at(0.0, last_column),
            self.normal_at(last_row, last_column),
        ];
        let slack = if self.parallel {
            0.0
        } else {
            NORMAL_RANGE_SLACK_MM
        };
        let min = corners.iter().copied().fold(f64::INFINITY, f64::min) - slack;
        let max = corners.iter().copied().fold(f64::NEG_INFINITY, f64::max) + slack;
        if !(min <= highest && max >= lowest) {
            return None;
        }
        let first = self.stack.bracket(min.max(lowest))?;
        let last = self.stack.bracket(max.min(highest))?;
        Some(first.low..last.high + 1)
    }

    /// Whether any displayed pixel center lies inside the volume. Along one
    /// displayed row each stack coordinate is affine in the column, so the
    /// columns inside form an interval; the columns at its ends are then
    /// checked with [`Self::locate`] itself so coverage and resampling agree.
    fn covers_a_pixel(&self) -> bool {
        let (lowest, highest) = self.stack.extent();
        let max_row = f64::from(self.plane_rows()) - 0.5;
        let max_column = f64::from(self.plane_columns()) - 0.5;
        let last_column = f64::from(self.target_columns.saturating_sub(1));
        let step = self.transform.source_step_for_target_column;
        for row in 0..self.target_rows {
            let row = f64::from(row);
            let [plane_row, plane_column] = self.transform.map(row, 0.0);
            let (mut start, mut end) = (0.0_f64, last_column);
            for (origin, slope, low, high) in [
                (plane_row, step[0], -0.5, max_row),
                (plane_column, step[1], -0.5, max_column),
                (
                    self.normal_at(row, 0.0),
                    self.normal_step[1],
                    lowest,
                    highest,
                ),
            ] {
                let (from, to) = if slope == 0.0 {
                    if (low..=high).contains(&origin) {
                        (f64::NEG_INFINITY, f64::INFINITY)
                    } else {
                        (f64::INFINITY, f64::NEG_INFINITY)
                    }
                } else {
                    let (a, b) = ((low - origin) / slope, (high - origin) / slope);
                    (a.min(b), b.max(a))
                };
                start = start.max(from);
                end = end.min(to);
            }
            // Rounding may move an end by a column either way.
            let mut column = (start - 1.0).ceil().max(0.0);
            let end = (end + 1.0).floor().min(last_column);
            while column <= end {
                if self.locate(row, column, None).is_some() {
                    return true;
                }
                column += 1.0;
            }
        }
        false
    }
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

    /// Plane values `100 * frame + 10 * row + column` for the frames a
    /// sample needs, so resampled values show plane and grid positions.
    fn resample(sample: &StackSample) -> Vec<f64> {
        let values = sample
            .frames()
            .into_iter()
            .map(|frame| {
                (0..16)
                    .map(|index| f64::from(100 * frame + 10 * (index / 4) + index % 4))
                    .collect()
            })
            .collect::<Vec<Vec<f64>>>();
        sample
            .resample(&values)
            .expect("plane values match the grid")
    }

    fn at(values: &[f64], row: usize, column: usize) -> f64 {
        values[row * 10 + column]
    }

    #[test]
    fn interpolates_between_bracketing_planes_and_maps_in_plane_pixels() {
        let sample = stack(vec![0.0, 4.0, 8.0])
            .sample(slice(16.0))
            .expect("covered");
        assert_eq!(sample.frames(), [1, 2]);
        // Displayed (2, 4) is plane pixel (1, 2), halfway between frames 1 and 2.
        assert_eq!(at(&resample(&sample), 2, 4), 162.0);

        let exact = stack(vec![0.0, 4.0, 8.0])
            .sample(slice(14.0))
            .expect("covered");
        assert_eq!(exact.frames(), [1]);
        assert_eq!(at(&resample(&exact), 2, 4), 112.0);
    }

    #[test]
    fn absolute_and_descending_offsets_place_the_same_planes() {
        // Absolute offsets: the first equals the grid's own z.
        let absolute = stack(vec![10.0, 14.0, 18.0]);
        assert_eq!(absolute, stack(vec![0.0, 4.0, 8.0]));
        let descending = stack(vec![0.0, -4.0]).sample(slice(7.0)).expect("covered");
        assert_eq!(descending.frames(), [1, 0]);
        // A quarter of the way from frame 1 (z = 6) to frame 0 (z = 10).
        assert_eq!(at(&resample(&descending), 0, 0), 75.0);
    }

    #[test]
    fn covers_half_a_spacing_beyond_the_end_planes_only() {
        let stack = stack(vec![0.0, 4.0, 8.0]);
        assert_eq!(stack.sample(slice(20.0)).unwrap().frames(), [2]);
        assert_eq!(stack.sample(slice(8.0)).unwrap().frames(), [0]);
        assert_eq!(stack.sample(slice(20.5)), Err(StackSampleError::NotCovered));
        assert_eq!(stack.sample(slice(7.5)), Err(StackSampleError::NotCovered));
    }

    #[test]
    fn leaves_pixels_beyond_the_grid_transparent_and_rejects_frames_beside_it() {
        let stack = stack(vec![0.0, 4.0]);
        let values = resample(&stack.sample(slice(12.0)).expect("covered"));
        // Displayed column 7 is plane column 3.5, the grid's edge; 8 is beyond.
        assert!(at(&values, 0, 7).is_finite());
        assert!(at(&values, 0, 8).is_nan());
        let mut beside = slice(12.0);
        beside.position[0] = 100.0;
        assert_eq!(stack.sample(beside), Err(StackSampleError::NotCovered));
    }

    #[test]
    fn samples_a_perpendicular_frame_pixel_by_pixel() {
        // A sagittal frame through x = 0: displayed (row, column) lies at
        // y = 2 * column and z = 12 - 2 * row, so each displayed row has its
        // own position along the stack normal.
        let mut sagittal = slice(12.0);
        sagittal.orientation = [0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
        let sample = stack(vec![0.0, 4.0]).sample(sagittal).expect("covered");
        assert_eq!(sample.frames(), [0, 1]);
        let values = resample(&sample);
        // Row 0 is z = 12, halfway between the planes at z = 10 and 14; row 1
        // is on the first plane; column 2 is plane row 1.
        assert_eq!(at(&values, 0, 2), 60.0);
        assert_eq!(at(&values, 1, 2), 10.0);
        // Row 2 is half a spacing below the first plane; row 3 is beyond it.
        assert_eq!(at(&values, 2, 2), 10.0);
        assert!(at(&values, 3, 2).is_nan());
        // Column 8 is plane row 4, beyond the grid.
        assert!(at(&values, 0, 8).is_nan());
    }

    #[test]
    fn parallel_fast_path_matches_per_pixel_sampling() {
        // A stack tilted about x (row cosines 1,0,0; column cosines
        // 0,0.6,0.8) and a parallel frame rotated 30 degrees within the
        // plane, a few millimeters along the normal: nothing is axis-aligned.
        let orientation = [1.0, 0.0, 0.0, 0.0, 0.6, 0.8];
        let mut geometry = dose_geometry(vec![0.0, 3.0, 6.0]);
        geometry.image_orientation_patient = Some(orientation);
        let stack = PlaneStack::from_dose_grid(4, 4, 3, &geometry).expect("tilted stack");
        let [row, column] = orientation_axes(orientation).unwrap();
        let normal = plane_normal(orientation).unwrap();
        let (sin, cos) = 30_f64.to_radians().sin_cos();
        let rotate = |a: [f64; 3], b: [f64; 3], (x, y): (f64, f64)| {
            [0, 1, 2].map(|axis| a[axis] * x + b[axis] * y)
        };
        let target_row = rotate(row, column, (cos, sin));
        let target_column = rotate(row, column, (-sin, cos));
        let origin = rotate(normal, row, (4.0, 1.0));
        let target = PatientFrameGeometry {
            position: [origin[0], origin[1], origin[2] + 10.0],
            orientation: [
                target_row[0],
                target_row[1],
                target_row[2],
                target_column[0],
                target_column[1],
                target_column[2],
            ],
            pixel_spacing: [1.5, 1.5],
            rows: 10,
            columns: 10,
        };

        let fast = stack.sample(target).expect("covered");
        let per_pixel = stack.sample_frame(target, false).expect("covered");
        assert!(fast.parallel && fast.normal_step == [0.0, 0.0]);
        assert!(!per_pixel.parallel);
        assert_eq!(fast.frames(), [1, 2]);
        assert_eq!(per_pixel.frames(), fast.frames());
        let (fast, per_pixel) = (resample(&fast), resample(&per_pixel));
        assert!(fast.iter().any(|value| value.is_finite()));
        for (fast, per_pixel) in fast.iter().zip(&per_pixel) {
            assert!(
                fast.is_nan() && per_pixel.is_nan() || (fast - per_pixel).abs() < 1.0e-9,
                "{fast} != {per_pixel}"
            );
        }
    }

    fn multiframe(positions: &[[f64; 3]]) -> FileEntry {
        let mut file = crate::loader::test_entry(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm"),
        );
        file.frame_count = positions.len() as u32;
        let metadata = &mut file.series_metadata;
        metadata.frame_image_positions_patient = positions.iter().copied().map(Some).collect();
        metadata.image_orientation_patient = Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        metadata.native_pixel.pixel_spacing = Some([4.0, 4.0]);
        file
    }

    #[test]
    fn frame_stacks_order_planes_by_position_along_the_normal() {
        let file = multiframe(&[[0.0, 0.0, 18.0], [0.0, 0.0, 10.0], [0.0, 0.0, 14.0]]);
        let stack = PlaneStack::from_frames(&file).expect("frame stack");
        let sample = stack.sample(slice(12.0)).expect("covered");
        assert_eq!(sample.frames(), [1, 2]);
        assert_eq!(at(&resample(&sample), 0, 0), 150.0);
    }

    #[test]
    fn frame_stacks_reject_shared_planes_and_in_plane_shifts() {
        let shared = multiframe(&[[0.0, 0.0, 10.0], [0.0, 0.0, 10.0]]);
        assert!(PlaneStack::from_frames(&shared).is_err());
        let shifted = multiframe(&[[0.0, 0.0, 10.0], [1.0, 0.0, 14.0]]);
        assert!(PlaneStack::from_frames(&shifted).is_err());
    }

    #[test]
    fn rejects_offset_vectors_that_do_not_describe_one_plane_per_frame() {
        assert!(PlaneStack::from_dose_grid(4, 4, 3, &dose_geometry(vec![0.0, 4.0])).is_err());
        assert!(PlaneStack::from_dose_grid(4, 4, 2, &dose_geometry(vec![0.0, 0.0])).is_err());
        assert!(PlaneStack::from_dose_grid(4, 4, 1, &dose_geometry(Vec::new())).is_ok());
    }
}
