use crate::api::contracts::WindowMode;
use crate::types::{FileEntry, NativePixelDataKind};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use image::{ImageBuffer, ImageFormat, Luma};
use std::io::Cursor;

use super::color::encode_rgb8_png_with_icc;
use super::header::open_header;
use super::overlay::apply_overlay_planes;
use super::shutter;
use super::window::{
    apply_modality_transform, apply_padding_background, apply_voi_lut_if_selected, apply_window,
    exclude_padding_samples, modality_value, read_pixel_padding_range,
    resolve_window_from_distribution, resolve_window_with_mode, selected_voi_lut, voi_lut_value,
    window_value, PixelPaddingRange, ValueDistribution,
};

pub(crate) struct LuminanceRenderOptions {
    pub(crate) frame: u32,
    pub(crate) rows: u32,
    pub(crate) columns: u32,
    pub(crate) requested_wc: Option<f64>,
    pub(crate) requested_ww: Option<f64>,
    pub(crate) window_mode: WindowMode,
}

/// One grayscale frame's stored values, as the presentation pipeline takes
/// them.
pub(crate) enum StoredSamples<'a> {
    /// Integers in an 8- or 16-bit little-endian container, already reduced
    /// to Bits Stored (and sign-extended when signed).
    Integer {
        bytes: &'a [u8],
        bits_allocated: u32,
        signed: bool,
    },
    /// Any other layout (one-bit, 32-bit or floating point), one value each.
    Values(&'a [f64]),
}

/// Renders one frame of stored grayscale samples as an 8-bit PNG.
///
/// Every grayscale decode path shares this presentation pipeline: Modality
/// LUT or rescale, VOI LUT or window, MONOCHROME1 inversion, Pixel Padding as
/// black background, display shutter, and overlay planes.
pub(crate) fn encode_windowed_luminance_png(
    file: &FileEntry,
    stored: StoredSamples<'_>,
    options: LuminanceRenderOptions,
) -> Result<Bytes> {
    let pixel_kind = file
        .series_metadata
        .native_pixel
        .pixel_data_kind
        .unwrap_or(NativePixelDataKind::Integer);
    let padding = open_header(&file.path)
        .ok()
        .and_then(|object| read_pixel_padding_range(&object, pixel_kind));
    let mut windowed = match stored {
        StoredSamples::Integer {
            bytes,
            bits_allocated,
            signed,
        } => window_through_table(file, bytes, bits_allocated, signed, padding, &options)?,
        StoredSamples::Values(values) => window_each_sample(file, values, padding, &options)?,
    };
    let LuminanceRenderOptions {
        frame,
        rows,
        columns,
        ..
    } = options;
    shutter::apply_to_luminance(&mut windowed, file, frame, rows, columns);
    apply_overlay_planes(
        &mut windowed,
        rows,
        columns,
        frame,
        &file.series_metadata.presentation.overlay_planes,
    );

    let image = ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(columns, rows, windowed)
        .ok_or_else(|| anyhow!("windowed buffer size mismatch"))?;
    let mut buffer = Cursor::new(Vec::<u8>::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut buffer, ImageFormat::Png)
        .context("png encoding failed")?;
    Ok(Bytes::from(buffer.into_inner()))
}

/// 8- and 16-bit frames: every step up to the displayed byte depends only on
/// the stored value, so it is computed once per possible value (256 or 65536)
/// into a table, and each sample is one lookup. Windows derived from the
/// frame's samples come from a histogram of stored values. The output is the
/// same as `window_each_sample` would give, without converting every sample
/// to floating point twice.
fn window_through_table(
    file: &FileEntry,
    bytes: &[u8],
    bits_allocated: u32,
    signed: bool,
    padding: Option<PixelPaddingRange>,
    options: &LuminanceRenderOptions,
) -> Result<Vec<u8>> {
    let stored_value: fn(usize) -> f64 = match (bits_allocated, signed) {
        (8, false) => |index| index as f64,
        (8, true) => |index| f64::from(index as u8 as i8),
        (16, false) => |index| index as f64,
        (16, true) => |index| f64::from(index as u16 as i16),
        _ => {
            return Err(anyhow!(
                "integer table windowing needs 8- or 16-bit samples"
            ))
        }
    };
    let native = &file.series_metadata.native_pixel;
    let rescaled = (0..1_usize << bits_allocated)
        .map(|index| {
            modality_value(
                stored_value(index),
                native.modality_lut.as_ref(),
                file.rescale_slope,
                file.rescale_intercept,
            )
        })
        .collect::<Vec<_>>();
    let is_padding =
        |index: usize| padding.is_some_and(|range| range.contains(stored_value(index)));

    let mut table = if let Some(voi_lut) = selected_voi_lut(
        options.window_mode,
        options.requested_wc,
        options.requested_ww,
        file.default_window,
        native.voi_lut.as_ref(),
    ) {
        rescaled
            .iter()
            .map(|value| voi_lut_value(voi_lut, *value))
            .collect::<Vec<_>>()
    } else {
        let mut counts = vec![0_u64; rescaled.len()];
        for index in stored_indexes(bytes, bits_allocated) {
            counts[index] += 1;
        }
        // Automatic windows ignore padding unless the frame is all padding.
        let unpadded = distribution(&rescaled, &counts, |index| !is_padding(index));
        let window_source = if unpadded.is_empty() {
            distribution(&rescaled, &counts, |_| true)
        } else {
            unpadded
        };
        let window = resolve_window_from_distribution(
            options.window_mode,
            options.requested_wc,
            options.requested_ww,
            file.default_window,
            &window_source,
        )
        .ok_or_else(|| anyhow!("could not resolve window"))?;
        rescaled
            .iter()
            .map(|value| window_value(*value, window.center, window.width.max(1.0)))
            .collect()
    };
    apply_monochrome1_inversion(&mut table, &file.photometric_interpretation);
    // Padding is background: black whatever the photometric interpretation.
    for (index, entry) in table.iter_mut().enumerate() {
        if is_padding(index) {
            *entry = 0;
        }
    }
    Ok(stored_indexes(bytes, bits_allocated)
        .map(|index| table[index])
        .collect())
}

/// Each sample's table index: its bit pattern in the 8- or 16-bit container.
fn stored_indexes(bytes: &[u8], bits_allocated: u32) -> impl Iterator<Item = usize> + '_ {
    let width = if bits_allocated == 8 { 1 } else { 2 };
    bytes.chunks_exact(width).map(move |sample| {
        if width == 1 {
            usize::from(sample[0])
        } else {
            usize::from(u16::from_le_bytes([sample[0], sample[1]]))
        }
    })
}

fn distribution(
    rescaled: &[f64],
    counts: &[u64],
    include: impl Fn(usize) -> bool,
) -> ValueDistribution {
    ValueDistribution::new(
        rescaled
            .iter()
            .zip(counts)
            .enumerate()
            .filter(|(index, (_, count))| **count > 0 && include(*index))
            .map(|(_, (value, count))| (*value, *count))
            .collect(),
    )
}

/// Frames that do not fit a table: every sample goes through the pipeline.
fn window_each_sample(
    file: &FileEntry,
    stored: &[f64],
    padding: Option<PixelPaddingRange>,
    options: &LuminanceRenderOptions,
) -> Result<Vec<u8>> {
    let padding_mask = padding.map(|padding| padding.mask(stored));
    let native = &file.series_metadata.native_pixel;
    let rescaled = apply_modality_transform(
        stored,
        native.modality_lut.as_ref(),
        file.rescale_slope,
        file.rescale_intercept,
    );
    let unpadded = padding_mask
        .as_deref()
        .map(|mask| exclude_padding_samples(&rescaled, mask));
    let window_source = unpadded
        .as_deref()
        .filter(|samples| !samples.is_empty())
        .unwrap_or(&rescaled);
    let mut windowed = if let Some(values) = apply_voi_lut_if_selected(
        options.window_mode,
        options.requested_wc,
        options.requested_ww,
        file.default_window,
        native.voi_lut.as_ref(),
        &rescaled,
    ) {
        values
    } else {
        let resolved_window = resolve_window_with_mode(
            options.window_mode,
            options.requested_wc,
            options.requested_ww,
            file.default_window,
            window_source,
        )
        .ok_or_else(|| anyhow!("could not resolve window"))?;
        apply_window(
            &rescaled,
            resolved_window.center,
            resolved_window.width.max(1.0),
        )
    };
    apply_monochrome1_inversion(&mut windowed, &file.photometric_interpretation);
    // Padding is background: black whatever the photometric interpretation.
    if let Some(mask) = padding_mask.as_deref() {
        apply_padding_background(&mut windowed, mask);
    }
    Ok(windowed)
}

/// Encodes one interleaved 8-bit RGB display frame as PNG. Every color
/// decode path ends here after converting its samples to RGB, so the display
/// shutter is applied once for all of them.
pub(crate) fn encode_rgb8_display_png(
    file: &FileEntry,
    frame: u32,
    mut rgb: Vec<u8>,
    columns: u32,
    rows: u32,
    icc_profile: Option<Vec<u8>>,
) -> Result<Bytes> {
    shutter::apply_to_rgb8(&mut rgb, file, frame, rows, columns);
    encode_rgb8_png_with_icc(rgb, columns, rows, icc_profile)
}

fn is_monochrome1(photometric_interpretation: &str) -> bool {
    photometric_interpretation
        .trim()
        .eq_ignore_ascii_case("MONOCHROME1")
}

pub(crate) fn apply_monochrome1_inversion(samples: &mut [u8], photometric_interpretation: &str) {
    if is_monochrome1(photometric_interpretation) {
        for sample in samples {
            *sample = 255_u8.saturating_sub(*sample);
        }
    }
}
