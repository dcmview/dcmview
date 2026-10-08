//! Shared, short-circuiting validation. Nested checks share one budget.

use crate::geometry::{quantize, Geometry, ImageSize, Mask, TileCoord, TilePayload};
use crate::limits::*;
use crate::{FrameScope, Invalid, Violation, ViolationCode};
use ViolationCode::*;

type Checked = Result<(), ()>;

#[derive(Default)]
struct Checks {
    violations: Vec<Violation>,
}

// A private trait is implemented here so no helper needs public visibility.
macro_rules! checked {
    ($ty:ty, $context:ty, $method:ident) => {
        impl crate::Check<$context> for $ty {
            fn check(&self, context: $context) -> Result<(), Invalid> {
                let mut checks = Checks::default();
                let _ = checks.$method(self, "", context);
                checks.finish()
            }
        }
    };
}
checked!(Geometry, ImageSize, geometry);
checked!(FrameScope, u32, frames);

impl Checks {
    fn finish(self) -> Result<(), Invalid> {
        if self.violations.is_empty() {
            Ok(())
        } else {
            Err(Invalid {
                violations: self.violations,
            })
        }
    }

    fn require(&mut self, valid: bool, code: ViolationCode, path: &str, detail: &str) -> Checked {
        if !valid {
            self.violations.push(Violation {
                code,
                path: path.to_owned(),
                detail: detail.to_owned(),
            });
            if self.violations.len() >= MAX_VIOLATIONS {
                return Err(());
            }
        }
        Ok(())
    }

    // A rejected list is never visited, even when budget remains.
    fn bound(
        &mut self,
        count: usize,
        max: usize,
        code: ViolationCode,
        path: &str,
    ) -> Result<bool, ()> {
        self.require(count <= max, code, path, "The list exceeds its size bound.")?;
        Ok(count <= max)
    }

    fn coordinate(&mut self, value: f64, max: u32, path: &str) -> Checked {
        let value = quantize(value);
        self.require(
            value.is_finite(),
            NonFinite,
            path,
            "The coordinate must be finite.",
        )?;
        if value.is_finite() {
            self.require(
                (0.0..=f64::from(max)).contains(&value),
                OutOfBounds,
                path,
                "The coordinate is outside the image.",
            )?;
        }
        Ok(())
    }

    fn geometry(&mut self, geometry: &Geometry, path: &str, size: ImageSize) -> Checked {
        match geometry {
            Geometry::Point { x, y } => {
                self.coordinate(*x, size.columns, &format!("{path}/x"))?;
                self.coordinate(*y, size.rows, &format!("{path}/y"))?;
            }
            Geometry::Line { points } => self.points(points, path, size)?,
            Geometry::Polyline { points } | Geometry::Polygon { points } => {
                if !self.bound(
                    points.len(),
                    MAX_POINTS,
                    TooManyPoints,
                    &format!("{path}/points"),
                )? {
                    return Ok(());
                }
                let min = if matches!(geometry, Geometry::Polygon { .. }) {
                    3
                } else {
                    2
                };
                self.require(
                    points.len() >= min,
                    TooFewPoints,
                    &format!("{path}/points"),
                    "The shape has too few vertices.",
                )?;
                self.points(points, path, size)?;
            }
            Geometry::Rect { x0, y0, x1, y1 } => {
                for (name, value, max) in [
                    ("x0", x0, size.columns),
                    ("y0", y0, size.rows),
                    ("x1", x1, size.columns),
                    ("y1", y1, size.rows),
                ] {
                    self.coordinate(*value, max, &format!("{path}/{name}"))?;
                }
                self.require(
                    quantize(*x0) < quantize(*x1) && quantize(*y0) < quantize(*y1),
                    Degenerate,
                    path,
                    "The rectangle must have ordered corners and positive area.",
                )?;
            }
            Geometry::Ellipse {
                cx,
                cy,
                rx,
                ry,
                angle,
            } => {
                let (cx, cy, rx, ry, angle) = (
                    quantize(*cx),
                    quantize(*cy),
                    quantize(*rx),
                    quantize(*ry),
                    quantize(*angle),
                );
                for (name, value) in [
                    ("cx", cx),
                    ("cy", cy),
                    ("rx", rx),
                    ("ry", ry),
                    ("angle", angle),
                ] {
                    self.require(
                        value.is_finite(),
                        NonFinite,
                        &format!("{path}/{name}"),
                        "The ellipse number must be finite.",
                    )?;
                }
                self.require(
                    rx > 0.0 && ry > 0.0,
                    Degenerate,
                    path,
                    "The ellipse radii must be positive.",
                )?;
                self.require(
                    (0.0..180.0).contains(&angle),
                    BadAngle,
                    &format!("{path}/angle"),
                    "The angle must be at least zero and below 180 degrees.",
                )?;
                if [cx, cy, rx, ry, angle].iter().all(|v| v.is_finite()) {
                    let (sin, cos) = angle.to_radians().sin_cos();
                    let dx = quantize((rx * cos).hypot(ry * sin));
                    let dy = quantize((rx * sin).hypot(ry * cos));
                    self.require(
                        cx - dx >= 0.0
                            && cx + dx <= f64::from(size.columns)
                            && cy - dy >= 0.0
                            && cy + dy <= f64::from(size.rows),
                        OutOfBounds,
                        path,
                        "The ellipse bounding box is outside the image.",
                    )?;
                }
            }
            Geometry::Mask(mask) => self.mask(mask, path, size)?,
        }
        Ok(())
    }

    fn points(&mut self, points: &[crate::Point], path: &str, size: ImageSize) -> Checked {
        for (i, point) in points.iter().enumerate() {
            self.coordinate(point.x, size.columns, &format!("{path}/points/{i}/x"))?;
            self.coordinate(point.y, size.rows, &format!("{path}/points/{i}/y"))?;
        }
        Ok(())
    }

    fn mask(&mut self, mask: &Mask, path: &str, size: ImageSize) -> Checked {
        self.require(
            mask.tile == 64 && matches!(mask.depth, 1 | 8),
            MaskLayout,
            path,
            "A mask needs 64-pixel tiles and depth 1 or 8.",
        )?;
        self.require(
            !mask.frames.is_empty(),
            MaskEmpty,
            &format!("{path}/frames"),
            "A mask must contain a frame.",
        )?;
        // Counting uses map lengths only; no tile payload is visited first.
        let mut count = 0_usize;
        for tiles in mask.frames.values() {
            if tiles.len() > MAX_MASK_TILES - count {
                self.require(
                    false,
                    TooManyTiles,
                    &format!("{path}/frames"),
                    "The mask exceeds its tile bound.",
                )?;
                return Ok(());
            }
            count += tiles.len();
            // Empty frames also consume validation budget, bounding this pass.
            self.require(
                !tiles.is_empty(),
                MaskEmpty,
                &format!("{path}/frames"),
                "A mask frame must contain a tile.",
            )?;
        }
        for (frame, tiles) in &mask.frames {
            let path = format!("{path}/frames/{}", frame.0);
            self.require(
                frame.0 < size.frames,
                FrameOutOfRange,
                &path,
                "The frame index is outside the file.",
            )?;
            for (coord, payload) in tiles {
                let path = format!("{path}/{},{}", coord.tx, coord.ty);
                self.tile(*coord, mask.tile, size, &path)?;
                self.payload(payload, &path)?;
            }
        }
        Ok(())
    }

    fn tile(&mut self, coord: TileCoord, tile: u32, size: ImageSize, path: &str) -> Checked {
        self.require(
            u64::from(coord.tx) * u64::from(tile) < u64::from(size.columns)
                && u64::from(coord.ty) * u64::from(tile) < u64::from(size.rows),
            MaskTileOutOfBounds,
            path,
            "The tile must start inside the image.",
        )
    }

    fn payload(&mut self, payload: &TilePayload, path: &str) -> Checked {
        let text = &payload.0;
        let valid = !text.is_empty()
            && text.len() <= MAX_TILE_PAYLOAD_CHARS
            && text.len().is_multiple_of(4)
            && {
                let body = text.trim_end_matches('=');
                text.len() - body.len() <= 2
                    && body
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
            };
        self.require(
            valid,
            MaskPayload,
            path,
            "The tile payload must be bounded standard base64 text.",
        )
    }

    fn frames(&mut self, scope: &FrameScope, path: &str, count: u32) -> Checked {
        let FrameScope::Set(frames) = scope else {
            return Ok(());
        };
        let set_path = format!("{path}/set");
        let bounded = self.bound(
            frames.set.len(),
            MAX_FRAMES_IN_SET,
            TooManyFrames,
            &set_path,
        )?;
        if bounded {
            self.require(
                !frames.set.is_empty(),
                FramesEmpty,
                &set_path,
                "An explicit frame set must not be empty.",
            )?;
            let mut previous = None;
            for (i, frame) in frames.set.iter().enumerate() {
                self.require(
                    previous.is_none_or(|p| p < *frame),
                    FramesNotNormalized,
                    &format!("{set_path}/{i}"),
                    "Frame indices must be strictly ascending.",
                )?;
                self.require(
                    *frame < count,
                    FrameOutOfRange,
                    &format!("{set_path}/{i}"),
                    "The frame index is outside the file.",
                )?;
                previous = Some(*frame);
            }
        }
        if let Some(written) = &frames.as_written {
            let path = format!("{path}/as_written");
            if self.bound(written.len(), MAX_FRAMES_IN_SET, TooManyFrames, &path)? && bounded {
                let mut normalized = written.clone();
                normalized.sort_unstable();
                normalized.dedup();
                self.require(
                    normalized == frames.set,
                    FramesAsWrittenMismatch,
                    &path,
                    "The original list must name exactly the frame set.",
                )?;
            }
        }
        Ok(())
    }
}
