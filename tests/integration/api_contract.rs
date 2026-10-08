use super::support;
use axum::http::{header, HeaderValue, StatusCode};
use axum_test::{TestResponse, TestServer};
use bytes::Bytes;
use dcmview::annotations::{AnnotationStore, EmbedRoiAnnotations};
use dcmview::api::contracts::{
    endpoints, Endpoint, ResponseHeaders, CACHE_HEADER, CACHE_HIT, CACHE_MISS,
    DISPLAY_FRAME_HEADERS, EXPORT_CONTENT_DISPOSITION_HEADER, EXPORT_CONTENT_DISPOSITION_VALUE,
    RAW_FRAME_HEADERS, SERVER_INSTANCE_HEADER, THUMBNAIL_HEADERS,
};
use dcmview::server;
use dcmview::types::WindowPreset;
use serde_json::Value;
use std::collections::HashMap;
use tempfile::tempdir;

#[tokio::test]
async fn json_endpoints_match_frontend_contract_shapes() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("contract.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        Some("1500"),
        Some("3000"),
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;
    entry.default_window = Some(WindowPreset {
        center: 1500.0,
        width: 3000.0,
    });

    let state = support::app_state_with_annotations(
        vec![entry],
        AnnotationStore::new(HashMap::from([(
            0,
            EmbedRoiAnnotations {
                num_roi: 1,
                roi_coords: vec![[1, 2, 3, 4]],
                roi_frames: vec![vec![0]],
            },
        )])),
    );

    let test_server = TestServer::new(server::router(state));

    let files: Value = test_server.get("/api/files").await.json();
    assert_object_keys(
        &files,
        &[
            "discovery",
            "files",
            "filtered",
            "masked",
            "scan_complete",
            "scanned",
            "server_start_ms",
            "skipped",
        ],
    );
    let file = &files["files"].as_array().expect("files array")[0];
    assert_object_keys(
        file,
        &[
            "burned_in_annotation",
            "columns",
            "default_window",
            "display_name",
            "file_format",
            "frame_count",
            "has_pixels",
            "index",
            "instance_number",
            "label",
            "modality",
            "object_kind",
            "path",
            "patient_id",
            "patient_name",
            "pixel_aspect_ratio",
            "presentation_layer",
            "raster",
            "raw_windowing_compatible",
            "raw_windowing_reason",
            "rows",
            "series_description",
            "series_instance_uid",
            "series_number",
            "sop_instance_uid",
            "sop_class_uid",
            "study_date",
            "study_description",
            "study_instance_uid",
            "support_reason",
            "support_state",
            "transfer_syntax_uid",
        ],
    );
    assert_object_keys(&file["default_window"], &["center", "width"]);

    let info: Value = test_server.get("/api/file/0/info").await.json();
    assert_object_keys(
        &info,
        &[
            "columns",
            "default_window",
            "frame_count",
            "has_pixels",
            "object_kind",
            "rows",
            "sop_class_uid",
            "support_reason",
            "support_state",
            "transfer_syntax_uid",
        ],
    );
    assert_eq!(file["object_kind"], "classic_image");
    assert_eq!(file["file_format"], "dicom");
    assert!(file["raster"].is_null());
    assert_eq!(file["support_state"], "renderable");
    assert!(file["support_reason"].is_null());
    assert_eq!(info["object_kind"], "classic_image");
    assert_eq!(info["support_state"], "renderable");
    assert_object_keys(&info["default_window"], &["center", "width"]);

    let tags: Value = test_server.get("/api/file/0/tags").await.json();
    let tag_rows = tags.as_array().expect("tag response array");
    let rows_tag = tag_rows
        .iter()
        .find(|row| row["keyword"] == "Rows")
        .expect("Rows tag");
    assert_tag_node_shape(rows_tag);
    assert_object_keys(&rows_tag["value"], &["type", "value"]);
    assert_eq!(rows_tag["value"]["type"], "number");

    let pixel_data_tag = tag_rows
        .iter()
        .find(|row| row["tag"] == "(7FE0,0010)")
        .expect("PixelData tag");
    assert_tag_node_shape(pixel_data_tag);
    assert_object_keys(&pixel_data_tag["value"], &["length", "type"]);
    assert_eq!(pixel_data_tag["value"]["type"], "binary");

    let annotations: Value = test_server.get("/api/file/0/annotations").await.json();
    assert_object_keys(&annotations, &["num_roi", "roi_coords", "roi_frames"]);
    assert_eq!(
        annotations["roi_coords"][0],
        serde_json::json!([1, 2, 3, 4])
    );
    assert_eq!(annotations["roi_frames"][0], serde_json::json!([0]));

    let missing = test_server.get("/api/file/99/info").await;
    missing.assert_status_not_found();
    let error: Value = missing.json();
    assert_object_keys(&error, &["code", "error"]);
    assert_eq!(error["code"], "not_found");
}

#[tokio::test]
async fn every_declared_endpoint_matches_its_runtime_contract() {
    let dir = tempdir().expect("temp dir");
    let test_server = TestServer::new(server::router(support::every_endpoint_state(dir.path())));
    let request =
        |endpoint: &Endpoint, index: &str| support::endpoint_request(&test_server, endpoint, index);

    for endpoint in endpoints::ALL {
        if endpoint.path.contains("{index}") {
            let missing = request(endpoint, "99").await;
            assert_json_error(endpoint.id, &missing, StatusCode::NOT_FOUND);
        }
        // Covered by their own fixture tests; see `NEEDS_LINKED_SOURCE`.
        if support::NEEDS_LINKED_SOURCE.contains(endpoint) {
            continue;
        }
        let response = request(endpoint, "0").await;

        assert_eq!(
            response.status_code().as_u16(),
            endpoint.success_status,
            "{} status contract",
            endpoint.id
        );
        assert_eq!(
            response
                .header(header::CONTENT_TYPE)
                .to_str()
                .expect("content type"),
            endpoint.response_media_type,
            "{} media-type contract",
            endpoint.id
        );
        assert_declared_response_headers(endpoint, &response);
    }
}

#[tokio::test]
async fn reference_endpoint_preserves_identity_and_resolves_navigable_frames() {
    let dir = tempdir().expect("temp dir");
    let source_path = dir.path().join("reference-source.dcm");
    let source_uid = "1.2.826.0.1.3680043.10.900.1";
    let target_uid = "1.2.826.0.1.3680043.10.900.2";
    let target_class = "1.2.840.10008.5.1.4.1.1.2";
    support::write_reference_dicom(
        &source_path,
        source_uid,
        target_class,
        target_uid,
        &[1, 4, 5],
    );

    let mut source = support::file_entry(source_path, "1.2.840.10008.1.2.1", 1);
    source.sop_instance_uid = source_uid.to_string();
    source.sop_class_uid = "1.2.840.10008.5.1.4.1.1.30".to_string();
    let mut target = support::file_entry(dir.path().join("target.dcm"), "1.2.840.10008.1.2.1", 4);
    target.sop_instance_uid = target_uid.to_string();
    target.sop_class_uid = target_class.to_string();

    let test_server = TestServer::new(server::router(support::app_state(vec![source, target])));
    let response = test_server.get("/api/file/0/references").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_object_keys(
        &body,
        &["references", "source_file_index", "source_sop_instance_uid"],
    );
    assert_eq!(body["source_file_index"], 0);
    assert_eq!(body["source_sop_instance_uid"], source_uid);
    let edge = &body["references"][0];
    assert_object_keys(edge, &["matches", "relationship", "target"]);
    assert_eq!(edge["relationship"], "source_image");
    assert_object_keys(
        &edge["target"],
        &[
            "frame_numbers",
            "segment_numbers",
            "series_instance_uid",
            "sop_class_uid",
            "sop_instance_uid",
        ],
    );
    assert_eq!(
        edge["target"]["frame_numbers"],
        serde_json::json!([1, 4, 5])
    );
    let resolved = &edge["matches"][0];
    assert_object_keys(
        resolved,
        &["file_index", "frame_indices", "path", "sop_instance_uid"],
    );
    assert_eq!(resolved["file_index"], 1);
    assert_eq!(resolved["frame_indices"], serde_json::json!([0, 3]));
}

#[tokio::test]
async fn raw_frame_endpoint_exposes_frontend_metadata_header_contract() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("raw-contract.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        Some("1500"),
        Some("3000"),
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;
    entry.default_window = Some(WindowPreset {
        center: 1500.0,
        width: 3000.0,
    });
    let has_default_window = entry.default_window.is_some();

    let test_server = TestServer::new(server::router(support::app_state(vec![entry])));
    let response = test_server.get("/api/file/0/frame/0/raw").await;
    response.assert_status_ok();

    assert_eq!(
        response
            .header(header::CONTENT_TYPE)
            .to_str()
            .expect("content-type"),
        "application/octet-stream"
    );
    assert!(response.maybe_header(CACHE_HEADER).is_some());
    for &(field, name) in RAW_FRAME_HEADERS {
        let present = response.maybe_header(name).is_some();
        if matches!(field, "defaultWc" | "defaultWw") {
            assert_eq!(
                present, has_default_window,
                "optional raw header {name} presence"
            );
        } else if matches!(field, "paddingLow" | "paddingHigh") {
            assert!(!present, "{name} without Pixel Padding");
        } else {
            assert!(present, "raw response missing {name}");
        }
    }
}

#[tokio::test]
async fn api_boundary_rejections_use_the_json_error_envelope() {
    let test_server = TestServer::new(server::router(support::app_state(Vec::new())));

    let cases = vec![
        (
            "malformed path",
            test_server.get("/api/file/not-a-number/info").await,
            StatusCode::BAD_REQUEST,
        ),
        (
            "malformed query",
            test_server.get("/api/file/0/frame/0?wc=not-a-number").await,
            StatusCode::BAD_REQUEST,
        ),
        (
            "malformed JSON",
            test_server
                .put("/api/file/0/annotations")
                .bytes(Bytes::from_static(b"{"))
                .add_header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )
                .await,
            StatusCode::BAD_REQUEST,
        ),
        (
            "invalid JSON shape",
            test_server
                .put("/api/file/0/annotations")
                .json(&serde_json::json!({}))
                .await,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "missing JSON content type",
            test_server
                .put("/api/file/0/annotations")
                .bytes(Bytes::from_static(b"{}"))
                .await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "unknown API route",
            test_server.get("/api/not-a-route").await,
            StatusCode::NOT_FOUND,
        ),
        (
            "wrong API method",
            test_server.post("/api/files").await,
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ];

    for (name, response, expected_status) in cases {
        assert_json_error(name, &response, expected_status);
    }
}

#[tokio::test]
async fn display_frame_rejects_invalid_window_queries_as_json() {
    let entry = support::file_entry("window-validation.dcm".into(), "1.2.840.10008.1.2.1", 1);
    let test_server = TestServer::new(server::router(support::app_state(vec![entry])));

    for query in [
        "wc=10",
        "ww=20",
        "wc=NaN&ww=20",
        "wc=10&ww=NaN",
        "wc=10&ww=0",
        "wc=10&ww=-1",
    ] {
        let response = test_server
            .get(&format!("/api/file/0/frame/0?{query}"))
            .await;
        assert_json_error(query, &response, StatusCode::BAD_REQUEST);
    }
}

fn assert_json_error(name: &str, response: &TestResponse, expected_status: StatusCode) {
    assert_eq!(response.status_code(), expected_status, "{name}");
    assert!(
        response
            .header(header::CONTENT_TYPE)
            .to_str()
            .expect("content type")
            .starts_with("application/json"),
        "{name} must return JSON"
    );
    let payload: Value = response.json();
    assert_object_keys(&payload, &["code", "error"]);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "{name} must include a non-empty error message"
    );
}

fn assert_declared_response_headers(endpoint: &Endpoint, response: &TestResponse) {
    assert!(
        response
            .header(SERVER_INSTANCE_HEADER)
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    match endpoint.response_headers {
        ResponseHeaders::None => {
            assert_no_cache_header(endpoint, response);
            assert_no_raw_frame_headers(endpoint, response);
            assert_no_display_frame_headers(endpoint, response);
            assert_no_export_header(endpoint, response);
        }
        ResponseHeaders::Cache => {
            assert_cache_header(endpoint, response);
            assert_no_raw_frame_headers(endpoint, response);
            assert_no_display_frame_headers(endpoint, response);
            assert_no_export_header(endpoint, response);
        }
        ResponseHeaders::DisplayFrame => {
            assert_cache_header(endpoint, response);
            // The window pair is sent together, as numbers, or not at all.
            let window = DISPLAY_FRAME_HEADERS
                .iter()
                .filter(|(field, _)| *field != "windowApplied")
                .map(|(_, name)| {
                    response.maybe_header(*name).map(|value| {
                        value
                            .to_str()
                            .ok()
                            .and_then(|text| text.parse::<f64>().ok())
                            .filter(|number| number.is_finite())
                            .unwrap_or_else(|| {
                                panic!("{} returned non-numeric {name}", endpoint.id)
                            })
                    })
                })
                .collect::<Vec<_>>();
            assert!(
                window.iter().all(Option::is_some) || window.iter().all(Option::is_none),
                "{} returned half a display window",
                endpoint.id
            );
            let applied =
                response.maybe_header(dcmview::api::contracts::DISPLAY_FRAME_HEADER_WINDOW_APPLIED);
            match applied.as_ref().and_then(|value| value.to_str().ok()) {
                Some("linear") => assert!(window.iter().all(Option::is_some)),
                Some("real_world" | "voi_lut") | None => {
                    assert!(window.iter().all(Option::is_none))
                }
                Some(other) => panic!("invalid applied window: {other}"),
            }
            assert_no_raw_frame_headers(endpoint, response);
            assert_no_export_header(endpoint, response);
        }
        ResponseHeaders::RawFrame => {
            assert_cache_header(endpoint, response);
            // The padding pair is present only for files that declare Pixel Padding.
            for (_, name) in RAW_FRAME_HEADERS
                .iter()
                .filter(|(field, _)| !matches!(*field, "paddingLow" | "paddingHigh"))
            {
                assert!(
                    response.maybe_header(*name).is_some(),
                    "{} is missing raw-frame header {name}",
                    endpoint.id
                );
            }
            assert_no_display_frame_headers(endpoint, response);
            assert_no_export_header(endpoint, response);
        }
        ResponseHeaders::Thumbnail => {
            assert_cache_header(endpoint, response);
            for (_, name) in THUMBNAIL_HEADERS {
                assert!(
                    response.maybe_header(*name).is_some(),
                    "{} is missing thumbnail header {name}",
                    endpoint.id
                );
            }
            assert_no_raw_frame_headers(endpoint, response);
            assert_no_display_frame_headers(endpoint, response);
            assert_no_export_header(endpoint, response);
        }
        ResponseHeaders::Export => {
            assert_no_cache_header(endpoint, response);
            assert_no_raw_frame_headers(endpoint, response);
            assert_no_display_frame_headers(endpoint, response);
            assert_eq!(
                response
                    .header(EXPORT_CONTENT_DISPOSITION_HEADER)
                    .to_str()
                    .expect("content-disposition"),
                EXPORT_CONTENT_DISPOSITION_VALUE,
                "{} content-disposition contract",
                endpoint.id
            );
        }
    }
}

fn assert_cache_header(endpoint: &Endpoint, response: &TestResponse) {
    let header = response.header(CACHE_HEADER);
    let value = header.to_str().expect("cache header");
    assert!(
        matches!(value, CACHE_HIT | CACHE_MISS),
        "{} returned invalid {CACHE_HEADER} value {value:?}",
        endpoint.id
    );
}

fn assert_no_cache_header(endpoint: &Endpoint, response: &TestResponse) {
    assert!(
        response.maybe_header(CACHE_HEADER).is_none(),
        "{} unexpectedly returned {CACHE_HEADER}",
        endpoint.id
    );
}

fn assert_no_display_frame_headers(endpoint: &Endpoint, response: &TestResponse) {
    for (_, name) in DISPLAY_FRAME_HEADERS {
        assert!(
            response.maybe_header(*name).is_none(),
            "{} unexpectedly returned display-frame header {name}",
            endpoint.id
        );
    }
}

fn assert_no_raw_frame_headers(endpoint: &Endpoint, response: &TestResponse) {
    for (_, name) in RAW_FRAME_HEADERS {
        assert!(
            response.maybe_header(*name).is_none(),
            "{} unexpectedly returned raw-frame header {name}",
            endpoint.id
        );
    }
}

fn assert_no_export_header(endpoint: &Endpoint, response: &TestResponse) {
    assert!(
        response
            .maybe_header(EXPORT_CONTENT_DISPOSITION_HEADER)
            .is_none(),
        "{} unexpectedly returned {EXPORT_CONTENT_DISPOSITION_HEADER}",
        endpoint.id
    );
}

fn assert_tag_node_shape(value: &Value) {
    assert_object_keys(value, &["keyword", "tag", "value", "vr"]);
    assert!(value["tag"].as_str().expect("tag string").starts_with('('));
    assert!(value["vr"].is_string());
    assert!(value["keyword"].is_string());
    assert!(value["value"]["type"].is_string());
}

fn assert_object_keys(value: &Value, expected: &[&str]) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("expected object, got {value:?}"));
    let mut actual = object.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();

    let mut expected = expected.to_vec();
    expected.sort_unstable();

    assert_eq!(actual, expected);
}

#[tokio::test]
async fn color_display_omits_the_applied_window_header() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden-rle-ybr-full-422-u8-single-frame.dcm");
    let report = support::discover(
        &[path],
        dcmview::loader::DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discover color fixture");
    let test_server = TestServer::new(server::router(support::app_state(report.files)));
    for _ in 0..2 {
        let response = test_server.get("/api/file/0/frame/0").await;
        response.assert_status_ok();
        for (_, name) in DISPLAY_FRAME_HEADERS {
            assert!(
                response.maybe_header(*name).is_none(),
                "color frame sent {name}"
            );
        }
    }
}

#[tokio::test]
async fn every_response_forbids_mime_sniffing() {
    let app = TestServer::new(server::router(support::app_state(vec![])));
    let responses = [
        app.get("/").await,
        app.get("/assets/missing.js").await,
        app.get("/elsewhere").await,
        app.get("/api/files").await,
        app.get("/api/unknown").await,
        app.get("/api/annotations/export.csv").await,
        app.post("/api/files").await,
        app.put("/api/file/0/annotations").text("bad JSON").await,
    ];
    for response in responses {
        response.assert_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    }
}

#[tokio::test]
async fn server_identity_header_covers_success_errors_and_fallbacks() {
    let app = TestServer::new(server::router(support::app_state(vec![])));
    let files = app.get("/api/files").await;
    let identity = files.json::<Value>()["server_start_ms"]
        .as_u64()
        .unwrap()
        .to_string();
    files.assert_header(SERVER_INSTANCE_HEADER, identity.as_str());
    for path in [
        "/api",
        "/api/",
        "/api/unknown",
        "/api/file/not-an-index/tags",
        "/api/file/99/info",
        "/api/health",
    ] {
        app.get(path)
            .await
            .assert_header(SERVER_INSTANCE_HEADER, identity.as_str());
    }
    app.post("/api/files")
        .await
        .assert_header(SERVER_INSTANCE_HEADER, identity.as_str());
    app.put("/api/file/0/annotations")
        .text("bad JSON")
        .await
        .assert_header(SERVER_INSTANCE_HEADER, identity.as_str());
    assert!(app
        .get("/")
        .await
        .headers()
        .get(SERVER_INSTANCE_HEADER)
        .is_none());
}

#[tokio::test]
async fn segmentation_context_always_includes_a_string_array_of_warnings() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in ["binary", "fractional", "binary-valued-fractional"] {
        let report = support::discover(
            &[root.join(format!("golden-seg-{name}.dcm"))],
            dcmview::loader::DiscoverOptions {
                recursive: true,
                filters: vec![],
                formats: Default::default(),
            },
        )
        .await
        .unwrap();
        let server = TestServer::new(server::router(support::app_state(report.files)));
        let response = server.get("/api/file/0/semantic-context").await;
        response.assert_status_ok();
        response.assert_header(header::CONTENT_TYPE, "application/json");
        let value: Value = response.json();
        let context = &value["context"];
        assert_eq!(context["kind"], "segmentation");
        let warnings = context["warnings"]
            .as_array()
            .expect("required warnings array");
        assert!(warnings.iter().all(Value::is_string));
        assert_eq!(
            warnings.len(),
            usize::from(name == "binary-valued-fractional")
        );
        // The additive field leaves the declared attributes and existing shape intact.
        let keys: std::collections::BTreeSet<_> = context
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "kind",
                "warnings",
                "segmentation_type",
                "segmentation_fractional_type",
                "maximum_fractional_value",
                "segments",
                "frame_mappings",
                "references",
                "overlay"
            ]
            .into_iter()
            .collect()
        );
    }
}
