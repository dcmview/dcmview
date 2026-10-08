//! The heap a decode holds as the frame grows (`pixels::raster_decode_heap_limit`).
//!
//! The limit has a part that does not depend on the image, and at the sizes
//! the other tests use that part is all of it. These files are large enough
//! for the frame to dominate, so what they pin is the factor: every decoder
//! path holds at most six times the frame and four times the file, with
//! less than a megabyte beside. A frame at the pixel limit is 2 GiB and is
//! not decoded here; each path's working memory is a multiple of its frame,
//! so the multiple measured at these sizes is the one that holds there.

use super::bounds::{decode, list, noise, MIB};
use super::raster_files::{self as files, CountedFile, TiffPage, TiffValue};
use dcmview::pixels;
use image::ExtendedColorType;

/// A side of the square stand-ins: 1.5 megapixels.
const SIDE: u32 = 1248;

fn riff_payload(webp: &[u8]) -> &[u8] {
    &webp[12..]
}

fn stand_ins() -> Vec<(&'static str, Vec<u8>)> {
    let side = SIDE as usize;
    let pixels = side * side;
    let gray = noise(pixels);
    let rgb = noise(pixels * 3);
    let rgba = noise(pixels * 4);

    // A palette of two colours with transparency, one bit a pixel: the
    // frame is four bytes for each bit stored.
    let bits = noise(side.div_ceil(8) * side);
    let palette = files::png_of(
        png::ColorType::Indexed,
        png::BitDepth::One,
        (SIDE, SIDE),
        &bits,
        |encoder| {
            encoder.set_palette(vec![0, 0, 0, 255, 255, 255]);
            encoder.set_trns(vec![0, 255]);
        },
    );

    // The embedded progressive, four-channel and lossy files are a few
    // blocks each. With the size in their headers raised they are files that
    // end early: the decoder allocates everything a full image needs, which
    // is what is measured, and decodes what is there.
    let resized_jpeg = |mut jpeg: Vec<u8>| {
        let marker = files::jpeg_frame_marker(&jpeg);
        let size = (SIDE as u16).to_be_bytes();
        jpeg[marker + 4..marker + 6].copy_from_slice(&size);
        jpeg[marker + 6..marker + 8].copy_from_slice(&size);
        jpeg
    };
    let mut lossy = files::lossy_webp();
    lossy[26..28].copy_from_slice(&(SIDE as u16).to_le_bytes());
    lossy[28..30].copy_from_slice(&(SIDE as u16).to_le_bytes());

    // An animation of one frame that covers the canvas.
    let frame = files::lossless_webp(ExtendedColorType::Rgba8, (SIDE, SIDE), &rgba);
    let mut animation = files::vp8x(0x12, (SIDE, SIDE));
    animation.extend(files::riff_chunk(b"ANIM", &[0; 6]));
    let mut anmf = vec![0; 6];
    anmf.extend_from_slice(&(SIDE - 1).to_le_bytes()[..3]);
    anmf.extend_from_slice(&(SIDE - 1).to_le_bytes()[..3]);
    anmf.extend_from_slice(&[100, 0, 0, 0]);
    anmf.extend_from_slice(riff_payload(&frame));
    animation.extend(files::riff_chunk(b"ANMF", &anmf));

    // 64-bit floats in deflated tiles with the floating-point predictor;
    // zeros are their own prediction.
    let tile = files::zlib(&vec![0; 256 * 256 * 8]);
    let tiles = (SIDE as usize).div_ceil(256).pow(2);
    let floats = TiffPage {
        tags: vec![
            (256, TiffValue::Long(vec![SIDE])),
            (257, TiffValue::Long(vec![SIDE])),
            (258, TiffValue::Short(vec![64])),
            (259, TiffValue::Short(vec![8])),
            (262, TiffValue::Short(vec![1])),
            (277, TiffValue::Short(vec![1])),
            (317, TiffValue::Short(vec![3])),
            (322, TiffValue::Long(vec![256])),
            (323, TiffValue::Long(vec![256])),
            (339, TiffValue::Short(vec![3])),
        ],
        chunks: vec![tile; tiles],
        tiled: true,
    };
    // Colour with associated alpha, 16 bits a sample, big endian.
    let deep = TiffPage::strip((SIDE, SIDE), &[16; 4], 2, noise(pixels * 8))
        .with(338, TiffValue::Short(vec![1]));

    vec![
        (
            "gray.png",
            files::png_of(
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                (SIDE, SIDE),
                &gray,
                |_| {},
            ),
        ),
        ("palette.png", palette),
        (
            "deep.png",
            files::png_of(
                png::ColorType::Rgba,
                png::BitDepth::Sixteen,
                (SIDE, SIDE),
                &noise(pixels * 8),
                |_| {},
            ),
        ),
        (
            "interlaced.png",
            files::png_adam7_gray8((side, side), &gray),
        ),
        (
            "gray.jpg",
            files::baseline_jpeg(ExtendedColorType::L8, (SIDE, SIDE), &gray),
        ),
        (
            "colour.jpg",
            files::baseline_jpeg(ExtendedColorType::Rgb8, (SIDE, SIDE), &rgb),
        ),
        ("progressive.jpg", resized_jpeg(files::progressive_jpeg())),
        ("subsampled.jpg", resized_jpeg(files::subsampled_jpeg())),
        ("four-channel.jpg", resized_jpeg(files::cmyk_jpeg())),
        (
            "lossless.webp",
            files::lossless_webp(ExtendedColorType::Rgb8, (SIDE, SIDE), &rgb),
        ),
        ("lossless-alpha.webp", frame),
        ("lossy.webp", lossy),
        ("animation.webp", files::webp_from_chunks(&animation)),
        (
            "gray.tif",
            files::tiff_file(false, &[TiffPage::strip((SIDE, SIDE), &[8], 1, gray)]).0,
        ),
        ("floats.tif", files::tiff_file(false, &[floats]).0),
        ("deep.tif", files::tiff_file(true, &[deep]).0),
    ]
}

/// Whatever the format and layout, a decode holds no more than six times
/// the frame and four times the file beside a fixed megabyte.
#[tokio::test]
async fn the_heap_of_a_decode_grows_with_the_frame_by_no_more_than_its_factor() {
    let files = stand_ins();
    let listed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for (name, bytes) in &files {
        let entry = scan.entry(name);
        assert_eq!((entry.rows, entry.columns), (SIDE, SIDE), "{name}");
        let frame = pixels::raster_frame_bytes(entry).expect("frame size");
        assert!(frame >= MIB, "{name}: a frame large enough to measure");
        let length = bytes.len() as u64;
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        let limit = pixels::raster_decode_heap_limit(entry, length).expect("heap limit");
        let scaled = limit - pixels::RASTER_DECODE_HEAP_BASE_BYTES + MIB;
        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!(
                "{name}: frame {frame}, file {length}, heap {} ({:.2} x frame), {}",
                cost.heap,
                cost.heap as f64 / frame as f64,
                match &cost.result {
                    Ok(_) => "decoded".to_string(),
                    Err(error) => format!("{error:#}"),
                },
            );
        }
        assert!(
            cost.heap <= scaled,
            "{name}: {} bytes of heap for a frame of {frame} from a file of {length}; \
             six frames, four files and a megabyte are {scaled}",
            cost.heap
        );
        if let Ok(decoded) = &cost.result {
            assert_eq!(decoded.bytes.len() as u64, frame, "{name}");
        }
    }
}
