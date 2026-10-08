//! Raster files of every layout the viewer decodes, each with the samples
//! its frames must hold (`docs/design/image-formats.md` sections 5.2 and 9).
//!
//! The expected samples of a case are the values its file was written from,
//! never values a decoder returned. `raster_decode.rs` reads every case
//! through the loader and the router; `tests/raster_cost` decodes and
//! damages the same files to measure what a decode costs.

// Two test binaries include this file and each uses part of it.
#![allow(dead_code)]

use super::raster_files::{self as files, TiffPage, TiffValue};
use image::ExtendedColorType;
use std::io::Cursor;
use tiff::encoder::{colortype, compression, TiffEncoder};

// ---------------------------------------------------------------------------
// Sample helpers

pub fn le16(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub fn be16(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

/// Rows of `width` samples of `depth` bits packed from the high bit, each
/// row padded to a whole byte, as PNG stores low-bit samples.
pub fn packed(values: &[u8], width: usize, depth: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in values.chunks(width) {
        let mut bits = 0_usize;
        let mut current = 0_u8;
        for value in row {
            current |= value << (8 - depth - bits % 8);
            bits += depth;
            if bits.is_multiple_of(8) {
                out.push(current);
                current = 0;
            }
        }
        if !bits.is_multiple_of(8) {
            out.push(current);
        }
    }
    out
}

/// `count` pixels of the given 8-bit colour.
pub fn flat(colour: &[u8], count: usize) -> Vec<u8> {
    colour.repeat(count)
}

/// Blocks of `side` x `side` pixels, one per colour, left to right.
fn blocks(colours: &[&[u8]], side: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..side {
        for colour in colours {
            out.extend(flat(colour, side));
        }
    }
    out
}

/// A TIFF written by the linked encoder, one image per `(width, height,
/// samples)` page.
pub fn tiff_pages<C>(pages: &[(u32, u32, &[C::Inner])]) -> Vec<u8>
where
    C: colortype::ColorType,
    [C::Inner]: tiff::encoder::TiffValue,
{
    let mut out = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut out).expect("TIFF encoder");
    for (width, height, samples) in pages {
        encoder
            .write_image::<C>(*width, *height, samples)
            .expect("write TIFF page");
    }
    out.into_inner()
}

/// The linked TIFF encoder writing into memory.
type Encoder<'a> = TiffEncoder<&'a mut Cursor<Vec<u8>>>;

// ---------------------------------------------------------------------------
// Cases

/// One file and what its raw frames must hold.
pub struct Case {
    pub name: &'static str,
    pub bytes: Vec<u8>,
    /// `(rows, columns)` of the stored grid.
    pub size: (u32, u32),
    /// `(samples_per_pixel, bits_allocated, pixel_representation,
    /// photometric_interpretation)` of the raw frames.
    pub layout: (u32, u32, u32, &'static str),
    /// The body of each raw frame, in frame order.
    pub frames: Vec<Vec<u8>>,
    /// How far an 8-bit sample of a lossy file may be from the value it was
    /// encoded from; 0 for every lossless file.
    pub tolerance: u8,
}

impl Case {
    fn lossless(
        name: &'static str,
        bytes: Vec<u8>,
        size: (u32, u32),
        layout: (u32, u32, u32, &'static str),
        frame: Vec<u8>,
    ) -> Self {
        Self {
            name,
            bytes,
            size,
            layout,
            frames: vec![frame],
            tolerance: 0,
        }
    }
}

const GRAY8: (u32, u32, u32, &str) = (1, 8, 0, "MONOCHROME2");
const GRAY16: (u32, u32, u32, &str) = (1, 16, 0, "MONOCHROME2");
const RGB8: (u32, u32, u32, &str) = (3, 8, 0, "RGB");
const RGBA8: (u32, u32, u32, &str) = (4, 8, 0, "RGBA");

pub fn png_cases() -> Vec<Case> {
    use png::{BitDepth, ColorType};
    let none = |_: &mut png::Encoder<'_, &mut Vec<u8>>| {};

    let bits: Vec<u8> = (0..30).map(|index| u8::from(index % 3 == 0)).collect();
    let two: Vec<u8> = vec![0, 1, 2, 3, 3, 3, 2, 1, 0, 0];
    let four: Vec<u8> = vec![0, 1, 7, 8, 14, 15];
    let eight: Vec<u8> = vec![0, 1, 127, 128, 200, 254, 255, 7];
    let sixteen: Vec<u16> = vec![0, 1, 256, 4095, 40_000, 65_535];
    let gray_alpha: Vec<u8> = vec![0, 0, 100, 128, 200, 255, 255, 1];
    let gray_alpha16: Vec<u16> = vec![0, 0, 1000, 32_768, 60_000, 65_535, 65_535, 257];
    let rgb: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30];
    let rgb16: Vec<u16> = vec![65_535, 0, 256, 1, 2, 3, 40_000, 50_000, 60_000, 0, 0, 0];
    let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 10, 20, 30, 1];
    let rgba16: Vec<u16> = vec![65_535, 0, 0, 65_535, 1000, 2000, 3000, 32_768];
    // Four colours, the first three with alpha 0, 128 and 255: the fourth
    // has none and is opaque.
    let palette: Vec<u8> = vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
    let transparency: Vec<u8> = vec![0, 128, 255];
    let indices: Vec<u8> = vec![0, 1, 2, 3, 3, 2, 1, 0];
    let expanded: Vec<u8> = indices
        .iter()
        .flat_map(|index| {
            let index = usize::from(*index);
            let mut pixel = palette[index * 3..index * 3 + 3].to_vec();
            pixel.push(transparency.get(index).copied().unwrap_or(255));
            pixel
        })
        .collect();
    let opaque_palette: Vec<u8> = vec![10, 20, 30, 40, 50, 60, 70, 80, 90];
    let opaque_indices: Vec<u8> = vec![2, 1, 0, 0, 1, 2];
    let opaque_expanded: Vec<u8> = opaque_indices
        .iter()
        .flat_map(|index| opaque_palette[usize::from(*index) * 3..][..3].to_vec())
        .collect();
    let interlaced: Vec<u8> = (0..9 * 9).map(|index| (index * 3 % 251) as u8).collect();

    // Two frames: the image itself, then another that is never a frame here.
    let mut animated = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut animated, 2, 2);
        encoder.set_color(ColorType::Grayscale);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_animated(2, 0).expect("animated PNG");
        let mut writer = encoder.write_header().expect("write PNG header");
        writer.write_image_data(&[1, 2, 3, 4]).expect("frame 0");
        writer.write_image_data(&[9, 9, 9, 9]).expect("frame 1");
        writer.finish().expect("finish PNG");
    }

    vec![
        // Low-bit samples keep their stored values: 0 and 1, not 0 and 255.
        Case::lossless(
            "gray1.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::One,
                (10, 3),
                &packed(&bits, 10, 1),
                none,
            ),
            (3, 10),
            GRAY8,
            bits.clone(),
        ),
        Case::lossless(
            "gray2.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::Two,
                (5, 2),
                &packed(&two, 5, 2),
                none,
            ),
            (2, 5),
            GRAY8,
            two.clone(),
        ),
        Case::lossless(
            "gray4.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::Four,
                (3, 2),
                &packed(&four, 3, 4),
                none,
            ),
            (2, 3),
            GRAY8,
            four.clone(),
        ),
        Case::lossless(
            "gray8.png",
            files::png_of(ColorType::Grayscale, BitDepth::Eight, (4, 2), &eight, none),
            (2, 4),
            GRAY8,
            eight.clone(),
        ),
        // PNG stores 16-bit samples big endian; raw frames are little endian.
        Case::lossless(
            "gray16.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::Sixteen,
                (3, 2),
                &be16(&sixteen),
                none,
            ),
            (2, 3),
            GRAY16,
            le16(&sixteen),
        ),
        // sBIT records the original precision; the samples stay as stored.
        Case::lossless(
            "gray16-sbit12.png",
            files::png_from_chunks(
                (3, 2),
                16,
                0,
                false,
                &[
                    files::png_chunk(b"sBIT", &[12]),
                    files::png_idat(&be16(&sixteen), 6),
                ],
            ),
            (2, 3),
            GRAY16,
            le16(&sixteen),
        ),
        // A transparent colour on gray is not an alpha channel.
        Case::lossless(
            "gray8-colour-key.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::Eight,
                (4, 2),
                &eight,
                |encoder| encoder.set_trns(vec![0, 127]),
            ),
            (2, 4),
            GRAY8,
            eight.clone(),
        ),
        Case::lossless(
            "gray-alpha8.png",
            files::png_of(
                ColorType::GrayscaleAlpha,
                BitDepth::Eight,
                (2, 2),
                &gray_alpha,
                none,
            ),
            (2, 2),
            (2, 8, 0, "MONOCHROME2"),
            gray_alpha.clone(),
        ),
        Case::lossless(
            "gray-alpha16.png",
            files::png_of(
                ColorType::GrayscaleAlpha,
                BitDepth::Sixteen,
                (2, 2),
                &be16(&gray_alpha16),
                none,
            ),
            (2, 2),
            (2, 16, 0, "MONOCHROME2"),
            le16(&gray_alpha16),
        ),
        Case::lossless(
            "rgb8.png",
            files::png_of(ColorType::Rgb, BitDepth::Eight, (2, 2), &rgb, none),
            (2, 2),
            RGB8,
            rgb.clone(),
        ),
        Case::lossless(
            "rgb16.png",
            files::png_of(
                ColorType::Rgb,
                BitDepth::Sixteen,
                (2, 2),
                &be16(&rgb16),
                none,
            ),
            (2, 2),
            (3, 16, 0, "RGB"),
            le16(&rgb16),
        ),
        Case::lossless(
            "rgba8.png",
            files::png_of(ColorType::Rgba, BitDepth::Eight, (2, 2), &rgba, none),
            (2, 2),
            RGBA8,
            rgba.clone(),
        ),
        Case::lossless(
            "rgba16.png",
            files::png_of(
                ColorType::Rgba,
                BitDepth::Sixteen,
                (2, 1),
                &be16(&rgba16),
                none,
            ),
            (1, 2),
            (4, 16, 0, "RGBA"),
            le16(&rgba16),
        ),
        // A palette is expanded to its colours, with tRNS as alpha.
        Case::lossless(
            "palette2.png",
            files::png_of(
                ColorType::Indexed,
                BitDepth::Two,
                (4, 2),
                &packed(&indices, 4, 2),
                |encoder| {
                    encoder.set_palette(palette.clone());
                    encoder.set_trns(transparency.clone());
                },
            ),
            (2, 4),
            RGBA8,
            expanded,
        ),
        Case::lossless(
            "palette8.png",
            files::png_of(
                ColorType::Indexed,
                BitDepth::Eight,
                (3, 2),
                &opaque_indices,
                |encoder| encoder.set_palette(opaque_palette.clone()),
            ),
            (2, 3),
            RGB8,
            opaque_expanded,
        ),
        Case::lossless(
            "interlaced.png",
            files::png_adam7_gray8((9, 9), &interlaced),
            (9, 9),
            GRAY8,
            interlaced.clone(),
        ),
        Case::lossless("animated.png", animated, (2, 2), GRAY8, vec![1, 2, 3, 4]),
    ]
}

pub fn jpeg_cases() -> Vec<Case> {
    let lossy = |name, bytes, size, layout, frame, tolerance| Case {
        name,
        bytes,
        size,
        layout,
        frames: vec![frame],
        tolerance,
    };
    let gray = blocks(&[&[40], &[200]], 8);
    let rgb = flat(&[60, 120, 180], 16 * 16);
    let primaries = blocks(
        &[
            &[200, 30, 30],
            &[30, 200, 30],
            &[30, 30, 200],
            &[128, 128, 128],
        ],
        8,
    );
    // No ink, cyan, black, magenta with yellow.
    let inks = blocks(
        &[&[255, 255, 255], &[0, 255, 255], &[0, 0, 0], &[255, 0, 0]],
        8,
    );
    let gray_file = files::baseline_jpeg(ExtendedColorType::L8, (16, 8), &gray);
    vec![
        lossy(
            "gray.jpg",
            gray_file.clone(),
            (8, 16),
            GRAY8,
            gray.clone(),
            3,
        ),
        lossy(
            "rgb.jpg",
            files::baseline_jpeg(ExtendedColorType::Rgb8, (16, 16), &rgb),
            (16, 16),
            RGB8,
            rgb,
            4,
        ),
        lossy(
            "progressive.jpg",
            files::progressive_jpeg(),
            (8, 32),
            RGB8,
            primaries,
            4,
        ),
        lossy(
            "subsampled.jpg",
            files::subsampled_jpeg(),
            (16, 16),
            RGB8,
            flat(&[90, 140, 200], 16 * 16),
            4,
        ),
        // Chroma at a quarter of the width (4:1:1), and at half the height
        // (4:4:0).
        lossy(
            "quarter-width-chroma.jpg",
            files::quarter_width_chroma_jpeg(),
            (16, 32),
            RGB8,
            flat(&[90, 140, 200], 32 * 16),
            4,
        ),
        lossy(
            "half-height-chroma.jpg",
            files::half_height_chroma_jpeg(),
            (16, 32),
            RGB8,
            flat(&[90, 140, 200], 32 * 16),
            4,
        ),
        // Four channels are served as an approximate RGB.
        lossy(
            "cmyk.jpg",
            files::cmyk_jpeg(),
            (8, 32),
            RGB8,
            inks.clone(),
            6,
        ),
        lossy("ycck.jpg", files::ycck_jpeg(), (8, 32), RGB8, inks, 6),
        // EXIF orientation 6 turns the picture on screen only: the frame is
        // the stored 16 x 8 grid.
        lossy(
            "rotated.jpg",
            files::with_exif_orientation(gray_file, 6),
            (8, 16),
            GRAY8,
            gray,
            3,
        ),
    ]
}

pub fn tiff_cases() -> Vec<Case> {
    let eight: Vec<u8> = vec![0, 1, 127, 128, 200, 254, 255, 7];
    let sixteen: Vec<u16> = vec![0, 1, 256, 4095, 40_000, 65_535];
    let wide: Vec<u32> = vec![0, 65_536, 4_000_000_000, 7];
    let signed8: Vec<i8> = vec![-128, -1, 0, 127];
    let signed16: Vec<i16> = vec![-32_768, -1, 0, 32_767];
    let signed32: Vec<i32> = vec![i32::MIN, -1, 0, i32::MAX];
    // Values an inversion and its undoing would not give back.
    let reals: Vec<f32> = vec![0.3, 1.0e-20, -2.5, 7.0];
    let doubles: Vec<f64> = vec![0.1, 1.0e-300, -2.5, 1.0e9];
    let rgb: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30];
    let rgb16: Vec<u16> = vec![65_535, 0, 256, 1, 2, 3, 40_000, 50_000, 60_000, 0, 0, 0];
    let rgba: Vec<u8> = vec![200, 100, 50, 255, 100, 50, 25, 128, 0, 0, 0, 0, 7, 7, 7, 7];
    // The same kind of pixels stored premultiplied: colour times alpha / 255.
    let premultiplied: Vec<u8> = vec![200, 100, 50, 255, 50, 25, 13, 128, 0, 0, 0, 0, 7, 7, 7, 7];
    let unassociated: Vec<u8> = premultiplied
        .chunks(4)
        .flat_map(|pixel| {
            let alpha = u32::from(pixel[3]);
            let colour = |sample: u8| match alpha {
                0 => 0,
                alpha => ((u32::from(sample) * 255 + alpha / 2) / alpha).min(255) as u8,
            };
            [
                colour(pixel[0]),
                colour(pixel[1]),
                colour(pixel[2]),
                pixel[3],
            ]
        })
        .collect();
    let premultiplied16: Vec<u16> = vec![30_000, 20_000, 10_000, 40_000, 5, 5, 5, 65_535];
    let unassociated16: Vec<u16> = premultiplied16
        .chunks(4)
        .flat_map(|pixel| {
            let alpha = u64::from(pixel[3]);
            let colour =
                |sample: u16| ((u64::from(sample) * 65_535 + alpha / 2) / alpha).min(65_535) as u16;
            [
                colour(pixel[0]),
                colour(pixel[1]),
                colour(pixel[2]),
                pixel[3],
            ]
        })
        .collect();
    let le32 = |values: &[u32]| -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    };
    let signed16_bytes: Vec<u8> = signed16
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let reals_bytes: Vec<u8> = reals.iter().flat_map(|value| value.to_le_bytes()).collect();
    let gray = |name, bytes, size, bits, signed, frame| {
        Case::lossless(name, bytes, size, (1, bits, signed, "MONOCHROME2"), frame)
    };
    let inverted = |name, bytes, bits, signed, frame| {
        Case::lossless(name, bytes, (2, 2), (1, bits, signed, "MONOCHROME1"), frame)
    };
    let with_encoder = |write: &dyn Fn(&mut Encoder<'_>)| {
        let mut out = Cursor::new(Vec::new());
        write(&mut TiffEncoder::new(&mut out).expect("TIFF encoder"));
        out.into_inner()
    };

    // 20 x 18 in four 16 x 16 tiles, the right and bottom ones padded.
    let tiled_pixels: Vec<u8> = (0..20 * 18).map(|index| (index % 251) as u8).collect();
    let tiles = (0..4)
        .map(|tile| {
            let (x0, y0) = (tile % 2 * 16, tile / 2 * 16);
            let mut bytes = vec![0xee; 16 * 16];
            for y in y0..(y0 + 16).min(18) {
                for x in x0..(x0 + 16).min(20) {
                    bytes[(y - y0) * 16 + (x - x0)] = tiled_pixels[y * 20 + x];
                }
            }
            bytes
        })
        .collect();
    let tiled = TiffPage {
        tags: vec![
            (256, TiffValue::Long(vec![20])),
            (257, TiffValue::Long(vec![18])),
            (258, TiffValue::Short(vec![8])),
            (259, TiffValue::Short(vec![1])),
            (262, TiffValue::Short(vec![1])),
            (277, TiffValue::Short(vec![1])),
            (322, TiffValue::Long(vec![16])),
            (323, TiffValue::Long(vec![16])),
        ],
        chunks: tiles,
        tiled: true,
    };
    // Two strips of one row each.
    let mut strips =
        TiffPage::strip((4, 2), &[8], 1, eight[..4].to_vec()).with(278, TiffValue::Long(vec![1]));
    strips.chunks.push(eight[4..].to_vec());
    // Horizontal differencing: each sample is stored minus its left
    // neighbour.
    let differenced: Vec<u8> = eight
        .chunks(4)
        .flat_map(|row| {
            let mut previous = 0_u8;
            row.iter()
                .map(|sample| {
                    let stored = sample.wrapping_sub(previous);
                    previous = *sample;
                    stored
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Three 16-bit pages of one size with a smaller one between them: three
    // frames, each with its own samples.
    let page = |seed: u16| -> Vec<u16> { (0..12).map(|index| seed + index).collect() };
    let stack = with_encoder(&|encoder| {
        for (width, height, samples) in [
            (4, 3, page(100)),
            (2, 2, vec![9; 4]),
            (4, 3, page(2000)),
            (4, 3, page(30_000)),
        ] {
            encoder
                .write_image::<colortype::Gray16>(width, height, &samples)
                .expect("write TIFF page");
        }
    });
    let big = {
        let mut out = Cursor::new(Vec::new());
        TiffEncoder::new_big(&mut out)
            .expect("BigTIFF encoder")
            .write_image::<colortype::Gray16>(3, 2, &sixteen)
            .expect("write BigTIFF page");
        out.into_inner()
    };

    vec![
        gray(
            "gray8-strips.tif",
            files::tiff_file(false, &[strips]).0,
            (2, 4),
            8,
            0,
            eight.clone(),
        ),
        gray(
            "gray8-tiles.tif",
            files::tiff_file(false, &[tiled]).0,
            (18, 20),
            8,
            0,
            tiled_pixels,
        ),
        gray(
            "gray8-predictor.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((4, 2), &[8], 1, differenced)
                    .with(317, TiffValue::Short(vec![2]))],
            )
            .0,
            (2, 4),
            8,
            0,
            eight.clone(),
        ),
        gray(
            "gray16.tif",
            tiff_pages::<colortype::Gray16>(&[(3, 2, &sixteen)]),
            (2, 3),
            16,
            0,
            le16(&sixteen),
        ),
        // A big-endian file is served little endian like any other.
        gray(
            "gray16-big-endian.tif",
            files::tiff_file(true, &[TiffPage::strip((3, 2), &[16], 1, be16(&sixteen))]).0,
            (2, 3),
            16,
            0,
            le16(&sixteen),
        ),
        gray("gray16-bigtiff.tif", big, (2, 3), 16, 0, le16(&sixteen)),
        gray(
            "gray16-lzw.tif",
            with_encoder(&|encoder| {
                encoder
                    .write_image_with_compression::<colortype::Gray16, _>(
                        3,
                        2,
                        compression::Lzw,
                        &sixteen,
                    )
                    .expect("write LZW page")
            }),
            (2, 3),
            16,
            0,
            le16(&sixteen),
        ),
        gray(
            "gray16-deflate.tif",
            with_encoder(&|encoder| {
                encoder
                    .write_image_with_compression::<colortype::Gray16, _>(
                        3,
                        2,
                        compression::Deflate::default(),
                        &sixteen,
                    )
                    .expect("write Deflate page")
            }),
            (2, 3),
            16,
            0,
            le16(&sixteen),
        ),
        gray(
            "gray16-packbits.tif",
            with_encoder(&|encoder| {
                encoder
                    .write_image_with_compression::<colortype::Gray16, _>(
                        3,
                        2,
                        compression::Packbits,
                        &sixteen,
                    )
                    .expect("write PackBits page")
            }),
            (2, 3),
            16,
            0,
            le16(&sixteen),
        ),
        gray(
            "gray32.tif",
            tiff_pages::<colortype::Gray32>(&[(2, 2, &wide)]),
            (2, 2),
            32,
            0,
            le32(&wide),
        ),
        gray(
            "int8.tif",
            tiff_pages::<colortype::GrayI8>(&[(2, 2, &signed8)]),
            (2, 2),
            8,
            1,
            signed8.iter().map(|value| *value as u8).collect(),
        ),
        gray(
            "int16.tif",
            tiff_pages::<colortype::GrayI16>(&[(2, 2, &signed16)]),
            (2, 2),
            16,
            1,
            signed16_bytes.clone(),
        ),
        gray(
            "int32.tif",
            tiff_pages::<colortype::GrayI32>(&[(2, 2, &signed32)]),
            (2, 2),
            32,
            1,
            signed32
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        gray(
            "float32.tif",
            tiff_pages::<colortype::Gray32Float>(&[(2, 2, &reals)]),
            (2, 2),
            32,
            0,
            reals_bytes.clone(),
        ),
        gray(
            "float64.tif",
            tiff_pages::<colortype::Gray64Float>(&[(2, 2, &doubles)]),
            (2, 2),
            64,
            0,
            doubles
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ),
        // WhiteIsZero files serve what they store: the decoder's inversion
        // is not in the raw frame, for integers and floats alike.
        inverted(
            "white-is-zero8.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((2, 2), &[8], 0, vec![0, 1, 200, 255])],
            )
            .0,
            8,
            0,
            vec![0, 1, 200, 255],
        ),
        inverted(
            "white-is-zero16.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip(
                    (2, 2),
                    &[16],
                    0,
                    le16(&[0, 1, 4095, 65_535]),
                )],
            )
            .0,
            16,
            0,
            le16(&[0, 1, 4095, 65_535]),
        ),
        inverted(
            "white-is-zero-int16.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((2, 2), &[16], 0, signed16_bytes.clone())
                    .with(339, TiffValue::Short(vec![2]))],
            )
            .0,
            16,
            1,
            signed16_bytes,
        ),
        inverted(
            "white-is-zero-float.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((2, 2), &[32], 0, reals_bytes.clone())
                    .with(339, TiffValue::Short(vec![3]))],
            )
            .0,
            32,
            0,
            reals_bytes,
        ),
        Case::lossless(
            "rgb8.tif",
            tiff_pages::<colortype::RGB8>(&[(2, 2, &rgb)]),
            (2, 2),
            RGB8,
            rgb,
        ),
        Case::lossless(
            "rgb16.tif",
            tiff_pages::<colortype::RGB16>(&[(2, 2, &rgb16)]),
            (2, 2),
            (3, 16, 0, "RGB"),
            le16(&rgb16),
        ),
        // ExtraSamples 2: unassociated alpha, served as stored.
        Case::lossless(
            "rgba8.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((2, 2), &[8, 8, 8, 8], 2, rgba.clone())
                    .with(338, TiffValue::Short(vec![2]))],
            )
            .0,
            (2, 2),
            RGBA8,
            rgba,
        ),
        // ExtraSamples 1: premultiplied colour is divided by its alpha.
        Case::lossless(
            "rgba8-associated.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((2, 2), &[8, 8, 8, 8], 2, premultiplied)
                    .with(338, TiffValue::Short(vec![1]))],
            )
            .0,
            (2, 2),
            RGBA8,
            unassociated,
        ),
        Case::lossless(
            "rgba16-associated.tif",
            files::tiff_file(
                false,
                &[
                    TiffPage::strip((2, 1), &[16, 16, 16, 16], 2, le16(&premultiplied16))
                        .with(338, TiffValue::Short(vec![1])),
                ],
            )
            .0,
            (1, 2),
            (4, 16, 0, "RGBA"),
            le16(&unassociated16),
        ),
        Case {
            name: "stack.tif",
            bytes: stack,
            size: (3, 4),
            layout: GRAY16,
            frames: vec![le16(&page(100)), le16(&page(2000)), le16(&page(30_000))],
            tolerance: 0,
        },
    ]
}

pub fn webp_cases() -> Vec<Case> {
    let rgb: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30];
    let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 1, 10, 20, 30, 77];
    vec![
        Case::lossless(
            "lossless.webp",
            files::lossless_webp(ExtendedColorType::Rgb8, (2, 2), &rgb),
            (2, 2),
            RGB8,
            rgb,
        ),
        Case::lossless(
            "lossless-alpha.webp",
            files::lossless_webp(ExtendedColorType::Rgba8, (2, 2), &rgba),
            (2, 2),
            RGBA8,
            rgba,
        ),
        Case {
            name: "lossy.webp",
            bytes: files::lossy_webp(),
            size: (16, 16),
            layout: RGB8,
            frames: vec![flat(&[90, 140, 200], 256)],
            tolerance: 4,
        },
        Case {
            name: "lossy-alpha.webp",
            bytes: files::lossy_alpha_webp(),
            size: (16, 16),
            layout: RGBA8,
            frames: vec![flat(&[90, 140, 200, 128], 256)],
            tolerance: 4,
        },
        // Only the first frame of an animation is a frame here.
        Case::lossless(
            "animated.webp",
            files::animated_webp(),
            (8, 8),
            RGB8,
            flat(&[10, 20, 30], 64),
        ),
    ]
}

/// Every file of the raw-frame table, as `(name, bytes)`: the valid inputs
/// the cost tests in `raster_bounds.rs` decode and damage.
pub fn valid_files() -> Vec<(&'static str, Vec<u8>)> {
    let mut cases = png_cases();
    cases.extend(jpeg_cases());
    cases.extend(tiff_cases());
    cases.extend(webp_cases());
    cases
        .into_iter()
        .map(|case| (case.name, case.bytes))
        .collect()
}
