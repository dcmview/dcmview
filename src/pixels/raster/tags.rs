//! The metadata tree of a raster image file: what the Metadata panel shows
//! for a PNG, JPEG, TIFF or WebP file (`docs/design/image-formats.md`
//! section 8).
//!
//! Raster metadata is input nobody vouches for, and it is where a file
//! carries personal data: EXIF GPS positions, device serial numbers, owner
//! and artist names, dates, comments, XMP. So this module is built so that
//! the two things that matter do not depend on a parser getting them right:
//!
//! - **What reading may cost** is enforced around the format readers, not by
//!   them: every byte comes through the one budgeted reader of
//!   `pixels/raster`, behind a source that counts reads, and every node
//!   enters the tree through [`TagSink`], which holds the node, depth and
//!   text limits.
//! - **What a value may contain** is decided by [`TagSink`] alone: it decodes
//!   and escapes every piece of text, and a format reader has no way to put
//!   a string of its own making into a node's `tag`, `vr` or `keyword`.
//!
//! [`read_raster_tags`] is the seam and its doc comment is the contract.
//! `src/masking.rs` decides what of the tree a masked session shows.

mod names;

use super::reader::Reader;
use super::RasterSource;
use crate::api::contracts::{FileFormat, TagNode, TagValue};
use crate::types::FileEntry;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

/// The most bytes reading one file's metadata is handed from the file,
/// charged as the pixel reader charges them: every byte a read returns,
/// read-ahead included, and every buffered byte handed out again.
pub const RASTER_TAGS_READ_BUDGET_BYTES: u64 = 4 * 1024 * 1024;

/// The most reads reading one file's metadata issues to the file. A PNG
/// written in 8 KiB `IDAT` chunks needs one small read per chunk to step
/// over its image data; 16,384 of them cover 128 MiB of such a file.
pub const RASTER_TAGS_MAX_READS: u64 = 16_384;

/// The most nodes [`read_raster_tags`] returns for one file, groups
/// included, at every depth together.
pub const RASTER_TAGS_MAX_NODES: usize = 1024;

/// The deepest a node lies: a top-level group, a group inside it, and that
/// group's entries.
pub const RASTER_TAGS_MAX_DEPTH: usize = 3;

/// The most characters of one text value that are shown; a longer value ends
/// with `…` after them.
pub const RASTER_TAG_TEXT_MAX_CHARS: usize = 1024;

/// The most bytes of text, as UTF-8, all the values of one file's tree hold
/// together (the `…` that ends a cut value is not counted).
pub const RASTER_TAGS_MAX_TEXT_BYTES: usize = 128 * 1024;

/// The most numbers one value shows; a longer one says how many it has.
pub const RASTER_TAG_NUMBERS_MAX: usize = 64;

/// The most bytes of one value that are read from the file to show it:
/// enough for [`RASTER_TAG_TEXT_MAX_CHARS`] characters of four bytes each,
/// or [`RASTER_TAG_NUMBERS_MAX`] values of 16 bytes.
pub const RASTER_TAG_VALUE_MAX_BYTES: u64 = 4096;

/// The most entries of one TIFF or EXIF directory that are shown.
pub const RASTER_TAGS_MAX_IFD_ENTRIES: usize = 256;

/// The most TIFF pages whose tags are shown, in the order the file chains
/// them.
pub const RASTER_TAGS_MAX_PAGES: usize = 16;

/// The most directories read in one file: for each page shown, the page and
/// its EXIF, GPS and interoperability directories.
pub const RASTER_TAGS_MAX_IFDS: usize = 4 * RASTER_TAGS_MAX_PAGES;

/// The most bytes one compressed stream (a PNG `zTXt` or `iTXt` text, a PNG
/// `iCCP` profile) is inflated to. What it would inflate to beyond that is
/// neither produced nor read.
pub const RASTER_TAGS_INFLATE_MAX_BYTES: usize = 64 * 1024;

/// How much of an ICC profile is looked at, from its start: its header, its
/// tag table and a description that lies within these bytes.
pub const RASTER_TAGS_ICC_HEAD_BYTES: usize = 64 * 1024;

/// The most heap one call of [`read_raster_tags`] holds at once, the tree it
/// returns included, whatever the file: the read buffer, an inflated
/// stream, the profile head, and the tree at its node and text limits.
pub const RASTER_TAGS_HEAP_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

/// What [`read_raster_tags`] found: the tree, and what a reader of it should
/// know about how complete it is.
#[derive(Debug, Clone)]
pub struct RasterTagTree {
    /// At most [`RASTER_TAGS_MAX_NODES`] nodes in all, in file order within
    /// each part; the layout is in [`read_raster_tags`].
    pub nodes: Vec<TagNode>,
    /// Each note at most once, in the order first met. Empty for a
    /// well-formed file within every limit.
    pub notes: Vec<RasterTagNote>,
}

/// Why a metadata tree is not all of a file's metadata. A note names a part
/// or a limit and never holds a byte of the file, so it can be shown in a
/// masked session and written to a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RasterTagNote {
    /// The part is damaged, cut short, loops back on itself or points
    /// outside the file. What could be read of it, and everything else, is
    /// shown.
    Damaged(RasterTagPart),
    /// A fixed limit was reached, so the file holds more than is shown.
    Limit(RasterTagLimit),
    /// The reader failed where it should have reported damage: a defect in
    /// dcmview, not a property of the file. What was read before is shown.
    ReaderFailed,
}

/// A part of a raster file's metadata, for [`RasterTagNote::Damaged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RasterTagPart {
    /// The file's own structure: PNG chunks, JPEG segments, WebP chunks, the
    /// TIFF header and its pages.
    Container,
    /// An EXIF block, or a TIFF page's EXIF, GPS or interoperability
    /// directory.
    Exif,
    /// The ICC profile.
    Icc,
    /// An XMP packet.
    Xmp,
    /// A PNG text chunk.
    Text,
}

/// A limit of [`read_raster_tags`], for [`RasterTagNote::Limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RasterTagLimit {
    /// [`RASTER_TAGS_READ_BUDGET_BYTES`].
    Bytes,
    /// [`RASTER_TAGS_MAX_READS`].
    Reads,
    /// [`RASTER_TAGS_MAX_NODES`].
    Nodes,
    /// [`RASTER_TAGS_MAX_DEPTH`].
    Depth,
    /// [`RASTER_TAGS_MAX_TEXT_BYTES`].
    Text,
    /// [`RASTER_TAGS_MAX_IFD_ENTRIES`].
    Entries,
    /// [`RASTER_TAGS_MAX_IFDS`].
    Directories,
    /// [`RASTER_TAGS_MAX_PAGES`].
    Pages,
    /// [`RASTER_TAGS_INFLATE_MAX_BYTES`] or [`RASTER_TAGS_ICC_HEAD_BYTES`].
    Inflate,
}

impl fmt::Display for RasterTagNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Damaged(part) => {
                let part = match part {
                    RasterTagPart::Container => "the file's structure",
                    RasterTagPart::Exif => "the EXIF data",
                    RasterTagPart::Icc => "the ICC profile",
                    RasterTagPart::Xmp => "the XMP packet",
                    RasterTagPart::Text => "a text chunk",
                };
                write!(
                    f,
                    "{part} is damaged or cut short; what could be read is shown"
                )
            }
            Self::Limit(limit) => {
                let (what, number) = match limit {
                    RasterTagLimit::Bytes => (
                        "bytes of metadata are read from one file",
                        RASTER_TAGS_READ_BUDGET_BYTES,
                    ),
                    RasterTagLimit::Reads => (
                        "reads are made for one file's metadata",
                        RASTER_TAGS_MAX_READS,
                    ),
                    RasterTagLimit::Nodes => (
                        "entries are shown for one file",
                        RASTER_TAGS_MAX_NODES as u64,
                    ),
                    RasterTagLimit::Depth => {
                        ("levels of entries are shown", RASTER_TAGS_MAX_DEPTH as u64)
                    }
                    RasterTagLimit::Text => (
                        "bytes of text are shown for one file",
                        RASTER_TAGS_MAX_TEXT_BYTES as u64,
                    ),
                    RasterTagLimit::Entries => (
                        "entries of one directory are shown",
                        RASTER_TAGS_MAX_IFD_ENTRIES as u64,
                    ),
                    RasterTagLimit::Directories => (
                        "directories are read in one file",
                        RASTER_TAGS_MAX_IFDS as u64,
                    ),
                    RasterTagLimit::Pages => {
                        ("pages have their tags shown", RASTER_TAGS_MAX_PAGES as u64)
                    }
                    RasterTagLimit::Inflate => (
                        "bytes of a compressed chunk or of a profile are looked at",
                        RASTER_TAGS_INFLATE_MAX_BYTES as u64,
                    ),
                };
                write!(f, "more metadata than is shown: at most {number} {what}")
            }
            Self::ReaderFailed => {
                f.write_str("the metadata reader failed on this file; what it read before is shown")
            }
        }
    }
}

/// Reads the metadata tree of a raster file of `format` from `source`, which
/// holds the file's `length` bytes.
///
/// `format` is the catalog's format of the file. Nothing else of the catalog
/// entry is given: no size, count or offset of the tree comes from what
/// discovery read, and none from what the file declares without a check
/// against a constant here and against `length`. `source` is the only thing
/// read; nothing is opened, logged or written to stderr.
///
/// # Outcomes
///
/// There is one: a tree and its notes. It never fails and never panics,
/// whatever `source` holds. A file that is not what `format` says, is cut
/// short, lies about a length or loops gives the nodes that could be read
/// before the damage and after it, and a [`RasterTagNote::Damaged`] naming
/// the part. A limit below that is reached gives what was read up to it and
/// a [`RasterTagNote::Limit`]. A well-formed file within the limits gives no
/// notes. [`RasterTagNote::ReaderFailed`] reports a panic caught here; a
/// reader that produces it has a defect, however hostile the file.
///
/// # What a read may cost
///
/// Every number is a constant of this module, the same for every file.
///
/// - **Bytes.** At most [`RASTER_TAGS_READ_BUDGET_BYTES`] handed out by the
///   reader, charged as `decode_raster_frame` states ("Bytes read"). A read
///   that would pass it is not issued.
/// - **Reads.** At most [`RASTER_TAGS_MAX_READS`] issued to `source`. A read
///   past it is refused before it reaches `source`.
/// - **Pixels are never read.** A PNG's `IDAT` payloads, a JPEG's
///   entropy-coded data, a WebP's image chunks and a TIFF's strips and tiles
///   are stepped over by seeking, which costs nothing: only chunk and
///   segment headers, directories and the values shown are read. A walk
///   names the bytes it needs (`Reader::read_span`) where a whole buffer of
///   read-ahead would fetch image data instead. A JPEG is walked to its
///   first start-of-scan marker and no further.
/// - **Nodes.** At most [`RASTER_TAGS_MAX_NODES`], at most
///   [`RASTER_TAGS_MAX_DEPTH`] deep.
/// - **Text.** One value shows at most [`RASTER_TAG_TEXT_MAX_CHARS`]
///   characters and the tree at most [`RASTER_TAGS_MAX_TEXT_BYTES`] bytes of
///   text; at most [`RASTER_TAG_VALUE_MAX_BYTES`] bytes of one value are
///   read to show it. A numeric value shows at most
///   [`RASTER_TAG_NUMBERS_MAX`] numbers and states its count.
/// - **Directories.** At most [`RASTER_TAGS_MAX_IFD_ENTRIES`] entries of a
///   directory are shown, whatever count it declares: its entry table is
///   read for those and no further. At most [`RASTER_TAGS_MAX_PAGES`] TIFF
///   pages and [`RASTER_TAGS_MAX_IFDS`] directories are read. A directory
///   reached twice, by any path, is not read again, so a chain or pointer
///   that loops ends there with a `Damaged` note. The only pointers
///   followed are a TIFF's page chain, and from an image directory tags
///   34665 (EXIF) and 34853 (GPS), from an EXIF directory tag 40965
///   (interoperability), and in an EXIF block the link from `IFD0` to
///   `IFD1`. Nothing in an EXIF, GPS or interoperability directory leads
///   further, so no file nests deeper than the tree's depth.
/// - **Offsets and counts.** An offset is used only when what it names lies
///   inside the file (inside its block, for an EXIF block): compared in
///   `u64`, without overflow, before anything is read. A value whose bytes
///   do not is shown as a problem and the directory goes on.
/// - **Compressed data.** A `zTXt`, compressed `iTXt` or `iCCP` stream is
///   inflated to at most [`RASTER_TAGS_INFLATE_MAX_BYTES`] and read only as
///   far as that takes. A stream that would inflate to more is cut there
///   (text ends with `…`).
/// - **Profiles.** Only the first [`RASTER_TAGS_ICC_HEAD_BYTES`] of a
///   profile are read; in a JPEG, only the first `APP2` profile segment with
///   sequence number 1.
/// - **Memory.** At most [`RASTER_TAGS_HEAP_LIMIT_BYTES`] of heap at once on
///   the calling thread, the returned tree included.
/// - **No decode permit** and no pixel decode: this runs beside frame
///   decodes, not through `decode_scheduler`, and there is no wall-clock
///   limit.
///
/// # The tree
///
/// A node is a *leaf* (a value) or a *group* (`TagValue::Sequence` with one
/// item: its children). Its `tag` is a name from the tables below or a tag
/// number, its `keyword` a name from a table of this module, and its `vr`
/// the TIFF value type of a directory entry (`ASCII`, `SHORT`, `RATIONAL`,
/// ...) and empty otherwise. None of the three ever holds bytes of the
/// file; only a value does.
///
/// Top level, in this order (a part the file lacks is absent):
///
/// 1. the container's own leaves, in file order;
/// 2. `EXIF`, a group (PNG `eXIf`, JPEG `APP1` `Exif`, WebP `EXIF`: the
///    first of each), with the groups `IFD0`, `Exif`, `GPS`, `Interop` and
///    `IFD1` in that order, each present when the block has that directory;
///    for TIFF there is none, the pages hold these directories;
/// 3. `XMP` leaf, keyword `Packet`: the start of the first XMP packet of a
///    JPEG (`APP1` `http://ns.adobe.com/xap/1.0/`) or WebP (`XMP `), as
///    UTF-8 text;
/// 4. the `ICC` leaves, from the first profile (PNG `iCCP`, JPEG `APP2`,
///    WebP `ICCP`, tag 34675 of the first TIFF page): `Size` (the length
///    its header states), `Version` (`major.minor.fix` from header bytes 8
///    and 9, composed), `Class` and `ColorSpace` (header bytes 12 to 16 and
///    16 to 20 as Latin-1 text), and `Description` (the `desc` tag: the
///    ASCII of a `desc` type, or the first record of an `mluc` type as
///    UTF-16 big endian) when it lies within the head. A profile shorter
///    than its 128-byte header or without `acsp` at byte 36 gives no `ICC`
///    leaves and a `Damaged(Icc)` note.
///
/// Container leaves, as `tag`: `keyword` (value):
///
/// | Format | Leaves |
/// |---|---|
/// | PNG | `PNG:IHDR`: `Width`, `Height`, `BitDepth`, `ColorType`, `Compression`, `Filter`, `Interlace` (numbers, from the first chunk). Then, for each chunk met up to `IEND` or the end of the file: `PNG:pHYs`: `PixelsPerUnitX`, `PixelsPerUnitY`, `Unit`; `PNG:gAMA`: `Gamma` (the stored integer); `PNG:cHRM`: `Chromaticities` (the eight stored integers); `PNG:sRGB`: `RenderingIntent`; `PNG:sBIT`: `SignificantBits` (its one to four bytes); `PNG:tIME`: `Time` (composed `YYYY-MM-DD HH:MM:SS`); `PNG:acTL`: `Frames`, `Plays`; `PNG:iCCP`: `ProfileName` (Latin-1); `PNG:tEXt`, `PNG:zTXt`, `PNG:iTXt`: `Text` (labelled text: the chunk's keyword, then its text; Latin-1 for `tEXt` and `zTXt`, UTF-8 for `iTXt`, whose language tag and translated keyword are not shown). Text chunks are shown every time they occur; of the others only the first of each type. Other chunks are stepped over. |
/// | JPEG | For each segment before the first scan: `JPEG:JFIF` (`APP0` `JFIF`): `Version` (composed `major.minor` with the minor in two digits), `Units`, `XDensity`, `YDensity`; `JPEG:Adobe` (`APP14` `Adobe`): `Version`, `Transform`; `JPEG:COM`: `Comment` (UTF-8 text), every time it occurs; `JPEG:SOFn` with `n` the frame header's marker minus `0xC0` in decimal: `Precision`, `Height`, `Width`, `Components`. Of `JFIF`, `Adobe` and the frame header only the first. Other segments are stepped over. |
/// | WebP | `WEBP:VP8X`: `Flags`, `CanvasWidth`, `CanvasHeight`, for an extended file; else `WEBP:VP8` or `WEBP:VP8L`: `Width`, `Height` as the bitstream states them. `WEBP:ANIM`: `LoopCount`. Other chunks are stepped over. |
/// | TIFF | One group `TIFF:page N` for each of the first pages of the chain, `N` from 0: the page's entries in the order the file lists them (a tag listed twice is shown twice), then the groups `Exif`, `GPS` and `Interop` for the directories the page points to. Classic and BigTIFF, either byte order. |
///
/// A directory entry is a leaf whose `tag` is its tag number as `0x` and
/// four upper-case hex digits, whose `keyword` is that tag's name in the
/// directory's kind (`Unknown` when the table has none) and whose `vr` is
/// its type's name. Its value follows its type:
///
/// | Type | Value |
/// |---|---|
/// | `ASCII` | text, UTF-8 |
/// | `BYTE`, `SHORT`, `LONG`, `LONG8`, `IFD`, `IFD8`, `SBYTE`, `SSHORT`, `SLONG`, `SLONG8`, `FLOAT`, `DOUBLE` | the numbers, as `f64` |
/// | `RATIONAL`, `SRATIONAL` | rationals: numerator and denominator as stored, never divided |
/// | `UNDEFINED` | binary: its length only |
/// | a type code that is none of these | the problem `UnknownType`; nothing is read for it |
///
/// with these exceptions, by tag number in any directory kind: 700 (XMP,
/// `BYTE` or `UNDEFINED`) is UTF-8 text; 40091 to 40095 (the `XP` tags,
/// `BYTE`) are UTF-16 little-endian text; in an EXIF directory 37510
/// (`UserComment`) is the text after its eight-byte character code when
/// that code is `ASCII\0\0\0` and binary otherwise, and 36864 and 40960
/// (`ExifVersion`, `FlashpixVersion`) are Latin-1 text. A maker note
/// (37500), an embedded thumbnail, IPTC and Photoshop blocks stay binary
/// lengths: they are never parsed or shown.
///
/// # Values are shown safely
///
/// Text reaches a node only through [`TagSink`], which decodes it (bytes
/// that are not valid in the encoding become U+FFFD), drops trailing NULs
/// and white space, shows a tab, line feed or carriage return as one
/// space, and writes every other control character (U+0000 to U+001F,
/// U+007F to U+009F), every bidirectional and invisible formatting
/// character (U+200B to U+200F, U+2028 to U+202E, U+2060 to U+2069, U+FEFF,
/// U+FFF9 to U+FFFB) as `\u{..}` with its code point in lower-case hex. So
/// an escape sequence in a value is visible text and cannot act on a
/// terminal or reorder what is shown, and a NUL inside a value is `\u{0}`.
/// A number that is not finite is a problem, not a value.
pub fn read_raster_tags(
    format: FileFormat,
    source: &mut dyn RasterSource,
    length: u64,
) -> RasterTagTree {
    let mut sink = TagSink::new();
    let mut counted = CountedReads {
        source,
        left: RASTER_TAGS_MAX_READS,
        refused: false,
        failed: false,
    };
    let (panicked, bytes_refused) = {
        let mut reader = Reader::new(&mut counted, length, RASTER_TAGS_READ_BUDGET_BYTES);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_format(format, &mut reader, length, &mut sink);
        }));
        (outcome.is_err(), reader.refused())
    };
    if counted.failed {
        sink.note(RasterTagNote::Damaged(RasterTagPart::Container));
    }
    if counted.refused {
        sink.note(RasterTagNote::Limit(RasterTagLimit::Reads));
    }
    if bytes_refused {
        sink.note(RasterTagNote::Limit(RasterTagLimit::Bytes));
    }
    if panicked {
        sink.note(RasterTagNote::ReaderFailed);
    }
    sink.finish()
}

/// Walks a file of `format` and gives [`TagSink`] what [`read_raster_tags`]
/// describes. `reader` is positioned at the start of the file's `length`
/// bytes and enforces the byte and read limits; `sink` enforces the node,
/// depth and text limits. This reports damage as notes and returns; it does
/// not panic.
///
/// A read can fail in two ways, and they are told apart by what was asked.
/// A read of bytes the file does not have (past `length`) is damage: check
/// the range first and note it. A read of bytes the file does have fails
/// only because a limit was reached or the disk failed: the walk ends
/// there, at once, and notes nothing, since [`read_raster_tags`] notes the
/// limit or the failure itself. The same when [`TagSink::refused`] turns
/// true.
fn read_format(_format: FileFormat, _reader: &mut Reader<'_>, _length: u64, _sink: &mut TagSink) {
    // Scaffolding: this names everything a reader is given, so that the
    // build has no unused item before the reader exists. It goes when the
    // body is written.
    let _ = (
        names::ifd_tag_keyword,
        names::ifd_type_name,
        names::ifd_type_size,
        [
            names::IfdKind::Image,
            names::IfdKind::Exif,
            names::IfdKind::Gps,
            names::IfdKind::Interop,
        ],
        [
            TextEncoding::Utf8,
            TextEncoding::Latin1,
            TextEncoding::Utf16Le,
            TextEncoding::Utf16Be,
        ],
        [
            TagName::Fixed(""),
            TagName::Entry(0),
            TagName::Page(0),
            TagName::FrameHeader(0),
        ],
        [
            TagProblem::OutsideFile,
            TagProblem::UnknownType,
            TagProblem::Unreadable,
        ],
        [
            TagData::Text {
                bytes: &[],
                encoding: TextEncoding::Utf8,
                more: false,
            },
            TagData::LabelledText {
                label: &[],
                bytes: &[],
                encoding: TextEncoding::Utf8,
                more: false,
            },
            TagData::Composed(String::new()),
            TagData::Number(0.0),
            TagData::Numbers {
                values: &[],
                total: 0,
            },
            TagData::Rationals {
                values: &[],
                total: 0,
            },
            TagData::Binary { length: 0 },
            TagData::Problem(TagProblem::Unreadable),
        ],
        TagSink::leaf,
        TagSink::open,
        TagSink::close,
        TagSink::refused,
        Reader::read_span,
        Reader::refused,
        [
            RasterTagPart::Container,
            RasterTagPart::Exif,
            RasterTagPart::Icc,
            RasterTagPart::Xmp,
            RasterTagPart::Text,
        ],
        [
            RasterTagLimit::Entries,
            RasterTagLimit::Directories,
            RasterTagLimit::Pages,
            RasterTagLimit::Inflate,
        ],
    );
    todo!("FMT3: read the metadata tree of a raster file")
}

/// A source that refuses reads past [`RASTER_TAGS_MAX_READS`].
struct CountedReads<'a> {
    source: &'a mut dyn RasterSource,
    left: u64,
    refused: bool,
    /// Whether the source itself failed to read.
    failed: bool,
}

impl Read for CountedReads<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            self.refused = true;
            return Err(io::Error::other("raster metadata read limit reached"));
        }
        self.left -= 1;
        self.source.read(out).inspect_err(|_| self.failed = true)
    }
}

impl Seek for CountedReads<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.source.seek(to)
    }
}

/// How the bytes of a text value are decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pixels::raster) enum TextEncoding {
    Utf8,
    /// Each byte is the code point of the same number (ISO 8859-1).
    Latin1,
    Utf16Le,
    Utf16Be,
}

/// The `tag` of a node: a fixed name, or one built from a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pixels::raster) enum TagName {
    /// A name from the tables of [`read_raster_tags`], such as `PNG:IHDR`.
    Fixed(&'static str),
    /// A directory entry: `0x010F`.
    Entry(u16),
    /// A TIFF page: `TIFF:page 3`.
    Page(u32),
    /// A JPEG frame header, by its marker minus `0xC0`: `JPEG:SOF2`.
    FrameHeader(u8),
}

impl TagName {
    fn text(self) -> String {
        match self {
            Self::Fixed(name) => name.to_string(),
            Self::Entry(tag) => format!("0x{tag:04X}"),
            Self::Page(page) => format!("TIFF:page {page}"),
            Self::FrameHeader(index) => format!("JPEG:SOF{index}"),
        }
    }
}

/// Why a value could not be shown; the node says so in fixed words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pixels::raster) enum TagProblem {
    /// The value's bytes do not lie inside the file or its block.
    OutsideFile,
    /// The entry's type code is not a TIFF value type.
    UnknownType,
    /// The value could not be read or inflated.
    Unreadable,
}

/// The value of a leaf, as a format reader hands it over.
pub(in crate::pixels::raster) enum TagData<'a> {
    /// Text of the file. `more` says the value goes on after `bytes`, so
    /// the text shown ends with `…`.
    Text {
        bytes: &'a [u8],
        encoding: TextEncoding,
        more: bool,
    },
    /// Text of the file under a label of the file (a PNG text chunk's
    /// keyword): shown as `label: text`.
    LabelledText {
        label: &'a [u8],
        bytes: &'a [u8],
        encoding: TextEncoding,
        more: bool,
    },
    /// Text put together here from numbers of the file, such as a version
    /// or a time. It is escaped and cut like any other text.
    Composed(String),
    Number(f64),
    /// The first numbers of a value that holds `total` of them.
    Numbers {
        values: &'a [f64],
        total: u64,
    },
    /// The first rationals of a value that holds `total`: numerator and
    /// denominator as stored.
    Rationals {
        values: &'a [(i64, i64)],
        total: u64,
    },
    /// A value shown by its length in bytes only.
    Binary {
        length: u64,
    },
    Problem(TagProblem),
}

/// Where every node of a raster's metadata tree is made. A format reader
/// offers leaves and groups; the sink names them, decodes, escapes and cuts
/// their text, and keeps the tree inside its limits.
pub(in crate::pixels::raster) struct TagSink {
    nodes: Vec<TagNode>,
    /// Open groups, outermost first, each with the children it has so far.
    open: Vec<(String, Vec<TagNode>)>,
    nodes_left: usize,
    text_left: usize,
    refused: bool,
    notes: Vec<RasterTagNote>,
}

impl TagSink {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            open: Vec::new(),
            nodes_left: RASTER_TAGS_MAX_NODES,
            text_left: RASTER_TAGS_MAX_TEXT_BYTES,
            refused: false,
            notes: Vec::new(),
        }
    }

    /// Records `note`, once.
    pub(in crate::pixels::raster) fn note(&mut self, note: RasterTagNote) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    /// Whether a node was refused because the tree is full. A reader that
    /// sees this stops reading: nothing it could add would be kept, and the
    /// limit is already noted.
    pub(in crate::pixels::raster) fn refused(&self) -> bool {
        self.refused
    }

    /// Adds a leaf to the innermost open group, or to the top level.
    /// Returns `false`, having noted the limit, when the tree is full.
    pub(in crate::pixels::raster) fn leaf(
        &mut self,
        tag: TagName,
        keyword: &'static str,
        vr: &'static str,
        data: TagData<'_>,
    ) -> bool {
        if !self.take_node() {
            return false;
        }
        let value = self.value(data);
        let node = TagNode {
            tag: tag.text(),
            vr: vr.to_string(),
            keyword: keyword.to_string(),
            value,
        };
        match self.open.last_mut() {
            Some((_, children)) => children.push(node),
            None => self.nodes.push(node),
        }
        true
    }

    /// Opens a group inside the innermost open group, or at the top level.
    /// Returns `false`, having noted the limit, when the tree is full or
    /// the group's entries would lie too deep; `close` is then not called.
    pub(in crate::pixels::raster) fn open(&mut self, tag: TagName) -> bool {
        if self.open.len() + 2 > RASTER_TAGS_MAX_DEPTH {
            self.note(RasterTagNote::Limit(RasterTagLimit::Depth));
            return false;
        }
        if !self.take_node() {
            return false;
        }
        self.open.push((tag.text(), Vec::new()));
        true
    }

    /// Closes the innermost open group. A group that got no children is
    /// left out of the tree.
    pub(in crate::pixels::raster) fn close(&mut self) {
        let Some((tag, children)) = self.open.pop() else {
            return;
        };
        if children.is_empty() {
            self.nodes_left += 1;
            return;
        }
        let node = TagNode {
            tag,
            vr: String::new(),
            keyword: String::new(),
            value: TagValue::Sequence {
                items: vec![children],
                truncated: false,
                total: None,
            },
        };
        match self.open.last_mut() {
            Some((_, children)) => children.push(node),
            None => self.nodes.push(node),
        }
    }

    fn finish(mut self) -> RasterTagTree {
        while !self.open.is_empty() {
            self.close();
        }
        RasterTagTree {
            nodes: self.nodes,
            notes: self.notes,
        }
    }

    fn take_node(&mut self) -> bool {
        if self.nodes_left == 0 {
            self.refused = true;
            self.note(RasterTagNote::Limit(RasterTagLimit::Nodes));
            return false;
        }
        self.nodes_left -= 1;
        true
    }

    fn value(&mut self, data: TagData<'_>) -> TagValue {
        match data {
            TagData::Text {
                bytes,
                encoding,
                more,
            } => self.text(&decode(bytes, encoding, more), more),
            TagData::LabelledText {
                label,
                bytes,
                encoding,
                more,
            } => {
                let label = decode(label, encoding, false);
                let text = decode(bytes, encoding, more);
                self.text(&format!("{}: {text}", trimmed(&label)), more)
            }
            TagData::Composed(text) => self.text(&text, false),
            TagData::Number(value) => numbers(&[value], 1),
            TagData::Numbers { values, total } => numbers(values, total),
            TagData::Rationals { values, total } => {
                let shown = &values[..values.len().min(RASTER_TAG_NUMBERS_MAX)];
                let mut text = shown
                    .iter()
                    .map(|(numerator, denominator)| format!("{numerator}/{denominator}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if total > shown.len() as u64 {
                    text.push_str(", …");
                }
                self.text(&text, false)
            }
            TagData::Binary { length } => TagValue::Binary {
                length: usize::try_from(length).unwrap_or(usize::MAX),
            },
            TagData::Problem(problem) => problem_value(problem),
        }
    }

    /// `text` as a node shows it: trimmed, escaped, and cut to the
    /// character limit of a value and the text limit of the tree.
    fn text(&mut self, text: &str, more: bool) -> TagValue {
        let mut shown = String::new();
        let mut chars = 0;
        let mut cut = more;
        let mut unit = String::new();
        for character in trimmed(text).chars() {
            unit.clear();
            escape_into(character, &mut unit);
            let unit_chars = unit.chars().count();
            if chars + unit_chars > RASTER_TAG_TEXT_MAX_CHARS {
                cut = true;
                break;
            }
            if shown.len() + unit.len() > self.text_left {
                self.note(RasterTagNote::Limit(RasterTagLimit::Text));
                cut = true;
                break;
            }
            shown.push_str(&unit);
            chars += unit_chars;
        }
        self.text_left -= shown.len();
        if cut {
            shown.push('…');
        }
        TagValue::String { value: shown }
    }
}

fn trimmed(text: &str) -> &str {
    text.trim_end_matches(|character: char| character == '\0' || character.is_whitespace())
}

fn numbers(values: &[f64], total: u64) -> TagValue {
    if values.iter().any(|value| !value.is_finite()) {
        return TagValue::Error {
            message: "not a finite number".to_string(),
        };
    }
    let total = total.max(values.len() as u64);
    if let ([value], 1) = (values, total) {
        return TagValue::Number { value: *value };
    }
    let shown = &values[..values.len().min(RASTER_TAG_NUMBERS_MAX)];
    let truncated = total > shown.len() as u64;
    TagValue::Numbers {
        value: shown.to_vec(),
        truncated,
        total: truncated.then_some(usize::try_from(total).unwrap_or(usize::MAX)),
    }
}

fn problem_value(problem: TagProblem) -> TagValue {
    TagValue::Error {
        message: match problem {
            TagProblem::OutsideFile => "the value lies outside the file",
            TagProblem::UnknownType => "the value has an unknown type",
            TagProblem::Unreadable => "the value could not be read",
        }
        .to_string(),
    }
}

/// `bytes` as text. When more of the value follows (`more`), a character
/// the bytes end in the middle of is left out, not shown as damage.
fn decode(bytes: &[u8], encoding: TextEncoding, more: bool) -> String {
    match encoding {
        TextEncoding::Utf8 => {
            let whole = if more {
                &bytes[..complete_utf8_prefix(bytes)]
            } else {
                bytes
            };
            String::from_utf8_lossy(whole).into_owned()
        }
        TextEncoding::Latin1 => bytes.iter().map(|byte| char::from(*byte)).collect(),
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let units = bytes.chunks_exact(2).map(|pair| {
                let pair = [pair[0], pair[1]];
                if encoding == TextEncoding::Utf16Le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            });
            char::decode_utf16(units)
                .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect()
        }
    }
}

/// The length of `bytes` without a UTF-8 sequence its end cuts short.
fn complete_utf8_prefix(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let at = bytes.len() - back;
        let byte = bytes[at];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        let needed = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        return if needed > back { at } else { bytes.len() };
    }
    bytes.len()
}

/// Appends `character` as a value shows it; see "Values are shown safely".
fn escape_into(character: char, out: &mut String) {
    use std::fmt::Write;
    match character {
        '\t' | '\n' | '\r' => out.push(' '),
        '\u{0}'..='\u{1f}'
        | '\u{7f}'..='\u{9f}'
        | '\u{200b}'..='\u{200f}'
        | '\u{2028}'..='\u{202e}'
        | '\u{2060}'..='\u{2069}'
        | '\u{feff}'
        | '\u{fff9}'..='\u{fffb}' => {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(character));
        }
        _ => out.push(character),
    }
}

/// What the Metadata panel shows for the raster `file`: leaves the catalog
/// entry gives without reading the file, the tree [`read_raster_tags`]
/// reads, and one `Note` leaf for each thing a reader of the tree should
/// know.
///
/// The `File` leaves: `Format` (the detected format), `Size` (bytes),
/// `Extension` (the file name's, in lower case, when it is 1 to 16 ASCII
/// letters and digits), `Pages` and `Frames`, and for a TIFF with pages
/// that are not frames `ExcludedPages` (how many) and one `ExcludedPage`
/// for each the catalog lists (`page 3: width`).
///
/// The notes: the catalog entry's warnings, a name whose extension belongs
/// to another format, an animated file of which only the first frame is a
/// frame, and the reader's notes. All are fixed words and numbers.
///
/// Errors only when the file cannot be opened or its length read.
pub(crate) fn raster_tag_nodes(file: &FileEntry) -> io::Result<Vec<TagNode>> {
    let mut source = std::fs::File::open(&file.path)?;
    let length = source.metadata()?.len();
    let tree = read_raster_tags(file.format, &mut source, length);

    let leaf = |tag: &str, keyword: &str, value: TagValue| TagNode {
        tag: tag.to_string(),
        vr: String::new(),
        keyword: keyword.to_string(),
        value,
    };
    let text = |value: String| TagValue::String { value };
    let number = |value: f64| TagValue::Number { value };
    let mut notes = Vec::new();

    let mut nodes = vec![
        leaf("File", "Format", text(file.format.as_str().to_string())),
        leaf("File", "Size", number(length as f64)),
    ];
    let extension = file
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            (1..=16).contains(&extension.len())
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase);
    if let Some(extension) = extension {
        let usual = match extension.as_str() {
            "png" => Some(FileFormat::Png),
            "jpg" | "jpeg" | "jpe" | "jfif" => Some(FileFormat::Jpeg),
            "tif" | "tiff" => Some(FileFormat::Tiff),
            "webp" => Some(FileFormat::Webp),
            "dcm" | "dicom" => Some(FileFormat::Dicom),
            _ => None,
        };
        if usual.is_some_and(|usual| usual != file.format) {
            notes.push("the file name's extension is that of another format".to_string());
        }
        nodes.push(leaf("File", "Extension", text(extension)));
    }
    if let Some(raster) = &file.raster {
        nodes.push(leaf("File", "Pages", number(f64::from(raster.pages_total))));
        nodes.push(leaf("File", "Frames", number(f64::from(file.frame_count))));
        if raster.excluded_pages_total > 0 {
            nodes.push(leaf(
                "File",
                "ExcludedPages",
                number(f64::from(raster.excluded_pages_total)),
            ));
        }
        for excluded in &raster.excluded_pages {
            let differs = serde_json::to_value(excluded.differs)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            nodes.push(leaf(
                "File",
                "ExcludedPage",
                text(format!("page {}: {differs}", excluded.page)),
            ));
        }
        if raster.animated {
            notes.push("an animation: only its first frame is shown".to_string());
        }
        notes.extend(raster.warnings.iter().cloned());
    }
    nodes.extend(tree.nodes);
    notes.extend(tree.notes.iter().map(ToString::to_string));
    nodes.extend(notes.into_iter().map(|note| leaf("Note", "", text(note))));
    Ok(nodes)
}
