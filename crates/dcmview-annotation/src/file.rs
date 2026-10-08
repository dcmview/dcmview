//! File references: the evidence that identifies a file
//! (`docs/design/annotation-model.md` 1.3, 1.6).

use crate::geometry::ImageSize;
use crate::key::FileKey;
use crate::number::serialize_number;
use crate::validate::Invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// Whether a file is a DICOM instance or a raster image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Dicom,
    Raster,
}

/// The pixel grid coordinates are expressed in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// The stored pixel matrix, before any EXIF, TIFF or view orientation.
    /// The only value in this version.
    #[default]
    Stored,
}

/// The coordinate space of a file's annotations. Always the stored grid; a
/// raster's EXIF or TIFF orientation is recorded so an adapter can derive
/// "as displayed" coordinates, and is never baked into geometry.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct Space {
    #[serde(default)]
    pub orientation: Orientation,
    /// The EXIF or TIFF orientation value, 1 to 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exif_orientation: Option<u8>,
    /// Members this version does not know, kept so they survive a round
    /// trip. The whole-slide space the design reserves arrives here.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// One frame's pixel spacing in millimetres, with the name of the DICOM
/// attribute it was read from (`docs/design/seams.md` 11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FrameSpacing {
    #[serde(serialize_with = "serialize_number")]
    pub row_mm: f64,
    #[serde(serialize_with = "serialize_number")]
    pub col_mm: f64,
    pub source: String,
}

/// Everything known about one file, so that matching a stored record to a
/// file does not depend on which key won.
///
/// ```json
/// { "key": "sop:1.2.840.113681.2863050711.1286.3688.28", "kind": "dicom",
///   "sop_instance_uid": "1.2.840.113681.2863050711.1286.3688.28",
///   "sop_class_uid": "1.2.840.10008.5.1.4.1.1.1.2",
///   "study_instance_uid": "1.2.3", "series_instance_uid": "1.2.3.4", "patient_id": "P1",
///   "path": "cohort_a/patient_0001/mg/lcc.dcm", "size_bytes": 34819072, "digest": null,
///   "rows": 4096, "columns": 3328, "frames": 1, "space": { "orientation": "stored" },
///   "format": "dicom", "pixel_digest": [null], "frame_source": null, "external_id": null }
/// ```
///
/// - `path` is relative to a root when one is known (written with `/`), and
///   absolute otherwise. An absolute path is the resolved one: owner
///   decision, EMBED parity amendment of 2026-10-05, "Paths through a
///   symlink are resolved on export." This crate never touches the
///   filesystem; whoever fills `path` resolves it.
/// - `digest` is the whole-file digest when known: `b3:<64 lowercase hex>`
///   (BLAKE3) or `sha256:<64 lowercase hex>`, over the file's bytes exactly
///   as stored.
/// - `rows`, `columns` and `frames` are a guard: a match that disagrees on
///   them is refused. They are 0 for a file without pixels.
/// - `pixel_digest` has one entry per frame, `px:<64 lowercase hex>` or
///   `null` for a frame not decoded yet; it may be empty when none is known.
/// - `frame_source` is the IFD index of each frame of a multi-page TIFF.
/// - `spacing` has one entry per frame, `null` where a frame has none.
///
/// Invariants, checked by [`FileRef::validate`] (`bad_file_ref` unless
/// another code is named): `rows` and `columns` are at most 1,048,576
/// ([`crate::limits::MAX_IMAGE_DIMENSION`]); `digest` and every pixel digest
/// have the forms above (`bad_digest`); `pixel_digest`, `frame_source` and
/// `spacing`, when not empty or absent, have exactly `frames` entries;
/// `exif_orientation`, when present, is 1 to 8; `path` is 1 to 4,096 bytes
/// ([`crate::limits::MAX_PATH_BYTES`]) and the UIDs, patient id, format and
/// external id at most 256 bytes ([`crate::limits::MAX_NAME_BYTES`])
/// (`too_long`); a `sop:` key names this file's `sop_instance_uid`; a spacing
/// is finite and positive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FileRef {
    pub key: FileKey,
    pub kind: FileKind,
    #[serde(default)]
    pub sop_instance_uid: Option<String>,
    #[serde(default)]
    pub sop_class_uid: Option<String>,
    #[serde(default)]
    pub study_instance_uid: Option<String>,
    #[serde(default)]
    pub series_instance_uid: Option<String>,
    #[serde(default)]
    pub patient_id: Option<String>,
    pub path: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub digest: Option<String>,
    pub rows: u32,
    pub columns: u32,
    pub frames: u32,
    #[serde(default)]
    pub space: Space,
    /// The detected format, by content: `dicom`, `png`, `jpeg`, `tiff`, ...
    pub format: String,
    #[serde(default)]
    pub pixel_digest: Vec<Option<String>>,
    #[serde(default)]
    pub frame_source: Option<Vec<u32>>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub spacing: Option<Vec<Option<FrameSpacing>>>,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

impl FileRef {
    /// The dimensions geometry and frames on this file are checked against.
    pub fn size(&self) -> ImageSize {
        ImageSize {
            columns: self.columns,
            rows: self.rows,
            frames: self.frames,
        }
    }

    /// Checks the invariants in the type's documentation.
    pub fn validate(&self) -> Result<(), Invalid> {
        crate::Check::check(self, ())
    }
}
