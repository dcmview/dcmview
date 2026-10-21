//! JPEG-compressed TIFF: gray, RGB and YCbCr pages in strips and tiles,
//! listed by the production loader and read through the router, compared
//! with what libtiff decodes the same files to.

use super::raster_bilevel_files::with_tag;
use super::raster_discovery::scan_dir;
use super::raster_jpeg_files::{
    fixture, largest_difference, libtiff_decode, small_fixtures, with_count, with_frame_precision,
    with_frame_size,
};
use dcmview::api::contracts::{
    RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
};
use std::fs;
use tempfile::tempdir;

/// How far a sample the viewer decodes may be from libtiff's: the two
/// decoders round the inverse DCT each its own way.
const TOLERANCE: u8 = 1;

/// How far a sample of a YCbCr page may be from libtiff's: beside the
/// inverse DCT, the decoders upsample the chroma and convert it to RGB each
/// with its own arithmetic.
const YCBCR_TOLERANCE: u8 = 4;

/// Every fixture is listed renderable as 8-bit gray or RGB, and its raw
/// frame is libtiff's decode of it to within [`TOLERANCE`], or
/// [`YCBCR_TOLERANCE`] for YCbCr: strips with a
/// last strip of fewer rows, tiles reaching past the image, tables shared in
/// `JPEGTables`, YCbCr with 2 x 2 subsampling converted to RGB, and
/// progressive streams. The display frame decodes too.
#[tokio::test]
async fn jpeg_pages_serve_what_libtiff_decodes() {
    let cases = small_fixtures();
    let dir = tempdir().expect("temp dir");
    for (name, _, _) in &cases {
        fs::write(dir.path().join(name), fixture(name)).expect("write fixture");
    }
    let scan = scan_dir(dir.path()).await;
    assert_eq!(scan.not_loaded(), [], "every file is listed");

    for (name, samples, decode) in &cases {
        let file = scan.file(name);
        assert_eq!(file["support_state"], "renderable", "{name}");
        assert_eq!(file["raster"]["bit_depth"], 8, "{name}");
        let index = scan.index(name);
        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 200, "{name}: {}", raw.text());
        let photometric = if *samples == 1 { "MONOCHROME2" } else { "RGB" };
        for (header, value) in [
            (RAW_FRAME_HEADER_SAMPLES_PER_PIXEL, samples.to_string()),
            (RAW_FRAME_HEADER_BITS_ALLOCATED, "8".to_string()),
            (
                RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION,
                photometric.to_string(),
            ),
        ] {
            assert_eq!(raw.header(header), value, "{name} {header}");
        }
        let expected = libtiff_decode(decode);
        let difference = largest_difference(raw.as_bytes(), &expected);
        if std::env::var_os("RASTER_JPEG_REPORT").is_some() {
            eprintln!("{name}: largest difference from libtiff {difference}");
        }
        let tolerance = if name.starts_with("ycbcr") {
            YCBCR_TOLERANCE
        } else {
            TOLERANCE
        };
        assert!(
            difference <= tolerance,
            "{name}: a sample differs from libtiff's by {difference}"
        );
        let display = scan.server.get(&format!("/api/file/{index}/frame/0")).await;
        assert_eq!(display.status_code(), 200, "{name}: {}", display.text());
    }
}

/// A WhiteIsZero JPEG page serves its stored samples, as every WhiteIsZero
/// page does, and is listed as MONOCHROME1 for display to invert.
#[tokio::test]
async fn a_white_is_zero_jpeg_page_serves_its_stored_samples() {
    let dir = tempdir().expect("temp dir");
    let gray = fixture("gray-strips-150x133.tif");
    fs::write(dir.path().join("black-is-zero.tif"), &gray).expect("write");
    fs::write(
        dir.path().join("white-is-zero.tif"),
        with_tag(gray, 262, 262, 0),
    )
    .expect("write");
    let scan = scan_dir(dir.path()).await;
    let mut frames = Vec::new();
    for name in ["black-is-zero.tif", "white-is-zero.tif"] {
        let index = scan.index(name);
        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 200, "{name}: {}", raw.text());
        frames.push((
            raw.header(RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION),
            raw.as_bytes().to_vec(),
        ));
    }
    assert_eq!(frames[0].0, "MONOCHROME2");
    assert_eq!(frames[1].0, "MONOCHROME1");
    assert_eq!(frames[0].1, frames[1].1);
}

/// A strip or tile whose frame header is not what its page lays out, or a
/// page whose tables are larger than any writer makes, is listed renderable
/// and its frame is a decode error, never a frame of another size.
#[tokio::test]
async fn jpeg_strips_and_tiles_that_differ_from_their_page_are_decode_errors() {
    let tiles = fixture("ycbcr-tiles-150x133.tif");
    let strips = fixture("gray-strips-150x133.tif");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "larger-tile.tif",
            with_frame_size(tiles.clone(), 324, 16_000, 16_000),
        ),
        ("smaller-tile.tif", with_frame_size(tiles, 324, 64, 64)),
        (
            "taller-strip.tif",
            with_frame_size(strips.clone(), 273, 150, 17),
        ),
        (
            "twelve-bit.tif",
            with_frame_precision(strips.clone(), 273, 12),
        ),
        ("large-tables.tif", with_count(strips, 347, 64 * 1024 + 1)),
    ];
    let dir = tempdir().expect("temp dir");
    for (name, bytes) in &cases {
        fs::write(dir.path().join(name), bytes).expect("write raster");
    }
    let scan = scan_dir(dir.path()).await;
    for (name, _) in &cases {
        assert_eq!(scan.file(name)["support_state"], "renderable", "{name}");
        let index = scan.index(name);
        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 500, "{name}: {}", raw.text());
    }
}

/// A page without `JPEGTables`, whose strip carries its own tables, decodes
/// as the same page with them.
#[tokio::test]
async fn a_jpeg_page_without_shared_tables_decodes() {
    let shared = fixture("ycbcr-progressive-150x133.tif");
    // The tag is renamed to one no reader knows; its strip, a whole stream
    // written by jpegtran, holds the same tables.
    let own = with_tag(shared.clone(), 347, 65_000, 0);
    let dir = tempdir().expect("temp dir");
    fs::write(dir.path().join("shared.tif"), shared).expect("write");
    fs::write(dir.path().join("own.tif"), own).expect("write");
    let scan = scan_dir(dir.path()).await;
    let mut frames = Vec::new();
    for name in ["shared.tif", "own.tif"] {
        let index = scan.index(name);
        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 200, "{name}: {}", raw.text());
        frames.push(raw.as_bytes().to_vec());
    }
    assert_eq!(frames[0], frames[1]);
}

/// A JPEG-compressed page after an uncompressed one of the same layout is
/// a frame of the file, decoded to what libtiff decodes it to.
#[tokio::test]
async fn a_jpeg_page_after_an_uncompressed_one_decodes() {
    let dir = tempdir().expect("temp dir");
    let name = "gray-two-pages-150x133.tif";
    fs::write(dir.path().join(name), fixture(name)).expect("write");
    let scan = scan_dir(dir.path()).await;
    assert_eq!(scan.file(name)["frame_count"], 2);
    let index = scan.index(name);
    let raw = scan
        .server
        .get(&format!("/api/file/{index}/frame/1/raw"))
        .await;
    assert_eq!(raw.status_code(), 200, "{}", raw.text());
    let difference =
        largest_difference(raw.as_bytes(), &libtiff_decode("gray-150x133.libtiff.tif"));
    assert!(difference <= TOLERANCE, "{difference} from libtiff's");
}
