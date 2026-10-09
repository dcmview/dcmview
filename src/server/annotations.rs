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

use super::FileRegistry;
use crate::annotations::{AnnotationIndexMap, AnnotationStore, EmbedRoiAnnotations};
use crate::api::contracts::FileKeyError;
use dcmview_annotation::{ApplyResult, OpEnvelope};

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
    let _ = (registry, store, index);
    todo!("the EMBED view of one file")
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
    let _ = (registry, store, index, rois);
    todo!("replace a file's EMBED ROIs through the store")
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
    let _ = (registry, store);
    todo!("the EMBED CSV of every file's view")
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
    let _ = (registry, store, rows);
    todo!("store the rows of an EMBED CSV")
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
    let _ = (registry, store, envelope);
    todo!("apply one operation envelope")
}
