//! What an annotation store does, whatever holds the records
//! (`docs/design/annotation-model.md` 7.4, `docs/design/seams.md` 8).

use dcmview_annotation::{
    Annotation, ApplyResult, Document, FileKey, FileSizes, Label, Layer, OpEnvelope,
};

/// Why a backend could not answer at all. A refused operation is not an
/// error: it is an [`ApplyResult`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The store could not be reached or is no longer usable. Nothing was
    /// applied by the call that returned this.
    #[error("the annotation store is unavailable: {0}")]
    Unavailable(String),
}

/// The current records of some files, read at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// The store's revision when the records were read
    /// ([`AnnotationBackend::revision`]).
    pub revision: u64,
    /// Every layer that is not deleted, in the order the layers were
    /// created.
    pub layers: Vec<Layer>,
    /// The annotations of the files asked for that are not deleted, in the
    /// order they were created. A restored annotation is where it was
    /// before it was deleted.
    pub annotations: Vec<Annotation>,
    /// The labels that have a value and whose target is one of the files
    /// asked for or a frame of one, in the order they were created.
    pub labels: Vec<Label>,
}

/// An annotation store: current state, changed only by operations.
///
/// # Keys
///
/// A backend addresses a file by its [`FileKey`] and by nothing else. It is
/// given, and it returns, the keys records are stored under: the caller has
/// already settled each one (`FileRegistry::ensure_key`) and undone whatever
/// form the session shows keys in. A backend never sees a file index, a
/// path or the registry.
///
/// # One envelope, one transaction
///
/// [`AnnotationBackend::apply`] applies each envelope as one transaction
/// that covers its validation, the change to the records, and the result
/// kept for its `op_id`:
///
/// - **All or nothing.** An envelope changes everything it names or
///   nothing. A `Batch` is one envelope: its operations are applied in
///   their order, each seeing what the ones before it did, and the first
///   one that is refused refuses the whole batch with that operation's
///   result and leaves the store as it was before the envelope.
/// - **One result.** `Ok` lists the new revision of every record and layer
///   the envelope changed, each id once, in the order they were first
///   changed. `Conflict` holds what the refused operation should have been
///   based on. `Invalid` holds the violations, each `path` a JSON Pointer
///   from the envelope (`/op/annotation/geometry/x1`,
///   `/op/ops/3/base_rev`).
/// - **Isolated.** No reader sees a transaction half applied, and two
///   transactions never interleave.
/// - **Idempotent by `op_id`.** An envelope whose `op_id` was applied
///   before, and whose result the store still remembers, returns the
///   result of that first application and changes nothing, whatever else
///   the envelope now holds. A refused envelope is not remembered: it
///   changed nothing, so sending it again judges it again. A store may
///   forget old results within stated bounds; a forgotten envelope is
///   judged again too, which for one that was applied ends in a refusal
///   and never in a second application.
///
/// # Order
///
/// The envelopes of one call are applied in the order given, each as its
/// own transaction; a refusal of one does not stop the ones after it, which
/// are judged against the state it left untouched. Concurrent calls are
/// applied in some order, one whole envelope at a time.
///
/// # Revisions
///
/// - A record's or layer's `rev` is 1 when it is created, whatever the
///   payload said, and rises by one with every operation that changes it,
///   delete and restore included. An operation's `base_rev` must equal it.
/// - The store has a revision of its own: the number of envelopes it has
///   applied. It starts at 0, rises by one for every envelope that is
///   applied, and is not changed by a refused envelope or by a repeated
///   `op_id`. Two reads that return the same revision saw the same records.
///
/// # Stamps
///
/// The store that owns the records writes `created_by`, `created_at`,
/// `modified_by` and `modified_at`; what a payload says for them, and the
/// envelope's `actor` and `ts`, are advisory.
pub trait AnnotationBackend: Send + Sync {
    /// The current records of `files`. A key no record names contributes
    /// nothing; an empty list asks for the layers alone.
    fn snapshot(&self, files: &[FileKey]) -> Result<Snapshot, BackendError>;

    /// Applies `envelopes` in order and returns one result for each, in the
    /// same order.
    ///
    /// `files` answers, for a file key, the size of that file. A key it
    /// does not know is a file that does not exist as far as this call is
    /// concerned (`unknown_file`).
    fn apply(
        &self,
        envelopes: Vec<OpEnvelope>,
        files: &dyn FileSizes,
    ) -> Result<Vec<ApplyResult>, BackendError>;

    /// Everything the store holds that is not deleted, as a document of the
    /// current format and version: every layer, annotation and label with a
    /// value, each list in creation order, and the label schema in force
    /// (`null` for a session without one).
    ///
    /// `files` is left empty. A backend holds keys, not the evidence a
    /// `FileRef` carries; whoever owns the file registry adds one `FileRef`
    /// for each key the records name before the document leaves the
    /// process. A record that was loaded leniently is in the document as it
    /// is held, so the document may fail `Document::validate`.
    fn export(&self) -> Result<Document, BackendError>;

    /// The store's revision (see "Revisions").
    fn revision(&self) -> Result<u64, BackendError>;
}
