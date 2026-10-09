//! Annotations as a session serves them: the annotation store joined to the
//! file registry.
//!
//! The store (`crate::annotations`) addresses a file by its key and knows
//! nothing of indexes, paths or what a masked session shows. A request
//! names a file by its catalog index (the EMBED endpoints) or by the key
//! the session sent the client (the operation endpoint). The functions here
//! do the joining, the same way for every door:
//!
//! - **Only a settled key reaches a record.** Before anything is written
//!   for a file, its key is settled with `FileRegistry::ensure_key`, which
//!   may hash the file and the files that share its UID first
//!   (`docs/design/annotation-model.md` 1.7: verified "before the first
//!   annotation write on either file"). The write waits for that. A file
//!   that cannot be given a key is not written to:
//!   [`AnnotationError::KeyUnavailable`].
//! - **Reads never hash.** A file whose key is not settled has no record,
//!   because no write got through for it, so a read answers "none" at once
//!   from `FileRegistry::key_status`.
//! - **Keys cross the wire in the session's form.** A key from a client is
//!   resolved with `FileRegistry::file_for_shown_key`, and a key sent back
//!   is written with `FileRegistry::shown_key_of`, so a masked session
//!   neither accepts nor sends a key that holds a real UID. Records hold
//!   real keys.

use super::{FileRegistry, KeyError};
use crate::annotations::{
    canonicalize_annotations,
    embed::{replacement_ops, rois_of, rows_as_creates, write_embed_csv},
    memory::{now, Checking, Write},
    AnnotationBackend, AnnotationIndexMap, AnnotationStore, EmbedRoiAnnotations,
    EMBED_IMPORT_AUTHOR,
};
use crate::api::contracts::FileKeyError;
use crate::types::FileEntry;
use dcmview_annotation::{
    new_id, ApplyResult, Author, Current, FileKey, ImageSize, LabelTarget, Op, OpEnvelope,
    Violation, ViolationCode,
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Why an annotation request could not be served. A refused operation is
/// not one of these: it is an [`ApplyResult`] inside [`OpOutcome`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnnotationError {
    /// No file has this index.
    #[error("file index {0} not found")]
    NotFound(usize),
    /// A ROI list the EMBED endpoint does not accept, with the reason
    /// `canonicalize_annotations` gives.
    #[error("{0}")]
    Rejected(String),
    /// The file has no key and cannot be given one, so nothing may be
    /// written for it (`FileRegistry::ensure_key` answered
    /// `KeyError::Unavailable`).
    #[error("file {index} has no key, so no annotation can be saved for it: {}", match .reason { FileKeyError::Unreadable => "it could not be read", FileKeyError::Changed => "it changed after it was loaded" })]
    KeyUnavailable { index: usize, reason: FileKeyError },
    /// The `--annotations` import failed earlier; the message is the
    /// import's own.
    #[error("{0}")]
    ImportFailed(String),
    /// The viewer is shutting down (`KeyError::Stopped`).
    #[error("the viewer is shutting down")]
    Stopped,
    /// The store could not be used.
    #[error("{0}")]
    Store(String),
}

/// What one operation envelope came to.
#[derive(Debug, Clone, PartialEq)]
pub struct OpOutcome {
    /// The store's result, with every file key in it written the way the
    /// session sends keys.
    pub result: ApplyResult,
    /// The store's revision after the envelope.
    pub revision: u64,
}

/// What an EMBED import did with its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmbedImportReport {
    /// Files whose rows are now records.
    pub files_loaded: usize,
    /// Files whose rows were dropped because the file has no key and could
    /// not be given one.
    pub files_without_key: usize,
    /// Files whose rows were dropped because the EMBED endpoint had already
    /// replaced that file's ROIs.
    pub files_edited: usize,
}

/// `GET /api/file/{index}/annotations`: the file's EMBED view.
///
/// Waits for the `--annotations` import ([`AnnotationStore::wait_until_ready`];
/// a failed import is [`AnnotationError::ImportFailed`] with its message).
/// Then, without waiting for anything else: for a file whose key is settled
/// (`FileRegistry::key_status`), `rois_of` the records
/// `MemoryBackend::embed_records` gives for that key and this index; for
/// any other file, no ROI (`EmbedRoiAnnotations::empty`).
///
/// It never starts or waits for hashing. Rectangles made through the
/// operation endpoint are in the view; other geometries are not.
pub async fn embed_rois(
    registry: &FileRegistry,
    store: &AnnotationStore,
    index: usize,
) -> Result<EmbedRoiAnnotations, AnnotationError> {
    store
        .wait_until_ready()
        .await
        .map_err(|error| AnnotationError::ImportFailed(error.to_string()))?;
    let file = registry
        .get(index)
        .ok_or(AnnotationError::NotFound(index))?;
    file_rois(registry, store, &file)
}

/// `PUT /api/file/{index}/annotations`: replaces the ROIs the file's EMBED
/// view shows, and returns them in canonical form. What a client sees is
/// what it saw before the store held records.
///
/// In this order:
///
/// 1. `canonicalize_annotations` against the file's rows, columns and frame
///    count: the whole list is accepted or [`AnnotationError::Rejected`]
///    with that function's message, before anything else happens. This is
///    the only check of the list. In particular the model's stricter rules
///    are not applied (an empty frame list for one ROI stays accepted), and
///    a list that repeats rows the view holds from a lenient import is
///    refused when those rows are out of bounds, even though nothing about
///    them would change.
/// 2. The file's key is settled (`FileRegistry::ensure_key`), which waits
///    for hashing when the file needs it:
///    [`AnnotationError::KeyUnavailable`] or [`AnnotationError::Stopped`],
///    with nothing written.
/// 3. The file is marked as edited ([`AnnotationStore::mark_edited`]), so a
///    running import drops its rows for it.
/// 4. One transaction: `replacement_ops` from the view's records
///    (`MemoryBackend::embed_records`) to the canonical list, as one
///    `Batch` under a new `op_id`, through `MemoryBackend::transact` with
///    `Checking::AsLoaded`, the store's own author and this index as the
///    EMBED slot. No operation means no transaction: saving what is already
///    shown changes no record and no revision. A `Conflict` means another
///    write got in between reading the view and writing; read the view
///    again and retry, up to 16 times, then [`AnnotationError::Store`].
///
/// The answer is the canonical list of step 1.
///
/// It does not wait for the `--annotations` import: an edit made while the
/// CSV is loading wins over the CSV's rows for that file.
pub async fn replace_embed_rois(
    registry: &FileRegistry,
    store: &AnnotationStore,
    index: usize,
    rois: EmbedRoiAnnotations,
) -> Result<EmbedRoiAnnotations, AnnotationError> {
    let file = registry
        .get(index)
        .ok_or(AnnotationError::NotFound(index))?;
    let wanted = canonicalize_annotations(rois, file.rows, file.columns, file.frame_count)
        .map_err(|error| AnnotationError::Rejected(error.to_string()))?;
    let key = settle_key(registry, index).await?;
    store.mark_edited(index).map_err(store_error)?;
    let backend = store.backend();
    let author = &backend.config().author;
    let sizes = BTreeMap::from([(key.clone(), image_size(&file))]);
    for _ in 0..16 {
        let current = backend.embed_records(&key, index).map_err(store_error)?;
        let stamp = now();
        let ops = replacement_ops(
            &current,
            &wanted,
            file.frame_count,
            backend.default_layer(),
            author,
            &stamp,
            &key,
        );
        if ops.is_empty() {
            return Ok(wanted);
        }
        let envelope = OpEnvelope {
            op_id: new_id(),
            actor: author.clone(),
            ts: stamp,
            op: Op::Batch { ops },
        };
        match backend
            .transact(
                &envelope,
                &sizes,
                Write {
                    checking: Checking::AsLoaded,
                    author,
                    embed_slot: Some(index),
                },
            )
            .map_err(store_error)?
        {
            ApplyResult::Ok { .. } => return Ok(wanted),
            ApplyResult::Conflict { .. } => continue,
            refused => {
                return Err(AnnotationError::Store(format!(
                    "The EMBED replacement was refused: {refused:?}"
                )))
            }
        }
    }
    Err(AnnotationError::Store(
        "The EMBED replacement conflicted 16 times.".to_string(),
    ))
}

/// `GET /api/annotations/export.csv`: the EMBED CSV of every file's view.
///
/// Waits for the import as [`embed_rois`] does. Then one row for each
/// loaded file whose view shows at least one ROI, in the registry's file
/// order (`FileRegistry::files_snapshot`), with the file's path exactly as
/// the registry holds it (`FileEntry::path`, `to_string_lossy`), through
/// `write_embed_csv`. Two files with the same key are two rows, each with
/// its own view. The bytes are those the export had before the store held
/// records; `tests/fixtures/embed-goldens/` freezes them.
///
/// A file's view is read as in [`embed_rois`], so the export hashes
/// nothing: a file without a settled key has no row.
pub async fn export_embed_csv(
    registry: &FileRegistry,
    store: &AnnotationStore,
) -> Result<String, AnnotationError> {
    store
        .wait_until_ready()
        .await
        .map_err(|error| AnnotationError::ImportFailed(error.to_string()))?;
    let mut rows = Vec::new();
    for file in registry.files_snapshot() {
        let rois = file_rois(registry, store, &file)?;
        if rois.num_roi > 0 {
            rows.push((file.path.to_string_lossy().into_owned(), rois));
        }
    }
    write_embed_csv(&rows).map_err(store_error)
}

/// Turns the rows an EMBED CSV matched to loaded files into records, and
/// ends the store's import.
///
/// For each file of `rows`, in ascending index order:
///
/// 1. Its key is settled (`FileRegistry::ensure_key`), which may hash it.
///    A file that cannot be given a key
///    ([`AnnotationError::KeyUnavailable`]) has its rows dropped and is
///    counted in `files_without_key`; the import goes on.
/// 2. Under [`AnnotationStore::load_unless_edited`]: one transaction of
///    `rows_as_creates` as one `Batch`, through `MemoryBackend::transact`
///    with `Checking::AsLoaded`, the author [`EMBED_IMPORT_AUTHOR`] and the
///    file's index as the EMBED slot. The rows are stored exactly as the
///    CSV wrote them: nothing is validated against the image, clamped,
///    reordered or rounded here. A file the EMBED endpoint has already
///    written to is counted in `files_edited` and gets none of its rows. A
///    file with no ROI gets no transaction and is not counted.
///
/// Then [`AnnotationStore::finish_loading`]. When the viewer is stopping
/// ([`AnnotationError::Stopped`]) or the store refuses a transaction, the
/// import is ended with [`AnnotationStore::fail_loading`] and that error is
/// returned; rows already stored stay.
///
/// [`EMBED_IMPORT_AUTHOR`]: crate::annotations::EMBED_IMPORT_AUTHOR
pub async fn import_embed_rows(
    registry: &FileRegistry,
    store: &AnnotationStore,
    rows: AnnotationIndexMap,
) -> Result<EmbedImportReport, AnnotationError> {
    let outcome = async {
        let mut rows: Vec<_> = rows.into_iter().collect();
        rows.sort_unstable_by_key(|(index, _)| *index);
        let mut report = EmbedImportReport::default();
        let author = Author::parse(EMBED_IMPORT_AUTHOR).map_err(store_error)?;
        let backend = store.backend();
        for (index, rois) in rows {
            let Some(file) = registry.get(index) else {
                continue;
            };
            if rois.roi_coords.is_empty() {
                continue;
            }
            let key = match settle_key(registry, index).await {
                Ok(key) => key,
                Err(AnnotationError::KeyUnavailable { .. }) => {
                    report.files_without_key += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let stamp = now();
            let ops = rows_as_creates(&rois, &key, backend.default_layer(), &author, &stamp);
            let envelope = OpEnvelope {
                op_id: new_id(),
                actor: author.clone(),
                ts: stamp,
                op: Op::Batch { ops },
            };
            let sizes = BTreeMap::from([(key, image_size(&file))]);
            let result = store
                .load_unless_edited(index, || {
                    backend.transact(
                        &envelope,
                        &sizes,
                        Write {
                            checking: Checking::AsLoaded,
                            author: &author,
                            embed_slot: Some(index),
                        },
                    )
                })
                .map_err(store_error)?;
            match result {
                None => report.files_edited += 1,
                Some(result) => match result.map_err(store_error)? {
                    ApplyResult::Ok { .. } => report.files_loaded += 1,
                    refused => {
                        return Err(AnnotationError::Store(format!(
                            "The EMBED import was refused: {refused:?}"
                        )))
                    }
                },
            }
        }
        store.finish_loading().map_err(store_error)?;
        Ok(report)
    }
    .await;
    if let Err(error) = &outcome {
        store.fail_loading(error.to_string()).map_err(store_error)?;
    }
    outcome
}

/// `POST /api/annotations/ops`: one operation envelope, as one transaction
/// of the store (`AnnotationBackend`).
///
/// 1. **Keys in.** Every file key the envelope holds is one the session
///    sent: an annotation's or snapshot's `file`, the `file` of
///    `update_annotation` and `mask_tiles`, and the key of a label's `file`
///    or `frame` target, in a `Batch` for each of its operations. Each
///    distinct one is resolved with `FileRegistry::file_for_shown_key`. If
///    any names no file, the envelope is refused here, before any key is
///    settled and without reaching the store: `Invalid` with one
///    `unknown_file` violation whose `path` is empty.
/// 2. **Settling.** Each file named is settled with
///    `FileRegistry::ensure_key`, in the order the keys first appear. This
///    is where a write waits while a file is hashed. A file without a key
///    fails the whole request with [`AnnotationError::KeyUnavailable`]
///    (or [`AnnotationError::Stopped`]) and nothing is applied.
/// 3. The envelope is applied with every key replaced by the settled one
///    (`AnnotationBackend::apply`, one envelope), and `files` answering
///    each settled key with that file's columns, rows and frame count. A
///    key that was replaced since the client learned it is thereby taken
///    as addressed to the file it names now
///    (`docs/design/annotation-model.md` 1.7).
/// 4. **Keys out.** Every file key in the result (the record of a
///    `Conflict`: an annotation's `file`, a label's `file` or `frame`
///    target) is rewritten with `FileRegistry::shown_key_of`.
///
/// It does not wait for the `--annotations` import. A patient, study or
/// series id of the `missing:<file key>` form is text of the client's and
/// is not translated.
pub async fn apply_op(
    registry: &FileRegistry,
    store: &AnnotationStore,
    envelope: OpEnvelope,
) -> Result<OpOutcome, AnnotationError> {
    let mut envelope = envelope;
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    visit_op_keys(&mut envelope.op, &mut |key| {
        if seen.insert(key.clone()) {
            keys.push(key.clone());
        }
        Ok(())
    })?;
    let mut resolved = Vec::new();
    for key in keys {
        let Some(index) = registry.file_for_shown_key(key.as_str()) else {
            return Ok(OpOutcome {
                result: ApplyResult::Invalid {
                    violations: vec![Violation {
                        code: ViolationCode::UnknownFile,
                        path: String::new(),
                        detail: "The operation names a file key this session does not know."
                            .to_string(),
                    }],
                },
                revision: store.backend().revision().map_err(store_error)?,
            });
        };
        resolved.push((key, index));
    }
    let mut settled = HashMap::new();
    let mut replacements = HashMap::new();
    let mut sizes = BTreeMap::new();
    for (shown, index) in resolved {
        let key = if let Some(key) = settled.get(&index) {
            key
        } else {
            let key = settle_key(registry, index).await?;
            let file = registry
                .get(index)
                .ok_or(AnnotationError::NotFound(index))?;
            sizes.insert(key.clone(), image_size(&file));
            settled.entry(index).or_insert(key)
        };
        replacements.insert(shown, key.clone());
    }
    visit_op_keys(&mut envelope.op, &mut |key| {
        *key = replacements.get(key).cloned().ok_or_else(|| {
            AnnotationError::Store("An operation key was not resolved.".to_string())
        })?;
        Ok(())
    })?;
    let mut result = store
        .backend()
        .apply(vec![envelope], &sizes)
        .map_err(store_error)?
        .pop()
        .ok_or_else(|| {
            AnnotationError::Store("The store returned no operation result.".to_string())
        })?;
    visit_result_keys(&mut result, &mut |key| {
        *key = FileKey::parse(&registry.shown_key_of(key)).map_err(store_error)?;
        Ok(())
    })?;
    Ok(OpOutcome {
        result,
        revision: store.backend().revision().map_err(store_error)?,
    })
}

fn store_error(error: impl std::fmt::Display) -> AnnotationError {
    AnnotationError::Store(error.to_string())
}

async fn settle_key(registry: &FileRegistry, index: usize) -> Result<FileKey, AnnotationError> {
    registry
        .ensure_key(index)
        .await
        .map_err(|error| match error {
            KeyError::NotFound(index) => AnnotationError::NotFound(index),
            KeyError::Unavailable(failure) => AnnotationError::KeyUnavailable {
                index,
                reason: failure.into(),
            },
            KeyError::Stopped => AnnotationError::Stopped,
        })
}

fn image_size(file: &FileEntry) -> ImageSize {
    ImageSize {
        columns: file.columns,
        rows: file.rows,
        frames: file.frame_count,
    }
}

fn file_rois(
    registry: &FileRegistry,
    store: &AnnotationStore,
    file: &FileEntry,
) -> Result<EmbedRoiAnnotations, AnnotationError> {
    if let Some(status) = registry.key_status(file.index) {
        if status.settled {
            if let Some(key) = status.key {
                let records = store
                    .backend()
                    .embed_records(&key, file.index)
                    .map_err(store_error)?;
                return Ok(rois_of(&records, file.frame_count));
            }
        }
    }
    Ok(EmbedRoiAnnotations::empty())
}

fn visit_target_key(
    target: &mut LabelTarget,
    visit: &mut impl FnMut(&mut FileKey) -> Result<(), AnnotationError>,
) -> Result<(), AnnotationError> {
    match target {
        LabelTarget::File { file } | LabelTarget::Frame { frame: file, .. } => visit(file),
        _ => Ok(()),
    }
}

fn visit_op_keys(
    op: &mut Op,
    visit: &mut impl FnMut(&mut FileKey) -> Result<(), AnnotationError>,
) -> Result<(), AnnotationError> {
    match op {
        Op::CreateAnnotation { annotation } => visit(&mut annotation.file),
        Op::UpdateAnnotation { file, .. } | Op::MaskTiles { file, .. } => visit(file),
        Op::DeleteAnnotation { snapshot, .. } | Op::RestoreAnnotation { snapshot, .. } => {
            visit(&mut snapshot.file)
        }
        Op::SetLabel { target, .. } => visit_target_key(target, visit),
        Op::CreateLayer { .. } | Op::UpdateLayer { .. } | Op::DeleteLayer { .. } => Ok(()),
        Op::Batch { ops } => {
            for op in ops {
                // Nested batches are refused by validation. Do not descend
                // into client-built recursive input before that check.
                if !matches!(op, Op::Batch { .. }) {
                    visit_op_keys(op, visit)?;
                }
            }
            Ok(())
        }
    }
}

fn visit_result_keys(
    result: &mut ApplyResult,
    visit: &mut impl FnMut(&mut FileKey) -> Result<(), AnnotationError>,
) -> Result<(), AnnotationError> {
    match result {
        ApplyResult::Conflict {
            current: Current::Annotation { record },
        } => visit(&mut record.file),
        ApplyResult::Conflict {
            current: Current::Label { record },
        } => visit_target_key(&mut record.target, visit),
        _ => Ok(()),
    }
}
