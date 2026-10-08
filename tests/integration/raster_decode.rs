//! What the frames of raster image files hold and show
//! (`docs/design/image-formats.md` sections 5.2 to 5.4, 6.2 and 9).
//!
//! Every case is a file written from sample values chosen in the test
//! (`raster_cases.rs`, `raster_files.rs`), listed by the production loader
//! and read through the router. The raw
//! tier must hold the samples the file stores, whatever its decoder makes of
//! them, and the display tier must show them once windowed, once inverted
//! and with alpha flattened. No expected value is one a decoder returned.

use super::raster_cases::{
    be16, flat, jpeg_cases, le16, png_cases, tiff_cases, tiff_pages, tiled_image, tiles_stored_in,
    webp_cases,
};
use super::raster_discovery::{scan_dir, scan_into, Scan};
use super::raster_files::{self as files, TiffPage, TiffValue};
use dcmview::api::contracts::{
    DISPLAY_FRAME_HEADER_WINDOW_CENTER, DISPLAY_FRAME_HEADER_WINDOW_WIDTH,
    RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_COLUMNS, RAW_FRAME_HEADER_DEFAULT_WC,
    RAW_FRAME_HEADER_DEFAULT_WW, RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION,
    RAW_FRAME_HEADER_PIXEL_REPRESENTATION, RAW_FRAME_HEADER_ROWS,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
};
use dcmview::loader::{DiscoverOptions, FormatSelection};
use dcmview::masking::Masker;
use dcmview::server::FileRegistry;
use image::{ExtendedColorType, ImageDecoder};
use serde_json::{json, Value};
use std::fs;
use std::io::Cursor;
use std::sync::Arc;
use tempfile::tempdir;
use tiff::encoder::colortype;

// ---------------------------------------------------------------------------
// Requests

async fn raw(scan: &Scan, name: &str, frame: usize) -> axum_test::TestResponse {
    let index = scan.index(name);
    scan.server
        .get(&format!("/api/file/{index}/frame/{frame}/raw"))
        .await
}

/// What a display frame shows.
struct Shown {
    response: axum_test::TestResponse,
    colour: image::ColorType,
    pixels: Vec<u8>,
    /// The ICC profile the PNG embeds.
    profile: Option<Vec<u8>>,
}

async fn display(scan: &Scan, name: &str, query: &str) -> Shown {
    let index = scan.index(name);
    let response = scan
        .server
        .get(&format!("/api/file/{index}/frame/0{query}"))
        .await;
    assert_eq!(response.status_code(), 200, "{name}: {}", response.text());
    assert_eq!(response.header("content-type"), "image/png", "{name}");
    let mut decoder =
        image::codecs::png::PngDecoder::new(Cursor::new(response.as_bytes().to_vec()))
            .expect("display frame is a PNG");
    let profile = decoder.icc_profile().expect("read profile");
    let colour = decoder.color_type();
    let mut pixels = vec![0; decoder.total_bytes() as usize];
    decoder.read_image(&mut pixels).expect("decode display PNG");
    Shown {
        response,
        colour,
        pixels,
        profile,
    }
}

fn assert_close(actual: &[u8], expected: &[u8], tolerance: u8, context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}: frame size");
    if tolerance == 0 {
        assert_eq!(actual, expected, "{context}");
        return;
    }
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            actual.abs_diff(*expected) <= tolerance,
            "{context}: sample {index} is {actual}, encoded from {expected}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests

/// The raw tier of every supported layout holds the samples the file
/// stores, in the layout the catalog declares for it.
#[tokio::test]
async fn raw_frames_hold_the_samples_each_file_stores() {
    let mut cases = png_cases();
    cases.extend(jpeg_cases());
    cases.extend(tiff_cases());
    cases.extend(webp_cases());
    let dir = tempdir().expect("temp dir");
    for case in &cases {
        fs::write(dir.path().join(case.name), &case.bytes).expect("write raster");
    }
    let scan = scan_dir(dir.path()).await;
    assert_eq!(scan.not_loaded(), [], "every case is listed");

    for case in &cases {
        let name = case.name;
        let file = scan.file(name);
        assert_eq!(file["support_state"], "renderable", "{name}");
        assert!(file["support_reason"].is_null(), "{name}");
        assert_eq!(file["frame_count"], case.frames.len(), "{name}");

        let (samples, bits, signed, photometric) = case.layout;
        for (frame, expected) in case.frames.iter().enumerate() {
            let response = raw(&scan, name, frame).await;
            assert_eq!(response.status_code(), 200, "{name}: {}", response.text());
            for (header, value) in [
                (RAW_FRAME_HEADER_ROWS, case.size.0.to_string()),
                (RAW_FRAME_HEADER_COLUMNS, case.size.1.to_string()),
                (RAW_FRAME_HEADER_SAMPLES_PER_PIXEL, samples.to_string()),
                (RAW_FRAME_HEADER_BITS_ALLOCATED, bits.to_string()),
                (RAW_FRAME_HEADER_PIXEL_REPRESENTATION, signed.to_string()),
                (
                    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION,
                    photometric.to_string(),
                ),
            ] {
                assert_eq!(response.header(header), value.as_str(), "{name} {header}");
            }
            assert_close(
                response.as_bytes(),
                expected,
                case.tolerance,
                &format!("{name} frame {frame}"),
            );
        }
    }

    // One pixel of a four-sample frame is its four samples.
    let index = scan.index("rgba8.png");
    let pixel = scan
        .server
        .get(&format!(
            "/api/file/{index}/frame/0/raw/pixel?row=0&column=1"
        ))
        .await;
    assert_eq!(pixel.status_code(), 200, "{}", pixel.text());
    assert_eq!(pixel.header(RAW_FRAME_HEADER_SAMPLES_PER_PIXEL), "4");
    assert_eq!(pixel.as_bytes().as_ref(), [0, 255, 0, 128]);
}

/// The display tier: gray is windowed over its stored range by default,
/// WhiteIsZero is inverted once, alpha is flattened over black, 16-bit
/// colour is reduced to 8 bits, and colour is never windowed.
#[tokio::test]
async fn display_frames_window_gray_once_and_flatten_alpha() {
    use png::{BitDepth, ColorType};
    let full = |bits: u32| (f64::from(1_u32 << (bits - 1)), f64::from(1_u32 << bits));
    let gray_alpha: Vec<u8> = vec![0, 0, 100, 128, 200, 255, 255, 1];
    let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 10, 20, 30, 1];
    let rgb16: Vec<u16> = vec![
        65_535, 0, 256, 1, 2, 3, 40_000, 50_000, 60_000, 128, 129, 32_768,
    ];
    let rgba16: Vec<u16> = vec![65_535, 0, 0, 65_535, 1000, 2000, 3000, 32_768];
    let over_black = |sample: u64, alpha: u64, max: u64| (sample * alpha + max / 2) / max;
    let to8 = |sample: u64| ((sample * 255 + 32_767) / 65_535) as u8;
    let none = |_: &mut png::Encoder<'_, &mut Vec<u8>>| {};

    struct Expected {
        name: &'static str,
        bytes: Vec<u8>,
        query: &'static str,
        /// The catalog's default window, `(center, width)`.
        window: Option<(f64, f64)>,
        colour: image::ColorType,
        pixels: Vec<u8>,
    }
    let gray = |name, bytes, query, window, pixels| Expected {
        name,
        bytes,
        query,
        window,
        colour: image::ColorType::L8,
        pixels,
    };
    let colour = |name, bytes, query, pixels| Expected {
        name,
        bytes,
        query,
        window: None,
        colour: image::ColorType::Rgb8,
        pixels,
    };
    let cases = vec![
        // 8 bits: shown as stored.
        gray(
            "gray8.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::Eight,
                (4, 1),
                &[0, 1, 128, 255],
                none,
            ),
            "",
            Some(full(8)),
            vec![0, 1, 128, 255],
        ),
        // Fewer bits: the stored range spans black to white.
        gray(
            "gray1.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::One,
                (2, 1),
                &[0b0100_0000],
                none,
            ),
            "",
            Some(full(1)),
            vec![0, 255],
        ),
        gray(
            "gray4.png",
            files::png_of(
                ColorType::Grayscale,
                BitDepth::Four,
                (4, 1),
                &[0x01, 0x8f],
                none,
            ),
            "",
            Some(full(4)),
            vec![0, 17, 136, 255],
        ),
        // A declared TIFF range is the window: 0 to 4095 here.
        gray(
            "declared-range.tif",
            files::tiff_file(
                false,
                &[
                    TiffPage::strip((4, 1), &[16], 1, le16(&[0, 2047, 4095, 60_000]))
                        .with(281, TiffValue::Short(vec![4095])),
                ],
            )
            .0,
            "",
            Some((2048.0, 4096.0)),
            vec![0, 127, 255, 255],
        ),
        // WhiteIsZero: the largest stored value is black.
        gray(
            "white-is-zero8.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((4, 1), &[8], 0, vec![0, 1, 200, 255])],
            )
            .0,
            "",
            Some(full(8)),
            vec![255, 254, 55, 0],
        ),
        // Tiles stored last first are shown where they belong.
        gray(
            "tiles-reversed.tif",
            tiles_stored_in(&[3, 2, 1, 0]),
            "",
            Some(full(8)),
            tiled_image(),
        ),
        // Samples without a default window, shown over their own range.
        gray(
            "float32.tif",
            tiff_pages::<colortype::Gray32Float>(&[(4, 1, &[0.0, 0.25, 0.5, 1.0])]),
            "?mode=full_dynamic",
            None,
            vec![0, 64, 128, 255],
        ),
        gray(
            "int32.tif",
            tiff_pages::<colortype::GrayI32>(&[(3, 1, &[-1_000_000, 0, 1_000_000])]),
            "?mode=full_dynamic",
            None,
            vec![0, 128, 255],
        ),
        // Gray with alpha: windowed, then multiplied by alpha.
        gray(
            "gray-alpha8.png",
            files::png_of(
                ColorType::GrayscaleAlpha,
                BitDepth::Eight,
                (4, 1),
                &gray_alpha,
                none,
            ),
            "",
            Some(full(8)),
            gray_alpha
                .chunks(2)
                .map(|pixel| over_black(pixel[0].into(), pixel[1].into(), 255) as u8)
                .collect(),
        ),
        // Colour is never windowed, whatever the request says.
        colour(
            "rgb8.png",
            files::png_of(
                ColorType::Rgb,
                BitDepth::Eight,
                (2, 1),
                &[255, 0, 1, 10, 20, 30],
                none,
            ),
            "?wc=5&ww=10",
            vec![255, 0, 1, 10, 20, 30],
        ),
        colour(
            "rgba8.png",
            files::png_of(ColorType::Rgba, BitDepth::Eight, (4, 1), &rgba, none),
            "",
            rgba.chunks(4)
                .flat_map(|pixel| {
                    pixel[..3]
                        .iter()
                        .map(|sample| over_black((*sample).into(), pixel[3].into(), 255) as u8)
                        .collect::<Vec<_>>()
                })
                .collect(),
        ),
        colour(
            "rgb16.png",
            files::png_of(
                ColorType::Rgb,
                BitDepth::Sixteen,
                (4, 1),
                &be16(&rgb16),
                none,
            ),
            "",
            rgb16.iter().map(|sample| to8((*sample).into())).collect(),
        ),
        colour(
            "rgba16.png",
            files::png_of(
                ColorType::Rgba,
                BitDepth::Sixteen,
                (2, 1),
                &be16(&rgba16),
                none,
            ),
            "",
            rgba16
                .chunks(4)
                .flat_map(|pixel| {
                    pixel[..3]
                        .iter()
                        .map(|sample| to8(over_black((*sample).into(), pixel[3].into(), 65_535)))
                        .collect::<Vec<_>>()
                })
                .collect(),
        ),
    ];
    let dir = tempdir().expect("temp dir");
    for case in &cases {
        fs::write(dir.path().join(case.name), &case.bytes).expect("write raster");
    }
    // The same 16-bit samples with and without sBIT.
    let sixteen = be16(&[0, 100, 3000, 4095, 20_000, 65_535]);
    fs::write(
        dir.path().join("plain16.png"),
        files::png_from_chunks((3, 2), 16, 0, false, &[files::png_idat(&sixteen, 6)]),
    )
    .expect("write PNG");
    fs::write(
        dir.path().join("sbit12.png"),
        files::png_from_chunks(
            (3, 2),
            16,
            0,
            false,
            &[
                files::png_chunk(b"sBIT", &[12]),
                files::png_idat(&sixteen, 6),
            ],
        ),
    )
    .expect("write PNG");
    let scan = scan_dir(dir.path()).await;

    for case in &cases {
        let name = case.name;
        let window = case
            .window
            .map(|(center, width)| json!({ "center": center, "width": width }));
        assert_eq!(
            scan.file(name)["default_window"],
            window.unwrap_or(Value::Null),
            "{name}"
        );
        let shown = display(&scan, name, case.query).await;
        assert_eq!(shown.colour, case.colour, "{name}");
        assert_eq!(shown.pixels, case.pixels, "{name}");
        if let Some((center, width)) = case.window {
            // The default window is the one applied, and the raw tier
            // offers it to a client that windows for itself.
            let raw = raw(&scan, name, 0).await;
            for (response, center_header, width_header) in [
                (
                    &shown.response,
                    DISPLAY_FRAME_HEADER_WINDOW_CENTER,
                    DISPLAY_FRAME_HEADER_WINDOW_WIDTH,
                ),
                (
                    &raw,
                    RAW_FRAME_HEADER_DEFAULT_WC,
                    RAW_FRAME_HEADER_DEFAULT_WW,
                ),
            ] {
                assert_eq!(
                    response.header(center_header),
                    center.to_string().as_str(),
                    "{name} {center_header}"
                );
                assert_eq!(
                    response.header(width_header),
                    width.to_string().as_str(),
                    "{name} {width_header}"
                );
            }
        }
    }

    // sBIT is informational: it narrows no window. The two files show the
    // same frame, over a window wider than twelve bits.
    assert_eq!(
        scan.file("sbit12.png")["raster"]["significant_bits"],
        json!([12])
    );
    assert!(scan.file("sbit12.png")["default_window"].is_null());
    let plain = display(&scan, "plain16.png", "").await;
    let marked = display(&scan, "sbit12.png", "").await;
    assert_eq!(marked.pixels, plain.pixels);
    let width: f64 = marked
        .response
        .header(DISPLAY_FRAME_HEADER_WINDOW_WIDTH)
        .to_str()
        .expect("window width")
        .parse()
        .expect("window width");
    assert!(width > 4096.0, "window width {width}");
}

/// A colour frame carries the file's profile to the browser when it
/// describes the pixels served, and no profile otherwise.
#[tokio::test]
async fn display_frames_carry_only_a_profile_that_describes_their_pixels() {
    let rgb: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30];
    let valid = files::icc_profile(b"RGB ", 600, 0x5a);
    let png_with_profile = |profile: &[u8]| {
        let mut chunk = b"profile\0\0".to_vec();
        chunk.extend(files::zlib(profile));
        files::png_from_chunks(
            (2, 2),
            8,
            2,
            false,
            &[files::png_chunk(b"iCCP", &chunk), files::png_idat(&rgb, 6)],
        )
    };
    let mut wrong_signature = valid.clone();
    wrong_signature[36..40].copy_from_slice(b"nope");
    let mut wrong_length = valid.clone();
    wrong_length[..4].copy_from_slice(&599_u32.to_be_bytes());
    // A WebP with a profile chunk between its header and its image.
    let lossless = files::lossless_webp(ExtendedColorType::Rgb8, (2, 2), &rgb);
    let mut extended = files::vp8x(0x20, (2, 2));
    extended.extend(files::riff_chunk(b"ICCP", &valid));
    extended.extend_from_slice(&lossless[12..]);
    // A JPEG with the profile in one APP2 segment after the start marker.
    let mut segment = b"ICC_PROFILE\0\x01\x01".to_vec();
    segment.extend_from_slice(&valid);
    let mut jpeg = files::baseline_jpeg(ExtendedColorType::Rgb8, (2, 2), &rgb);
    let mut app2 = vec![0xff, 0xe2];
    app2.extend_from_slice(&(segment.len() as u16 + 2).to_be_bytes());
    app2.extend(segment);
    jpeg.splice(2..2, app2.clone());
    let mut cmyk_claiming_rgb = files::cmyk_jpeg();
    cmyk_claiming_rgb.splice(2..2, app2);

    // (name, bytes, the profile the display frame carries)
    type Case<'a> = (&'a str, Vec<u8>, Option<&'a [u8]>);
    let cases: Vec<Case<'_>> = vec![
        ("profile.png", png_with_profile(&valid), Some(&valid)),
        (
            "profile.webp",
            files::webp_from_chunks(&extended),
            Some(&valid),
        ),
        ("profile.jpg", jpeg, Some(&valid)),
        (
            "profile.tif",
            files::tiff_file(
                false,
                &[TiffPage::strip((2, 2), &[8, 8, 8], 2, rgb.clone())
                    .with(34675, TiffValue::Bytes(valid.clone()))],
            )
            .0,
            Some(&valid),
        ),
        // A profile for another colour space does not describe RGB pixels.
        (
            "gray-profile.png",
            png_with_profile(&files::icc_profile(b"GRAY", 600, 0x5a)),
            None,
        ),
        (
            "bad-signature.png",
            png_with_profile(&wrong_signature),
            None,
        ),
        ("bad-length.png", png_with_profile(&wrong_length), None),
        // No profile of a four-channel JPEG is used, whatever space it
        // claims: the pixels served are an approximate RGB.
        ("cmyk-profile.jpg", files::cmyk_jpeg_with_profile(), None),
        ("cmyk-rgb-profile.jpg", cmyk_claiming_rgb, None),
    ];
    let dir = tempdir().expect("temp dir");
    for (name, bytes, _) in &cases {
        fs::write(dir.path().join(name), bytes).expect("write raster");
    }
    let scan = scan_dir(dir.path()).await;

    for (name, _, profile) in cases {
        assert_eq!(scan.file(name)["raster"]["has_icc"], true, "{name}");
        let shown = display(&scan, name, "").await;
        assert_eq!(shown.colour, image::ColorType::Rgb8, "{name}");
        assert_eq!(shown.profile.as_deref(), profile, "{name}");
    }
}

/// Redaction boxes and a masked session apply to a raster frame as they do
/// to a DICOM frame: a box is black in the display frame, the thumbnail and
/// the presentation layer and blank in the raw samples, and masking leaves
/// the pixels alone.
#[tokio::test]
async fn redaction_and_masking_apply_to_raster_frames_and_thumbnails() {
    // 32 x 32: a gray ramp, and one opaque colour.
    let ramp: Vec<u8> = (0..32 * 32)
        .map(|index| 64 + (index % 32) as u8 * 4)
        .collect();
    let tinted = flat(&[200, 100, 50, 255], 32 * 32);
    let dir = tempdir().expect("temp dir");
    for (name, color, data) in [
        ("ramp.png", png::ColorType::Grayscale, &ramp),
        ("tinted.png", png::ColorType::Rgba, &tinted),
    ] {
        fs::write(
            dir.path().join(name),
            files::png_of(color, png::BitDepth::Eight, (32, 32), data, |_| {}),
        )
        .expect("write PNG");
    }
    let masked = scan_into(
        &[dir.path().to_path_buf()],
        DiscoverOptions {
            recursive: true,
            filters: Vec::new(),
            formats: FormatSelection::all(),
        },
        FileRegistry::masked(Arc::new(Masker::new())),
    )
    .await;
    let scan = scan_dir(dir.path()).await;

    async fn thumbnail(scan: &Scan, name: &str) -> (axum_test::TestResponse, image::RgbImage) {
        let index = scan.index(name);
        let response = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/thumbnail?size=64"))
            .await;
        assert_eq!(response.status_code(), 200, "{name}: {}", response.text());
        assert_eq!(response.header("content-type"), "image/jpeg", "{name}");
        let image = image::load_from_memory(response.as_bytes()).expect("thumbnail is a JPEG");
        (response, image.to_rgb8())
    }

    // A masked session serves a raster's pixels unchanged: it hides
    // identifiers, and a raster's pixels are not one.
    for name in ["ramp.png", "tinted.png"] {
        assert_eq!(
            display(&masked, name, "").await.pixels,
            display(&scan, name, "").await.pixels,
            "{name}"
        );
        assert_eq!(
            raw(&masked, name, 0).await.as_bytes(),
            raw(&scan, name, 0).await.as_bytes(),
            "{name}"
        );
        assert_eq!(
            thumbnail(&masked, name).await.1.dimensions(),
            (32, 32),
            "{name}"
        );
    }

    // Before any box: the thumbnail is the frame, and is kept.
    let (first, image) = thumbnail(&scan, "tinted.png").await;
    assert_eq!(first.header("x-cache"), "MISS");
    let pixel = image.get_pixel(4, 4).0;
    for (channel, expected) in pixel.iter().zip([200_u8, 100, 50]) {
        assert!(channel.abs_diff(expected) <= 6, "thumbnail pixel {pixel:?}");
    }
    assert_eq!(
        thumbnail(&scan, "tinted.png").await.0.header("x-cache"),
        "HIT"
    );

    // Rows 0 to 16, columns 0 to 8 of both files.
    for name in ["ramp.png", "tinted.png"] {
        let index = scan.index(name);
        scan.server
            .put(&format!("/api/file/{index}/redactions"))
            .json(&json!({ "num_roi": 1, "roi_coords": [[0, 0, 16, 8]], "roi_frames": [] }))
            .await
            .assert_status_ok();
    }
    let inside = |row: usize, column: usize| row < 16 && column < 8;

    let shown = display(&scan, "ramp.png", "").await;
    let raw_ramp = raw(&scan, "ramp.png", 0).await;
    for row in 0..32 {
        for column in 0..32 {
            let at = row * 32 + column;
            // Raw samples carry the frame's darkest value under a box.
            let (expected, stored) = if inside(row, column) {
                (0, 64)
            } else {
                (ramp[at], ramp[at])
            };
            assert_eq!(shown.pixels[at], expected, "display ({row}, {column})");
            assert_eq!(raw_ramp.as_bytes()[at], stored, "raw ({row}, {column})");
        }
    }

    let shown = display(&scan, "tinted.png", "").await;
    let raw_tinted = raw(&scan, "tinted.png", 0).await;
    let (redacted, image) = thumbnail(&scan, "tinted.png").await;
    assert_eq!(redacted.header("x-cache"), "MISS", "the boxes changed");
    let index = scan.index("tinted.png");
    let layer = scan
        .server
        .get(&format!("/api/file/{index}/frame/0/presentation-layer"))
        .await;
    assert_eq!(layer.status_code(), 200, "{}", layer.text());
    let layer = image::load_from_memory(layer.as_bytes())
        .expect("presentation layer is a PNG")
        .to_rgba8();
    for (row, column) in [(0, 0), (15, 7), (16, 8), (31, 31)] {
        let at = row * 32 + column;
        let (colour, samples, cover) = if inside(row, column) {
            ([0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 255])
        } else {
            ([200, 100, 50], [200, 100, 50, 255], [0, 0, 0, 0])
        };
        assert_eq!(
            shown.pixels[at * 3..at * 3 + 3],
            colour,
            "display ({row}, {column})"
        );
        assert_eq!(
            raw_tinted.as_bytes()[at * 4..at * 4 + 4],
            samples,
            "raw ({row}, {column})"
        );
        assert_eq!(
            layer.get_pixel(column as u32, row as u32).0,
            cover,
            "presentation layer ({row}, {column})"
        );
    }
    // Well inside the box the thumbnail is black; well outside, the colour.
    assert!(image.get_pixel(2, 4).0.iter().all(|channel| *channel <= 6));
    assert!(image.get_pixel(24, 24).0[0].abs_diff(200) <= 6);
}

/// A frame larger than the raw cache is served every time and never kept,
/// and what the catalog says about the file does not depend on the budget.
#[tokio::test]
async fn a_frame_larger_than_the_raw_cache_is_served_and_not_kept() {
    let pixels: Vec<u8> = (0..64 * 64).map(|index| (index % 251) as u8).collect();
    let dir = tempdir().expect("temp dir");
    fs::write(
        dir.path().join("image.png"),
        files::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            (64, 64),
            &pixels,
            |_| {},
        ),
    )
    .expect("write PNG");
    let listed = scan_dir(dir.path()).await;
    let state = super::support::app_state(listed.entries.clone()).with_cache_budget(
        dcmview::pixels::CacheBudget {
            frame_bytes: 1024,
            raw_bytes: 1024,
            overlay_bytes: 1024,
            thumbnail_bytes: 1024,
        },
    );
    let server = axum_test::TestServer::new(dcmview::server::router(state));

    let catalog: Value = server.get("/api/files").await.json();
    assert_eq!(catalog["files"][0]["support_state"], "renderable");
    for _ in 0..2 {
        let raw = server.get("/api/file/0/frame/0/raw").await;
        assert_eq!(raw.status_code(), 200, "{}", raw.text());
        assert_eq!(raw.header("x-cache"), "MISS");
        assert_eq!(raw.as_bytes().as_ref(), pixels.as_slice());
    }
}
