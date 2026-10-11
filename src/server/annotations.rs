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
    canonicalize_annotations, common_directory, document_path,
    embed::{replacement_ops, rows_as_creates},
    memory::{now, Checking, ViewCheck, Write},
    AnnotationBackend, AnnotationIndexMap, AnnotationStore, EmbedRoiAnnotations, FileLookup,
    ImportStatus, NativeSource, EMBED_IMPORT_AUTHOR, NATIVE_IMPORT_AUTHOR,
};
use crate::api::contracts::{AnnotationImportResponse, AnnotationImportState, FileKeyError};
use crate::types::FileEntry;
use dcmview_adapters::embed::EmbedRow;
use dcmview_adapters::{
    ExportAdapter, ExportContext, ExportError, FileResolver, ImportAdapter, ImportContext,
    ImportError, LossCode, Matcher, NativeJson, NativeJsonLines, Refusal, Report, Resolution,
    Resolved,
};
use dcmview_annotation::{
    new_id, ApplyResult, Author, Current, FileKey, FileKind, FileRef, ImageSize, KeyScheme,
    LabelTarget, Op, OpEnvelope, Space, Violation, ViolationCode,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

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
    /// The session's records do not fit one JSON document
    /// (`dcmview_annotation::limits::MAX_DOCUMENT_BYTES`).
    #[error(
        "the annotations do not fit one JSON document of {limit} bytes; export them as JSON Lines"
    )]
    ExportTooLarge { limit: u64 },
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
                    bindings: None,
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
/// loaded file whose view shows at least one ROI, with the file's path
/// exactly as the registry holds it (`FileEntry::path`,
/// `to_string_lossy`), through the EMBED adapter's writer
/// (`dcmview_adapters::embed::write_csv`), which sorts the rows by path.
/// Two files with the same key are two rows, each with its own view. Each
/// row's bytes are those the export always wrote;
/// `tests/fixtures/embed-goldens/` freezes them.
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
                    (rois.num_roi > 0).then(|| embed_row(file.path.to_string_lossy(), rois))
                });
                return embed_csv(rows);
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
            embed_csv(
                rows.iter()
                    .map(|(path, rois)| embed_row(path.clone(), rois.as_ref())),
            )
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

/// Which native form an export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeForm {
    /// One JSON document (`dcmview_adapters::NativeJson`).
    Json,
    /// JSON Lines (`dcmview_adapters::NativeJsonLines`).
    JsonLines,
}

/// A native export, ready to send.
pub struct NativeExport {
    pub bytes: Vec<u8>,
    /// What the adapter wrote.
    pub report: Report,
}

/// `GET /api/annotations/export.json` and `export.jsonl`: every record of
/// the session as a `dcmview.annotations` document.
///
/// In this order:
///
/// 1. Waits for the `--annotations` import, as [`embed_rois`] does
///    ([`AnnotationError::ImportFailed`]).
/// 2. Takes the session's document export permit
///    (`AnnotationStore::export_permit`), before anything else is read or
///    copied, and holds it until the text is written (step 7), not while
///    the text is sent. So one document is built at a time, a second export
///    waits here holding nothing, and a client that reads its download
///    slowly holds up no other export.
/// 3. **Staged rows become records first.** A document holds records, and
///    a record holds a settled key. For every loaded file, in index order,
///    that has rows staged (`AnnotationStore::with_staged` says which): its key is
///    settled with `FileRegistry::ensure_key`, which is where the export
///    waits while a file is hashed, and its rows are made records exactly
///    as a read of its view makes them ([`embed_rois`], step 2). A file
///    that cannot be given a key fails the export with
///    [`AnnotationError::KeyUnavailable`] for that file
///    ([`AnnotationError::Stopped`] when the viewer is stopping), and
///    nothing is written out: an export never leaves rows out silently.
///    Rows that became records before the failure stay records, which
///    changes nothing any view shows. A file without staged rows is not
///    settled and not read.
/// 4. One read of the store, `AnnotationBackend::export`: every layer,
///    annotation and label at one moment. A write that comes after it is
///    not in the document; one that came before it is there whole.
/// 5. **Files.** For each file key the document's annotations name, then
///    each a label names in a `file` or `frame` target, in the order first
///    named, one `FileRef` in `files`, for the loaded file with the lowest
///    index whose settled key it is (`FileRegistry::key_status`; no key is
///    settled here). A record whose key no loaded file has is
///    [`AnnotationError::Store`]. The reference holds:
///    - `key`: the key as the session shows it
///      (`FileRegistry::shown_key_of`);
///    - `kind` and `format`: `dicom`, or `raster` with the file's format
///      name (`FileFormat::as_str`);
///    - `sop_instance_uid`, `sop_class_uid`, `study_instance_uid`,
///      `series_instance_uid`, `patient_id`: the file's, `None` for an
///      empty one. A masked session (`FileRegistry::masker`) writes the
///      patient id and the three instance UIDs as its catalog shows them
///      (`Masker::summary`), never the file's own;
///    - `path`: `annotations::document_path` below
///      `annotations::common_directory` of the files loaded when the
///      export reads them;
///    - `size_bytes`, `rows`, `columns`, `frames` (the frame count): the
///      file's;
///    - `digest`: `b3:` and the 64 lowercase hex digits of the file's
///      digest when the registry has computed it
///      (`FileKeyStatus::digest`), else `None`;
///    - `space`: `stored`, with the raster's EXIF or TIFF orientation when
///      it is not 1;
///    - `pixel_digest` empty; `frame_source`, `external_id` and `spacing`
///      `None`.
/// 6. **Keys go out in the session's form.** Every annotation's `file` and
///    every `file` or `frame` label target is rewritten with
///    `FileRegistry::shown_key_of`. A patient, study, series or folder
///    target is written as it is held.
/// 7. The document is written by the adapter for `form` with no path
///    lookup and no bound of the caller's
///    (`ExportContext::max_bytes` is `u64::MAX`), into memory.
///    `ExportError::TooLarge` is [`AnnotationError::ExportTooLarge`] with
///    its limit; any other failure is [`AnnotationError::Store`].
///
/// Steps 4 to 7 copy every record and write every one, so they run on a
/// blocking thread (`tokio::task::spawn_blocking`), never on the runtime's
/// own; a thread that fails is [`AnnotationError::Store`].
///
/// What exports make the process hold: the copy of the records and the
/// text for the one export being built, and the text of each export that
/// is being sent. Nothing a record holds is
/// checked, clamped or left out, so a rectangle the EMBED import kept as
/// read is written as it is held. Redaction boxes are in another store and
/// are never in a document.
pub async fn export_native(
    registry: &FileRegistry,
    store: &AnnotationStore,
    form: NativeForm,
) -> Result<NativeExport, AnnotationError> {
    store
        .wait_until_ready()
        .await
        .map_err(|error| AnnotationError::ImportFailed(error.to_string()))?;
    let permit = store.export_permit().await.map_err(store_error)?;
    let files = registry.files_snapshot();
    let mut indexes = store
        .with_staged(|staged| staged.keys().copied().collect::<Vec<_>>())
        .map_err(store_error)?;
    indexes.sort_unstable();
    for index in indexes {
        let key = settle_key(registry, index).await?;
        let file = files.get(index).ok_or(AnnotationError::NotFound(index))?;
        store_staged_rows(store, file, &key)?;
    }
    let registry = registry.clone();
    let backend = store.backend().clone();
    tokio::task::spawn_blocking(move || {
        let mut document = backend.export().map_err(store_error)?;
        let mut by_key = HashMap::new();
        for file in &files {
            if let Some(key) = settled_key(&registry, file.index) {
                by_key
                    .entry(key)
                    .and_modify(|index: &mut usize| *index = (*index).min(file.index))
                    .or_insert(file.index);
            }
        }
        let root = common_directory(&files);
        let mut shown = HashMap::<FileKey, FileKey>::new();
        let mut visit = |key: &mut FileKey| {
            if let Some(replacement) = shown.get(key) {
                *key = replacement.clone();
                return Ok(());
            }
            let file = by_key
                .get(key)
                .and_then(|index| files.get(*index))
                .ok_or_else(|| {
                    AnnotationError::Store("The store names a file that is not loaded.".to_string())
                })?;
            let replacement = FileKey::parse(&registry.shown_key_of(key)).map_err(store_error)?;
            document.files.push(document_file_ref(
                &registry,
                file,
                replacement.clone(),
                root.as_deref(),
            ));
            shown.insert(key.clone(), replacement.clone());
            *key = replacement;
            Ok(())
        };
        for annotation in &mut document.annotations {
            visit(&mut annotation.file)?;
        }
        for label in &mut document.labels {
            visit_target_key(&mut label.target, &mut visit)?;
        }
        let paths = BTreeMap::<FileKey, Vec<String>>::new();
        let context = ExportContext {
            paths: &paths,
            max_bytes: u64::MAX,
            limits: dcmview_adapters::Limits::DEFAULT,
        };
        let mut bytes = Vec::new();
        let report = match form {
            NativeForm::Json => NativeJson.export(&document, &context, &mut bytes),
            NativeForm::JsonLines => NativeJsonLines.export(&document, &context, &mut bytes),
        }
        .map_err(|error| match error {
            ExportError::TooLarge { limit } => AnnotationError::ExportTooLarge { limit },
            error => store_error(error),
        })?;
        // The permit is held to here: while the records are copied and
        // their text is written, and no longer.
        drop(permit);
        Ok(NativeExport { bytes, report })
    })
    .await
    .map_err(|_| AnnotationError::Store("The annotation export thread failed.".to_string()))?
}

fn document_file_ref(
    registry: &FileRegistry,
    file: &FileEntry,
    key: FileKey,
    root: Option<&std::path::Path>,
) -> FileRef {
    let summary = registry.masker().map(|masker| masker.summary(file));
    let present = |value: &str| (!value.is_empty()).then(|| value.to_string());
    FileRef {
        key,
        kind: if file.format.is_raster() {
            FileKind::Raster
        } else {
            FileKind::Dicom
        },
        format: file.format.as_str().to_string(),
        sop_instance_uid: present(
            summary
                .as_ref()
                .map_or(&file.sop_instance_uid, |summary| &summary.sop_instance_uid),
        ),
        sop_class_uid: present(&file.sop_class_uid),
        study_instance_uid: present(
            summary
                .as_ref()
                .map_or(&file.study_instance_uid, |summary| {
                    &summary.study_instance_uid
                }),
        ),
        series_instance_uid: present(
            summary
                .as_ref()
                .map_or(&file.series_instance_uid, |summary| {
                    &summary.series_instance_uid
                }),
        ),
        patient_id: present(
            summary
                .as_ref()
                .map_or(&file.patient_id, |summary| &summary.patient_id),
        ),
        path: document_path(root, &file.path),
        size_bytes: file.size_bytes,
        rows: file.rows,
        columns: file.columns,
        frames: file.frame_count,
        digest: registry
            .key_status(file.index)
            .and_then(|status| status.digest)
            .map(|digest| FileKey::blake3(&digest).as_str().to_string()),
        space: Space {
            exif_orientation: file
                .raster
                .as_ref()
                .map(|raster| raster.orientation)
                .filter(|value| *value != 1),
            ..Space::default()
        },
        pixel_digest: Vec::new(),
        frame_source: None,
        external_id: None,
        spacing: None,
        unknown: BTreeMap::new(),
    }
}

/// Ends the store's import with a native `--annotations` document
/// (`dcmview_adapters::native`): reads it, places its records on the
/// loaded files and applies them as one transaction. All or nothing.
///
/// # Reading
///
/// The file is opened and read on a blocking thread
/// (`tokio::task::spawn_blocking`) by `dcmview_adapters::NativeJson`, with
/// this context: the session's resolver (below); the store's label schema
/// (`MemoryConfig::schema`); the store's default layer; the author
/// [`NATIVE_IMPORT_AUTHOR`] and `now()`; and `keep_going`.
///
/// # Which file a record lands on
///
/// The resolver answers for one `FileRef` of the document, over the files
/// loaded when the import starts (`FileRegistry::files_snapshot`, taken
/// once). It finds a candidate by the first of these that names one:
///
/// 1. **The key**, as this session shows keys:
///    `FileRegistry::file_for_shown_key`. A `sop:` key whose UID more than
///    one loaded file carries (`FileRegistry::shares_uid`) names none of
///    them here: a shared key cannot say which file is meant.
/// 2. **The SOP Instance UID**, when the reference has one that can be a
///    key (`FileKey::sop`): as step 1 with that key. So a masked session
///    matches a UID in the form it shows and never a file's real UID.
/// 3. **The path**: a relative path below
///    `annotations::common_directory` of the loaded files (the directory
///    an export of this session writes paths below), an absolute one as it
///    is, each also by its canonical form (`annotations::FileLookup`); of
///    several files the one with the lowest index.
///
/// No candidate is `Resolution::Unmatched`. A candidate is then checked,
/// in this order:
///
/// - when the reference and the file both have a SOP Instance UID and the
///   reference's is neither the file's own nor the one this session shows
///   for it (`Masker::uid`): `Refusal::EvidenceConflict`. So a document
///   that a session without masking wrote loads into a masked one, its
///   files found by their paths;
/// - the file's key is settled with `FileRegistry::ensure_key`, waited for
///   from the blocking thread (`tokio::runtime::Handle::block_on`), which
///   may hash the file. A file that cannot be given a key, or a viewer that
///   is stopping, is `Refusal::KeyUnavailable`;
/// - when the reference has a `b3:` digest and the registry has computed
///   the file's (`FileKeyStatus::digest`) and they differ:
///   `Refusal::EvidenceConflict`. Nothing is hashed to find out.
///
/// What is left is `Resolution::Matched` with the file's index as the
/// slot, its **settled** key, its columns, rows and frame count, its size
/// in bytes, and the step that found it. The adapter refuses a file whose
/// size differs. So a record lands on the file its reference names, under
/// that file's settled key, or on no file; byte-identical copies share a
/// key, and a record on that key shows on each of them, as one an
/// operation made does.
///
/// # Applying
///
/// The fragment is applied as **one transaction**
/// (`MemoryBackend::transact`): one `Batch` of a `create_layer` for each
/// layer, a `create_annotation` for each annotation and a `set_label`
/// (`base_rev: null`, `before: null`, `after` the value) for each label,
/// in the fragment's order, under a new `op_id`, with
/// `Checking::AsLoaded` (the adapter has checked every record; a batch of
/// an import is not bound by the envelope's 10,000 operations), the
/// author [`NATIVE_IMPORT_AUTHOR`], no EMBED slot, `import: false` and no
/// view check. An empty fragment is no transaction.
///
/// The store stamps the records and gives each `rev` 1. A label keeps its
/// id, target, field, layer and value; the report counts the labels that
/// carried anything else (`extensions`, unknown members, `derived_from`,
/// `score`) as `label_members_dropped`.
///
/// The transaction is what makes the import all or nothing: the store
/// refuses it whole when a record's id or a layer's id is already taken, a
/// layer is missing, or two labels are one. Nothing is read from the store
/// beforehand, so there is no moment between a check and the write.
///
/// # How it ends
///
/// - Applied: the adapter's report is kept
///   (`AnnotationStore::record_import_report`), then the import is ended
///   (`AnnotationStore::finish_loading`), and the report is returned.
/// - Anything else: the import is ended as failed
///   (`AnnotationStore::fail_loading`) with the message of the
///   [`AnnotationError::ImportFailed`] returned, and no record of the
///   document is in the store. The message is the adapter's error text
///   (`ImportError`, which holds no text of the file); or
///   `the annotations file could not be opened` with the
///   `std::io::ErrorKind`; or, for a refused transaction,
///   `the annotation store refused the import: ` and the first
///   violation's code (`{code:?}`), or `conflict`.
///
/// While the import runs the EMBED reads wait, as for a CSV; writes do
/// not, and a record a client makes meanwhile is kept. Records are added
/// to the files they name whether or not the EMBED endpoint wrote to a
/// file meanwhile.
///
/// [`NATIVE_IMPORT_AUTHOR`]: crate::annotations::NATIVE_IMPORT_AUTHOR
pub async fn import_native(
    registry: &FileRegistry,
    store: &AnnotationStore,
    source: &NativeSource,
    keep_going: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Report, AnnotationError> {
    let result = read_native_into_store(registry, store, source, keep_going).await;
    match result {
        Ok(report) => {
            let finish = store
                .record_import_report(report.clone())
                .and_then(|()| store.finish_loading());
            if finish.is_err() {
                let error = native_store_failure();
                store
                    .fail_loading(error.to_string())
                    .map_err(|_| native_store_failure())?;
                return Err(error);
            }
            Ok(report)
        }
        Err(error) => {
            store
                .fail_loading(error.to_string())
                .map_err(|_| native_store_failure())?;
            Err(error)
        }
    }
}

fn native_store_failure() -> AnnotationError {
    AnnotationError::ImportFailed("the annotation store refused the import: conflict".to_string())
}

async fn read_native_into_store(
    registry: &FileRegistry,
    store: &AnnotationStore,
    source: &NativeSource,
    keep_going: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Report, AnnotationError> {
    let handle = tokio::runtime::Handle::current();
    let registry = registry.clone();
    let backend = store.backend().clone();
    let source = source.clone();
    let author = Author::parse(NATIVE_IMPORT_AUTHOR).map_err(|_| native_store_failure())?;
    let import_author = author.clone();
    let imported = tokio::task::spawn_blocking(move || {
        let files = registry.files_snapshot();
        let base = match common_directory(&files) {
            Some(base) => base,
            None => std::env::current_dir().map_err(|error| {
                AnnotationError::ImportFailed(
                    ImportError::Io {
                        kind: error.kind().to_string(),
                    }
                    .to_string(),
                )
            })?,
        };
        let lookup = FileLookup::new(&files, base).map_err(|_| native_store_failure())?;
        let resolver = NativeResolver {
            registry: &registry,
            files: &files,
            lookup: &lookup,
            handle: &handle,
        };
        let mut file = std::fs::File::open(source.path()).map_err(|error| {
            AnnotationError::ImportFailed(format!(
                "the annotations file could not be opened: {}",
                error.kind()
            ))
        })?;
        let stamp = now();
        let context = ImportContext {
            resolver: &resolver,
            schema: backend.config().schema.as_ref(),
            layer: backend.default_layer(),
            author: &import_author,
            now: &stamp,
            keep_going: &*keep_going,
            limits: dcmview_adapters::Limits::DEFAULT,
        };
        NativeJson
            .import(&mut file, &context)
            .map_err(|error| AnnotationError::ImportFailed(error.to_string()))
    })
    .await
    .map_err(|_| native_store_failure())??;
    let dcmview_adapters::Imported {
        fragment,
        mut report,
    } = imported;
    let dropped = fragment
        .labels
        .iter()
        .filter(|label| {
            !label.extensions.is_empty()
                || !label.unknown.is_empty()
                || label.meta.derived_from.is_some()
                || label.meta.score.is_some()
        })
        .count();
    report.lose(LossCode::LabelMembersDropped, None, dropped as u64, None);
    let mut ops = Vec::with_capacity(
        fragment.layers.len() + fragment.annotations.len() + fragment.labels.len(),
    );
    ops.extend(
        fragment
            .layers
            .into_iter()
            .map(|layer| Op::CreateLayer { layer }),
    );
    ops.extend(
        fragment
            .annotations
            .into_iter()
            .map(|annotation| Op::CreateAnnotation {
                annotation: Box::new(annotation),
            }),
    );
    ops.extend(fragment.labels.into_iter().map(|label| Op::SetLabel {
        id: label.id,
        base_rev: None,
        target: label.target,
        field: label.field,
        layer: label.layer,
        before: None,
        after: Some(label.value),
    }));
    if !ops.is_empty() {
        let envelope = OpEnvelope {
            op_id: new_id(),
            actor: author.clone(),
            ts: now(),
            op: Op::Batch { ops },
        };
        let transacted = store
            .backend()
            .transact(
                &envelope,
                &BTreeMap::<FileKey, ImageSize>::new(),
                Write {
                    checking: Checking::AsLoaded,
                    author: &author,
                    embed_slot: None,
                    import: false,
                    view: None,
                    bindings: None,
                },
            )
            .map_err(|_| native_store_failure())?;
        match transacted.result {
            ApplyResult::Ok { .. } => {}
            ApplyResult::Invalid { violations } => {
                let code = violations
                    .first()
                    .map_or(ViolationCode::TooManyItems, |violation| violation.code);
                return Err(AnnotationError::ImportFailed(format!(
                    "the annotation store refused the import: {code:?}"
                )));
            }
            ApplyResult::Conflict { .. } => return Err(native_store_failure()),
        }
    }
    Ok(report)
}

struct NativeResolver<'a> {
    registry: &'a FileRegistry,
    files: &'a [Arc<FileEntry>],
    lookup: &'a FileLookup,
    handle: &'a tokio::runtime::Handle,
}

impl FileResolver for NativeResolver<'_> {
    fn resolve(&self, reference: &FileRef) -> Resolution {
        let by_key = |key: &FileKey| {
            self.registry
                .file_for_shown_key(key.as_str())
                .filter(|index| key.scheme() != KeyScheme::Sop || !self.registry.shares_uid(*index))
        };
        let candidate = by_key(&reference.key)
            .map(|index| (index, Matcher::Key))
            .or_else(|| {
                reference
                    .sop_instance_uid
                    .as_deref()
                    .and_then(|uid| FileKey::sop(uid).ok())
                    .and_then(|key| by_key(&key))
                    .map(|index| (index, Matcher::SopInstanceUid))
            })
            .or_else(|| {
                self.lookup
                    .indexes(&reference.path)
                    .into_iter()
                    .min()
                    .map(|index| (index, Matcher::Path))
            });
        let Some((index, matcher)) = candidate else {
            return Resolution::Unmatched;
        };
        let Some(file) = self.files.get(index) else {
            return Resolution::Unmatched;
        };
        if reference.sop_instance_uid.as_ref().is_some_and(|uid| {
            !file.sop_instance_uid.is_empty()
                && uid != &file.sop_instance_uid
                && self
                    .registry
                    .masker()
                    .is_none_or(|masker| uid != &masker.uid(&file.sop_instance_uid))
        }) {
            return Resolution::Refused(Refusal::EvidenceConflict);
        }
        let key = match self.handle.block_on(self.registry.ensure_key(index)) {
            Ok(key) => key,
            Err(_) => return Resolution::Refused(Refusal::KeyUnavailable),
        };
        if let (Some(recorded), Some(digest)) = (
            reference
                .digest
                .as_ref()
                .filter(|digest| digest.starts_with("b3:")),
            self.registry
                .key_status(index)
                .and_then(|status| status.digest),
        ) {
            if recorded != FileKey::blake3(&digest).as_str() {
                return Resolution::Refused(Refusal::EvidenceConflict);
            }
        }
        Resolution::Matched(
            Resolved {
                slot: index,
                key: Some(key),
                size: image_size(file),
                size_bytes: file.size_bytes,
            },
            matcher,
        )
    }
}

/// `GET /api/annotations/import`: where the session's `--annotations`
/// import stands, without waiting for it.
pub fn import_status(store: &AnnotationStore) -> Result<AnnotationImportResponse, AnnotationError> {
    let (state, report, error) = match store.import_status().map_err(store_error)? {
        ImportStatus::None => (AnnotationImportState::None, None, None),
        ImportStatus::Loading => (AnnotationImportState::Loading, None, None),
        ImportStatus::Ready(report) => (AnnotationImportState::Ready, report, None),
        ImportStatus::Failed(message) => (AnnotationImportState::Failed, None, Some(message)),
    };
    Ok(AnnotationImportResponse {
        state,
        report,
        error,
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
                bindings: None,
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

fn embed_row<'a>(path: std::borrow::Cow<'a, str>, rois: &'a EmbedRoiAnnotations) -> EmbedRow<'a> {
    EmbedRow {
        path,
        roi_coords: &rois.roi_coords,
        roi_frames: &rois.roi_frames,
    }
}

fn embed_csv<'a>(rows: impl IntoIterator<Item = EmbedRow<'a>>) -> Result<String, AnnotationError> {
    let mut csv = Vec::new();
    dcmview_adapters::embed::write_csv(rows, &mut csv).map_err(store_error)?;
    String::from_utf8(csv).map_err(store_error)
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
                bindings: None,
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
    /// One document is built at a time. An export that has to wait has
    /// read and changed nothing yet, and a finished one holds nothing up.
    #[tokio::test]
    async fn a_second_document_export_waits_for_the_first() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let registry = FileRegistry::from_files(vec![crate::loader::test_entry(&fixture)]);
        let store = AnnotationStore::loading();
        import_embed_rows(&registry, &store, HashMap::from([(0, rois([0, 0, 1, 1]))]))
            .expect("import");

        // The permit is taken: an export is being sent.
        let sending = store.export_permit().await.expect("permit");
        let mut export = Box::pin(export_native(&registry, &store, NativeForm::Json));
        assert!(
            futures::poll!(export.as_mut()).is_pending(),
            "an export waits while another holds the permit"
        );
        assert!(
            store.staged_rows(0).expect("staged rows").is_some(),
            "an export that waits has made no staged row a record"
        );
        drop(sending);
        let export = export.await.expect("the export");
        assert_eq!(export.report.counts.annotations_out, 1);
        assert!(store.staged_rows(0).expect("staged rows").is_none());

        // The result holds no permit: sending it holds up no other export.
        let next = export_native(&registry, &store, NativeForm::JsonLines).await;
        assert!(next.is_ok());
        drop(export);
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
