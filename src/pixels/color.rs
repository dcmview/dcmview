use anyhow::{anyhow, Result};
use bytes::Bytes;
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use std::io::Cursor;

use super::syntax::ColorSamples;

/// Converts a decoded three-sample frame to interleaved display RGB.
pub(super) fn color_samples_to_rgb8(
    samples: ColorSamples,
    stored: &[u8],
    pixel_count: usize,
    planar_configuration: u32,
) -> Result<Vec<u8>> {
    match samples {
        ColorSamples::Rgb => rgb8_interleaved(stored, pixel_count, planar_configuration),
        ColorSamples::YbrFull => ybr_full_to_rgb8(stored, pixel_count, planar_configuration),
    }
}

pub(super) fn rgb8_interleaved(
    stored: &[u8],
    pixel_count: usize,
    planar_configuration: u32,
) -> Result<Vec<u8>> {
    let expected = pixel_count
        .checked_mul(3)
        .ok_or_else(|| anyhow!("RGB frame length overflowed"))?;
    if stored.len() != expected {
        return Err(anyhow!(
            "RGB frame contains {} bytes, expected {expected}",
            stored.len()
        ));
    }
    match planar_configuration {
        0 => Ok(stored.to_vec()),
        1 => {
            let mut interleaved = Vec::with_capacity(expected);
            for pixel in 0..pixel_count {
                interleaved.extend_from_slice(&[
                    stored[pixel],
                    stored[pixel_count + pixel],
                    stored[pixel_count * 2 + pixel],
                ]);
            }
            Ok(interleaved)
        }
        value => Err(anyhow!("unsupported PlanarConfiguration {value}")),
    }
}

pub(super) fn ybr_full_to_rgb8(
    stored: &[u8],
    pixel_count: usize,
    planar_configuration: u32,
) -> Result<Vec<u8>> {
    let ybr = rgb8_interleaved(stored, pixel_count, planar_configuration)?;
    Ok(convert_interleaved_ybr(&ybr))
}

pub(super) fn encode_rgb8_png_with_icc(
    rgb: Vec<u8>,
    columns: u32,
    rows: u32,
    icc_profile: Option<Vec<u8>>,
) -> Result<Bytes> {
    let expected = usize::try_from(columns)
        .ok()
        .and_then(|columns| {
            usize::try_from(rows)
                .ok()
                .and_then(|rows| columns.checked_mul(rows))
        })
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| anyhow!("RGB image geometry overflowed"))?;
    if rgb.len() != expected {
        return Err(anyhow!("RGB buffer size does not match image geometry"));
    }
    let mut encoded = Cursor::new(Vec::new());
    let mut encoder = PngEncoder::new(&mut encoded);
    if let Some(profile) = icc_profile {
        encoder
            .set_icc_profile(profile)
            .map_err(|error| anyhow!("invalid ICC profile for PNG encoding: {error}"))?;
    }
    encoder
        .write_image(&rgb, columns, rows, ExtendedColorType::Rgb8)
        .map_err(|error| anyhow!("RGB PNG encoding failed: {error}"))?;
    Ok(Bytes::from(encoded.into_inner()))
}

fn convert_interleaved_ybr(ybr: &[u8]) -> Vec<u8> {
    ybr.chunks_exact(3)
        .flat_map(|pixel| {
            let y = f64::from(pixel[0]);
            let cb = f64::from(pixel[1]) - 128.0;
            let cr = f64::from(pixel[2]) - 128.0;
            [
                clamp_u8(y + 1.402 * cr),
                clamp_u8(y - 0.344_136 * cb - 0.714_136 * cr),
                clamp_u8(y + 1.772 * cb),
            ]
        })
        .collect()
}

fn clamp_u8(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::{encode_rgb8_png_with_icc, rgb8_interleaved, ybr_full_to_rgb8};
    use image::{codecs::png::PngDecoder, ImageDecoder};
    use std::io::Cursor;

    const RGB_QUADRANTS: [u8; 12] = [
        255, 0, 0, // red
        0, 255, 0, // green
        0, 0, 255, // blue
        255, 255, 255, // white
    ];

    #[test]
    fn normalizes_interleaved_and_planar_rgb() {
        assert_eq!(
            rgb8_interleaved(&RGB_QUADRANTS, 4, 0).unwrap(),
            RGB_QUADRANTS
        );
        let planar = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255];
        assert_eq!(rgb8_interleaved(&planar, 4, 1).unwrap(), RGB_QUADRANTS);
    }

    #[test]
    fn converts_prepared_ybr_full_quadrants() {
        let ybr = [76, 85, 255, 150, 44, 21, 29, 255, 107, 255, 128, 128];
        let rgb = ybr_full_to_rgb8(&ybr, 4, 0).unwrap();
        assert_eq!(&rgb[0..3], &[254, 0, 0]);
        assert_eq!(&rgb[3..6], &[0, 255, 1]);
        assert_eq!(&rgb[6..9], &[0, 0, 254]);
        assert_eq!(&rgb[9..12], &[255, 255, 255]);
    }

    #[test]
    fn rejects_invalid_layouts_and_lengths() {
        assert!(rgb8_interleaved(&[0; 3], 1, 2).is_err());
        assert!(rgb8_interleaved(&[0; 2], 1, 0).is_err());
    }

    #[test]
    fn embeds_icc_profile_without_changing_rgb_samples() {
        let mut profile = vec![0; 128];
        profile[..4].copy_from_slice(&128_u32.to_be_bytes());
        profile[36..40].copy_from_slice(b"acsp");
        let png = encode_rgb8_png_with_icc(RGB_QUADRANTS.to_vec(), 2, 2, Some(profile.clone()))
            .expect("encode ICC PNG");

        let mut decoder = PngDecoder::new(Cursor::new(png.clone())).expect("decode PNG");
        assert_eq!(
            decoder.icc_profile().expect("read ICC profile"),
            Some(profile)
        );
        let pixels = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("load PNG")
            .to_rgb8()
            .into_raw();
        assert_eq!(pixels, RGB_QUADRANTS);
    }
}
