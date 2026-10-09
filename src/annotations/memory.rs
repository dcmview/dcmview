//! The in-memory annotation store: the default backend of a session
//! (`docs/design/annotation-model.md` 7.3).

#![expect(dead_code, reason = "nothing calls into the store yet")]

use super::backend::{AnnotationBackend, BackendError, Snapshot};
use dcmview_annotation::{
    limits, Annotation, ApplyResult, Author, Context, Current, Document, FileKey, FileSizes,
    FrameIndex, Geometry, Invalid, Label, LabelSchema, LabelTarget, Layer, LayerId, LayerKind,
    LayerSource, Op, OpEnvelope, RecordMeta, RevEntry, Timestamp, Violation, ViolationCode,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use uuid::Uuid;

/// The id of the layer every session starts with. The EMBED import and the
/// EMBED endpoints write into it (`docs/design/annotation-model.md` 5: the
/// import goes "straight into the default layer", "so today's single-set UX
/// is unchanged").
pub const DEFAULT_LAYER_ID: &str = "default";

/// The name of that layer.
pub const DEFAULT_LAYER_NAME: &str = "Annotations";

/// How many applied envelopes the store remembers the result of, for
/// answering a repeated `op_id`: 65,536, the most recent ones. A retry
/// follows its first attempt by seconds, so an `op_id` that old is a new
/// operation as far as the store can tell.
pub const REMEMBERED_OPS: usize = 65_536;

/// What a session fixes for its in-memory store.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryConfig {
    /// Who the store stamps on a record it creates or changes: the
    /// session's user ([`super::session_author`]).
    pub author: Author,
    /// The label schema in force, or `None` for a session without one,
    /// which is validated against `LabelSchema::implicit`.
    pub schema: Option<LabelSchema>,
}

/// How strictly a write is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checking {
    /// Every rule of the model and of the store. What an operation from a
    /// client gets.
    Strict,
    /// The rules of the store only (see [`MemoryBackend`], "What the store
    /// checks"). The envelope and the records it leaves are not run through
    /// the model's `validate`, and a geometry is held as given, not
    /// quantized. For rows the EMBED CSV import read and for ROI lists the
    /// EMBED endpoint accepted, both already checked by the rule EMBED
    /// has, which the model's rule would narrow
    /// (`docs/design/annotation-model.md` 2.1: "A record loaded this way
    /// stays as loaded until edited").
    AsLoaded,
}

/// What one write through [`MemoryBackend::transact`] is made with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Write<'a> {
    pub checking: Checking,
    /// Who is stamped on what the write creates and changes.
    pub author: &'a Author,
    /// The file index the annotations this write creates are bound to in
    /// the EMBED view ([`MemoryBackend::embed_records`]), or `None`.
    pub embed_slot: Option<usize>,
}

/// The current instant as the model writes one.
pub(crate) fn now() -> Timestamp {
    let text = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    Timestamp::parse(&text).expect("a UTC instant in the model's form")
}

/// Records by id, in server memory for the session. Nothing is written
/// anywhere, and the store dies with the process.
///
/// It keeps current state only: the records and layers that exist, the
/// ones that were deleted (so that a revision check still has something to
/// compare with), and the result of each of the last [`REMEMBERED_OPS`]
/// applied envelopes. It keeps no log of operations.
///
/// The contract of a transaction, of ordering and of revisions is
/// [`AnnotationBackend`]'s. This type adds what the store itself checks.
///
/// # What the store checks
///
/// A transaction first looks its `op_id` up; then, for a
/// [`Checking::Strict`] write, validates the envelope with
/// `OpEnvelope::validate` against the files it was given and the schema in
/// force (`Invalid` with what that reports); then applies the operation, or
/// each operation of a `Batch` in order, by this table. `Conflict` and
/// `Invalid` refuse the whole envelope.
///
/// | Operation | Refused when | Effect |
/// |---|---|---|
/// | `create_annotation` | the id exists, live or deleted: `Invalid` `duplicate_id` at `annotation/id`. The layer does not exist or is deleted: `Invalid` `unknown_layer` at `annotation/layer`. | The record is stored with `rev` 1 and this write's stamps; its `derived_from`, `score`, `extensions` and unknown members are kept. |
/// | `update_annotation` | see "The target of an operation"; `file` is not the record's file: `Conflict` with the record. A layer `after` names does not exist: `Invalid` `unknown_layer` at `after/layer`. The record with `after` applied fails `Annotation::validate`: `Invalid` with its violations. | `after` is applied (`Patch::apply_to`); `rev` rises by one. |
/// | `delete_annotation` | see "The target of an operation". | The record is kept as deleted: it leaves every snapshot, export and EMBED view, and `rev` rises by one. |
/// | `restore_annotation` | no record has the id: `Conflict` `missing`. The record is live: `Conflict` with it. `base_rev` is not the deleted record's `rev`: `Conflict` `deleted` with that `rev`. Its layer is deleted: `Invalid` `unknown_layer` at `snapshot/layer`. | The record is live again as it was when it was deleted, in its old place in creation order, with `rev` one higher. The store restores what it kept; of `snapshot` it reads only the id. |
/// | `mask_tiles` | as `update_annotation`; the record's geometry is not a mask: `Invalid` `geometry_not_allowed` at `id`. The record with the tiles changed fails `Annotation::validate` (a mask left without a tile is `mask_empty`). | Each tile with `after` is set and each with `after: null` removed, in the frame the operation names; a frame left without a tile is removed; the record's `frames` becomes the set of frames that hold tiles; `rev` rises by one. |
/// | `set_label`, `base_rev: null` | the id exists: `Invalid` `duplicate_id` at `id`. `after` is `null`: `Invalid` `empty_patch` at `after`. The layer does not exist: `Invalid` `unknown_layer` at `layer`. A label, with or without a value, already exists for this target, field, layer and author: `Invalid` `duplicate_label` at `id`. | The label is stored with `rev` 1. |
/// | `set_label`, `base_rev` given | no label has the id: `Conflict` `missing`. `base_rev` is not its `rev`, or the target, field or layer is not the label's: `Conflict` with the label, or `deleted` with its `rev` when it has no value. | `after` becomes the value, or the label loses its value (`after: null`) and leaves snapshots and exports while keeping its id and `rev`; `rev` rises by one. |
/// | `create_layer` | the id exists, live or deleted: `Invalid` `duplicate_id` at `layer/id`. 4,096 layers exist that are not deleted (`limits::MAX_SCHEMA_ITEMS`): `Invalid` `too_many_items` at `layer`. | The layer is stored with `rev` 1. |
/// | `update_layer` | see "The target of an operation". The layer with `after` applied fails `Layer::validate`. | `after` is applied (`LayerPatch::apply_to`); `rev` rises by one. |
/// | `delete_layer` | see "The target of an operation". The layer is the default layer, or holds a live annotation or a label with a value, after the operations before this one in the same batch: `Invalid` `bad_layer` at `id`. | The layer is kept as deleted; `rev` rises by one. |
///
/// A path in this table is relative to the operation: `/op/annotation/id`
/// for an envelope that is that operation, `/op/ops/2/annotation/id` for
/// the third operation of a batch.
///
/// ## The target of an operation
///
/// For an operation that names a record or layer by id and `base_rev`: no
/// such id is `Conflict` `missing`; a deleted one is `Conflict` `deleted`
/// with its `rev`; a `base_rev` that is not its `rev` is `Conflict` with
/// the record or layer as it is.
///
/// `before`, and the `snapshot` of a delete, are never compared with the
/// stored state: `base_rev` is the check.
///
/// ## What a strict write does to a geometry
///
/// A geometry that a [`Checking::Strict`] write creates or patches is stored
/// quantized (`Geometry::quantized`). A [`Checking::AsLoaded`] write stores
/// every value as given.
///
/// # Cost
///
/// A transaction costs time and memory in proportion to the envelope and to
/// the records it names, not to the size of the store: a refused envelope
/// is undone from what it changed, never by copying the store. The one
/// exception is `delete_layer`, which may visit every record to learn that
/// the layer is empty.
pub struct MemoryBackend {
    config: MemoryConfig,
    default_layer: LayerId,
    state: Mutex<State>,
}

/// Everything the store holds, under its one lock.
struct State {
    revision: u64,
    layers: Vec<LayerEntry>,
    layer_ids: HashMap<LayerId, usize>,
    live_layers: usize,
    annotations: Vec<AnnotationEntry>,
    annotation_ids: HashMap<Uuid, usize>,
    files: HashMap<FileKey, Vec<usize>>,
    labels: Vec<LabelEntry>,
    label_ids: HashMap<Uuid, usize>,
    label_targets: HashMap<LabelKey, Uuid>,
    remembered: HashMap<Uuid, ApplyResult>,
    remembered_order: VecDeque<Uuid>,
}

impl MemoryBackend {
    /// An empty store with the default layer: id [`DEFAULT_LAYER_ID`], name
    /// [`DEFAULT_LAYER_NAME`], kind `user`, its source's author
    /// `config.author`, and `rev` 1. The store's revision is 0.
    pub fn new(config: MemoryConfig) -> Self {
        let default_layer = LayerId::parse(DEFAULT_LAYER_ID).expect("a valid layer id");
        let layer = Layer {
            id: default_layer.clone(),
            name: DEFAULT_LAYER_NAME.to_string(),
            kind: LayerKind::User,
            exclusive_masks: false,
            color: None,
            readonly: false,
            source: LayerSource {
                author: Some(config.author.clone()),
                unknown: BTreeMap::new(),
            },
            rev: 1,
            unknown: BTreeMap::new(),
        };
        Self {
            config,
            default_layer,
            state: Mutex::new(State {
                revision: 0,
                layer_ids: HashMap::from([(layer.id.clone(), 0)]),
                layers: vec![LayerEntry {
                    record: layer,
                    deleted: false,
                }],
                live_layers: 1,
                annotations: Vec::new(),
                annotation_ids: HashMap::new(),
                files: HashMap::new(),
                labels: Vec::new(),
                label_ids: HashMap::new(),
                label_targets: HashMap::new(),
                remembered: HashMap::new(),
                remembered_order: VecDeque::new(),
            }),
        }
    }

    /// What the store was made with.
    pub fn config(&self) -> &MemoryConfig {
        &self.config
    }

    /// The id of the default layer.
    pub fn default_layer(&self) -> &LayerId {
        &self.default_layer
    }

    /// One envelope as one transaction (see [`AnnotationBackend`]), checked
    /// and stamped as `write` says. [`AnnotationBackend::apply`] is this
    /// with [`Checking::Strict`], the session's author and no EMBED slot.
    ///
    /// An annotation the transaction creates is bound to
    /// `write.embed_slot`, for good; nothing else about a record depends on
    /// it. A poisoned lock is [`BackendError::Unavailable`].
    pub(crate) fn transact(
        &self,
        envelope: &OpEnvelope,
        files: &dyn FileSizes,
        write: Write<'_>,
    ) -> Result<ApplyResult, BackendError> {
        let mut state = self.state.lock().map_err(lock_error)?;
        if let Some(result) = state.remembered.get(&envelope.op_id) {
            return Ok(result.clone());
        }
        let implicit;
        let schema = match &self.config.schema {
            Some(schema) => schema,
            None => {
                implicit = LabelSchema::implicit();
                &implicit
            }
        };
        let context = Context { files, schema };
        if write.checking == Checking::Strict {
            if let Err(invalid) = envelope.validate(&context) {
                return Ok(ApplyResult::Invalid {
                    violations: invalid.violations,
                });
            }
        }
        let mut transaction = Transaction {
            state: &mut state,
            context,
            write,
            stamp: now(),
            default_layer: &self.default_layer,
            undo: Vec::new(),
            revs: Vec::new(),
            rev_places: HashMap::new(),
        };
        let outcome = match &envelope.op {
            Op::Batch { ops } => ops
                .iter()
                .enumerate()
                .try_for_each(|(i, op)| transaction.apply(op, &format!("/op/ops/{i}"))),
            op => transaction.apply(op, "/op"),
        };
        if let Err(refusal) = outcome {
            transaction.rollback();
            return Ok(refusal);
        }
        let result = ApplyResult::Ok {
            revs: transaction.revs,
        };
        state.revision += 1;
        state.remembered.insert(envelope.op_id, result.clone());
        state.remembered_order.push_back(envelope.op_id);
        if state.remembered_order.len() > REMEMBERED_OPS {
            if let Some(oldest) = state.remembered_order.pop_front() {
                state.remembered.remove(&oldest);
            }
        }
        Ok(result)
    }

    /// The records the EMBED view of one file shows, in creation order: the
    /// live annotations on `key` whose geometry is a rectangle, whatever
    /// their class and layer, that are bound to `slot` or to no slot.
    ///
    /// An annotation is bound to a slot (a file index) when an EMBED write
    /// for that file created it: a row of the `--annotations` CSV, or the
    /// EMBED endpoint. One made by any other operation is bound to none and
    /// is in the view of every file that has its key. The binding is what
    /// keeps the ROI rows of two byte-identical files apart, which share a
    /// key and so a set of records: each file's view shows the rows that
    /// were written for it.
    pub(crate) fn embed_records(
        &self,
        key: &FileKey,
        slot: usize,
    ) -> Result<Vec<Annotation>, BackendError> {
        let state = self.state.lock().map_err(lock_error)?;
        Ok(state
            .files
            .get(key)
            .into_iter()
            .flatten()
            .map(|&place| &state.annotations[place])
            .filter(|entry| {
                !entry.deleted
                    && matches!(entry.record.geometry, Geometry::Rect { .. })
                    && (entry.embed_slot.is_none() || entry.embed_slot == Some(slot))
            })
            .map(|entry| entry.record.clone())
            .collect())
    }
}

impl AnnotationBackend for MemoryBackend {
    fn snapshot(&self, files: &[FileKey]) -> Result<Snapshot, BackendError> {
        let state = self.state.lock().map_err(lock_error)?;
        let keys: HashSet<_> = files.iter().collect();
        let mut places: Vec<_> = keys
            .iter()
            .filter_map(|key| state.files.get(*key))
            .flatten()
            .copied()
            .collect();
        places.sort_unstable();
        Ok(Snapshot {
            revision: state.revision,
            layers: state
                .layers
                .iter()
                .filter(|entry| !entry.deleted)
                .map(|entry| entry.record.clone())
                .collect(),
            annotations: places
                .into_iter()
                .map(|place| &state.annotations[place])
                .filter(|entry| !entry.deleted)
                .map(|entry| entry.record.clone())
                .collect(),
            labels: state
                .labels
                .iter()
                .filter(|entry| {
                    !entry.cleared
                        && match &entry.record.target {
                            LabelTarget::File { file } | LabelTarget::Frame { frame: file, .. } => {
                                keys.contains(file)
                            }
                            _ => false,
                        }
                })
                .map(|entry| entry.record.clone())
                .collect(),
        })
    }

    fn apply(
        &self,
        envelopes: Vec<OpEnvelope>,
        files: &dyn FileSizes,
    ) -> Result<Vec<ApplyResult>, BackendError> {
        envelopes
            .iter()
            .map(|envelope| {
                self.transact(
                    envelope,
                    files,
                    Write {
                        checking: Checking::Strict,
                        author: &self.config.author,
                        embed_slot: None,
                    },
                )
            })
            .collect()
    }

    fn export(&self) -> Result<Document, BackendError> {
        let state = self.state.lock().map_err(lock_error)?;
        let mut document = Document::new();
        document.schema = self.config.schema.clone();
        document.layers = state
            .layers
            .iter()
            .filter(|entry| !entry.deleted)
            .map(|entry| entry.record.clone())
            .collect();
        document.annotations = state
            .annotations
            .iter()
            .filter(|entry| !entry.deleted)
            .map(|entry| entry.record.clone())
            .collect();
        document.labels = state
            .labels
            .iter()
            .filter(|entry| !entry.cleared)
            .map(|entry| entry.record.clone())
            .collect();
        Ok(document)
    }

    fn revision(&self) -> Result<u64, BackendError> {
        Ok(self.state.lock().map_err(lock_error)?.revision)
    }
}

#[derive(Clone)]
struct AnnotationEntry {
    record: Annotation,
    deleted: bool,
    embed_slot: Option<usize>,
}

#[derive(Clone)]
struct LayerEntry {
    record: Layer,
    deleted: bool,
}

#[derive(Clone)]
struct LabelEntry {
    record: Label,
    cleared: bool,
}

type LabelKey = (String, String, LayerId, Author);

fn label_key(record: &Label) -> LabelKey {
    (
        record.target.canonical_id(),
        record.field.clone(),
        record.layer.clone(),
        record.meta.created_by.clone(),
    )
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> BackendError {
    BackendError::Unavailable("annotations store lock poisoned".to_string())
}

fn invalid(code: ViolationCode, path: &str, detail: &str) -> ApplyResult {
    ApplyResult::Invalid {
        violations: vec![Violation {
            code,
            path: path.to_string(),
            detail: detail.to_string(),
        }],
    }
}

fn prefixed(mut invalid: Invalid, path: &str) -> ApplyResult {
    for violation in &mut invalid.violations {
        violation.path.insert_str(0, path);
    }
    ApplyResult::Invalid {
        violations: invalid.violations,
    }
}

fn conflict(current: Current) -> ApplyResult {
    ApplyResult::Conflict { current }
}

fn missing(id: impl ToString) -> ApplyResult {
    conflict(Current::Missing { id: id.to_string() })
}

fn deleted(id: impl ToString, rev: u64) -> ApplyResult {
    conflict(Current::Deleted {
        id: id.to_string(),
        rev,
    })
}

// Replacements keep the old slot; creations are undone by popping the last
// slot and removing precisely the index entries that creation added.
enum Undo {
    Annotation(usize, Box<AnnotationEntry>),
    Layer(usize, Box<LayerEntry>),
    Label(usize, Box<LabelEntry>),
    CreatedAnnotation,
    CreatedLayer,
    CreatedLabel,
}

struct Transaction<'a> {
    state: &'a mut State,
    context: Context<'a>,
    write: Write<'a>,
    stamp: Timestamp,
    default_layer: &'a LayerId,
    undo: Vec<Undo>,
    revs: Vec<RevEntry>,
    rev_places: HashMap<String, usize>,
}

impl Transaction<'_> {
    fn changed(&mut self, id: impl ToString, rev: u64) {
        let id = id.to_string();
        if let Some(&place) = self.rev_places.get(&id) {
            self.revs[place].rev = rev;
        } else {
            self.rev_places.insert(id.clone(), self.revs.len());
            self.revs.push(RevEntry { id, rev });
        }
    }

    fn stamp_change(&self, meta: &mut RecordMeta) {
        meta.rev += 1;
        meta.modified_by = self.write.author.clone();
        meta.modified_at = self.stamp.clone();
    }

    fn stamp_create(&self, meta: &mut RecordMeta) {
        meta.rev = 0;
        meta.created_by = self.write.author.clone();
        meta.created_at = self.stamp.clone();
        self.stamp_change(meta);
    }

    fn require_layer(&self, id: &LayerId, path: &str) -> Result<(), ApplyResult> {
        if self
            .state
            .layer_ids
            .get(id)
            .is_some_and(|&place| !self.state.layers[place].deleted)
        {
            Ok(())
        } else {
            Err(invalid(
                ViolationCode::UnknownLayer,
                path,
                "The layer does not exist.",
            ))
        }
    }

    fn annotation_target(
        &self,
        id: &Uuid,
        base_rev: u64,
        restore: bool,
    ) -> Result<usize, ApplyResult> {
        let &place = self
            .state
            .annotation_ids
            .get(id)
            .ok_or_else(|| missing(id))?;
        let entry = &self.state.annotations[place];
        if entry.deleted {
            if !restore || entry.record.meta.rev != base_rev {
                return Err(deleted(id, entry.record.meta.rev));
            }
        } else if restore || entry.record.meta.rev != base_rev {
            return Err(conflict(Current::Annotation {
                record: Box::new(entry.record.clone()),
            }));
        }
        Ok(place)
    }

    fn layer_target(&self, id: &LayerId, base_rev: u64) -> Result<usize, ApplyResult> {
        let &place = self.state.layer_ids.get(id).ok_or_else(|| missing(id))?;
        let entry = &self.state.layers[place];
        if entry.deleted {
            return Err(deleted(id, entry.record.rev));
        }
        if entry.record.rev != base_rev {
            return Err(conflict(Current::Layer {
                record: Box::new(entry.record.clone()),
            }));
        }
        Ok(place)
    }

    fn replace_annotation(&mut self, place: usize, mut entry: AnnotationEntry) {
        self.stamp_change(&mut entry.record.meta);
        self.changed(entry.record.id, entry.record.meta.rev);
        let old = std::mem::replace(&mut self.state.annotations[place], entry);
        self.undo.push(Undo::Annotation(place, Box::new(old)));
    }

    fn apply(&mut self, op: &Op, path: &str) -> Result<(), ApplyResult> {
        match op {
            Op::CreateAnnotation { annotation } => {
                if self.state.annotation_ids.contains_key(&annotation.id) {
                    return Err(invalid(
                        ViolationCode::DuplicateId,
                        &format!("{path}/annotation/id"),
                        "The annotation id already exists.",
                    ));
                }
                self.require_layer(&annotation.layer, &format!("{path}/annotation/layer"))?;
                let mut record = annotation.as_ref().clone();
                self.stamp_create(&mut record.meta);
                if self.write.checking == Checking::Strict {
                    record.geometry = record.geometry.quantized();
                }
                let place = self.state.annotations.len();
                self.state.annotation_ids.insert(record.id, place);
                self.state
                    .files
                    .entry(record.file.clone())
                    .or_default()
                    .push(place);
                self.changed(record.id, record.meta.rev);
                self.state.annotations.push(AnnotationEntry {
                    record,
                    deleted: false,
                    embed_slot: self.write.embed_slot,
                });
                self.undo.push(Undo::CreatedAnnotation);
            }
            Op::UpdateAnnotation {
                id,
                file,
                base_rev,
                after,
                ..
            } => {
                let place = self.annotation_target(id, *base_rev, false)?;
                let mut entry = self.state.annotations[place].clone();
                if &entry.record.file != file {
                    return Err(conflict(Current::Annotation {
                        record: Box::new(entry.record),
                    }));
                }
                if let Some(layer) = &after.layer {
                    self.require_layer(layer, &format!("{path}/after/layer"))?;
                }
                after.apply_to(&mut entry.record);
                if self.write.checking == Checking::Strict {
                    if after.geometry.is_some() {
                        entry.record.geometry = entry.record.geometry.quantized();
                    }
                    entry
                        .record
                        .validate(&self.context)
                        .map_err(|error| prefixed(error, path))?;
                }
                self.replace_annotation(place, entry);
            }
            Op::DeleteAnnotation { id, base_rev, .. }
            | Op::RestoreAnnotation { id, base_rev, .. } => {
                let restore = matches!(op, Op::RestoreAnnotation { .. });
                let place = self.annotation_target(id, *base_rev, restore)?;
                let mut entry = self.state.annotations[place].clone();
                if restore {
                    self.require_layer(&entry.record.layer, &format!("{path}/snapshot/layer"))?;
                }
                entry.deleted = !restore;
                self.replace_annotation(place, entry);
            }
            Op::MaskTiles {
                id,
                file,
                base_rev,
                frame,
                tiles,
            } => {
                let place = self.annotation_target(id, *base_rev, false)?;
                let mut entry = self.state.annotations[place].clone();
                if &entry.record.file != file {
                    return Err(conflict(Current::Annotation {
                        record: Box::new(entry.record),
                    }));
                }
                let Geometry::Mask(mask) = &mut entry.record.geometry else {
                    return Err(invalid(
                        ViolationCode::GeometryNotAllowed,
                        &format!("{path}/id"),
                        "The annotation is not a mask.",
                    ));
                };
                let frame = FrameIndex(*frame);
                let mut map = mask.frames.remove(&frame).unwrap_or_default();
                for tile in tiles {
                    if let Some(payload) = &tile.after {
                        map.insert(tile.coord(), payload.clone());
                    } else {
                        map.remove(&tile.coord());
                    }
                }
                if !map.is_empty() {
                    mask.frames.insert(frame, map);
                }
                entry.record.frames = mask.frame_scope();
                if self.write.checking == Checking::Strict {
                    entry
                        .record
                        .validate(&self.context)
                        .map_err(|error| prefixed(error, path))?;
                }
                self.replace_annotation(place, entry);
            }
            Op::SetLabel {
                id,
                base_rev,
                target,
                field,
                layer,
                after,
                ..
            } => {
                if let Some(base_rev) = base_rev {
                    let &place = self.state.label_ids.get(id).ok_or_else(|| missing(id))?;
                    let old = &self.state.labels[place];
                    if old.record.meta.rev != *base_rev
                        || &old.record.target != target
                        || &old.record.field != field
                        || &old.record.layer != layer
                    {
                        return Err(if old.cleared {
                            deleted(id, old.record.meta.rev)
                        } else {
                            conflict(Current::Label {
                                record: Box::new(old.record.clone()),
                            })
                        });
                    }
                    let mut entry = old.clone();
                    entry.cleared = after.is_none();
                    if let Some(value) = after {
                        entry.record.value = value.clone();
                    }
                    self.stamp_change(&mut entry.record.meta);
                    self.changed(id, entry.record.meta.rev);
                    let old = std::mem::replace(&mut self.state.labels[place], entry);
                    self.undo.push(Undo::Label(place, Box::new(old)));
                } else {
                    if self.state.label_ids.contains_key(id) {
                        return Err(invalid(
                            ViolationCode::DuplicateId,
                            &format!("{path}/id"),
                            "The label id already exists.",
                        ));
                    }
                    let value = after.as_ref().ok_or_else(|| {
                        invalid(
                            ViolationCode::EmptyPatch,
                            &format!("{path}/after"),
                            "A new label needs a value.",
                        )
                    })?;
                    self.require_layer(layer, &format!("{path}/layer"))?;
                    let key = (
                        target.canonical_id(),
                        field.clone(),
                        layer.clone(),
                        self.write.author.clone(),
                    );
                    if self.state.label_targets.contains_key(&key) {
                        return Err(invalid(
                            ViolationCode::DuplicateLabel,
                            &format!("{path}/id"),
                            "A label already exists for this target, field, layer and author.",
                        ));
                    }
                    let record = Label {
                        id: *id,
                        target: target.clone(),
                        field: field.clone(),
                        value: value.clone(),
                        layer: layer.clone(),
                        meta: RecordMeta {
                            rev: 1,
                            created_by: self.write.author.clone(),
                            created_at: self.stamp.clone(),
                            modified_by: self.write.author.clone(),
                            modified_at: self.stamp.clone(),
                            derived_from: None,
                            score: None,
                        },
                        extensions: BTreeMap::new(),
                        unknown: BTreeMap::new(),
                    };
                    self.state.label_ids.insert(*id, self.state.labels.len());
                    self.state.label_targets.insert(key, *id);
                    self.state.labels.push(LabelEntry {
                        record,
                        cleared: false,
                    });
                    self.changed(id, 1);
                    self.undo.push(Undo::CreatedLabel);
                }
            }
            Op::CreateLayer { layer } => {
                if self.state.layer_ids.contains_key(&layer.id) {
                    return Err(invalid(
                        ViolationCode::DuplicateId,
                        &format!("{path}/layer/id"),
                        "The layer id already exists.",
                    ));
                }
                if self.state.live_layers >= limits::MAX_SCHEMA_ITEMS {
                    return Err(invalid(
                        ViolationCode::TooManyItems,
                        &format!("{path}/layer"),
                        "The store has too many live layers.",
                    ));
                }
                let mut record = layer.clone();
                record.rev = 1;
                self.changed(&record.id, 1);
                self.state
                    .layer_ids
                    .insert(record.id.clone(), self.state.layers.len());
                self.state.layers.push(LayerEntry {
                    record,
                    deleted: false,
                });
                self.state.live_layers += 1;
                self.undo.push(Undo::CreatedLayer);
            }
            Op::UpdateLayer {
                id,
                base_rev,
                after,
                ..
            } => {
                let place = self.layer_target(id, *base_rev)?;
                let mut entry = self.state.layers[place].clone();
                after.apply_to(&mut entry.record);
                if self.write.checking == Checking::Strict {
                    entry
                        .record
                        .validate()
                        .map_err(|error| prefixed(error, path))?;
                }
                entry.record.rev += 1;
                self.changed(id, entry.record.rev);
                let old = std::mem::replace(&mut self.state.layers[place], entry);
                self.undo.push(Undo::Layer(place, Box::new(old)));
            }
            Op::DeleteLayer { id, base_rev, .. } => {
                let place = self.layer_target(id, *base_rev)?;
                if id == self.default_layer
                    || self
                        .state
                        .annotations
                        .iter()
                        .any(|entry| !entry.deleted && &entry.record.layer == id)
                    || self
                        .state
                        .labels
                        .iter()
                        .any(|entry| !entry.cleared && &entry.record.layer == id)
                {
                    return Err(invalid(
                        ViolationCode::BadLayer,
                        &format!("{path}/id"),
                        "The default layer or a layer with live records cannot be deleted.",
                    ));
                }
                let mut entry = self.state.layers[place].clone();
                entry.deleted = true;
                entry.record.rev += 1;
                self.changed(id, entry.record.rev);
                let old = std::mem::replace(&mut self.state.layers[place], entry);
                self.state.live_layers -= 1;
                self.undo.push(Undo::Layer(place, Box::new(old)));
            }
            Op::Batch { .. } => {
                return Err(invalid(
                    ViolationCode::NestedBatch,
                    path,
                    "A batch cannot contain another batch.",
                ))
            }
        }
        Ok(())
    }

    fn rollback(self) {
        for undo in self.undo.into_iter().rev() {
            match undo {
                Undo::Annotation(place, old) => self.state.annotations[place] = *old,
                Undo::Label(place, old) => self.state.labels[place] = *old,
                Undo::Layer(place, old) => {
                    self.state.live_layers += usize::from(!old.deleted);
                    self.state.live_layers -= usize::from(!self.state.layers[place].deleted);
                    self.state.layers[place] = *old;
                }
                Undo::CreatedAnnotation => {
                    if let Some(entry) = self.state.annotations.pop() {
                        self.state.annotation_ids.remove(&entry.record.id);
                        if let Some(places) = self.state.files.get_mut(&entry.record.file) {
                            places.pop();
                            if places.is_empty() {
                                self.state.files.remove(&entry.record.file);
                            }
                        }
                    }
                }
                Undo::CreatedLabel => {
                    if let Some(entry) = self.state.labels.pop() {
                        self.state.label_ids.remove(&entry.record.id);
                        self.state.label_targets.remove(&label_key(&entry.record));
                    }
                }
                Undo::CreatedLayer => {
                    if let Some(entry) = self.state.layers.pop() {
                        self.state.layer_ids.remove(&entry.record.id);
                        self.state.live_layers -= 1;
                    }
                }
            }
        }
    }
}
