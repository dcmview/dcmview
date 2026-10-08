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

use crate::{
    Annotation, ClassDef, Code, Context, FieldDef, FieldType, FileRef, FileSizes, GeometryType,
    KeyScheme, Label, LabelSchema, LabelTarget, LabelValue, Layer, LayerKind, OptionDef,
    RecordMeta, TargetKind,
};
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

// One call owns these indexes. Callers check each list's bound before asking
// for its index; constructing the context alone visits no schema items.
struct IndexedContext<'a> {
    files: &'a dyn FileSizes,
    schema: &'a LabelSchema,
    fields: OnceCell<HashMap<&'a str, (usize, &'a FieldDef)>>,
    classes: OnceCell<HashMap<&'a str, (usize, &'a ClassDef)>>,
    options: RefCell<HashMap<usize, HashSet<&'a str>>>,
    targets: RefCell<HashMap<usize, HashSet<TargetKind>>>,
    attributes: RefCell<HashMap<usize, HashSet<&'a str>>>,
    geometries: RefCell<HashMap<usize, HashSet<GeometryType>>>,
}

impl<'a> IndexedContext<'a> {
    fn new(context: &Context<'a>) -> Self {
        Self {
            files: context.files,
            schema: context.schema,
            fields: OnceCell::new(),
            classes: OnceCell::new(),
            options: RefCell::new(HashMap::new()),
            targets: RefCell::new(HashMap::new()),
            attributes: RefCell::new(HashMap::new()),
            geometries: RefCell::new(HashMap::new()),
        }
    }

    fn field(&self, id: &str) -> Option<(usize, &'a FieldDef)> {
        self.fields
            .get_or_init(|| {
                let mut fields = HashMap::new();
                for (position, field) in self.schema.fields.iter().enumerate() {
                    fields.entry(field.id.as_str()).or_insert((position, field));
                }
                fields
            })
            .get(id)
            .copied()
    }

    fn class(&self, id: &str) -> Option<(usize, &'a ClassDef)> {
        self.classes
            .get_or_init(|| {
                let mut classes = HashMap::new();
                for (position, class) in self.schema.classes.iter().enumerate() {
                    classes
                        .entry(class.id.as_str())
                        .or_insert((position, class));
                }
                classes
            })
            .get(id)
            .copied()
    }

    fn has_option(&self, position: usize, options: &'a [OptionDef], id: &str) -> bool {
        self.options
            .borrow_mut()
            .entry(position)
            .or_insert_with(|| options.iter().map(|option| option.id.as_str()).collect())
            .contains(id)
    }

    fn has_target(&self, position: usize, field: &FieldDef, kind: TargetKind) -> bool {
        self.targets
            .borrow_mut()
            .entry(position)
            .or_insert_with(|| field.applies_to.iter().copied().collect())
            .contains(&kind)
    }

    fn has_attribute(&self, position: usize, class: &'a ClassDef, id: &str) -> bool {
        self.attributes
            .borrow_mut()
            .entry(position)
            .or_insert_with(|| class.attributes.iter().map(String::as_str).collect())
            .contains(id)
    }

    fn has_geometry(&self, position: usize, class: &ClassDef, kind: GeometryType) -> bool {
        self.geometries
            .borrow_mut()
            .entry(position)
            .or_insert_with(|| class.geometry.iter().copied().collect())
            .contains(&kind)
    }
}

macro_rules! checked_with_schema {
    ($ty:ty, $method:ident) => {
        impl crate::Check<&Context<'_>> for $ty {
            fn check(&self, context: &Context<'_>) -> Result<(), Invalid> {
                let context = IndexedContext::new(context);
                let mut checks = Checks::default();
                let _ = checks.$method(self, "", &context);
                checks.finish()
            }
        }
    };
}

checked!(LabelSchema, (), schema);
checked!(FileRef, (), file);
checked!(Layer, (), layer);
checked!(LabelTarget, (), target);
checked_with_schema!(Annotation, annotation);
checked_with_schema!(Label, label);

fn id_syntax(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ID_BYTES
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn pointer_member(text: &str) -> String {
    text.replace('~', "~0").replace('/', "~1")
}

fn digest_syntax(text: &str, schemes: &[&str]) -> bool {
    text.len() <= 71
        && text.split_once(':').is_some_and(|(scheme, body)| {
            schemes.contains(&scheme)
                && body.len() == 64
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

impl Checks {
    fn name(&mut self, text: &str, path: &str) -> Checked {
        self.require(
            text.len() <= MAX_NAME_BYTES,
            TooLong,
            path,
            "The name exceeds its byte bound.",
        )
    }

    fn id(&mut self, text: &str, path: &str) -> Checked {
        self.require(
            id_syntax(text),
            BadId,
            path,
            "The identifier must use the bounded identifier syntax.",
        )
    }

    fn color(&mut self, text: &str, path: &str) -> Checked {
        self.require(
            text.len() == 7
                && text.starts_with('#')
                && text.bytes().skip(1).all(|b| b.is_ascii_hexdigit()),
            BadColor,
            path,
            "The color must be # followed by six hex digits.",
        )
    }

    fn code(&mut self, code: &Option<Code>, path: &str) -> Checked {
        if let Some(code) = code {
            self.name(&code.scheme, &format!("{path}/scheme"))?;
            self.name(&code.value, &format!("{path}/value"))?;
            self.name(&code.meaning, &format!("{path}/meaning"))?;
        }
        Ok(())
    }

    fn schema(&mut self, schema: &LabelSchema, path: &str, _: ()) -> Checked {
        self.id(&schema.schema_id, &format!("{path}/schema_id"))?;
        let fields_bounded = self.bound(
            schema.fields.len(),
            MAX_SCHEMA_ITEMS,
            TooManyItems,
            &format!("{path}/fields"),
        )?;
        let mut fields = HashSet::new();
        if fields_bounded {
            for (i, field) in schema.fields.iter().enumerate() {
                let path = format!("{path}/fields/{i}");
                self.id(&field.id, &format!("{path}/id"))?;
                if field.id.len() <= MAX_ID_BYTES {
                    self.require(
                        fields.insert(field.id.as_str()),
                        DuplicateId,
                        &format!("{path}/id"),
                        "Field identifiers must be distinct.",
                    )?;
                }
                self.name(&field.name, &format!("{path}/name"))?;
                self.bound(
                    field.applies_to.len(),
                    MAX_SCHEMA_ITEMS,
                    TooManyItems,
                    &format!("{path}/applies_to"),
                )?;
                match &field.field_type {
                    FieldType::Category { options, .. } | FieldType::MultiCategory { options } => {
                        let path = format!("{path}/options");
                        if !self.bound(options.len(), MAX_SCHEMA_ITEMS, TooManyItems, &path)? {
                            continue;
                        }
                        self.require(
                            !options.is_empty(),
                            BadSchema,
                            &path,
                            "A categorical field needs an option.",
                        )?;
                        let mut ids = HashSet::new();
                        for (i, option) in options.iter().enumerate() {
                            let path = format!("{path}/{i}");
                            self.id(&option.id, &format!("{path}/id"))?;
                            if option.id.len() <= MAX_ID_BYTES {
                                self.require(
                                    ids.insert(option.id.as_str()),
                                    DuplicateId,
                                    &format!("{path}/id"),
                                    "Option identifiers must be distinct within a field.",
                                )?;
                            }
                            self.name(&option.name, &format!("{path}/name"))?;
                            self.code(&option.code, &format!("{path}/code"))?;
                        }
                    }
                    FieldType::Number { min, max, unit, .. } => {
                        self.require(
                            min.is_none_or(f64::is_finite)
                                && max.is_none_or(f64::is_finite)
                                && !matches!((min, max), (Some(min), Some(max)) if min > max),
                            BadSchema,
                            &path,
                            "Numeric bounds must be finite and ordered.",
                        )?;
                        if let Some(unit) = unit {
                            self.name(unit, &format!("{path}/unit"))?;
                        }
                    }
                    _ => {}
                }
            }
        }
        if self.bound(
            schema.classes.len(),
            MAX_SCHEMA_ITEMS,
            TooManyItems,
            &format!("{path}/classes"),
        )? {
            let mut ids = HashSet::new();
            for (i, class) in schema.classes.iter().enumerate() {
                let path = format!("{path}/classes/{i}");
                self.id(&class.id, &format!("{path}/id"))?;
                if class.id.len() <= MAX_ID_BYTES {
                    self.require(
                        ids.insert(class.id.as_str()),
                        DuplicateId,
                        &format!("{path}/id"),
                        "Class identifiers must be distinct.",
                    )?;
                }
                self.name(&class.name, &format!("{path}/name"))?;
                if let Some(color) = &class.color {
                    self.color(color, &format!("{path}/color"))?;
                }
                self.code(&class.code, &format!("{path}/code"))?;
                self.bound(
                    class.geometry.len(),
                    MAX_SCHEMA_ITEMS,
                    TooManyItems,
                    &format!("{path}/geometry"),
                )?;
                self.require(
                    !class.geometry.is_empty(),
                    BadSchema,
                    &format!("{path}/geometry"),
                    "A class must allow a geometry type.",
                )?;
                if self.bound(
                    class.attributes.len(),
                    MAX_SCHEMA_ITEMS,
                    TooManyItems,
                    &format!("{path}/attributes"),
                )? {
                    for (i, attribute) in class.attributes.iter().enumerate() {
                        self.id(attribute, &format!("{path}/attributes/{i}"))?;
                        if fields_bounded {
                            self.require(
                                attribute.len() <= MAX_ID_BYTES
                                    && fields.contains(attribute.as_str()),
                                UnknownField,
                                &format!("{path}/attributes/{i}"),
                                "The attribute must name a schema field.",
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn file(&mut self, file: &FileRef, path: &str, _: ()) -> Checked {
        for (name, dimension) in [("rows", file.rows), ("columns", file.columns)] {
            self.require(
                dimension <= MAX_IMAGE_DIMENSION,
                BadFileRef,
                &format!("{path}/{name}"),
                "The image dimension exceeds its bound.",
            )?;
        }
        self.require(
            !file.path.is_empty(),
            BadFileRef,
            &format!("{path}/path"),
            "The file path must not be empty.",
        )?;
        self.require(
            file.path.len() <= MAX_PATH_BYTES,
            TooLong,
            &format!("{path}/path"),
            "The path exceeds its byte bound.",
        )?;
        for (name, text) in [
            ("sop_instance_uid", &file.sop_instance_uid),
            ("sop_class_uid", &file.sop_class_uid),
            ("study_instance_uid", &file.study_instance_uid),
            ("series_instance_uid", &file.series_instance_uid),
            ("patient_id", &file.patient_id),
            ("external_id", &file.external_id),
        ] {
            if let Some(text) = text {
                self.name(text, &format!("{path}/{name}"))?;
            }
        }
        self.name(&file.format, &format!("{path}/format"))?;
        if file.key.scheme() == KeyScheme::Sop {
            self.require(
                file.sop_instance_uid.as_deref() == Some(file.key.body()),
                BadFileRef,
                &format!("{path}/sop_instance_uid"),
                "The SOP key must name the file's instance UID.",
            )?;
        }
        if let Some(digest) = &file.digest {
            self.require(
                digest_syntax(digest, &["b3", "sha256"]),
                BadDigest,
                &format!("{path}/digest"),
                "The file digest must be a full lowercase digest with a known scheme.",
            )?;
        }
        if let Some(orientation) = file.space.exif_orientation {
            self.require(
                (1..=8).contains(&orientation),
                BadFileRef,
                &format!("{path}/space/exif_orientation"),
                "The EXIF orientation must be between 1 and 8.",
            )?;
        }
        if self.frame_list(
            file.pixel_digest.len(),
            file.frames,
            &format!("{path}/pixel_digest"),
        )? {
            for (i, digest) in file.pixel_digest.iter().enumerate() {
                if let Some(digest) = digest {
                    self.require(
                        digest_syntax(digest, &["px"]),
                        BadDigest,
                        &format!("{path}/pixel_digest/{i}"),
                        "A pixel digest must be a full lowercase px digest.",
                    )?;
                }
            }
        }
        if let Some(source) = &file.frame_source {
            self.frame_list(source.len(), file.frames, &format!("{path}/frame_source"))?;
        }
        if let Some(spacing) = &file.spacing {
            if self.frame_list(spacing.len(), file.frames, &format!("{path}/spacing"))? {
                for (i, spacing) in spacing.iter().enumerate() {
                    if let Some(spacing) = spacing {
                        let path = format!("{path}/spacing/{i}");
                        for (name, value) in
                            [("row_mm", spacing.row_mm), ("col_mm", spacing.col_mm)]
                        {
                            self.require(
                                value.is_finite() && value > 0.0,
                                BadFileRef,
                                &format!("{path}/{name}"),
                                "Spacing must be finite and positive.",
                            )?;
                        }
                        self.name(&spacing.source, &format!("{path}/source"))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn frame_list(&mut self, len: usize, frames: u32, path: &str) -> Result<bool, ()> {
        let valid = len == 0 || len as u64 == u64::from(frames);
        self.require(
            valid,
            BadFileRef,
            path,
            "A nonempty per-frame list must match the frame count.",
        )?;
        Ok(valid)
    }

    fn layer_name(&mut self, name: &str, path: &str) -> Checked {
        self.name(name, path)?;
        self.require(
            !name.is_empty(),
            BadLayer,
            path,
            "The layer name must not be empty.",
        )
    }

    fn layer(&mut self, layer: &Layer, path: &str, _: ()) -> Checked {
        self.layer_name(&layer.name, &format!("{path}/name"))?;
        if let Some(color) = &layer.color {
            self.color(color, &format!("{path}/color"))?;
        }
        self.require(
            layer.kind != LayerKind::Review || layer.readonly,
            BadLayer,
            &format!("{path}/readonly"),
            "A review layer must be read-only.",
        )
    }

    fn target(&mut self, target: &LabelTarget, path: &str, _: ()) -> Checked {
        match target {
            LabelTarget::Patient { patient: id }
            | LabelTarget::Study { study: id }
            | LabelTarget::Series { series: id } => {
                let member = match target {
                    LabelTarget::Patient { .. } => "patient",
                    LabelTarget::Study { .. } => "study",
                    _ => "series",
                };
                self.require(
                    !id.is_empty()
                        && id.len() <= MAX_NAME_BYTES
                        && !id.chars().any(char::is_control),
                    BadTarget,
                    &format!("{path}/{member}"),
                    "The target identifier must be nonempty, bounded and free of controls.",
                )?;
            }
            LabelTarget::Folder { folder, root } => {
                self.require(
                    id_syntax(root),
                    BadTarget,
                    &format!("{path}/root"),
                    "The folder root must use the identifier syntax.",
                )?;
                self.require(
                    folder.len() <= MAX_PATH_BYTES
                        && !folder.chars().any(|c| c == '\\' || c.is_control())
                        && (folder.is_empty()
                            || folder
                                .split('/')
                                .all(|part| !matches!(part, "" | "." | ".."))),
                    BadTarget,
                    &format!("{path}/folder"),
                    "The folder must be a bounded relative path without empty or dot segments.",
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn uuid(&mut self, id: Uuid, path: &str) -> Checked {
        self.require(
            id.get_version_num() == 7 && id.get_variant() == uuid::Variant::RFC4122,
            IdNotUuidV7,
            path,
            "The identifier must be an RFC 4122 UUIDv7.",
        )
    }

    fn meta(&mut self, meta: &RecordMeta, path: &str) -> Checked {
        if let Some(id) = meta.derived_from {
            self.uuid(id, &format!("{path}/derived_from"))?;
        }
        if let Some(score) = meta.score {
            self.require(
                score.is_finite() && (0.0..=1.0).contains(&score),
                BadScore,
                &format!("{path}/score"),
                "The score must be finite and between zero and one.",
            )?;
        }
        Ok(())
    }

    fn field<'a>(
        &mut self,
        id: &str,
        context: &IndexedContext<'a>,
        path: &str,
    ) -> Result<Option<(usize, &'a FieldDef)>, ()> {
        if !self.bound(
            context.schema.fields.len(),
            MAX_SCHEMA_ITEMS,
            TooManyItems,
            path,
        )? {
            return Ok(None);
        }
        let field = if id.len() <= MAX_ID_BYTES {
            context.field(id)
        } else {
            None
        };
        self.require(
            field.is_some(),
            UnknownField,
            path,
            "The field must exist in the schema.",
        )?;
        Ok(field)
    }

    fn value<'a>(
        &mut self,
        value: &LabelValue,
        (position, field): (usize, &'a FieldDef),
        path: &str,
        context: &IndexedContext<'a>,
    ) -> Checked {
        match (&field.field_type, value) {
            (FieldType::Boolean, LabelValue::Bool(_)) => {}
            (
                FieldType::Number {
                    min, max, integer, ..
                },
                LabelValue::Number(n),
            ) => {
                self.require(
                    n.is_finite()
                        && min.is_none_or(|min| *n >= min)
                        && max.is_none_or(|max| *n <= max)
                        && (!integer || n.fract() == 0.0),
                    ValueOutOfRange,
                    path,
                    "The number must satisfy the field's range and integer rule.",
                )?;
            }
            (FieldType::Text { max_length }, LabelValue::Text(text)) => {
                let max =
                    max_length.map_or(MAX_TEXT_BYTES, |max| (max as usize).min(MAX_TEXT_BYTES));
                self.require(
                    text.len() <= max,
                    TooLong,
                    path,
                    "The text exceeds the field's byte bound.",
                )?;
            }
            (FieldType::Category { options, .. }, LabelValue::Text(id)) => {
                if self.bound(options.len(), MAX_SCHEMA_ITEMS, TooManyItems, path)? {
                    self.require(
                        id.len() <= MAX_ID_BYTES && context.has_option(position, options, id),
                        UnknownOption,
                        path,
                        "The option must exist in the field.",
                    )?;
                }
            }
            (FieldType::MultiCategory { options }, LabelValue::Many(ids)) => {
                if !self.bound(ids.len(), MAX_VALUES, TooManyItems, path)? {
                    return Ok(());
                }
                if !self.bound(options.len(), MAX_SCHEMA_ITEMS, TooManyItems, path)? {
                    return Ok(());
                }
                let mut seen = HashSet::new();
                for (i, id) in ids.iter().enumerate() {
                    let path = format!("{path}/{i}");
                    self.require(
                        id.len() <= MAX_ID_BYTES && context.has_option(position, options, id),
                        UnknownOption,
                        &path,
                        "The option must exist in the field.",
                    )?;
                    if id.len() <= MAX_ID_BYTES {
                        self.require(
                            seen.insert(id),
                            DuplicateId,
                            &path,
                            "A multi-category value must not repeat an option.",
                        )?;
                    }
                }
            }
            _ => self.require(
                false,
                ValueType,
                path,
                "The value has the wrong type for the field.",
            )?,
        }
        Ok(())
    }

    fn annotation(
        &mut self,
        annotation: &Annotation,
        path: &str,
        context: &IndexedContext<'_>,
    ) -> Checked {
        self.uuid(annotation.id, &format!("{path}/id"))?;
        if let Some(size) = context.files.size_of(&annotation.file) {
            self.geometry(&annotation.geometry, &format!("{path}/geometry"), size)?;
            self.frames(&annotation.frames, &format!("{path}/frames"), size.frames)?;
        } else {
            self.require(
                false,
                UnknownFile,
                &format!("{path}/file"),
                "The annotation must name a known file.",
            )?;
        }
        if let Geometry::Mask(mask) = &annotation.geometry {
            // Compare without allocating a scope from an unchecked mask.
            let matches = match &annotation.frames {
                FrameScope::Set(frames)
                    if frames.as_written.is_none()
                        && frames.set.len() <= MAX_FRAMES_IN_SET
                        && frames.set.len() == mask.frames.len() =>
                {
                    frames
                        .set
                        .iter()
                        .copied()
                        .eq(mask.frames.keys().map(|f| f.0))
                }
                _ => false,
            };
            self.require(
                matches,
                MaskFramesMismatch,
                &format!("{path}/frames"),
                "A mask scope must equal its tile frames without an original list.",
            )?;
        }
        if self.bound(
            context.schema.classes.len(),
            MAX_SCHEMA_ITEMS,
            TooManyItems,
            &format!("{path}/class"),
        )? {
            let class = if annotation.class.len() <= MAX_ID_BYTES {
                context.class(&annotation.class)
            } else {
                None
            };
            self.require(
                class.is_some(),
                UnknownClass,
                &format!("{path}/class"),
                "The annotation class must exist in the schema.",
            )?;
            if let Some((position, class)) = class {
                if self.bound(
                    class.geometry.len(),
                    MAX_SCHEMA_ITEMS,
                    TooManyItems,
                    &format!("{path}/class"),
                )? {
                    self.require(
                        context.has_geometry(position, class, annotation.geometry.geometry_type()),
                        GeometryNotAllowed,
                        &format!("{path}/geometry"),
                        "The class must allow this geometry type.",
                    )?;
                }
                if self.bound(
                    annotation.attributes.len(),
                    MAX_VALUES,
                    TooManyItems,
                    &format!("{path}/attributes"),
                )? && self.bound(
                    class.attributes.len(),
                    MAX_SCHEMA_ITEMS,
                    TooManyItems,
                    &format!("{path}/class"),
                )? {
                    for (id, value) in &annotation.attributes {
                        if id.len() > MAX_ID_BYTES {
                            self.require(
                                false,
                                AttributeNotAllowed,
                                &format!("{path}/attributes"),
                                "The attribute is not allowed by the class.",
                            )?;
                            continue;
                        }
                        let value_path = format!("{path}/attributes/{}", pointer_member(id));
                        self.require(
                            context.has_attribute(position, class, id),
                            AttributeNotAllowed,
                            &value_path,
                            "The attribute is not allowed by the class.",
                        )?;
                        if let Some(field) = self.field(id, context, &value_path)? {
                            self.value(value, field, &value_path, context)?;
                        }
                    }
                }
            } else {
                self.bound(
                    annotation.attributes.len(),
                    MAX_VALUES,
                    TooManyItems,
                    &format!("{path}/attributes"),
                )?;
            }
        }
        self.meta(&annotation.meta, path)
    }

    fn label_content(
        &mut self,
        target: &LabelTarget,
        field: &str,
        value: Option<&LabelValue>,
        value_member: &str,
        path: &str,
        context: &IndexedContext<'_>,
    ) -> Checked {
        self.target(target, &format!("{path}/target"), ())?;
        let file = match target {
            LabelTarget::File { file } => Some((file, None)),
            LabelTarget::Frame { frame, index } => Some((frame, Some(index))),
            _ => None,
        };
        if let Some((file, frame)) = file {
            if let Some(size) = context.files.size_of(file) {
                if let Some(frame) = frame {
                    self.require(
                        *frame < size.frames,
                        FrameOutOfRange,
                        &format!("{path}/target/index"),
                        "The frame index is outside the file.",
                    )?;
                }
            } else {
                self.require(
                    false,
                    UnknownFile,
                    &format!("{path}/target"),
                    "The target must name a known file.",
                )?;
            }
        }
        if let Some((position, field)) = self.field(field, context, &format!("{path}/field"))? {
            if self.bound(
                field.applies_to.len(),
                MAX_SCHEMA_ITEMS,
                TooManyItems,
                &format!("{path}/field"),
            )? {
                self.require(
                    context.has_target(position, field, target.kind()),
                    TargetNotAllowed,
                    &format!("{path}/target"),
                    "The field must apply to this target kind.",
                )?;
            }
            if let Some(value) = value {
                self.value(
                    value,
                    (position, field),
                    &format!("{path}/{value_member}"),
                    context,
                )?;
            }
        }
        Ok(())
    }

    fn label(&mut self, label: &Label, path: &str, context: &IndexedContext<'_>) -> Checked {
        self.uuid(label.id, &format!("{path}/id"))?;
        self.label_content(
            &label.target,
            &label.field,
            Some(&label.value),
            "value",
            path,
            context,
        )?;
        self.meta(&label.meta, path)
    }
}

checked!(crate::Document, (), document);

impl Checks {
    fn document(&mut self, document: &crate::Document, path: &str, _: ()) -> Checked {
        let implicit;
        let schema = if let Some(schema) = &document.schema {
            self.schema(schema, &format!("{path}/schema"), ())?;
            schema
        } else {
            implicit = LabelSchema::implicit();
            &implicit
        };
        let mut files = std::collections::HashMap::new();
        for (i, file) in document.files.iter().enumerate() {
            let path = format!("{path}/files/{i}");
            self.file(file, &path, ())?;
            self.require(
                files.insert(&file.key, file.size()).is_none(),
                DuplicateFileKey,
                &format!("{path}/key"),
                "File keys must be distinct.",
            )?;
        }
        let mut layers = HashSet::new();
        let layers_bounded = self.bound(
            document.layers.len(),
            MAX_SCHEMA_ITEMS,
            TooManyItems,
            &format!("{path}/layers"),
        )?;
        if layers_bounded {
            for (i, layer) in document.layers.iter().enumerate() {
                let path = format!("{path}/layers/{i}");
                self.layer(layer, &path, ())?;
                self.require(
                    layers.insert(&layer.id),
                    DuplicateId,
                    &format!("{path}/id"),
                    "Layer identifiers must be distinct.",
                )?;
            }
        }
        let lookup = |key: &crate::FileKey| files.get(key).copied();
        let context = IndexedContext::new(&Context {
            files: &lookup,
            schema,
        });
        let mut annotation_ids = HashSet::new();
        for (i, annotation) in document.annotations.iter().enumerate() {
            let path = format!("{path}/annotations/{i}");
            self.annotation(annotation, &path, &context)?;
            if layers_bounded {
                self.require(
                    layers.contains(&annotation.layer),
                    UnknownLayer,
                    &format!("{path}/layer"),
                    "The annotation must name a layer in the document.",
                )?;
            }
            self.require(
                annotation_ids.insert(annotation.id),
                DuplicateId,
                &format!("{path}/id"),
                "Annotation identifiers must be distinct.",
            )?;
        }
        let mut label_ids = HashSet::new();
        let mut label_keys = HashSet::new();
        for (i, label) in document.labels.iter().enumerate() {
            let path = format!("{path}/labels/{i}");
            self.label(label, &path, &context)?;
            if layers_bounded {
                self.require(
                    layers.contains(&label.layer),
                    UnknownLayer,
                    &format!("{path}/layer"),
                    "The label must name a layer in the document.",
                )?;
            }
            self.require(
                label_ids.insert(label.id),
                DuplicateId,
                &format!("{path}/id"),
                "Label identifiers must be distinct.",
            )?;
            // Do not allocate canonical ids from targets rejected for size.
            let target_bounded = match &label.target {
                LabelTarget::Patient { patient: id }
                | LabelTarget::Study { study: id }
                | LabelTarget::Series { series: id } => id.len() <= MAX_NAME_BYTES,
                LabelTarget::Folder { folder, root } => {
                    folder.len() <= MAX_PATH_BYTES && root.len() <= MAX_ID_BYTES
                }
                _ => true,
            };
            if target_bounded && label.field.len() <= MAX_ID_BYTES {
                let key = (
                    label.target.canonical_id(),
                    &label.field,
                    &label.layer,
                    &label.meta.created_by,
                );
                self.require(
                    label_keys.insert(key),
                    DuplicateLabel,
                    &path,
                    "Labels must differ by target, field, layer or creator.",
                )?;
            }
        }
        Ok(())
    }
}

use crate::{LayerPatch, Op, OpEnvelope, Patch};
checked_with_schema!(Op, op);
checked_with_schema!(OpEnvelope, envelope);

fn patch_members(patch: &Patch) -> [bool; 5] {
    [
        patch.geometry.is_some(),
        patch.class.is_some(),
        patch.attributes.is_some(),
        patch.frames.is_some(),
        patch.layer.is_some(),
    ]
}

fn layer_patch_members(patch: &LayerPatch) -> [bool; 4] {
    [
        patch.name.is_some(),
        patch.color.is_some(),
        patch.exclusive_masks.is_some(),
        patch.readonly.is_some(),
    ]
}

impl Checks {
    fn envelope(
        &mut self,
        envelope: &OpEnvelope,
        path: &str,
        context: &IndexedContext<'_>,
    ) -> Checked {
        self.uuid(envelope.op_id, &format!("{path}/op_id"))?;
        self.op(&envelope.op, &format!("{path}/op"), context)
    }

    fn patch_members(&mut self, before: &[bool], after: &[bool], path: &str) -> Checked {
        self.require(
            after.iter().any(|present| *present),
            EmptyPatch,
            &format!("{path}/after"),
            "A patch must set at least one member.",
        )?;
        self.require(
            before == after,
            PatchFieldsMismatch,
            path,
            "Before and after must set exactly the same members.",
        )
    }

    fn op(&mut self, op: &Op, path: &str, context: &IndexedContext<'_>) -> Checked {
        match op {
            Op::CreateAnnotation { annotation } => {
                self.annotation(annotation, &format!("{path}/annotation"), context)?
            }
            Op::UpdateAnnotation {
                id,
                file,
                before,
                after,
                ..
            } => {
                self.uuid(*id, &format!("{path}/id"))?;
                self.patch_members(&patch_members(before), &patch_members(after), path)?;
                if let Some(size) = context.files.size_of(file) {
                    if let Some(geometry) = &after.geometry {
                        self.geometry(geometry, &format!("{path}/after/geometry"), size)?;
                    }
                    if let Some(frames) = &after.frames {
                        self.frames(frames, &format!("{path}/after/frames"), size.frames)?;
                    }
                } else {
                    self.require(
                        false,
                        UnknownFile,
                        &format!("{path}/file"),
                        "The operation must name a known file.",
                    )?;
                }
            }
            Op::DeleteAnnotation { id, snapshot, .. }
            | Op::RestoreAnnotation { id, snapshot, .. } => {
                self.uuid(*id, &format!("{path}/id"))?;
                self.require(
                    *id == snapshot.id,
                    SnapshotIdMismatch,
                    &format!("{path}/snapshot/id"),
                    "The snapshot must have the operation's identifier.",
                )?;
            }
            Op::MaskTiles {
                id,
                file,
                frame,
                tiles,
                ..
            } => {
                self.uuid(*id, &format!("{path}/id"))?;
                let size = context.files.size_of(file);
                self.require(
                    size.is_some(),
                    UnknownFile,
                    &format!("{path}/file"),
                    "The operation must name a known file.",
                )?;
                if let Some(size) = size {
                    self.require(
                        *frame < size.frames,
                        FrameOutOfRange,
                        &format!("{path}/frame"),
                        "The frame index is outside the file.",
                    )?;
                }
                let path = format!("{path}/tiles");
                if !self.bound(tiles.len(), MAX_MASK_TILES, TooManyTiles, &path)? {
                    return Ok(());
                }
                self.require(
                    !tiles.is_empty(),
                    EmptyPatch,
                    &path,
                    "A tile operation must contain a tile change.",
                )?;
                let mut seen = HashSet::new();
                for (i, tile) in tiles.iter().enumerate() {
                    let path = format!("{path}/{i}");
                    self.require(
                        seen.insert(tile.coord()),
                        DuplicateId,
                        &path,
                        "A tile position must occur only once.",
                    )?;
                    if let Some(size) = size {
                        self.tile(tile.coord(), 64, size, &path)?;
                    }
                    if let Some(payload) = &tile.before {
                        self.payload(payload, &format!("{path}/before"))?;
                    }
                    if let Some(payload) = &tile.after {
                        self.payload(payload, &format!("{path}/after"))?;
                    }
                }
            }
            Op::SetLabel {
                id,
                target,
                field,
                after,
                ..
            } => {
                self.uuid(*id, &format!("{path}/id"))?;
                self.label_content(target, field, after.as_ref(), "after", path, context)?;
            }
            Op::CreateLayer { layer } => self.layer(layer, &format!("{path}/layer"), ())?,
            Op::UpdateLayer { before, after, .. } => {
                self.patch_members(
                    &layer_patch_members(before),
                    &layer_patch_members(after),
                    path,
                )?;
                if let Some(name) = &after.name {
                    self.layer_name(name, &format!("{path}/after/name"))?;
                }
                if let Some(Some(color)) = &after.color {
                    self.color(color, &format!("{path}/after/color"))?;
                }
            }
            Op::DeleteLayer { id, snapshot, .. } => {
                self.require(
                    *id == snapshot.id,
                    SnapshotIdMismatch,
                    &format!("{path}/snapshot/id"),
                    "The snapshot must have the operation's identifier.",
                )?;
            }
            Op::Batch { ops } => {
                let path = format!("{path}/ops");
                if !self.bound(ops.len(), MAX_BATCH_OPS, TooManyOps, &path)? {
                    return Ok(());
                }
                self.require(
                    !ops.is_empty(),
                    EmptyBatch,
                    &path,
                    "A batch must contain an operation.",
                )?;
                for (i, op) in ops.iter().enumerate() {
                    let path = format!("{path}/{i}");
                    if matches!(op, Op::Batch { .. }) {
                        self.require(
                            false,
                            NestedBatch,
                            &path,
                            "A batch must not contain a batch.",
                        )?;
                    } else {
                        self.op(op, &path, context)?;
                    }
                }
            }
        }
        Ok(())
    }
}
