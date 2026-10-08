//! Annotations and the metadata every record carries.

use crate::frames::FrameScope;
use crate::geometry::Geometry;
use crate::key::{Author, FileKey, LayerId, Timestamp};
use crate::label::LabelValue;
use crate::number::serialize_optional_number;
use crate::validate::{Context, Invalid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;
use uuid::Uuid;

/// A new record or operation id: a UUIDv7 for the current time. Ids are time
/// ordered and unique across users and processes, so records from several
/// annotators merge with no renumbering.
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

/// What every annotation and label carries beside its content
/// (`docs/design/annotation-model.md` 6.1). Serialized into the record's own
/// object.
///
/// - `rev` counts the record's changes. The store assigns it: a created
///   record has `rev` 1 whatever the create payload said, and every applied
///   change adds one. It is what `base_rev` in an operation is compared with.
/// - `created_by` and `modified_by` are stamped by the store that owns the
///   record (in hub mode the hub, so a spoke cannot write as someone else);
///   what a client sends is advisory.
/// - Timestamps come from the server clock, not the browser.
/// - `derived_from` is the id of the record this one was copied or accepted
///   from.
/// - `score` is a model's confidence, from 0 to 1 inclusive. Human records
///   leave it out.
///
/// Invariants, checked wherever a record is validated: `score`, when
/// present, is finite and in `[0, 1]` (`bad_score`); `derived_from`, when
/// present, is a UUIDv7 (`id_not_uuid_v7`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RecordMeta {
    pub rev: u64,
    pub created_by: Author,
    pub created_at: Timestamp,
    pub modified_by: Author,
    pub modified_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub derived_from: Option<Uuid>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_number"
    )]
    #[ts(optional)]
    pub score: Option<f64>,
}

/// One annotation: a geometry of one class, on some frames of one file, in
/// one layer (`docs/design/annotation-model.md` 6.4).
///
/// ```json
/// { "id": "0199...", "file": "sop:1.2.840.1", "frames": "all",
///   "layer": "L-01", "class": "clip",
///   "geometry": { "type": "rect", "x0": 340, "y0": 120, "x1": 430, "y1": 220 },
///   "attributes": { "clip_shape": "ribbon" },
///   "rev": 1, "created_by": "user:alice", "created_at": "2026-09-29T21:04:11.120Z",
///   "modified_by": "user:alice", "modified_at": "2026-09-29T21:04:11.120Z" }
/// ```
///
/// Attribute values live on the annotation because they share its lifecycle:
/// deleting the shape deletes them, in one undo step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Annotation {
    /// UUIDv7, created by the client that creates the record.
    pub id: Uuid,
    pub file: FileKey,
    pub frames: FrameScope,
    pub layer: LayerId,
    /// The id of a class of the schema.
    pub class: String,
    pub geometry: Geometry,
    /// Values of the class's attribute fields, keyed by field id.
    #[serde(default)]
    pub attributes: BTreeMap<String, LabelValue>,
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

impl Annotation {
    /// The strict check every write of an annotation goes through.
    ///
    /// - `id` is a UUIDv7 (`id_not_uuid_v7`).
    /// - `file` is a file the context knows (`unknown_file`).
    /// - `geometry` is valid for that file ([`Geometry::validate`]).
    /// - `frames` is valid for that file's frame count
    ///   ([`FrameScope::validate`]). For a mask, `frames` is exactly the set
    ///   of frames that hold tiles, with nothing kept as written
    ///   (`mask_frames_mismatch`).
    /// - `class` is a class of the schema (`unknown_class`) that allows the
    ///   geometry's type (`geometry_not_allowed`). A deprecated class is
    ///   still a class.
    /// - Every attribute key is one of the class's attributes
    ///   (`attribute_not_allowed`) and its value fits that field (see
    ///   [`Context`]). At most 1,024 attributes
    ///   ([`crate::limits::MAX_VALUES`], `too_many_items`).
    /// - The record metadata is sound (see [`RecordMeta`]).
    ///
    /// Whether `layer` exists is not checked here, because a layer is state:
    /// [`crate::Document::validate`] checks it for a document, and a store
    /// checks it for an operation.
    pub fn validate(&self, context: &Context<'_>) -> Result<(), Invalid> {
        crate::Check::check(self, context)
    }
}
