//! Redaction boxes over the committed masking fixtures, whose 320x240 images
//! hold bright block text in a 40-row banner above a textured wedge.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server::{self, FileRegistry};
use serde_json::{json, Value};
use std::path::PathBuf;

const COLUMNS: usize = 320;
const BANNER_ROWS: usize = 40;

/// A server over patient A's two files (one series) and patient B's file.
async fn serve() -> TestServer {
    let paths = [
        "golden-masking-patient-a-us-1.dcm",
        "golden-masking-patient-a-us-2.dcm",
        "golden-masking-patient-b-us.dcm",
    ]
    .map(|name| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    });
    let report = support::discover(
        &paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover masking fixtures");
    let registry = FileRegistry::new();
    for path in &paths {
        let file = report
            .files
            .iter()
            .find(|file| &file.path == path)
            .expect("fixture discovered")
            .clone();
        registry.insert(file);
    }
    registry.mark_scan_complete();
    TestServer::new(server::router(support::app_state_with_registry(registry)))
}

fn banner_box() -> Value {
    json!({ "num_roi": 1, "roi_coords": [[0, 0, BANNER_ROWS, COLUMNS]], "roi_frames": [] })
}

async fn display_pixels(server: &TestServer, file: usize) -> (Vec<u8>, String) {
    let response = server.get(&format!("/api/file/{file}/frame/0")).await;
    response.assert_status_ok();
    let cache = response
        .header("x-cache")
        .to_str()
        .expect("cache header")
        .to_string();
    let image = image::load_from_memory(response.as_bytes()).expect("decode display PNG");
    (image.into_luma8().into_raw(), cache)
}

async fn raw_pixels(server: &TestServer, file: usize) -> Vec<u8> {
    let response = server.get(&format!("/api/file/{file}/frame/0/raw")).await;
    response.assert_status_ok();
    response.as_bytes().to_vec()
}

fn banner(pixels: &[u8]) -> &[u8] {
    &pixels[..BANNER_ROWS * COLUMNS]
}

fn below_banner(pixels: &[u8]) -> &[u8] {
    &pixels[BANNER_ROWS * COLUMNS..]
}

#[tokio::test]
async fn a_box_blanks_both_frame_endpoints_and_is_never_served_from_an_older_cache() {
    let server = serve().await;
    let (before, _) = display_pixels(&server, 0).await;
    let raw_before = raw_pixels(&server, 0).await;
    assert!(banner(&before).contains(&255), "the banner holds text");
    assert!(banner(&raw_before).contains(&255));

    let stored = server
        .put("/api/file/0/redactions")
        .json(&banner_box())
        .await;
    stored.assert_status_ok();
    assert_eq!(stored.json::<Value>(), banner_box());
    assert_eq!(
        server.get("/api/file/0/redactions").await.json::<Value>(),
        banner_box()
    );

    let (redacted, cache) = display_pixels(&server, 0).await;
    assert_eq!(
        cache, "MISS",
        "the frame cached before the box is not served"
    );
    assert!(banner(&redacted).iter().all(|&pixel| pixel == 0));
    assert_eq!(below_banner(&redacted), below_banner(&before));
    let (_, cache) = display_pixels(&server, 0).await;
    assert_eq!(cache, "HIT");

    let raw = raw_pixels(&server, 0).await;
    assert!(banner(&raw).iter().all(|&sample| sample == 0));
    assert_eq!(below_banner(&raw), below_banner(&raw_before));
    let pixel = server
        .get("/api/file/0/frame/0/raw/pixel?row=10&column=8")
        .await;
    pixel.assert_status_ok();
    assert_eq!(pixel.as_bytes().as_ref(), [0]);

    // Other windows of the frame are redacted too.
    let windowed = server.get("/api/file/0/frame/0?wc=40&ww=80").await;
    windowed.assert_status_ok();
    let windowed = image::load_from_memory(windowed.as_bytes())
        .expect("decode windowed PNG")
        .into_luma8()
        .into_raw();
    assert!(banner(&windowed).iter().all(|&pixel| pixel == 0));

    // Files without boxes are untouched.
    let (other, _) = display_pixels(&server, 1).await;
    assert!(banner(&other).contains(&255));
}

#[tokio::test]
async fn removing_the_boxes_shows_the_frame_again() {
    let server = serve().await;
    server
        .put("/api/file/0/redactions")
        .json(&banner_box())
        .await
        .assert_status_ok();
    let (redacted, _) = display_pixels(&server, 0).await;
    assert!(banner(&redacted).iter().all(|&pixel| pixel == 0));

    server
        .put("/api/file/0/redactions")
        .json(&json!({ "num_roi": 0, "roi_coords": [], "roi_frames": [] }))
        .await
        .assert_status_ok();
    let (shown, _) = display_pixels(&server, 0).await;
    assert!(banner(&shown).contains(&255));
    assert!(banner(&raw_pixels(&server, 0).await).contains(&255));
}

#[tokio::test]
async fn boxes_copy_to_the_files_of_the_series_with_the_same_size() {
    let server = serve().await;
    server
        .put("/api/file/0/redactions")
        .json(&banner_box())
        .await
        .assert_status_ok();

    let copied = server.put("/api/file/0/redactions/series").await;
    copied.assert_status_ok();
    assert_eq!(copied.json::<Value>(), json!({ "file_indices": [1] }));

    assert_eq!(
        server.get("/api/file/1/redactions").await.json::<Value>(),
        banner_box()
    );
    let (same_series, _) = display_pixels(&server, 1).await;
    assert!(banner(&same_series).iter().all(|&pixel| pixel == 0));
    // Patient B's file has the same size but is another series.
    let (other_series, _) = display_pixels(&server, 2).await;
    assert!(banner(&other_series).contains(&255));
}

#[tokio::test]
async fn boxes_are_validated_and_stay_out_of_the_roi_export() {
    let server = serve().await;

    let outside = server
        .put("/api/file/0/redactions")
        .json(&json!({ "num_roi": 1, "roi_coords": [[0, 0, 241, 10]], "roi_frames": [] }))
        .await;
    outside.assert_status(StatusCode::BAD_REQUEST);
    server
        .get("/api/file/99/redactions")
        .await
        .assert_status(StatusCode::NOT_FOUND);

    server
        .put("/api/file/0/redactions")
        .json(&banner_box())
        .await
        .assert_status_ok();
    let roi: Value = server.get("/api/file/0/annotations").await.json();
    assert_eq!(roi["num_roi"], 0);
    let export = server.get("/api/annotations/export.csv").await.text();
    assert_eq!(export.lines().count(), 1, "only the header: {export}");
}
