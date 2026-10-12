//! Every size bound the adapters enforce, as fixed numbers.
//!
//! A file handed to an importer is input nobody vouches for. Each bound
//! here is checked while the input is read, before the thing it bounds is
//! built: an importer never allocates from a count the input declares
//! (`num_ROI`, a length it has not yet read), only for what it has read.
//! Past a bound the import fails with `ImportError::TooLarge`; nothing is
//! truncated. The model's own bounds (`dcmview_annotation::limits`) apply to
//! what the adapters build.
//!
//! Raising a bound is a compatible change; lowering one is not.

/// How many bytes of the start of a file a sniff is shown, at most: 4,096.
pub const SNIFF_BYTES: usize = 4_096;

/// Longest record of an EMBED CSV, in bytes: 16,777,216 (16 MiB), the
/// model's bound on one operation envelope. A record is one row, however
/// many lines its quoted fields span, counted with the line terminator
/// that ends it; the header is a record too.
pub const MAX_CSV_ROW_BYTES: u64 = 16_777_216;

/// How far past [`MAX_CSV_ROW_BYTES`] the reader may have read when it
/// refuses a record, in bytes: 65,536. The CSV parser reads ahead in blocks,
/// so the bytes consumed for one record are counted to this slack.
pub const CSV_READ_AHEAD_BYTES: u64 = 65_536;

/// Most records an EMBED CSV may hold after its header: 16,777,216.
pub const MAX_CSV_ROWS: u64 = 16_777_216;

/// Most ROIs one EMBED row may hold: 65,536.
pub const MAX_ROIS_PER_ROW: usize = 65_536;

/// Most frame indices one ROI's frame list may hold: 65,536, the model's
/// bound on a frame set (`dcmview_annotation::limits::MAX_FRAMES_IN_SET`).
pub const MAX_FRAMES_PER_ROI: usize = dcmview_annotation::limits::MAX_FRAMES_IN_SET;

/// Longest `anon_dicom_path` that can name a file, in bytes: 4,096
/// (`dcmview_annotation::limits::MAX_PATH_BYTES`). A longer one names no
/// file; its row is unmatched, not an error.
pub const MAX_CSV_PATH_BYTES: usize = dcmview_annotation::limits::MAX_PATH_BYTES;

/// Largest native JSON document, and longest first line of a native JSON
/// Lines file, in bytes: 268,435,456 (256 MiB), the model's bound on a
/// document (`dcmview_annotation::limits::MAX_DOCUMENT_BYTES`).
pub const MAX_DOCUMENT_BYTES: u64 = dcmview_annotation::limits::MAX_DOCUMENT_BYTES as u64;

/// Longest record line of a native JSON Lines file, in bytes, its line feed
/// not counted: 16,777,216 (16 MiB).
pub const MAX_LINE_BYTES: u64 = 16_777_216;

/// Most files (`FileRef`s) one native import reads: 1,048,576.
pub const MAX_FILES: usize = 1_048_576;

/// Most annotations and labels together one native import reads: 1,048,576.
pub const MAX_RECORDS: usize = 1_048_576;

/// Most JSON values one unit of a native document may hold: 1,048,576. A
/// unit is one record (a file, layer, annotation or label), or everything
/// of a document that is not one of its records (its header). A value is
/// an object, an array, a string (a member's name included), a number,
/// `true`, `false` or `null`. The largest records the model allows are a
/// mask of 262,144 tiles in 65,536 frames, some 720,000 values, and a
/// polygon of 100,000 points, some 500,000.
pub const MAX_UNIT_VALUES: u64 = 1_048_576;

/// Most objects and arrays one unit may hold: 131,072. A polygon of
/// 100,000 points has 100,003 and the largest mask 65,540. Objects and
/// arrays are what costs most to hold as read, so they have a bound of
/// their own beside [`MAX_UNIT_VALUES`]; together the two bound what
/// building one unit holds ([`UNIT_TRANSIENT_BYTES`]).
pub const MAX_UNIT_CONTAINERS: u64 = 131_072;

/// What building one unit may hold beside what is already counted, for
/// any text: 256 MiB. A unit is in the parser's buffered form and in the
/// form it is kept in at once, each value in vectors that may be twice
/// as large as they need. Measured at the bounds, a number or a string
/// costs up to 130 bytes and an object of one member 960, most of it
/// the map node it is kept in. 131,072 such objects (393,216 values with
/// their members) are 120 MiB and the 655,360 values left to the bound
/// 81 MiB: 201 MiB, and 203 MiB as measured for that unit, the costliest
/// found (`tests/raster_cost/annotations.rs` holds each shape to this).
pub const UNIT_TRANSIENT_BYTES: u64 = 256 * 1024 * 1024;

/// Deepest nesting of arrays and objects in a native document: 64.
pub const MAX_DEPTH: u32 = 64;

/// What one native import may be charged for what it keeps, in bytes:
/// 4,294,967,296 (4 GiB). The charge is counted on the records as they are
/// built ([`crate::held`]), not predicted from the text, and does not
/// depend on the text's length: a document of small maps and short
/// strings costs many times its text, and a session that holds one must
/// still be able to write it and read it back. An export is held to the
/// same number by the same count, so what is written is read.
///
/// The number comes from the bounds beside it. The largest session of the
/// common kind those allow, [`MAX_RECORDS`] rectangles on [`MAX_FILES`]
/// files, is charged 3.1 GiB (1,889 bytes a rectangle and 1,283 a file
/// reference as the viewer writes them), so every such session exports
/// and reloads with room to spare, and nothing much larger than the
/// largest of them is held for any text. With the text itself (a JSON
/// document of [`MAX_DOCUMENT_BYTES`] in a buffer of up to three times
/// that) and [`UNIT_TRANSIENT_BYTES`], an import holds at most 5 GiB,
/// from the file to the store.
pub const MAX_HELD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// How many row references one entry of a `Report` keeps: the first 16. The
/// entry's `count` still counts every occurrence.
pub const MAX_REPORT_ROWS: usize = 16;

/// The bounds an import or an export runs within, as one value a caller
/// hands over. [`Limits::DEFAULT`] is every constant of this module, and is
/// what the viewer uses; a test hands over smaller ones, so that a bound
/// is met with an input of a few lines.
///
/// An error that reports a bound gives the number that applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// [`MAX_CSV_ROW_BYTES`].
    pub csv_row_bytes: u64,
    /// [`MAX_CSV_ROWS`].
    pub csv_rows: u64,
    /// [`MAX_ROIS_PER_ROW`].
    pub rois_per_row: usize,
    /// [`MAX_FRAMES_PER_ROI`].
    pub frames_per_roi: usize,
    /// [`MAX_CSV_PATH_BYTES`].
    pub csv_path_bytes: usize,
    /// [`MAX_DOCUMENT_BYTES`].
    pub document_bytes: u64,
    /// [`MAX_LINE_BYTES`].
    pub line_bytes: u64,
    /// [`MAX_FILES`].
    pub files: usize,
    /// [`MAX_RECORDS`].
    pub records: usize,
    /// [`MAX_UNIT_VALUES`].
    pub unit_values: u64,
    /// [`MAX_DEPTH`].
    pub depth: u32,
    /// [`MAX_UNIT_CONTAINERS`].
    pub unit_containers: u64,
    /// [`MAX_HELD_BYTES`].
    pub held_bytes: u64,
}

impl Limits {
    /// The constants of this module.
    pub const DEFAULT: Limits = Limits {
        csv_row_bytes: MAX_CSV_ROW_BYTES,
        csv_rows: MAX_CSV_ROWS,
        rois_per_row: MAX_ROIS_PER_ROW,
        frames_per_roi: MAX_FRAMES_PER_ROI,
        csv_path_bytes: MAX_CSV_PATH_BYTES,
        document_bytes: MAX_DOCUMENT_BYTES,
        line_bytes: MAX_LINE_BYTES,
        files: MAX_FILES,
        records: MAX_RECORDS,
        unit_values: MAX_UNIT_VALUES,
        depth: MAX_DEPTH,
        unit_containers: MAX_UNIT_CONTAINERS,
        held_bytes: MAX_HELD_BYTES,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
