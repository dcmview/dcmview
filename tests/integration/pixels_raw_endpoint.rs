use super::support;
use axum_test::TestServer;
use dcmview::pixels::{load_raw_frame, new_raw_cache, RawFrameRequest};
use dcmview::server;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a named response header as a string.
fn header_str(response: &axum_test::TestResponse, name: &str) -> String {
    response
        .header(name)
        .to_str()
        .expect("valid header utf-8")
        .to_string()
}

fn header_u32(response: &axum_test::TestResponse, name: &str) -> u32 {
    header_str(response, name)
        .parse()
        .unwrap_or_else(|_| panic!("header {name} is not u32"))
}

fn header_f64(response: &axum_test::TestResponse, name: &str) -> f64 {
    header_str(response, name)
        .parse()
        .unwrap_or_else(|_| panic!("header {name} is not f64"))
}

fn maybe_header_f64(response: &axum_test::TestResponse, name: &str) -> Option<f64> {
    response
        .maybe_header(name)
        .and_then(|v| v.to_str().ok()?.parse().ok())
}

// ---------------------------------------------------------------------------
// /raw endpoint: metadata and body shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raw_endpoint_returns_correct_metadata_headers_for_uncompressed() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-meta.dcm");
    // 2×2 frame, 2 frames, u16 LE values 0..=7
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000, 500, 1500, 2500, 3500],
        Some("1500"),
        Some("3000"),
    );

    let report = support::discover(
        &[path],
        dcmview::loader::DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover raw metadata fixture");
    let app = server::router(support::app_state(report.files));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/frame/0/raw").await;
    response.assert_status_ok();

    // Metadata headers
    assert_eq!(header_u32(&response, "X-Frame-Rows"), 2);
    assert_eq!(header_u32(&response, "X-Frame-Columns"), 2);
    assert_eq!(header_u32(&response, "X-Frame-Bits-Allocated"), 16);
    assert_eq!(header_u32(&response, "X-Frame-Pixel-Representation"), 0);
    assert_eq!(header_u32(&response, "X-Frame-Samples-Per-Pixel"), 1);
    assert_eq!(
        header_str(&response, "X-Frame-Photometric-Interpretation"),
        "MONOCHROME2"
    );
    assert!((header_f64(&response, "X-Frame-Rescale-Slope") - 1.0).abs() < 1e-9);
    assert!((header_f64(&response, "X-Frame-Rescale-Intercept") - 0.0).abs() < 1e-9);

    // Default window headers
    let wc = maybe_header_f64(&response, "X-Frame-Default-Wc");
    let ww = maybe_header_f64(&response, "X-Frame-Default-Ww");
    assert_eq!(wc, Some(1500.0), "default WC should be present");
    assert_eq!(ww, Some(3000.0), "default WW should be present");

    // Body: 2×2 frame of u16 LE = 8 bytes
    let body = response.as_bytes();
    assert_eq!(body.len(), 8, "2×2 frame of u16 = 8 bytes");

    // Verify first pixel value: 0 → [0x00, 0x00] LE
    assert_eq!(body[0], 0x00);
    assert_eq!(body[1], 0x00);
    // Verify second pixel value: 1000 → 0x03E8 LE → [0xE8, 0x03]
    assert_eq!(body[2], 0xE8);
    assert_eq!(body[3], 0x03);
}

#[tokio::test]
async fn raw_endpoint_x_cache_miss_then_hit() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-cache.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        None,
        None,
    );
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;

    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let first = test_server.get("/api/file/0/frame/0/raw").await;
    first.assert_status_ok();
    assert_eq!(
        header_str(&first, "X-Cache"),
        "MISS",
        "first raw request must be MISS"
    );

    let second = test_server.get("/api/file/0/frame/0/raw").await;
    second.assert_status_ok();
    assert_eq!(
        header_str(&second, "X-Cache"),
        "HIT",
        "repeat raw request must be HIT"
    );

    // Bodies must be identical
    assert_eq!(
        first.as_bytes(),
        second.as_bytes(),
        "cached body must match original"
    );
}

#[tokio::test]
async fn concurrent_and_abandoned_requests_share_one_decode() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-shared.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        None,
        None,
    );
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;

    let cache = new_raw_cache();
    let responses = futures::future::join_all((0..6).map(|_| {
        load_raw_frame(
            entry.clone().into(),
            cache.clone(),
            RawFrameRequest { frame: 0 },
        )
    }))
    .await;
    let decoded = responses
        .into_iter()
        .map(|response| response.expect("raw frame"))
        .filter(|response| !response.cache_hit)
        .count();
    assert_eq!(decoded, 1, "identical concurrent requests decode once");

    // A request dropped mid-decode (a client abort) still leaves its frame
    // for the next request.
    let other = new_raw_cache();
    let abandoned = load_raw_frame(
        entry.clone().into(),
        other.clone(),
        RawFrameRequest { frame: 0 },
    );
    let _ = tokio::time::timeout(std::time::Duration::ZERO, abandoned).await;
    let next = load_raw_frame(entry.into(), other, RawFrameRequest { frame: 0 })
        .await
        .expect("raw frame");
    assert!(next.cache_hit, "the abandoned decode is reused");
}

#[tokio::test]
async fn unsupported_layouts_are_422_for_display_and_only_unreadable_ones_for_raw() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("layouts.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        None,
        None,
    );
    let entry = |photometric: &str, rows: u32| {
        let mut entry = support::file_entry(path.clone(), "1.2.840.10008.1.2.1", 1);
        entry.rows = rows;
        entry.columns = 2;
        entry.photometric_interpretation = photometric.to_string();
        entry
    };
    // Display cannot present an unknown photometric interpretation, but the
    // stored samples are still readable.
    let server = TestServer::new(server::router(support::app_state(vec![entry("HSL", 2)])));
    let display = server.get("/api/file/0/frame/0").await;
    display.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        display.json::<serde_json::Value>()["code"],
        "unsupported_pixel_layout"
    );
    server
        .get("/api/file/0/frame/0/raw")
        .await
        .assert_status_ok();

    // Zero rows leaves nothing to read on either path.
    let server = TestServer::new(server::router(support::app_state(vec![entry(
        "MONOCHROME2",
        0,
    )])));
    for url in ["/api/file/0/frame/0", "/api/file/0/frame/0/raw"] {
        let response = server.get(url).await;
        response.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(response.json::<serde_json::Value>()["error"]
            .as_str()
            .is_some_and(|error| error.contains("pixel_layout.invalid_geometry")));
    }
}

#[tokio::test]
async fn raw_endpoint_returns_404_for_out_of_range_frame() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-oob.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        None,
        None,
    );
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;

    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/frame/99/raw").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn raw_endpoint_returns_404_for_file_without_pixel_data() {
    // Use an entry with has_pixels = false.
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-no-pixels.dcm");
    support::write_uncompressed_u16_dicom(&path, "1.2.840.10008.1.2.1", 1, 1, vec![0], None, None);
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.has_pixels = false;

    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/frame/0/raw").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn raw_endpoint_returns_422_for_unsupported_transfer_syntax() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-unsupported.dcm");
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.81", vec![vec![1, 2, 3, 4]]);

    let app = server::router(support::app_state(vec![support::file_entry(
        path,
        "1.2.840.10008.1.2.4.81",
        1,
    )]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/frame/0/raw").await;
    assert_eq!(
        response.status_code(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "JPEG-LS Near-Lossless must return 422 on /raw"
    );
}

#[tokio::test]
async fn raw_endpoint_returns_422_for_multicomponent_jpeg2000() {
    // A 2x2, three-component JP2 codestream generated by OpenJPEG.
    let rgb_jp2 = vec![
        0x00, 0x00, 0x00, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a, 0x00, 0x00, 0x00,
        0x14, 0x66, 0x74, 0x79, 0x70, 0x6a, 0x70, 0x32, 0x20, 0x00, 0x00, 0x00, 0x00, 0x6a, 0x70,
        0x32, 0x20, 0x00, 0x00, 0x00, 0x2d, 0x6a, 0x70, 0x32, 0x68, 0x00, 0x00, 0x00, 0x16, 0x69,
        0x68, 0x64, 0x72, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x03, 0x0f, 0x07,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x63, 0x6f, 0x6c, 0x72, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x10, 0x00, 0x00, 0x00, 0x97, 0x6a, 0x70, 0x32, 0x63, 0xff, 0x4f, 0xff, 0x51, 0x00,
        0x2f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x0f, 0x01, 0x01, 0x0f, 0x01, 0x01, 0x0f, 0x01,
        0x01, 0xff, 0x52, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x04, 0x04, 0x00, 0x01,
        0xff, 0x5c, 0x00, 0x04, 0x40, 0x80, 0xff, 0x64, 0x00, 0x25, 0x00, 0x01, 0x43, 0x72, 0x65,
        0x61, 0x74, 0x65, 0x64, 0x20, 0x62, 0x79, 0x20, 0x4f, 0x70, 0x65, 0x6e, 0x4a, 0x50, 0x45,
        0x47, 0x20, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x32, 0x2e, 0x35, 0x2e, 0x34,
        0xff, 0x90, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f, 0x00, 0x01, 0xff, 0x93, 0xcf,
        0xfc, 0x30, 0x14, 0x08, 0x90, 0xde, 0x98, 0x47, 0x80, 0xdf, 0xf8, 0x90, 0x18, 0x01, 0xca,
        0xbf, 0xff, 0xd9,
    ];

    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-rgb-jp2.dcm");
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.90", vec![rgb_jp2]);
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.4.90", 1);
    entry.samples_per_pixel = 3;
    entry.photometric_interpretation = "RGB".to_string();
    let test_server = TestServer::new(server::router(support::app_state(vec![entry])));

    let response = test_server.get("/api/file/0/frame/0/raw").await;
    response.assert_status_unprocessable_entity();
    let payload: serde_json::Value = response.json();
    assert_eq!(payload["code"], "unsupported_pixel_layout");
}

#[tokio::test]
async fn raw_jpeg_transport_decodes_to_8bit_samples() {
    // A valid 1×1 white JPEG: SOF0 with one 1×1 pixel = 0xFF.
    // Construct a minimal JFIF JPEG representing a 1×1 white grayscale image.
    // We write it with the encapsulated path and verify the /raw endpoint decodes it.
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-jpeg.dcm");

    // Minimal valid 1×1 gray JPEG (values: one white pixel = 0xFF)
    // Using a hand-crafted minimal JFIF from the libjpeg test suite.
    // This is the smallest valid JPEG that decodes to a single 8-bit pixel.
    let jpeg_bytes: Vec<u8> = vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06,
        0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B,
        0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20,
        0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29, 0x2C, 0x30, 0x31,
        0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF,
        0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00,
        0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
        0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05,
        0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21,
        0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
        0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A,
        0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
        0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56,
        0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75,
        0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93,
        0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9,
        0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
        0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
        0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
        0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xFB, 0xD3,
        0xFF, 0xD9,
    ];
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.50", vec![jpeg_bytes]);

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.4.50", 1);
    entry.pixel_representation = 1;
    entry.samples_per_pixel = 3;
    entry.photometric_interpretation = "YBR_FULL".to_string();
    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/frame/0/raw").await;
    response.assert_status_ok();

    // JPEG Baseline always decodes to 8-bit
    assert_eq!(
        header_u32(&response, "X-Frame-Bits-Allocated"),
        8,
        "JPEG raw must be 8-bit"
    );
    assert_eq!(
        header_u32(&response, "X-Frame-Pixel-Representation"),
        0,
        "JPEG raw must be unsigned"
    );
    assert_eq!(
        header_u32(&response, "X-Frame-Samples-Per-Pixel"),
        1,
        "JPEG raw is normalized to grayscale"
    );
    assert_eq!(
        header_str(&response, "X-Frame-Photometric-Interpretation"),
        "MONOCHROME2",
        "JPEG raw metadata must describe the normalized response bytes"
    );

    // Body length = rows × columns × 1 byte
    let rows = header_u32(&response, "X-Frame-Rows");
    let cols = header_u32(&response, "X-Frame-Columns");
    let body = response.as_bytes();
    assert_eq!(
        body.len() as u32,
        rows * cols,
        "raw JPEG body length must equal rows×columns"
    );
}

#[tokio::test]
async fn raw_endpoint_multiframe_second_frame_has_correct_pixels() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-multiframe.dcm");
    // 2×2 with 2 frames: frame 0 = [0,1000,2000,3000], frame 1 = [500,1500,2500,3500]
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000, 500, 1500, 2500, 3500],
        None,
        None,
    );
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 2);
    entry.rows = 2;
    entry.columns = 2;

    let cache = new_raw_cache();

    let frame0 = load_raw_frame(
        entry.clone().into(),
        cache.clone(),
        RawFrameRequest { frame: 0 },
    )
    .await
    .expect("frame 0");
    let frame1 = load_raw_frame(entry.into(), cache, RawFrameRequest { frame: 1 })
        .await
        .expect("frame 1");

    // Bodies must differ (different pixel values)
    assert_ne!(
        frame0.body.as_ref(),
        frame1.body.as_ref(),
        "frames with different pixel values must produce different raw bodies"
    );

    // Verify frame 0 first pixel = 0x0000 (u16 LE)
    assert_eq!(frame0.body[0], 0x00);
    assert_eq!(frame0.body[1], 0x00);

    // Verify frame 1 first pixel = 500 = 0x01F4 LE → [0xF4, 0x01]
    assert_eq!(frame1.body[0], 0xF4, "frame 1 first pixel low byte");
    assert_eq!(frame1.body[1], 0x01, "frame 1 first pixel high byte");
}

#[tokio::test]
async fn raw_native_frame_ignores_nested_icon_pixel_data() {
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::tags;
    use dicom_object::InMemDicomObject;

    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("icon.dcm");
    let image_samples = vec![100_u16, 200, 300, 400];
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        image_samples.clone(),
        None,
        None,
    );
    // An Icon Image Sequence sorts before (7FE0,0010) and carries its own
    // Pixel Data; the frame reader must skip it and read the image's pixels.
    let mut object = dicom_object::open_file(&path).expect("reopen fixture");
    let icon = InMemDicomObject::from_element_iter([
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(2_u16)),
        DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::from(vec![9_u8; 4]),
        ),
    ]);
    object.put(DataElement::new(
        tags::ICON_IMAGE_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(vec![icon]),
    ));
    object.write_to_file(&path).expect("write icon fixture");

    let mut file = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    file.rows = 2;
    file.columns = 2;
    let raw = load_raw_frame(file.into(), new_raw_cache(), RawFrameRequest { frame: 0 })
        .await
        .expect("raw native frame");

    let expected = image_samples
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect::<Vec<_>>();
    assert_eq!(raw.body.as_ref(), expected.as_slice());
}
