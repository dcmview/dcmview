//! Import and export adapters over the dcmview annotation model.
//!
//! An adapter turns a document of the model (`dcmview-annotation`) into the
//! bytes of one format, or the bytes of one format into records. This crate
//! is shared by the viewer and by other repositories that pin it by dcmview
//! release tag, so the same input gives the same bytes wherever it runs. It
//! has no axum, tokio, DICOM or pixel-pipeline dependency and touches
//! neither the filesystem nor the network: an adapter reads the `Read` and
//! writes the `Write` it is handed, and learns about files from the context
//! its caller fills.
//!
//! The design is `docs/design/output-adapters.md` and
//! `docs/design/annotation-model.md` 8 and 9.
//!
//! # What is here
//!
//! | Module | Holds |
//! |---|---|
//! | [`adapter`] | [`ExportAdapter`] and [`ImportAdapter`], [`AdapterInfo`], the contexts, [`FileResolver`] and [`Fragment`] |
//! | [`registry`] | [`Registry`]: the adapters a process offers, by id, and which one an input is for |
//! | [`report`] | [`Report`]: what an import or export did, in fixed codes, counts and row numbers |
//! | [`error`] | [`ImportError`] and [`ExportError`] |
//! | [`limits`] | Every size bound, as a fixed number |
//! | [`embed`] | EMBED ROI CSV: rows, and [`EmbedAdapter`] |
//! | [`native`] | The native formats: [`NativeJson`] and [`NativeJsonLines`] |
//!
//! # Rules
//!
//! - **Input is untrusted.** An importer never panics on its input, reads
//!   it within the bounds of [`limits`], allocates only for what it has
//!   read and never from a count the input states, and does work in
//!   proportion to the input.
//! - **An import is all or nothing.** It hands on everything it read or
//!   fails; invalid data on a record that matched a file fails the whole
//!   import. What matched no file is counted and left out.
//! - **Nothing is silent.** Every import and export answers with a
//!   [`Report`] of what it left out, changed or could not place.
//! - **No text of the input leaves in a report or an error.** Both are
//!   fixed words, numbers and row numbers, so they can be shown and logged
//!   in a session that masks identifiers.
//! - **An export changes nothing** and gives the same bytes for the same
//!   document and context.
//! - **Redaction boxes are not annotations** and no adapter reads or
//!   writes one.

pub mod adapter;
pub mod embed;
pub mod error;
pub mod held;
pub mod limits;
pub mod native;
pub mod registry;
pub mod report;

pub use adapter::{
    AdapterInfo, Capabilities, Confidence, Directions, ExportAdapter, ExportContext, ExportPaths,
    FileResolver, Fragment, ImportAdapter, ImportContext, Imported, Matcher, Refusal, Resolution,
    Resolved,
};
pub use embed::{EmbedAdapter, EmbedFileRois, EmbedRead, EmbedRois, EmbedRow};
pub use error::{Bound, ExportError, ImportError};
pub use limits::Limits;
pub use native::{NativeJson, NativeJsonLines};
pub use registry::Registry;
pub use report::{
    AdapterRef, Counts, Direction, Loss, LossCode, MatchedBy, Report, RowRef, Warning, WarningCode,
};
