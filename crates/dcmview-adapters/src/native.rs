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
//! - **Every line between:** an object with exactly one member, whose name
//!   says what the value is: `{"file": <FileRef>}`, `{"layer": <Layer>}`,
//!   `{"annotation": <Annotation>}` or `{"label": <Label>}`.
//! - **The last line, the end:**
//!   `{"end":{"files":2,"layers":1,"annotations":5,"labels":0}}`, the
//!   number of lines of each kind before it. It is what tells a whole file
//!   from one that was cut off at a line, so it is always written, also
//!   for a document without a record.
//!
//! A writer puts the files first, then the layers, the annotations and the
//! labels, each in the document's order, then the end. A reader takes the
//! lines between header and end in any order. The two forms hold the same
//! document: reading either gives the same result, the same report and the
//! same row numbers ([`crate::RowRef`]).
//!
//! # Reading either form
//!
//! [`NativeJson`] reads both, so a file need not say which it is. A UTF-8
//! byte order mark at the very start of the input (`EF BB BF`) is passed
//! over, here and in [`ImportAdapter::sniff`]. It reads
//! the first line (to the first line feed or the end, at most
//! [`crate::limits::MAX_DOCUMENT_BYTES`]) and hands it to
//! `Document::from_json_str`:
//!
//! - when that reads it and nothing but whitespace follows, the line is
//!   the document, **unless** its four lists are all empty: a text on one
//!   line that holds no record is a JSON Lines header whose lines, the
//!   end line with them, are missing, and it is
//!   [`ImportError::Truncated`], whether or not a line feed follows it.
//!   (No export of a session is such a text: a session always has a
//!   layer. A document without a record that is written over several
//!   lines is read as the document it is.);
//! - when that reads it and more follows, the line is a header and the
//!   rest is record lines, each at most [`crate::limits::MAX_LINE_BYTES`];
//!   a line of whitespace only is skipped and still counted; the header's
//!   own four lists must be empty ([`ImportError::Malformed`] at row 1),
//!   and a line that is neither one of the four record objects nor the
//!   end is [`ImportError::Malformed`] at its row. **The lines must end
//!   with the end line**, and its four counts must be the numbers of
//!   lines of each kind read before it: an input that stops without one,
//!   or whose counts differ, is [`ImportError::Truncated`], and nothing of
//!   it is handed on. A line that holds anything after the end line is
//!   [`ImportError::Malformed`] at its row;
//! - when that answers `Malformed` and more follows, the whole input, at
//!   most [`crate::limits::MAX_DOCUMENT_BYTES`], is read as one document
//!   (one written over several lines), [`ImportError::Malformed`] at row 1
//!   when it is not one.
//!
//! A member of the document that is there twice is
//! [`ImportError::Malformed`].
//!
//! `Document::from_json_str` checks `format` and `version`
//! ([`ImportError::WrongFormat`], [`ImportError::UnsupportedVersion`]) and
//! reads no JSON nested deeper than serde_json's 128 levels, which
//! [`crate::limits::MAX_DEPTH`] refuses before it is parsed
//! ([`ImportError::TooLarge`] with `Bound::Depth`). Input that is not UTF-8 is
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
//!    as the lines are read, before the line past the bound is kept, and
//!    so is the bound on layers of step 4.
//! 2. **The schema** the input carries is not taken. When it is there and
//!    is not equal to `context.schema`, the report warns
//!    `schema_not_applied` at row 1. Records are checked against
//!    `context.schema`, or `LabelSchema::implicit` without one.
//! 3. **Files**, in order. Each must pass `FileRef::validate`, and no two
//!    may share a key (`duplicate_file_key`, at the second). Each is then
//!    resolved (`context.resolver.resolve`):
//!    - no match: `file_unmatched` at its row;
//!    - a refusal: `evidence_conflict`, `file_unverified`,
//!      `file_ambiguous` or `key_unavailable` at its row, by the
//!      [`Refusal`];
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
//! ## A box an EMBED import kept as read
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
//! **For any input, an import holds at most the budget, the transient
//! and the text's buffer**, each a number of bytes that does not depend
//! on what the text is made of:
//!
//! - **The budget**, [`crate::Limits::held_bytes`] (4 GiB by default,
//!   [`crate::limits::MAX_HELD_BYTES`], which says where the number comes
//!   from): what the records of one document may be charged.
//! - **The transient**, [`crate::limits::UNIT_TRANSIENT_BYTES`]
//!   (256 MiB): what building one record, or the header, may hold before
//!   it is charged.
//! - **The text**: a JSON document or a first line is read into a buffer
//!   that grows by doubling, up to three times its length while it grows
//!   (768 MiB for the largest document); a JSON Lines file has one line
//!   in such a buffer, up to three times the longest line (48 MiB).
//!
//! **The budget is charged, not predicted, and is not a multiple of the
//! text.** What a value costs to hold cannot be told from its text: the
//! model keeps `extensions` and members it does not know as JSON values,
//! a member of two bytes can be a map node of several hundred, and a
//! mask of one tile a frame is charged many times its text. So after each
//! file reference, layer, annotation and label is built, and after the
//! header is, the reader adds its charge ([`crate::held`]: what the item
//! holds, counted on the item, and the room an import needs for it on
//! its way into a session) to a total, and stops when the total is past
//! the budget: [`ImportError::TooLarge`] with `Bound::HeldBytes`, and
//! nothing is handed on. The charge covers the whole way of a record,
//! from this reader's lists to a store that takes the records without
//! keeping a second copy of them all, so a caller that hands the records
//! on one at a time holds no more than was charged.
//!
//! **A writer holds a document to everything a reader does**, with the
//! same functions, and refuses it ([`ExportError::TooMany`] naming the
//! bound) before or while it writes:
//!
//! - the numbers of files and of records ([`crate::Limits::files`],
//!   [`crate::Limits::records`]);
//! - the charge ([`crate::held::document_charge`], `Bound::HeldBytes`),
//!   which is a function of what the records say and so is the same
//!   before a document is written and after it is read;
//! - for each unit as it will be read (the header; each record, alone in
//!   the JSON form and in its line in the JSON Lines form), the values,
//!   the objects and arrays, and the depth (`Bound::UnitValues`,
//!   `Bound::UnitContainers`, `Bound::Depth`), by the pass the reader
//!   makes over the same text;
//! - the length: of the JSON document ([`ExportError::TooLarge`]), and in
//!   the JSON Lines form of the first line and of each record line
//!   (`Bound::DocumentBytes`, `Bound::LineBytes`).
//!
//! **So a document is written exactly when the same bounds would read
//! it.** The forms do not have the same bounds: a JSON document has a
//! bound on its whole length and none on one record's, a JSON Lines file
//! the other way round, and a record stands in a line of its own in one
//! and two levels down in the other, with a member name more. So a
//! session can be past a bound of one form and within the other's.
//!
//! **The transient is bounded by construction.** One unit (a record, or
//! everything of a document that is not a record) is in the parser's
//! buffered form and in its kept form at once, before it can be charged.
//! One pass over the text that allocates nothing, before any value of it
//! is parsed, holds a unit to [`crate::limits::MAX_UNIT_VALUES`] JSON
//! values (objects, arrays, strings, member names, numbers, `true`,
//! `false`, `null`), to [`crate::limits::MAX_UNIT_CONTAINERS`] objects and
//! arrays among them, and to [`crate::limits::MAX_DEPTH`] levels of
//! nesting ([`ImportError::TooLarge`] with `Bound::UnitValues`,
//! `Bound::UnitContainers` or `Bound::Depth`). The two counts times what a value and an object cost at
//! worst are the transient (derived at
//! [`crate::limits::UNIT_TRANSIENT_BYTES`]). The bounds are set from the
//! model's: the largest mask (262,144 tiles in 65,536 frames) and the
//! largest polygon (100,000 points) are within them.
//!
//! A JSON document is read member by member, and each of its four lists
//! an element at a time, so that only one record is in the buffered form
//! at once; a JSON Lines file is read a line at a time. Placing the
//! records makes every structure that has an entry a record once, at its
//! final size. `tests/raster_cost/annotations.rs` imports texts made to
//! cost the most, from the file to the store, under a budget of exactly
//! their charge and of one byte less, and holds the peak to the charge;
//! builds the costliest units within the transient; and compares what is
//! counted with what the allocator gave out.
//!
//! An import looks at each record a fixed number of times and finds a
//! file, a layer and an id through a map, not by scanning a list for each
//! record. The resolver is asked once for each file of the input.

use crate::adapter::{
    AdapterInfo, Capabilities, Confidence, Directions, ExportAdapter, ExportContext, Fragment,
    ImportAdapter, ImportContext, Imported, Matcher, Refusal, Resolution,
};
use crate::error::{Bound, ExportError, ImportError};
use crate::held::{document_charge, header_charge, Charged};
use crate::limits::Limits;
use crate::report::{AdapterRef, Direction, Report, RowRef, WarningCode};
use dcmview_annotation::{
    Annotation, Context, Document, DocumentError, FileKey, FileRef, FrameScope, Geometry,
    GeometryType, ImageSize, Invalid, Label, LabelSchema, LabelTarget, Layer, ViolationCode,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};

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
    /// A document with more files than `context.limits.files`, more
    /// annotations and labels together than `context.limits.records`, or a
    /// charge ([`crate::held::document_charge`]) past
    /// `context.limits.held_bytes`, is [`ExportError::TooMany`] with that
    /// bound and number, before anything is written: an export never
    /// writes what an import within the same bounds refuses for its size,
    /// and writes everything such an import reads. Output past `context.max_bytes` or past
    /// `context.limits.document_bytes`, whichever is smaller, is
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
        check_export_counts(document, &context.limits)?;
        check_export_units(document, &context.limits)?;
        let limit = context.max_bytes.min(context.limits.document_bytes);
        let mut writer = LimitedWriter {
            out,
            remaining: limit,
            exceeded: false,
        };
        let result = serde_json::to_writer(&mut writer, document).map_err(json_export_error);
        if writer.exceeded {
            return Err(ExportError::TooLarge { limit });
        }
        result?;
        Ok(export_report(document, NATIVE_ADAPTER_ID))
    }
}

impl ImportAdapter for NativeJson {
    fn info(&self) -> AdapterInfo {
        Self::describe()
    }

    /// [`Confidence::None`] unless the first byte of `head` that is not
    /// ASCII whitespace, after a byte order mark when `head` starts with
    /// one, is `{`; then [`Confidence::Certain`] when `head`
    /// holds the bytes `"dcmview.annotations"` (with the quotes), else
    /// [`Confidence::Possible`].
    fn sniff(&self, head: &[u8]) -> Confidence {
        let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
        if head.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            Confidence::None
        } else if head
            .windows(b"\"dcmview.annotations\"".len())
            .any(|window| window == b"\"dcmview.annotations\"")
        {
            Confidence::Certain
        } else {
            Confidence::Possible
        }
    }

    /// Reads a native document in either form, by the module
    /// documentation. The report's adapter is [`NATIVE_ADAPTER_ID`]
    /// version 1.
    fn import(
        &self,
        input: &mut dyn Read,
        context: &ImportContext<'_>,
    ) -> Result<Imported, ImportError> {
        let (document, rows) = read_document(input, &context.limits)?;
        place_document(document, rows, context)
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
    /// After the last record comes the end line, with the number of lines
    /// of each kind written.
    ///
    /// A document with more files or records than `context.limits` allows
    /// is [`ExportError::TooMany`], as for [`NativeJson`]. Output past
    /// `context.max_bytes` is [`ExportError::TooLarge`]; the form has no
    /// bound on its bytes of its own. The report is [`NativeJson`]'s, under
    /// [`NATIVE_JSONL_ADAPTER_ID`].
    fn export(
        &self,
        document: &Document,
        context: &ExportContext<'_>,
        out: &mut dyn Write,
    ) -> Result<Report, ExportError> {
        check_export_counts(document, &context.limits)?;
        let header = header_of(document);
        let limits = &context.limits;
        let mut scratch = Vec::new();
        let mut writer = LimitedWriter {
            out,
            remaining: context.max_bytes,
            exceeded: false,
        };
        let result = (|| {
            write_line(&mut writer, &mut scratch, &header, true, limits)?;
            for file in &document.files {
                write_line(
                    &mut writer,
                    &mut scratch,
                    &OutputLine::File(file),
                    false,
                    limits,
                )?;
            }
            for layer in &document.layers {
                write_line(
                    &mut writer,
                    &mut scratch,
                    &OutputLine::Layer(layer),
                    false,
                    limits,
                )?;
            }
            for annotation in &document.annotations {
                let line = OutputLine::Annotation(annotation);
                write_line(&mut writer, &mut scratch, &line, false, limits)?;
            }
            for label in &document.labels {
                write_line(
                    &mut writer,
                    &mut scratch,
                    &OutputLine::Label(label),
                    false,
                    limits,
                )?;
            }
            let end = OutputLine::End(EndCounts::of(document));
            write_line(&mut writer, &mut scratch, &end, false, limits)?;
            Ok(())
        })();
        if writer.exceeded {
            return Err(ExportError::TooLarge {
                limit: context.max_bytes,
            });
        }
        result?;
        Ok(export_report(document, NATIVE_JSONL_ADAPTER_ID))
    }
}

fn check_export_counts(document: &Document, limits: &Limits) -> Result<(), ExportError> {
    for (count, limit, bound) in [
        (document.files.len(), limits.files, Bound::Files),
        (
            document.annotations.len() + document.labels.len(),
            limits.records,
            Bound::Records,
        ),
    ] {
        if count > limit {
            return Err(ExportError::TooMany {
                bound,
                limit: limit as u64,
            });
        }
    }
    if document_charge(document) > limits.held_bytes {
        return Err(ExportError::TooMany {
            bound: Bound::HeldBytes,
            limit: limits.held_bytes,
        });
    }
    Ok(())
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct EndCounts {
    files: u64,
    layers: u64,
    annotations: u64,
    labels: u64,
}

impl EndCounts {
    fn of(document: &Document) -> Self {
        Self {
            files: document.files.len() as u64,
            layers: document.layers.len() as u64,
            annotations: document.annotations.len() as u64,
            labels: document.labels.len() as u64,
        }
    }
}

struct LimitedWriter<'a> {
    out: &'a mut dyn Write,
    remaining: u64,
    exceeded: bool,
}
impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            self.exceeded = true;
            return Err(std::io::ErrorKind::Other.into());
        }
        let written = self.out.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

fn json_export_error(error: serde_json::Error) -> ExportError {
    ExportError::Io {
        kind: error
            .io_error_kind()
            .unwrap_or(std::io::ErrorKind::Other)
            .to_string(),
    }
}

fn export_report(document: &Document, id: &str) -> Report {
    let mut report = Report::new(
        AdapterRef {
            id: id.to_string(),
            version: 1,
        },
        Direction::Export,
    );
    report.counts.files = document.files.len() as u64;
    report.counts.annotations_in = document.annotations.len() as u64;
    report.counts.annotations_out = report.counts.annotations_in;
    report.counts.labels_in = document.labels.len() as u64;
    report.counts.labels_out = report.counts.labels_in;
    report
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum OutputLine<'a> {
    File(&'a FileRef),
    Layer(&'a Layer),
    Annotation(&'a Annotation),
    Label(&'a Label),
    End(EndCounts),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputLine {
    File(FileRef),
    Layer(Layer),
    Annotation(Annotation),
    Label(Label),
    End(#[serde(deserialize_with = "read_end_counts")] EndCounts),
}

fn read_end_counts<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<EndCounts, D::Error> {
    struct EndVisitor;
    impl<'de> serde::de::Visitor<'de> for EndVisitor {
        type Value = EndCounts;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object with the four record counts")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(self, map: M) -> Result<EndCounts, M::Error> {
            EndCounts::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(EndVisitor)
}

/// Writes `value` as one line, after holding the line to what a reader
/// holds a line to: the first line as a document's header, any other as
/// one unit, and each to its length.
fn write_line(
    writer: &mut dyn Write,
    scratch: &mut Vec<u8>,
    value: &impl Serialize,
    header: bool,
    limits: &Limits,
) -> Result<(), ExportError> {
    scratch.clear();
    serde_json::to_writer(&mut *scratch, value).map_err(json_export_error)?;
    let (bound, limit) = if header {
        (Bound::DocumentBytes, limits.document_bytes)
    } else {
        (Bound::LineBytes, limits.line_bytes)
    };
    if scratch.len() as u64 > limit {
        return Err(ExportError::TooMany { bound, limit });
    }
    check_values(scratch, header, 0, limits).map_err(|error| unit_refusal(error, limits))?;
    scratch.push(b'\n');
    writer.write_all(scratch).map_err(|error| ExportError::Io {
        kind: error.kind().to_string(),
    })
}

/// What [`check_values`] refused, as a writer says it.
fn unit_refusal(error: ImportError, limits: &Limits) -> ExportError {
    match error {
        ImportError::TooLarge { bound, limit } => ExportError::TooMany { bound, limit },
        // It refuses for nothing else.
        _ => ExportError::TooMany {
            bound: Bound::UnitValues,
            limit: limits.unit_values,
        },
    }
}

/// Holds every unit of `document` to what a reader of the JSON form holds
/// it to ([`check_values`]): the header, and each element of the four
/// lists, which stands two levels down in the document.
fn check_export_units(document: &Document, limits: &Limits) -> Result<(), ExportError> {
    let mut scratch = Vec::new();
    let mut check = |value: &dyn erased::Unit, whole: bool| {
        scratch.clear();
        value.write(&mut scratch)?;
        check_values(&scratch, whole, if whole { 0 } else { 2 }, limits)
            .map_err(|error| unit_refusal(error, limits))
    };
    check(&header_of(document), true)?;
    for file in &document.files {
        check(file, false)?;
    }
    for layer in &document.layers {
        check(layer, false)?;
    }
    for annotation in &document.annotations {
        check(annotation, false)?;
    }
    for label in &document.labels {
        check(label, false)?;
    }
    Ok(())
}

/// The document without its four lists: what a JSON Lines file has as its
/// first line.
fn header_of(document: &Document) -> Document {
    Document {
        format: document.format.clone(),
        version: document.version.clone(),
        schema: document.schema.clone(),
        extensions: document.extensions.clone(),
        unknown: document.unknown.clone(),
        files: Vec::new(),
        layers: Vec::new(),
        annotations: Vec::new(),
        labels: Vec::new(),
    }
}

mod erased {
    use super::{json_export_error, ExportError};

    /// Something that can be written as JSON, whatever it is.
    pub(super) trait Unit {
        fn write(&self, out: &mut Vec<u8>) -> Result<(), ExportError>;
    }

    impl<T: serde::Serialize> Unit for T {
        fn write(&self, out: &mut Vec<u8>) -> Result<(), ExportError> {
            serde_json::to_writer(out, self).map_err(json_export_error)
        }
    }
}

#[derive(Default)]
struct RecordRows {
    files: Vec<u64>,
    layers: Vec<u64>,
    annotations: Vec<u64>,
    labels: Vec<u64>,
}

fn import_io(error: std::io::Error) -> ImportError {
    ImportError::Io {
        kind: error.kind().to_string(),
    }
}

fn read_line(
    reader: &mut impl BufRead,
    buffer: &mut Vec<u8>,
    limit: u64,
    bound: Bound,
) -> Result<bool, ImportError> {
    buffer.clear();
    let count = reader
        .take(limit + 1)
        .read_until(b'\n', buffer)
        .map_err(import_io)?;
    if count == 0 {
        return Ok(false);
    }
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
    } else if buffer.len() as u64 > limit {
        return Err(ImportError::TooLarge { bound, limit });
    }
    Ok(true)
}

/// Holds `text` to [`Limits::unit_values`] and [`Limits::depth`] before
/// anything of it is parsed, in one pass that allocates nothing.
///
/// With `whole` unset the text is one unit: a line of the JSON Lines form.
/// With it set the text is a document: each element of the top-level
/// `files`, `layers`, `annotations` and `labels` arrays is a unit of its
/// own, and everything else together is one, the header. A member name
/// written with an escape is not taken for one of the four, so its value
/// counts with the header.
///
/// The pass does not check that the text is JSON: text that is not fails
/// when it is parsed. It only has to count no fewer values than a parser
/// would make of the text, and it counts every token.
fn check_values(text: &[u8], whole: bool, outer: u32, bounds: &Limits) -> Result<(), ImportError> {
    let too_many = ImportError::TooLarge {
        bound: Bound::UnitValues,
        limit: bounds.unit_values,
    };
    let too_many_containers = ImportError::TooLarge {
        bound: Bound::UnitContainers,
        limit: bounds.unit_containers,
    };
    // `outer` is how deep the text itself is nested: an element of a
    // list of a document is two levels down.
    let mut depth = outer;
    // Values, and the objects and arrays among them, of the header and of
    // the record being read.
    let mut header = [0_u64; 2];
    let mut unit = [0_u64; 2];
    // Inside one of the four lists of a document.
    let mut in_list = false;
    // The name of the top-level member being read, once it is complete.
    let mut list_next = false;
    let mut expect_name = false;
    let mut string_start = None::<usize>;
    let mut escaped = false;
    let mut bare = false;
    for (at, byte) in text.iter().enumerate() {
        if let Some(start) = string_start {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                if whole && depth == 1 && expect_name {
                    list_next = matches!(
                        &text[start + 1..at],
                        b"files" | b"layers" | b"annotations" | b"labels"
                    );
                }
                string_start = None;
            }
            continue;
        }
        let mut value = false;
        let mut container = false;
        match byte {
            b'"' => {
                string_start = Some(at);
                bare = false;
                value = true;
            }
            b'{' | b'[' => {
                bare = false;
                value = true;
                container = true;
                depth += 1;
                if depth > bounds.depth {
                    return Err(ImportError::TooLarge {
                        bound: Bound::Depth,
                        limit: u64::from(bounds.depth),
                    });
                }
                if whole && depth == 1 {
                    expect_name = *byte == b'{';
                } else if whole && depth == 2 {
                    in_list = list_next && *byte == b'[';
                    unit = [0; 2];
                    // The list itself is the header's; its elements are
                    // units of their own.
                    if in_list {
                        header[0] += 1;
                        header[1] += 1;
                        value = false;
                    }
                }
            }
            b'}' | b']' => {
                bare = false;
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    in_list = false;
                }
            }
            b',' => {
                bare = false;
                if whole && depth == 1 {
                    expect_name = true;
                    list_next = false;
                } else if in_list && depth == 2 {
                    unit = [0; 2];
                }
            }
            b':' => {
                bare = false;
                if whole && depth == 1 {
                    expect_name = false;
                }
            }
            byte if byte.is_ascii_whitespace() => bare = false,
            _ => {
                value = !bare;
                bare = true;
            }
        }
        if value {
            let counted = if in_list { &mut unit } else { &mut header };
            counted[0] += 1;
            counted[1] += u64::from(container);
            if counted[0] > bounds.unit_values {
                return Err(too_many);
            }
            if counted[1] > bounds.unit_containers {
                return Err(too_many_containers);
            }
        }
    }
    Ok(())
}

/// What the things read so far are charged ([`crate::held`]), against
/// [`Limits::held_bytes`].
struct HeldTotal {
    bytes: u64,
    exceeded: bool,
}

impl HeldTotal {
    /// Adds `bytes`. `false` when the total is past the budget.
    fn add(&mut self, bytes: u64, bounds: &Limits) -> bool {
        self.bytes = self.bytes.saturating_add(bytes);
        self.exceeded |= self.bytes > bounds.held_bytes;
        !self.exceeded
    }

    fn error(bounds: &Limits) -> ImportError {
        ImportError::TooLarge {
            bound: Bound::HeldBytes,
            limit: bounds.held_bytes,
        }
    }
}

/// Puts `item` at the end of `list`, which grows by a quarter when it is
/// full, and returns what the item is charged.
fn keep<T: Charged>(list: &mut Vec<T>, item: T) -> u64 {
    let charge = item.charge();
    if list.len() == list.capacity() {
        list.reserve_exact((list.capacity() / 4).max(16));
    }
    list.push(item);
    charge
}

/// One of a document's four lists, read an element at a time, so that no
/// more than one element is held in the parser's own form at once.
struct List<'a, T> {
    into: &'a mut Vec<T>,
    kept: &'a std::cell::RefCell<HeldTotal>,
    bounds: &'a Limits,
}

impl<'de, T: Deserialize<'de> + Charged> serde::de::DeserializeSeed<'de> for List<'_, T> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, T: Deserialize<'de> + Charged> serde::de::Visitor<'de> for List<'_, T> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an array")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut items: A) -> Result<(), A::Error> {
        while let Some(item) = items.next_element::<T>()? {
            let held = keep(self.into, item);
            if !self.kept.borrow_mut().add(held, self.bounds) {
                return Err(serde::de::Error::custom("too much held"));
            }
        }
        Ok(())
    }
}

/// A document read member by member: the four lists into `document`, an
/// element at a time, and every other member into a map that
/// [`check_values`] has bounded.
struct Members<'a> {
    document: &'a mut Lists,
    kept: &'a std::cell::RefCell<HeldTotal>,
    bounds: &'a Limits,
}

#[derive(Default)]
struct Lists {
    files: Vec<FileRef>,
    layers: Vec<Layer>,
    annotations: Vec<Annotation>,
    labels: Vec<Label>,
    seen: [bool; 4],
}

impl<'de> serde::de::Visitor<'de> for Members<'_> {
    type Value = serde_json::Map<String, serde_json::Value>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a document")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut members: A,
    ) -> Result<Self::Value, A::Error> {
        let mut rest = serde_json::Map::new();
        while let Some(name) = members.next_key::<String>()? {
            let list = ["files", "layers", "annotations", "labels"]
                .iter()
                .position(|known| *known == name);
            if let Some(list) = list {
                if std::mem::replace(&mut self.document.seen[list], true) {
                    return Err(serde::de::Error::custom("a member twice"));
                }
            }
            macro_rules! list {
                ($field:ident) => {
                    members.next_value_seed(List {
                        into: &mut self.document.$field,
                        kept: self.kept,
                        bounds: self.bounds,
                    })?
                };
            }
            match list {
                Some(0) => list!(files),
                Some(1) => list!(layers),
                Some(2) => list!(annotations),
                Some(3) => list!(labels),
                _ => {
                    if rest.insert(name, members.next_value()?).is_some() {
                        return Err(serde::de::Error::custom("a member twice"));
                    }
                }
            }
        }
        Ok(rest)
    }
}

/// Reads one document from `text`, which [`check_values`] has passed: the
/// lists an element at a time, the rest through `Document::from_json_str`.
fn parse_document(text: &str, bounds: &Limits) -> Result<(Document, u64), ParseError> {
    let kept = std::cell::RefCell::new(HeldTotal {
        bytes: 0,
        exceeded: false,
    });
    let mut lists = Lists::default();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let rest = serde::Deserializer::deserialize_map(
        &mut deserializer,
        Members {
            document: &mut lists,
            kept: &kept,
            bounds,
        },
    )
    .and_then(|rest| deserializer.end().map(|()| rest));
    if kept.borrow().exceeded {
        return Err(ParseError::Held);
    }
    let rest = rest.map_err(|_| ParseError::Document(DocumentError::Malformed(String::new())))?;
    let header = serde_json::to_string(&rest)
        .map_err(|_| ParseError::Document(DocumentError::Malformed(String::new())))?;
    drop(rest);
    let mut document = Document::from_json_str(&header).map_err(ParseError::Document)?;
    drop(header);
    if !kept.borrow_mut().add(header_charge(&document), bounds) {
        return Err(ParseError::Held);
    }
    document.files = lists.files;
    document.layers = lists.layers;
    document.annotations = lists.annotations;
    document.labels = lists.labels;
    let held = kept.borrow().bytes;
    Ok((document, held))
}

enum ParseError {
    Document(DocumentError),
    Held,
}

fn document_error(error: DocumentError, bounds: &Limits) -> ImportError {
    match error {
        DocumentError::TooLarge { .. } => ImportError::TooLarge {
            bound: Bound::DocumentBytes,
            limit: bounds.document_bytes,
        },
        DocumentError::WrongFormat { .. } => ImportError::WrongFormat,
        DocumentError::UnsupportedVersion { .. } => ImportError::UnsupportedVersion,
        DocumentError::Malformed(_) => ImportError::Malformed { row: 1 },
    }
}

fn text_at(buffer: &[u8], row: u64) -> Result<&str, ImportError> {
    std::str::from_utf8(buffer).map_err(|_| ImportError::Malformed { row })
}

fn document_rows(document: &Document) -> RecordRows {
    let mut row = 2..;
    RecordRows {
        files: row.by_ref().take(document.files.len()).collect(),
        layers: row.by_ref().take(document.layers.len()).collect(),
        annotations: row.by_ref().take(document.annotations.len()).collect(),
        labels: row.take(document.labels.len()).collect(),
    }
}

fn read_document(
    input: &mut dyn Read,
    bounds: &Limits,
) -> Result<(Document, RecordRows), ImportError> {
    let mut reader = BufReader::new(input);
    let mut buffer = Vec::new();
    if !read_line(
        &mut reader,
        &mut buffer,
        bounds.document_bytes,
        Bound::DocumentBytes,
    )? {
        return Err(ImportError::Malformed { row: 1 });
    }
    if buffer.starts_with(b"\xef\xbb\xbf") {
        buffer.drain(..3);
    }
    let parsed = |buffer: &[u8]| {
        check_values(buffer, true, 0, bounds)?;
        match parse_document(text_at(buffer, 1)?, bounds) {
            Ok(document) => Ok(Ok(document)),
            Err(ParseError::Held) => Err(HeldTotal::error(bounds)),
            Err(ParseError::Document(error)) => Ok(Err(error)),
        }
    };
    let (mut document, held) = match parsed(&buffer)? {
        Ok(document) => document,
        Err(DocumentError::Malformed(_)) => {
            buffer.push(b'\n');
            let remaining = bounds.document_bytes.saturating_sub(buffer.len() as u64);
            reader
                .take(remaining + 1)
                .read_to_end(&mut buffer)
                .map_err(import_io)?;
            if buffer.len() as u64 > bounds.document_bytes {
                return Err(ImportError::TooLarge {
                    bound: Bound::DocumentBytes,
                    limit: bounds.document_bytes,
                });
            }
            let (document, _) = parsed(&buffer)?.map_err(|error| document_error(error, bounds))?;
            let rows = document_rows(&document);
            return Ok((document, rows));
        }
        Err(error) => return Err(document_error(error, bounds)),
    };
    // Release the possibly document-sized first line before reading records.
    drop(buffer);
    let mut buffer = Vec::new();
    let mut rows = RecordRows::default();
    let mut kept = HeldTotal {
        bytes: held,
        exceeded: false,
    };
    let mut has_lines = false;
    let mut ended = false;
    let mut row = 1;
    while read_line(
        &mut reader,
        &mut buffer,
        bounds.line_bytes,
        Bound::LineBytes,
    )? {
        row += 1;
        if buffer.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if ended {
            return Err(ImportError::Malformed { row });
        }
        if !has_lines {
            if !document.files.is_empty()
                || !document.layers.is_empty()
                || !document.annotations.is_empty()
                || !document.labels.is_empty()
            {
                return Err(ImportError::Malformed { row: 1 });
            }
            has_lines = true;
        }
        check_values(&buffer, false, 0, bounds)?;
        let line: InputLine = serde_json::from_str(text_at(&buffer, row)?)
            .map_err(|_| ImportError::Malformed { row })?;
        let held = match line {
            InputLine::End(counts) => {
                if counts != EndCounts::of(&document) {
                    return Err(ImportError::Truncated);
                }
                ended = true;
                0
            }
            InputLine::File(file) => {
                check_count(document.files.len() + 1, bounds.files, Bound::Files)?;
                rows.files.push(row);
                keep(&mut document.files, file)
            }
            InputLine::Layer(layer) => {
                if document.layers.len() >= dcmview_annotation::limits::MAX_SCHEMA_ITEMS {
                    return Err(ImportError::Invalid {
                        row,
                        code: ViolationCode::TooManyItems,
                    });
                }
                rows.layers.push(row);
                keep(&mut document.layers, layer)
            }
            InputLine::Annotation(annotation) => {
                check_count(
                    document.annotations.len() + document.labels.len() + 1,
                    bounds.records,
                    Bound::Records,
                )?;
                rows.annotations.push(row);
                keep(&mut document.annotations, annotation)
            }
            InputLine::Label(label) => {
                check_count(
                    document.annotations.len() + document.labels.len() + 1,
                    bounds.records,
                    Bound::Records,
                )?;
                rows.labels.push(row);
                keep(&mut document.labels, label)
            }
        };
        if !kept.add(held, bounds) {
            return Err(HeldTotal::error(bounds));
        }
    }
    if !has_lines {
        // A text on one line that holds no record is a JSON Lines header
        // whose lines are missing, the end line with them.
        if document.files.is_empty()
            && document.layers.is_empty()
            && document.annotations.is_empty()
            && document.labels.is_empty()
        {
            return Err(ImportError::Truncated);
        }
        rows = document_rows(&document);
    } else if !ended {
        return Err(ImportError::Truncated);
    }
    Ok((document, rows))
}

fn check_count(count: usize, limit: usize, bound: Bound) -> Result<(), ImportError> {
    if count > limit {
        Err(ImportError::TooLarge {
            bound,
            limit: limit as u64,
        })
    } else {
        Ok(())
    }
}

fn invalid_at(row: u64, invalid: Invalid) -> ImportError {
    ImportError::Invalid {
        row,
        code: invalid
            .violations
            .first()
            .map_or(ViolationCode::TooManyItems, |violation| violation.code),
    }
}

enum Placement {
    Matched(FileKey, ImageSize),
    Unmatched,
    Refused,
}

fn place_document(
    document: Document,
    rows: RecordRows,
    context: &ImportContext<'_>,
) -> Result<Imported, ImportError> {
    let bounds = &context.limits;
    check_count(document.files.len(), bounds.files, Bound::Files)?;
    check_count(
        document.annotations.len() + document.labels.len(),
        bounds.records,
        Bound::Records,
    )?;
    let mut report = Report::new(
        AdapterRef {
            id: NATIVE_ADAPTER_ID.to_string(),
            version: 1,
        },
        Direction::Import,
    );
    if document.schema.is_some() && document.schema.as_ref() != context.schema {
        report.warn(WarningCode::SchemaNotApplied, RowRef { row: 1, roi: None });
    }
    let implicit = LabelSchema::implicit();
    let schema = context.schema.unwrap_or(&implicit);
    // Every structure with an entry a record is made once, at its final
    // size: none of them doubles while records are placed.
    let mut files = HashMap::with_capacity(document.files.len());
    for (file, row) in document.files.into_iter().zip(rows.files) {
        file.validate().map_err(|error| invalid_at(row, error))?;
        if files.contains_key(&file.key) {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::DuplicateFileKey,
            });
        }
        let at = RowRef { row, roi: None };
        let placement = match context.resolver.resolve(&file) {
            Resolution::Unmatched => {
                report.warn(WarningCode::FileUnmatched, at);
                Placement::Unmatched
            }
            Resolution::Refused(reason) => {
                report.warn(
                    match reason {
                        Refusal::EvidenceConflict => WarningCode::EvidenceConflict,
                        Refusal::KeyUnavailable => WarningCode::KeyUnavailable,
                        Refusal::Unverified => WarningCode::FileUnverified,
                        Refusal::Ambiguous => WarningCode::FileAmbiguous,
                    },
                    at,
                );
                Placement::Refused
            }
            Resolution::Matched(target, matcher) => {
                if file.size() != target.size {
                    report.warn(WarningCode::SizeMismatch, at);
                    Placement::Refused
                } else if let Some(key) = target.key {
                    report.counts.files += 1;
                    if let Some(matched) = &mut report.matched_by {
                        match matcher {
                            Matcher::Key => matched.key += 1,
                            Matcher::SopInstanceUid => matched.sop_instance_uid += 1,
                            Matcher::Path => matched.path += 1,
                        }
                    }
                    if file.size_bytes != target.size_bytes {
                        report.warn(WarningCode::FileSizeDiffers, at);
                    }
                    Placement::Matched(key, target.size)
                } else {
                    report.warn(WarningCode::KeyUnavailable, at);
                    Placement::Refused
                }
            }
        };
        files.insert(file.key, placement);
    }
    let mut fragment = Fragment::default();
    let mut layers = HashSet::new();
    for (i, (layer, row)) in document.layers.into_iter().zip(rows.layers).enumerate() {
        if i >= dcmview_annotation::limits::MAX_SCHEMA_ITEMS {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::TooManyItems,
            });
        }
        layer.validate().map_err(|error| invalid_at(row, error))?;
        if !layers.insert(layer.id.clone()) {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::DuplicateId,
            });
        }
        if &layer.id != context.layer {
            fragment.layers.push(layer);
        }
    }
    layers.insert(context.layer.clone());
    let mut ids = HashSet::with_capacity(document.annotations.len());
    fragment
        .annotations
        .reserve_exact(document.annotations.len());
    for (mut annotation, row) in document.annotations.into_iter().zip(rows.annotations) {
        if !(context.keep_going)() {
            return Err(ImportError::Cancelled);
        }
        report.counts.annotations_in += 1;
        let Some((key, size)) = placed_file(&files, &annotation.file, row, &mut report)? else {
            continue;
        };
        annotation.file = key.clone();
        if !layers.contains(&annotation.layer) {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::UnknownLayer,
            });
        }
        if !ids.insert(annotation.id) {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::DuplicateId,
            });
        }
        annotation.geometry = annotation.geometry.quantized();
        let sizes = |candidate: &FileKey| (candidate == key).then_some(size);
        if let Err(error) = annotation.validate(&Context {
            files: &sizes,
            schema,
        }) {
            if !matches!(annotation.geometry, Geometry::Rect { .. })
                || !error.violations.iter().all(|violation| {
                    matches!(
                        violation.code,
                        ViolationCode::OutOfBounds
                            | ViolationCode::Degenerate
                            | ViolationCode::FramesEmpty
                    )
                })
            {
                return Err(invalid_at(row, error));
            }
            report_box(&mut report, &annotation, size, row);
        }
        report.counts.annotations_out += 1;
        fragment.annotations.push(annotation);
    }
    drop(ids);
    let mut ids = HashSet::with_capacity(document.labels.len());
    fragment.labels.reserve_exact(document.labels.len());
    for (mut label, row) in document.labels.into_iter().zip(rows.labels) {
        if !(context.keep_going)() {
            return Err(ImportError::Cancelled);
        }
        report.counts.labels_in += 1;
        let matched = match &mut label.target {
            LabelTarget::File { file } | LabelTarget::Frame { frame: file, .. } => {
                let Some((key, size)) = placed_file(&files, file, row, &mut report)? else {
                    continue;
                };
                *file = key.clone();
                Some((key, size))
            }
            _ => None,
        };
        if !layers.contains(&label.layer) {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::UnknownLayer,
            });
        }
        if !ids.insert(label.id) {
            return Err(ImportError::Invalid {
                row,
                code: ViolationCode::DuplicateId,
            });
        }
        let sizes = |candidate: &FileKey| {
            matched.and_then(|(key, size)| (candidate == key).then_some(size))
        };
        label
            .validate(&Context {
                files: &sizes,
                schema,
            })
            .map_err(|error| invalid_at(row, error))?;
        report.counts.labels_out += 1;
        fragment.labels.push(label);
    }
    Ok(Imported { fragment, report })
}

fn placed_file<'a>(
    files: &'a HashMap<FileKey, Placement>,
    file: &FileKey,
    row: u64,
    report: &mut Report,
) -> Result<Option<(&'a FileKey, ImageSize)>, ImportError> {
    match files.get(file) {
        None => Err(ImportError::Invalid {
            row,
            code: ViolationCode::UnknownFile,
        }),
        Some(Placement::Unmatched) => {
            report.counts.unmatched += 1;
            Ok(None)
        }
        Some(Placement::Refused) => {
            report.counts.refused += 1;
            Ok(None)
        }
        Some(Placement::Matched(key, size)) => Ok(Some((key, *size))),
    }
}

fn report_box(report: &mut Report, annotation: &Annotation, size: ImageSize, row: u64) {
    let Geometry::Rect { x0, y0, x1, y1 } = annotation.geometry else {
        return;
    };
    for (applies, code) in [
        (
            [x0, y0, x1, y1].iter().any(|value| *value < 0.0)
                || x0 > f64::from(size.columns)
                || x1 > f64::from(size.columns)
                || y0 > f64::from(size.rows)
                || y1 > f64::from(size.rows),
            WarningCode::OutOfRange,
        ),
        (x0 == x1 || y0 == y1, WarningCode::ZeroArea),
        (x0 > x1 || y0 > y1, WarningCode::CornersOutOfOrder),
        (
            matches!(&annotation.frames, FrameScope::Set(frames) if frames.set.is_empty()),
            WarningCode::FramesEmpty,
        ),
    ] {
        if applies {
            report.warn(code, RowRef { row, roi: None });
        }
    }
}
