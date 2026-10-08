//! File keys: which stable key each loaded file has, and the whole-file
//! digests behind `b3:` keys (`docs/design/annotation-model.md` 1.3, 1.7).
//!
//! - `table` decides keys. [`KeyTable`] is a plain data structure: it is
//!   told about files, served frames and digest outcomes, and answers with
//!   what changed. It never reads a file, takes a lock or spawns anything,
//!   so its rules are tested without a filesystem.
//! - `hash` computes one file's digest in bounded slices ([`FileHasher`]).
//!
//! The catalog (`server/catalog.rs`) owns a `KeyTable` beside its file list,
//! runs the hashing in the background and turns key changes into catalog
//! revisions. The key syntax and [`KEY_RULES`] come from the
//! `dcmview-annotation` crate, which a hub uses to compute the same keys.

mod hash;
mod table;

pub use dcmview_annotation::{FileKey, KeyScheme, KEY_RULES};
pub use hash::{FileHasher, HashProgress, KEY_HASH_SLICE_BYTES};
pub use table::{FileIdentity, FileKeyStatus, KeyChanges, KeyFailure, KeyTable, KeyView, Rekey};
