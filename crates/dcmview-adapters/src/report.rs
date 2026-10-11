//! What an import or an export did, for a person and for a program
//! (`docs/design/output-adapters.md` 9.3).
//!
//! Every adapter answers with a [`Report`]: an import or an export is never
//! silent about what it left out, changed or could not place.
//!
//! # A report holds no text of the input
//!
//! A report is shown to whoever is at the screen, written to logs and sent
//! over HTTP, also in a session that masks identifiers. So it is made of
//! fixed words, numbers and row references, and nothing else:
//!
//! - codes are closed enums with stable wire strings;
//! - [`Loss::detail`] is the name of a geometry type or absent;
//! - a row is named by its number ([`RowRef`]), never by a path, a file
//!   key, a UID, a record id or any other value the input held;
//! - [`AdapterRef::id`] is the id of a registered adapter.
//!
//! Its size is bounded whatever the input: one entry per code (per code and
//! detail for a loss), and at most [`crate::limits::MAX_REPORT_ROWS`] row
//! references in an entry.

use dcmview_annotation::GeometryType;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which adapter wrote a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AdapterRef {
    pub id: String,
    pub version: u32,
}

/// Whether a report is of an import or of an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Import,
    Export,
}

/// One place in an input, by number.
///
/// - EMBED CSV: `row` is the record's number in the file, the header being
///   row 1, so the first data row is 2 (the number the import's error
///   messages use). `roi` is the position of a ROI in that row's
///   `ROI_coords`, from 0, when the entry is about one ROI.
/// - Native JSON Lines: `row` is the line number, from 1 (the header line).
/// - Native JSON document: `row` is the line the record would be on in the
///   JSON Lines form of the same document: 1 for the document itself, then
///   every file, layer, annotation and label, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct RowRef {
    pub row: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub roi: Option<u32>,
}

/// What a report counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct Counts {
    /// Import: the loaded files that were given records. Export: the files
    /// written (for EMBED, rows).
    pub files: u64,
    /// Annotations read from the input (import; for EMBED, the ROIs of the
    /// rows that matched a file) or held by the document (export).
    pub annotations_in: u64,
    /// Annotations handed on (import) or written (export).
    pub annotations_out: u64,
    pub labels_in: u64,
    pub labels_out: u64,
    /// Import: what named no loaded file. For EMBED, rows; for the native
    /// formats, annotations and labels.
    pub unmatched: u64,
    /// Import: annotations and labels of a file that matched and was
    /// refused (see the refusal codes of [`WarningCode`]).
    pub refused: u64,
}

/// Something an export could not carry, or an import could not keep.
/// Variants are only added; the wire strings are the snake-case names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LossCode {
    /// Annotations of a geometry type the format cannot hold were left out.
    /// `detail` names the type; one entry per type.
    TypeSkipped,
    /// Rectangles off the pixel grid were written rounded outward.
    RoundedOutward,
    /// Annotations of more than one class were written into a format that
    /// has none. `count` is the number of classes.
    ClassesMerged,
    /// Annotations that carried attributes were written without them.
    AttributesDropped,
    /// Annotations that carried a score were written without it.
    ScoresDropped,
    /// Labels were not written.
    LabelsDropped,
    /// Imported labels lost members a label operation cannot carry
    /// (`extensions`, unknown members, `derived_from`, `score`).
    LabelMembersDropped,
}

/// Something a reader of the result should know. Variants are only added;
/// the wire strings are the snake-case names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    /// EMBED import: a box had a negative coordinate, which was read as 0.
    NegativeCoordinate,
    /// EMBED import: a box had a coordinate that is not a whole number,
    /// which was rounded to the nearest pixel.
    NonIntegerCoordinate,
    /// A box reaches past the image edge. It is kept as read.
    OutOfRange,
    /// A box has no area. It is kept as read.
    ZeroArea,
    /// A box has its corners out of order. It is kept as read.
    CornersOutOfOrder,
    /// A box has an explicit frame list that is empty. It is kept as read.
    FramesEmpty,
    /// Native import: a file of the document matched no loaded file. Its
    /// annotations and labels are counted in [`Counts::unmatched`].
    FileUnmatched,
    /// Native import, a refusal: a file of the document matched a loaded
    /// file whose rows, columns or frame count differ.
    SizeMismatch,
    /// Native import, a refusal: a file of the document matched a loaded
    /// file that contradicts its SOP Instance UID or its digest.
    EvidenceConflict,
    /// Native import, a refusal: a file of the document matched a loaded
    /// file that has no key and cannot be given one.
    KeyUnavailable,
    /// Native import: a file of the document matched a loaded file of
    /// another size in bytes. It is still matched.
    FileSizeDiffers,
    /// Native import: the document carries a label schema, which was not
    /// taken; the records were checked against the schema in force.
    SchemaNotApplied,
    /// Native import, a refusal: a file of the document names its file by
    /// content, and the loaded file found for it could not be read to
    /// compare the two.
    FileUnverified,
}

impl WarningCode {
    /// The wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NegativeCoordinate => "negative_coordinate",
            Self::NonIntegerCoordinate => "non_integer_coordinate",
            Self::OutOfRange => "out_of_range",
            Self::ZeroArea => "zero_area",
            Self::CornersOutOfOrder => "corners_out_of_order",
            Self::FramesEmpty => "frames_empty",
            Self::FileUnmatched => "file_unmatched",
            Self::SizeMismatch => "size_mismatch",
            Self::EvidenceConflict => "evidence_conflict",
            Self::KeyUnavailable => "key_unavailable",
            Self::FileSizeDiffers => "file_size_differs",
            Self::SchemaNotApplied => "schema_not_applied",
            Self::FileUnverified => "file_unverified",
        }
    }
}

/// One kind of loss, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Loss {
    pub code: LossCode,
    /// The snake-case name of a geometry type (`polygon`), or absent.
    /// Never text of the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    /// How many times it happened. What is counted is in the code's
    /// documentation; where that says nothing, annotations.
    pub count: u64,
    /// The first places it happened, at most
    /// [`crate::limits::MAX_REPORT_ROWS`], in the order they were met.
    /// Empty for an export.
    #[serde(default)]
    pub rows: Vec<RowRef>,
}

/// One kind of warning, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Warning {
    pub code: WarningCode,
    /// How many boxes, files or documents it applies to: one for each box
    /// for the six codes about a box (a box with two bad coordinates counts
    /// once), one for each file of the document for the six about a file,
    /// and 1 for `schema_not_applied`.
    pub count: u64,
    /// The first places it happened, at most
    /// [`crate::limits::MAX_REPORT_ROWS`], in the order they were met.
    #[serde(default)]
    pub rows: Vec<RowRef>,
}

/// How the files an import placed were found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct MatchedBy {
    pub key: u64,
    pub sop_instance_uid: u64,
    /// For EMBED, the rows whose path named a loaded file.
    pub path: u64,
}

/// What one import or export did.
///
/// ```json
/// { "adapter": { "id": "embed", "version": 1 }, "direction": "import",
///   "counts": { "files": 2, "annotations_in": 3, "annotations_out": 3,
///               "labels_in": 0, "labels_out": 0, "unmatched": 1, "refused": 0 },
///   "losses": [],
///   "warnings": [ { "code": "negative_coordinate", "count": 1,
///                   "rows": [ { "row": 3, "roi": 0 } ] } ],
///   "matched_by": { "key": 0, "sop_instance_uid": 0, "path": 2 } }
/// ```
///
/// `losses` and `warnings` hold one entry per code (per code and `detail`
/// for a loss), in the order each was first met. `matched_by` is present
/// for an import and absent for an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Report {
    pub adapter: AdapterRef,
    pub direction: Direction,
    pub counts: Counts,
    #[serde(default)]
    pub losses: Vec<Loss>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub matched_by: Option<MatchedBy>,
}

impl Report {
    /// An empty report: zero counts, no loss, no warning, and `matched_by`
    /// all zero for an import and absent for an export.
    pub fn new(adapter: AdapterRef, direction: Direction) -> Self {
        Self {
            adapter,
            direction,
            counts: Counts::default(),
            losses: Vec::new(),
            warnings: Vec::new(),
            matched_by: (direction == Direction::Import).then(MatchedBy::default),
        }
    }

    /// Counts one more occurrence of `code`, at `at`.
    ///
    /// The entry for `code` is the one already there, or a new one at the
    /// end of `warnings`. Its `count` rises by one; `at` is added to its
    /// `rows` while it holds fewer than
    /// [`crate::limits::MAX_REPORT_ROWS`].
    pub fn warn(&mut self, code: WarningCode, at: RowRef) {
        let warning = match self.warnings.iter_mut().find(|entry| entry.code == code) {
            Some(warning) => warning,
            None => {
                self.warnings.push(Warning {
                    code,
                    count: 0,
                    rows: vec![],
                });
                let Some(warning) = self.warnings.last_mut() else {
                    return;
                };
                warning
            }
        };
        warning.count += 1;
        if warning.rows.len() < crate::limits::MAX_REPORT_ROWS {
            warning.rows.push(at);
        }
    }

    /// Counts `count` more occurrences of the loss `code` for `detail`
    /// (a geometry type, written as its snake-case name), at `at` when the
    /// loss has a place.
    ///
    /// The entry for this code and detail is the one already there, or a
    /// new one at the end of `losses`. Its `count` rises by `count`; `at`,
    /// when given, is added to its `rows` while it holds fewer than
    /// [`crate::limits::MAX_REPORT_ROWS`]. A `count` of 0 adds nothing and
    /// makes no entry.
    pub fn lose(
        &mut self,
        code: LossCode,
        detail: Option<GeometryType>,
        count: u64,
        at: Option<RowRef>,
    ) {
        if count == 0 {
            return;
        }
        let detail = detail.map(|kind| {
            match kind {
                GeometryType::Point => "point",
                GeometryType::Line => "line",
                GeometryType::Polyline => "polyline",
                GeometryType::Polygon => "polygon",
                GeometryType::Rect => "rect",
                GeometryType::Ellipse => "ellipse",
                GeometryType::Mask => "mask",
            }
            .to_string()
        });
        let loss = match self
            .losses
            .iter_mut()
            .find(|entry| entry.code == code && entry.detail == detail)
        {
            Some(loss) => loss,
            None => {
                self.losses.push(Loss {
                    code,
                    detail,
                    count: 0,
                    rows: vec![],
                });
                let Some(loss) = self.losses.last_mut() else {
                    return;
                };
                loss
            }
        };
        loss.count += count;
        if let Some(at) = at {
            if loss.rows.len() < crate::limits::MAX_REPORT_ROWS {
                loss.rows.push(at);
            }
        }
    }
}
