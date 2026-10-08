use super::{checked_profile, reader::Reader, RasterFrame, RASTER_ICC_MAX_BYTES};
use crate::api::contracts::{RasterColorType, RasterSampleFormat};
use crate::types::FileEntry;
use anyhow::{ensure, Context, Result};

pub(super) fn decode(
    file: &FileEntry,
    source: &mut Reader<'_>,
    length: u64,
) -> Result<RasterFrame> {
    let raster = file.raster.as_ref().context("missing raster metadata")?;
    let color = match raster.color_type {
        RasterColorType::Gray => png::ColorType::Grayscale,
        RasterColorType::GrayAlpha => png::ColorType::GrayscaleAlpha,
        RasterColorType::Rgb => png::ColorType::Rgb,
        RasterColorType::Rgba => png::ColorType::Rgba,
        RasterColorType::Palette => png::ColorType::Indexed,
        _ => anyhow::bail!("invalid PNG catalog color"),
    };
    let row_bytes =
        (u64::from(file.columns) * color.samples() as u64 * u64::from(raster.bit_depth))
            .div_ceil(8);
    let mut decoder = png::Decoder::new(source);
    decoder.set_transformations(png::Transformations::IDENTITY);
    decoder.set_ignore_text_chunk(true);
    decoder.set_limits(png::Limits {
        bytes: usize::try_from(RASTER_ICC_MAX_BYTES as u64 + 2 * row_bytes + length)?,
    });
    let mut reader = decoder.read_info()?;
    let info = reader.info();
    ensure!(
        info.width == file.columns
            && info.height == file.rows
            && info.color_type == color
            && info.bit_depth as u32 == raster.bit_depth
            && raster.sample_format == RasterSampleFormat::Uint,
        "PNG header differs from catalog"
    );
    if color == png::ColorType::Indexed {
        ensure!(
            info.trns.is_some() == raster.has_alpha,
            "PNG palette alpha differs from catalog"
        );
    }
    let mut packed = vec![0; reader.output_buffer_size()];
    let output = reader.next_frame(&mut packed)?;
    let info = reader.info();
    let expected = super::raster_frame_bytes(file).context("invalid raster frame size")? as usize;
    let bytes = if color == png::ColorType::Indexed || raster.bit_depth < 8 {
        let mut bytes = Vec::with_capacity(expected);
        for row in packed[..output.buffer_size()].chunks_exact(output.line_size) {
            for x in 0..file.columns as usize {
                let depth = raster.bit_depth as usize;
                let value = (row[x * depth / 8] >> (8 - depth - x * depth % 8))
                    & ((1_u16 << depth) - 1) as u8;
                if color == png::ColorType::Indexed {
                    let palette = info.palette.as_deref().context("PNG has no palette")?;
                    let start = usize::from(value) * 3;
                    bytes.extend_from_slice(
                        palette
                            .get(start..start + 3)
                            .context("PNG index beyond palette")?,
                    );
                    if file.samples_per_pixel == 4 {
                        bytes.push(
                            info.trns
                                .as_deref()
                                .and_then(|alpha| alpha.get(usize::from(value)))
                                .copied()
                                .unwrap_or(255),
                        );
                    }
                } else {
                    bytes.push(value);
                }
            }
        }
        bytes
    } else {
        packed.truncate(output.buffer_size());
        if raster.bit_depth == 16 {
            for sample in packed.chunks_exact_mut(2) {
                sample.swap(0, 1);
            }
        }
        packed
    };
    let icc_profile = checked_profile(file, info.icc_profile.as_deref());
    Ok(RasterFrame { bytes, icc_profile })
}
