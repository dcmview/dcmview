use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use dicom_object::open_file;
use image::ImageFormat;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn committed_fixtures_have_unique_sop_instance_uids() {
    let mut seen = std::collections::HashMap::new();
    for entry in std::fs::read_dir(fixture_path("")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("dcm") {
            continue;
        }
        let object = dicom_object::OpenFileOptions::new()
            .read_until(dicom_dictionary_std::tags::FLOAT_PIXEL_DATA)
            .open_file(&path)
            .unwrap();
        let uid = object
            .element(dicom_dictionary_std::tags::SOP_INSTANCE_UID)
            .unwrap()
            .to_str()
            .unwrap()
            .into_owned();
        assert!(
            !uid.is_empty(),
            "{} has no SOP Instance UID",
            path.display()
        );
        if let Some(previous) = seen.insert(uid.clone(), path.clone()) {
            panic!(
                "duplicate SOP Instance UID {uid}: {} and {}",
                previous.display(),
                path.display()
            );
        }
    }
}

fn decode_u16_le(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn apply_window(samples: &[u16], center: f64, width: f64) -> Vec<u8> {
    let low = center - width / 2.0;
    let high = center + width / 2.0;
    samples
        .iter()
        .map(|value| {
            (((((*value as f64).clamp(low, high)) - low) / (high - low)) * 255.0).round() as u8
        })
        .collect()
}

fn apply_window_f64(samples: &[f64], center: f64, width: f64) -> Vec<u8> {
    let low = center - width / 2.0;
    let high = center + width / 2.0;
    samples
        .iter()
        .map(|value| ((((value.clamp(low, high)) - low) / (high - low)) * 255.0).round() as u8)
        .collect()
}

fn assert_pixels_close(actual: &[u8], expected: &[u8], tolerance: u8) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "pixel buffers must have the same length"
    );
    for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
        let delta = actual.abs_diff(*expected);
        assert!(
            delta <= tolerance,
            "pixel mismatch at index {index}: actual={actual} expected={expected} delta={delta}"
        );
    }
}

#[tokio::test]
async fn golden_uncompressed_fixture_matches_raw_and_display_contracts() {
    let path = fixture_path("golden-uncompressed-u16-multiframe.dcm");
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover uncompressed golden fixture");
    assert_eq!(report.files.len(), 1);

    let file = &report.files[0];
    assert!(file.has_pixels);
    assert_eq!(file.frame_count, 3);
    assert_eq!(file.rows, 4);
    assert_eq!(file.columns, 4);
    assert_eq!(file.default_window.expect("default window").center, 750.0);

    let test_server = TestServer::new(server::router(support::app_state(report.files)));

    let raw = test_server.get("/api/file/0/frame/1/raw").await;
    raw.assert_status_ok();
    let expected_frame1 = vec![
        50, 150, 250, 350, 450, 550, 650, 750, 850, 950, 1050, 1150, 1250, 1350, 1450, 1550,
    ];
    assert_eq!(decode_u16_le(raw.as_bytes().as_ref()), expected_frame1);

    let display = test_server.get("/api/file/0/frame/0").await;
    display.assert_status_ok();
    let image = image::load_from_memory_with_format(display.as_bytes().as_ref(), ImageFormat::Png)
        .expect("valid png")
        .to_luma8();
    let expected_frame0 = vec![
        0, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1100, 1200, 1300, 1400, 1500,
    ];
    assert_eq!(
        image.into_raw(),
        apply_window(&expected_frame0, 750.0, 1500.0)
    );
}

#[tokio::test]
async fn golden_single_frame_jpeg_fixture_round_trips_server_decode() {
    let path = fixture_path("golden-jpeg-baseline-single-frame.dcm");
    let obj = open_file(&path).expect("open JPEG fixture");
    let pixel_data = obj.element_by_name("PixelData").expect("pixel data");
    let fragment = pixel_data.fragments().expect("jpeg fragments")[0].clone();
    let expected = image::load_from_memory(&fragment)
        .expect("decode source jpeg")
        .to_luma8()
        .into_raw();

    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover JPEG fixture");
    let expected_samples = expected
        .iter()
        .map(|value| f64::from(*value))
        .collect::<Vec<_>>();
    let window = dcmview::pixels::resolve_window(
        None,
        None,
        report.files[0].default_window,
        &expected_samples,
    )
    .expect("JPEG fixture resolves fallback window");
    let expected_windowed =
        apply_window_f64(&expected_samples, window.center, window.width.max(1.0));
    let test_server = TestServer::new(server::router(support::app_state(report.files)));

    let response = test_server.get("/api/file/0/frame/0").await;
    response.assert_status_ok();
    let decoded =
        image::load_from_memory_with_format(response.as_bytes().as_ref(), ImageFormat::Png)
            .expect("decoded PNG")
            .to_luma8();
    assert_eq!(decoded.width(), 16);
    assert_eq!(decoded.height(), 16);
    assert_pixels_close(&decoded.into_raw(), &expected_windowed, 2);
    assert_ne!(response.as_bytes().as_ref(), fragment.as_slice());
}

#[tokio::test]
async fn golden_large_single_frame_jpeg_fixture_exercises_viewer_geometry() {
    let path = fixture_path("golden-jpeg-baseline-large-single-frame.dcm");
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover large JPEG fixture");
    assert_eq!(report.files.len(), 1);

    let file = &report.files[0];
    assert!(file.has_pixels);
    assert_eq!(file.frame_count, 1);
    assert_eq!(file.rows, 2560);
    assert_eq!(file.columns, 3328);
    assert_eq!(file.default_window.expect("default window").center, 128.0);

    let test_server = TestServer::new(server::router(support::app_state(report.files)));
    let response = test_server.get("/api/file/0/frame/0").await;
    response.assert_status_ok();

    let decoded =
        image::load_from_memory_with_format(response.as_bytes().as_ref(), ImageFormat::Png)
            .expect("decoded large PNG")
            .to_luma8();
    assert_eq!(decoded.width(), 3328);
    assert_eq!(decoded.height(), 2560);
}

#[tokio::test]
async fn golden_multiframe_jpeg_fixture_has_offset_table_and_decodes_by_frame() {
    let path = fixture_path("golden-jpeg-baseline-multiframe-bot.dcm");
    let obj = open_file(&path).expect("open multiframe jpeg fixture");
    let pixel_data = obj.element_by_name("PixelData").expect("pixel data");
    let offset_table = pixel_data.offset_table().expect("offset table");
    assert_eq!(offset_table.len(), 3);
    assert_eq!(offset_table[0], 0);
    assert!(offset_table[1] > 0);
    assert!(offset_table[2] > offset_table[1]);

    let fragments = pixel_data.fragments().expect("fragments");
    assert_eq!(fragments.len(), 3);
    let expected = image::load_from_memory(&fragments[2])
        .expect("decode frame 2 jpeg")
        .to_luma8()
        .into_raw();

    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover multiframe jpeg fixture");
    let test_server = TestServer::new(server::router(support::app_state(report.files)));

    let frame0 = test_server.get("/api/file/0/frame/0").await;
    let frame2 = test_server.get("/api/file/0/frame/2").await;
    frame0.assert_status_ok();
    frame2.assert_status_ok();

    let decoded_frame2 =
        image::load_from_memory_with_format(frame2.as_bytes().as_ref(), ImageFormat::Png)
            .expect("decoded PNG")
            .to_luma8()
            .into_raw();
    // Lossy JPEG decode can differ by a few luminance levels across decoder builds.
    assert_pixels_close(&decoded_frame2, &expected, 3);
    assert_ne!(frame0.as_bytes(), frame2.as_bytes());
}

#[tokio::test]
async fn golden_sr_fixture_reports_no_pixels_and_rejects_frame_access() {
    let path = fixture_path("golden-no-pixels-sr.dcm");
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover sr fixture");
    assert_eq!(report.files.len(), 1);
    assert!(!report.files[0].has_pixels);

    let test_server = TestServer::new(server::router(support::app_state(report.files)));
    let files = test_server
        .get("/api/files")
        .await
        .json::<serde_json::Value>();
    assert_eq!(files["files"][0]["has_pixels"], false);

    let frame = test_server.get("/api/file/0/frame/0").await;
    assert_eq!(frame.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn golden_image_metadata_without_pixel_data_reports_no_pixels() {
    let path = fixture_path("golden-image-no-pixels.dcm");
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover image metadata without pixels fixture");
    assert_eq!(report.files.len(), 1);

    let file = &report.files[0];
    assert!(!file.has_pixels);
    assert_eq!(file.rows, 16);
    assert_eq!(file.columns, 16);

    let test_server = TestServer::new(server::router(support::app_state(report.files)));
    let frame = test_server.get("/api/file/0/frame/0").await;
    assert_eq!(frame.status_code(), StatusCode::NOT_FOUND);
}
