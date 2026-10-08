//! Decoding raster image files (PNG, JPEG, TIFF, WebP) into frames of stored
//! samples, and presenting those frames (`docs/design/image-formats.md`
//! sections 2.3, 5.2 to 5.4 and 6.2).
//!
//! A raster file is input nobody vouches for. Two functions hold everything
//! that reads one or interprets its samples, and their doc comments are the
//! contract: [`decode_raster_frame`] (what is read, how much, and what the
//! samples are) and [`render_raster_frame`] (how a decoded frame is shown).
//! The rest of this file is the wiring that puts them behind the pixel
//! service: `Codec::Raster` in `syntax.rs` is dispatched here by
//! `service.rs`, so a raster frame takes the same caches, decode permits,
//! redaction and thumbnail paths as a DICOM frame.

mod display;
mod image;
mod png;
mod reader;
mod tiff;

use super::error::{PixelError, PixelResult};
use super::render::{DisplayBuffer, DisplayPng};
use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::{FileEntry, RASTER_MAX_FRAME_PIXELS};
use anyhow::Context;
use bytes::Bytes;
use std::fs::File;
use std::io::{Read, Seek};
use std::sync::Arc;
use tokio::task;

/// The buffer a raster file is read through. A read issued to the file asks
/// for this many bytes, or for less when less is left of the file, of the
/// read budget, or of the run of TIFF strips or tiles it begins in.
pub const RASTER_READ_BUFFER_BYTES: usize = 64 * 1024;

/// The part of [`raster_read_budget`] that does not depend on the image: room
/// for what a file carries beside its samples (profiles, EXIF, text, a
/// thumbnail, an appended video) and for reading between scattered chunks.
pub const RASTER_READ_BUDGET_BASE_BYTES: u64 = 64 * 1024 * 1024;

/// The part of [`raster_read_budget`] that grows with the frame: no encoding
/// of an image in these formats is this many times larger than its samples.
pub const RASTER_READ_BUDGET_PER_DECODED_BYTE: u64 = 4;

/// The largest ICC profile a decoded frame carries. Display profiles are a
/// few kilobytes; one above this is dropped, not passed to every display
/// frame of the file.
pub const RASTER_ICC_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The most strips or tiles one TIFF page may have, and so the most values
/// any tag that lays out its samples may hold. A 16,384-row image stored one
/// row to a strip has 16,384.
pub const RASTER_TIFF_MAX_CHUNKS: usize = 65_536;

/// The most entries the IFD of a TIFF page may have. Real pages have tens.
pub const RASTER_TIFF_MAX_TAGS: usize = 4096;

/// How much larger than its image a TIFF tile may be: its width must be less
/// than this many pixels more than the image's, and so must its length. The
/// decoder reads and discards what a tile holds beside the image, so this
/// bounds work the frame's size does not account for. Writers use tiles of
/// up to 4,096 pixels, whatever the image, or one tile of the image's size
/// rounded up to a multiple of 16.
pub const RASTER_TIFF_TILE_MARGIN: u32 = 4096;

/// The most scans a progressive JPEG may have and be decoded. Every scan is a
/// pass over the whole image, and a scan can be a few bytes long, so without
/// a limit a small file buys unbounded work. Encoders write about ten.
///
/// This constant is not passed to a decoder. The limit is the default of
/// the JPEG decoder `image` links (`zune-jpeg`), and `image` gives a caller
/// no way to set it. The constant states that default, and
/// `tests/raster_cost/bounds.rs` holds the decoder to it: a file with this
/// many scans decodes and one with a scan more does not, so a release of
/// the crate with another default fails there.
pub const RASTER_JPEG_MAX_SCANS: usize = 100;

/// The part of [`raster_decode_heap_limit`] that does not depend on the
/// image: 16 MiB for decoder state and the copies made of a profile, and
/// room for the one allocation a decoder crate sizes from the file that
/// cannot be checked first. A lossy WebP's decoder allocates each partition
/// of coefficients at the length its bitstream declares, up to 2^24 - 1
/// bytes held in four-byte words, before reading it, and gives up at the
/// first one the file does not hold. So one partition of 16 MiB can be held
/// with no bytes behind it, beside the read buffer it was to be filled
/// through.
pub const RASTER_DECODE_HEAP_BASE_BYTES: u64 =
    16 * 1024 * 1024 + (1 << 24) + RASTER_READ_BUFFER_BYTES as u64;

/// The bytes one decoded frame of `file` holds: `rows * columns *
/// samples_per_pixel * bits_allocated / 8`, from the catalog entry. `None`
/// when the frame has more than [`RASTER_MAX_FRAME_PIXELS`] pixels or the
/// entry is not a raster.
///
/// At the limit this is 2 GiB (four 16-bit samples, or one 64-bit sample,
/// per pixel).
pub fn raster_frame_bytes(file: &FileEntry) -> Option<u64> {
    let pixels = u64::from(file.rows) * u64::from(file.columns);
    if !file.format.is_raster() || pixels == 0 || pixels > RASTER_MAX_FRAME_PIXELS {
        return None;
    }
    Some(pixels * u64::from(file.samples_per_pixel) * u64::from(file.bits_allocated / 8))
}

/// The most bytes decoding one frame of `file` may be handed from the file:
/// [`RASTER_READ_BUDGET_BASE_BYTES`] plus
/// [`RASTER_READ_BUDGET_PER_DECODED_BYTE`] times [`raster_frame_bytes`]. It
/// is fixed by the catalog entry, whatever the file's length and whatever its
/// chunks, segments and tags declare. What is charged to it is stated under
/// "Bytes read" in [`decode_raster_frame`]. `None` exactly when
/// [`raster_frame_bytes`] is.
pub fn raster_read_budget(file: &FileEntry) -> Option<u64> {
    Some(
        RASTER_READ_BUDGET_BASE_BYTES
            + RASTER_READ_BUDGET_PER_DECODED_BYTE * raster_frame_bytes(file)?,
    )
}

/// The most heap one decode of a frame of `file`, from a file of `length`
/// bytes, may hold at once on the decoding thread. With the entry's
/// `raster.file_length` as `length`, it is also what the decode reserves of
/// the decode memory budget before it starts (`admission::decode_estimate`):
///
/// ```text
/// RASTER_DECODE_HEAP_BASE_BYTES
///   + 6 * raster_frame_bytes(file)
///   + 4 * min(length, raster_read_budget(file))
/// ```
///
/// The first term covers decoder state, the read buffer, the copies made
/// of a profile of [`RASTER_ICC_MAX_BYTES`] and one partition of a lossy
/// WebP at the largest length a bitstream can declare for it; the second
/// the frame, the decoder's own copy of it and its working rows, planes or
/// coefficients; the third an encoded image held in memory while it is
/// decoded (JPEG and WebP) and what is assembled from its segments. Nothing
/// in it is a number the file declares: the frame size is the catalog's,
/// checked against the file before anything is allocated for it, and
/// `length` is bytes that exist. `None` exactly when [`raster_frame_bytes`]
/// is.
pub fn raster_decode_heap_limit(file: &FileEntry, length: u64) -> Option<u64> {
    let budget = raster_read_budget(file)?;
    Some(RASTER_DECODE_HEAP_BASE_BYTES + 6 * raster_frame_bytes(file)? + 4 * length.min(budget))
}

/// One decoded frame of a raster file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterFrame {
    /// The frame's samples in stored sample semantics: exactly
    /// [`raster_frame_bytes`] bytes, rows from the top, pixels left to right
    /// in the stored grid, the samples of a pixel interleaved, each sample
    /// `bits_allocated / 8` bytes, little endian.
    pub bytes: Vec<u8>,
    /// The file's ICC profile when it describes these samples and the frame
    /// is colour; see [`decode_raster_frame`].
    pub icc_profile: Option<Vec<u8>>,
}

/// What a raster frame is decoded from: the file, or a test's counted bytes.
pub trait RasterSource: Read + Seek {}

impl<T: Read + Seek> RasterSource for T {}

/// Decodes frame `frame` of the raster `file` from `source`, which holds the
/// file's `length` bytes and is positioned at its start.
///
/// `file` is the catalog entry discovery made for this file
/// (`loader/raster.rs`): its `format`, `rows`, `columns`, `samples_per_pixel`,
/// `bits_allocated`, `pixel_representation`, `photometric_interpretation` and
/// `raster` metadata say what the frame must be. `source` is the only thing
/// read: this function never opens `file.path`.
///
/// # Outcomes
///
/// - `Ok(frame)`: `frame.bytes` is exactly [`raster_frame_bytes`] long and
///   holds the samples described under "Samples".
/// - `Err(PixelError::FrameOutOfRange)` when `frame >= file.frame_count`.
/// - `Err(PixelError::UnsupportedLayout(reason))` with the `raster.*` reason
///   id of `classify_pixel_support(file)` when the entry is not renderable,
///   and with `raster.unsupported_compression`,
///   `raster.unsupported_sample_format` or `raster.unsupported_color` when a
///   TIFF frame's own page turns out to have a compression, sample layout or
///   plane layout outside the table below (pages of one file need not share
///   a compression, and the catalog reports page 0's). Both are decided
///   before any sample is read.
/// - `Err(PixelError::Decode { .. })` (`PixelError::frame_decode`) for
///   everything else: a truncated or corrupt stream, a file that no longer
///   matches its entry, a limit below reached. The message says which.
///   `length` greater than the entry's `raster.file_length` is one of
///   these, decided before anything is read: the memory reserved for the
///   decode (`admission::decode_estimate`) was computed for the length
///   discovery measured, and a file that has grown may hold more.
///
/// It never panics, whatever `source` holds: a panic inside a decoder crate
/// is caught here and is a decode error. A PNG or TIFF that ends before its
/// frame is complete is an error, never a frame padded out. What a damaged
/// JPEG or WebP still yields is its decoder's decision (a progressive JPEG
/// that lost its last scans decodes, less finely); whatever is returned has
/// the entry's size.
///
/// # The entry is checked against the file
///
/// Discovery read the header once; the file may have changed since, and
/// discovery trusts some declarations it does not verify (a WebP `VP8X`
/// canvas and its flags, a TIFF page it found by offset). So before any
/// buffer is sized, the image found in `source` must have the entry's width,
/// height, colour type (after the conversions below), bit depth and sample
/// format, and for TIFF the page at the frame's recorded offset must be read
/// again. Any difference is a decode error, never a frame of another size or
/// layout and never an allocation sized by the file's own claim.
///
/// The rule covers every size a decoder would otherwise take from the file
/// on its own: nothing the file declares sizes an allocation or bounds a
/// loop before it has been compared with the entry or with a constant here.
/// Beyond the image header of each format, that is:
///
/// - **WebP.** A lossy bitstream (`VP8 `) states its own width and height
///   and its decoder allocates by them, while the entry of an extended file
///   (`VP8X`) is made from the canvas. So before the decoder is given the
///   file, its chunks are walked, through the same reader and budget, up to
///   the first image. In a simple file that is the first chunk, `VP8 ` or
///   `VP8L`, and it must state the entry's size. In an extended file the
///   canvas must have the entry's size, and of the chunks after `VP8X` the
///   first that is `VP8 `, `VP8L` or `ANMF` decides: a `VP8 ` or `VP8L`
///   chunk must state the entry's size, in a file not flagged animated; an
///   `ANMF` chunk must be in a file flagged animated, its frame must lie
///   inside the canvas, and the bitstream it holds (after an `ALPH` chunk,
///   when there is one, and then `VP8 `) must state the frame's size. A
///   file with no such chunk, or whose first one breaks a rule, is a decode
///   error. A first frame smaller than the canvas is not: it is composed on
///   the canvas, as the format intends. The size a `VP8 ` bitstream states
///   is the two 14-bit numbers after its start code `9D 01 2A`; a `VP8L`
///   bitstream's is the two 14-bit numbers, each one less than the size,
///   after its signature byte `2F`.
/// - **WebP key frames.** A `VP8 ` bitstream must be a key frame (the lowest
///   bit of its first byte is 0), as a still image and the first frame of
///   an animation are. Only a key frame has a start code and a size; one
///   that says it is not is a decode error before the decoder is given the
///   file.
/// - **WebP profile.** An `ICCP` chunk is read only when it comes before the
///   first image chunk, as the format requires, declares at most
///   [`RASTER_ICC_MAX_BYTES`] and ends inside the file. Any other is left
///   out without being read and the frame is decoded. When several come
///   before the image only the first is considered, whatever a later one
///   holds.
/// - **TIFF tags.** A page whose IFD holds any tag twice is a decode error,
///   whichever tag it is: readers disagree on which entry counts, so the
///   page checked here would not be the page the decoder reads. (Discovery
///   does not list such a page; a file can become one afterwards.) Entries
///   need not be in ascending order.
/// - **TIFF tiles.** A tile must be less than [`RASTER_TIFF_TILE_MARGIN`]
///   pixels wider than the image and less than that many longer; a page
///   with a larger tile is a decode error before any tile is read. Rows per
///   strip may exceed the image's rows (the format's default is 2^32 - 1,
///   meaning one strip), since a strip never holds more than the image.
/// - **TIFF strips and tiles.** A strip or tile is read to the end of the
///   bytes its byte count gives it and no further, whatever its compressed
///   stream goes on to say.
/// - **TIFF photometric interpretation** may be stored as a SHORT or a
///   LONG; discovery reads both, so a decode does.
///
/// # What a decode may cost
///
/// All of it is fixed by the entry and by constants here, never by a length,
/// count or size the file declares.
///
/// - **Pixels.** A frame of more than [`RASTER_MAX_FRAME_PIXELS`] is refused
///   (`raster.too_large`) before anything is read.
/// - **Bytes read.** Everything read from `source` goes through one reader,
///   which charges [`raster_read_budget`] for every byte a decoder is
///   handed: each byte `source.read` returns, read-ahead included, and each
///   buffered byte handed out again after a seek back. No read is issued
///   and no byte handed out that would take the charge past the budget, and
///   reaching it is a decode error. So the budget bounds the reading of the
///   file and the work of a decoder sent over the same bytes again and
///   again alike: strips or tiles that share bytes are charged for each
///   use, whether or not the file had to be read for it. A PNG, JPEG or
///   WebP file longer than the budget is refused before the first read:
///   these are decoded from front to back, and the JPEG and WebP decoders
///   hold the encoded image in memory. Bytes after the image (a video
///   appended to a photo) are not an error when the file fits the budget.
/// - **Reads.** The reader buffers [`RASTER_READ_BUFFER_BYTES`]: a read issued
///   to `source` asks for a whole buffer (less at the end of the file or of
///   the budget), and a seek that lands inside the buffered bytes issues no
///   read. So a PNG or JPEG costs at most `length / buffer + 2` reads and no
///   byte twice, and a WebP one read more for each chunk before its image
///   that is stepped over, by the check above and again by its decoder.
/// - **TIFF strips and tiles in any order.** A TIFF frame costs at most four
///   reads to reach its page, and the page's out-of-line tag values twice,
///   once for the checks here and once by the decoder. Its strips or tiles
///   are then read in index order, wherever they lie. Those that lie end to
///   start in that order form a run, and a read that begins in a run asks
///   for the rest of the run or a buffer, whichever is less, never for what
///   lies beyond it. So reading every strip or tile once costs its own
///   bytes, in one read for each run and one for each further buffer of it,
///   whether the file stores them first to last, last to first or
///   scattered: an honest file is never charged more than its length and
///   the page's tags a second time, and cannot exhaust the budget by the
///   order it was written in.
/// - **TIFF pages.** Frame `k` is read from the IFD at
///   `raster.frame_offsets[k]` and nothing before it: no walk along the page
///   chain, so the last frame of a file costs what the first does. Only that
///   page's tags, strips and tiles are read. A page whose IFD has more than
///   [`RASTER_TIFF_MAX_TAGS`] entries, or whose layout tags (254, 256 to
///   259, 262, 273, 277 to 279, 284, 317, 322 to 325, 338, 339) hold more
///   than [`RASTER_TIFF_MAX_CHUNKS`] values each, is a decode error before
///   any of those values is read. So is a page with a repeated tag or a
///   tile past [`RASTER_TIFF_TILE_MARGIN`], before any strip or tile is.
/// - **JPEG scans.** At most [`RASTER_JPEG_MAX_SCANS`]; a file with more is a
///   decode error, not a longer decode.
/// - **Memory.** The decoding thread's heap never holds more than
///   [`raster_decode_heap_limit`]. In particular a profile, text or EXIF
///   chunk that inflates past [`RASTER_ICC_MAX_BYTES`] or names a length
///   beyond the file is never allocated at its declared size, and a
///   compressed strip, tile or chunk is never buffered at a declared length
///   larger than the read budget. One allocation is sized by the file
///   unchecked, and the limit has room for it: a partition of a lossy
///   WebP's coefficients, at the length its bitstream declares, which is
///   less than 16 MiB. The decoder stops at the first partition the file
///   does not hold, so at most one is held without bytes behind it.
/// - **Time.** There are no wall-clock limits and no cancellation inside one
///   frame: like every decode it holds a permit of the decode scheduler and
///   runs to completion, and the limits above are what bound it.
///
/// # Formats
///
/// | Format | Decoded | Notes |
/// |---|---|---|
/// | PNG | every colour type and bit depth, Adam7 interlacing | the `png` crate with no output transformation; only the image itself (`IDAT`) of an animated file |
/// | JPEG | 8-bit baseline, extended sequential and progressive Huffman; one, three or four components | `image`'s JPEG decoder; never rotated by EXIF |
/// | TIFF | gray of 8, 16 or 32 bits unsigned or signed, or 32- or 64-bit float; RGB and RGB with alpha of 8 or 16 bits unsigned. Strips and tiles; none, LZW, Deflate (8 and 32946) and PackBits; predictors 1, 2 and 3; classic and BigTIFF, either byte order | the `tiff` crate, on the frame's own page. JPEG compression (7) is not decoded: the crate hands a strip or tile to a JPEG decoder with no limit on the size that stream declares |
/// | WebP | lossy and lossless stills, with or without alpha; the first frame of an animation, composed on the canvas | `image`'s WebP decoder |
///
/// # Samples
///
/// The samples are the ones the file stores, whatever its decoder returns
/// (section 5.2), in the layout the entry declares:
///
/// | Stored | `samples_per_pixel`, `bits_allocated` | `bytes` holds |
/// |---|---|---|
/// | PNG gray, 1, 2 or 4 bits | 1, 8 | one byte per sample with the stored value, `0..=2^depth - 1`; never scaled to `0..=255` |
/// | PNG, TIFF gray 8 or 16 bits | 1, 8 or 16 | the stored value |
/// | TIFF gray 32-bit integer, 32- or 64-bit float | 1, 32 or 64 | the stored value, bit for bit, signed as stored |
/// | TIFF WhiteIsZero | as its depth | the stored value: the decoder's inversion is not in the result (floats included, bit for bit). `photometric_interpretation` is `MONOCHROME1` and display inverts once |
/// | PNG gray + alpha | 2, 8 or 16 | gray, alpha |
/// | RGB | 3, 8 or 16 | R, G, B |
/// | RGB + alpha | 4, 8 or 16 | R, G, B, A with unassociated alpha |
/// | PNG palette | 3, 8, or 4, 8 with `tRNS` | each index replaced by its palette colour, and by its `tRNS` alpha (255 beyond the end of `tRNS`); an index beyond the palette is a decode error |
/// | JPEG YCbCr | 3, 8 | the decoder's RGB |
/// | JPEG CMYK, YCCK | 3, 8 | the decoder's approximate RGB; no profile |
/// | WebP | 3, 8, or 4, 8 with alpha | R, G, B (, A) |
///
/// - 16-bit and wider samples are little endian whatever the file's order.
/// - PNG `tRNS` on a gray or RGB image names one transparent colour; it is
///   not applied, and those frames have one or three samples.
/// - TIFF associated alpha (`raster.alpha_associated`) is un-premultiplied:
///   each colour sample `c` with alpha `a` and full scale `max` (255 or
///   65535) becomes `min(max, (c * max + a / 2) / a)`, and 0 when `a` is 0.
///   The raw tier therefore always holds unassociated alpha (section 5.4).
/// - Orientation is never applied: rows and columns are the stored grid.
///
/// # The profile
///
/// `icc_profile` is `Some` only for a frame of three or four samples whose
/// file embeds a profile (PNG `iCCP`, JPEG `APP2`, TIFF tag 34675 of the
/// frame's page, WebP `ICCP`) that is at most [`RASTER_ICC_MAX_BYTES`], is
/// as long as its header says, has the `acsp` signature, and whose data
/// colour space (bytes 16 to 20) is `RGB `. A profile that fails any of
/// these is dropped and the frame is still decoded; so is every profile of a
/// CMYK or YCCK JPEG, whose pixels it no longer describes (section 6.2), and
/// every profile of a gray frame.
pub fn decode_raster_frame(
    _file: &FileEntry,
    _frame: u32,
    _source: &mut dyn RasterSource,
    _length: u64,
) -> PixelResult<RasterFrame> {
    let (file, frame, source, length) = (_file, _frame, _source, _length);
    PixelError::ensure_frame(frame, file.frame_count)?;
    if let Some(reason) = super::syntax::classify_pixel_support(file).reason_id() {
        return Err(PixelError::UnsupportedLayout(reason.into()));
    }
    let expected = raster_frame_bytes(file)
        .ok_or_else(|| PixelError::UnsupportedLayout("raster.too_large".into()))?;
    let budget = raster_read_budget(file).expect("checked frame size");
    let listed = file.raster.as_ref().map_or(0, |raster| raster.file_length);
    if length > listed {
        return Err(PixelError::frame_decode(anyhow::anyhow!(
            "the file has changed since it was listed (it is longer now); \
             reopen the folder to view it"
        )));
    }
    if file.format != crate::api::contracts::FileFormat::Tiff && length > budget {
        return Err(PixelError::frame_decode(anyhow::anyhow!(
            "raster length exceeds read budget"
        )));
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut reader = reader::Reader::new(source, length, budget);
        let decoded = decode_format(file, frame, &mut reader, length, expected, budget)?;
        if decoded.bytes.len() as u64 != expected {
            return Err(PixelError::frame_decode(anyhow::anyhow!(
                "raster sample count differs from catalog"
            )));
        }
        Ok(decoded)
    }))
    .unwrap_or_else(|_| {
        Err(PixelError::frame_decode(anyhow::anyhow!(
            "raster decoder panicked"
        )))
    })
}

/// Renders a decoded raster frame for display: what a display frame and a
/// thumbnail of it show before anything is drawn over them.
///
/// `decoded` is the result of [`decode_raster_frame`] for `file` and
/// `frame`. The buffer has the frame's rows and columns.
///
/// | Frame | Buffer |
/// |---|---|
/// | one sample, 8- or 16-bit integer | `render_windowed_luminance` over the samples as `StoredSamples::Integer` (signed when `pixel_representation` is 1): the shared window, then MONOCHROME1 inversion once. The same pixels and the same reported window as the raw-tier display path (`service::window_raw_samples`) gives for these frames |
/// | one sample, 32-bit integer or float | `render_windowed_luminance` over `StoredSamples::Values` |
/// | gray + alpha | the gray samples windowed as above, then each pixel multiplied by its alpha: `(g * a + max / 2) / max` with `max` 255 or 65535 for the alpha's depth. `Gray8`, reporting the gray window |
/// | RGB, 8-bit | `DisplayBuffer::rgb8` of the samples, with the frame's profile |
/// | RGB + alpha | each colour sample flattened over black at its stored depth, `(c * a + max / 2) / max`, then as RGB |
/// | 16-bit colour | after flattening, each sample reduced to 8 bits, `(v * 255 + 32767) / 65535`; `DisplayBuffer::rgb8` with the frame's profile |
///
/// The window follows the shared resolution order: full dynamic, then the
/// requested center and width, then `file.default_window` (discovery sets it
/// for samples of 8 bits or fewer and for a declared TIFF range), then the
/// frame's percentiles. Colour is never windowed. Alpha is flattened for
/// display only; the raw tier keeps it (section 5.4).
///
/// Errors when `decoded.bytes` is not the entry's frame size.
pub(super) fn render_raster_frame(
    _file: &FileEntry,
    _frame: u32,
    _decoded: RasterFrame,
    _requested_wc: Option<f64>,
    _requested_ww: Option<f64>,
    _window_mode: WindowMode,
) -> anyhow::Result<DisplayBuffer> {
    display::render(
        _file,
        _frame,
        _decoded,
        _requested_wc,
        _requested_ww,
        _window_mode,
    )
}

/// Opens `file.path` and decodes `frame` from it.
fn decode_raster_file(file: &FileEntry, frame: u32) -> PixelResult<RasterFrame> {
    let open = || -> anyhow::Result<(File, u64)> {
        let source = File::open(&file.path)
            .with_context(|| format!("failed to open {}", file.path.display()))?;
        let length = source.metadata()?.len();
        Ok((source, length))
    };
    let (mut source, length) = open().map_err(PixelError::frame_decode)?;
    decode_raster_frame(file, frame, &mut source, length)
}

/// The raw frame of a raster: the decoded samples and the layout the catalog
/// entry declares for them. Rasters have no rescale and no padding; the
/// default window is the entry's.
pub(super) async fn decode_raw_raster(
    file: Arc<FileEntry>,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || {
        let decoded = decode_raster_file(&file, frame)?;
        let metadata = RawFrameMetadata {
            rows: file.rows,
            columns: file.columns,
            bits_allocated: file.bits_allocated,
            pixel_representation: file.pixel_representation,
            samples_per_pixel: file.samples_per_pixel,
            photometric_interpretation: file.photometric_interpretation.clone(),
            rescale_slope: file.rescale_slope,
            rescale_intercept: file.rescale_intercept,
            default_wc: file.default_window.map(|window| window.center),
            default_ww: file.default_window.map(|window| window.width),
            padding_low: None,
            padding_high: None,
        };
        Ok((Bytes::from(decoded.bytes), metadata))
    })
    .await
    .map_err(|error| {
        PixelError::raw_decode(anyhow::anyhow!("raster decode task failed: {error}"))
    })?
}

/// The display buffer of a raster frame, decoded for this call.
pub(super) async fn render_raster(
    file: Arc<FileEntry>,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> PixelResult<DisplayBuffer> {
    task::spawn_blocking(move || {
        let decoded = decode_raster_file(&file, frame)?;
        render_raster_frame(
            &file,
            frame,
            decoded,
            requested_wc,
            requested_ww,
            window_mode,
        )
        .map_err(PixelError::frame_decode)
    })
    .await
    .map_err(|error| {
        PixelError::frame_decode(anyhow::anyhow!("raster decode task failed: {error}"))
    })?
}

/// The display frame of a raster frame, decoded for this call.
pub(super) async fn decode_raster_to_png(
    file: Arc<FileEntry>,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> PixelResult<DisplayPng> {
    let buffer =
        render_raster(file.clone(), frame, requested_wc, requested_ww, window_mode).await?;
    task::spawn_blocking(move || buffer.into_display_png(&file, frame))
        .await
        .map_err(|error| {
            PixelError::frame_decode(anyhow::anyhow!("raster encode task failed: {error}"))
        })?
        .map_err(PixelError::frame_decode)
}

fn decode_format(
    _file: &FileEntry,
    _frame: u32,
    _reader: &mut reader::Reader<'_>,
    _length: u64,
    _expected: u64,
    _budget: u64,
) -> PixelResult<RasterFrame> {
    match _file.format {
        crate::api::contracts::FileFormat::Png => {
            png::decode(_file, _reader, _length).map_err(PixelError::frame_decode)
        }
        crate::api::contracts::FileFormat::Jpeg | crate::api::contracts::FileFormat::Webp => {
            image::decode(_file, _reader, _length, _expected).map_err(PixelError::frame_decode)
        }
        crate::api::contracts::FileFormat::Tiff => {
            tiff::decode(_file, _frame, _reader, _length, _expected, _budget)
        }
        _ => Err(PixelError::frame_decode(anyhow::anyhow!(
            "raster codec not implemented"
        ))),
    }
}

/// Profiles describe the output channels, not merely the file's source color.
fn checked_profile(file: &FileEntry, profile: Option<&[u8]>) -> Option<Vec<u8>> {
    use crate::api::contracts::{FileFormat, RasterColorType};
    let profile = profile?;
    let cmyk = file.format == FileFormat::Jpeg
        && file
            .raster
            .as_ref()
            .is_some_and(|raster| raster.color_type == RasterColorType::Cmyk);
    if !(3..=4).contains(&file.samples_per_pixel)
        || cmyk
        || profile.len() < 128
        || profile.len() > RASTER_ICC_MAX_BYTES
        || u32::from_be_bytes(profile[..4].try_into().expect("profile header")) as usize
            != profile.len()
        || &profile[36..40] != b"acsp"
        || &profile[16..20] != b"RGB "
    {
        tracing::debug!("dropping raster profile that does not describe RGB output");
        None
    } else {
        Some(profile.to_vec())
    }
}
