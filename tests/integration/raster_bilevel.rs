//! Bilevel TIFF: one-bit gray pages in every compression the viewer
//! decodes, listed by the production loader and read through the router.
//!
//! The files are libtiff's own output of a test card whose every pixel the
//! test computes (`raster_bilevel_files.rs`), so no expected value is one a
//! decoder returned.

use super::raster_bilevel_files::{
    card, fixture, small_fixtures, tag_values, with_tag, with_value,
};
use super::raster_discovery::scan_dir;
use dcmview::api::contracts::{
    RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_DEFAULT_WC, RAW_FRAME_HEADER_DEFAULT_WW,
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
};
use image::ImageDecoder;
use std::fs;
use std::io::Cursor;
use tempfile::tempdir;

/// Every compression, strips, tiles and both fill orders: the raw tier holds
/// the stored bit of each pixel, 0 or 1, and the display shows the card,
/// black and white, inverted once for a page that stores white as zero.
#[tokio::test]
async fn bilevel_pages_serve_their_stored_bits_and_show_black_and_white() {
    let cases = small_fixtures();
    let dir = tempdir().expect("temp dir");
    for (name, _) in &cases {
        fs::write(dir.path().join(name), fixture(name)).expect("write fixture");
    }
    let scan = scan_dir(dir.path()).await;
    assert_eq!(scan.not_loaded(), [], "every file is listed");
    let image = card(37, 21);

    for (name, white_is_zero) in &cases {
        let file = scan.file(name);
        assert_eq!(file["support_state"], "renderable", "{name}");
        assert_eq!(file["raster"]["bit_depth"], 1, "{name}");
        let index = scan.index(name);

        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 200, "{name}: {}", raw.text());
        let photometric = if *white_is_zero {
            "MONOCHROME1"
        } else {
            "MONOCHROME2"
        };
        for (header, value) in [
            (RAW_FRAME_HEADER_SAMPLES_PER_PIXEL, "1"),
            (RAW_FRAME_HEADER_BITS_ALLOCATED, "8"),
            (RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, photometric),
            (RAW_FRAME_HEADER_DEFAULT_WC, "1"),
            (RAW_FRAME_HEADER_DEFAULT_WW, "2"),
        ] {
            assert_eq!(raw.header(header), value, "{name} {header}");
        }
        let stored: Vec<u8> = if *white_is_zero {
            image.iter().map(|value| 1 - value).collect()
        } else {
            image.clone()
        };
        assert!(raw.as_bytes().iter().all(|&value| value <= 1), "{name}");
        assert_eq!(raw.as_bytes().as_ref(), stored.as_slice(), "{name}");

        let display = scan.server.get(&format!("/api/file/{index}/frame/0")).await;
        assert_eq!(display.status_code(), 200, "{name}: {}", display.text());
        let decoder = image::codecs::png::PngDecoder::new(Cursor::new(display.as_bytes().to_vec()))
            .expect("display frame is a PNG");
        assert_eq!(decoder.color_type(), image::ColorType::L8, "{name}");
        let mut pixels = vec![0; decoder.total_bytes() as usize];
        decoder.read_image(&mut pixels).expect("decode display PNG");
        let shown: Vec<u8> = image.iter().map(|value| value * 255).collect();
        assert_eq!(pixels, shown, "{name}");
    }
}

/// Group 4 is decoded for bilevel pages up to 65,535 pixels wide; Group 3
/// and Modified Huffman are not decoded, and neither is a wider Group 4
/// page. A decoded page whose strip is cut short is a decode error, never a
/// frame padded out.
#[tokio::test]
async fn bilevel_compressions_the_viewer_does_not_decode_say_why() {
    let g4 = fixture("g4-37x21.tif");
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "group3.tif",
            with_tag(g4.clone(), 259, 259, 3),
            "raster.unsupported_compression",
        ),
        (
            "modified-huffman.tif",
            with_tag(g4.clone(), 259, 259, 2),
            "raster.unsupported_compression",
        ),
        (
            "wide-group4.tif",
            with_tag(g4.clone(), 256, 256, 65_536),
            "raster.unsupported_compression",
        ),
    ];
    let half = tag_values(&g4, 279)[0] / 2;
    let cut = with_value(g4.clone(), 279, 0, half);

    let dir = tempdir().expect("temp dir");
    for (name, bytes, _) in &cases {
        fs::write(dir.path().join(name), bytes).expect("write raster");
    }
    fs::write(
        dir.path().join("widest-group4.tif"),
        with_tag(g4, 256, 256, 65_535),
    )
    .expect("write raster");
    fs::write(dir.path().join("cut.tif"), cut).expect("write raster");
    let scan = scan_dir(dir.path()).await;

    for (name, _, reason) in &cases {
        let file = scan.file(name);
        assert_eq!(file["support_state"], "unsupported", "{name}");
        assert_eq!(file["support_reason"], *reason, "{name}");
        let index = scan.index(name);
        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 422, "{name}");
    }
    assert_eq!(
        scan.file("widest-group4.tif")["support_state"],
        "renderable"
    );
    assert_eq!(scan.file("cut.tif")["support_state"], "renderable");
    let index = scan.index("cut.tif");
    let raw = scan
        .server
        .get(&format!("/api/file/{index}/frame/0/raw"))
        .await;
    assert_eq!(raw.status_code(), 500, "{}", raw.text());
}
