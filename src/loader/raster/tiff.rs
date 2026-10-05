//! Read only IFD entries and the small sample-layout tag values. Strip, tile,
//! profile and description payloads are never read or allocated.
use super::{is_io_failure, orientation, Header, HeaderReader};
use crate::api::contracts::{
    RasterColorType, RasterExcludedPage, RasterPageDifference, RasterSampleFormat,
};
use crate::types::{RasterUnsupported, RASTER_EXCLUDED_PAGES_LISTED, RASTER_WARNINGS_MAX};
use anyhow::{ensure, Result};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy)]
struct Encoding {
    little: bool,
    big: bool,
}

impl Encoding {
    fn number(self, bytes: &[u8]) -> u64 {
        if self.little {
            bytes.iter().rev().fold(0, |n, b| (n << 8) | u64::from(*b))
        } else {
            bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b))
        }
    }
    fn offset_size(self) -> u64 {
        if self.big {
            8
        } else {
            4
        }
    }
}

struct Page {
    width: u32,
    height: u32,
    samples: u64,
    format: Vec<u64>,
    bits: Vec<u64>,
    photometric: u64,
    extra: Vec<u64>,
    orientation: u8,
    subfile: u64,
    icc: bool,
}

impl Page {
    fn difference(&self, first: &Self) -> Option<RasterPageDifference> {
        use RasterPageDifference::*;
        [
            (self.subfile & 1 != 0, ReducedResolution),
            (self.subfile & 4 != 0, Mask),
            (self.width != first.width, Width),
            (self.height != first.height, Height),
            (self.samples != first.samples, SamplesPerPixel),
            (self.format != first.format, SampleFormat),
            (self.bits != first.bits, BitsPerSample),
            (self.photometric != first.photometric, Photometric),
            (self.extra != first.extra, Alpha),
            (self.orientation != first.orientation, Orientation),
        ]
        .into_iter()
        .find_map(|(differs, reason)| differs.then_some(reason))
    }

    fn header(&self) -> Result<Header> {
        let depth = self.bits[0];
        ensure!(
            self.bits.iter().all(|v| *v == depth) && matches!(depth, 1 | 2 | 4 | 8 | 16 | 32 | 64),
            "unsupported TIFF depth"
        );
        let sample = self.format[0];
        ensure!(
            self.format.iter().all(|v| *v == sample),
            "mixed TIFF sample formats"
        );
        let sample_format = match sample {
            1 => RasterSampleFormat::Uint,
            2 => RasterSampleFormat::Int,
            3 => RasterSampleFormat::Float,
            _ => anyhow::bail!("unsupported TIFF sample format"),
        };
        let color = match (self.photometric, self.samples, self.extra.as_slice()) {
            (0 | 1, 1, []) => RasterColorType::Gray,
            (0 | 1, 2, [1 | 2]) => RasterColorType::GrayAlpha,
            (2 | 6, 3, []) => RasterColorType::Rgb,
            (2, 4, [1 | 2]) => RasterColorType::Rgba,
            (3, 1, []) => RasterColorType::Palette,
            (5, 4, []) => RasterColorType::Cmyk,
            _ => RasterColorType::Other,
        };
        let alpha = matches!(color, RasterColorType::GrayAlpha | RasterColorType::Rgba);
        let mut header = Header::new(self.width, self.height, color, depth as u32);
        header.white_is_zero = self.photometric == 0;
        header.stored_samples = self.samples;
        header.metadata.unsupported = if color == RasterColorType::Other {
            Some(RasterUnsupported::Color)
        } else if sample_format == RasterSampleFormat::Float && depth == 16 {
            Some(RasterUnsupported::SampleFormat)
        } else {
            None
        };
        header.metadata.sample_format = sample_format;
        header.metadata.has_alpha = alpha;
        header.metadata.alpha_associated = alpha && self.extra.contains(&1);
        header.metadata.orientation = self.orientation;
        header.metadata.has_icc = self.icc;
        Ok(header)
    }
}

pub(super) fn inspect(input: &mut HeaderReader) -> Result<Header> {
    let signature = input.read::<8>(0)?;
    let little = match &signature[..2] {
        b"II" => true,
        b"MM" => false,
        _ => anyhow::bail!("invalid TIFF byte order"),
    };
    let mut encoding = Encoding { little, big: false };
    let mut offset = match encoding.number(&signature[2..4]) {
        42 => encoding.number(&signature[4..8]),
        43 => {
            ensure!(
                encoding.number(&signature[4..6]) == 8 && encoding.number(&signature[6..8]) == 0,
                "invalid BigTIFF header"
            );
            encoding.big = true;
            encoding.number(&input.read::<8>(8)?)
        }
        _ => anyhow::bail!("invalid TIFF version"),
    };
    input.step()?;
    let (first, next) = read_page(input, encoding, offset)?;
    let mut header = first.header()?;
    // Keep exact cycle detection so a repeated page is never counted twice.
    // The step check precedes insertion, bounding this set to 65,535 offsets.
    let mut seen = HashSet::from([offset]);
    let mut notes = 0;
    offset = next;
    while offset != 0 {
        input.step()?;
        let page = header.metadata.pages_total;
        let result = if seen.insert(offset) {
            read_page(input, encoding, offset)
        } else {
            Err(anyhow::anyhow!("cyclic TIFF IFD chain"))
        };
        let (current, next) = match result {
            Ok(result) => result,
            Err(error) if input.exhausted.is_some() || is_io_failure(&error) => return Err(error),
            Err(error) => {
                note(
                    &mut header,
                    &mut notes,
                    format!("TIFF page walk stopped at page {page}: {error}"),
                );
                break;
            }
        };
        if let Some(differs) = current.difference(&first) {
            header.metadata.excluded_pages_total += 1;
            if header.metadata.excluded_pages.len() < RASTER_EXCLUDED_PAGES_LISTED {
                header
                    .metadata
                    .excluded_pages
                    .push(RasterExcludedPage { page, differs });
            }
        } else {
            header.metadata.frame_pages.push(page);
            if current.icc != first.icc {
                note(
                    &mut header,
                    &mut notes,
                    format!("TIFF page {page} ICC profile presence differs from page 0"),
                );
            }
        }
        header.metadata.pages_total =
            header.metadata.frame_pages.len() as u32 + header.metadata.excluded_pages_total;
        offset = next;
    }
    Ok(header)
}

fn note(header: &mut Header, count: &mut usize, message: String) {
    *count += 1;
    let warnings = &mut header.metadata.warnings;
    if *count <= RASTER_WARNINGS_MAX {
        warnings.push(message);
    } else {
        warnings[RASTER_WARNINGS_MAX - 1] = format!(
            "{} more notes not shown",
            *count - (RASTER_WARNINGS_MAX - 1)
        );
    }
}

fn read_page(input: &mut HeaderReader, encoding: Encoding, offset: u64) -> Result<(Page, u64)> {
    ensure!(offset != 0, "missing TIFF IFD");
    let (count, count_size, entry_size) = if encoding.big {
        (encoding.number(&input.read::<8>(offset)?), 8, 20)
    } else {
        (encoding.number(&input.read::<2>(offset)?), 2, 12)
    };
    let start = offset
        .checked_add(count_size)
        .ok_or_else(|| anyhow::anyhow!("IFD offset overflow"))?;
    let table_len = count
        .checked_mul(entry_size)
        .and_then(|n| n.checked_add(encoding.offset_size()))
        .ok_or_else(|| anyhow::anyhow!("IFD length overflow"))?;
    let table = input.bytes(start, table_len)?;
    let mut tags = BTreeMap::new();
    let mut icc = false;
    let entries_len = table.len() - encoding.offset_size() as usize;
    for entry in table[..entries_len].chunks_exact(entry_size as usize) {
        let tag = encoding.number(&entry[..2]);
        if tag == 34675 {
            icc = true;
        }
        if !matches!(tag, 254 | 256 | 257 | 258 | 262 | 274 | 277 | 338 | 339) {
            continue;
        }
        ensure!(!tags.contains_key(&tag), "duplicate TIFF layout tag");
        let size = match encoding.number(&entry[2..4]) {
            1 => 1,
            3 => 2,
            4 => 4,
            16 => 8,
            _ => anyhow::bail!("invalid TIFF layout tag type"),
        };
        let (values, value_slot) = if encoding.big {
            (encoding.number(&entry[4..12]), 12)
        } else {
            (encoding.number(&entry[4..8]), 8)
        };
        ensure!(values > 0, "empty TIFF layout tag");
        ensure!(
            values <= u64::from(u16::MAX),
            "TIFF layout values exceed the file budget"
        );
        // These are per-channel values or scalars, never pixel arrays;
        // the fixed channel cap bounds even a corrupt value count.
        let len = values
            .checked_mul(size)
            .ok_or_else(|| anyhow::anyhow!("TIFF tag length overflow"))?;
        let data = if len <= encoding.offset_size() {
            entry[value_slot..value_slot + len as usize].to_vec()
        } else {
            let data_offset =
                encoding.number(&entry[value_slot..value_slot + encoding.offset_size() as usize]);
            input.bytes(data_offset, len)?
        };
        let values = data
            .chunks_exact(size as usize)
            .map(|bytes| encoding.number(bytes))
            .collect::<Vec<_>>();
        tags.insert(tag, values);
    }
    let scalar = |tag, default| -> Result<u64> {
        match tags.get(&tag) {
            Some(values) => {
                ensure!(values.len() == 1, "TIFF scalar has multiple values");
                Ok(values[0])
            }
            None => Ok(default),
        }
    };
    let width = u32::try_from(scalar(256, 0)?)?;
    let height = u32::try_from(scalar(257, 0)?)?;
    ensure!(width > 0 && height > 0, "zero TIFF dimensions");
    let samples = scalar(277, 1)?;
    ensure!(samples > 0, "invalid TIFF sample count");
    let channel_values = |tag, default| -> Result<Vec<u64>> {
        let values = tags.get(&tag).cloned().unwrap_or_else(|| vec![default]);
        ensure!(
            values.len() == 1 || values.len() as u64 == samples,
            "invalid TIFF per-sample values"
        );
        // Uniform arrays and an omitted default describe the same layout.
        if values.iter().all(|v| *v == values[0]) {
            Ok(vec![values[0]])
        } else {
            Ok(values)
        }
    };
    let page = Page {
        width,
        height,
        samples,
        bits: channel_values(258, 1)?,
        format: channel_values(339, 1)?,
        photometric: scalar(262, u64::MAX)?,
        extra: tags.get(&338).cloned().unwrap_or_default(),
        orientation: orientation(scalar(274, 1)?),
        subfile: scalar(254, 0)?,
        icc,
    };
    let next = encoding.number(&table[entries_len..]);
    Ok((page, next))
}
