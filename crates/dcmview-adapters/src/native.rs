//! The native formats: a `dcmview.annotations` document as JSON, and the
//! same records one per line (`docs/design/annotation-model.md` 6.4,
//! `docs/design/output-adapters.md` 7.1).
//!
//! # The JSON document
//!
//! One `dcmview_annotation::Document`, written compact by
//! `serde_json::to_writer` with nothing after it. At most
//! [`crate::limits::MAX_DOCUMENT_BYTES`].
//!
//! # JSON Lines
//!
//! Every line is one JSON value, compact, ended by one line feed.
//!
//! - **Line 1, the header:** the document with its `files`, `layers`,
//!   `annotations` and `labels` empty. It carries `format`, `version`,
//!   `schema`, `extensions` and the members this version does not know.
//! - **Every later line:** an object with exactly one member, whose name
//!   says what the value is: `{"file": <FileRef>}`, `{"layer": <Layer>}`,
//!   `{"annotation": <Annotation>}` or `{"label": <Label>}`.
//!
//! A writer puts the files first, then the layers, the annotations and the
//! labels, each in the document's order. A reader takes the lines in any
//! order. The two forms hold the same document: reading either gives the
//! same result, the same report and the same row numbers
//! ([`crate::RowRef`]).
//!
//! # Reading either form
//!
//! [`NativeJson`] reads both, so a file need not say which it is. It reads
//! the first line (to the first line feed or the end, at most
//! [`crate::limits::MAX_DOCUMENT_BYTES`]) and hands it to
//! `Document::from_json_str`:
//!
//! - when that reads it and nothing but whitespace follows, the line is
//!   the document;
//! - when that reads it and more follows, the line is a header and the
//!   rest is record lines, each at most [`crate::limits::MAX_LINE_BYTES`];
//!   a line of whitespace only is skipped and still counted; the header's
//!   own four lists must be empty ([`ImportError::Malformed`] at row 1),
//!   and a line that is not one of the four record objects is
//!   [`ImportError::Malformed`] at its row;
//! - when that answers `Malformed` and more follows, the whole input, at
//!   most [`crate::limits::MAX_DOCUMENT_BYTES`], is read as one document
//!   (one written over several lines), [`ImportError::Malformed`] at row 1
//!   when it is not one.
//!
//! `Document::from_json_str` checks `format` and `version`
//! ([`ImportError::WrongFormat`], [`ImportError::UnsupportedVersion`]) and
//! refuses JSON nested deeper than serde_json's 128 levels
//! ([`ImportError::Malformed`]). Input that is not UTF-8 is
//! [`ImportError::Malformed`] at the row it is on.
//!
//! # What an import checks, and what it hands on
//!
//! The native formats are checked strictly
//! (`docs/design/output-adapters.md` 8), in this order, the first failure
//! ending the import with [`ImportError::Invalid`] at that record's row and
//! with the first violation's code:
//!
//! 1. **Counts.** At most [`crate::limits::MAX_FILES`] files and
//!    [`crate::limits::MAX_RECORDS`] annotations and labels
//!    ([`ImportError::TooLarge`]); in the JSON Lines form this is checked
//!    as the lines are read, before the line past the bound is kept.
//! 2. **The schema** the input carries is not taken. When it is there and
//!    is not equal to `context.schema`, the report warns
//!    `schema_not_applied` at row 1. Records are checked against
//!    `context.schema`, or `LabelSchema::implicit` without one.
//! 3. **Files**, in order. Each must pass `FileRef::validate`, and no two
//!    may share a key (`duplicate_file_key`, at the second). Each is then
//!    resolved (`context.resolver.resolve`):
//!    - no match: `file_unmatched` at its row;
//!    - a refusal: `evidence_conflict` or `key_unavailable` at its row;
//!    - a match whose rows, columns or frame count are not the
//!      reference's: refused, `size_mismatch`;
//!    - a match without a key ([`crate::Resolved::key`]): refused,
//!      `key_unavailable`;
//!    - otherwise matched, counted in `matched_by` under the matcher and
//!      once in `counts.files`; `file_size_differs` when the size in bytes
//!      is not the reference's.
//! 4. **Layers**, in order. Each must pass `Layer::validate`; no two may
//!    share an id (`duplicate_id`); at most 4,096
//!    (`dcmview_annotation::limits::MAX_SCHEMA_ITEMS`, `too_many_items`).
//!    A layer whose id is `context.layer` is not handed on: its records
//!    join the caller's layer of that id. Every other one is, as read.
//! 5. **Annotations**, in order, `keep_going` asked before each
//!    ([`ImportError::Cancelled`]). Its `file` must be the key of a file
//!    of the input (`unknown_file`). When that file was not matched the
//!    annotation is counted in `counts.unmatched`, when it was refused in
//!    `counts.refused`, and nothing more of it is looked at. Otherwise:
//!    its `file` becomes the key the resolver gave; its `layer` must be a
//!    layer of the input or `context.layer` (`unknown_layer`); no earlier
//!    annotation of the input may have its id (`duplicate_id`); its
//!    geometry is quantized (`Geometry::quantized`); and it must pass
//!    `Annotation::validate` against the matched file's size and the
//!    schema, with the one exception below.
//! 6. **Labels**, in order, `keep_going` asked before each. A label on a
//!    file or a frame is placed like an annotation (`unknown_file`,
//!    unmatched, refused, the key replaced); a label on a patient, study,
//!    series or folder is taken as written. Its `layer` must be known
//!    (`unknown_layer`), its id new among the labels (`duplicate_id`), and
//!    it must pass `Label::validate`.
//!
//! `counts.annotations_in` and `labels_in` count what the input held;
//! `annotations_out` and `labels_out` what the fragment holds.
//!
//! ## A box an EMBED import kept as read (OWNER)
//!
//! A record the lenient EMBED import read is held unchanged, so a native
//! export can hold a rectangle the strict check refuses
//! (`docs/design/annotation-model.md` 2.1 leaves the answer to this
//! format). The native import takes such a rectangle as it takes it from
//! EMBED: **when the geometry is a rectangle and every violation
//! `Annotation::validate` reports has the code `out_of_bounds`,
//! `degenerate` or `frames_empty`, the annotation is kept as read and
//! reported**, once for each code that applies, at its row:
//! `out_of_range` when a coordinate is below 0, `x0` or `x1` above the
//! columns, or `y0` or `y1` above the rows; `zero_area` when `x0 == x1` or
//! `y0 == y1`; `corners_out_of_order` when `x0 > x1` or `y0 > y1`;
//! `frames_empty` when its frame set is empty. Any other violation, and
//! any violation of another geometry, fails the import. So a document this
//! crate wrote can be read by it, and nothing is changed or dropped
//! without a line in the report.
//!
//! # Cost
//!
//! An import holds the input's text once (the document, or one line at a
//! time) and the records it read once; it looks at each record a fixed
//! number of times and finds a file, a layer and an id through a map, not
//! by scanning a list for each record. The resolver is asked once for each
//! file of the input.

use crate::adapter::{
    AdapterInfo, Capabilities, Confidence, Directions, ExportAdapter, ExportContext, ImportAdapter,
    ImportContext, Imported,
};
use crate::error::{ExportError, ImportError};
use crate::report::Report;
use dcmview_annotation::{Document, GeometryType};
use std::io::{Read, Write};

/// The id of the native JSON adapter.
pub const NATIVE_ADAPTER_ID: &str = "native";

/// The id of the native JSON Lines adapter.
pub const NATIVE_JSONL_ADAPTER_ID: &str = "native-jsonl";

fn capabilities() -> Capabilities {
    Capabilities {
        geometry: GeometryType::ALL.to_vec(),
        classes: true,
        attributes: true,
        labels: true,
        multi_frame: true,
        scores: true,
        lossless: true,
    }
}

/// The native JSON document: writes one, and reads both native forms.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeJson;

impl NativeJson {
    fn describe() -> AdapterInfo {
        AdapterInfo {
            id: NATIVE_ADAPTER_ID.to_string(),
            name: "dcmview annotations (JSON)".to_string(),
            version: 1,
            directions: Directions::Both,
            extension: "json".to_string(),
            media_type: "application/json".to_string(),
            capabilities: capabilities(),
        }
    }
}

impl ExportAdapter for NativeJson {
    fn info(&self) -> AdapterInfo {
        Self::describe()
    }

    /// Writes `document` as one JSON document ("The JSON document"),
    /// exactly as it is: nothing is checked, clamped or left out, and a
    /// record that was loaded leniently is written as it is held.
    ///
    /// Output past `context.max_bytes` or past
    /// [`crate::limits::MAX_DOCUMENT_BYTES`], whichever is smaller, is
    /// [`ExportError::TooLarge`] with that limit. The bytes are counted as
    /// they are written, and nothing is buffered beyond what `out` buffers.
    ///
    /// The report has no loss and no warning: `counts.files` is the number
    /// of the document's files, `annotations_in` and `annotations_out` its
    /// annotations, `labels_in` and `labels_out` its labels.
    fn export(
        &self,
        document: &Document,
        context: &ExportContext<'_>,
        out: &mut dyn Write,
    ) -> Result<Report, ExportError> {
        let _ = (document, context.max_bytes, out);
        todo!("ADP1")
    }
}

impl ImportAdapter for NativeJson {
    fn info(&self) -> AdapterInfo {
        Self::describe()
    }

    /// [`Confidence::None`] unless the first byte of `head` that is not
    /// ASCII whitespace is `{`; then [`Confidence::Certain`] when `head`
    /// holds the bytes `"dcmview.annotations"` (with the quotes), else
    /// [`Confidence::Possible`].
    fn sniff(&self, head: &[u8]) -> Confidence {
        let _ = head;
        todo!("ADP1")
    }

    /// Reads a native document in either form, by the module
    /// documentation. The report's adapter is [`NATIVE_ADAPTER_ID`]
    /// version 1.
    fn import(
        &self,
        input: &mut dyn Read,
        context: &ImportContext<'_>,
    ) -> Result<Imported, ImportError> {
        let _ = (input, context.layer);
        todo!("ADP1")
    }
}

/// The native JSON Lines form: writes it. [`NativeJson`] reads it.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeJsonLines;

impl ExportAdapter for NativeJsonLines {
    fn info(&self) -> AdapterInfo {
        AdapterInfo {
            id: NATIVE_JSONL_ADAPTER_ID.to_string(),
            name: "dcmview annotations (JSON Lines)".to_string(),
            version: 1,
            directions: Directions::Export,
            extension: "jsonl".to_string(),
            media_type: "application/jsonl".to_string(),
            capabilities: capabilities(),
        }
    }

    /// Writes `document` in the JSON Lines form ("JSON Lines"): the
    /// header, then one line for each file, layer, annotation and label,
    /// in that order and in the document's order, each record exactly as
    /// it is. The header is made without copying the document's four
    /// lists.
    ///
    /// Output past `context.max_bytes` is [`ExportError::TooLarge`]; the
    /// form has no bound of its own. The report is [`NativeJson`]'s, under
    /// [`NATIVE_JSONL_ADAPTER_ID`].
    fn export(
        &self,
        document: &Document,
        context: &ExportContext<'_>,
        out: &mut dyn Write,
    ) -> Result<Report, ExportError> {
        let _ = (document, context.max_bytes, out);
        todo!("ADP1")
    }
}
