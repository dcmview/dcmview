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

/// How many row references one entry of a `Report` keeps: the first 16. The
/// entry's `count` still counts every occurrence.
pub const MAX_REPORT_ROWS: usize = 16;
