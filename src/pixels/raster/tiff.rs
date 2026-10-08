//! Read one recorded IFD, then present that page as the decoder's first page.
use super::{
    checked_profile, reader::Reader, PixelError, PixelResult, RasterFrame, RASTER_ICC_MAX_BYTES,
    RASTER_TIFF_MAX_CHUNKS, RASTER_TIFF_MAX_TAGS,
};
use crate::api::contracts::RasterSampleFormat;
use crate::types::FileEntry;
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use tiff::decoder::{Decoder, DecodingResult, Limits};

#[derive(Clone, Copy)]
struct Order {
    little: bool,
}
impl Order {
    fn number(self, bytes: &[u8]) -> u64 {
        if self.little {
            bytes.iter().rev().fold(0, |n, &b| (n << 8) | u64::from(b))
        } else {
            bytes.iter().fold(0, |n, &b| (n << 8) | u64::from(b))
        }
    }
    fn bytes(self, value: u64, size: usize) -> Vec<u8> {
        if self.little {
            value.to_le_bytes()[..size].to_vec()
        } else {
            value.to_be_bytes()[8 - size..].to_vec()
        }
    }
}

struct Entry {
    tag: u16,
    kind: u16,
    count: u64,
    value: [u8; 8],
    position: u64,
}

struct Page {
    order: Order,
    wide: bool,
    entries: Vec<Entry>,
}

impl Page {
    fn read(reader: &mut Reader<'_>, offset: u64, length: u64) -> Result<Self> {
        let mut header = [0; 8];
        reader.read_exact(&mut header)?;
        ensure!(
            &header[..2] == b"II" || &header[..2] == b"MM",
            "invalid TIFF byte order"
        );
        let order = Order {
            little: &header[..2] == b"II",
        };
        let version = order.number(&header[2..4]);
        ensure!(version == 42 || version == 43, "invalid TIFF version");
        let wide = version == 43;
        if wide {
            ensure!(
                order.number(&header[4..6]) == 8 && order.number(&header[6..8]) == 0,
                "invalid BigTIFF header"
            );
        }
        let count_size = if wide { 8 } else { 2 };
        ensure!(
            offset <= length && length - offset >= count_size,
            "TIFF page outside file"
        );
        reader.seek(SeekFrom::Start(offset))?;
        let mut count = [0; 8];
        reader.read_exact(&mut count[..count_size as usize])?;
        let count = order.number(&count[..count_size as usize]);
        ensure!(
            count <= RASTER_TIFF_MAX_TAGS as u64,
            "TIFF tag count exceeds limit"
        );
        let size = if wide { 20 } else { 12 };
        ensure!(
            count * size + if wide { 8 } else { 4 } <= length - offset - count_size,
            "truncated TIFF page"
        );
        let mut entries = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut raw = [0; 20];
            reader.read_exact(&mut raw[..size as usize])?;
            let tag = order.number(&raw[..2]) as u16;
            let kind = order.number(&raw[2..4]) as u16;
            let value_start = if wide { 12 } else { 8 };
            let count = order.number(&raw[4..value_start]);
            if matches!(tag, 254 | 256..=259 | 262 | 273 | 277..=279 | 284 | 317 | 322..=325 | 338 | 339)
            {
                ensure!(
                    count <= RASTER_TIFF_MAX_CHUNKS as u64,
                    "TIFF layout tag count exceeds limit"
                );
            }
            let mut value = [0; 8];
            value[..size as usize - value_start].copy_from_slice(&raw[value_start..size as usize]);
            entries.push(Entry {
                tag,
                kind,
                count,
                value,
                position: offset + count_size + index * size + value_start as u64,
            });
        }
        Ok(Self {
            order,
            wide,
            entries,
        })
    }

    fn entry(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }

    fn values(
        &self,
        reader: &mut Reader<'_>,
        tag: u16,
        default: u64,
        length: u64,
    ) -> Result<Vec<u64>> {
        let Some(entry) = self.entry(tag) else {
            return Ok(vec![default]);
        };
        let size = match entry.kind {
            1 => 1,
            3 => 2,
            4 => 4,
            16 => 8,
            _ => bail!("invalid TIFF layout tag type"),
        };
        ensure!(
            entry.count > 0 && entry.count <= RASTER_TIFF_MAX_CHUNKS as u64,
            "invalid TIFF layout tag count"
        );
        let bytes = entry.count * size;
        let inline = if self.wide { 8 } else { 4 };
        if bytes <= inline {
            return Ok(entry.value[..bytes as usize]
                .chunks_exact(size as usize)
                .map(|value| self.order.number(value))
                .collect());
        }
        let offset = self.order.number(&entry.value[..inline as usize]);
        ensure!(
            offset <= length && bytes <= length - offset,
            "TIFF tag value outside file"
        );
        reader.seek(SeekFrom::Start(offset))?;
        let mut values = Vec::with_capacity(entry.count as usize);
        for _ in 0..entry.count {
            let mut value = [0; 8];
            reader.read_exact(&mut value[..size as usize])?;
            values.push(self.order.number(&value[..size as usize]));
        }
        Ok(values)
    }

    fn scalar(&self, reader: &mut Reader<'_>, tag: u16, default: u64, length: u64) -> Result<u64> {
        let values = self.values(reader, tag, default, length)?;
        ensure!(values.len() == 1, "TIFF scalar tag has multiple values");
        Ok(values[0])
    }

    fn profile(&self, reader: &mut Reader<'_>, length: u64) -> Result<Option<Vec<u8>>> {
        let Some(entry) = self.entry(34675) else {
            return Ok(None);
        };
        if !matches!(entry.kind, 1 | 7) || entry.count > RASTER_ICC_MAX_BYTES as u64 {
            return Ok(None);
        }
        let inline = if self.wide { 8 } else { 4 };
        if entry.count <= inline {
            return Ok(Some(entry.value[..entry.count as usize].to_vec()));
        }
        let offset = self.order.number(&entry.value[..inline as usize]);
        if offset > length || entry.count > length - offset {
            return Ok(None);
        }
        reader.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; entry.count as usize];
        reader.read_exact(&mut bytes)?;
        Ok(Some(bytes))
    }
}

struct Patch {
    offset: u64,
    bytes: Vec<u8>,
}
struct PageView<'a, 'b> {
    reader: &'a mut Reader<'b>,
    patches: Vec<Patch>,
    /// Where each strip or tile begins, and where the bytes its byte count
    /// gives it end.
    chunks: HashMap<u64, u64>,
    /// The end of the strip or tile being read: set by a seek to its first
    /// byte, and the end of what that read may be given. The crate's Deflate
    /// reader takes no length, and would read on into whatever follows.
    chunk_end: Option<u64>,
}
impl Read for PageView<'_, '_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let start = self.reader.stream_position()?;
        let most = match self.chunk_end {
            Some(end) => (end.saturating_sub(start)).min(output.len() as u64) as usize,
            None => output.len(),
        };
        let output = &mut output[..most];
        let count = self.reader.read(output)?;
        for patch in &self.patches {
            let low = start.max(patch.offset);
            let high = (start + count as u64).min(patch.offset + patch.bytes.len() as u64);
            if low < high {
                output[(low - start) as usize..(high - start) as usize].copy_from_slice(
                    &patch.bytes[(low - patch.offset) as usize..(high - patch.offset) as usize],
                );
            }
        }
        Ok(count)
    }
}
impl Seek for PageView<'_, '_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = self.reader.seek(position)?;
        self.chunk_end = self.chunks.get(&position).copied();
        Ok(position)
    }
}

fn unsupported(reason: &str) -> anyhow::Error {
    PixelError::UnsupportedLayout(reason.into()).into()
}

pub(super) fn decode(
    file: &FileEntry,
    frame: u32,
    reader: &mut Reader<'_>,
    length: u64,
    expected: u64,
    budget: u64,
) -> PixelResult<RasterFrame> {
    decode_page(file, frame, reader, length, expected, budget).map_err(|error| {
        match error.downcast::<PixelError>() {
            Ok(error) => error,
            Err(error) => PixelError::frame_decode(error),
        }
    })
}

fn decode_page(
    file: &FileEntry,
    frame: u32,
    reader: &mut Reader<'_>,
    length: u64,
    expected: u64,
    budget: u64,
) -> Result<RasterFrame> {
    let raster = file.raster.as_ref().context("missing raster metadata")?;
    let offset = *raster
        .frame_offsets
        .get(frame as usize)
        .context("missing TIFF frame offset")?;
    let page = Page::read(reader, offset, length)?;
    let compression = page.scalar(reader, 259, 1, length)?;
    if !matches!(compression, 1 | 5 | 8 | 32946 | 32773) {
        return Err(unsupported("raster.unsupported_compression"));
    }
    let samples = page.scalar(reader, 277, 1, length)?;
    let planar = page.scalar(reader, 284, 1, length)?;
    let photometric = page.scalar(reader, 262, u64::MAX, length)?;
    let alpha = page.values(reader, 338, 0, length)?;
    if (samples > 1 && planar == 2)
        || !matches!((photometric, samples), (0 | 1, 1) | (2, 3 | 4))
        || (samples == 4 && (alpha.len() != 1 || !matches!(alpha[0], 1 | 2)))
    {
        return Err(unsupported("raster.unsupported_color"));
    }
    let depths = page.values(reader, 258, 1, length)?;
    let formats = page.values(reader, 339, 1, length)?;
    let depth = depths[0];
    let format = formats[0];
    if !depths.iter().all(|&d| d == depth)
        || !formats.iter().all(|&f| f == format)
        || !matches!(
            (samples, format, depth),
            (1, 1 | 2, 8 | 16 | 32) | (1, 3, 32 | 64) | (3 | 4, 1, 8 | 16)
        )
    {
        return Err(unsupported("raster.unsupported_sample_format"));
    }
    let expected_format = match raster.sample_format {
        RasterSampleFormat::Uint => 1,
        RasterSampleFormat::Int => 2,
        RasterSampleFormat::Float => 3,
    };
    let expected_photometric = match file.photometric_interpretation.as_str() {
        "MONOCHROME1" => 0,
        "MONOCHROME2" => 1,
        "RGB" | "RGBA" => 2,
        _ => u64::MAX,
    };
    ensure!(
        page.scalar(reader, 256, 0, length)? == u64::from(file.columns)
            && page.scalar(reader, 257, 0, length)? == u64::from(file.rows)
            && samples == u64::from(file.samples_per_pixel)
            && depth == u64::from(file.bits_allocated)
            && depth == u64::from(raster.bit_depth)
            && format == expected_format
            && photometric == expected_photometric
            && (samples != 4 || (alpha[0] == 1) == raster.alpha_associated),
        "TIFF page differs from catalog"
    );
    // read_image iterates the offset table, leaving missing chunks as zeros.
    // Require complete coverage rather than accepting a partially filled frame.
    let (offset_tag, count_tag, chunks) = if page.entry(324).is_some() {
        let width = page.scalar(reader, 322, 0, length)?;
        let height = page.scalar(reader, 323, 0, length)?;
        ensure!(width > 0 && height > 0, "invalid TIFF tile dimensions");
        if page.scalar(reader, 317, 1, length)? == 3 {
            // tiff 0.9 allocates this padded predictor row without consulting
            // Limits. Leave room in the fixed heap allowance for codec state
            // and the retained profile, independently of declared tile width.
            let row = width
                .checked_mul(samples)
                .and_then(|n| n.checked_mul(depth / 8))
                .context("TIFF predictor row size overflow")?;
            ensure!(
                row <= expected + RASTER_ICC_MAX_BYTES as u64,
                "TIFF predictor row exceeds heap allowance"
            );
        }
        (
            324,
            325,
            u64::from(file.columns).div_ceil(width) * u64::from(file.rows).div_ceil(height),
        )
    } else {
        let rows = page.scalar(reader, 278, u64::from(file.rows), length)?;
        ensure!(rows > 0, "invalid TIFF strip height");
        (273, 279, u64::from(file.rows).div_ceil(rows))
    };
    ensure!(
        page.entry(offset_tag)
            .is_some_and(|entry| entry.count == chunks)
            && page
                .entry(count_tag)
                .is_some_and(|entry| entry.count == chunks),
        "TIFF chunks do not cover the frame"
    );
    // From here on the file is read chunk by chunk, in index order. Tell the
    // reader where the chunks lie, so one stored out of that order is read
    // for its own bytes and not for a buffer of its neighbours'.
    let offsets = page.values(reader, offset_tag, 0, length)?;
    let counts = page.values(reader, count_tag, 0, length)?;
    // No chunk is read past its byte count (`PageView`), and none may claim
    // more than the whole decode may read.
    ensure!(
        counts.iter().all(|&count| count <= budget),
        "TIFF chunk exceeds read budget"
    );
    reader.read_spans(chunk_runs(&offsets, &counts));
    // Chunks that share a first byte are read to the longest of them.
    let mut chunk_ends = HashMap::with_capacity(offsets.len());
    for (&start, &count) in offsets.iter().zip(&counts) {
        let end = chunk_ends.entry(start).or_insert(start);
        *end = (*end).max(start.saturating_add(count));
    }
    let profile = page.profile(reader, length)?;
    let icc_profile = checked_profile(file, profile.as_deref());
    drop(profile);
    let mut patches = vec![Patch {
        offset: if page.wide { 8 } else { 4 },
        bytes: page.order.bytes(offset, if page.wide { 8 } else { 4 }),
    }];
    if photometric == 0 {
        let entry = page.entry(262).context("missing TIFF photometric")?;
        ensure!(
            entry.kind == 3 && entry.count == 1,
            "TIFF photometric is not one SHORT"
        );
        patches.push(Patch {
            offset: entry.position,
            bytes: page.order.bytes(1, 2),
        });
    }
    reader.seek(SeekFrom::Start(0))?;
    let view = PageView {
        reader,
        patches,
        chunks: chunk_ends,
        chunk_end: None,
    };
    let mut limits = Limits::default();
    limits.decoding_buffer_size = usize::try_from(expected)?;
    limits.intermediate_buffer_size = usize::try_from(budget)?;
    let mut decoder = Decoder::new(view)?.with_limits(limits);
    let color = match samples {
        1 => tiff::ColorType::Gray(depth as u8),
        3 => tiff::ColorType::RGB(depth as u8),
        4 => tiff::ColorType::RGBA(depth as u8),
        _ => unreachable!(),
    };
    ensure!(
        decoder.dimensions()? == (file.columns, file.rows) && decoder.colortype()? == color,
        "TIFF decoded layout differs from catalog"
    );
    let decoded = decoder.read_image()?;
    let mut bytes = Vec::with_capacity(expected as usize);
    macro_rules! write_samples {
        ($values:expr, $format:expr, $depth:expr) => {{
            ensure!(
                format == $format && depth == $depth,
                "TIFF decoded sample format differs from catalog"
            );
            for sample in $values {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }};
    }
    match decoded {
        DecodingResult::U8(values) => {
            ensure!(
                format == 1 && depth == 8,
                "TIFF decoded sample format differs from catalog"
            );
            bytes = values;
        }
        DecodingResult::I8(values) => write_samples!(values, 2, 8),
        DecodingResult::U16(values) => write_samples!(values, 1, 16),
        DecodingResult::I16(values) => write_samples!(values, 2, 16),
        DecodingResult::U32(values) => write_samples!(values, 1, 32),
        DecodingResult::I32(values) => write_samples!(values, 2, 32),
        DecodingResult::F32(values) => write_samples!(values, 3, 32),
        DecodingResult::F64(values) => write_samples!(values, 3, 64),
        _ => bail!("unsupported TIFF decoded sample type"),
    }
    if raster.alpha_associated {
        unassociate(&mut bytes, depth as usize);
    }
    Ok(RasterFrame { bytes, icc_profile })
}

/// The ranges of the file that hold a page's strips or tiles, a chunk that
/// begins where the one before it in index order ends joined to it: each
/// range is then read from start to end, once, in the order the chunks are
/// decoded.
fn chunk_runs(offsets: &[u64], counts: &[u64]) -> Vec<Range<u64>> {
    let mut runs: Vec<Range<u64>> = Vec::new();
    for (&start, &count) in offsets.iter().zip(counts) {
        let end = start.saturating_add(count);
        match runs.last_mut() {
            Some(run) if run.end == start => run.end = end,
            _ => runs.push(start..end),
        }
    }
    runs
}

fn unassociate(bytes: &mut [u8], depth: usize) {
    let size = depth / 8;
    let max = if depth == 8 { 255_u64 } else { 65535 };
    for pixel in bytes.chunks_exact_mut(4 * size) {
        let sample = |data: &[u8]| {
            if size == 1 {
                u64::from(data[0])
            } else {
                u64::from(u16::from_le_bytes([data[0], data[1]]))
            }
        };
        let alpha = sample(&pixel[3 * size..]);
        for color in pixel[..3 * size].chunks_exact_mut(size) {
            let value = if alpha == 0 {
                0
            } else {
                ((sample(color) * max + alpha / 2) / alpha).min(max)
            };
            color.copy_from_slice(&value.to_le_bytes()[..size]);
        }
    }
}
