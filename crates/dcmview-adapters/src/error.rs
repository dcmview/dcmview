//! Why an import or an export failed.
//!
//! An error ends the whole import or export: nothing of it is handed on.
//! Like a [`crate::Report`], an error is fixed words and numbers. Its text
//! never holds text of the input: not a path, a key, a name, nor the
//! message of a parser that quotes what it read. The numbers are row
//! numbers, positions, bounds and, where a variant has one, a count or a
//! frame index the file states (`num_ROI`, an entry of `ROI_frames`), which
//! are whole numbers that parsed and nothing else. So an error can be
//! shown, logged and sent in a session that masks identifiers.

use dcmview_annotation::ViolationCode;

/// Which bound an input went past ([`crate::limits`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bound {
    /// [`crate::limits::MAX_CSV_ROW_BYTES`].
    CsvRowBytes,
    /// [`crate::limits::MAX_CSV_ROWS`].
    CsvRows,
    /// [`crate::limits::MAX_ROIS_PER_ROW`].
    RoisPerRow,
    /// [`crate::limits::MAX_FRAMES_PER_ROI`].
    FramesPerRoi,
    /// [`crate::limits::MAX_DOCUMENT_BYTES`].
    DocumentBytes,
    /// [`crate::limits::MAX_LINE_BYTES`].
    LineBytes,
    /// [`crate::limits::MAX_FILES`].
    Files,
    /// [`crate::limits::MAX_RECORDS`].
    Records,
    /// [`crate::limits::MAX_UNIT_VALUES`].
    UnitValues,
    /// [`crate::limits::MAX_UNIT_CONTAINERS`].
    UnitContainers,
    /// [`crate::limits::MAX_DEPTH`].
    Depth,
    /// [`crate::limits::MAX_HELD_BYTES`]: what the document's records are
    /// charged for the memory they hold.
    HeldBytes,
}

impl std::fmt::Display for Bound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CsvRowBytes => "the bytes of one CSV row",
            Self::CsvRows => "the rows of a CSV",
            Self::RoisPerRow => "the ROIs of one row",
            Self::FramesPerRoi => "the frames of one ROI",
            Self::DocumentBytes => "the bytes of a JSON document or of a first line",
            Self::LineBytes => "the bytes of one record line",
            Self::Files => "the files of a document",
            Self::Records => "the annotations and labels of a document",
            Self::UnitValues => "the JSON values of one record or of a document's header",
            Self::UnitContainers => {
                "the objects and arrays of one record or of a document's header"
            }
            Self::Depth => "the levels its values nest",
            Self::HeldBytes => "the bytes of memory its records hold",
        })
    }
}

/// Why an import failed. Nothing was handed on.
///
/// `row` is a [`crate::RowRef::row`]. The messages of the EMBED variants
/// are the ones the `--annotations` import has always given, without the
/// parser's own text after them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    /// The input could not be read. `kind` is the text of the
    /// `std::io::ErrorKind`, which names no file.
    #[error("the annotations file could not be read: {kind}")]
    Io { kind: String },
    /// The input is past a fixed bound.
    #[error(
        "the annotations file is larger than this version reads: {bound} are limited to {limit}"
    )]
    TooLarge { bound: Bound, limit: u64 },
    /// The caller asked the import to stop (`keep_going` answered `false`).
    #[error("annotation loading cancelled")]
    Cancelled,

    /// The EMBED CSV has no header that can be read.
    #[error("failed to read annotations CSV header")]
    CsvHeader,
    /// The EMBED CSV's header lacks `anon_dicom_path` or `ROI_coords`.
    #[error("annotations CSV missing required column `{column}`")]
    MissingColumn { column: &'static str },
    /// A record is not CSV, or has another number of fields than the
    /// header.
    #[error("annotations CSV row {row} could not be parsed")]
    CsvRow { row: u64 },
    #[error("annotations CSV row {row}: anon_dicom_path must not be empty")]
    EmptyPath { row: u64 },
    /// `ROI_coords` is not a JSON list of lists of numbers.
    #[error("annotations CSV row {row}: ROI_coords must be a JSON list of [ymin, xmin, ymax, xmax] arrays")]
    RoiCoords { row: u64 },
    /// One box has other than four numbers.
    #[error("annotations CSV row {row}: ROI_coords[{roi}] must contain exactly 4 integers [ymin, xmin, ymax, xmax]")]
    RoiCoordsEntry { row: u64, roi: usize },
    /// One box has a number that is above 4,294,967,295 once corrected.
    #[error("annotations CSV row {row}: ROI_coords[{roi}] holds a coordinate above 4294967295")]
    CoordinateTooLarge { row: u64, roi: usize },
    #[error("annotations CSV row {row}: num_ROI must be an integer")]
    NumRoi { row: u64 },
    #[error(
        "annotations CSV row {row}: len(ROI_coords) must equal num_ROI ({coords} != {num_roi})"
    )]
    NumRoiMismatch {
        row: u64,
        coords: usize,
        num_roi: usize,
    },
    #[error("annotations CSV row {row}: ROI_frames must be a JSON list of frame-index lists")]
    RoiFrames { row: u64 },
    #[error("annotations CSV row {row}: len(ROI_frames) must equal num_ROI when ROI_frames is not empty ({frames} != {num_roi})")]
    RoiFramesLength {
        row: u64,
        frames: usize,
        num_roi: usize,
    },
    #[error("annotations CSV row {row}: ROI_frames[{roi}] contains frame {frame}, but matched DICOM has {frame_count} frame(s)")]
    FrameOutOfRange {
        row: u64,
        roi: usize,
        frame: u32,
        frame_count: u32,
    },
    #[error("annotations CSV row {row}: duplicate matching anon_dicom_path for loaded file (already matched at row {previous_row})")]
    DuplicatePath { row: u64, previous_row: u64 },

    /// A native document, a first line or a record line is not JSON of the
    /// expected shape; or a JSON Lines first line holds records of its own.
    #[error("annotations record {row} is not JSON of the dcmview.annotations format")]
    Malformed { row: u64 },
    /// A JSON Lines file does not end with its last line: it was cut off,
    /// or records were added after it was written.
    #[error("the annotations file is incomplete: its last line is missing or does not count its records")]
    Truncated,
    /// The `format` member is not `dcmview.annotations`.
    #[error("the annotations file is not a dcmview.annotations document")]
    WrongFormat,
    /// The `version` member is not one this version reads.
    #[error("the annotations file has a dcmview.annotations version this version does not read")]
    UnsupportedVersion,
    /// A file, layer, annotation or label breaks a rule of the annotation
    /// model. `code` is the first violation found.
    #[error("annotations record {row} breaks a rule of the annotation model: {code:?}")]
    Invalid { row: u64, code: ViolationCode },
}

/// Why an export failed. What was written so far is not a valid output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    /// The output could not be written. `kind` is the text of the
    /// `std::io::ErrorKind`.
    #[error("the export could not be written: {kind}")]
    Io { kind: String },
    /// The output would be longer than the context or the format allows.
    #[error("the export is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    /// The document is past a bound an import holds it to: its files or
    /// records, the memory its records are charged, or the values, the
    /// objects and arrays, the depth or the line length of one unit
    /// ([`crate::native`], "Cost"). What was written is not a document.
    #[error("the export holds more than {limit} of {bound}")]
    TooMany { bound: Bound, limit: u64 },
    /// The document cannot be written in this format: an annotation names
    /// a file the document has no `FileRef` for.
    #[error("the document names a file it does not describe")]
    UnknownFile,
}
