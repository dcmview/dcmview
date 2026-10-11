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
//!   the document, **unless** the line was ended by a line feed and its
//!   four lists are all empty: that is a JSON Lines header whose lines,
//!   the end line with them, are missing, and it is
//!   [`ImportError::Truncated`]. (A document without a record that is not
//!   followed by a line feed is a document. No export of a session is
//!   one: a session always has a layer.);
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
//!    - a refusal: `evidence_conflict`, `file_unverified` or
//!      `key_unavailable` at its row, by the [`Refusal`];
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
//! An import holds the records it read once, and the text of the document
//! or of one line at a time while it is read, in a buffer that grows as
//! it reads. What that comes to depends on the text, because the model
//! keeps members it does not know, `attributes` and `extensions` as JSON
//! values, and reads an object that has such members through them:
//!
//! - a document a session wrote (rectangles, a few members a record):
//!   within **five times** the text's length, the records it leaves
//!   included;
//! - text made to be costly: up to **44 times** its length while it is
//!   read. The worst found is an unknown member that is an array of
//!   one-digit numbers (41 times); a long string holds 8 times, and text
//!   that is no document less than twice.
//!
//! So a JSON document or a first line of the bound's 256 MiB can make the
//! reader hold about 11 GiB, and one record line of 16 MiB about 0.7 GiB:
//! the JSON Lines form is the one to use for large or untrusted files.
//! `tests/raster_cost/annotations.rs` pins these multiples.
//!
//! It looks at each record a fixed number of times and finds a file, a
//! layer and an id through a map, not by scanning a list for each record.
//! The resolver is asked once for each file of the input.

use crate::adapter::{
    AdapterInfo, Capabilities, Confidence, Directions, ExportAdapter, ExportContext, Fragment,
    ImportAdapter, ImportContext, Imported, Matcher, Refusal, Resolution,
};
use crate::error::{Bound, ExportError, ImportError};
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
    /// A document with more files than `context.limits.files`, or more
    /// annotations and labels together than `context.limits.records`, is
    /// [`ExportError::TooMany`] with that bound and number, before anything
    /// is written: an export never writes what an import within the same
    /// bounds refuses for its size. Output past `context.max_bytes` or past
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
        let header = Document {
            format: document.format.clone(),
            version: document.version.clone(),
            schema: document.schema.clone(),
            extensions: document.extensions.clone(),
            unknown: document.unknown.clone(),
            files: Vec::new(),
            layers: Vec::new(),
            annotations: Vec::new(),
            labels: Vec::new(),
        };
        let mut writer = LimitedWriter {
            out,
            remaining: context.max_bytes,
            exceeded: false,
        };
        let result = (|| {
            write_line(&mut writer, &header)?;
            for file in &document.files {
                write_line(&mut writer, &OutputLine::File(file))?;
            }
            for layer in &document.layers {
                write_line(&mut writer, &OutputLine::Layer(layer))?;
            }
            for annotation in &document.annotations {
                write_line(&mut writer, &OutputLine::Annotation(annotation))?;
            }
            for label in &document.labels {
                write_line(&mut writer, &OutputLine::Label(label))?;
            }
            write_line(&mut writer, &OutputLine::End(EndCounts::of(document)))?;
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

fn write_line(writer: &mut dyn Write, value: &impl Serialize) -> Result<(), ExportError> {
    serde_json::to_writer(&mut *writer, value).map_err(json_export_error)?;
    writer.write_all(b"\n").map_err(|error| ExportError::Io {
        kind: error.kind().to_string(),
    })
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
) -> Result<Option<LineEnd>, ImportError> {
    buffer.clear();
    let count = reader
        .take(limit + 1)
        .read_until(b'\n', buffer)
        .map_err(import_io)?;
    if count == 0 {
        return Ok(None);
    }
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
        return Ok(Some(LineEnd::LineFeed));
    }
    if buffer.len() as u64 > limit {
        return Err(ImportError::TooLarge { bound, limit });
    }
    Ok(Some(LineEnd::Input))
}

/// What ended a line that was read: a line feed, or the input.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineEnd {
    LineFeed,
    Input,
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
    let Some(first_end) = read_line(
        &mut reader,
        &mut buffer,
        bounds.document_bytes,
        Bound::DocumentBytes,
    )?
    else {
        return Err(ImportError::Malformed { row: 1 });
    };
    if buffer.starts_with(b"\xef\xbb\xbf") {
        buffer.drain(..3);
    }
    let mut document = match Document::from_json_str(text_at(&buffer, 1)?) {
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
            let document = Document::from_json_str(text_at(&buffer, 1)?)
                .map_err(|error| document_error(error, bounds))?;
            let rows = document_rows(&document);
            return Ok((document, rows));
        }
        Err(error) => return Err(document_error(error, bounds)),
    };
    // Release the possibly document-sized first line before reading records.
    drop(buffer);
    let mut buffer = Vec::new();
    let mut rows = RecordRows::default();
    let mut has_lines = false;
    let mut ended = false;
    let mut row = 1;
    while read_line(
        &mut reader,
        &mut buffer,
        bounds.line_bytes,
        Bound::LineBytes,
    )?
    .is_some()
    {
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
        let line: InputLine = serde_json::from_str(text_at(&buffer, row)?)
            .map_err(|_| ImportError::Malformed { row })?;
        match line {
            InputLine::End(counts) => {
                if counts != EndCounts::of(&document) {
                    return Err(ImportError::Truncated);
                }
                ended = true;
            }
            InputLine::File(file) => {
                check_count(document.files.len() + 1, bounds.files, Bound::Files)?;
                document.files.push(file);
                rows.files.push(row);
            }
            InputLine::Layer(layer) => {
                if document.layers.len() >= dcmview_annotation::limits::MAX_SCHEMA_ITEMS {
                    return Err(ImportError::Invalid {
                        row,
                        code: ViolationCode::TooManyItems,
                    });
                }
                document.layers.push(layer);
                rows.layers.push(row);
            }
            InputLine::Annotation(annotation) => {
                check_count(
                    document.annotations.len() + document.labels.len() + 1,
                    bounds.records,
                    Bound::Records,
                )?;
                document.annotations.push(annotation);
                rows.annotations.push(row);
            }
            InputLine::Label(label) => {
                check_count(
                    document.annotations.len() + document.labels.len() + 1,
                    bounds.records,
                    Bound::Records,
                )?;
                document.labels.push(label);
                rows.labels.push(row);
            }
        }
    }
    if !has_lines {
        // A line of its own that holds no record is a JSON Lines header
        // whose lines are missing, the end line with them.
        if first_end == LineEnd::LineFeed
            && document.files.is_empty()
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
    let mut files = HashMap::new();
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
    let mut ids = HashSet::new();
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
    let mut ids = HashSet::new();
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
