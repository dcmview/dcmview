//! EMBED ROI CSV: the format `--annotations` and `Export ROIs` have always
//! read and written (`docs/design/output-adapters.md` 1, 6, 8;
//! `docs/design/annotation-model.md` 9 and its EMBED parity amendment).
//!
//! The format is rows: a path, and the boxes drawn on the file at that
//! path. This module has it at two levels.
//!
//! - **Rows** ([`read_rows`], [`write_csv`], [`EmbedRois`]). What a caller
//!   uses that holds EMBED rows by file and not as records: the viewer
//!   stages the rows of an import by file index without settling a key, and
//!   writes one row for each loaded path, byte-identical copies included.
//! - **Records** ([`EmbedAdapter`]). The same reader and writer behind the
//!   adapter traits, between rows and the rectangle annotations of a
//!   document, for a caller that holds records under file keys.
//!
//! The bytes this module writes, and what it reads without complaint, are
//! released behaviour: `tests/fixtures/embed-goldens/` in the viewer holds
//! them.
//!
//! # A box as a rectangle
//!
//! `[ymin, xmin, ymax, xmax]` is the rectangle `x0 = xmin`, `y0 = ymin`,
//! `x1 = xmax`, `y1 = ymax`, each number as it stands. A row whose
//! `ROI_frames` is empty gives every box of the row every frame; otherwise
//! box `i` has `ROI_frames[i]` as written, order and repeats included, and
//! an empty list for one box stays an empty set ([`scope_at`]).
//!
//! # Rectangles as boxes
//!
//! [`rois_of`]: `roi_coords[i]` is rectangle `i` as `[y0, x0, y1, x1]`, a
//! coordinate off the pixel grid rounded outward (the first two down, the
//! last two up) and each then brought into `0..=4294967295`. `roi_frames`
//! is empty when every rectangle is for every frame; otherwise it has one
//! list per rectangle, the list as written for a set and every frame index
//! of the file, ascending, for one that is for every frame.

use crate::adapter::{
    AdapterInfo, Capabilities, Confidence, Directions, ExportAdapter, ExportContext, FileResolver,
    ImportAdapter, ImportContext, Imported,
};
use crate::error::{ExportError, ImportError};
use crate::report::Report;
use dcmview_annotation::{
    new_id, Annotation, Author, Document, FileKey, FrameScope, Geometry, GeometryType, LayerId,
    RecordMeta, Timestamp, IMPLICIT_CLASS_ID,
};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{Read, Write};

/// The id of the EMBED adapter.
pub const EMBED_ADAPTER_ID: &str = "embed";

/// The columns of an EMBED CSV as this module writes it, in order.
pub const EMBED_COLUMNS: [&str; 4] = ["anon_dicom_path", "num_ROI", "ROI_coords", "ROI_frames"];

/// The boxes of one file as an EMBED row holds them: `roi_coords[i]` is
/// `[ymin, xmin, ymax, xmax]`, and `roi_frames` is empty or has one list
/// for each box.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmbedRois {
    pub roi_coords: Vec<[u32; 4]>,
    pub roi_frames: Vec<Vec<u32>>,
}

/// One row to write: the path, and the boxes, borrowed from whoever holds
/// them.
#[derive(Debug, Clone)]
pub struct EmbedRow<'a> {
    pub path: Cow<'a, str>,
    pub roi_coords: &'a [[u32; 4]],
    pub roi_frames: &'a [Vec<u32>],
}

/// The boxes one row gave one loaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedFileRois {
    /// The resolver's slot for the file ([`crate::Resolved::slot`]).
    pub slot: usize,
    /// The number of the row that named the file ([`crate::RowRef::row`]).
    pub row: u64,
    pub rois: EmbedRois,
}

/// What [`read_rows`] read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedRead {
    /// The boxes of each loaded file a row named, in the order of the rows
    /// and, for a row that names several files, of the resolver's answer.
    /// A slot appears once.
    pub files: Vec<EmbedFileRois>,
    pub report: Report,
}

/// The rectangle a box reads as ("A box as a rectangle").
pub fn rectangle([ymin, xmin, ymax, xmax]: [u32; 4]) -> Geometry {
    Geometry::Rect {
        x0: f64::from(xmin),
        y0: f64::from(ymin),
        x1: f64::from(xmax),
        y1: f64::from(ymax),
    }
}

/// The frame scope of box `i` of a row whose frame lists are `roi_frames`
/// ("A box as a rectangle").
pub fn scope_at(roi_frames: &[Vec<u32>], i: usize) -> FrameScope {
    roi_frames.get(i).map_or(FrameScope::All, |frames| {
        FrameScope::from_written(frames.clone())
    })
}

/// The annotation one box is: a rectangle of class `roi` in `layer`, under
/// a new id, with `author` and `stamp` as its metadata and `rev` 1.
pub fn annotation_of(
    coords: [u32; 4],
    frames: FrameScope,
    file: &FileKey,
    layer: &LayerId,
    author: &Author,
    stamp: &Timestamp,
) -> Annotation {
    Annotation {
        id: new_id(),
        file: file.clone(),
        layer: layer.clone(),
        class: IMPLICIT_CLASS_ID.to_string(),
        geometry: rectangle(coords),
        frames,
        attributes: BTreeMap::new(),
        extensions: BTreeMap::new(),
        unknown: BTreeMap::new(),
        meta: RecordMeta {
            rev: 1,
            created_by: author.clone(),
            created_at: stamp.clone(),
            modified_by: author.clone(),
            modified_at: stamp.clone(),
            derived_from: None,
            score: None,
        },
    }
}

/// The boxes `records` are, in their order ("Rectangles as boxes").
/// `frame_count` is the file's. A record that is not a rectangle is left
/// out.
pub fn rois_of<'a>(
    records: impl IntoIterator<Item = &'a Annotation>,
    frame_count: u32,
) -> EmbedRois {
    let mut roi_coords = Vec::new();
    let mut scopes = Vec::new();
    for record in records {
        if let Geometry::Rect { x0, y0, x1, y1 } = record.geometry {
            roi_coords.push([
                edge(y0.floor()),
                edge(x0.floor()),
                edge(y1.ceil()),
                edge(x1.ceil()),
            ]);
            scopes.push(&record.frames);
        }
    }
    let roi_frames = if scopes.iter().all(|scope| **scope == FrameScope::All) {
        Vec::new()
    } else {
        scopes
            .into_iter()
            .map(|scope| {
                scope
                    .written()
                    .map_or_else(|| (0..frame_count).collect(), <[u32]>::to_vec)
            })
            .collect()
    };
    EmbedRois {
        roi_coords,
        roi_frames,
    }
}

fn edge(value: f64) -> u32 {
    if value.is_nan() {
        0
    } else {
        value.clamp(0.0, u32::MAX as f64) as u32
    }
}

/// Writes `rows` as an EMBED CSV.
///
/// The header [`EMBED_COLUMNS`], then one record per row, **sorted by
/// path**: ascending byte order of the UTF-8 path as given, rows with the
/// same path in the order given (`docs/design/output-adapters.md` 6.2). A
/// record is the path, the number of boxes in decimal, and `roi_coords` and
/// `roi_frames` as compact JSON (`serde_json::to_string`), written by a
/// `csv::Writer` with its default settings: a field is quoted only when it
/// needs to be, and records end with a line feed. A row is written as
/// given, one without a box included; the caller leaves those out.
///
/// It holds the rows it is given (borrowed) to sort them, and no copy of
/// their boxes.
pub fn write_csv<'a>(
    rows: impl IntoIterator<Item = EmbedRow<'a>>,
    out: &mut dyn Write,
) -> Result<(), ExportError> {
    let _ = (rows.into_iter().count(), out);
    todo!("ADP1")
}

/// Checks that `input` starts with the header of an EMBED CSV: a first
/// record that holds the columns `anon_dicom_path` and `ROI_coords`, by
/// exact name, anywhere in it and among any others.
///
/// Reads the header and no row. [`ImportError::CsvHeader`] when no first
/// record can be read; [`ImportError::MissingColumn`] naming
/// `anon_dicom_path` when that is missing, else `ROI_coords`;
/// [`ImportError::TooLarge`] for a header past
/// [`crate::limits::MAX_CSV_ROW_BYTES`].
pub fn check_header(input: &mut dyn Read) -> Result<(), ImportError> {
    let _ = input;
    todo!("ADP1")
}

/// Reads an EMBED CSV as rows for the loaded files its paths name.
///
/// # What is read
///
/// The first record is the header ([`check_header`]'s rule). The columns
/// are found by exact name, the first of a name when it is there twice:
/// `anon_dicom_path` and `ROI_coords` are required, `num_ROI` and
/// `ROI_frames` optional, every other column ignored. Every later record is
/// a row; its number is its position in the file with the header as 1
/// ([`crate::RowRef`]). Records have the header's number of fields
/// (`csv::ReaderBuilder` with `flexible(false)`).
///
/// For each row, in this order:
///
/// 1. `keep_going()`; `false` is [`ImportError::Cancelled`].
/// 2. The path is the `anon_dicom_path` field without surrounding
///    whitespace. Empty is [`ImportError::EmptyPath`]. It is handed to
///    `resolver.by_path`, unless it is longer than
///    [`crate::limits::MAX_CSV_PATH_BYTES`], which names no file.
/// 3. **A row that names no loaded file is counted and nothing else of it
///    is looked at**, whatever its other fields hold.
/// 4. `ROI_coords`, without surrounding whitespace, is a JSON list of
///    lists of numbers ([`ImportError::RoiCoords`]), each of exactly four
///    ([`ImportError::RoiCoordsEntry`]), at most
///    [`crate::limits::MAX_ROIS_PER_ROW`] of them. Each number is taken by
///    "Lenient coordinates" below.
/// 5. `num_ROI`, when the column is there, is a non-negative integer
///    without surrounding whitespace ([`ImportError::NumRoi`]) equal to the
///    number of boxes ([`ImportError::NumRoiMismatch`]).
/// 6. `ROI_frames`, when the column is there, is a JSON list of lists of
///    integers from 0 to 4,294,967,295 ([`ImportError::RoiFrames`]), each
///    of at most [`crate::limits::MAX_FRAMES_PER_ROI`], and is empty or has
///    one list for each box ([`ImportError::RoiFramesLength`]). Without the
///    column it is empty.
/// 7. For each file the path names, in the resolver's order: a slot an
///    earlier row was matched to is [`ImportError::DuplicatePath`]; then
///    every frame index must be below that file's frame count
///    ([`ImportError::FrameOutOfRange`], the first in box order, then list
///    order); then the file gets the row's boxes.
///
/// After the last row `keep_going()` is asked once more.
///
/// Any error ends the read with nothing handed on: the import is all or
/// nothing, as it always was.
///
/// # Lenient coordinates
///
/// Owner decision of 2026-10-08 (EMBED parity amendment): "keep as loaded
/// and clamp only negative/non-integer values". For each of a box's four
/// numbers:
///
/// - a number below zero is read as 0, and the box is reported once as
///   `negative_coordinate`;
/// - otherwise a number that is not whole is rounded to the nearest whole
///   number, halves away from zero (`f64::round`), and the box is reported
///   once as `non_integer_coordinate`;
/// - a number that is then above 4,294,967,295 is
///   [`ImportError::RoiCoordsEntry`];
/// - a whole number written with a fraction or an exponent (`20.0`, `2e1`)
///   is that number, without a report.
///
/// Nothing else is changed. A box that reaches past the image edge, has no
/// area or has its corners out of order is kept exactly as read, and
/// reported: `out_of_range` when any of its four numbers, after the
/// correction above, is greater than the rows (the first and third) or the
/// columns (the second and fourth) of the first file the row names;
/// `zero_area` when `ymin == ymax` or `xmin == xmax`;
/// `corners_out_of_order` when `ymin > ymax` or `xmin > xmax`;
/// `frames_empty` when the row has frame lists and this box's is empty. A
/// box is reported once for each code that applies, in the order
/// `negative_coordinate`, `non_integer_coordinate`, `out_of_range`,
/// `zero_area`, `corners_out_of_order`, `frames_empty`, at the row and the
/// box's position in it.
///
/// # The report
///
/// The adapter is [`EMBED_ADAPTER_ID`] version 1, the direction import.
/// `counts.unmatched` is the rows that named no file;
/// `matched_by.path` the rows that named one; `counts.annotations_in` the
/// boxes of those rows; `counts.files` the files that were given boxes and
/// `counts.annotations_out` the boxes they were given (a row that names
/// two files gives its boxes to both).
///
/// # Bounds and cost
///
/// One record is at most [`crate::limits::MAX_CSV_ROW_BYTES`]: the reader
/// counts the bytes it takes from `input` for the record it is reading and
/// fails with [`ImportError::TooLarge`] before it has taken more than that
/// plus [`crate::limits::CSV_READ_AHEAD_BYTES`], so a file that never ends
/// a quoted field is not read into memory. A file holds at most
/// [`crate::limits::MAX_CSV_ROWS`] rows. Nothing is allocated from
/// `num_ROI` or from any other number the file states.
///
/// The read holds the rows it hands on, the one record it is reading with
/// what that record parses to, and for each matched slot the number of the
/// row that matched it. It reads `input` once, front to back, and looks at
/// each row once.
pub fn read_rows(
    input: &mut dyn Read,
    resolver: &dyn FileResolver,
    keep_going: &dyn Fn() -> bool,
) -> Result<EmbedRead, ImportError> {
    let _ = (input, keep_going());
    let mut found = Vec::new();
    resolver.by_path("", &mut found);
    todo!("ADP1")
}

/// EMBED ROI CSV behind the adapter traits, for a caller that holds
/// records under file keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmbedAdapter;

impl EmbedAdapter {
    fn describe() -> AdapterInfo {
        AdapterInfo {
            id: EMBED_ADAPTER_ID.to_string(),
            name: "EMBED ROI CSV".to_string(),
            version: 1,
            directions: Directions::Both,
            extension: "csv".to_string(),
            media_type: "text/csv; charset=utf-8".to_string(),
            capabilities: Capabilities {
                geometry: vec![GeometryType::Rect],
                classes: false,
                attributes: false,
                labels: false,
                multi_frame: true,
                scores: false,
                lossless: false,
            },
        }
    }
}

impl ExportAdapter for EmbedAdapter {
    fn info(&self) -> AdapterInfo {
        Self::describe()
    }

    /// Writes the rectangles of `document` as an EMBED CSV ([`write_csv`]).
    ///
    /// For each file of `document.files`, the rectangle annotations whose
    /// `file` is its key, in the document's order, are its boxes
    /// ([`rois_of`] with the file's `frames`). A file with at least one box
    /// is written once for each path `context.paths` gives for its key, or
    /// once under its `FileRef::path` when it gives none. An annotation
    /// whose `file` no `FileRef` of the document has is
    /// [`ExportError::UnknownFile`]. Output past `context.max_bytes` is
    /// [`ExportError::TooLarge`].
    ///
    /// The report: `counts.annotations_in` and `labels_in` are the
    /// document's; `annotations_out` the rectangles written (each once,
    /// however many paths its file has); `files` the rows written;
    /// `labels_out` 0. Losses, in this order, each only when its count is
    /// not zero: `type_skipped` for each geometry type in
    /// `GeometryType::ALL` order that the document has annotations of other
    /// than `rect`, with the type as detail and their number;
    /// `rounded_outward`, the written rectangles with a coordinate that is
    /// not whole; `classes_merged`, the number of different classes among
    /// the written rectangles when there is more than one;
    /// `attributes_dropped`, the written rectangles that have an
    /// attribute; `scores_dropped`, those that have a score;
    /// `labels_dropped`, the document's labels. No loss has rows.
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

impl ImportAdapter for EmbedAdapter {
    fn info(&self) -> AdapterInfo {
        Self::describe()
    }

    /// [`Confidence::Certain`] when the first line of `head` (all of it
    /// when it holds no line feed), read as one CSV record, has the fields
    /// `anon_dicom_path` and `ROI_coords`; [`Confidence::None`] otherwise.
    fn sniff(&self, head: &[u8]) -> Confidence {
        let _ = head;
        todo!("ADP1")
    }

    /// Reads an EMBED CSV ([`read_rows`] with `context.resolver` and
    /// `context.keep_going`) as rectangle annotations.
    ///
    /// Each box of each file the rows name becomes one annotation
    /// ([`annotation_of`] with [`scope_at`]) under the key the resolver
    /// gave for that file, in `context.layer`, with `context.author` and
    /// `context.now`, in the order of [`EmbedRead::files`]. A file the
    /// resolver gave no key for gets no annotation: it is reported as
    /// `key_unavailable` (one for the file, at the row that named it) and
    /// its boxes are counted in `counts.refused` and taken out of
    /// `counts.annotations_out` and `counts.files`. The fragment has no
    /// layer and no label.
    fn import(
        &self,
        input: &mut dyn Read,
        context: &ImportContext<'_>,
    ) -> Result<Imported, ImportError> {
        let _ = (input, context.layer);
        todo!("ADP1")
    }
}
