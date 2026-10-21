//! Labels: values set on a patient, study, series, file, frame or folder
//! (`docs/design/annotation-model.md` 4.3).

use crate::key::{FileKey, LayerId};
use crate::number::serialize_number;
use crate::record::RecordMeta;
use crate::schema::TargetKind;
use crate::validate::{Context, Invalid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;
use uuid::Uuid;

/// Prefix of the id that stands in for a missing PatientID,
/// StudyInstanceUID or SeriesInstanceUID: `missing:<file key>`. It keeps two
/// files without the id from becoming one labellable patient, study or
/// series.
pub const MISSING_ID_PREFIX: &str = "missing:";

/// What a label is set on.
///
/// On the wire, an object whose first member names the kind:
///
/// ```json
/// { "patient": "<PatientID>" }
/// { "study": "<StudyInstanceUID>" }
/// { "series": "<SeriesInstanceUID>" }
/// { "file": "<file key>" }
/// { "frame": "<file key>", "index": 12 }
/// { "folder": "<relative path>", "root": "<root id>" }
/// ```
///
/// An object with any other member, or with the members of two kinds, is
/// refused when it is read.
///
/// Invariants, checked by [`LabelTarget::validate`]:
///
/// - A patient, study or series id is 1 to 256 bytes
///   ([`crate::limits::MAX_NAME_BYTES`]) and holds no control character.
/// - A folder's `root` is 1 to 64 characters
///   ([`crate::limits::MAX_ID_BYTES`]), each an ASCII letter, digit, `.`,
///   `-` or `_`. Its path is relative to that root, at most 4,096 bytes
///   ([`crate::limits::MAX_PATH_BYTES`]), written with `/` between segments,
///   with no empty, `.` or `..` segment, no leading or trailing `/`, no `\`
///   and no control character. The empty path is the root itself.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(untagged, deny_unknown_fields)]
pub enum LabelTarget {
    Patient { patient: String },
    Study { study: String },
    Series { series: String },
    File { file: FileKey },
    Frame { frame: FileKey, index: u32 },
    Folder { folder: String, root: String },
}

impl LabelTarget {
    /// Which kind of target this is.
    pub fn kind(&self) -> TargetKind {
        match self {
            LabelTarget::Patient { .. } => TargetKind::Patient,
            LabelTarget::Study { .. } => TargetKind::Study,
            LabelTarget::Series { .. } => TargetKind::Series,
            LabelTarget::File { .. } => TargetKind::File,
            LabelTarget::Frame { .. } => TargetKind::Frame,
            LabelTarget::Folder { .. } => TargetKind::Folder,
        }
    }

    /// The id that stands in for a hierarchy id `file` does not have:
    /// `missing:<file key>`, for use as the id of a patient, study or series
    /// target.
    pub fn missing_id(file: &FileKey) -> String {
        format!("{MISSING_ID_PREFIX}{file}")
    }

    /// The one string form of the target: what labels are indexed by, and
    /// what two targets are equal by.
    ///
    /// | Target | Canonical id |
    /// |---|---|
    /// | patient | `patient:<id>` |
    /// | study | `study:<uid>` |
    /// | series | `series:<uid>` |
    /// | folder | `folder:<root>/<path>` (`folder:<root>/` for the root itself) |
    /// | file | the file key |
    /// | frame | `<file key>#<index>`, the index in decimal |
    pub fn canonical_id(&self) -> String {
        match self {
            Self::Patient { patient } => format!("patient:{patient}"),
            Self::Study { study } => format!("study:{study}"),
            Self::Series { series } => format!("series:{series}"),
            Self::File { file } => file.to_string(),
            Self::Frame { frame, index } => format!("{frame}#{index}"),
            Self::Folder { folder, root } => format!("folder:{root}/{folder}"),
        }
    }

    /// Checks the invariants in the type's documentation.
    ///
    /// Violation code: `bad_target`.
    pub fn validate(&self) -> Result<(), Invalid> {
        crate::Check::check(self, ())
    }
}

/// The value of a label or of an attribute. Which variants a field accepts
/// follows its type: `category` takes `Text` holding an option id,
/// `multi_category` takes `Many` holding distinct option ids, `boolean` takes
/// `Bool`, `number` takes `Number`, and `text` takes `Text`.
///
/// On the wire the value is the bare JSON boolean, number, string or array of
/// strings. A whole number is written without a fraction (`30`, not `30.0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum LabelValue {
    Bool(bool),
    Number(#[serde(serialize_with = "serialize_number")] f64),
    Text(String),
    Many(Vec<String>),
}

/// One label: the value of one field on one target, in one layer.
///
/// There is one label per target, field, layer and `created_by`; a frame
/// target's index is part of the target.
///
/// ```json
/// { "id": "0199...", "target": { "series": "1.2.3" }, "field": "image_quality",
///   "value": "good", "layer": "L-01", "rev": 1,
///   "created_by": "user:alice", "created_at": "2026-09-29T21:04:11.120Z",
///   "modified_by": "user:alice", "modified_at": "2026-09-29T21:04:11.120Z" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Label {
    /// UUIDv7, created by the client on the first set.
    pub id: Uuid,
    pub target: LabelTarget,
    /// The id of a field of the schema.
    pub field: String,
    pub value: LabelValue,
    pub layer: LayerId,
    #[serde(flatten)]
    pub meta: RecordMeta,
    /// Adapter-specific data, keyed by adapter id.
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

impl Label {
    /// Checks one label on its own: its id is a UUIDv7 (`id_not_uuid_v7`),
    /// its target is well formed (`bad_target`), a file or frame target names
    /// a file the context knows (`unknown_file`) and a frame index below that
    /// file's frame count (`frame_out_of_range`), its field exists
    /// (`unknown_field`) and applies to the target's kind
    /// (`target_not_allowed`), its value fits the field
    /// (see [`crate::validate::Context`]), and its record metadata is sound
    /// (see [`RecordMeta`]).
    ///
    /// The implicit schema has no fields, so under it every label is
    /// `unknown_field`.
    pub fn validate(&self, context: &Context<'_>) -> Result<(), Invalid> {
        crate::Check::check(self, context)
    }
}
