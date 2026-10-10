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
//! - **Reads never hash, and neither does the import.** A file whose key
//!   is not settled has no record, because no write got through for it, so
//!   a read answers at once from `FileRegistry::key_status` and from the
//!   rows the import staged (`AnnotationStore`, "Staged rows"). No read
//!   and no import calls `ensure_key`: what the EMBED endpoints show and
//!   export costs what it cost before the store held records, whatever the
//!   size of the files.
//! - **Keys cross the wire in the session's form.** A key from a client is
//!   resolved with `FileRegistry::file_for_shown_key`, and a key sent back
//!   is written with `FileRegistry::shown_key_of`, so a masked session
//!   neither accepts nor sends a key that holds a real UID. Records hold
//!   real keys.
//! - **An operation is recorded on the file it was drawn on, or on none.**
//!   A key in the catalog may be provisional: shared by files that have
//!   one UID and one size and have not been compared, of which
//!   `file_for_shown_key` can name only the first. A key alone therefore
//!   cannot say which of them an operation is for, and nothing the server
//!   does after the fact can find out. So the request says it: beside the
//!   envelope, and outside the annotation model, it may carry the catalog
//!   index of the file the operation was drawn on (the viewer's page
//!   always knows it, and the EMBED endpoints address a file the same
//!   way). Then that file's key is settled, and every key in the envelope
//!   must be it, else [`AnnotationError::KeyReplaced`]; settling has by
//!   then given that file its own key in the catalog, whatever happened to
//!   the others that shared the old one, so one look at the catalog gives
//!   the key the operation goes through under. A request without an index
//!   is refused with [`AnnotationError::KeyAmbiguous`] when it names a
//!   `sop:` key whose UID more than one loaded file carries; any other key
//!   it names must still be the settled key of the file it resolves to
//!   (`docs/design/annotation-model.md` 1.7: records "stay with the file
//!   they were drawn on").

use super::{FileRegistry, KeyError};
use crate::annotations::{
    canonicalize_annotations,
    embed::{replacement_ops, rows_as_creates, write_embed_csv},
    memory::{now, Checking, ViewCheck, Write},
    AnnotationBackend, AnnotationIndexMap, AnnotationStore, EmbedRoiAnnotations,
    EMBED_IMPORT_AUTHOR,
};
use crate::api::contracts::FileKeyError;
use crate::types::FileEntry;
use dcmview_annotation::{
    new_id, ApplyResult, Author, Current, FileKey, ImageSize, KeyScheme, LabelTarget, Op,
    OpEnvelope, Violation, ViolationCode,
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
    ///
    /// `group` says the file itself is not the one at fault: its key is
    /// `sop:<uid>`, which names the bytes of the first file loaded with
    /// that UID, and it is that file that could not be read or has
    /// changed. A file that is readable and unchanged is then still
    /// without a key.
    #[error("file {index} has no key, so no annotation can be saved for it: {} {}", if *.group { "the first file loaded with its SOP Instance UID, which its key is checked against," } else { "it" }, match .reason { FileKeyError::Unreadable => "could not be read", FileKeyError::Changed => "changed after it was loaded", FileKeyError::Uncompared => "could not be compared" })]
    KeyUnavailable {
        index: usize,
        reason: FileKeyError,
        group: bool,
    },
    /// An operation without a file index named a file by a `sop:` key
    /// whose UID more than one loaded file carries. Nothing was settled or
    /// applied.
    #[error("the file key {sent} is carried by more than one loaded file: say which file the operation is for (the `file` query)")]
    KeyAmbiguous { sent: String },
    /// An operation named a file by a key that is not the settled key of
    /// the file it resolves to: the key was provisional and the files that
    /// shared it turned out to differ, or it was replaced since the client
    /// learned it. Nothing was applied. Settling has by now made the
    /// catalog show the current key of the files that were compared.
    #[error("the file key {sent} is no longer the key of one file: read the file's key from the catalog and send the operation again")]
    KeyReplaced { sent: String },
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
    /// The store's revision as the envelope's transaction left it
    /// (`Transacted::revision`), not one read afterwards: two applied
    /// envelopes never report the same revision.
    pub revision: u64,
}

/// What an EMBED import did with its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmbedImportReport {
    /// Files whose rows are now staged for the EMBED view.
    pub files_loaded: usize,
    /// Files whose rows were dropped because the EMBED endpoint had already
    /// replaced that file's ROIs.
    pub files_edited: usize,
}

/// `GET /api/file/{index}/annotations`: the file's EMBED view.
///
/// Waits for the `--annotations` import ([`AnnotationStore::wait_until_ready`];
/// a failed import is [`AnnotationError::ImportFailed`] with its message).
/// Then, without waiting for anything else, the file's view:
///
/// 1. The file's key is not settled (`FileRegistry::key_status`): the rows
///    staged for it ([`AnnotationStore::staged_rows`]) exactly as they are,
///    or no ROI (`EmbedRoiAnnotations::empty`) when it has none. No record
///    exists under a key that is not settled.
/// 2. The file's key is settled: first, rows still staged for it are made
///    records ([`AnnotationStore::take_staged`]: one transaction of
///    `rows_as_creates` as one `Batch`, through `MemoryBackend::transact`
///    with `Checking::AsLoaded`, the author [`EMBED_IMPORT_AUTHOR`], this
///    index as the EMBED slot and no view check). Then `rois_of` the
///    records `MemoryBackend::embed_records` gives for that key and this
///    index.
///
/// The same view is what [`export_embed_csv`] writes for the file. Rows
/// the mapping read come back from it unchanged, so the two cases show the
/// same ROIs for the same rows.
///
/// It never starts or waits for hashing. Rectangles made through the
/// operation endpoint are in the view; other geometries are not.
///
/// [`EMBED_IMPORT_AUTHOR`]: crate::annotations::EMBED_IMPORT_AUTHOR
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
///    running import drops its rows for it, and rows already staged for it
///    are dropped ([`AnnotationStore::take_staged`]): the save replaces
///    them.
/// 4. One transaction against the view it read. The view's records are
///    read (`MemoryBackend::embed_records`);
///    [`AnnotationStore::before_embed_write`] is called; then
///    `replacement_ops` from those records to the canonical list go, as one
///    `Batch` under a new `op_id`, through `MemoryBackend::transact` with
///    `Checking::AsLoaded`, the store's own author, this index as the EMBED
///    slot, and a `ViewCheck` of this key, this index and those records.
///    The check is what makes the save whole: when another write changed
///    the view in between, the transaction is a `Conflict` and nothing of
///    it is applied; the view is read again and the step repeated, up to 16
///    times, then [`AnnotationError::Store`]. Of two saves of one file the
///    one whose transaction is applied last decides everything the view
///    shows; ROIs of the other never remain beside it.
///
///    No operation means no transaction: saving what is already shown
///    changes no record and no revision.
///
/// The answer is the canonical list of step 1, and a read of the view
/// right after the save shows exactly that list.
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
    store.take_staged(index, drop).map_err(store_error)?;
    let backend = store.backend();
    let author = &backend.config().author;
    let sizes = BTreeMap::from([(key.clone(), image_size(&file))]);
    for _ in 0..16 {
        let current = backend.embed_records(&key, index).map_err(store_error)?;
        store.before_embed_write();
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
                    import: false,
                    view: Some(ViewCheck {
                        key: &key,
                        slot: index,
                        records: &current,
                    }),
                },
            )
            .map_err(store_error)?
            .result
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
/// nothing: a file whose key is not settled is written from its staged
/// rows, and has no row when it has none.
///
/// The export is one read. The staged rows are read under the import's
/// lock (`AnnotationStore::with_staged`), which is held to the end. Rows
/// staged for a file whose key is settled are made records first; then the
/// views of every file with a settled key are read with the store held
/// still (`MemoryBackend::read_views`), and which keys are settled is read
/// inside that same hold. A record is written only after its file's key
/// was settled, so every record the read holds is seen with its key; a
/// write that comes during the export is in no row. No row of the CSV
/// therefore shows a write that another row of the same CSV does not. A
/// file whose key settles while the export runs is written from its
/// staged rows. While the store holds no annotation at all, every view is
/// its file's staged rows and no key is looked at.
pub async fn export_embed_csv(
    registry: &FileRegistry,
    store: &AnnotationStore,
) -> Result<String, AnnotationError> {
    store
        .wait_until_ready()
        .await
        .map_err(|error| AnnotationError::ImportFailed(error.to_string()))?;
    let files = registry.files_snapshot();
    let backend = store.backend();
    store
        .with_staged(|staged| {
            // With no annotation in the store every view is the file's
            // staged rows, whatever its key: no key is looked at.
            if backend.holds_no_annotation().map_err(store_error)? {
                let rows = files.iter().filter_map(|file| {
                    let rois = staged.get(&file.index)?;
                    (rois.num_roi > 0).then(|| (file.path.to_string_lossy(), rois))
                });
                return write_embed_csv(rows).map_err(store_error);
            }
            // Rows staged for a file whose key is settled become records
            // first, as a read of that file's view would make them.
            let indexes: Vec<usize> = staged.keys().copied().collect();
            for index in indexes {
                let Some(key) = settled_key(registry, index) else {
                    continue;
                };
                if let (Some(file), Some(rows)) = (files.get(index), staged.remove(&index)) {
                    make_records(store, file, &key, &rows)?;
                }
            }
            // Then one read with the store held still. Which files have a
            // settled key is read inside it: a key settles before its
            // first record is written, so a record the read holds is under
            // a key that is seen as settled here, and a write that waits
            // for this read is in none of the rows.
            let rows: Vec<(
                std::borrow::Cow<'_, str>,
                std::borrow::Cow<'_, EmbedRoiAnnotations>,
            )> = backend
                .read_views(|view| {
                    files
                        .iter()
                        .filter_map(|file| {
                            let rois = match staged.get(&file.index) {
                                // Its key settled after the step above:
                                // its rows as the import read them.
                                Some(rows) => std::borrow::Cow::Borrowed(rows),
                                None => std::borrow::Cow::Owned(view(
                                    &settled_key(registry, file.index)?,
                                    file.index,
                                    file.frame_count,
                                )),
                            };
                            (rois.num_roi > 0).then(|| (file.path.to_string_lossy(), rois))
                        })
                        .collect()
                })
                .map_err(store_error)?;
            write_embed_csv(
                rows.iter()
                    .map(|(path, rois)| (path.clone(), rois.as_ref())),
            )
            .map_err(store_error)
        })
        .map_err(store_error)?
}

/// Ends the store's import with the rows an EMBED CSV matched to loaded
/// files: stages them ([`AnnotationStore::stage_rows`]) and lets the EMBED
/// reads through.
///
/// It reads no file, settles no key and makes no record, so it costs what
/// keeping the rows costs and does not depend on the size of the files.
/// The rows are kept exactly as the CSV wrote them: nothing is validated
/// against the image, clamped, reordered or rounded. A file the EMBED
/// endpoint has already written to is counted in `files_edited` and gets
/// none of its rows; a file with no ROI, and an index no file has, are
/// left out and not counted.
pub fn import_embed_rows(
    registry: &FileRegistry,
    store: &AnnotationStore,
    rows: AnnotationIndexMap,
) -> Result<EmbedImportReport, AnnotationError> {
    let files = registry.status().file_count;
    let rows = rows
        .into_iter()
        .filter(|(index, _)| *index < files)
        .collect();
    let (files_loaded, files_edited) = store.stage_rows(rows).map_err(store_error)?;
    Ok(EmbedImportReport {
        files_loaded,
        files_edited,
    })
}

/// `POST /api/annotations/ops`: one operation envelope, as one transaction
/// of the store (`AnnotationBackend`). `file` is the catalog index of the
/// file the operation was drawn on, when the request gave one.
///
/// 1. **Keys in.** Every file key the envelope holds is one the session
///    sent: an annotation's or snapshot's `file`, the `file` of
///    `update_annotation` and `mask_tiles`, and the key of a label's `file`
///    or `frame` target, in a `Batch` for each of its operations. Each
///    distinct one is resolved with `FileRegistry::file_for_shown_key`. If
///    any names no file, the envelope is refused here, before any key is
///    settled and without reaching the store: `Invalid` with one
///    `unknown_file` violation whose `path` is empty.
/// 2. **Which file.** With `file`, that file is the one every key is held
///    to ([`AnnotationError::NotFound`] when no file has the index).
///    Without it, each key is held to the file it resolved to, and a `sop:`
///    key whose UID more than one loaded file carries
///    (`FileRegistry::shares_uid`) fails the request with
///    [`AnnotationError::KeyAmbiguous`] before anything is read.
/// 3. **Settling.** Each file is settled with `FileRegistry::ensure_key`.
///    This is where a write waits while a file is hashed. A file without a
///    key fails the whole request with
///    [`AnnotationError::KeyUnavailable`] (or [`AnnotationError::Stopped`])
///    and nothing is applied.
/// 4. **The key must be the file's own.** For each key, the settled key of
///    its file, written with `FileRegistry::shown_key_of`, must be the text
///    the client sent. When one is not, the whole request fails with
///    [`AnnotationError::KeyReplaced`] naming the first such key as it was
///    sent, and nothing is applied. With `file`, the catalog shows that
///    file's settled key from this moment on.
/// 5. Rows still staged for a file the envelope names are made records
///    first, as in [`embed_rois`], so the operation meets them as records.
/// 6. The envelope is applied, with every key replaced by the settled one,
///    as one `MemoryBackend::transact` with `Checking::Strict`, the store's
///    author, no EMBED slot and no view check, and `files` answering each
///    settled key with that file's columns, rows and frame count.
/// 7. **Keys out.** Every file key in the result (the record of a
///    `Conflict`: an annotation's `file`, a label's `file` or `frame`
///    target) is rewritten with `FileRegistry::shown_key_of`.
///
/// `revision` in the outcome is `Transacted::revision` of step 6, and the
/// store's revision when the request was refused in step 1.
///
/// It does not wait for the `--annotations` import. A patient, study or
/// series id of the `missing:<file key>` form is text of the client's and
/// is not translated.
pub async fn apply_op(
    registry: &FileRegistry,
    store: &AnnotationStore,
    envelope: OpEnvelope,
    file: Option<usize>,
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
        // The file the request names is the one meant, whatever file the
        // key alone would resolve to.
        resolved.push((key, file.unwrap_or(index)));
    }
    if let Some(index) = file {
        registry
            .get(index)
            .ok_or(AnnotationError::NotFound(index))?;
    } else if let Some((sent, _)) = resolved
        .iter()
        .find(|(key, index)| key.scheme() == KeyScheme::Sop && registry.shares_uid(*index))
    {
        return Err(AnnotationError::KeyAmbiguous {
            sent: sent.as_str().to_string(),
        });
    }
    let mut settled = HashMap::new();
    let mut sizes = BTreeMap::new();
    for (_, index) in &resolved {
        if !settled.contains_key(index) {
            let key = settle_key(registry, *index).await?;
            let file = registry
                .get(*index)
                .ok_or(AnnotationError::NotFound(*index))?;
            sizes.insert(key.clone(), image_size(&file));
            settled.insert(*index, (key, file));
        }
    }
    let mut replacements = HashMap::new();
    for (sent, index) in &resolved {
        let (key, _) = &settled[index];
        if registry.shown_key_of(key) != sent.as_str() {
            return Err(AnnotationError::KeyReplaced {
                sent: sent.as_str().to_string(),
            });
        }
        replacements.insert(sent.clone(), key.clone());
    }
    for (key, file) in settled.values() {
        store_staged_rows(store, file, key)?;
    }
    visit_op_keys(&mut envelope.op, &mut |key| {
        *key = replacements.get(key).cloned().ok_or_else(|| {
            AnnotationError::Store("An operation key was not resolved.".to_string())
        })?;
        Ok(())
    })?;
    let transacted = store
        .backend()
        .transact(
            &envelope,
            &sizes,
            Write {
                checking: Checking::Strict,
                author: &store.backend().config().author,
                embed_slot: None,
                import: false,
                view: None,
            },
        )
        .map_err(store_error)?;
    let mut result = transacted.result;
    visit_result_keys(&mut result, &mut |key| {
        *key = FileKey::parse(&registry.shown_key_of(key)).map_err(store_error)?;
        Ok(())
    })?;
    Ok(OpOutcome {
        result,
        revision: transacted.revision,
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
                // No failure of its own on record: the group's first file's.
                group: registry
                    .key_status(index)
                    .is_some_and(|status| status.failure.is_none()),
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

/// The file's key when it is settled.
fn settled_key(registry: &FileRegistry, index: usize) -> Option<FileKey> {
    registry
        .key_status(index)
        .filter(|status| status.settled)
        .and_then(|status| status.key)
}

fn file_rois(
    registry: &FileRegistry,
    store: &AnnotationStore,
    file: &FileEntry,
) -> Result<EmbedRoiAnnotations, AnnotationError> {
    let Some(key) = settled_key(registry, file.index) else {
        return Ok(store
            .staged_rows(file.index)
            .map_err(store_error)?
            .unwrap_or_else(EmbedRoiAnnotations::empty));
    };
    store_staged_rows(store, file, &key)?;
    store
        .backend()
        .embed_rois(&[(key, file.index, file.frame_count)])
        .map_err(store_error)?
        .pop()
        .ok_or_else(|| AnnotationError::Store("The store returned no view.".to_string()))
}

/// Makes the rows still staged for `file` records under its settled key.
fn store_staged_rows(
    store: &AnnotationStore,
    file: &FileEntry,
    key: &FileKey,
) -> Result<(), AnnotationError> {
    store
        .take_staged(file.index, |rows| make_records(store, file, key, &rows))
        .map_err(store_error)?
        .unwrap_or(Ok(()))
}

/// One transaction that makes import rows of `file` records.
fn make_records(
    store: &AnnotationStore,
    file: &FileEntry,
    key: &FileKey,
    rows: &EmbedRoiAnnotations,
) -> Result<(), AnnotationError> {
    let author = Author::parse(EMBED_IMPORT_AUTHOR).map_err(store_error)?;
    let stamp = now();
    let backend = store.backend();
    let envelope = OpEnvelope {
        op_id: new_id(),
        actor: author.clone(),
        ts: stamp.clone(),
        op: Op::Batch {
            ops: rows_as_creates(rows, key, backend.default_layer(), &author, &stamp),
        },
    };
    let sizes = BTreeMap::from([(key.clone(), image_size(file))]);
    let transacted = backend
        .transact(
            &envelope,
            &sizes,
            Write {
                checking: Checking::AsLoaded,
                author: &author,
                embed_slot: Some(file.index),
                import: true,
                view: None,
            },
        )
        .map_err(store_error)?;
    match transacted.result {
        ApplyResult::Ok { .. } => Ok(()),
        refused => Err(AnnotationError::Store(format!(
            "The staged EMBED rows were refused: {refused:?}"
        ))),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::{MemoryBackend, MemoryConfig};
    use dcmview_annotation::Author;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};

    fn rois(coords: [u32; 4]) -> EmbedRoiAnnotations {
        EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![coords],
            roi_frames: Vec::new(),
        }
    }

    /// A save is "replace this file's ROIs": of two saves of one file the
    /// one applied last decides the whole view. Here a second save gets in
    /// at the one point where it could do harm, after the first has read
    /// the view (empty) and before it writes. The first save must notice,
    /// read again and replace what the second left.
    #[tokio::test]
    async fn of_two_saves_of_one_file_the_one_applied_last_decides_the_whole_view() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let registry = FileRegistry::from_files(vec![crate::loader::test_entry(&fixture)]);
        let session: Arc<OnceLock<(FileRegistry, AnnotationStore)>> = Arc::default();
        let (inside, interleaved) = (session.clone(), Arc::new(AtomicBool::new(false)));
        let ran = interleaved.clone();
        let store = AnnotationStore::with_backend(MemoryBackend::new(MemoryConfig {
            author: Author::parse("user:test").expect("author"),
            schema: None,
        }))
        .with_before_embed_write(Arc::new(move || {
            // Once: the save made here reaches this point too.
            if ran.swap(true, Ordering::SeqCst) {
                return;
            }
            let (registry, store) = inside.get().expect("session");
            futures::executor::block_on(replace_embed_rois(registry, store, 0, rois([0, 0, 1, 1])))
                .expect("the save in between");
        }));
        assert!(session.set((registry.clone(), store.clone())).is_ok());

        let saved = replace_embed_rois(&registry, &store, 0, rois([1, 1, 2, 2]))
            .await
            .expect("the first save");

        assert!(
            interleaved.load(Ordering::SeqCst),
            "a save reads the view, then calls before_embed_write, then writes"
        );
        assert_eq!(saved, rois([1, 1, 2, 2]));
        assert_eq!(
            embed_rois(&registry, &store, 0).await.expect("view"),
            rois([1, 1, 2, 2]),
            "the ROI of the save in between must not remain beside it"
        );
    }
    /// The view a save was worked out from is its records at their
    /// revisions, not only which records: a record another client changed
    /// in between, and which the save would not have touched, is put back
    /// to what the save says.
    #[tokio::test]
    async fn a_save_notices_a_record_that_changed_under_it() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let registry = FileRegistry::from_files(vec![crate::loader::test_entry(&fixture)]);
        let session: Arc<OnceLock<(FileRegistry, AnnotationStore)>> = Arc::default();
        let (inside, interleaved) = (session.clone(), Arc::new(AtomicBool::new(false)));
        let ran = interleaved.clone();
        let store = AnnotationStore::with_backend(MemoryBackend::new(MemoryConfig {
            author: Author::parse("user:test").expect("author"),
            schema: None,
        }))
        .with_before_embed_write(Arc::new(move || {
            // Once, and only during the save under test.
            let Some((registry, store)) = inside.get() else {
                return;
            };
            if ran.swap(true, Ordering::SeqCst) {
                return;
            }
            // An operation moves the first ROI: same records, one new rev.
            let key = registry.key_status(0).and_then(|status| status.key).expect("key");
            let record = &store.backend().embed_records(&key, 0).expect("view")[0];
            let moved = serde_json::json!({
                "op_id": new_id(), "actor": "user:test", "ts": "2026-10-09T08:00:00.000Z",
                "op": {
                    "type": "update_annotation", "id": record.id, "file": key,
                    "base_rev": record.meta.rev,
                    "before": { "geometry": { "type": "rect", "x0": 0, "y0": 0, "x1": 1, "y1": 1 } },
                    "after": { "geometry": { "type": "rect", "x0": 2, "y0": 2, "x1": 3, "y1": 3 } },
                },
            });
            let envelope = serde_json::from_value(moved).expect("envelope");
            let outcome = futures::executor::block_on(apply_op(registry, store, envelope, None))
                .expect("the operation in between");
            assert!(matches!(outcome.result, ApplyResult::Ok { .. }), "{outcome:?}");
        }));
        let two = |second: [u32; 4]| EmbedRoiAnnotations {
            num_roi: 2,
            roi_coords: vec![[0, 0, 1, 1], second],
            roi_frames: Vec::new(),
        };
        replace_embed_rois(&registry, &store, 0, two([1, 1, 2, 2]))
            .await
            .expect("two ROIs");
        assert!(session.set((registry.clone(), store.clone())).is_ok());

        // The save keeps the first ROI as it was and changes the second.
        let saved = replace_embed_rois(&registry, &store, 0, two([1, 1, 3, 3]))
            .await
            .expect("the save");

        assert!(interleaved.load(Ordering::SeqCst));
        assert_eq!(
            embed_rois(&registry, &store, 0).await.expect("view"),
            saved,
            "the view is what the save answered, the moved ROI included"
        );
    }
    /// A row the import read past the image edge is held as loaded, and an
    /// operation that only deletes or restores it does not judge it: it
    /// goes and comes back as it was.
    #[tokio::test]
    async fn a_leniently_loaded_record_can_be_deleted_and_restored_as_it_was() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let registry = FileRegistry::from_files(vec![crate::loader::test_entry(&fixture)]);
        let store = AnnotationStore::loading();
        let outside = EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[500, 600, 700, 800]],
            roi_frames: Vec::new(),
        };
        import_embed_rows(
            &registry,
            &store,
            std::collections::HashMap::from([(0, outside.clone())]),
        )
        .expect("import");
        let key = settle_key(&registry, 0).await.expect("key");
        assert_eq!(
            embed_rois(&registry, &store, 0).await.expect("view"),
            outside
        );
        let record = store.backend().embed_records(&key, 0).expect("records")[0].clone();
        let send = |kind: &str, base_rev: u64| {
            let envelope = serde_json::from_value(serde_json::json!({
                "op_id": new_id(), "actor": "user:test", "ts": "2026-10-09T08:00:00.000Z",
                "op": { "type": kind, "id": record.id, "base_rev": base_rev, "snapshot": record },
            }))
            .expect("envelope");
            apply_op(&registry, &store, envelope, Some(0))
        };

        let deleted = send("delete_annotation", 1).await.expect("delete");
        assert!(
            matches!(deleted.result, ApplyResult::Ok { .. }),
            "{deleted:?}"
        );
        assert_eq!(
            embed_rois(&registry, &store, 0)
                .await
                .expect("view")
                .num_roi,
            0
        );
        let restored = send("restore_annotation", 2).await.expect("restore");
        assert!(
            matches!(restored.result, ApplyResult::Ok { .. }),
            "{restored:?}"
        );
        assert_eq!(
            embed_rois(&registry, &store, 0).await.expect("view"),
            outside
        );
    }
}
