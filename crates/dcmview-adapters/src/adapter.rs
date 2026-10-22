//! The two things an adapter is, and what each is handed
//! (`docs/design/annotation-model.md` 8.1,
//! `docs/design/output-adapters.md` 13).
//!
//! An adapter is code: it reads a `Read` or writes a `Write` and touches
//! nothing else. What a format needs beyond the model (which loaded file a
//! path names, under which path a file is exported) comes through the
//! context, which the viewer and a hub each fill from their own file
//! registry. The same adapter therefore gives the same bytes in both.

use crate::error::{ExportError, ImportError};
use crate::limits::Limits;
use crate::report::Report;
use dcmview_annotation::{
    Annotation, Author, Document, FileKey, FileRef, GeometryType, ImageSize, Label, LabelSchema,
    Layer, LayerId, Timestamp,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use ts_rs::TS;

/// Which way an adapter works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Directions {
    Import,
    Export,
    Both,
}

/// What a format can hold, for a check before an export
/// (`docs/design/output-adapters.md` 7.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Capabilities {
    /// The geometry types the format holds as they are.
    #[ts(type = "Array<string>")]
    pub geometry: Vec<GeometryType>,
    pub classes: bool,
    pub attributes: bool,
    pub labels: bool,
    /// Whether an annotation may name some frames of a file.
    pub multi_frame: bool,
    pub scores: bool,
    /// Whether a document written and read back is the same document.
    pub lossless: bool,
}

/// What an adapter says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AdapterInfo {
    /// Stable, lower-case, unique in a registry: `embed`, `native`,
    /// `native-jsonl`. An import is authored `import:<id>`.
    pub id: String,
    /// For a menu.
    pub name: String,
    /// Rises when the bytes the adapter writes for the same input change.
    pub version: u32,
    pub directions: Directions,
    /// The file name extension of its output, without the dot.
    pub extension: String,
    pub media_type: String,
    pub capabilities: Capabilities,
}

/// How sure an importer is that an input is in its format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    /// Not this format.
    None,
    /// Could be: nothing rules it out.
    Possible,
    /// The format's own marker is there.
    Certain,
}

/// A loaded file an input named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The resolver's own handle for the file (the viewer: its catalog
    /// index). Two answers with one slot are one file.
    pub slot: usize,
    /// The key records for this file are to hold, or `None` when the file
    /// has none the caller may rely on. The viewer's EMBED import stages
    /// rows by slot and needs no key; every importer that makes records
    /// refuses a file without one.
    pub key: Option<FileKey>,
    pub size: ImageSize,
    /// The file's size in bytes.
    pub size_bytes: u64,
}

/// Which evidence found a file (`docs/design/annotation-model.md` 1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Matcher {
    Key,
    SopInstanceUid,
    Path,
}

/// Why a resolver will not attach records to a file it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The file's SOP Instance UID or digest contradicts the reference's.
    EvidenceConflict,
    /// The file has no key and cannot be given one.
    KeyUnavailable,
    /// The reference names its file by content, and a file found for it
    /// could not be read to compare. Nothing is placed on a file that was
    /// not compared.
    Unverified,
    /// The reference's path or UID names more loaded files than a
    /// resolver compares, and nothing else of the reference finds a file.
    Ambiguous,
}

/// What a resolver answers for one file reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Matched(Resolved, Matcher),
    /// No loaded file is this one.
    Unmatched,
    Refused(Refusal),
}

/// Which loaded files an input names. The caller's file registry, as an
/// importer sees it.
///
/// A resolver decides *which* file; the importer checks the rest: that
/// rows, columns and frame count agree, and whether a key is needed. A
/// resolver may take long (the viewer's settles a file's key, which can
/// mean reading the file), and is called once per file reference or path,
/// never once per record.
pub trait FileResolver {
    /// The loaded file `file` describes.
    ///
    /// The order of evidence is the model's (1.3): the key, then the SOP
    /// Instance UID, then the path. A candidate that contradicts a strong
    /// identifier both sides have is a refusal, not a match. The default
    /// knows no file.
    fn resolve(&self, file: &FileRef) -> Resolution {
        let _ = file;
        Resolution::Unmatched
    }

    /// Appends to `out` every loaded file `path` names, as a format that
    /// knows files by path alone (EMBED) wrote it. One path can name
    /// several files (a link and its target); none means the path is
    /// unmatched. The default knows no file.
    fn by_path(&self, path: &str, out: &mut Vec<Resolved>) {
        let _ = (path, out);
    }
}

/// What an importer is handed besides its input.
#[derive(Clone, Copy)]
pub struct ImportContext<'a> {
    pub resolver: &'a dyn FileResolver,
    /// The label schema in force where the records will live, or `None`
    /// for the implicit one (`LabelSchema::implicit`). Records are checked
    /// against it, not against a schema the input carries, unless
    /// `take_schema` lets the import take that one.
    pub schema: Option<&'a LabelSchema>,
    /// Whether a format that carries a schema (native) may put it in force
    /// where `schema` is `None`: records are then checked against the
    /// input's schema, which is handed on in [`Fragment::schema`]. A
    /// session that has no schema of its own sets it.
    pub take_schema: bool,
    /// The caller's default layer. A format without layers (EMBED) puts
    /// its records in it; a format with layers lets records that name this
    /// id join it and does not hand the layer itself on.
    pub layer: &'a LayerId,
    /// The stamps of records the importer makes itself (EMBED). A store
    /// writes its own when the records are applied.
    pub author: &'a Author,
    pub now: &'a Timestamp,
    /// Asked between records; `false` ends the import with
    /// [`ImportError::Cancelled`].
    pub keep_going: &'a dyn Fn() -> bool,
    /// The bounds the import reads within ([`Limits::DEFAULT`] outside
    /// tests).
    pub limits: Limits,
}

/// What an import read, ready to be applied as operations. Every
/// annotation, and every label on a file or a frame, names a key a
/// resolver answered ([`Resolved::key`]); nothing in it names a file the
/// resolver did not match.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fragment {
    /// The layers to create, in the input's order. Never the context's
    /// own layer.
    pub layers: Vec<Layer>,
    /// In the input's order.
    pub annotations: Vec<Annotation>,
    /// In the input's order.
    pub labels: Vec<Label>,
    /// The input's schema, when the import took it
    /// ([`ImportContext::take_schema`]); `None` otherwise.
    pub schema: Option<LabelSchema>,
}

/// An import's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub fragment: Fragment,
    pub report: Report,
}

/// Under which paths a file is exported, for a format that names files by
/// path (EMBED). The model keeps absolute paths out of a document, so they
/// come from the caller's registry.
pub trait ExportPaths {
    /// Appends every path the file with `key` is exported under: one for
    /// each loaded path, aliases included. None means the file's
    /// `FileRef::path` is used.
    fn paths_of(&self, key: &FileKey, out: &mut Vec<String>);
}

impl ExportPaths for BTreeMap<FileKey, Vec<String>> {
    fn paths_of(&self, key: &FileKey, out: &mut Vec<String>) {
        if let Some(paths) = self.get(key) {
            out.extend(paths.iter().cloned());
        }
    }
}

/// What an exporter is handed besides the document.
#[derive(Clone, Copy)]
pub struct ExportContext<'a> {
    pub paths: &'a dyn ExportPaths,
    /// The most bytes the export may write. One more is
    /// [`ExportError::TooLarge`]. A format's own bound applies as well.
    pub max_bytes: u64,
    /// The bounds of the formats ([`Limits::DEFAULT`] outside tests). An
    /// export writes nothing an import within the same bounds refuses to
    /// read for its size.
    pub limits: Limits,
}

/// Writes a document in one format.
///
/// An export never changes the document, writes nothing but `out`, and
/// gives the same bytes for the same document and context. It reports
/// everything it left out.
pub trait ExportAdapter: Send + Sync {
    fn info(&self) -> AdapterInfo;

    /// Writes `document` to `out` and reports what was written.
    fn export(
        &self,
        document: &Document,
        context: &ExportContext<'_>,
        out: &mut dyn Write,
    ) -> Result<Report, ExportError>;
}

/// Reads one format into records.
///
/// # Untrusted input
///
/// The input is a file nobody vouches for. An importer never panics on it,
/// reads it within the bounds of [`crate::limits`], allocates only for
/// what it has read, and does work in proportion to the input, each row or
/// record looked at a fixed number of times. What it hands on is all of
/// the input or an error: invalid data on a matched record fails the whole
/// import, and what matched no file is counted and left out
/// (`docs/design/output-adapters.md` 8).
pub trait ImportAdapter: Send + Sync {
    fn info(&self) -> AdapterInfo;

    /// How sure the adapter is that an input starting with `head` (at most
    /// [`crate::limits::SNIFF_BYTES`] of it) is in its format. Never
    /// panics; reads nothing but `head`.
    fn sniff(&self, head: &[u8]) -> Confidence;

    /// Reads `input` to its end.
    fn import(
        &self,
        input: &mut dyn Read,
        context: &ImportContext<'_>,
    ) -> Result<Imported, ImportError>;
}
