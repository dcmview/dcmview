//! The image crate leaves JPEG and WebP samples in the stored orientation.
use super::{
    checked_profile, raster_decode_heap_limit, reader::Reader, RasterFrame, RASTER_ICC_MAX_BYTES,
};
use crate::api::contracts::{FileFormat, RasterSampleFormat};
use crate::types::FileEntry;
use anyhow::{ensure, Context, Result};
use image::{ColorType, ImageDecoder, Limits};
use std::io::{Read, Seek, SeekFrom};

pub(super) fn decode(
    file: &FileEntry,
    source: &mut Reader<'_>,
    length: u64,
    expected: u64,
) -> Result<RasterFrame> {
    match file.format {
        FileFormat::Jpeg => read(
            file,
            image::codecs::jpeg::JpegDecoder::new(source)?,
            length,
            expected,
            true,
        ),
        FileFormat::Webp => {
            let read_profile = check_webp(file, source, length)?;
            source.seek(SeekFrom::Start(0))?;
            read(
                file,
                image::codecs::webp::WebPDecoder::new(source)?,
                length,
                expected,
                read_profile,
            )
        }
        _ => anyhow::bail!("invalid image codec"),
    }
}

fn read_at<const N: usize>(source: &mut Reader<'_>, offset: u64) -> Result<[u8; N]> {
    source.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0; N];
    source.read_exact(&mut bytes)?;
    Ok(bytes)
}

struct WebpChunk {
    name: [u8; 4],
    size: u64,
    payload: u64,
    next: u64,
}

impl WebpChunk {
    fn read(source: &mut Reader<'_>, offset: u64) -> Result<Self> {
        let [a, b, c, d, s0, s1, s2, s3] = read_at(source, offset)?;
        let size = u64::from(u32::from_le_bytes([s0, s1, s2, s3]));
        let payload = offset
            .checked_add(8)
            .context("WebP chunk offset overflow")?;
        let next = payload
            .checked_add(size)
            .and_then(|end| end.checked_add(size % 2))
            .context("WebP chunk end overflow")?;
        Ok(Self {
            name: [a, b, c, d],
            size,
            payload,
            next,
        })
    }

    fn check_size(&self, source: &mut Reader<'_>, expected: (u64, u64)) -> Result<()> {
        let size = match &self.name {
            b"VP8 " => {
                let [_, _, _, s0, s1, s2, w0, w1, h0, h1] = read_at(source, self.payload)?;
                ensure!(
                    [s0, s1, s2] == [0x9d, 0x01, 0x2a],
                    "invalid WebP VP8 start code"
                );
                (
                    u64::from(u16::from_le_bytes([w0, w1]) & 0x3fff),
                    u64::from(u16::from_le_bytes([h0, h1]) & 0x3fff),
                )
            }
            b"VP8L" => {
                let [signature, b0, b1, b2, b3] = read_at(source, self.payload)?;
                ensure!(signature == 0x2f, "invalid WebP VP8L signature");
                let bits = u32::from_le_bytes([b0, b1, b2, b3]);
                (
                    u64::from((bits & 0x3fff) + 1),
                    u64::from(((bits >> 14) & 0x3fff) + 1),
                )
            }
            _ => anyhow::bail!("missing WebP bitstream"),
        };
        ensure!(
            size == expected,
            "WebP bitstream size differs from its frame"
        );
        Ok(())
    }
}

fn webp_u24(bytes: [u8; 3]) -> u64 {
    let [a, b, c] = bytes;
    u64::from(u32::from_le_bytes([a, b, c, 0]))
}

// The VP8 decoder allocates from its own frame header before comparing it
// with VP8X. Check that header first, including a first animation subframe.
fn check_webp(file: &FileEntry, source: &mut Reader<'_>, length: u64) -> Result<bool> {
    let [r, i, f0, f1, _, _, _, _, w, e, b, p] = read_at(source, 0)?;
    ensure!(
        [r, i, f0, f1] == *b"RIFF" && [w, e, b, p] == *b"WEBP",
        "invalid WebP RIFF header"
    );
    let expected = (u64::from(file.columns), u64::from(file.rows));
    let first = WebpChunk::read(source, 12)?;
    match &first.name {
        b"VP8 " | b"VP8L" => {
            first.check_size(source, expected)?;
            return Ok(false);
        }
        b"VP8X" => {}
        _ => anyhow::bail!("invalid WebP first chunk"),
    }
    let [flags, _, _, _, w0, w1, w2, h0, h1, h2] = read_at(source, first.payload)?;
    ensure!(
        (webp_u24([w0, w1, w2]) + 1, webp_u24([h0, h1, h2]) + 1) == expected,
        "WebP canvas differs from catalog"
    );
    let animated = flags & 0x02 != 0;
    let mut profile = None;
    let mut offset = first.next;
    while offset <= length && length - offset >= 8 {
        let chunk = WebpChunk::read(source, offset)?;
        match &chunk.name {
            b"VP8 " | b"VP8L" => {
                ensure!(!animated, "WebP animation has an unframed image");
                chunk.check_size(source, expected)?;
                return Ok(profile.unwrap_or(false));
            }
            b"ANMF" => {
                ensure!(animated, "WebP still has an animation frame");
                let [x0, x1, x2, y0, y1, y2, w0, w1, w2, h0, h1, h2, _, _, _, _] =
                    read_at(source, chunk.payload)?;
                let x = webp_u24([x0, x1, x2]) * 2;
                let y = webp_u24([y0, y1, y2]) * 2;
                let width = webp_u24([w0, w1, w2]) + 1;
                let height = webp_u24([h0, h1, h2]) + 1;
                ensure!(
                    x + width <= expected.0 && y + height <= expected.1,
                    "WebP animation frame lies outside canvas"
                );
                let start = chunk
                    .payload
                    .checked_add(16)
                    .context("WebP frame offset overflow")?;
                let mut image = WebpChunk::read(source, start)?;
                if &image.name == b"ALPH" {
                    image = WebpChunk::read(source, image.next)?;
                    ensure!(&image.name == b"VP8 ", "WebP alpha has no VP8 image");
                }
                image.check_size(source, (width, height))?;
                return Ok(profile.unwrap_or(false));
            }
            b"ICCP" if profile.is_none() => {
                // The codec retains the first ICCP chunk and allocates its
                // declared size. Only let it read a bounded, present payload.
                profile = Some(
                    chunk.size <= RASTER_ICC_MAX_BYTES as u64
                        && chunk.payload <= length
                        && chunk.size <= length - chunk.payload,
                );
            }
            _ => {}
        }
        offset = chunk.next;
    }
    anyhow::bail!("missing WebP image chunk")
}

fn read(
    file: &FileEntry,
    mut decoder: impl ImageDecoder,
    length: u64,
    expected: u64,
    read_profile: bool,
) -> Result<RasterFrame> {
    let raster = file.raster.as_ref().context("missing raster metadata")?;
    let color = match file.samples_per_pixel {
        1 => ColorType::L8,
        3 => ColorType::Rgb8,
        4 => ColorType::Rgba8,
        _ => anyhow::bail!("invalid image catalog color"),
    };
    ensure!(
        decoder.dimensions() == (file.columns, file.rows)
            && decoder.color_type() == color
            && file.bits_allocated == 8
            && raster.bit_depth == 8
            && raster.sample_format == RasterSampleFormat::Uint,
        "image header differs from catalog"
    );
    let mut limits = Limits::default();
    limits.max_image_width = Some(file.columns);
    limits.max_image_height = Some(file.rows);
    limits.max_alloc = raster_decode_heap_limit(file, length);
    decoder.set_limits(limits)?;
    let profile = if read_profile {
        decoder.icc_profile()?
    } else {
        None
    };
    let icc_profile = checked_profile(file, profile.as_deref());
    drop(profile);
    let mut bytes = vec![0; usize::try_from(expected)?];
    decoder.read_image(&mut bytes)?;
    Ok(RasterFrame { bytes, icc_profile })
}
