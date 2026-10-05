//! Redaction boxes: rectangles of a frame whose pixels are withheld, drawn by
//! hand over burned-in text. Both frame endpoints apply them, so a redacted
//! region is never sent: display frames show it black, and raw frames carry
//! one uniform value there that every window renders as the darkest gray.

use super::color::png_encoder;
use crate::api::contracts::RawFrameMetadata;
use crate::types::{FileEntry, NativePixelDataKind};
use anyhow::{Context, Result};
use bytes::Bytes;
use image::codecs::png::PngDecoder;
use image::{ImageDecoder, ImageEncoder};
use std::io::Cursor;

/// The boxes that apply to one frame, as `[row0, column0, row1, column1]`
/// with exclusive ends, and the revision of the file's boxes they come from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redaction {
    pub revision: u64,
    pub boxes: Vec<[u32; 4]>,
}

impl Redaction {
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }
}

/// Each box clipped to the image, as `(rows, columns)` ranges.
pub(super) fn clipped(
    boxes: &[[u32; 4]],
    rows: u32,
    columns: u32,
) -> impl Iterator<Item = (std::ops::Range<usize>, std::ops::Range<usize>)> + '_ {
    boxes.iter().map(move |&[row0, column0, row1, column1]| {
        (
            row0.min(rows) as usize..row1.min(rows) as usize,
            column0.min(columns) as usize..column1.min(columns) as usize,
        )
    })
}

/// A display PNG with the boxes painted opaque black, re-encoded with its
/// color type and ICC profile.
pub(super) fn redact_png(png: &Bytes, boxes: &[[u32; 4]]) -> Result<Bytes> {
    let mut decoder = PngDecoder::new(Cursor::new(png)).context("redaction could not read PNG")?;
    let icc_profile = decoder.icc_profile().ok().flatten();
    let (columns, rows) = decoder.dimensions();
    let color = decoder.color_type();
    let mut pixels = vec![0; decoder.total_bytes() as usize];
    decoder
        .read_image(&mut pixels)
        .context("redaction could not decode PNG")?;

    let pixel_size = color.bytes_per_pixel() as usize;
    let alpha_size = if color.has_alpha() {
        pixel_size / color.channel_count() as usize
    } else {
        0
    };
    for (box_rows, box_columns) in clipped(boxes, rows, columns) {
        for row in box_rows {
            let start = (row * columns as usize + box_columns.start) * pixel_size;
            let end = (row * columns as usize + box_columns.end) * pixel_size;
            for pixel in pixels[start..end].chunks_exact_mut(pixel_size) {
                let (color, alpha) = pixel.split_at_mut(pixel_size - alpha_size);
                color.fill(0);
                alpha.fill(u8::MAX);
            }
        }
    }

    let mut encoded = Cursor::new(Vec::new());
    let mut encoder = png_encoder(&mut encoded);
    if let Some(profile) = icc_profile {
        encoder
            .set_icc_profile(profile)
            .context("redaction could not keep the ICC profile")?;
    }
    encoder
        .write_image(&pixels, columns, rows, color.into())
        .context("redaction could not encode PNG")?;
    Ok(Bytes::from(encoded.into_inner()))
}

/// An RGBA layer of `columns` pixels per row with the boxes painted opaque
/// black.
pub(super) fn redact_rgba(layer: &mut [u8], rows: u32, columns: u32, boxes: &[[u32; 4]]) {
    for (box_rows, box_columns) in clipped(boxes, rows, columns) {
        for row in box_rows {
            let start = (row * columns as usize + box_columns.start) * 4;
            let end = (row * columns as usize + box_columns.end) * 4;
            for pixel in layer[start..end].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0, 0, 0, u8::MAX]);
            }
        }
    }
}

/// Where the samples of a pixel are in a raw frame body, which keeps the
/// stored layout: color-by-plane native frames, and native YBR_FULL_422 as
/// Y0 Y1 Cb Cr per pixel pair.
pub(super) struct RawLayout {
    /// Bytes per sample.
    pub(super) size: usize,
    columns: usize,
    pixel_count: usize,
    pub(super) samples: usize,
    subsampled: bool,
    planar: bool,
}

impl RawLayout {
    pub(super) fn of(
        file: &FileEntry,
        metadata: &RawFrameMetadata,
        body_len: usize,
    ) -> Option<Self> {
        let size = match metadata.bits_allocated {
            1 | 8 => 1,
            bits @ (16 | 32 | 64) => (bits / 8) as usize,
            _ => return None,
        };
        let columns = metadata.columns as usize;
        let pixel_count = metadata.rows as usize * columns;
        let subsampled = metadata
            .photometric_interpretation
            .trim()
            .eq_ignore_ascii_case("YBR_FULL_422")
            && body_len == pixel_count * 2 * size;
        let planar = file.series_metadata.native_pixel.planar_configuration == Some(1)
            && super::syntax::codec_for_syntax(&file.transfer_syntax_uid)
                == Some(super::syntax::Codec::Native);
        Some(Self {
            size,
            columns,
            pixel_count,
            samples: metadata.samples_per_pixel as usize,
            subsampled,
            planar,
        })
    }

    /// The sample indices of one pixel, in color-by-pixel order.
    pub(super) fn sample_indices(
        &self,
        row: usize,
        column: usize,
    ) -> impl Iterator<Item = usize> + '_ {
        let pixel = row * self.columns + column;
        let pair = row * self.columns * 2 + (column / 2) * 4;
        let count = if self.subsampled { 3 } else { self.samples };
        (0..count).map(move |sample| {
            if self.samples == 1 {
                pixel
            } else if self.subsampled {
                [pair + column % 2, pair + 2, pair + 3][sample]
            } else if self.planar {
                sample * self.pixel_count + pixel
            } else {
                pixel * self.samples + sample
            }
        })
    }
}

/// A raw frame body with the boxes filled with one value: for grayscale the
/// frame's darkest stored value (its largest for MONOCHROME1), so automatic
/// windows computed from the samples are unchanged and any window shows the
/// region as the darkest gray; for color, black. A layout that cannot be
/// addressed yields an all-zero frame rather than unredacted samples.
pub(super) fn redact_raw(
    file: &FileEntry,
    body: &Bytes,
    metadata: &RawFrameMetadata,
    boxes: &[[u32; 4]],
) -> Bytes {
    let Some(layout) = RawLayout::of(file, metadata, body.len()) else {
        return Bytes::from(vec![0; body.len()]);
    };
    let photometric = metadata
        .photometric_interpretation
        .trim()
        .to_ascii_uppercase();
    let fills: Vec<Vec<u8>> = if layout.samples == 1 {
        vec![darkest_sample(file, body, metadata, &layout, &photometric)]
    } else {
        // Chrominance is centred on half the sample range.
        let mut neutral = vec![0; layout.size];
        neutral[layout.size - 1] = 0x80;
        (0..3)
            .map(|sample| {
                if sample > 0 && photometric.starts_with("YBR") {
                    neutral.clone()
                } else {
                    vec![0; layout.size]
                }
            })
            .collect()
    };

    let mut redacted = body.to_vec();
    for (box_rows, box_columns) in clipped(boxes, metadata.rows, metadata.columns) {
        for row in box_rows {
            for column in box_columns.clone() {
                for (sample, index) in layout.sample_indices(row, column).enumerate() {
                    let fill = &fills[sample.min(fills.len() - 1)];
                    if let Some(bytes) =
                        redacted.get_mut(index * layout.size..(index + 1) * layout.size)
                    {
                        bytes.copy_from_slice(fill);
                    }
                }
            }
        }
    }
    Bytes::from(redacted)
}

/// The bytes of the grayscale sample that displays darkest.
fn darkest_sample(
    file: &FileEntry,
    body: &[u8],
    metadata: &RawFrameMetadata,
    layout: &RawLayout,
    photometric: &str,
) -> Vec<u8> {
    let signed = metadata.pixel_representation == 1;
    let float = matches!(
        file.series_metadata.native_pixel.pixel_data_kind,
        Some(NativePixelDataKind::Float32 | NativePixelDataKind::Float64)
    );
    let value = |sample: &[u8]| -> f64 {
        match (sample.len(), float, signed) {
            (1, _, false) => f64::from(sample[0]),
            (1, _, true) => f64::from(sample[0] as i8),
            (2, _, false) => f64::from(u16::from_le_bytes([sample[0], sample[1]])),
            (2, _, true) => f64::from(i16::from_le_bytes([sample[0], sample[1]])),
            (4, true, _) => f64::from(f32::from_le_bytes(sample.try_into().expect("4 bytes"))),
            (4, false, false) => f64::from(u32::from_le_bytes(sample.try_into().expect("4 bytes"))),
            (4, false, true) => f64::from(i32::from_le_bytes(sample.try_into().expect("4 bytes"))),
            (8, _, _) => f64::from_le_bytes(sample.try_into().expect("8 bytes")),
            _ => 0.0,
        }
    };
    let brightest_is_darkest = photometric == "MONOCHROME1";
    body.chunks_exact(layout.size)
        .filter(|sample| !value(sample).is_nan())
        .min_by(|left, right| {
            let order = value(left).total_cmp(&value(right));
            if brightest_is_darkest {
                order.reverse()
            } else {
                order
            }
        })
        .map_or_else(|| vec![0; layout.size], <[u8]>::to_vec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ExtendedColorType;

    fn file(photometric: &str, samples: u32, signed: bool) -> FileEntry {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let mut file = crate::loader::test_entry(&fixture);
        file.photometric_interpretation = photometric.to_string();
        file.samples_per_pixel = samples;
        file.pixel_representation = u32::from(signed);
        file
    }

    fn metadata(file: &FileEntry, rows: u32, columns: u32, bits: u32) -> RawFrameMetadata {
        file.raw_metadata(rows, columns, bits, file.samples_per_pixel)
    }

    fn png(pixels: &[u8], columns: u32, rows: u32, color: ExtendedColorType) -> Bytes {
        let mut encoded = Vec::new();
        png_encoder(&mut encoded)
            .write_image(pixels, columns, rows, color)
            .expect("encode test PNG");
        Bytes::from(encoded)
    }

    fn decoded(png: &Bytes) -> Vec<u8> {
        image::load_from_memory(png)
            .expect("decode PNG")
            .into_bytes()
    }

    #[test]
    fn display_png_boxes_are_black_and_the_rest_is_untouched() {
        let gray = png(&[9; 12], 4, 3, ExtendedColorType::L8);
        let redacted = redact_png(&gray, &[[0, 1, 2, 3]]).expect("redact gray");
        assert_eq!(decoded(&redacted), [9, 0, 0, 9, 9, 0, 0, 9, 9, 9, 9, 9]);

        let rgba = png(&[7; 16], 2, 2, ExtendedColorType::Rgba8);
        let redacted = redact_png(&rgba, &[[1, 0, 2, 1]]).expect("redact RGBA");
        assert_eq!(
            decoded(&redacted),
            [7, 7, 7, 7, 7, 7, 7, 7, 0, 0, 0, 255, 7, 7, 7, 7]
        );
    }

    #[test]
    fn boxes_reaching_past_the_image_are_clipped() {
        let gray = png(&[9; 4], 2, 2, ExtendedColorType::L8);
        let redacted = redact_png(&gray, &[[1, 1, 50, 50]]).expect("redact gray");
        assert_eq!(decoded(&redacted), [9, 9, 9, 0]);
    }

    #[test]
    fn grayscale_raw_boxes_take_the_darkest_stored_value() {
        let unsigned = file("MONOCHROME2", 1, false);
        let body = Bytes::from(
            [500_u16, 40, 900, 700]
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let redacted = redact_raw(
            &unsigned,
            &body,
            &metadata(&unsigned, 2, 2, 16),
            &[[0, 0, 1, 1]],
        );
        assert_eq!(&redacted[..2], 40_u16.to_le_bytes());
        assert_eq!(&redacted[2..], &body[2..]);

        // MONOCHROME1 displays its largest value darkest.
        let inverted = file("MONOCHROME1", 1, false);
        let redacted = redact_raw(
            &inverted,
            &body,
            &metadata(&inverted, 2, 2, 16),
            &[[0, 0, 1, 1]],
        );
        assert_eq!(&redacted[..2], 900_u16.to_le_bytes());

        let signed = file("MONOCHROME2", 1, true);
        let body = Bytes::from(
            [5_i16, -300, 20, 7]
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let redacted = redact_raw(
            &signed,
            &body,
            &metadata(&signed, 2, 2, 16),
            &[[1, 1, 2, 2]],
        );
        assert_eq!(&redacted[6..], (-300_i16).to_le_bytes());
    }

    #[test]
    fn color_raw_boxes_are_black_in_the_stored_layout() {
        let rgb = file("RGB", 3, false);
        let body = Bytes::from(vec![200_u8; 12]);
        let mut by_pixel = metadata(&rgb, 2, 2, 8);
        by_pixel.photometric_interpretation = "RGB".to_string();
        let redacted = redact_raw(&rgb, &body, &by_pixel, &[[0, 1, 1, 2]]);
        assert_eq!(
            &redacted[..],
            [200, 200, 200, 0, 0, 0, 200, 200, 200, 200, 200, 200]
        );

        let ybr = file("YBR_FULL", 3, false);
        let mut ybr_metadata = metadata(&ybr, 2, 2, 8);
        ybr_metadata.photometric_interpretation = "YBR_FULL".to_string();
        let redacted = redact_raw(&ybr, &body, &ybr_metadata, &[[0, 0, 1, 1]]);
        assert_eq!(&redacted[..3], [0, 128, 128]);
    }
}
