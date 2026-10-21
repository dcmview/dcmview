use super::{raster_frame_bytes, RasterFrame};
use crate::api::contracts::{RasterSampleFormat, WindowMode};
use crate::pixels::render::{
    render_windowed_luminance, DisplayBuffer, DisplayPixels, LuminanceRenderOptions, StoredSamples,
};
use crate::types::FileEntry;
use anyhow::{ensure, Context, Result};

pub(super) fn render(
    file: &FileEntry,
    frame: u32,
    decoded: RasterFrame,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<DisplayBuffer> {
    ensure!(
        Some(decoded.bytes.len() as u64) == raster_frame_bytes(file),
        "raster sample count differs from catalog"
    );
    let samples = file.samples_per_pixel as usize;
    let size = (file.bits_allocated / 8) as usize;
    ensure!(
        (1..=4).contains(&samples) && matches!(size, 1 | 2 | 4 | 8),
        "invalid raster sample layout"
    );
    let max = if size == 1 { 255_u64 } else { 65535 };
    let integer = |bytes: &[u8]| {
        if size == 1 {
            u64::from(bytes[0])
        } else {
            u64::from(u16::from_le_bytes([bytes[0], bytes[1]]))
        }
    };
    if samples >= 3 {
        ensure!(size <= 2, "invalid raster color depth");
        if samples == 3 && size == 1 {
            return DisplayBuffer::rgb8(
                decoded.bytes,
                file.columns,
                file.rows,
                decoded.icc_profile,
            );
        }
        let mut rgb = Vec::with_capacity(file.rows as usize * file.columns as usize * 3);
        for pixel in decoded.bytes.chunks_exact(samples * size) {
            let alpha = if samples == 4 {
                integer(&pixel[3 * size..])
            } else {
                max
            };
            for sample in pixel[..3 * size].chunks_exact(size) {
                let flattened = (integer(sample) * alpha + max / 2) / max;
                rgb.push(((flattened * 255 + max / 2) / max) as u8);
            }
        }
        return DisplayBuffer::rgb8(rgb, file.columns, file.rows, decoded.icc_profile);
    }
    let gray_plane;
    let gray = if samples == 2 {
        ensure!(size <= 2, "invalid raster alpha depth");
        gray_plane = decoded
            .bytes
            .chunks_exact(2 * size)
            .flat_map(|pixel| pixel[..size].iter().copied())
            .collect::<Vec<_>>();
        gray_plane.as_slice()
    } else {
        decoded.bytes.as_slice()
    };
    let values;
    let stored = if size <= 2 {
        StoredSamples::Integer {
            bytes: gray,
            bits_allocated: file.bits_allocated,
            signed: file.pixel_representation == 1,
        }
    } else {
        let format = file
            .raster
            .as_ref()
            .context("missing raster metadata")?
            .sample_format;
        values = gray
            .chunks_exact(size)
            .map(|sample| match (format, size) {
                (RasterSampleFormat::Uint, 4) => {
                    Ok(f64::from(u32::from_le_bytes(sample.try_into()?)))
                }
                (RasterSampleFormat::Int, 4) => {
                    Ok(f64::from(i32::from_le_bytes(sample.try_into()?)))
                }
                (RasterSampleFormat::Float, 4) => {
                    Ok(f64::from(f32::from_le_bytes(sample.try_into()?)))
                }
                (RasterSampleFormat::Float, 8) => Ok(f64::from_le_bytes(sample.try_into()?)),
                _ => anyhow::bail!("invalid raster gray sample format"),
            })
            .collect::<Result<Vec<f64>>>()?;
        StoredSamples::Values(&values)
    };
    let mut buffer = render_windowed_luminance(
        file,
        stored,
        LuminanceRenderOptions {
            frame,
            rows: file.rows,
            columns: file.columns,
            requested_wc,
            requested_ww,
            window_mode,
        },
    )?;
    if samples == 2 {
        if let DisplayPixels::Gray8(pixels) = &mut buffer.pixels {
            for (gray, source) in pixels.iter_mut().zip(decoded.bytes.chunks_exact(2 * size)) {
                *gray = ((u64::from(*gray) * integer(&source[size..]) + max / 2) / max) as u8;
            }
        }
    }
    Ok(buffer)
}
