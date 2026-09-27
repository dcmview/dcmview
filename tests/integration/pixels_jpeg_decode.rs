use super::support;
use axum::http::{header, HeaderValue};
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::pixels::{load_frame, new_cache, FrameRequest};
use dcmview::server;
use image::ImageFormat;
use std::path::PathBuf;
use tempfile::tempdir;

#[tokio::test]
async fn decodes_requested_jpeg_display_frame_to_png() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("jpeg-frames.dcm");
    let frame0 = support::grayscale_jpeg_fragment_16x16(20);
    let frame1 = support::grayscale_jpeg_fragment_16x16(80);
    support::write_encapsulated_dicom(
        &path,
        "1.2.840.10008.1.2.4.50",
        vec![frame0.clone(), frame1.clone()],
    );

    let file = support::file_entry(path.clone(), "1.2.840.10008.1.2.4.50", 2);
    let request = |frame| FrameRequest {
        frame,
        window_center: None,
        window_width: None,
        window_mode: dcmview::types::WindowMode::Default,
    };

    let first = load_frame(file.clone().into(), new_cache(), request(1))
        .await
        .expect("decoded JPEG frame 1");
    assert_eq!(first.content_type, "image/png");
    let first_image = image::load_from_memory_with_format(first.body.as_ref(), ImageFormat::Png)
        .expect("valid decoded JPEG PNG")
        .to_luma8();
    assert_eq!(first_image.width(), 16);
    assert_eq!(first_image.height(), 16);
    assert_ne!(
        first.body.as_ref(),
        frame1.as_slice(),
        "display endpoint must not return raw JPEG bytes"
    );

    let other = load_frame(file.into(), new_cache(), request(0))
        .await
        .expect("decoded JPEG frame 0");
    assert_ne!(
        first.body, other.body,
        "each frame index must decode its own fragment"
    );
}

#[tokio::test]
async fn jpeg_lossless_decodes_server_side_to_windowed_png() {
    // The committed fixture holds a process-14 codestream with the 4x4 raster
    // 0, 100, ..., 1500 and a 750/1500 default window.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden-jpeg-lossless-u16-single-frame.dcm");
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover JPEG Lossless fixture");
    let test_server = TestServer::new(server::router(support::app_state(report.files)));

    // Even a client that prefers JPEG gets a server-decoded PNG.
    let response = test_server
        .get("/api/file/0/frame/0")
        .add_header(header::ACCEPT, HeaderValue::from_static("image/jpeg"))
        .await;
    response.assert_status_ok();
    assert_eq!(
        response
            .header("content-type")
            .to_str()
            .expect("content type"),
        "image/png"
    );
    let rendered =
        image::load_from_memory_with_format(response.as_bytes().as_ref(), ImageFormat::Png)
            .expect("JPEG Lossless display should be a PNG")
            .to_luma8();
    assert_eq!(rendered.dimensions(), (4, 4));
    // Window 750/1500 spans 0..1500, so each 100-unit step is 17 grey levels.
    let expected = (0_u8..16).map(|step| step * 17).collect::<Vec<_>>();
    assert_eq!(rendered.into_raw(), expected);
}
