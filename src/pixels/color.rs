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

/// Converts DICOM CIELab PCS-values (PS3.3 C.10.7.1.1: L* 0-100 and a*, b*
/// -128-127 scaled to 0-FFFFH, relative to the D50 PCS white) to 8-bit sRGB,
/// adapting D50 to sRGB's D65 white with the Bradford transform.
pub(super) fn cielab_to_srgb8([l, a, b]: [u16; 3]) -> [u8; 3] {
    let lightness = f64::from(l) * 100.0 / 65_535.0;
    let a = f64::from(a) * 255.0 / 65_535.0 - 128.0;
    let b = f64::from(b) * 255.0 / 65_535.0 - 128.0;

    let fy = (lightness + 16.0) / 116.0;
    let inverse = |t: f64| {
        const DELTA: f64 = 6.0 / 29.0;
        if t > DELTA {
            t * t * t
        } else {
            3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
        }
    };
    // D50 reference white.
    let x = 0.964_22 * inverse(fy + a / 500.0);
    let y = inverse(fy);
    let z = 0.825_21 * inverse(fy - b / 200.0);

    // Bradford-adapted D50 XYZ to linear sRGB.
    let linear = [
        3.133_856_1 * x - 1.616_866_7 * y - 0.490_614_6 * z,
        -0.978_768_4 * x + 1.916_141_5 * y + 0.033_454_0 * z,
        0.071_945_3 * x - 0.228_991_1 * y + 1.405_242_7 * z,
    ];
    linear.map(|channel| {
        let channel = channel.clamp(0.0, 1.0);
        let encoded = if channel <= 0.003_130_8 {
            12.92 * channel
        } else {
            1.055 * channel.powf(1.0 / 2.4) - 0.055
        };
        clamp_u8(encoded * 255.0)
    })
}

fn clamp_u8(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::{cielab_to_srgb8, encode_rgb8_png_with_icc, rgb8_interleaved, ybr_full_to_rgb8};
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

    /// Encodes L*, a*, b* as DICOM PCS-values.
    fn pcs([lightness, a, b]: [f64; 3]) -> [u16; 3] {
        [
            (lightness * 65_535.0 / 100.0).round() as u16,
            ((a + 128.0) * 65_535.0 / 255.0).round() as u16,
            ((b + 128.0) * 65_535.0 / 255.0).round() as u16,
        ]
    }

    #[test]
    fn converts_cielab_pcs_values_to_srgb() {
        // 0x8080 is a* = b* = 0: the neutral axis.
        assert_eq!(cielab_to_srgb8([0xFFFF, 0x8080, 0x8080]), [255, 255, 255]);
        assert_eq!(cielab_to_srgb8([0x0000, 0x8080, 0x8080]), [0, 0, 0]);
        // D50 Lab of the sRGB primaries (Bradford adaptation).
        for (lab, rgb) in [
            ([54.2905, 80.8049, 69.8910], [255, 0, 0]),
            ([87.8185, -79.2711, 80.9946], [0, 255, 0]),
            ([29.5683, 68.2874, -112.0297], [0, 0, 255]),
        ] {
            let converted = cielab_to_srgb8(pcs(lab));
            for (channel, expected) in converted.iter().zip(rgb) {
                assert!(channel.abs_diff(expected) <= 1, "{lab:?} -> {converted:?}");
            }
        }
    }
}
