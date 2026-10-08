//! Compressed frames are held to the image their file's header declares.
//!
//! A compressed frame describes its own image a second time, inside its
//! codestream: a JPEG frame header, a JPEG 2000 `SIZ` segment, a JPEG XL
//! image header. The codec libraries size their buffers and loops from that
//! description. The catalog entry ([`FileEntry`]) holds what the data set
//! says: Rows, Columns, Samples per Pixel and Bits Allocated. Nothing checks
//! that the two agree unless the viewer does, and a frame that declares a
//! larger image than its header costs what the codestream says, not what the
//! entry says.
//!
//! So every decoder calls this module before it hands a frame to a codec
//! library:
//!
//! 1. [`declared`] reads what the codestream declares, from the encoded
//!    bytes alone, within the fixed bounds below. It allocates nothing that
//!    grows with what the frame declares.
//! 2. [`agrees`] compares that with the entry, by the accept table in its
//!    documentation.
//!
//! [`checked`] runs both. A frame that fails either is a decode error for
//! that frame, with [`CODESTREAM_MISMATCH`] in its message; the file stays
//! listed and its other frames decode.
//!
//! Two decoders are bounded differently, because their formats have no
//! header that settles the cost: [`decode_jpeg_xl`] decodes under the
//! decoder's own allocation limit, and [`inflate_frame`] stops at the size
//! the entry gives.
//!
//! RLE Lossless and native pixel data need none of this: their decoders size
//! everything from the entry already.

use crate::types::FileEntry;
use anyhow::{anyhow, Result};
use dicom_core::Tag;
use dicom_dictionary_std::tags;
use dicom_object::DefaultDicomObject;
use std::ops::Range;

use super::syntax::{codec_for_syntax, Codec};

/// Part of the message of every decode error raised because a frame's
/// pixel data and its header describe different images.
pub const CODESTREAM_MISMATCH: &str = "pixel data disagrees with the header";

/// The most marker segments [`declared`] steps over before a JPEG or
/// JPEG-LS frame header, and the most it reads of a JPEG 2000 main header.
/// Only the four bytes that start a segment are read to step over it, so
/// this bounds the read whatever the segments claim to hold.
pub const CODESTREAM_MAX_HEADER_SEGMENTS: usize = 1024;

/// The most scans a JPEG frame may have. Every scan is a pass over the
/// whole image and can be a few bytes long.
pub const JPEG_MAX_SCANS: u32 = 100;

/// The most boxes [`declared`] reads of a JP2 file before its codestream
/// box.
pub const JP2_MAX_BOXES: usize = 64;

/// The most tiles a JPEG 2000 frame may have. The decoder keeps a block of
/// coding parameters for every tile for as long as it decodes.
pub const J2K_MAX_TILES: u64 = 4096;

/// A JPEG 2000 tile may have this many packets for one component whatever
/// its size, and one more for every [`J2K_PIXELS_PER_PACKET`] pixels of the
/// tile beyond that. The decoder keeps a structure for every precinct, and a
/// packet is one precinct of one quality layer.
pub const J2K_MIN_PACKET_BUDGET: u64 = 65_536;

/// See [`J2K_MIN_PACKET_BUDGET`].
pub const J2K_PIXELS_PER_PACKET: u64 = 64;

/// The most bytes of a JPEG XL frame [`declared`] feeds the decoder for it
/// to read the image header (which holds an embedded colour profile).
pub const JXL_HEADER_MAX_BYTES: usize = 4 * 1024 * 1024 + 65_536;

/// What [`decode_jpeg_xl`] lets the decoder allocate whatever the frame:
/// see [`jxl_decode_limit`].
pub const JXL_DECODE_BASE_BYTES: u64 = 4 * 1024 * 1024;

/// What [`decode_jpeg_xl`] lets the decoder allocate for each sample of the
/// entry's frame: see [`jxl_decode_limit`].
pub const JXL_DECODE_BYTES_PER_SAMPLE: u64 = 64;

/// Which header a compressed frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodestreamKind {
    /// ITU-T T.81: JPEG Baseline and JPEG Lossless.
    Jpeg,
    /// ITU-T T.87.
    JpegLs,
    /// ITU-T T.800: a bare codestream, or one inside a JP2 file.
    Jpeg2000,
    /// ISO/IEC 18181: a bare codestream, or one inside the box container.
    JpegXl,
}

impl CodestreamKind {
    /// The header the frames of `codec` carry; `None` for the codecs whose
    /// frames have none (native, RLE Lossless, Deflated Image Frame).
    pub fn of(codec: Codec) -> Option<Self> {
        match codec {
            Codec::JpegBaseline | Codec::JpegLossless => Some(Self::Jpeg),
            Codec::JpegLs => Some(Self::JpegLs),
            Codec::Jpeg2000 => Some(Self::Jpeg2000),
            Codec::JpegXl => Some(Self::JpegXl),
            Codec::Native | Codec::Rle | Codec::DeflatedImageFrame => None,
        }
    }
}

/// What one compressed frame declares about its own image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// Samples in a row of the decoded image.
    pub columns: u32,
    /// Rows of the decoded image.
    pub rows: u32,
    /// Components the decoder returns for each pixel.
    pub components: u32,
    /// Bits in a sample.
    pub precision: u32,
    /// What else the frame declares that sizes its decode.
    pub structure: Structure,
}

/// The parts of a frame's structure that size its decode, by header kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Structure {
    /// [`CodestreamKind::Jpeg`] and [`CodestreamKind::JpegLs`].
    Jpeg {
        /// The second byte of the frame header's marker: `0xC0` baseline,
        /// `0xC1` extended sequential, `0xC2` progressive, `0xC3` lossless,
        /// `0xF7` JPEG-LS, and so on.
        process: u8,
        /// Start-of-scan markers in the frame. Counted for
        /// [`CodestreamKind::Jpeg`] only, and never past
        /// [`JPEG_MAX_SCANS`]` + 1`; 0 for JPEG-LS.
        scans: u32,
    },
    /// [`CodestreamKind::Jpeg2000`].
    Jpeg2000 {
        /// Where the codestream lies in the frame: the whole frame, or the
        /// contents of the codestream box of a JP2 file. The decoder is
        /// given these bytes only, so no other box of a JP2 file takes part
        /// in the decode.
        codestream: Range<usize>,
        /// Every component is stored at full resolution with the same
        /// precision and sign.
        uniform: bool,
        /// Tiles in the image.
        tiles: u64,
        /// Pixels in a tile of nominal size (no larger than the image).
        tile_pixels: u64,
        /// Packets of one component of a nominal tile: the most quality
        /// layers any coding style in the codestream declares, times the
        /// most precincts any of them gives such a tile.
        packets: u64,
    },
    /// [`CodestreamKind::JpegXl`].
    JpegXl {
        /// Samples are integers, not floating point.
        integer_samples: bool,
        /// The image header declares an animation.
        animated: bool,
    },
}

/// Reads what `frame`, the encoded bytes of one frame, declares.
///
/// Never panics, whatever `frame` holds: no indexing or slicing that can go
/// out of range, no arithmetic that can overflow. Allocates nothing whose
/// size comes from `frame`, except through the JPEG XL decoder's own header
/// parser as described below. Every failure is an error; none of them is a
/// mismatch by itself, [`checked`] adds that.
///
/// All multi-byte numbers below are big endian. "Error" means return `Err`.
///
/// # `Jpeg` and `JpegLs`
///
/// 1. The frame starts with `FF D8`, else error.
/// 2. From offset 2, read markers the way the decoders do: skip bytes until
///    an `FF`; skip every further `FF`; the next byte is the marker code,
///    unless it is `00`, in which case start over after it. Running off the
///    end of the frame is an error.
/// 3. Before the frame header, these markers have a two-byte length that
///    counts itself, and are stepped over without reading their contents:
///    `C4`, `DB`, `DD`, `FE`, `E0` to `EF`, and for `JpegLs` also `F8`. A
///    length under 2, or a segment that passes the end of the frame, is an
///    error. Any other marker that is not a frame header is an error.
///    Stepping over more than [`CODESTREAM_MAX_HEADER_SEGMENTS`] segments
///    is an error.
/// 4. The frame header is, for `Jpeg`, any of `C0` to `CF` except `C4`,
///    `C8` and `CC`; for `JpegLs`, `F7`. After the marker: length (2),
///    precision (1), rows (2), columns (2), components (1), then three
///    bytes for each component. The length must be `8 + 3 * components`
///    and lie inside the frame; rows, columns and components must not be 0;
///    else error.
/// 5. For `Jpeg` only, count the scans in the rest of the frame. Continue
///    reading markers as in 2 after the frame header segment. `DA` starts a
///    scan: count it, step over its segment by its length, then read the
///    entropy-coded bytes: an `FF` followed by `00` or by `D0` to `D7` is
///    data; an `FF` followed by `FF` is examined again from the second; an
///    `FF` followed by anything else is the next marker. `D9` ends the
///    count, and so does the end of the frame. Every other marker has a
///    length and is stepped over (a length under 2 ends the count). Stop as
///    soon as the count passes [`JPEG_MAX_SCANS`].
///
/// # `Jpeg2000`
///
/// 1. If the frame starts with the twelve bytes
///    `00 00 00 0C 6A 50 20 20 0D 0A 87 0A` it is a JP2 file. Read its
///    boxes from offset 0: length (4), type (4); a length of 1 is followed
///    by the real length (8); a length of 0 means the box ends the frame; a
///    box shorter than its own header or passing the end of the frame is an
///    error. The first box of type `jp2c` holds the codestream: everything
///    after its header. Reading [`JP2_MAX_BOXES`] boxes without finding it
///    is an error. Otherwise the codestream is the whole frame.
/// 2. The codestream starts with `FF 4F FF 51`, else error. With offsets
///    from the start of the codestream: `Lsiz` at 4 (2), `Xsiz` at 8,
///    `Ysiz` at 12, `XOsiz` at 16, `YOsiz` at 20, `XTsiz` at 24, `YTsiz` at
///    28, `XTOsiz` at 32, `YTOsiz` at 36 (4 each), `Csiz` at 40 (2), then
///    three bytes for each component from 42: `Ssiz`, `XRsiz`, `YRsiz`.
///    Errors: `Csiz` not 1 to 4; `Lsiz` not `38 + 3 * Csiz`; the segment
///    not inside the codestream; `Xsiz <= XOsiz` or `Ysiz <= YOsiz`; a tile
///    size of 0; `XTOsiz > XOsiz` or `YTOsiz > YOsiz`; an `XRsiz` or
///    `YRsiz` of 0.
/// 3. `columns = Xsiz - XOsiz`, `rows = Ysiz - YOsiz`,
///    `components = Csiz`, `precision = (Ssiz & 0x7F) + 1` of the first
///    component. `uniform` when every component has `XRsiz == 1`,
///    `YRsiz == 1` and the first component's `Ssiz`.
///    `tiles = ceil((Xsiz - XTOsiz) / XTsiz) * ceil((Ysiz - YTOsiz) / YTsiz)`
///    in `u64`. A nominal tile is `tw = min(XTsiz, columns)` by
///    `th = min(YTsiz, rows)`; `tile_pixels = tw * th`.
/// 4. Read the main header's marker segments after `SIZ`: marker (2, the
///    first byte `FF`, else error), length (2, at least 2, inside the
///    codestream, else error). `FF 90` (start of tile-part) ends the main
///    header; `FF D9` ends the codestream. Reading more than
///    [`CODESTREAM_MAX_HEADER_SEGMENTS`] segments is an error. `FF 52`
///    (`COD`) and `FF 53` (`COC`) are coding styles; a main header without
///    a `COD` is an error.
/// 5. Then read the tile-parts, each from its `FF 90`: `Lsot` (2) must be
///    10; `Psot` is at 6 from the marker (4). Its header's marker segments
///    start at 12 from the marker and are read as in 4 until `FF 93`
///    (start of data); `COD` and `COC` there are coding styles too. The
///    next tile-part starts `Psot` bytes after this one's marker. A `Psot`
///    of 0 ends the walk, and so does a `Psot` that reaches the end of the
///    codestream or an `FF D9` where the next tile-part would start. A
///    `Psot` from 1 to 13, one passing the end of the codestream, or
///    anything but `FF 90` or `FF D9` at the next position is an error.
/// 6. A coding style gives layers and precincts. `COD` after its length:
///    `Scod` (1), progression (1), layers (2), transform (1), then the
///    style parameters. `COC` after its length: component (1), `Scoc` (1),
///    then the style parameters; it has no layers. Style parameters:
///    decomposition levels `NL` (1), four bytes not read here, then, when
///    bit 0 of `Scod` or `Scoc` is set, `NL + 1` precinct bytes, lowest
///    resolution first, the low nibble `PPx` and the high nibble `PPy`.
///    Without them every `PPx` and `PPy` is 15. Errors: `NL > 32`, layers
///    of 0, a segment too short for what it must hold. The precincts of a
///    nominal tile under one style are the sum over `r` from 0 to `NL` of
///    `ceil(rw / 2^PPx[r]) * ceil(rh / 2^PPy[r])`, where
///    `rw = ceil(tw / 2^(NL - r))` and `rh = ceil(th / 2^(NL - r))`,
///    computed in `u64` with saturating arithmetic.
/// 7. `packets` is the largest layer count of any `COD` times the largest
///    precinct count of any coding style, saturating.
///
/// # `JpegXl`
///
/// The header is read by the decoder this crate links (`jxl-oxide`): build
/// an uninitialized image with an allocation tracker limited to
/// [`JXL_DECODE_BASE_BYTES`]
/// (`JxlImage::builder().alloc_tracker(AllocTracker::with_limit(..))
/// .build_uninit()`), feed it at most the first [`JXL_HEADER_MAX_BYTES`] of
/// the frame (`feed_bytes`) and ask it to initialize (`try_init`). A
/// decoder error is an error; so is a decoder that still needs more data
/// after those bytes. Then `columns = image.width()`,
/// `rows = image.height()`, `components` is the channel count of
/// `image.pixel_format()` (colour and alpha), and from
/// `image.image_header().metadata`: `precision` and `integer_samples` from
/// `bit_depth` (bits per sample; integer or floating point), `animated`
/// when `animation` is present.
///
/// One part of this read is bounded by the decoder and not here: a header
/// that declares an embedded colour profile has it read by the decoder,
/// which caps the profile at 256 MiB encoded and 256 MiB decoded and does
/// not count it against the tracker.
pub fn declared(kind: CodestreamKind, frame: &[u8]) -> Result<Declared> {
    match kind {
        CodestreamKind::Jpeg | CodestreamKind::JpegLs => read_jpeg(frame, kind),
        CodestreamKind::Jpeg2000 | CodestreamKind::JpegXl => {
            todo!("read the remaining codestream headers")
        }
    }
}

// Advancing a borrowed slice avoids unchecked offset arithmetic. Segments
// borrow their contents too; none of these reads allocates a payload buffer.
struct HeaderReader<'a> {
    remaining: &'a [u8],
}

impl<'a> HeaderReader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let bytes = self
            .remaining
            .get(..length)
            .ok_or_else(|| anyhow!("truncated codestream header"))?;
        self.remaining = self
            .remaining
            .get(length..)
            .ok_or_else(|| anyhow!("truncated codestream header"))?;
        Ok(bytes)
    }

    fn byte(&mut self) -> Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| anyhow!("missing header byte"))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }

    fn segment(&mut self) -> Result<HeaderReader<'a>> {
        let length = self
            .u16()?
            .checked_sub(2)
            .ok_or_else(|| anyhow!("segment length is less than 2"))?;
        Ok(HeaderReader {
            remaining: self.take(usize::from(length))?,
        })
    }

    fn jpeg_marker(&mut self) -> Result<u8> {
        loop {
            while self.byte()? != 0xFF {}
            let mut code = self.byte()?;
            while code == 0xFF {
                code = self.byte()?;
            }
            if code != 0 {
                return Ok(code);
            }
        }
    }
}

fn read_jpeg(frame: &[u8], kind: CodestreamKind) -> Result<Declared> {
    let mut reader = HeaderReader { remaining: frame };
    if reader.u16()? != 0xFFD8 {
        return Err(anyhow!("missing JPEG start of image"));
    }
    let is_ls = kind == CodestreamKind::JpegLs;
    let mut segments = 0;
    loop {
        let process = reader.jpeg_marker()?;
        let is_header = if is_ls {
            process == 0xF7
        } else {
            matches!(process, 0xC0..=0xCF) && !matches!(process, 0xC4 | 0xC8 | 0xCC)
        };
        if is_header {
            let mut header = reader.segment()?;
            let precision = u32::from(header.byte()?);
            let rows = u32::from(header.u16()?);
            let columns = u32::from(header.u16()?);
            let components = header.byte()?;
            if rows == 0 || columns == 0 || components == 0 {
                return Err(anyhow!("JPEG dimensions and components must be nonzero"));
            }
            if header.remaining.len() != 3 * usize::from(components) {
                return Err(anyhow!(
                    "JPEG frame header length does not match components"
                ));
            }
            return Ok(Declared {
                columns,
                rows,
                components: u32::from(components),
                precision,
                structure: Structure::Jpeg {
                    process,
                    scans: if is_ls { 0 } else { jpeg_scans(reader) },
                },
            });
        }
        if !matches!(process, 0xC4 | 0xDB | 0xDD | 0xFE | 0xE0..=0xEF)
            && !(is_ls && process == 0xF8)
        {
            return Err(anyhow!(
                "unexpected JPEG marker {process:02X} before frame header"
            ));
        }
        if segments == CODESTREAM_MAX_HEADER_SEGMENTS {
            return Err(anyhow!("too many JPEG header segments"));
        }
        reader.segment()?;
        segments += 1;
    }
}

fn jpeg_scans(mut reader: HeaderReader<'_>) -> u32 {
    let mut scans = 0;
    let mut in_scan = false;
    while let Ok(marker) = reader.jpeg_marker() {
        if marker == 0xD9 {
            break;
        }
        // Stuffed bytes are skipped by jpeg_marker, as are repeated FFs.
        // Restart markers have no length and remain part of the scan data.
        if in_scan && matches!(marker, 0xD0..=0xD7) {
            continue;
        }
        in_scan = marker == 0xDA;
        if marker == 0xDA {
            scans += 1;
            if scans > JPEG_MAX_SCANS {
                break;
            }
        }
        if reader.segment().is_err() {
            break;
        }
    }
    scans
}

/// Whether what a frame declares is what the entry says, so that decoding
/// it costs what the entry's image costs.
///
/// `Ok` exactly when every rule for the frame's kind holds. This table is
/// the whole rule: anything it does not name is not compared (the colour
/// transform, the number of resolution levels, chroma subsampling, restart
/// intervals, a point transform, Bits Stored, the sign of the samples), so
/// a file that differs from its header only in those keeps decoding.
///
/// | Kind | Rule |
/// |---|---|
/// | all | `columns == file.columns`, `rows == file.rows`, `components == file.samples_per_pixel` |
/// | JPEG, process `C0`, `C1`, `C2` | `precision == 8` and `file.bits_allocated == 8` |
/// | JPEG, process `C3` (lossless) | `precision == 8` and `file.bits_allocated == 8`; or `precision` 2 to 7 or 9 to 16 and `file.bits_allocated == 16` (the decoder returns two bytes a sample for every precision but 8) |
/// | JPEG, any other process | refused: the decoder does not decode it |
/// | JPEG | `scans <= JPEG_MAX_SCANS` |
/// | JPEG-LS (process `F7`) | `precision` 2 to 8 and `file.bits_allocated == 8`; or `precision` 9 to 16 and `file.bits_allocated == 16` |
/// | JPEG 2000 | `uniform`; `1 <= precision <= file.bits_allocated`; `tiles <= J2K_MAX_TILES`; `packets <= max(J2K_MIN_PACKET_BUDGET, tile_pixels / J2K_PIXELS_PER_PACKET)` |
/// | JPEG XL | `integer_samples`; `1 <= precision <= file.bits_allocated`; not `animated` |
///
/// The error says which rule failed and both values; it does not need to
/// contain [`CODESTREAM_MISMATCH`], [`checked`] adds it.
pub fn agrees(file: &FileEntry, declared: &Declared) -> Result<()> {
    let _ = (file, declared);
    todo!("DCM1: compare what the codestream declares with the entry")
}

/// What `frame` declares, once it is known to agree with `file`.
///
/// Every decoder calls this (directly or through
/// `pixeldata_frame::decode_object`) before a codec library sees the frame.
pub(crate) fn checked(file: &FileEntry, kind: CodestreamKind, frame: &[u8]) -> Result<Declared> {
    let declared = declared(kind, frame).map_err(|error| {
        anyhow!("{CODESTREAM_MISMATCH}: its own header cannot be read: {error:#}")
    })?;
    agrees(file, &declared).map_err(|error| anyhow!("{CODESTREAM_MISMATCH}: {error:#}"))?;
    Ok(declared)
}

/// The header kind of `file`'s frames, from its transfer syntax.
pub(crate) fn kind_of(file: &FileEntry) -> Option<CodestreamKind> {
    codec_for_syntax(&file.transfer_syntax_uid).and_then(CodestreamKind::of)
}

/// Whether the data set read for a decode still says what the entry says.
///
/// A file can be replaced after it was listed. The codec adapters size
/// their output from the data set they are given, so its Rows, Columns,
/// Samples per Pixel, Bits Allocated and transfer syntax must be the
/// entry's before they see it. An attribute the data set lacks is not
/// compared: the adapter refuses the frame for it.
pub(crate) fn header_agrees(file: &FileEntry, object: &DefaultDicomObject) -> Result<()> {
    let number = |tag: Tag| -> Option<u32> { object.get(tag)?.to_int::<u32>().ok() };
    for (name, tag, listed) in [
        ("Rows", tags::ROWS, file.rows),
        ("Columns", tags::COLUMNS, file.columns),
        (
            "Samples per Pixel",
            tags::SAMPLES_PER_PIXEL,
            file.samples_per_pixel,
        ),
        ("Bits Allocated", tags::BITS_ALLOCATED, file.bits_allocated),
    ] {
        if let Some(now) = number(tag).filter(|now| *now != listed) {
            return Err(anyhow!(
                "the file changed since it was listed: {name} is {now}, was {listed}"
            ));
        }
    }
    let trim = |uid: &str| uid.trim_end_matches('\0').trim().to_string();
    let now = trim(object.meta().transfer_syntax());
    if now != trim(&file.transfer_syntax_uid) {
        return Err(anyhow!(
            "the file changed since it was listed: its transfer syntax is {now}"
        ));
    }
    Ok(())
}

/// The most bytes the JPEG XL decoder may allocate for one frame of `file`:
/// [`JXL_DECODE_BASE_BYTES`] plus [`JXL_DECODE_BYTES_PER_SAMPLE`] for each
/// of the frame's `rows * columns * samples_per_pixel` samples, saturating.
///
/// A lossless frame decodes in about five bytes a sample at 8 bits and
/// eleven at 16, so an honest frame is far inside this; a frame whose own
/// frame header, extra channels or reference frames ask for more than the
/// entry's image is stopped by it. An embedded colour profile is outside
/// it: see [`declared`].
pub fn jxl_decode_limit(file: &FileEntry) -> u64 {
    u64::from(file.rows)
        .saturating_mul(u64::from(file.columns))
        .saturating_mul(u64::from(file.samples_per_pixel))
        .saturating_mul(JXL_DECODE_BYTES_PER_SAMPLE)
        .saturating_add(JXL_DECODE_BASE_BYTES)
}

/// Decodes one JPEG XL frame that [`checked`] has accepted, as the frame's
/// samples in pixel order: one byte a sample when `file.bits_allocated` is
/// 8, two bytes little endian when it is 16.
///
/// The decoder (`jxl-oxide`) is built with an allocation tracker limited to
/// [`jxl_decode_limit`] (`JxlImage::builder().alloc_tracker(
/// AllocTracker::with_limit(..))`), reads the whole frame, and renders its
/// first keyframe (`render_frame(0)`); the samples are taken from the
/// render's stream (`stream()`, then `write_to_buffer` into a `u8` or `u16`
/// buffer of `channels * width * height` samples), as the data set adapter
/// did before. Errors, never panics: the decoder's own errors (running out
/// of its allowance is one); a stream whose width, height or channel count
/// is not the entry's columns, rows and samples per pixel; a
/// `file.bits_allocated` other than 8 or 16; a short write. The output
/// buffer is sized from the entry, never from the stream.
pub(crate) fn decode_jpeg_xl(file: &FileEntry, frame: &[u8]) -> Result<Vec<u8>> {
    let _ = (file, frame);
    todo!("DCM1: decode a JPEG XL frame under the allocation limit")
}

/// Inflates one Deflated Image Frame fragment (a raw deflate stream) that
/// must hold exactly `expected` bytes.
///
/// Reads at most `expected + 1` bytes out of the inflater, so a fragment
/// that would inflate to more costs no more than one that is right. Errors:
/// the stream is damaged, or it holds fewer or more bytes than `expected`;
/// the message of the last two contains [`CODESTREAM_MISMATCH`]. The output
/// is allocated at `expected`, which the caller derives from the entry.
pub(crate) fn inflate_frame(fragment: &[u8], expected: usize) -> Result<Vec<u8>> {
    let _ = (fragment, expected);
    todo!("DCM1: inflate a frame to the size its entry gives")
}
