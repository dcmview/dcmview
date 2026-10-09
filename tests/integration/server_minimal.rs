use super::support;
use axum::http::{header, HeaderValue};
use axum_test::TestServer;
use dcmview::loader::{DiscoveryDisposition, DiscoveryReason, DiscoveryRecord};
use dcmview::server;
use dcmview::server::FileRegistry;
use serde_json::Value;
use tempfile::tempdir;

#[tokio::test]
async fn exposes_files_info_and_frame_endpoints_with_cache_headers() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("server-jpeg.dcm");
    let frame = support::grayscale_jpeg_fragment_16x16(42);
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.50", vec![frame.clone()]);

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.4.50", 1);
    entry.bits_allocated = 8;
    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let files_response = test_server.get("/api/files").await;
    files_response.assert_status_ok();
    let files_json: Value = files_response.json();
    assert_eq!(
        files_json["files"].as_array().expect("files array").len(),
        1
    );

    let info_response = test_server.get("/api/file/0/info").await;
    info_response.assert_status_ok();
    let info_json: Value = info_response.json();
    assert_eq!(info_json["frame_count"], 1);
    assert_eq!(info_json["transfer_syntax_uid"], "1.2.840.10008.1.2.4.50");

    let first_frame = test_server
        .get("/api/file/0/frame/0")
        .add_header(header::ACCEPT, HeaderValue::from_static("image/jpeg"))
        .await;
    first_frame.assert_status_ok();
    assert_eq!(
        first_frame
            .header("X-Cache")
            .to_str()
            .expect("cache header"),
        "MISS"
    );
    assert_eq!(
        first_frame
            .header(header::CONTENT_TYPE)
            .to_str()
            .expect("content-type"),
        "image/png"
    );
    assert_ne!(first_frame.as_bytes().as_ref(), frame.as_slice());

    let second_frame = test_server
        .get("/api/file/0/frame/0")
        .add_header(header::ACCEPT, HeaderValue::from_static("image/jpeg"))
        .await;
    second_frame.assert_status_ok();
    assert_eq!(
        second_frame
            .header("X-Cache")
            .to_str()
            .expect("cache header"),
        "HIT"
    );
}

#[tokio::test]
async fn health_endpoint_reports_ready_state() {
    let app = server::router(support::app_state(vec![support::file_entry(
        "fixture.dcm".into(),
        "1.2.840.10008.1.2.1",
        1,
    )]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/health").await;
    response.assert_status_ok();
    let health: Value = response.json();

    assert_eq!(health["status"], "ok");
    assert_eq!(health["viewer"]["name"], "dcmview");
    assert_eq!(health["viewer"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(health["viewer"]["build_target"].as_str().is_some());
    assert!(health["viewer"]["build_profile"].as_str().is_some());
    assert_eq!(health["file_count"], 1);
    assert_eq!(health["masked"], false);
    assert!(
        health["server_start_ms"]
            .as_u64()
            .expect("server_start_ms should be a number")
            > 0
    );
}

#[tokio::test]
async fn file_registry_serves_snapshots_while_scan_is_incomplete() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("mid-scan.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        Some("1500"),
        Some("3000"),
    );

    let registry = FileRegistry::new();
    let app = server::router(support::app_state_with_registry(registry.clone()));
    let test_server = TestServer::new(app);

    let initial_health: Value = test_server.get("/api/health").await.json();
    assert_eq!(initial_health["file_count"], 0);

    let initial_files: Value = test_server.get("/api/files").await.json();
    assert_eq!(initial_files["scan_complete"], false);
    assert_eq!(
        initial_files["files"]
            .as_array()
            .expect("files array")
            .len(),
        0
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;
    entry.default_window = Some(dcmview::types::WindowPreset {
        center: 1500.0,
        width: 3000.0,
    });
    let discovered_path = entry.path.clone();
    registry.insert(entry);
    registry.record_discovery(DiscoveryRecord {
        path: discovered_path,
        disposition: DiscoveryDisposition::Selected,
        reason: DiscoveryReason::ValidDicom,
    });

    let mid_files: Value = test_server.get("/api/files").await.json();
    assert_eq!(mid_files["scan_complete"], false);
    assert_eq!(mid_files["scanned"], 1);
    assert_eq!(
        mid_files["discovery"],
        serde_json::json!([]),
        "accepted files are listed as files, not as discovery records"
    );
    let mid_file = &mid_files["files"].as_array().expect("files array")[0];
    assert_eq!(mid_file["index"], 0);

    let info = test_server.get("/api/file/0/info").await;
    info.assert_status_ok();
    let tags = test_server.get("/api/file/0/tags").await;
    tags.assert_status_ok();
    let frame = test_server.get("/api/file/0/frame/0").await;
    frame.assert_status_ok();

    registry.mark_scan_complete();
    let final_files: Value = test_server.get("/api/files").await.json();
    assert_eq!(final_files["scan_complete"], true);
    assert_eq!(final_files["files"][0]["index"], mid_file["index"]);
}

#[tokio::test]
async fn returns_not_found_for_out_of_range_frame() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("server-jpeg.dcm");
    support::write_encapsulated_dicom(&path, "1.2.840.10008.1.2.4.50", vec![vec![1, 2, 3, 4]]);

    let app = server::router(support::app_state(vec![support::file_entry(
        path,
        "1.2.840.10008.1.2.4.50",
        1,
    )]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/frame/3").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn serves_embedded_frontend_shell_at_root() {
    let app = server::router(support::app_state(Vec::new()));
    let test_server = TestServer::new(app);

    let response = test_server.get("/").await;
    response.assert_status_ok();
    assert!(
        response
            .header(header::CONTENT_TYPE)
            .to_str()
            .expect("content-type header")
            .starts_with("text/html"),
        "root endpoint should return embedded index.html"
    );
}

#[tokio::test]
async fn serves_js_and_css_assets_with_correct_mime_types() {
    let app = server::router(support::app_state(Vec::new()));
    let test_server = TestServer::new(app);

    // Discover actual asset filenames from the index.html body
    let index_body = test_server.get("/").await.text();

    // Page-relative references let a reverse proxy serve the viewer under a
    // path prefix; a root-absolute one would escape it.
    for absolute in ["src=\"/", "href=\"/"] {
        assert!(
            !index_body.contains(absolute),
            "index.html should reference assets relative to the page, found {absolute}"
        );
    }

    let js_path = index_body
        .split("src=\"./assets/")
        .filter_map(|s| s.split('"').next())
        .find(|path| path.ends_with(".js"))
        .expect("JS asset referenced in index.html");

    let css_path = index_body
        .split("href=\"./assets/")
        .filter_map(|s| s.split('"').next())
        .find(|path| path.ends_with(".css"))
        .expect("CSS asset referenced in index.html");

    let js_response = test_server.get(&format!("/assets/{js_path}")).await;
    js_response.assert_status_ok();
    let js_ct_header = js_response.header(header::CONTENT_TYPE);
    let js_ct = js_ct_header.to_str().expect("js content-type");
    assert!(
        js_ct.starts_with("text/javascript"),
        "JS asset should be text/javascript, got: {js_ct}"
    );
    assert!(
        !js_response.as_bytes().is_empty(),
        "JS body should be non-empty"
    );

    let css_response = test_server.get(&format!("/assets/{css_path}")).await;
    css_response.assert_status_ok();
    let css_ct_header = css_response.header(header::CONTENT_TYPE);
    let css_ct = css_ct_header.to_str().expect("css content-type");
    assert!(
        css_ct.starts_with("text/css"),
        "CSS asset should be text/css, got: {css_ct}"
    );
    let css_body = css_response.text();
    assert!(!css_body.is_empty(), "CSS body should be non-empty");
    assert!(
        !css_body.contains("url(/"),
        "CSS should reference fonts relative to the stylesheet"
    );
}

#[tokio::test]
async fn default_and_full_dynamic_modes_occupy_independent_cache_slots() {
    // Verifies that ?mode=full_dynamic and no-mode (default) produce separate cache entries,
    // so switching mode always yields a MISS before the first HIT for that mode.
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("server-mode-cache.dcm");
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

    // Warm the cache for default mode.
    let default_first = test_server.get("/api/file/0/frame/0").await;
    default_first.assert_status_ok();
    assert_eq!(
        default_first.header("X-Cache").to_str().expect("cache"),
        "MISS"
    );

    // Default mode is now cached — full_dynamic must still be a MISS.
    let dynamic_first = test_server
        .get("/api/file/0/frame/0?mode=full_dynamic&wc=10&ww=20")
        .await;
    dynamic_first.assert_status_ok();
    assert_eq!(
        dynamic_first.header("X-Cache").to_str().expect("cache"),
        "MISS",
        "full_dynamic must be a MISS even after default mode was cached"
    );

    // Subsequent full_dynamic request: HIT.
    let dynamic_second = test_server
        .get("/api/file/0/frame/0?mode=full_dynamic&wc=30&ww=40")
        .await;
    dynamic_second.assert_status_ok();
    assert_eq!(
        dynamic_second.header("X-Cache").to_str().expect("cache"),
        "HIT"
    );
}

#[tokio::test]
async fn drag_previews_read_but_never_fill_the_display_cache() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("server-preview-cache.dcm");
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
    let test_server = TestServer::new(server::router(support::app_state(vec![entry])));
    let cache_state = |response: &axum_test::TestResponse| {
        response
            .header("X-Cache")
            .to_str()
            .expect("cache")
            .to_string()
    };

    let preview = "/api/file/0/frame/0?wc=1500&ww=3000&preview=true";
    let settled = "/api/file/0/frame/0?wc=1500&ww=3000";
    let first_preview = test_server.get(preview).await;
    first_preview.assert_status_ok();
    assert_eq!(cache_state(&first_preview), "MISS");
    assert_eq!(cache_state(&test_server.get(preview).await), "MISS");

    let first_settled = test_server.get(settled).await;
    assert_eq!(
        cache_state(&first_settled),
        "MISS",
        "a preview is not cached"
    );
    assert_eq!(first_settled.as_bytes(), first_preview.as_bytes());
    assert_eq!(cache_state(&test_server.get(preview).await), "HIT");
    // A preview reports the window it was rendered with, like any frame.
    assert_eq!(
        first_preview
            .header(dcmview::api::contracts::DISPLAY_FRAME_HEADER_WINDOW_CENTER)
            .to_str()
            .expect("window header"),
        "1500"
    );
}

#[tokio::test]
async fn display_frame_reports_the_window_it_applied() {
    use dcmview::api::contracts::{
        WindowMode, DISPLAY_FRAME_HEADER_WINDOW_CENTER, DISPLAY_FRAME_HEADER_WINDOW_WIDTH,
    };
    use dcmview::pixels::resolve_window_with_mode;

    // No Window Center/Width, so the default request is windowed from the
    // frame's own samples.
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("server-window-headers.dcm");
    let samples = [0_u16, 1000, 2000, 3000];
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        samples.to_vec(),
        None,
        None,
    );
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;
    let test_server = TestServer::new(server::router(support::app_state(vec![entry])));
    let values = samples.map(f64::from);

    let automatic = resolve_window_with_mode(WindowMode::Default, None, None, None, &values)
        .expect("percentile window");
    let full_dynamic = resolve_window_with_mode(WindowMode::FullDynamic, None, None, None, &values)
        .expect("min/max window");
    for (query, center, width) in [
        ("", automatic.center, automatic.width),
        (
            "?mode=full_dynamic",
            full_dynamic.center,
            full_dynamic.width,
        ),
        ("?wc=1200&ww=400", 1200.0, 400.0),
    ] {
        // A cached frame reports the same window as its first render.
        for cache in ["MISS", "HIT"] {
            let response = test_server
                .get(&format!("/api/file/0/frame/0{query}"))
                .await;
            response.assert_status_ok();
            assert_eq!(response.header("X-Cache").to_str().expect("cache"), cache);
            let header = |name: &str| -> f64 {
                response
                    .header(name)
                    .to_str()
                    .expect("window header")
                    .parse()
                    .expect("numeric window header")
            };
            assert_eq!(
                (
                    header(DISPLAY_FRAME_HEADER_WINDOW_CENTER),
                    header(DISPLAY_FRAME_HEADER_WINDOW_WIDTH)
                ),
                (center, width),
                "window reported for {query:?} on a {cache}"
            );
        }
    }
}
