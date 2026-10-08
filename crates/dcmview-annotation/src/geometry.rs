//! Geometry: coordinates, the seven geometry types and their invariants.
//!
//! # Coordinates (`docs/design/annotation-model.md` 2)
//!
//! Corner origin, continuous, in the stored pixel grid: `x` is the column and
//! `y` the row, the top-left corner of the top-left pixel is `(0, 0)` and its
//! centre `(0.5, 0.5)`. The valid range is `[0, columns]` by `[0, rows]`, both
//! ends included, so a rectangle over whole pixels has integer corners and may
//! end on the far edge. Never millimetres and never display orientation.
//!
//! Coordinates are quantized to 1/1000 px ([`quantize`]). On the wire every
//! geometry number (an ellipse's radii and angle included) is written
//! quantized, with at most three decimals, and a whole number is written
//! without a fraction (`340`, never `340.0`).
//!
//! # Records that break the invariants
//!
//! Deserialization checks shape only: a geometry whose numbers break the
//! invariants below (a rectangle past the image edge, with no area, or with
//! its corners in the wrong order) still deserializes and still serializes
//! unchanged. That is on purpose. The lenient EMBED CSV import keeps such
//! rectangles exactly as they were written until they are edited
//! (`docs/design/annotation-model.md` 2.1, 9.1). [`Geometry::validate`] is the
//! strict check that every write goes through.

use crate::frames::FrameScope;
use crate::number::serialize_coordinate;
use crate::validate::Invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// Number of quantization steps in one pixel: coordinates are multiples of
/// 1/1000 px once quantized.
pub const QUANTA_PER_PIXEL: f64 = 1000.0;

/// The dimensions a geometry or a frame scope is checked against: one stored
/// pixel grid of `rows` by `columns`, the same for each of `frames` frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageSize {
    pub columns: u32,
    pub rows: u32,
    pub frames: u32,
}

/// Rounds a coordinate to the nearest 1/1000 px, halves away from zero, and
/// returns positive zero for anything that rounds to zero.
///
/// A value is returned unchanged when it is not finite, and when it is too
/// large to be scaled: `value * 1000` is infinite for a magnitude above about
/// 1.8e305, and such a value has no fraction to round. So a finite value
/// stays finite: a coordinate that can be read can be written, and
/// [`Geometry::validate`] reports it as `out_of_bounds`, not `non_finite`.
///
/// `quantize(12.3456) == 12.346`, `quantize(-0.0004) == 0.0` (positive),
/// `quantize(1e306) == 1e306`.
pub fn quantize(value: f64) -> f64 {
    let scaled = value * QUANTA_PER_PIXEL;
    if !scaled.is_finite() {
        return value;
    }
    let rounded = scaled.round() / QUANTA_PER_PIXEL;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// One position: `x` is the column, `y` the row.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Point {
    #[serde(serialize_with = "serialize_coordinate")]
    pub x: f64,
    #[serde(serialize_with = "serialize_coordinate")]
    pub y: f64,
}

/// The name of a geometry type, as a class lists the types it allows and as
/// the `type` member of a serialized [`Geometry`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum GeometryType {
    Point,
    Line,
    Polyline,
    Polygon,
    Rect,
    Ellipse,
    Mask,
}

impl GeometryType {
    /// Every geometry type, in the order the design lists them.
    pub const ALL: [GeometryType; 7] = [
        GeometryType::Point,
        GeometryType::Line,
        GeometryType::Polyline,
        GeometryType::Polygon,
        GeometryType::Rect,
        GeometryType::Ellipse,
        GeometryType::Mask,
    ];
}

/// The shape of one annotation (`docs/design/annotation-model.md` 3.1).
///
/// Serialized as an object whose `type` member names the variant, for example
/// `{ "type": "rect", "x0": 340, "y0": 120, "x1": 430, "y1": 220 }`.
///
/// Invariants, checked by [`Geometry::validate`] and not by deserialization:
///
/// | Type | Invariant |
/// |---|---|
/// | all vector types | every number is finite; every position lies in `[0, columns]` by `[0, rows]` |
/// | `polyline` | 2 to 100,000 points |
/// | `polygon` | 3 to 100,000 points; implicitly closed; self-intersection is not checked |
/// | `rect` | `x0 < x1` and `y0 < y1` |
/// | `ellipse` | `rx > 0`, `ry > 0`, `0 <= angle < 180`, and its axis-aligned bounding box lies in the image |
/// | `mask` | see [`Mask`] |
///
/// The model is x first. EMBED's `[ymin, xmin, ymax, xmax]` and COCO's
/// `[x, y, w, h]` are reordered by their adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    Point {
        #[serde(serialize_with = "serialize_coordinate")]
        x: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        y: f64,
    },
    /// A segment from `points[0]` to `points[1]`.
    Line {
        points: [Point; 2],
    },
    /// An open chain of segments.
    Polyline {
        points: Vec<Point>,
    },
    /// A closed outline; the last point joins the first.
    Polygon {
        points: Vec<Point>,
    },
    /// An axis-aligned rectangle from the corner `(x0, y0)` to `(x1, y1)`.
    Rect {
        #[serde(serialize_with = "serialize_coordinate")]
        x0: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        y0: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        x1: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        y1: f64,
    },
    /// An ellipse centred on `(cx, cy)` with radius `rx` along its first axis
    /// and `ry` along its second. `angle` is the rotation of the first axis
    /// from the x axis towards the y axis, in degrees; the tools of the first
    /// release always write 0. A circle is an ellipse with `rx == ry`.
    Ellipse {
        #[serde(serialize_with = "serialize_coordinate")]
        cx: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        cy: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        rx: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        ry: f64,
        #[serde(serialize_with = "serialize_coordinate")]
        angle: f64,
    },
    Mask(Mask),
}

/// What [`Geometry::clamped`] did to a geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Clamped {
    /// The geometry after rounding and clamping.
    pub geometry: Geometry,
    /// At least one number was not on the grid the snap asked for and was
    /// rounded onto it.
    pub rounded: bool,
    /// At least one number lay outside the image, after rounding, and was
    /// moved onto its nearest edge.
    pub moved: bool,
}

/// The grid [`Geometry::clamped`] rounds onto before it clamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    /// Whole pixel edges: every number is rounded to the nearest integer,
    /// halves away from zero. What the EMBED CSV import uses.
    PixelEdges,
    /// The model's own grid: every number is [`quantize`]d.
    Quantum,
}

impl Geometry {
    /// Which geometry type this is.
    pub fn geometry_type(&self) -> GeometryType {
        match self {
            Geometry::Point { .. } => GeometryType::Point,
            Geometry::Line { .. } => GeometryType::Line,
            Geometry::Polyline { .. } => GeometryType::Polyline,
            Geometry::Polygon { .. } => GeometryType::Polygon,
            Geometry::Rect { .. } => GeometryType::Rect,
            Geometry::Ellipse { .. } => GeometryType::Ellipse,
            Geometry::Mask(_) => GeometryType::Mask,
        }
    }

    /// The same geometry with every number [`quantize`]d. A mask is returned
    /// unchanged. This is what a store applies when it commits a write.
    pub fn quantized(&self) -> Geometry {
        map_numbers(self, |value, _| quantize(value))
    }

    /// Checks the invariants in the type's documentation against an image of
    /// `size`. The frame count is used for a mask's frames only.
    ///
    /// Every comparison is made on [`quantize`]d values, so a position within
    /// half a quantum of an edge is on it, and a rectangle whose width
    /// quantizes to nothing has no area. An ellipse's bounding box has the half
    /// extents `sqrt(rx² cos² a + ry² sin² a)` along x and
    /// `sqrt(rx² sin² a + ry² cos² a)` along y for its angle `a`, quantized
    /// before they are compared with the image.
    ///
    /// Bounded work: at most [`crate::limits::MAX_POINTS`] points and
    /// [`crate::limits::MAX_MASK_TILES`] tiles are looked at; a geometry with
    /// more is refused on its count alone.
    ///
    /// Violation codes: `non_finite`, `out_of_bounds`, `degenerate` (a
    /// rectangle with no area or with corners out of order, an ellipse radius
    /// that is not positive), `bad_angle`, `too_few_points`,
    /// `too_many_points`, and the mask codes listed on [`Mask`].
    pub fn validate(&self, size: ImageSize) -> Result<(), Invalid> {
        crate::Check::check(self, size)
    }

    /// Rounds the geometry onto `snap`'s grid, then moves every position that
    /// is still outside the image onto the nearest edge: a negative value
    /// becomes 0, an `x` past `columns` becomes `columns`, a `y` past `rows`
    /// becomes `rows`.
    ///
    /// Each number is treated on its own, so the result keeps the order of a
    /// rectangle's corners and may have no area; the caller decides what a
    /// box with no area means. Applies to `point`, `line`, `polyline`,
    /// `polygon` and `rect`. An `ellipse` and a `mask` are returned unchanged
    /// with both flags false. Numbers that are not finite are left as they
    /// are, set neither flag, and are reported by [`Geometry::validate`]. A
    /// result of zero is positive zero.
    ///
    /// Owner decision, EMBED parity amendment of 2026-10-05: a coordinate
    /// that is negative or not an integer is clamped with a warning rather
    /// than failing the import; it is rounded first, then clamped, and the
    /// row is reported. `rounded` and `moved` are what the report needs.
    pub fn clamped(&self, size: ImageSize, snap: Snap) -> Clamped {
        let mut rounded = false;
        let mut moved = false;
        let geometry = match self {
            Self::Ellipse { .. } | Self::Mask(_) => self.clone(),
            _ => map_numbers(self, |value, x| {
                if !value.is_finite() {
                    return value;
                }
                let snapped = match snap {
                    Snap::PixelEdges => value.round(),
                    Snap::Quantum => quantize(value),
                };
                rounded |= snapped != value;
                let clamped =
                    snapped.clamp(0.0, f64::from(if x { size.columns } else { size.rows }));
                moved |= clamped != snapped;
                if clamped == 0.0 {
                    0.0
                } else {
                    clamped
                }
            }),
        };
        Clamped {
            geometry,
            rounded,
            moved,
        }
    }
}

/// The encoding of a mask's tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub enum MaskEncoding {
    /// Sparse square tiles; each stored tile is the base64 text of the
    /// tile's deflated pixels (`docs/design/annotation-model.md` 3.2).
    #[serde(rename = "tiles-v1")]
    TilesV1,
}

/// A tile's position in the tile grid: column `tx` and row `ty`, written on
/// the wire as the string `"tx,ty"` in decimal with no spaces and no leading
/// zeros (it is an object key). Tile `(tx, ty)` covers the pixels
/// `[tx * tile, (tx + 1) * tile)` by `[ty * tile, (ty + 1) * tile)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(type = "string")]
pub struct TileCoord {
    pub tx: u32,
    pub ty: u32,
}

impl TryFrom<String> for TileCoord {
    type Error = crate::key::InvalidValue;

    /// Parses `"tx,ty"`. Fails for anything but two decimal `u32`s without
    /// sign, space or leading zero, joined by one comma; a string longer
    /// than 21 bytes is refused on its length.
    fn try_from(text: String) -> Result<Self, Self::Error> {
        if text.len() <= 21 {
            if let Some((x, y)) = text.split_once(',') {
                if let (Some(tx), Some(ty)) = (decimal_index(x), decimal_index(y)) {
                    return Ok(Self { tx, ty });
                }
            }
        }
        Err(crate::key::InvalidValue {
            kind: "tile coordinate",
            reason: "Expected two canonical decimal indices.".to_owned(),
        })
    }
}

impl From<TileCoord> for String {
    fn from(coord: TileCoord) -> String {
        format!("{},{}", coord.tx, coord.ty)
    }
}

impl JsonSchema for TileCoord {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TileCoord".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^(0|[1-9][0-9]*),(0|[1-9][0-9]*)$"
        })
    }
}

/// A frame index as an object key: the zero-based index in decimal, with no
/// sign, space or leading zero (`"0"`, `"12"`), at most ten digits and at
/// most `u32::MAX`. A string, because JSON object keys are strings and a
/// mask is read through serde's buffered path, which does not turn a key
/// back into a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(type = "string")]
pub struct FrameIndex(pub u32);

impl TryFrom<String> for FrameIndex {
    type Error = crate::key::InvalidValue;

    /// Parses a decimal `u32` without sign, space or leading zero.
    fn try_from(text: String) -> Result<Self, Self::Error> {
        decimal_index(&text)
            .map(Self)
            .ok_or_else(|| crate::key::InvalidValue {
                kind: "frame index",
                reason: "Expected a canonical decimal index.".to_owned(),
            })
    }
}

impl From<FrameIndex> for String {
    fn from(index: FrameIndex) -> String {
        index.0.to_string()
    }
}

impl JsonSchema for FrameIndex {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FrameIndex".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^(0|[1-9][0-9]*)$" })
    }
}

/// One tile's pixels as base64 text. This crate treats the text as opaque:
/// it bounds its length and checks its alphabet, and leaves decoding to the
/// mask tools.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(transparent)]
pub struct TilePayload(pub String);

/// One segment painted as sparse tiles (`docs/design/annotation-model.md`
/// 3.2), for example
/// `{ "type": "mask", "encoding": "tiles-v1", "tile": 64, "depth": 1,
/// "frames": { "0": { "5,3": "<base64>" } } }`.
///
/// `frames` maps a frame index to that frame's stored tiles. The frames with
/// tiles are the mask's frame set, so the `frames` member of the annotation
/// that holds a mask must be exactly that set.
///
/// Invariants, checked by [`Geometry::validate`]:
///
/// - `tile` is 64 and `depth` is 1 or 8 (`mask_layout`). Depth 8 is reserved
///   for model outputs; the tools write 1.
/// - There is at least one frame, and no frame has an empty tile map
///   (`mask_empty`).
/// - Every frame index is below the file's frame count
///   (`frame_out_of_range`).
/// - Every tile starts inside the image: `tx * tile < columns` and
///   `ty * tile < rows` (`mask_tile_out_of_bounds`).
/// - At most 262,144 tiles in total ([`crate::limits::MAX_MASK_TILES`],
///   `too_many_tiles`).
/// - Every payload is 1 to 8,192 characters
///   ([`crate::limits::MAX_TILE_PAYLOAD_CHARS`]) of padded standard base64:
///   a multiple of four characters from `A`-`Z`, `a`-`z`, `0`-`9`, `+` and
///   `/`, the last one or two of which may be `=` (`mask_payload`). The
///   payload is not decoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Mask {
    pub encoding: MaskEncoding,
    /// Tile edge in pixels.
    pub tile: u32,
    /// Bits per pixel.
    pub depth: u8,
    pub frames: BTreeMap<FrameIndex, BTreeMap<TileCoord, TilePayload>>,
}

impl Mask {
    /// The frames that hold tiles, as the frame scope the annotation must
    /// carry.
    pub fn frame_scope(&self) -> FrameScope {
        FrameScope::set(self.frames.keys().map(|frame| frame.0).collect())
    }
}

fn decimal_index(text: &str) -> Option<u32> {
    if text.is_empty()
        || text.len() > 10
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    text.parse().ok()
}

fn map_numbers(geometry: &Geometry, mut map: impl FnMut(f64, bool) -> f64) -> Geometry {
    match geometry {
        Geometry::Point { x, y } => Geometry::Point {
            x: map(*x, true),
            y: map(*y, false),
        },
        Geometry::Rect { x0, y0, x1, y1 } => Geometry::Rect {
            x0: map(*x0, true),
            y0: map(*y0, false),
            x1: map(*x1, true),
            y1: map(*y1, false),
        },
        Geometry::Ellipse {
            cx,
            cy,
            rx,
            ry,
            angle,
        } => Geometry::Ellipse {
            cx: map(*cx, true),
            cy: map(*cy, false),
            rx: map(*rx, true),
            ry: map(*ry, false),
            angle: map(*angle, false),
        },
        Geometry::Line { points } => Geometry::Line {
            points: points.map(|p| Point {
                x: map(p.x, true),
                y: map(p.y, false),
            }),
        },
        Geometry::Polyline { points } => Geometry::Polyline {
            points: points
                .iter()
                .map(|p| Point {
                    x: map(p.x, true),
                    y: map(p.y, false),
                })
                .collect(),
        },
        Geometry::Polygon { points } => Geometry::Polygon {
            points: points
                .iter()
                .map(|p| Point {
                    x: map(p.x, true),
                    y: map(p.y, false),
                })
                .collect(),
        },
        Geometry::Mask(mask) => Geometry::Mask(mask.clone()),
    }
}
