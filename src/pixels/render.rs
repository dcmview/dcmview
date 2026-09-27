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
    exclude_padding_samples, read_pixel_padding_range, resolve_window_with_mode,
};

pub(crate) struct LuminanceRenderOptions {
    pub(crate) frame: u32,
    pub(crate) rows: u32,
    pub(crate) columns: u32,
    pub(crate) requested_wc: Option<f64>,
    pub(crate) requested_ww: Option<f64>,
    pub(crate) window_mode: WindowMode,
}

/// Renders one frame of stored grayscale samples as an 8-bit PNG.
///
/// Every grayscale decode path shares this presentation pipeline: Modality
/// LUT or rescale, VOI LUT or window, MONOCHROME1 inversion, Pixel Padding as
/// black background, display shutter, and overlay planes.
pub(crate) fn encode_windowed_luminance_png(
    file: &FileEntry,
    stored: &[f64],
    options: LuminanceRenderOptions,
) -> Result<Bytes> {
    let LuminanceRenderOptions {
        frame,
        rows,
        columns,
        requested_wc,
        requested_ww,
        window_mode,
    } = options;
    let pixel_kind = file
        .series_metadata
        .native_pixel
        .pixel_data_kind
        .unwrap_or(NativePixelDataKind::Integer);
    let padding_mask = open_header(&file.path)
        .ok()
        .and_then(|object| read_pixel_padding_range(&object, pixel_kind))
        .map(|padding| padding.mask(stored));
    let rescaled = apply_modality_transform(
        stored,
        file.series_metadata.native_pixel.modality_lut.as_ref(),
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
        window_mode,
        requested_wc,
        requested_ww,
        file.default_window,
        file.series_metadata.native_pixel.voi_lut.as_ref(),
        &rescaled,
    ) {
        values
    } else {
        let resolved_window = resolve_window_with_mode(
            window_mode,
            requested_wc,
            requested_ww,
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
