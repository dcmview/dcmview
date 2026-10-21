//! TIFF pages decoded here rather than by the `tiff` crate: bilevel gray
//! (one sample of one bit), with compression none, PackBits, LZW, Deflate or
//! CCITT Group 4; and JPEG-compressed pages (`Compression` 7) of 8-bit gray,
//! RGB or YCbCr.
//!
//! `tiff.rs` sends a page here only after every check it makes of a page
//! has passed: the page matches the catalog entry, its tiles are within the
//! margin, its strips or tiles cover the frame, and no byte count exceeds
//! the read budget. This module then reads the strips or tiles in index
//! order, each from its offset to the end of its byte count and no further,
//! through the same budget-charging reader, and writes the pixels that lie
//! inside the image into one frame of exactly the catalog's size.
//!
//! Beside the frame a bilevel page holds one packed row of a strip or tile,
//! a fixed input buffer, the fixed state of the LZW and Deflate decoders,
//! and for Group 4 the transitions of two lines; none of these is sized by a
//! length the file declares. [`group4`] says what bounds the transitions.
//! [`decode_jpeg`] says what a JPEG page holds.
use super::reader::Reader;
use super::{RASTER_JPEG_MAX_SCANS, RASTER_TIFF_TILE_MARGIN};
use anyhow::{bail, ensure, Context, Result};
use std::cell::Cell;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// The bytes of a strip or tile read from the file at a time.
const INPUT_BUFFER_BYTES: usize = 8 * 1024;

/// How a page's strips or tiles lay out its image.
pub(super) struct Layout {
    pub width: u32,
    pub height: u32,
    /// The pixels across and down one strip or tile, as stored. A strip is
    /// the image's width across and `RowsPerStrip` down.
    pub chunk_width: u64,
    pub chunk_height: u64,
    pub tiled: bool,
    pub offsets: Vec<u64>,
    pub counts: Vec<u64>,
}

/// One strip or tile, and the part of it that lies inside the image.
pub(super) struct Chunk {
    pub left: u64,
    pub top: u64,
    /// The columns and rows of this chunk that are inside the image.
    pub columns: u64,
    pub rows: u64,
}

impl Layout {
    fn across(&self) -> u64 {
        u64::from(self.width).div_ceil(self.chunk_width)
    }

    fn chunk(&self, index: usize) -> Chunk {
        let across = self.across();
        let left = (index as u64 % across) * self.chunk_width;
        let top = (index as u64 / across) * self.chunk_height;
        Chunk {
            left,
            top,
            columns: self.chunk_width.min(u64::from(self.width) - left),
            rows: self.chunk_height.min(u64::from(self.height) - top),
        }
    }

    /// Calls `decode` for each strip or tile in index order, with the bytes
    /// its byte count gives it, bit-reversed when `reverse` is set.
    fn each_chunk(
        &self,
        reader: &mut Reader<'_>,
        reverse: bool,
        mut decode: impl FnMut(&Chunk, &mut ChunkInput<'_, '_>) -> Result<()>,
    ) -> Result<()> {
        let mut buffer = vec![0; INPUT_BUFFER_BYTES];
        for (index, (&offset, &count)) in self.offsets.iter().zip(&self.counts).enumerate() {
            let chunk = self.chunk(index);
            reader.seek(SeekFrom::Start(offset))?;
            let mut input = ChunkInput {
                reader,
                left: count,
                reverse,
                buffer: &mut buffer,
                start: 0,
                end: 0,
            };
            decode(&chunk, &mut input).with_context(|| {
                format!(
                    "TIFF {} {index} could not be decoded",
                    if self.tiled { "tile" } else { "strip" }
                )
            })?;
        }
        Ok(())
    }
}

/// The bytes of one strip or tile: at most its byte count, read through the
/// decode's reader.
struct ChunkInput<'a, 'b> {
    reader: &'a mut Reader<'b>,
    left: u64,
    reverse: bool,
    buffer: &'a mut [u8],
    start: usize,
    end: usize,
}

impl BufRead for ChunkInput<'_, '_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.start == self.end && self.left > 0 {
            let most = (self.buffer.len() as u64).min(self.left) as usize;
            let count = self.reader.read(&mut self.buffer[..most])?;
            if self.reverse {
                for byte in &mut self.buffer[..count] {
                    *byte = byte.reverse_bits();
                }
            }
            self.left -= count as u64;
            self.start = 0;
            self.end = count;
            if count == 0 {
                // The file ends before the byte count does.
                self.left = 0;
            }
        }
        Ok(&self.buffer[self.start..self.end])
    }

    fn consume(&mut self, amount: usize) {
        self.start = (self.start + amount).min(self.end);
    }
}

impl Read for ChunkInput<'_, '_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(out.len());
        out[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

/// The compressions a bilevel page may have here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Bilevel {
    None,
    PackBits,
    Lzw,
    Deflate,
    Group4,
}

impl Bilevel {
    /// The compression of TIFF tag 259, when a bilevel page may have it.
    pub(super) fn of(compression: u64) -> Option<Self> {
        Some(match compression {
            1 => Self::None,
            32773 => Self::PackBits,
            5 => Self::Lzw,
            8 | 32946 => Self::Deflate,
            4 => Self::Group4,
            _ => return None,
        })
    }
}

/// Decodes a bilevel page: `expected` bytes, one per pixel, each the stored
/// bit (0 or 1). `FillOrder` 2 (`reverse`) reverses the bits of every byte
/// of every strip or tile before it is decompressed, as libtiff writes and
/// reads it. The stored value is kept for WhiteIsZero too.
pub(super) fn decode_bilevel(
    reader: &mut Reader<'_>,
    layout: &Layout,
    compression: Bilevel,
    reverse: bool,
    expected: u64,
) -> Result<Vec<u8>> {
    ensure!(
        expected == u64::from(layout.width) * u64::from(layout.height),
        "TIFF bilevel frame size differs from catalog"
    );
    let width = layout.width as usize;
    let mut frame = vec![0_u8; usize::try_from(expected)?];
    // One packed row of a strip or tile: at most the image's width plus the
    // tile margin, in bits.
    let mut row = vec![0_u8; usize::try_from(layout.chunk_width.div_ceil(8))?];
    layout.each_chunk(reader, reverse, |chunk, input| {
        let mut rows = frame[(chunk.top as usize) * width..]
            .chunks_exact_mut(width)
            .take(chunk.rows as usize)
            .map(|line| &mut line[chunk.left as usize..(chunk.left + chunk.columns) as usize]);
        let Some(mut packed) = Packed::new(compression, input) else {
            return group4(input, layout.chunk_width, chunk.rows, &mut rows);
        };
        for line in rows {
            packed.fill(&mut row)?;
            for (column, pixel) in line.iter_mut().enumerate() {
                *pixel = (row[column / 8] >> (7 - column % 8)) & 1;
            }
        }
        Ok(())
    })?;
    Ok(frame)
}

/// The packed rows of a strip or tile compressed with something other than
/// Group 4, one row at a time.
enum Packed<'i, 'a, 'b> {
    None(&'i mut ChunkInput<'a, 'b>),
    PackBits(PackBits<'i, 'a, 'b>),
    Lzw(&'i mut ChunkInput<'a, 'b>, weezl::decode::Decoder),
    Deflate(flate2::bufread::ZlibDecoder<&'i mut ChunkInput<'a, 'b>>),
}

impl<'i, 'a, 'b> Packed<'i, 'a, 'b> {
    /// `None` for Group 4, which is decoded line by line ([`group4`]).
    fn new(compression: Bilevel, input: &'i mut ChunkInput<'a, 'b>) -> Option<Self> {
        Some(match compression {
            Bilevel::Group4 => return None,
            Bilevel::None => Self::None(input),
            Bilevel::PackBits => Self::PackBits(PackBits {
                input,
                literal: 0,
                repeat: 0,
                value: 0,
            }),
            Bilevel::Lzw => Self::Lzw(
                input,
                weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8),
            ),
            Bilevel::Deflate => Self::Deflate(flate2::bufread::ZlibDecoder::new(input)),
        })
    }

    /// Fills `row` with the next packed row.
    fn fill(&mut self, row: &mut [u8]) -> Result<()> {
        match self {
            Self::None(input) => input
                .read_exact(row)
                .context("the uncompressed data ends early"),
            Self::PackBits(packbits) => packbits.fill(row),
            Self::Lzw(input, decoder) => {
                let mut filled = 0;
                while filled < row.len() {
                    let available = input.fill_buf()?;
                    let result = decoder.decode_bytes(available, &mut row[filled..]);
                    input.consume(result.consumed_in);
                    filled += result.consumed_out;
                    match result.status {
                        Ok(weezl::LzwStatus::Ok) => {}
                        Ok(weezl::LzwStatus::Done | weezl::LzwStatus::NoProgress) => {
                            ensure!(filled == row.len(), "the LZW data ends early");
                        }
                        Err(_) => bail!("the LZW data is corrupt"),
                    }
                }
                Ok(())
            }
            Self::Deflate(decoder) => decoder
                .read_exact(row)
                .context("the Deflate data is corrupt or ends early"),
        }
    }
}

/// A PackBits stream, whose runs may continue from one row into the next.
struct PackBits<'i, 'a, 'b> {
    input: &'i mut ChunkInput<'a, 'b>,
    /// Bytes still to copy of the current literal run.
    literal: usize,
    /// Times still to write `value` of the current repeat run.
    repeat: usize,
    value: u8,
}

impl PackBits<'_, '_, '_> {
    fn byte(&mut self) -> Result<u8> {
        let mut byte = [0];
        self.input
            .read_exact(&mut byte)
            .context("the PackBits data ends early")?;
        Ok(byte[0])
    }

    fn fill(&mut self, row: &mut [u8]) -> Result<()> {
        let mut filled = 0;
        while filled < row.len() {
            if self.literal > 0 {
                let count = self.literal.min(row.len() - filled);
                self.input
                    .read_exact(&mut row[filled..filled + count])
                    .context("the PackBits data ends early")?;
                self.literal -= count;
                filled += count;
            } else if self.repeat > 0 {
                let count = self.repeat.min(row.len() - filled);
                row[filled..filled + count].fill(self.value);
                self.repeat -= count;
                filled += count;
            } else {
                let header = self.byte()? as i8;
                match header {
                    0..=127 => self.literal = header as usize + 1,
                    -127..=-1 => {
                        self.value = self.byte()?;
                        self.repeat = 1 + header.unsigned_abs() as usize;
                    }
                    // -128 is a no-op.
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Decodes `rows` lines of a Group 4 strip or tile `chunk_width` pixels
/// across, writing each line's pixels inside the image to the next of
/// `lines`. White runs are stored as 0 and black runs as 1, as libtiff
/// stores them.
///
/// Two limits are enforced here. A line may consume at most
/// [`group4_line_bytes`] of the chunk (no honest line comes near it), and a
/// decoded line may have at most `chunk_width + 1` transitions; one with
/// more is an error. What a line holds while it is decoded is the `fax`
/// decoder's own: version 0.2.7, which `Cargo.toml` pins, adds at most one
/// transition for each bit it reads, so with the first limit a line holds
/// a few MiB at most at a width of 65,535. A newer `fax` must be checked
/// for the same before the pin moves.
fn group4<'l>(
    input: &mut ChunkInput<'_, '_>,
    chunk_width: u64,
    rows: u64,
    lines: &mut impl Iterator<Item = &'l mut [u8]>,
) -> Result<()> {
    let width = u16::try_from(chunk_width)
        .ok()
        .context("a Group 4 strip or tile is wider than 65,535 pixels")?;
    let allowance = Cell::new(group4_line_bytes(chunk_width));
    let failure = Cell::new(None);
    let bytes = Group4Bytes {
        input,
        allowance: &allowance,
        failure: &failure,
    };
    let mut decoder = fax::decoder::Group4Decoder::new(bytes, width)
        .map_err(|_| anyhow::anyhow!("the Group 4 data could not be read"))?;
    for _ in 0..rows {
        allowance.set(group4_line_bytes(chunk_width));
        let status = decoder.advance();
        if let Some(reason) = failure.take() {
            bail!("{reason}");
        }
        match status {
            Ok(fax::decoder::DecodeStatus::Incomplete) => {}
            Ok(fax::decoder::DecodeStatus::End) => bail!("the Group 4 data ends early"),
            Err(_) => bail!("the Group 4 data is corrupt or ends early"),
        }
        let transitions = decoder.transition();
        ensure!(
            transitions.len() as u64 <= chunk_width + 1,
            "a Group 4 line has more transitions than its width allows"
        );
        let line = lines.next().context("missing TIFF frame row")?;
        let mut black = false;
        let mut from = 0_usize;
        for &position in transitions.iter().chain(std::iter::once(&width)) {
            let to = usize::from(position).min(line.len()).max(from);
            line[from..to].fill(u8::from(black));
            from = to;
            black = !black;
        }
    }
    Ok(())
}

/// The most bytes one Group 4 line `chunk_width` pixels across may take of
/// its strip or tile: two for each pixel and 64 more. A line costs at most
/// seven bits for each change of colour, four for each pass over the
/// reference line (at most one for every two pixels) and its runs' make-up
/// codes, so no honest line reaches half of this.
fn group4_line_bytes(chunk_width: u64) -> u64 {
    2 * (chunk_width + 1) + 64
}

/// The bytes of a Group 4 strip or tile as the decoder takes them, one at a
/// time, up to the allowance of the line being decoded.
struct Group4Bytes<'i, 'a, 'b> {
    input: &'i mut ChunkInput<'a, 'b>,
    allowance: &'i Cell<u64>,
    failure: &'i Cell<Option<&'static str>>,
}

impl Iterator for Group4Bytes<'_, '_, '_> {
    type Item = Result<u8, ()>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failure.get().is_some() {
            return None;
        }
        if self.allowance.get() == 0 {
            self.failure
                .set(Some("a Group 4 line is longer than its width allows"));
            return Some(Err(()));
        }
        match self.input.fill_buf() {
            Ok([]) => None,
            Ok(available) => {
                let byte = available[0];
                self.input.consume(1);
                self.allowance.set(self.allowance.get() - 1);
                Some(Ok(byte))
            }
            Err(_) => {
                self.failure
                    .set(Some("the Group 4 data could not be read from the file"));
                Some(Err(()))
            }
        }
    }
}

/// The largest frame header a JPEG strip or tile may declare in either
/// direction: the 16 bits of its height and width.
const JPEG_MAX_SIDE: u64 = u16::MAX as u64;

/// What a JPEG-compressed page needs beside its layout.
pub(super) struct Jpeg {
    /// `JPEGTables` (tag 347): the quantization and Huffman tables its
    /// strips or tiles share, from SOI to EOI, when the page has them.
    pub tables: Option<Vec<u8>>,
    /// `SamplesPerPixel`: 1 or 3.
    pub samples: u64,
    /// `PhotometricInterpretation` 6: the stream's YCbCr is converted to
    /// RGB. Gray and RGB are served as the stream stores them.
    pub ycbcr: bool,
    /// The most bytes one strip or tile may decode to
    /// (`raster_tiff_jpeg_chunk_bytes`).
    pub chunk_bytes: u64,
    /// The file's length: no strip or tile is buffered at more than the
    /// bytes of it the file holds.
    pub file_length: u64,
}

/// Decodes a JPEG-compressed page: `expected` bytes, `samples` a pixel.
///
/// Each strip or tile is one JPEG stream. When the page has `JPEGTables`,
/// the stream is those tables without their EOI followed by the strip's or
/// tile's bytes without their SOI, as libtiff joins them. Before the
/// decoder is given a stream, its frame header must declare 8-bit samples,
/// `samples` components sampled as [`frame_header`] allows, and the strip's
/// or tile's own size: a tile's width and length, a strip's width and
/// `RowsPerStrip`, or for the last strip the rows that remain (as libtiff
/// writes it). A strip taller than the image by the tile margin or more
/// may declare only the image's rows. The decoder is also told that size
/// as its limit, and what it reports after reading the headers is compared
/// again. So no stream is decoded at any size but the one the page's
/// layout gives.
///
/// Beside the frame this holds the bytes of one strip or tile and the
/// tables (at most the file's length), one decoded strip or tile (at most
/// `chunk_bytes`), and what `zune-jpeg` holds while it decodes one; its
/// bound is stated at `raster_decode_heap_limit`.
pub(super) fn decode_jpeg(
    reader: &mut Reader<'_>,
    layout: &Layout,
    jpeg: &Jpeg,
    expected: u64,
) -> Result<Vec<u8>> {
    let samples = jpeg.samples;
    ensure!(
        expected == u64::from(layout.width) * u64::from(layout.height) * samples,
        "TIFF JPEG frame size differs from catalog"
    );
    let tables = match &jpeg.tables {
        Some(tables) => {
            ensure!(
                tables.len() >= 4
                    && tables[..2] == [0xFF, 0xD8]
                    && tables[tables.len() - 2..] == [0xFF, 0xD9],
                "the TIFF JPEG tables are not one JPEG stream"
            );
            &tables[..tables.len() - 2]
        }
        None => &[][..],
    };
    // The height every frame header but the last strip's must declare: a
    // tile's length, or `RowsPerStrip`. A strip as tall as the image or
    // taller is the only one, and may declare the image's rows; it may
    // declare `RowsPerStrip` too only when that is within the tile margin,
    // so no strip or tile is decoded taller than the image and the margin.
    let tallest = if layout.tiled
        || layout.chunk_height < u64::from(layout.height) + u64::from(RASTER_TIFF_TILE_MARGIN)
    {
        layout.chunk_height
    } else {
        u64::from(layout.height)
    };
    ensure!(
        layout.chunk_width <= JPEG_MAX_SIDE && tallest <= JPEG_MAX_SIDE,
        "a TIFF JPEG strip or tile is larger than a JPEG frame can be"
    );
    let largest = layout.chunk_width * tallest * samples;
    ensure!(
        largest <= jpeg.chunk_bytes,
        "a TIFF JPEG strip or tile decodes to more than {} bytes",
        jpeg.chunk_bytes
    );
    // The bytes of the longest strip or tile the file holds, joined to the
    // tables: never more than exist.
    let longest = layout
        .offsets
        .iter()
        .zip(&layout.counts)
        .map(|(&offset, &count)| count.min(jpeg.file_length.saturating_sub(offset)))
        .max()
        .unwrap_or(0);
    let mut stream = Vec::with_capacity(usize::try_from(tables.len() as u64 + longest)?);
    let mut decoded = vec![0_u8; usize::try_from(largest)?];
    let mut frame = vec![0_u8; usize::try_from(expected)?];
    let row_bytes = (u64::from(layout.width) * samples) as usize;
    layout.each_chunk(reader, false, |chunk, input| {
        stream.clear();
        stream.extend_from_slice(tables);
        if !tables.is_empty() {
            let mut start = [0; 2];
            input
                .read_exact(&mut start)
                .context("the JPEG data ends early")?;
            ensure!(start == [0xFF, 0xD8], "the JPEG data has no start of image");
        }
        input
            .read_to_end(&mut stream)
            .context("the JPEG data could not be read from the file")?;
        let header = frame_header(&stream, jpeg.ycbcr)?;
        let height = u64::from(header.height);
        ensure!(
            u64::from(header.width) == layout.chunk_width
                && (height == tallest || (!layout.tiled && height == chunk.rows)),
            "a JPEG frame header declares {} x {}, not the size of its {}",
            header.width,
            header.height,
            if layout.tiled { "tile" } else { "strip" }
        );
        ensure!(
            header.precision == 8,
            "a JPEG frame header declares {}-bit samples",
            header.precision
        );
        ensure!(
            u64::from(header.components) == samples,
            "a JPEG frame header declares {} components for {samples} samples",
            header.components
        );
        let size = (layout.chunk_width * height * samples) as usize;
        let mut options = DecoderOptions::default()
            .set_strict_mode(false)
            .set_max_width(layout.chunk_width as usize)
            .set_max_height(tallest as usize)
            .jpeg_set_max_scans(RASTER_JPEG_MAX_SCANS)
            .jpeg_set_out_colorspace(if samples == 1 {
                ColorSpace::Luma
            } else {
                ColorSpace::RGB
            });
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(stream.as_slice()), options);
        decoder
            .decode_headers()
            .context("the JPEG decoder refused the strip or tile")?;
        if !jpeg.ycbcr && samples == 3 {
            // RGB is served as stored, unconverted, as libtiff decodes an
            // RGB page: the output is whatever the decoder takes the stream
            // to hold. zune-jpeg 0.5 takes a stream of three components to
            // be YCbCr unless its Adobe marker says it is untransformed,
            // which it reports as CMYK until it decodes it, and then as
            // RGB.
            let stored = match decoder.input_colorspace() {
                Some(ColorSpace::CMYK | ColorSpace::RGB) => ColorSpace::RGB,
                _ => ColorSpace::YCbCr,
            };
            options = options.jpeg_set_out_colorspace(stored);
            decoder.set_options(options);
        }
        let info = decoder.info().context("missing JPEG frame header")?;
        // `pixel_density` is the sample precision the decoder read.
        ensure!(
            (info.width, info.height, info.components, info.pixel_density)
                == (header.width, header.height, header.components, 8)
                && decoder.output_buffer_size() == Some(size),
            "the JPEG decoder read another frame header"
        );
        decoder
            .decode_into(&mut decoded[..size])
            .context("the JPEG decoder could not decode the strip or tile")?;
        let left = (chunk.left * samples) as usize;
        let across = (chunk.columns * samples) as usize;
        let stride = (layout.chunk_width * samples) as usize;
        for (row, line) in decoded[..size]
            .chunks_exact(stride)
            .take(chunk.rows as usize)
            .enumerate()
        {
            let at = (chunk.top as usize + row) * row_bytes + left;
            frame[at..at + across].copy_from_slice(&line[..across]);
        }
        Ok(())
    })?;
    Ok(frame)
}

/// What the frame header (SOF) of a JPEG stream declares.
#[derive(Debug, PartialEq, Eq)]
struct FrameHeader {
    precision: u8,
    height: u16,
    width: u16,
    components: u8,
}

/// Reads the frame header of `stream`, the first SOF marker segment after
/// its SOI, stepping over the segments before it by their lengths.
///
/// As libtiff requires, only the luma of a YCbCr page (`ycbcr`) may be
/// sampled more often than the other components; it may be sampled 1, 2 or
/// 4 times in each direction, the factors `YCbCrSubSampling` may have.
/// Every other component is sampled once. So a decoder's unit of decoding
/// is at most 32 pixels across and down.
fn frame_header(stream: &[u8], ycbcr: bool) -> Result<FrameHeader> {
    ensure!(
        stream.starts_with(&[0xFF, 0xD8]),
        "the JPEG data has no start of image"
    );
    let byte = |at: usize| {
        stream
            .get(at)
            .copied()
            .context("the JPEG data ends before its frame header")
    };
    let mut at = 2;
    loop {
        ensure!(byte(at)? == 0xFF, "the JPEG data has no frame header");
        let mut marker = byte(at + 1)?;
        while marker == 0xFF {
            at += 1;
            marker = byte(at + 1)?;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            at += 2;
            continue;
        }
        ensure!(
            !matches!(marker, 0xD8..=0xDA),
            "the JPEG data has no frame header"
        );
        let length = usize::from(byte(at + 2)?) << 8 | usize::from(byte(at + 3)?);
        ensure!(length >= 2, "a JPEG marker segment is too short");
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let components = byte(at + 9)?;
            ensure!(
                length == 8 + 3 * usize::from(components),
                "the JPEG frame header has the wrong length"
            );
            for component in 0..usize::from(components) {
                let factors = byte(at + 11 + 3 * component)?;
                let allowed = if ycbcr && component == 0 {
                    matches!(factors >> 4, 1 | 2 | 4) && matches!(factors & 15, 1 | 2 | 4)
                } else {
                    factors == 0x11
                };
                ensure!(
                    allowed,
                    "a JPEG component is sampled other than its page allows"
                );
            }
            return Ok(FrameHeader {
                precision: byte(at + 4)?,
                height: u16::from(byte(at + 5)?) << 8 | u16::from(byte(at + 6)?),
                width: u16::from(byte(at + 7)?) << 8 | u16::from(byte(at + 8)?),
                components,
            });
        }
        at += 2 + length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_runs_continue_across_rows() {
        // A literal of three bytes, a repeat of four, and a no-op, read as
        // rows of two bytes.
        let data = [2_u8, 1, 2, 3, 0xFD, 9, 0x80];
        let mut source = io::Cursor::new(data.to_vec());
        let mut reader = Reader::new(&mut source, data.len() as u64, 1 << 20);
        let mut buffer = vec![0; INPUT_BUFFER_BYTES];
        let mut input = ChunkInput {
            reader: &mut reader,
            left: data.len() as u64,
            reverse: false,
            buffer: &mut buffer,
            start: 0,
            end: 0,
        };
        let mut packbits = PackBits {
            input: &mut input,
            literal: 0,
            repeat: 0,
            value: 0,
        };
        let mut rows = Vec::new();
        for _ in 0..3 {
            let mut row = [0; 2];
            packbits.fill(&mut row).expect("row");
            rows.push(row);
        }
        assert_eq!(rows, [[1, 2], [3, 9], [9, 9]]);
        let mut row = [0; 2];
        assert!(packbits.fill(&mut row).is_err());
    }

    #[test]
    fn chunk_input_stops_at_its_count_and_reverses_bits() {
        let data = [0b1000_0001_u8, 0b1100_0000, 0xFF, 0xFF];
        let mut source = io::Cursor::new(data.to_vec());
        let mut reader = Reader::new(&mut source, data.len() as u64, 1 << 20);
        let mut buffer = vec![0; INPUT_BUFFER_BYTES];
        let mut input = ChunkInput {
            reader: &mut reader,
            left: 2,
            reverse: true,
            buffer: &mut buffer,
            start: 0,
            end: 0,
        };
        let mut out = Vec::new();
        input.read_to_end(&mut out).expect("read");
        assert_eq!(out, [0b1000_0001, 0b0000_0011]);
    }

    #[test]
    fn layout_clips_edge_tiles_to_the_image() {
        let layout = Layout {
            width: 37,
            height: 21,
            chunk_width: 16,
            chunk_height: 16,
            tiled: true,
            offsets: vec![0; 6],
            counts: vec![0; 6],
        };
        let edge = layout.chunk(5);
        assert_eq!(
            (edge.left, edge.top, edge.columns, edge.rows),
            (32, 16, 5, 5)
        );
    }

    #[test]
    fn frame_header_is_read_after_the_tables() {
        // SOI, a DQT segment of two bytes' payload, a fill byte, then a
        // SOF0 of one component sampled 2 x 1.
        let stream = [
            0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xFF, 0xC0, 0x00, 0x0B, 0x08,
            0x00, 0x10, 0x00, 0x20, 0x01, 0x01, 0x21, 0x00,
        ];
        // Only the luma of a YCbCr page may be sampled more than once.
        assert!(frame_header(&stream, false).is_err());
        assert_eq!(
            frame_header(&stream, true).expect("frame header"),
            FrameHeader {
                precision: 8,
                height: 16,
                width: 32,
                components: 1
            }
        );
        let mut sampled = stream;
        sampled[20] = 0x81;
        assert!(frame_header(&sampled, true).is_err());
        assert!(frame_header(&stream[..stream.len() - 2], true).is_err());
        // A scan before any frame header.
        assert!(frame_header(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02], true).is_err());
    }
}
