use super::support;
use axum::http::StatusCode;
use axum_test::{TestResponse, TestServer};
use dcmview::loader::{self, DiscoverOptions};
use dcmview::pixels;
use dcmview::pixels::Codec;
use dcmview::server;
use dcmview::types::{WindowMode, WindowPreset};
use image::ImageFormat;
use std::path::PathBuf;
use tempfile::tempdir;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn header(response: &TestResponse, name: &str) -> String {
    response
        .header(name)
        .to_str()
        .expect("response header should be valid UTF-8")
        .to_string()
}

async fn assert_compressed_fixture_contract(
    name: &str,
    transfer_syntax_uid: &str,
    rows: u32,
    columns: u32,
    bits_allocated: u32,
    default_window: (f64, f64),
    expected_raw: Vec<u8>,
) {
    let report = loader::discover(
        &[fixture_path(name)],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover compressed golden fixture");
    assert_eq!(report.files.len(), 1);

    let file = &report.files[0];
    assert!(file.has_pixels);
    assert_eq!(file.frame_count, 1);
    assert_eq!(file.rows, rows);
    assert_eq!(file.columns, columns);
    assert_eq!(file.bits_allocated, bits_allocated);
    assert_eq!(file.samples_per_pixel, 1);
    assert_eq!(file.photometric_interpretation, "MONOCHROME2");
    assert_eq!(file.transfer_syntax_uid, transfer_syntax_uid);

    let test_server = TestServer::new(server::router(support::app_state(report.files)));

    let first_display = test_server.get("/api/file/0/frame/0").await;
    first_display.assert_status_ok();
    assert_eq!(header(&first_display, "content-type"), "image/png");
    assert_eq!(header(&first_display, "x-cache"), "MISS");
    let display_image =
        image::load_from_memory_with_format(first_display.as_bytes().as_ref(), ImageFormat::Png)
            .expect("display endpoint should return a valid PNG");
    assert_eq!(display_image.height(), rows);
    assert_eq!(display_image.width(), columns);

    let second_display = test_server.get("/api/file/0/frame/0").await;
    second_display.assert_status_ok();
    assert_eq!(header(&second_display, "x-cache"), "HIT");
    assert_eq!(first_display.as_bytes(), second_display.as_bytes());

    let first_raw = test_server.get("/api/file/0/frame/0/raw").await;
    first_raw.assert_status_ok();
    assert_eq!(
        header(&first_raw, "content-type"),
        "application/octet-stream"
    );
    assert_eq!(header(&first_raw, "x-cache"), "MISS");
    assert_eq!(header(&first_raw, "x-frame-rows"), rows.to_string());
    assert_eq!(header(&first_raw, "x-frame-columns"), columns.to_string());
    assert_eq!(
        header(&first_raw, "x-frame-bits-allocated"),
        bits_allocated.to_string()
    );
    assert_eq!(header(&first_raw, "x-frame-pixel-representation"), "0");
    assert_eq!(header(&first_raw, "x-frame-samples-per-pixel"), "1");
    assert_eq!(
        header(&first_raw, "x-frame-photometric-interpretation"),
        "MONOCHROME2"
    );
    assert_eq!(header(&first_raw, "x-frame-rescale-slope"), "1");
    assert_eq!(header(&first_raw, "x-frame-rescale-intercept"), "0");
    assert_eq!(
        header(&first_raw, "x-frame-default-wc"),
        default_window.0.to_string()
    );
    assert_eq!(
        header(&first_raw, "x-frame-default-ww"),
        default_window.1.to_string()
    );
    assert_eq!(first_raw.as_bytes().as_ref(), expected_raw);

    let second_raw = test_server.get("/api/file/0/frame/0/raw").await;
    second_raw.assert_status_ok();
    assert_eq!(header(&second_raw, "x-cache"), "HIT");
    assert_eq!(first_raw.as_bytes(), second_raw.as_bytes());
}

#[tokio::test]
async fn jpeg_lossless_fixture_satisfies_display_and_raw_contracts() {
    let samples = [
        0_u16, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1100, 1200, 1300, 1400, 1500,
    ];
    let expected_raw = samples
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();

    assert_compressed_fixture_contract(
        "golden-jpeg-lossless-u16-single-frame.dcm",
        "1.2.840.10008.1.2.4.70",
        4,
        4,
        16,
        (750.0, 1500.0),
        expected_raw,
    )
    .await;
}

#[tokio::test]
async fn jpeg2000_lossless_fixture_satisfies_display_and_raw_contracts() {
    let mut expected_raw = Vec::with_capacity(16 * 16);
    for row in 0_u8..16 {
        expected_raw.extend_from_slice(&[row * 17; 16]);
    }

    assert_compressed_fixture_contract(
        "golden-jpeg2000-lossless-u8-single-frame.dcm",
        "1.2.840.10008.1.2.4.90",
        16,
        16,
        8,
        (127.5, 255.0),
        expected_raw,
    )
    .await;
}

#[tokio::test]
async fn deflated_explicit_vr_little_endian_satisfies_display_and_raw_contracts() {
    const UID: &str = "1.2.840.10008.1.2.1.99";
    let directory = tempdir().expect("temporary fixture directory");
    let path = directory.path().join("deflated-u16.dcm");
    let samples = [0_u16, 85, 170, 255];
    support::write_uncompressed_u16_dicom(&path, UID, 2, 2, samples.to_vec(), None, None);

    let report = loader::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover deflated fixture");
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].transfer_syntax_uid, UID);
    assert_eq!(pixels::codec_for_syntax(UID), Some(Codec::Native));
    assert_eq!(
        pixels::classify_pixel_support(&report.files[0]).state,
        dcmview::api::contracts::SupportState::Renderable
    );

    let test_server = TestServer::new(server::router(support::app_state(report.files)));
    let display = test_server
        .get("/api/file/0/frame/0?mode=full_dynamic")
        .await;
    display.assert_status_ok();
    assert_eq!(header(&display, "content-type"), "image/png");
    assert_eq!(header(&display, "x-cache"), "MISS");
    let rendered =
        image::load_from_memory_with_format(display.as_bytes().as_ref(), ImageFormat::Png)
            .expect("deflated display should return PNG")
            .to_luma8();
    assert_eq!(rendered.dimensions(), (2, 2));
    assert_eq!(rendered.into_raw(), [0, 85, 171, 255]);

    let raw = test_server.get("/api/file/0/frame/0/raw").await;
    raw.assert_status_ok();
    assert_eq!(header(&raw, "content-type"), "application/octet-stream");
    assert_eq!(header(&raw, "x-cache"), "MISS");
    assert_eq!(header(&raw, "x-frame-bits-allocated"), "16");
    let expected_raw = samples
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    assert_eq!(raw.as_bytes().as_ref(), expected_raw);
    let raw_repeat = test_server.get("/api/file/0/frame/0/raw").await;
    raw_repeat.assert_status_ok();
    assert_eq!(header(&raw_repeat, "x-cache"), "HIT");
    assert_eq!(raw_repeat.as_bytes(), raw.as_bytes());
}

#[tokio::test]
async fn rle_lossless_reconstructs_standard_most_significant_byte_planes() {
    let dir = tempdir().expect("temporary fixture directory");
    let path = dir.path().join("rle-u16.dcm");
    let mut fragment = vec![0_u8; 64];
    fragment[0..4].copy_from_slice(&2_u32.to_le_bytes());
    fragment[4..8].copy_from_slice(&64_u32.to_le_bytes());
    fragment[8..12].copy_from_slice(&67_u32.to_le_bytes());
    // Annex G orders the most-significant plane before the least-significant
    // plane. Each segment is a two-byte PackBits literal run.
    fragment.extend_from_slice(&[1, 0x12, 0xab, 1, 0x34, 0xcd]);
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.5", vec![fragment]);

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.5", 1);
    entry.rows = 1;
    entry.columns = 2;
    let test_server = TestServer::new(server::router(support::app_state(vec![entry])));

    let display = test_server.get("/api/file/0/frame/0").await;
    display.assert_status_ok();
    assert_eq!(header(&display, "content-type"), "image/png");
    assert_eq!(header(&display, "x-cache"), "MISS");
    let image = image::load_from_memory_with_format(display.as_bytes().as_ref(), ImageFormat::Png)
        .expect("RLE display should be a valid PNG");
    assert_eq!((image.width(), image.height()), (2, 1));

    let raw = test_server.get("/api/file/0/frame/0/raw").await;
    raw.assert_status_ok();
    assert_eq!(header(&raw, "x-cache"), "MISS");
    assert_eq!(raw.as_bytes().as_ref(), [0x34, 0x12, 0xcd, 0xab]);
}

#[tokio::test]
async fn jpeg2000_display_applies_rescale_before_every_window_mode() {
    let report = loader::discover(
        &[fixture_path("golden-jpeg2000-lossless-u8-single-frame.dcm")],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover JPEG 2000 golden fixture");
    let mut file = report
        .files
        .into_iter()
        .next()
        .expect("fixture should produce one file");

    // The codestream contains rows 0, 17, ..., 255. A negative affine rescale
    // reverses that ramp in modality space, making missing or late rescale
    // application visible for explicit, DICOM-default, and full-dynamic modes.
    file.rescale_slope = -2.0;
    file.rescale_intercept = 510.0;
    file.default_window = Some(WindowPreset {
        center: 255.0,
        width: 340.0,
    });

    let cases = [
        (
            pixels::FrameRequest {
                frame: 0,
                window_center: Some(255.0),
                window_width: Some(170.0),
                window_mode: WindowMode::Default,
            },
            [
                255, 255, 255, 255, 255, 255, 205, 154, 103, 51, 0, 0, 0, 0, 0, 0,
            ],
        ),
        (
            pixels::FrameRequest {
                frame: 0,
                window_center: None,
                window_width: None,
                window_mode: WindowMode::Default,
            },
            [
                255, 255, 255, 243, 217, 192, 166, 141, 115, 90, 64, 38, 13, 0, 0, 0,
            ],
        ),
        (
            pixels::FrameRequest {
                frame: 0,
                // Full-dynamic mode must ignore both these values and the DICOM
                // default while still operating on rescaled samples.
                window_center: Some(1.0),
                window_width: Some(1.0),
                window_mode: WindowMode::FullDynamic,
            },
            [
                255, 238, 221, 204, 187, 170, 153, 136, 119, 102, 85, 68, 51, 34, 17, 0,
            ],
        ),
    ];

    for (request, expected_rows) in cases {
        let response = pixels::load_frame(file.clone(), pixels::new_cache(), request)
            .await
            .expect("JPEG 2000 display decode");
        let rendered =
            image::load_from_memory_with_format(response.body.as_ref(), ImageFormat::Png)
                .expect("display response should be PNG")
                .to_luma8()
                .into_raw();
        let expected = expected_rows
            .into_iter()
            .flat_map(|value| [value; 16])
            .collect::<Vec<_>>();
        assert_eq!(rendered, expected);
    }
}

#[test]
fn codec_table_routes_every_supported_transfer_syntax() {
    let cases = [
        ("1.2.840.10008.1.2.4.50", Some(Codec::JpegBaseline)),
        ("1.2.840.10008.1.2.4.51", None),
        ("1.2.840.10008.1.2.4.57", Some(Codec::JpegLossless)),
        ("1.2.840.10008.1.2.4.70", Some(Codec::JpegLossless)),
        ("1.2.840.10008.1.2.4.90", Some(Codec::Jpeg2000)),
        ("1.2.840.10008.1.2.4.91", None),
        ("1.2.840.10008.1.2.4.110", Some(Codec::JpegXl)),
        ("1.2.840.10008.1.2", Some(Codec::Native)),
        ("1.2.840.10008.1.2.1", Some(Codec::Native)),
        ("1.2.840.10008.1.2.2", Some(Codec::Native)),
        ("1.2.840.10008.1.2.1.99", Some(Codec::Native)),
        ("1.2.840.10008.1.2.4.80", Some(Codec::JpegLs)),
        ("1.2.840.10008.1.2.4.81", None),
        ("1.2.840.10008.1.2.5", Some(Codec::Rle)),
        ("1.2.840.10008.1.2.8.1", Some(Codec::DeflatedImageFrame)),
        ("9.9.9", None),
    ];

    for (uid, expected) in cases {
        assert_eq!(
            pixels::codec_for_syntax(uid),
            expected,
            "classification mismatch for transfer syntax {uid}"
        );
    }
}

#[tokio::test]
async fn unsupported_syntax_classes_return_422_from_both_frame_endpoints() {
    let dir = tempdir().expect("temporary fixture directory");
    let cases = [
        ("jpeg-extended-12bit", "1.2.840.10008.1.2.4.51"),
        ("jpeg-2000-lossy", "1.2.840.10008.1.2.4.91"),
        ("jpeg-ls-near-lossless", "1.2.840.10008.1.2.4.81"),
        ("unknown", "9.9.9"),
    ];

    for (label, transfer_syntax_uid) in cases {
        let path = dir.path().join(format!("{label}.dcm"));
        // Unknown syntaxes are rejected from registry metadata before the file
        // is opened, so use a writable known syntax for the inert container.
        let encoded_transfer_syntax_uid = if label == "unknown" {
            "1.2.840.10008.1.2.4.80"
        } else {
            transfer_syntax_uid
        };
        support::write_encapsulated_dicom(&path, encoded_transfer_syntax_uid, vec![vec![0]]);
        let entry = support::file_entry(path, transfer_syntax_uid, 1);
        let test_server = TestServer::new(server::router(support::app_state(vec![entry])));

        for endpoint in ["/api/file/0/frame/0", "/api/file/0/frame/0/raw"] {
            let response = test_server.get(endpoint).await;
            assert_eq!(
                response.status_code(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{label} should be rejected by {endpoint}"
            );
            assert_eq!(header(&response, "content-type"), "application/json");
            let body = response.json::<serde_json::Value>();
            assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|message| message.contains(transfer_syntax_uid)),
                "error response should identify transfer syntax {transfer_syntax_uid}: {body}"
            );
        }
    }
}
