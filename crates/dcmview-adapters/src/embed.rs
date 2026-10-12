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
    Fragment, ImportAdapter, ImportContext, Imported, Resolved,
};
use crate::error::{Bound, ExportError, ImportError};
use crate::limits::{self, Limits};
use crate::report::{AdapterRef, Direction, LossCode, Report, RowRef, WarningCode};
use dcmview_annotation::{
    new_id, Annotation, Author, Document, FileKey, FrameScope, Geometry, GeometryType, LayerId,
    RecordMeta, Timestamp, IMPLICIT_CLASS_ID,
};
use serde::de::IgnoredAny;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::rc::Rc;

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
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut writer = csv::Writer::from_writer(out);
    writer
        .write_record(EMBED_COLUMNS)
        .map_err(csv_export_error)?;
    for row in rows {
        let count = row.roi_coords.len().to_string();
        let coords = serde_json::to_string(row.roi_coords)
            .map_err(|_| export_io(std::io::ErrorKind::Other))?;
        let frames = serde_json::to_string(row.roi_frames)
            .map_err(|_| export_io(std::io::ErrorKind::Other))?;
        writer
            .write_record([row.path.as_ref(), &count, &coords, &frames])
            .map_err(csv_export_error)?;
    }
    writer.flush().map_err(|error| export_io(error.kind()))
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
    let bounds = &Limits::DEFAULT;
    let (mut reader, budget) = csv_reader(input, bounds);
    read_header(&mut reader, &budget, bounds).map(|_| ())
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
/// 4. `ROI_coords`, without surrounding whitespace, is a JSON list
///    ([`ImportError::RoiCoords`]) of at most
///    [`crate::limits::MAX_ROIS_PER_ROW`] entries
///    ([`ImportError::TooLarge`]), each a list
///    ([`ImportError::RoiCoords`]) of exactly four entries
///    ([`ImportError::RoiCoordsEntry`] for the first that is not), each a
///    number ([`ImportError::RoiCoords`]); judged in that order, by
///    "Counting before keeping" below. Each number is taken by "Lenient
///    coordinates".
/// 5. `num_ROI`, when the column is there, is a non-negative integer
///    without surrounding whitespace ([`ImportError::NumRoi`]) equal to the
///    number of boxes ([`ImportError::NumRoiMismatch`]).
/// 6. `ROI_frames`, when the column is there, is a JSON list of lists of
///    integers from 0 to 4,294,967,295 ([`ImportError::RoiFrames`]): at
///    most [`crate::limits::MAX_ROIS_PER_ROW`] lists, each of at most
///    [`crate::limits::MAX_FRAMES_PER_ROI`] ([`ImportError::TooLarge`]),
///    counted as in step 4. It is empty or has one list for each box
///    ([`ImportError::RoiFramesLength`]). Without the column it is empty.
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
///   `negative_coordinate`. Negative zero (`-0`, `-0.0`) is zero and is
///   not below it: it is read as 0 without a report;
/// - otherwise a number that is not whole is rounded to the nearest whole
///   number, halves away from zero (`f64::round`), and the box is reported
///   once as `non_integer_coordinate`;
/// - a number that is then above 4,294,967,295 is
///   [`ImportError::CoordinateTooLarge`];
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
/// boxes of those rows; `counts.files` the files those rows named (a file
/// is counted for a row without a box too) and `counts.annotations_out`
/// the boxes they were given (a row that names two files gives its boxes
/// to both).
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
/// ## Counting before keeping
///
/// A field of a matched row is counted before its values are kept, so
/// that a row of millions of empty boxes, or one box of millions of
/// numbers, costs no more than the row's own text. `ROI_coords` and
/// `ROI_frames` are each read from the row's text three times: as a list
/// of anything (`Vec<serde::de::IgnoredAny>`, which holds nothing) for the
/// number of entries; then as a list of lists of anything for the length
/// of each entry; and only then, both within their bounds, as values
/// (`Vec<[f64; 4]>` and `Vec<Vec<u32>>`).
///
/// The read holds the rows it hands on, the one record it is reading with
/// what that record parses to, and for each matched slot the number of the
/// row that matched it. A row that is refused for its counts is refused
/// while the reader holds no more than three times
/// [`crate::limits::MAX_CSV_ROW_BYTES`]. It reads `input` once, front to
/// back.
pub fn read_rows(
    input: &mut dyn Read,
    resolver: &dyn FileResolver,
    keep_going: &dyn Fn() -> bool,
) -> Result<EmbedRead, ImportError> {
    read_rows_within(input, resolver, keep_going, &Limits::DEFAULT)
}

/// [`read_rows`] within `bounds` instead of the constants of
/// [`crate::limits`]: every bound the reader's contract names is the member
/// of `bounds` that stands for it, and an error gives that number.
pub fn read_rows_within(
    input: &mut dyn Read,
    resolver: &dyn FileResolver,
    keep_going: &dyn Fn() -> bool,
    bounds: &Limits,
) -> Result<EmbedRead, ImportError> {
    let (mut reader, budget) = csv_reader(input, bounds);
    let columns = read_header(&mut reader, &budget, bounds)?;
    let mut report = Report::new(
        AdapterRef {
            id: EMBED_ADAPTER_ID.to_string(),
            version: 1,
        },
        Direction::Import,
    );
    let mut files = Vec::new();
    let mut matched = HashMap::new();
    let mut targets = Vec::new();
    let mut record = csv::StringRecord::new();
    let mut row = 1;
    loop {
        if !keep_going() {
            return Err(ImportError::Cancelled);
        }
        budget.reset();
        let start = reader.position().byte();
        let read = reader.read_record(&mut record);
        if budget.exceeded.get() || reader.position().byte() - start > bounds.csv_row_bytes {
            return Err(csv_row_too_large(bounds));
        }
        let has_record = read.map_err(|error| match error.kind() {
            csv::ErrorKind::Io(error) => ImportError::Io {
                kind: error.kind().to_string(),
            },
            _ => ImportError::CsvRow { row: row + 1 },
        })?;
        if !has_record {
            break;
        }
        row += 1;
        if row - 1 > bounds.csv_rows {
            return Err(ImportError::TooLarge {
                bound: Bound::CsvRows,
                limit: bounds.csv_rows,
            });
        }
        let path = record.get(columns.path).unwrap_or_default().trim();
        if path.is_empty() {
            return Err(ImportError::EmptyPath { row });
        }
        targets.clear();
        if path.len() <= bounds.csv_path_bytes {
            resolver.by_path(path, &mut targets);
        }
        let Some(first) = targets.first() else {
            report.counts.unmatched += 1;
            continue;
        };
        let size = first.size;
        let text = record.get(columns.coords).unwrap_or_default().trim();
        let count: Vec<IgnoredAny> =
            serde_json::from_str(text).map_err(|_| ImportError::RoiCoords { row })?;
        check_roi_count(count.len(), bounds)?;
        let shapes: Vec<Vec<IgnoredAny>> =
            serde_json::from_str(text).map_err(|_| ImportError::RoiCoords { row })?;
        if let Some(roi) = shapes.iter().position(|shape| shape.len() != 4) {
            return Err(ImportError::RoiCoordsEntry { row, roi });
        }
        drop(shapes);
        let values: Vec<[f64; 4]> =
            serde_json::from_str(text).map_err(|_| ImportError::RoiCoords { row })?;
        let mut coords = Vec::with_capacity(values.len());
        let mut corrections = Vec::with_capacity(values.len());
        for (roi, values) in values.into_iter().enumerate() {
            let mut negative = false;
            let mut fractional = false;
            let mut converted = [0; 4];
            for (place, number) in converted.iter_mut().zip(values) {
                let value = if number < 0.0 {
                    negative = true;
                    0.0
                } else {
                    let rounded = number.round();
                    fractional |= rounded != number;
                    rounded
                };
                if value > f64::from(u32::MAX) {
                    return Err(ImportError::CoordinateTooLarge { row, roi });
                }
                *place = value as u32;
            }
            coords.push(converted);
            corrections.push((negative, fractional));
        }
        if let Some(column) = columns.count {
            let num_roi = record
                .get(column)
                .unwrap_or_default()
                .trim()
                .parse::<usize>()
                .map_err(|_| ImportError::NumRoi { row })?;
            if num_roi != coords.len() {
                return Err(ImportError::NumRoiMismatch {
                    row,
                    coords: coords.len(),
                    num_roi,
                });
            }
        }
        let frames = if let Some(column) = columns.frames {
            let text = record.get(column).unwrap_or_default().trim();
            let count: Vec<IgnoredAny> =
                serde_json::from_str(text).map_err(|_| ImportError::RoiFrames { row })?;
            check_roi_count(count.len(), bounds)?;
            let shapes: Vec<Vec<IgnoredAny>> =
                serde_json::from_str(text).map_err(|_| ImportError::RoiFrames { row })?;
            if shapes
                .iter()
                .any(|shape| shape.len() > bounds.frames_per_roi)
            {
                return Err(ImportError::TooLarge {
                    bound: Bound::FramesPerRoi,
                    limit: bounds.frames_per_roi as u64,
                });
            }
            drop(shapes);
            let frames: Vec<Vec<u32>> =
                serde_json::from_str(text).map_err(|_| ImportError::RoiFrames { row })?;
            if !frames.is_empty() && frames.len() != coords.len() {
                return Err(ImportError::RoiFramesLength {
                    row,
                    frames: frames.len(),
                    num_roi: coords.len(),
                });
            }
            frames
        } else {
            Vec::new()
        };
        let rois = EmbedRois {
            roi_coords: coords,
            roi_frames: frames,
        };
        for target in &targets {
            if let Some(previous_row) = matched.insert(target.slot, row) {
                return Err(ImportError::DuplicatePath { row, previous_row });
            }
            for (roi, frames) in rois.roi_frames.iter().enumerate() {
                for &frame in frames {
                    if frame >= target.size.frames {
                        return Err(ImportError::FrameOutOfRange {
                            row,
                            roi,
                            frame,
                            frame_count: target.size.frames,
                        });
                    }
                }
            }
            files.push(EmbedFileRois {
                slot: target.slot,
                row,
                rois: rois.clone(),
            });
        }
        for (i, ([ymin, xmin, ymax, xmax], (negative, fractional))) in
            rois.roi_coords.iter().zip(corrections).enumerate()
        {
            let at = RowRef {
                row,
                roi: Some(i as u32),
            };
            for (applies, code) in [
                (negative, WarningCode::NegativeCoordinate),
                (fractional, WarningCode::NonIntegerCoordinate),
                (
                    *ymin > size.rows
                        || *ymax > size.rows
                        || *xmin > size.columns
                        || *xmax > size.columns,
                    WarningCode::OutOfRange,
                ),
                (ymin == ymax || xmin == xmax, WarningCode::ZeroArea),
                (ymin > ymax || xmin > xmax, WarningCode::CornersOutOfOrder),
                (
                    rois.roi_frames.get(i).is_some_and(Vec::is_empty),
                    WarningCode::FramesEmpty,
                ),
            ] {
                if applies {
                    report.warn(code, at);
                }
            }
        }
        report.counts.annotations_in += rois.roi_coords.len() as u64;
        report.counts.annotations_out += rois.roi_coords.len() as u64 * targets.len() as u64;
        report.counts.files += targets.len() as u64;
        if let Some(matched_by) = &mut report.matched_by {
            matched_by.path += 1;
        }
    }
    Ok(EmbedRead { files, report })
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
        let mut report = Report::new(
            AdapterRef {
                id: EMBED_ADAPTER_ID.to_string(),
                version: 1,
            },
            Direction::Export,
        );
        report.counts.annotations_in = document.annotations.len() as u64;
        report.counts.labels_in = document.labels.len() as u64;
        let known: HashSet<_> = document.files.iter().map(|file| &file.key).collect();
        let mut rectangles: HashMap<&FileKey, Vec<&Annotation>> = HashMap::new();
        let mut skipped = HashMap::new();
        let mut classes = HashSet::new();
        let (mut rounded, mut attributes, mut scores) = (0, 0, 0);
        for annotation in &document.annotations {
            if !known.contains(&annotation.file) {
                return Err(ExportError::UnknownFile);
            }
            if let Geometry::Rect { x0, y0, x1, y1 } = annotation.geometry {
                rectangles
                    .entry(&annotation.file)
                    .or_default()
                    .push(annotation);
                report.counts.annotations_out += 1;
                rounded += u64::from([x0, y0, x1, y1].iter().any(|value| value.fract() != 0.0));
                classes.insert(&annotation.class);
                attributes += u64::from(!annotation.attributes.is_empty());
                scores += u64::from(annotation.meta.score.is_some());
            } else {
                *skipped
                    .entry(annotation.geometry.geometry_type())
                    .or_insert(0) += 1;
            }
        }
        let mut rows = Vec::new();
        for file in &document.files {
            let Some(records) = rectangles.get(&file.key) else {
                continue;
            };
            let rois = rois_of(records.iter().copied(), file.frames);
            let mut paths = Vec::new();
            context.paths.paths_of(&file.key, &mut paths);
            if paths.is_empty() {
                paths.push(file.path.clone());
            }
            report.counts.files += paths.len() as u64;
            rows.push((paths, rois));
        }
        let mut writer = LimitedWriter {
            out,
            remaining: context.max_bytes,
            exceeded: false,
        };
        let result = write_csv(
            rows.iter().flat_map(|(paths, rois)| {
                paths.iter().map(|path| EmbedRow {
                    path: Cow::Borrowed(path),
                    roi_coords: &rois.roi_coords,
                    roi_frames: &rois.roi_frames,
                })
            }),
            &mut writer,
        );
        if writer.exceeded {
            return Err(ExportError::TooLarge {
                limit: context.max_bytes,
            });
        }
        result?;
        for kind in GeometryType::ALL {
            report.lose(
                LossCode::TypeSkipped,
                Some(kind),
                skipped.get(&kind).copied().unwrap_or(0),
                None,
            );
        }
        report.lose(LossCode::RoundedOutward, None, rounded, None);
        report.lose(
            LossCode::ClassesMerged,
            None,
            if classes.len() > 1 {
                classes.len() as u64
            } else {
                0
            },
            None,
        );
        report.lose(LossCode::AttributesDropped, None, attributes, None);
        report.lose(LossCode::ScoresDropped, None, scores, None);
        report.lose(
            LossCode::LabelsDropped,
            None,
            document.labels.len() as u64,
            None,
        );
        Ok(report)
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
        let head = head.split(|byte| *byte == b'\n').next().unwrap_or_default();
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(head);
        let mut record = csv::ByteRecord::new();
        if matches!(reader.read_byte_record(&mut record), Ok(true))
            && record.iter().any(|field| field == b"anon_dicom_path")
            && record.iter().any(|field| field == b"ROI_coords")
        {
            Confidence::Certain
        } else {
            Confidence::None
        }
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
        let resolver = RememberKeys {
            inner: context.resolver,
            keys: RefCell::new(HashMap::new()),
        };
        let EmbedRead { files, mut report } =
            read_rows_within(input, &resolver, context.keep_going, &context.limits)?;
        let keys = resolver.keys.into_inner();
        let mut fragment = Fragment::default();
        for file in files {
            let Some(Some(key)) = keys.get(&file.slot) else {
                report.warn(
                    WarningCode::KeyUnavailable,
                    RowRef {
                        row: file.row,
                        roi: None,
                    },
                );
                report.counts.refused += file.rois.roi_coords.len() as u64;
                report.counts.annotations_out -= file.rois.roi_coords.len() as u64;
                report.counts.files -= 1;
                continue;
            };
            for (i, coords) in file.rois.roi_coords.iter().enumerate() {
                fragment.annotations.push(annotation_of(
                    *coords,
                    scope_at(&file.rois.roi_frames, i),
                    key,
                    context.layer,
                    context.author,
                    context.now,
                ));
            }
        }
        Ok(Imported { fragment, report })
    }
}

fn export_io(kind: std::io::ErrorKind) -> ExportError {
    ExportError::Io {
        kind: kind.to_string(),
    }
}

fn csv_export_error(error: csv::Error) -> ExportError {
    export_io(match error.kind() {
        csv::ErrorKind::Io(error) => error.kind(),
        _ => std::io::ErrorKind::Other,
    })
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

#[derive(Clone)]
struct RowBudget {
    /// What one record may take from the input.
    allowed: u64,
    remaining: Rc<Cell<u64>>,
    exceeded: Rc<Cell<bool>>,
}

impl RowBudget {
    fn reset(&self) {
        self.remaining.set(self.allowed);
        self.exceeded.set(false);
    }
}

struct BudgetReader<'a> {
    input: &'a mut dyn Read,
    budget: RowBudget,
}

impl Read for BudgetReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.budget.remaining.get();
        if remaining == 0 {
            self.budget.exceeded.set(true);
            return Err(std::io::ErrorKind::Other.into());
        }
        let take = bytes.len().min(remaining as usize);
        let read = self.input.read(&mut bytes[..take])?;
        self.budget.remaining.set(remaining - read as u64);
        Ok(read)
    }
}

fn csv_reader<'a>(
    input: &'a mut dyn Read,
    bounds: &Limits,
) -> (csv::Reader<BudgetReader<'a>>, RowBudget) {
    let budget = RowBudget {
        allowed: bounds.csv_row_bytes + limits::CSV_READ_AHEAD_BYTES,
        remaining: Rc::new(Cell::new(0)),
        exceeded: Rc::new(Cell::new(false)),
    };
    let reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(BudgetReader {
            input,
            budget: budget.clone(),
        });
    (reader, budget)
}

struct Columns {
    path: usize,
    coords: usize,
    count: Option<usize>,
    frames: Option<usize>,
}

fn read_header(
    reader: &mut csv::Reader<BudgetReader<'_>>,
    budget: &RowBudget,
    bounds: &Limits,
) -> Result<Columns, ImportError> {
    budget.reset();
    let start = reader.position().byte();
    let headers = reader.headers().map_err(|_| {
        if budget.exceeded.get() {
            csv_row_too_large(bounds)
        } else {
            ImportError::CsvHeader
        }
    })?;
    let path = headers.iter().position(|field| field == "anon_dicom_path");
    let coords = headers.iter().position(|field| field == "ROI_coords");
    let count = headers.iter().position(|field| field == "num_ROI");
    let frames = headers.iter().position(|field| field == "ROI_frames");
    if reader.position().byte() - start > bounds.csv_row_bytes {
        return Err(csv_row_too_large(bounds));
    }
    Ok(Columns {
        path: path.ok_or(ImportError::MissingColumn {
            column: "anon_dicom_path",
        })?,
        coords: coords.ok_or(ImportError::MissingColumn {
            column: "ROI_coords",
        })?,
        count,
        frames,
    })
}

fn csv_row_too_large(bounds: &Limits) -> ImportError {
    ImportError::TooLarge {
        bound: Bound::CsvRowBytes,
        limit: bounds.csv_row_bytes,
    }
}

fn check_roi_count(count: usize, bounds: &Limits) -> Result<(), ImportError> {
    if count > bounds.rois_per_row {
        Err(ImportError::TooLarge {
            bound: Bound::RoisPerRow,
            limit: bounds.rois_per_row as u64,
        })
    } else {
        Ok(())
    }
}

struct RememberKeys<'a> {
    inner: &'a dyn FileResolver,
    keys: RefCell<HashMap<usize, Option<FileKey>>>,
}

impl FileResolver for RememberKeys<'_> {
    fn by_path(&self, path: &str, out: &mut Vec<Resolved>) {
        let start = out.len();
        self.inner.by_path(path, out);
        let mut keys = self.keys.borrow_mut();
        for target in out.iter().skip(start) {
            keys.insert(target.slot, target.key.clone());
        }
    }
}
