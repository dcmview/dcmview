//! Malformed inputs are skipped during discovery or answered with a JSON error,
//! and neither stops the viewer from serving healthy files.

use super::support;
use axum_test::TestServer;
use dcmview::loader::{self, DiscoverOptions};
use dcmview::server;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use serde_json::Value;
use std::path::Path;
use tempfile::tempdir;

const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";

async fn discover(dir: &Path) -> dcmview::types::LoadReport {
    loader::discover(
        &[dir.to_path_buf()],
        DiscoverOptions {
            recursive: true,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discovery completes")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[tokio::test]
async fn malformed_pixel_payloads_fail_per_request_and_the_server_keeps_serving() {
    let dir = tempdir().expect("temp dir");
    support::write_uncompressed_u16_dicom(
        &dir.path().join("healthy.dcm"),
        EXPLICIT_LE,
        2,
        2,
        vec![1, 2, 3, 4],
        None,
        None,
    );
    // An RLE header must declare at most 15 segments (PS3.5 Annex G.4).
    let mut rle_header = vec![0_u8; 64];
    rle_header[0] = 99;
    support::write_encapsulated_dicom(
        &dir.path().join("bad-rle-header.dcm"),
        "1.2.840.10008.1.2.5",
        vec![rle_header],
    );
    let jpeg = support::grayscale_jpeg_fragment_16x16(7);
    support::write_encapsulated_dicom(
        &dir.path().join("truncated-jpeg.dcm"),
        "1.2.840.10008.1.2.4.50",
        vec![jpeg[..jpeg.len() / 3].to_vec()],
    );
    // Declares 4x4 16-bit samples but carries only four of them.
    support::write_uncompressed_u16_dicom(
        &dir.path().join("short-native.dcm"),
        EXPLICIT_LE,
        4,
        4,
        vec![1, 2, 3, 4],
        None,
        None,
    );

    let report = discover(dir.path()).await;
    assert_eq!(
        report.files.len(),
        4,
        "malformed pixels still parse as DICOM"
    );
    let healthy = report
        .files
        .iter()
        .find(|file| file_name(&file.path.to_string_lossy()) == "healthy.dcm")
        .map(|file| file.index)
        .expect("healthy file discovered");
    let malformed = report
        .files
        .iter()
        .filter(|file| file.index != healthy)
        .map(|file| (file.index, file.path.display().to_string()))
        .collect::<Vec<_>>();
    let server = TestServer::new(server::router(support::app_state(report.files)));

    for (index, path) in malformed {
        for endpoint in [
            format!("/api/file/{index}/frame/0"),
            format!("/api/file/{index}/frame/0/raw"),
        ] {
            let response = server.get(&endpoint).await;
            let status = response.status_code().as_u16();
            assert!(
                (400..600).contains(&status),
                "{path} {endpoint} returned {status}"
            );
            let body: Value = response.json();
            assert!(
                body["code"].is_string() && body["error"].is_string(),
                "{path} {endpoint} must use the JSON error envelope: {body}"
            );
        }
        server
            .get(&format!("/api/file/{healthy}/frame/0/raw"))
            .await
            .assert_status_ok();
    }
}

#[tokio::test]
async fn unreadable_files_are_skipped_without_stopping_discovery() {
    let dir = tempdir().expect("temp dir");
    let healthy = dir.path().join("healthy.dcm");
    support::write_uncompressed_u16_dicom(
        &healthy,
        EXPLICIT_LE,
        2,
        2,
        vec![1, 2, 3, 4],
        None,
        None,
    );

    let bytes = std::fs::read(&healthy).expect("read healthy fixture");
    std::fs::write(dir.path().join("truncated.dcm"), &bytes[..bytes.len() / 2])
        .expect("write truncated file");
    std::fs::write(dir.path().join("not-dicom.dcm"), b"plain text, not DICOM")
        .expect("write non-DICOM file");

    let charset = dir.path().join("unknown-charset.dcm");
    support::write_uncompressed_u16_dicom(
        &charset,
        EXPLICIT_LE,
        2,
        2,
        vec![1, 2, 3, 4],
        None,
        None,
    );
    let mut object = dicom_object::open_file(&charset).expect("reopen charset fixture");
    object.put(DataElement::new(
        tags::SPECIFIC_CHARACTER_SET,
        VR::CS,
        PrimitiveValue::from("ISO_IR 999"),
    ));
    object
        .write_to_file(&charset)
        .expect("write charset fixture");

    let report = discover(dir.path()).await;

    let loaded = report
        .files
        .iter()
        .map(|file| file_name(&file.path.to_string_lossy()).to_string())
        .collect::<Vec<_>>();
    assert_eq!(loaded, ["healthy.dcm"]);
    assert_eq!(report.skipped, 3);
}
