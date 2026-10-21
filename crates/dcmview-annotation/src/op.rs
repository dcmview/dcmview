//! Operations: the unit of every write and of syncing
//! (`docs/design/annotation-model.md` 7, `docs/design/seams.md` 9).
//!
//! A write is an [`OpEnvelope`]. Applying the same `op_id` twice returns the
//! first result, so a retry is safe. Every operation names a versioned
//! target and carries the state before it, so its inverse can be built
//! without reading history. Undo is not an operation: undoing sends the
//! inverse as an ordinary new operation under a fresh `op_id`.
//!
//! This module defines the operations, their validation and their queue
//! keys. Applying them to state belongs to a store, not to this crate.

use crate::frames::FrameScope;
use crate::geometry::{Geometry, TileCoord, TilePayload};
use crate::key::{Author, FileKey, LayerId, Timestamp};
use crate::label::{Label, LabelTarget, LabelValue};
use crate::layer::{Layer, LayerPatch};
use crate::record::Annotation;
use crate::validate::{Context, Invalid, Violation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;
use uuid::Uuid;

/// The members of an [`Annotation`] an `UpdateAnnotation` can change. A
/// member that is absent is not changed; one that is present replaces the
/// record's whole value, so a patch of `frames` drops a list kept as written
/// and a patch of `attributes` replaces every attribute.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct Patch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub geometry: Option<Geometry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attributes: Option<BTreeMap<String, LabelValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub frames: Option<FrameScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub layer: Option<LayerId>,
}

impl Patch {
    /// Sets on `annotation` every member this patch carries, and nothing
    /// else (the record metadata included: the store owns it).
    ///
    /// A store applies `after` and then validates the whole record with
    /// [`Annotation::validate`], because whether a class allows a geometry
    /// can only be judged on the result.
    pub fn apply_to(&self, annotation: &mut Annotation) {
        if let Some(geometry) = &self.geometry {
            annotation.geometry = geometry.clone();
        }
        if let Some(class) = &self.class {
            annotation.class = class.clone();
        }
        if let Some(attributes) = &self.attributes {
            annotation.attributes = attributes.clone();
        }
        if let Some(frames) = &self.frames {
            annotation.frames = frames.clone();
        }
        if let Some(layer) = &self.layer {
            annotation.layer = layer.clone();
        }
    }
}

/// One tile of a `MaskTiles` operation: its content before and after the
/// stroke. `null` is a tile with no pixel set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TileChange {
    pub tx: u32,
    pub ty: u32,
    pub before: Option<TilePayload>,
    pub after: Option<TilePayload>,
}

impl TileChange {
    /// The tile's position.
    pub fn coord(&self) -> TileCoord {
        TileCoord {
            tx: self.tx,
            ty: self.ty,
        }
    }
}

/// One change to the model.
///
/// Serialized as an object whose `type` member names the variant in snake
/// case, for example
/// `{ "type": "delete_annotation", "id": "0199...", "base_rev": 4, "snapshot": { ... } }`.
///
/// `base_rev` is the revision the client last saw of the record or layer the
/// operation changes; a store refuses the operation as a conflict when the
/// target has moved on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    /// Creates a record under an id that does not exist yet, live or
    /// deleted. Never an upsert.
    CreateAnnotation {
        annotation: Box<Annotation>,
    },
    /// Changes members of an annotation. `before` holds the old value of
    /// exactly the members `after` sets.
    UpdateAnnotation {
        id: Uuid,
        /// The annotation's file, which is the operation's queue key.
        file: FileKey,
        base_rev: u64,
        before: Box<Patch>,
        after: Box<Patch>,
    },
    /// Deletes an annotation. `snapshot` is the record as it was, so the
    /// inverse can restore it.
    DeleteAnnotation {
        id: Uuid,
        base_rev: u64,
        snapshot: Box<Annotation>,
    },
    /// The inverse of a delete: recreates the record under the same id from
    /// `snapshot`. `base_rev` is the revision the delete left.
    RestoreAnnotation {
        id: Uuid,
        base_rev: u64,
        snapshot: Box<Annotation>,
    },
    /// Changes tiles of one frame of a mask annotation. On an exclusive
    /// layer the stroke also clears pixels of other segments; those changes
    /// are separate `MaskTiles` operations in the same `Batch`.
    MaskTiles {
        id: Uuid,
        /// The annotation's file, which is the operation's queue key.
        file: FileKey,
        base_rev: u64,
        frame: u32,
        tiles: Vec<TileChange>,
    },
    /// Sets, changes or clears one label. `base_rev` is `null` only when the
    /// operation creates the record. `after: null` clears the value; the
    /// record is kept with its revision so a retried or stale operation
    /// fails its `base_rev` check.
    SetLabel {
        id: Uuid,
        base_rev: Option<u64>,
        target: LabelTarget,
        field: String,
        layer: LayerId,
        before: Option<LabelValue>,
        after: Option<LabelValue>,
    },
    CreateLayer {
        layer: Layer,
    },
    /// Changes members of a layer. `before` holds the old value of exactly
    /// the members `after` sets.
    UpdateLayer {
        id: LayerId,
        base_rev: u64,
        before: LayerPatch,
        after: LayerPatch,
    },
    /// Deletes a layer that holds no live record. Deleting a layer with
    /// content is a `Batch` of the record deletes and this.
    DeleteLayer {
        id: LayerId,
        base_rev: u64,
        snapshot: Box<Layer>,
    },
    /// One user gesture and one undo step. Atomic: every operation applies
    /// or none does, with one result. Holds 1 to 10,000 operations
    /// ([`crate::limits::MAX_BATCH_OPS`]), none of them a `Batch`.
    Batch {
        ops: Vec<Op>,
    },
}

/// What a client orders and tracks an operation by
/// (`docs/design/annotation-model.md` 7.2). The three kinds never collide as
/// strings: a file key starts with `sop:` or `b3:`, a canonical target id
/// with `patient:`, `study:`, `series:` or `folder:`, and a layer id holds no
/// colon.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueueKey {
    /// Annotation operations, and `SetLabel` on a file or frame target.
    File(FileKey),
    /// `SetLabel` on a patient, study, series or folder target: the target's
    /// canonical id ([`LabelTarget::canonical_id`]).
    Target(String),
    /// Layer operations.
    Layer(LayerId),
}

impl QueueKey {
    /// The key as one string.
    pub fn as_str(&self) -> &str {
        match self {
            QueueKey::File(key) => key.as_str(),
            QueueKey::Target(id) => id,
            QueueKey::Layer(id) => id.as_str(),
        }
    }
}

impl Op {
    /// The queue keys the operation touches, without repeats, in the order
    /// it first touches them. One key for every operation but a `Batch`,
    /// which has the keys of its operations.
    ///
    /// | Operation | Key |
    /// |---|---|
    /// | `create_annotation`, `delete_annotation`, `restore_annotation` | the file of the annotation or snapshot |
    /// | `update_annotation`, `mask_tiles` | `file` |
    /// | `set_label` on a file or frame target | that file's key |
    /// | `set_label` on any other target | the target's canonical id |
    /// | `create_layer`, `update_layer`, `delete_layer` | the layer id |
    pub fn queue_keys(&self) -> Vec<QueueKey> {
        let mut keys = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Iterative even for an invalid, deeply nested Rust-built batch.
        let mut pending = vec![self];
        while let Some(op) = pending.pop() {
            let key = match op {
                Self::CreateAnnotation { annotation } => QueueKey::File(annotation.file.clone()),
                Self::DeleteAnnotation { snapshot, .. }
                | Self::RestoreAnnotation { snapshot, .. } => QueueKey::File(snapshot.file.clone()),
                Self::UpdateAnnotation { file, .. } | Self::MaskTiles { file, .. } => {
                    QueueKey::File(file.clone())
                }
                Self::SetLabel { target, .. } => match target {
                    LabelTarget::File { file } | LabelTarget::Frame { frame: file, .. } => {
                        QueueKey::File(file.clone())
                    }
                    _ => QueueKey::Target(target.canonical_id()),
                },
                Self::CreateLayer { layer } => QueueKey::Layer(layer.id.clone()),
                Self::UpdateLayer { id, .. } | Self::DeleteLayer { id, .. } => {
                    QueueKey::Layer(id.clone())
                }
                Self::Batch { ops } => {
                    pending.extend(ops.iter().rev());
                    continue;
                }
            };
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }
        keys
    }

    /// Checks what can be checked about an operation without the store's
    /// state. Existence, revisions and layer membership are the store's.
    ///
    /// - `create_annotation`: [`Annotation::validate`].
    /// - `update_annotation`: `id` is a UUIDv7 (`id_not_uuid_v7`); `after`
    ///   sets at least one member (`empty_patch`); `before` sets exactly the
    ///   same members (`patch_fields_mismatch`); `file` is known
    ///   (`unknown_file`); a geometry in `after` is valid for that file and
    ///   frames in `after` are valid for its frame count. `before` is not
    ///   validated: it describes a record that may predate the rules.
    /// - `delete_annotation`, `restore_annotation`: `id` is a UUIDv7 and
    ///   equals the snapshot's id (`snapshot_id_mismatch`). The snapshot is
    ///   otherwise not validated, so a record that was imported leniently
    ///   can be deleted and restored as it was.
    /// - `mask_tiles`: `id` is a UUIDv7; `file` is known; `frame` is below
    ///   its frame count (`frame_out_of_range`); 1 to 262,144 tiles
    ///   ([`crate::limits::MAX_MASK_TILES`], `empty_patch` or
    ///   `too_many_tiles`); no position twice (`duplicate_id`); every tile
    ///   starts inside the image (`mask_tile_out_of_bounds`); every payload
    ///   present is well formed (`mask_payload`).
    /// - `set_label`: `id` is a UUIDv7; the target, field and `after` value
    ///   are checked as [`Label::validate`] checks them. `before` is not
    ///   validated.
    /// - `create_layer`: [`Layer::validate`].
    /// - `update_layer`: `after` sets at least one member (`empty_patch`);
    ///   `before` sets exactly the same members (`patch_fields_mismatch`); a
    ///   name or color in `after` is checked as [`Layer::validate`] checks
    ///   them.
    /// - `delete_layer`: `id` equals the snapshot's id
    ///   (`snapshot_id_mismatch`).
    /// - `batch`: at least one operation (`empty_batch`), at most 10,000
    ///   (`too_many_ops`), none a batch (`nested_batch`), and each one valid.
    ///
    /// Bounded work: an over-long list is refused on its length alone.
    pub fn validate(&self, context: &Context<'_>) -> Result<(), Invalid> {
        crate::Check::check(self, context)
    }
}

/// One write as it travels: `{ "op_id": "0199...", "actor": "user:alice",
/// "ts": "2026-09-29T21:04:11.120Z", "op": { ... } }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct OpEnvelope {
    /// UUIDv7, created by the client. A retry resends the same id; a redone
    /// change goes out under a new one.
    pub op_id: Uuid,
    /// Who the client says it is. Advisory: the store that owns the records
    /// stamps authorship.
    pub actor: Author,
    /// The client's clock when it made the change. Advisory.
    pub ts: Timestamp,
    pub op: Op,
}

/// Why an envelope could not be read.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EnvelopeError {
    /// The text is longer than [`crate::limits::MAX_ENVELOPE_BYTES`].
    #[error("operation envelope is {bytes} bytes, more than the {limit} allowed")]
    TooLarge { bytes: usize, limit: usize },
    /// The text is not JSON, or not the JSON of an envelope.
    #[error("operation envelope is malformed: {0}")]
    Malformed(String),
}

impl OpEnvelope {
    /// Reads one envelope from JSON text.
    ///
    /// Bounded: text longer than 16,777,216 bytes
    /// ([`crate::limits::MAX_ENVELOPE_BYTES`]) is refused before it is
    /// parsed, and JSON nested deeper than serde_json's limit of 128 levels
    /// is refused by the parser. Never panics on any input. The envelope is
    /// not validated; call [`OpEnvelope::validate`].
    pub fn from_json_str(text: &str) -> Result<OpEnvelope, EnvelopeError> {
        let limit = crate::limits::MAX_ENVELOPE_BYTES;
        if text.len() > limit {
            return Err(EnvelopeError::TooLarge {
                bytes: text.len(),
                limit,
            });
        }
        serde_json::from_str(text).map_err(|error| EnvelopeError::Malformed(error.to_string()))
    }

    /// `op_id` is a UUIDv7 (`id_not_uuid_v7`), and the operation is valid
    /// ([`Op::validate`]).
    pub fn validate(&self, context: &Context<'_>) -> Result<(), Invalid> {
        crate::Check::check(self, context)
    }
}

/// The new revision of one record or layer an operation changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RevEntry {
    /// The record's UUID or the layer's id.
    pub id: String,
    pub rev: u64,
}

/// The state a conflicting operation should have been based on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Current {
    Annotation {
        record: Box<Annotation>,
    },
    Label {
        record: Box<Label>,
    },
    Layer {
        record: Box<Layer>,
    },
    /// The target was deleted, or a label's value was cleared; `rev` is the
    /// revision that left.
    Deleted {
        id: String,
        rev: u64,
    },
    /// No record or layer has this id.
    Missing {
        id: String,
    },
}

/// The result of one envelope (`docs/design/annotation-model.md` 7.4). A
/// `Batch` is one envelope and gets one result: `ok` with every affected
/// revision, or the first conflict or invalid operation with nothing
/// applied.
///
/// Serialized with a `status` member: `ok`, `conflict` or `invalid`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApplyResult {
    Ok { revs: Vec<RevEntry> },
    Conflict { current: Current },
    Invalid { violations: Vec<Violation> },
}
