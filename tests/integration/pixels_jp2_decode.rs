use super::support;
use dcmview::pixels::{load_frame, new_cache, FrameRequest, PixelError};
use tempfile::tempdir;

#[tokio::test]
async fn invalid_jp2_payload_surfaces_server_side_decode_error() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("jp2-frames.dcm");
    let frame = vec![
        0x00, 0x00, 0x00, 0x0c, b'j', b'P', b' ', b' ', 0x0d, 0x0a, 0x87, 0x0a,
    ];
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.90", vec![frame.clone()]);

    let file = support::file_entry(path, "1.2.840.10008.1.2.4.90", 1);
    let error = load_frame(
        file,
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect_err("invalid JP2 payload should fail server-side decoding");

    assert!(
        matches!(error, PixelError::Decode { .. }),
        "JP2 display path should decode instead of returning raw bytes: {error}"
    );
}

#[tokio::test]
async fn invalid_jp2_codestream_surfaces_decode_context() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("jp2-fallback.dcm");
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.90", vec![vec![1, 2, 3, 4]]);

    let file = support::file_entry(path, "1.2.840.10008.1.2.4.90", 1);
    let error = load_frame(
        file,
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect_err("invalid JP2 payload should fail fallback decoding");

    assert!(
        matches!(error, PixelError::Decode { .. }),
        "fallback path should surface JP2 decode failure: {error}"
    );
}

#[tokio::test]
async fn jp2_grayscale_display_applies_the_shared_presentation_pipeline() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden-jpeg2000-lossless-u8-single-frame.dcm");
    let report = support::discover(
        &[path],
        dcmview::loader::DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover JPEG 2000 golden fixture");
    let mut file = report.files.into_iter().next().expect("one JPEG 2000 file");
    // Open only the top-left pixel; everything else must take the shutter's
    // white P-value, as it would for native, RLE, or JPEG sources.
    file.series_metadata.presentation.display_shutter = Some(dcmview::types::DisplayShutter {
        shapes: vec![dcmview::types::ShutterShape::Rectangular {
            left_vertical_edge: 1,
            right_vertical_edge: 1,
            upper_horizontal_edge: 1,
            lower_horizontal_edge: 1,
        }],
        presentation_value: u16::MAX,
        presentation_color_cielab: None,
    });

    let frame = load_frame(
        file,
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::FullDynamic,
        },
    )
    .await
    .expect("JPEG 2000 display frame");
    let pixels = image::load_from_memory(&frame.body)
        .expect("decode PNG")
        .into_luma8()
        .into_raw();

    assert!(pixels.len() > 1);
    assert!(
        pixels[1..].iter().all(|&value| value == 255),
        "pixels outside the shutter opening must use the shutter value: {pixels:?}"
    );
}
