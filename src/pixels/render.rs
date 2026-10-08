use crate::api::contracts::{FrameWindowApplied, RealWorldValueMap, WindowMode};
use crate::types::{FileEntry, ResolvedWindow};
use crate::value_mapping::map_value;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use image::{ExtendedColorType, ImageEncoder};

use super::color::{encode_rgb8_png_with_icc, png_encoder};
use super::overlay::{apply_overlay_planes, OVERLAY_PRESENTATION_VALUE};
use super::shutter;
use super::window::{
    apply_modality_transform, apply_padding_background, apply_voi_lut_if_selected,
    exclude_padding_samples, modality_value, resolve_window_from_distribution,
    resolve_window_with_function, selected_voi_lut, voi_lut_value, PixelPaddingRange,
    ValueDistribution, WindowFunction,
};

pub(crate) struct LuminanceRenderOptions {
    pub(crate) frame: u32,
    pub(crate) rows: u32,
    pub(crate) columns: u32,
    pub(crate) requested_wc: Option<f64>,
    pub(crate) requested_ww: Option<f64>,
    pub(crate) window_mode: WindowMode,
}

/// The presentation actually applied to an encoded frame.
#[derive(Debug, Clone, PartialEq)]
pub enum AppliedWindow {
    Linear(ResolvedWindow),
    RealWorld,
    VoiLut,
    Color,
}

impl AppliedWindow {
    pub fn kind(&self) -> Option<FrameWindowApplied> {
        match self {
            Self::Linear(_) => Some(FrameWindowApplied::Linear),
            Self::RealWorld => Some(FrameWindowApplied::RealWorld),
            Self::VoiLut => Some(FrameWindowApplied::VoiLut),
            Self::Color => None,
        }
    }
}

/// One encoded display frame, with its actual presentation kept in the cache.
#[derive(Debug, Clone)]
pub struct DisplayPng {
    pub png: Bytes,
    pub window: AppliedWindow,
}

impl DisplayPng {
    pub(crate) fn color(png: Bytes) -> Self {
        Self {
            png,
            window: AppliedWindow::Color,
        }
    }
}

/// One frame rendered for display: 8-bit (or, for one color path, 16-bit)
/// pixels in the stored pixel grid, after every step that depends on the
/// frame's samples (Modality LUT or rescale, VOI LUT or window, MONOCHROME1
/// inversion, Pixel Padding as background, palette and YBR conversion) and
/// before anything is drawn over it or encoded.
///
/// This is the seam between rendering and encoding. Every decode path
/// produces one; what happens next is the caller's presentation:
///
/// - a display frame draws the presentation graphics
///   ([`Self::draw_presentation_graphics`]) and encodes a PNG
///   ([`Self::encode_png`]); [`Self::into_display_png`] is those two steps;
/// - a thumbnail draws no graphics, paints the redaction boxes
///   ([`Self::redact`]) and is resampled and encoded as JPEG by
///   `thumbnail::encode_thumbnail_jpeg`.
///
/// A buffer never has graphics or redaction applied when a decode path
/// returns it, so one buffer can serve either presentation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisplayBuffer {
    pub(crate) rows: u32,
    pub(crate) columns: u32,
    pub(crate) pixels: DisplayPixels,
    /// The presentation the pixels were rendered with.
    pub(crate) window: AppliedWindow,
    /// The source ICC profile of an 8-bit RGB frame, which the display PNG
    /// carries. Thumbnails drop it.
    pub(crate) icc_profile: Option<Vec<u8>>,
}

/// The pixels of a [`DisplayBuffer`], row-major from the top-left pixel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DisplayPixels {
    /// One byte per pixel.
    Gray8(Vec<u8>),
    /// Three bytes per pixel, R G B.
    Rgb8(Vec<u8>),
    /// Three samples per pixel, R G B, as the 9- to 16-bit JPEG 2000 color
    /// path decodes them. `full_scale` is the largest value of the declared
    /// precision; the shutter fill is scaled to it.
    Rgb16 { samples: Vec<u16>, full_scale: u16 },
}

impl DisplayBuffer {
    /// An 8-bit RGB frame. Errors when `rgb` is not `rows * columns * 3`
    /// bytes or the geometry overflows.
    pub(crate) fn rgb8(
        rgb: Vec<u8>,
        columns: u32,
        rows: u32,
        icc_profile: Option<Vec<u8>>,
    ) -> Result<Self> {
        let expected = (rows as usize)
            .checked_mul(columns as usize)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| anyhow!("RGB image geometry overflowed"))?;
        if rgb.len() != expected {
            return Err(anyhow!("RGB buffer size does not match image geometry"));
        }
        Ok(Self {
            rows,
            columns,
            pixels: DisplayPixels::Rgb8(rgb),
            window: AppliedWindow::Color,
            icc_profile,
        })
    }

    /// A 9- to 16-bit RGB frame whose samples reach `full_scale`. Errors
    /// when `samples` is not `rows * columns * 3` values.
    pub(crate) fn rgb16(
        samples: Vec<u16>,
        full_scale: u16,
        columns: u32,
        rows: u32,
    ) -> Result<Self> {
        let expected = (rows as usize)
            .checked_mul(columns as usize)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| anyhow!("RGB image geometry overflowed"))?;
        if samples.len() != expected {
            return Err(anyhow!("RGB buffer size does not match image geometry"));
        }
        Ok(Self {
            rows,
            columns,
            pixels: DisplayPixels::Rgb16 {
                samples,
                full_scale,
            },
            window: AppliedWindow::Color,
            icc_profile: None,
        })
    }

    /// Draws what a display frame carries over its pixels and a thumbnail
    /// omits: the display shutter, then for a grayscale frame the overlay
    /// planes: `shutter::apply_to_luminance` then `apply_overlay_planes` at
    /// [`OVERLAY_PRESENTATION_VALUE`] for `Gray8`, `shutter::apply_to_rgb8`
    /// for `Rgb8`, `shutter::apply_to_rgb16` with `full_scale` for `Rgb16`.
    pub(crate) fn draw_presentation_graphics(&mut self, file: &FileEntry, frame: u32) {
        match &mut self.pixels {
            DisplayPixels::Gray8(pixels) => draw_presentation_graphics(
                pixels,
                |gray| [gray],
                file,
                frame,
                self.rows,
                self.columns,
            ),
            DisplayPixels::Rgb8(pixels) => {
                shutter::apply_to_rgb8(pixels, file, frame, self.rows, self.columns)
            }
            DisplayPixels::Rgb16 {
                samples,
                full_scale,
            } => {
                shutter::apply_to_rgb16(samples, *full_scale, file, frame, self.rows, self.columns)
            }
        }
    }

    /// Paints `boxes` (`[row0, column0, row1, column1]`, exclusive ends,
    /// stored-grid coordinates) black, clipped to the frame. Black is 0 in
    /// every channel whatever the photometric interpretation, as on a
    /// display frame.
    pub(crate) fn redact(&mut self, boxes: &[[u32; 4]]) {
        for (rows, columns) in super::redaction::clipped(boxes, self.rows, self.columns) {
            for row in rows {
                for column in columns.clone() {
                    let index = row * self.columns as usize + column;
                    match &mut self.pixels {
                        DisplayPixels::Gray8(pixels) => pixels[index] = 0,
                        DisplayPixels::Rgb8(pixels) => pixels[index * 3..index * 3 + 3].fill(0),
                        DisplayPixels::Rgb16 { samples, .. } => {
                            samples[index * 3..index * 3 + 3].fill(0)
                        }
                    }
                }
            }
        }
    }

    /// Encodes the buffer as the display PNG: `Gray8` as 8-bit grayscale,
    /// `Rgb8` as 8-bit RGB with the ICC profile when there is one, `Rgb16`
    /// as 16-bit RGB. The 8-bit forms are written with `png_encoder`, and
    /// `Rgb16` with `image`'s default PNG writer, so a display frame's bytes
    /// are what they were before rendering and encoding were separate
    /// steps.
    pub(crate) fn encode_png(self) -> Result<DisplayPng> {
        let png = match self.pixels {
            DisplayPixels::Gray8(pixels) => {
                let mut encoded = Vec::new();
                png_encoder(&mut encoded)
                    .write_image(&pixels, self.columns, self.rows, ExtendedColorType::L8)
                    .context("png encoding failed")?;
                Bytes::from(encoded)
            }
            DisplayPixels::Rgb8(pixels) => {
                encode_rgb8_png_with_icc(pixels, self.columns, self.rows, self.icc_profile)?
            }
            DisplayPixels::Rgb16 { samples, .. } => {
                let image = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(
                    self.columns,
                    self.rows,
                    samples,
                )
                .ok_or_else(|| anyhow!("JP2 decoded buffer size mismatch"))?;
                let mut buffer = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgb16(image)
                    .write_to(&mut buffer, image::ImageFormat::Png)
                    .context("JP2 decode failed: png encoding failed")?;
                Bytes::from(buffer.into_inner())
            }
        };
        match self.window {
            AppliedWindow::Color => Ok(DisplayPng::color(png)),
            window => Ok(DisplayPng { png, window }),
        }
    }

    /// The display frame of this buffer: the presentation graphics, then the
    /// PNG.
    pub(crate) fn into_display_png(mut self, file: &FileEntry, frame: u32) -> Result<DisplayPng> {
        self.draw_presentation_graphics(file, frame);
        self.encode_png()
    }
}

/// Renders one frame of stored grayscale samples to a `Gray8` buffer.
///
/// Every grayscale decode path shares this pipeline: Modality LUT or
/// rescale, VOI LUT or window, MONOCHROME1 inversion and Pixel Padding as
/// black background. The display shutter and overlay planes are not drawn
/// here; see [`DisplayBuffer::draw_presentation_graphics`].
/// [`encode_windowed_luminance_png`] is this buffer's
/// [`DisplayBuffer::into_display_png`].
pub(crate) fn render_windowed_luminance(
    file: &FileEntry,
    stored: StoredSamples<'_>,
    options: LuminanceRenderOptions,
) -> Result<DisplayBuffer> {
    let padding = pixel_padding(file);
    let (windowed, window) = match stored {
        StoredSamples::Integer {
            bytes,
            bits_allocated,
            signed,
        } => window_through_table(file, bytes, bits_allocated, signed, padding, &options)?,
        StoredSamples::Values(values) => window_each_sample(file, values, padding, &options)?,
    };
    Ok(DisplayBuffer {
        rows: options.rows,
        columns: options.columns,
        pixels: DisplayPixels::Gray8(windowed),
        window,
        icc_profile: None,
    })
}

/// Renders 8- or 16-bit stored integers windowed over a real-world mapping's
/// values to a `Gray8` buffer whose window is [`AppliedWindow::RealWorld`],
/// windowed as [`encode_real_world_windowed_png`] describes; that function
/// is this buffer's [`DisplayBuffer::into_display_png`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_real_world_windowed(
    file: &FileEntry,
    bytes: &[u8],
    bits_allocated: u32,
    signed: bool,
    map: &RealWorldValueMap,
    window: (f64, f64),
    frame: u32,
    dimensions: (u32, u32),
) -> Result<DisplayBuffer> {
    let (center, width) = window;
    let (rows, columns) = dimensions;
    let _ = frame;

    let stored_value = stored_value_reader(bits_allocated, signed)?;
    let low = center - width / 2.0;
    let table = (0..1_usize << bits_allocated)
        .map(|index| match map_value(map, stored_value(index)) {
            Some(mapped) if mapped.is_finite() => {
                (((mapped - low) / width).clamp(0.0, 1.0) * 255.0).round() as u8
            }
            _ => 0,
        })
        .collect::<Vec<_>>();
    let padding = pixel_padding(file);
    let is_padding =
        |index: usize| padding.is_some_and(|range| range.contains(stored_value(index)));
    let windowed = look_up_samples(file, table, is_padding, bytes, bits_allocated);
    Ok(DisplayBuffer {
        rows,
        columns,
        pixels: DisplayPixels::Gray8(windowed),
        window: AppliedWindow::RealWorld,
        icc_profile: None,
    })
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
) -> Result<DisplayPng> {
    let frame = options.frame;
    render_windowed_luminance(file, stored, options)?.into_display_png(file, frame)
}

/// Renders 8- or 16-bit stored integers windowed over a real-world mapping's
/// values instead of Modality values, the way the viewer's raw renderer
/// windows them: `map` is applied to each stored value, the window follows
/// the linear VOI function without the integer half-unit offsets (which
/// assume Modality integers), and stored values the mapping does not cover
/// are the window's low end. MONOCHROME1 inversion, Pixel Padding, shutter
/// and overlays follow as for any grayscale frame.
///
/// Reports RealWorld explicitly: its window has no linear Modality equivalent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_real_world_windowed_png(
    file: &FileEntry,
    bytes: &[u8],
    bits_allocated: u32,
    signed: bool,
    map: &RealWorldValueMap,
    (center, width): (f64, f64),
    frame: u32,
    (rows, columns): (u32, u32),
) -> Result<DisplayPng> {
    render_real_world_windowed(
        file,
        bytes,
        bits_allocated,
        signed,
        map,
        (center, width),
        frame,
        (rows, columns),
    )?
    .into_display_png(file, frame)
}

/// The graphics a grayscale display frame carries over its windowed values,
/// as an RGBA PNG of the frame's size: shutter fill and overlay graphics are
/// opaque gray, everything else transparent. Drawn over a frame windowed
/// anywhere (the viewer's raw renderer), it gives exactly the display frame,
/// since neither depends on the window.
pub(crate) fn encode_presentation_layer_png(
    file: &FileEntry,
    frame: u32,
    redaction_boxes: &[[u32; 4]],
) -> Result<Bytes> {
    let (rows, columns) = (file.rows, file.columns);
    let pixels = (rows as usize)
        .checked_mul(columns as usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| anyhow!("invalid image geometry"))?;
    let mut layer = vec![0; pixels];
    draw_presentation_graphics(
        &mut layer,
        |gray| [gray, gray, gray, 255],
        file,
        frame,
        rows,
        columns,
    );
    // The layer is drawn over the image, so it carries the redaction too.
    super::redaction::redact_rgba(&mut layer, rows, columns, redaction_boxes);
    let mut encoded = Vec::new();
    png_encoder(&mut encoded)
        .write_image(&layer, columns, rows, ExtendedColorType::Rgba8)
        .context("png encoding failed")?;
    Ok(Bytes::from(encoded))
}

/// Draws the display shutter (Shutter Presentation Value), then overlay
/// planes (presentation value 255), each pixel as `pixel(gray)`. Neither
/// depends on the window.
fn draw_presentation_graphics<const N: usize>(
    pixels: &mut [u8],
    pixel: fn(u8) -> [u8; N],
    file: &FileEntry,
    frame: u32,
    rows: u32,
    columns: u32,
) {
    shutter::apply_to_luminance(pixels, pixel, file, frame, rows, columns);
    apply_overlay_planes(
        pixels,
        pixel(OVERLAY_PRESENTATION_VALUE),
        rows,
        columns,
        frame,
        &file.series_metadata.presentation.overlay_planes,
    );
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
) -> Result<(Vec<u8>, AppliedWindow)> {
    let stored_value = stored_value_reader(bits_allocated, signed)?;
    let native = &file.series_metadata.native_pixel;
    let function = WindowFunction::for_file(file);
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

    let (table, window) = if let Some(voi_lut) = selected_voi_lut(
        options.window_mode,
        options.requested_wc,
        options.requested_ww,
        file.default_window,
        native.voi_lut.as_ref(),
    ) {
        let table = rescaled
            .iter()
            .map(|value| voi_lut_value(voi_lut, *value))
            .collect::<Vec<_>>();
        (table, AppliedWindow::VoiLut)
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
            function,
        )
        .ok_or_else(|| anyhow!("could not resolve window"))
        .map(|window| function.applied(window))?;
        let table = rescaled
            .iter()
            .map(|value| function.value(*value, &window))
            .collect();
        (table, AppliedWindow::Linear(window))
    };
    let windowed = look_up_samples(file, table, is_padding, bytes, bits_allocated);
    Ok((windowed, window))
}

/// Finishes a per-stored-value table of windowed bytes (MONOCHROME1
/// inversion, then Pixel Padding as black background whatever the
/// photometric interpretation) and maps every sample through it.
fn look_up_samples(
    file: &FileEntry,
    mut table: Vec<u8>,
    is_padding: impl Fn(usize) -> bool,
    bytes: &[u8],
    bits_allocated: u32,
) -> Vec<u8> {
    apply_monochrome1_inversion(&mut table, &file.photometric_interpretation);
    for (index, entry) in table.iter_mut().enumerate() {
        if is_padding(index) {
            *entry = 0;
        }
    }
    stored_indexes(bytes, bits_allocated)
        .map(|index| table[index])
        .collect()
}

fn pixel_padding(file: &FileEntry) -> Option<PixelPaddingRange> {
    file.series_metadata
        .native_pixel
        .pixel_padding
        .map(|[low, high]| PixelPaddingRange::new(low, Some(high)))
}

/// The stored value of each table index (a sample's bit pattern in the 8- or
/// 16-bit container).
fn stored_value_reader(bits_allocated: u32, signed: bool) -> Result<fn(usize) -> f64> {
    Ok(match (bits_allocated, signed) {
        (8, false) => |index| index as f64,
        (8, true) => |index| f64::from(index as u8 as i8),
        (16, false) => |index| index as f64,
        (16, true) => |index| f64::from(index as u16 as i16),
        _ => {
            return Err(anyhow!(
                "integer table windowing needs 8- or 16-bit samples"
            ))
        }
    })
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
) -> Result<(Vec<u8>, AppliedWindow)> {
    let padding_mask = padding.map(|padding| padding.mask(stored));
    let native = &file.series_metadata.native_pixel;
    let function = WindowFunction::for_file(file);
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
    let (mut windowed, window) = if let Some(values) = apply_voi_lut_if_selected(
        options.window_mode,
        options.requested_wc,
        options.requested_ww,
        file.default_window,
        native.voi_lut.as_ref(),
        &rescaled,
    ) {
        (values, AppliedWindow::VoiLut)
    } else {
        let resolved_window = resolve_window_with_function(
            options.window_mode,
            options.requested_wc,
            options.requested_ww,
            file.default_window,
            window_source,
            function,
        )
        .ok_or_else(|| anyhow!("could not resolve window"))
        .map(|window| function.applied(window))?;
        let windowed = rescaled
            .iter()
            .map(|value| function.value(*value, &resolved_window))
            .collect();
        (windowed, AppliedWindow::Linear(resolved_window))
    };
    apply_monochrome1_inversion(&mut windowed, &file.photometric_interpretation);
    // Padding is background: black whatever the photometric interpretation.
    if let Some(mask) = padding_mask.as_deref() {
        apply_padding_background(&mut windowed, mask);
    }
    Ok((windowed, window))
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
