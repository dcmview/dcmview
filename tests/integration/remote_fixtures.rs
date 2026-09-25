#[cfg(feature = "remote-fixtures")]
use axum::http::{header, HeaderValue};
#[cfg(feature = "remote-fixtures")]
use axum_test::TestServer;
#[cfg(feature = "remote-fixtures")]
use dcmview::loader::{self, DiscoverOptions};
#[cfg(feature = "remote-fixtures")]
use dcmview::server;
#[cfg(feature = "remote-fixtures")]
use serde_json::Value;

#[cfg(feature = "remote-fixtures")]
use super::support;

#[cfg(feature = "remote-fixtures")]
#[tokio::test]
#[ignore = "downloads and caches data with dicom-test-files; run explicitly when network or cache is available"]
async fn loads_remote_dicom_test_file_through_loader_and_http_contracts() {
    let path = dicom_test_files::path("pydicom/liver.dcm").expect("fetch dicom-test-files fixture");
    let report = loader::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover remote fixture");

    assert_eq!(report.files.len(), 1);
    assert_eq!(report.skipped, 0);
    assert!(
        report.files[0].has_pixels,
        "remote fixture should include pixel data"
    );
    assert!(
        report.files[0].rows > 0,
        "remote fixture should expose Rows"
    );
    assert!(
        report.files[0].columns > 0,
        "remote fixture should expose Columns"
    );

    let app = server::router(support::app_state(report.files));
    let test_server = TestServer::new(app);

    let files: Value = test_server.get("/api/files").await.json();
    assert_eq!(files["files"].as_array().expect("files array").len(), 1);
    assert!(files["server_start_ms"].as_u64().is_some());

    let info: Value = test_server.get("/api/file/0/info").await.json();
    assert_eq!(info["has_pixels"], true);
    assert!(info["rows"].as_u64().unwrap_or_default() > 0);
    assert!(info["columns"].as_u64().unwrap_or_default() > 0);
}

#[cfg(feature = "remote-fixtures")]
#[tokio::test]
#[ignore = "downloads and caches data with dicom-test-files; run explicitly when network or cache is available"]
async fn decodes_remote_jpeg2000_fixture_through_display_endpoint() {
    let path = dicom_test_files::path("pydicom/MR_small_jp2klossless.dcm")
        .expect("fetch dicom-test-files jpeg2000 fixture");
    let report = loader::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover remote jpeg2000 fixture");

    assert_eq!(report.files.len(), 1);
    assert!(report.files[0].has_pixels);

    let app = server::router(support::app_state(report.files));
    let test_server = TestServer::new(app);

    let first = test_server
        .get("/api/file/0/frame/0")
        .add_header(header::ACCEPT, HeaderValue::from_static("image/png"))
        .await;
    first.assert_status_ok();
    assert_eq!(
        first
            .header(header::CONTENT_TYPE)
            .to_str()
            .expect("content-type"),
        "image/png"
    );
    assert_eq!(first.header("x-cache").to_str().expect("x-cache"), "MISS");

    let second = test_server
        .get("/api/file/0/frame/0")
        .add_header(header::ACCEPT, HeaderValue::from_static("image/png"))
        .await;
    second.assert_status_ok();
    assert_eq!(second.header("x-cache").to_str().expect("x-cache"), "HIT");
}

#[cfg(not(feature = "remote-fixtures"))]
#[test]
#[ignore = "enable the remote-fixtures feature to compile dicom-test-files coverage"]
fn loads_remote_dicom_test_file_through_loader_and_http_contracts() {}

#[cfg(not(feature = "remote-fixtures"))]
#[test]
#[ignore = "enable the remote-fixtures feature to compile dicom-test-files coverage"]
fn decodes_remote_jpeg2000_fixture_through_display_endpoint() {}
