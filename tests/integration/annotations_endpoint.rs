use super::support;
use axum_test::TestServer;
use dcmview::annotations::{self, AnnotationStore, EmbedRoiAnnotations};
use dcmview::server;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn returns_annotations_for_matching_file_index() {
    let dir = tempdir().expect("temp dir");
    let entry = support::file_entry(
        dir.path().join("annotated.dcm"),
        "1.2.840.10008.1.2.4.50",
        10,
    );
    let state = support::app_state_with_annotations(
        vec![entry],
        AnnotationStore::new(HashMap::from([(
            0,
            EmbedRoiAnnotations {
                num_roi: 2,
                roi_coords: vec![[11, 22, 33, 44], [55, 66, 77, 88]],
                roi_frames: vec![vec![0, 1, 2], vec![3]],
            },
        )])),
    );

    let app = server::router(state);
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/annotations").await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert_eq!(body["num_roi"], 2);
    assert_eq!(body["roi_coords"][0], serde_json::json!([11, 22, 33, 44]));
    assert_eq!(body["roi_coords"][1], serde_json::json!([55, 66, 77, 88]));
    assert_eq!(body["roi_frames"][0], serde_json::json!([0, 1, 2]));
    assert_eq!(body["roi_frames"][1], serde_json::json!([3]));
}

#[tokio::test]
async fn returns_empty_annotations_payload_when_file_has_no_match() {
    let dir = tempdir().expect("temp dir");
    let entry = support::file_entry(
        dir.path().join("unannotated.dcm"),
        "1.2.840.10008.1.2.4.50",
        1,
    );
    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/annotations").await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert_eq!(body["num_roi"], 0);
    assert_eq!(body["roi_coords"], serde_json::json!([]));
    assert_eq!(body["roi_frames"], serde_json::json!([]));
}

#[tokio::test]
async fn returns_not_found_for_out_of_range_file_index() {
    let app = server::router(support::app_state(Vec::new()));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/99/annotations").await;
    response.assert_status_not_found();
    let body: Value = response.json();
    assert_eq!(body["error"], "file index out of range");
}

#[tokio::test]
async fn health_and_annotation_edits_remain_available_while_csv_is_loading() {
    let dir = tempdir().expect("temp dir");
    let mut entry = support::file_entry(
        dir.path().join("editable-while-loading.dcm"),
        "1.2.840.10008.1.2.4.50",
        1,
    );
    entry.rows = 64;
    entry.columns = 64;
    let store = AnnotationStore::loading();
    let state = support::app_state_with_annotations(vec![entry], store.clone());
    let test_server = TestServer::new(server::router(state));

    test_server.get("/api/health").await.assert_status_ok();
    test_server
        .put("/api/file/0/annotations")
        .json(&EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[1, 2, 3, 4]],
            roi_frames: vec![vec![0]],
        })
        .await
        .assert_status_ok();

    store
        .commit_csv_if_unedited(HashMap::from([(
            0,
            EmbedRoiAnnotations {
                num_roi: 1,
                roi_coords: vec![[10, 20, 30, 40]],
                roi_frames: vec![vec![0]],
            },
        )]))
        .expect("complete CSV loading");

    let response = test_server.get("/api/file/0/annotations").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["roi_coords"][0], serde_json::json!([1, 2, 3, 4]));
}

#[tokio::test]
async fn annotation_load_failure_returns_json_without_affecting_health() {
    let dir = tempdir().expect("temp dir");
    let entry = support::file_entry(
        dir.path().join("failed-annotations.dcm"),
        "1.2.840.10008.1.2.4.50",
        1,
    );
    let store = AnnotationStore::loading();
    let state = support::app_state_with_annotations(vec![entry], store.clone());
    let test_server = TestServer::new(server::router(state));
    store
        .fail_loading("annotations CSV row 2: invalid ROI_coords")
        .expect("fail loading");

    let response = test_server.get("/api/file/0/annotations").await;
    response.assert_status_internal_server_error();
    let body: Value = response.json();
    assert_eq!(body["error"], "annotations CSV row 2: invalid ROI_coords");
    test_server.get("/api/health").await.assert_status_ok();
}

/// Exercises the full pipeline: CSV parsing → path matching → HTTP response.
/// The earlier tests manually insert annotations into AppState, bypassing
/// `load_annotations_for_files`. This test confirms the two halves compose
/// correctly: a CSV row whose path matches a loaded FileEntry must produce a
/// non-empty annotation response for that file's index.
#[tokio::test]
async fn serves_annotations_loaded_from_csv_for_matching_file() {
    let dir = tempdir().expect("temp dir");
    let dcm_path = dir.path().join("annotated.dcm");
    let csv_path = dir.path().join("annotations.csv");

    support::write_encapsulated_dicom(
        &dcm_path,
        "1.2.840.10008.1.2.4.50",
        vec![vec![0u8; 16 * 16]],
    );

    fs::write(
        &csv_path,
        format!(
            "anon_dicom_path,ROI_coords,ROI_frames\n{},\"[[10,20,30,40]]\",\"[[0]]\"\n",
            dcm_path.display()
        ),
    )
    .expect("write annotations CSV");

    let entry = support::file_entry(dcm_path, "1.2.840.10008.1.2.4.50", 1);
    let annotation_map =
        annotations::load_annotations_for_files(&csv_path, &[std::sync::Arc::new(entry.clone())])
            .expect("load_annotations_for_files should succeed");

    assert!(
        !annotation_map.is_empty(),
        "annotation_map is empty — CSV path did not match FileEntry path; path normalization may be broken"
    );

    let state =
        support::app_state_with_annotations(vec![entry], AnnotationStore::new(annotation_map));

    let app = server::router(state);
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/annotations").await;
    response.assert_status_ok();

    let body: Value = response.json();
    assert_eq!(body["num_roi"], 1, "expected 1 ROI from CSV");
    assert_eq!(body["roi_coords"][0], serde_json::json!([10, 20, 30, 40]));
    assert_eq!(body["roi_frames"][0], serde_json::json!([0]));
}

#[tokio::test]
async fn put_replaces_annotations_and_canonicalizes_payload() {
    let dir = tempdir().expect("temp dir");
    let mut entry =
        support::file_entry(dir.path().join("editable.dcm"), "1.2.840.10008.1.2.4.50", 4);
    entry.rows = 100;
    entry.columns = 120;
    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let response = test_server
        .put("/api/file/0/annotations")
        .json(&EmbedRoiAnnotations {
            num_roi: 99,
            roi_coords: vec![[30, 40, 10, 20]],
            roi_frames: vec![vec![0, 3]],
        })
        .await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["num_roi"], 1);
    assert_eq!(body["roi_coords"][0], serde_json::json!([10, 20, 30, 40]));
    assert_eq!(body["roi_frames"][0], serde_json::json!([0, 3]));

    let get_response = test_server.get("/api/file/0/annotations").await;
    get_response.assert_status_ok();
    let get_body: Value = get_response.json();
    assert_eq!(get_body["num_roi"], 1);
    assert_eq!(
        get_body["roi_coords"][0],
        serde_json::json!([10, 20, 30, 40])
    );
}

#[tokio::test]
async fn put_rejects_out_of_bounds_coords_and_frames() {
    let dir = tempdir().expect("temp dir");
    let mut entry =
        support::file_entry(dir.path().join("editable.dcm"), "1.2.840.10008.1.2.4.50", 2);
    entry.rows = 16;
    entry.columns = 16;
    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    let bounds_response = test_server
        .put("/api/file/0/annotations")
        .json(&EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[0, 0, 17, 5]],
            roi_frames: vec![vec![0]],
        })
        .await;
    bounds_response.assert_status_bad_request();
    let bounds_body: Value = bounds_response.json();
    assert!(bounds_body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("exceeds image bounds"));

    let frame_response = test_server
        .put("/api/file/0/annotations")
        .json(&EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[0, 0, 8, 8]],
            roi_frames: vec![vec![2]],
        })
        .await;
    frame_response.assert_status_bad_request();
    let frame_body: Value = frame_response.json();
    assert!(frame_body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("contains frame 2"));
}

#[tokio::test]
async fn export_csv_returns_current_in_memory_annotations() {
    let dir = tempdir().expect("temp dir");
    let mut entry =
        support::file_entry(dir.path().join("exported.dcm"), "1.2.840.10008.1.2.4.50", 3);
    entry.rows = 64;
    entry.columns = 64;
    let app = server::router(support::app_state(vec![entry]));
    let test_server = TestServer::new(app);

    test_server
        .put("/api/file/0/annotations")
        .json(&EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[1, 2, 3, 4]],
            roi_frames: vec![vec![1, 2]],
        })
        .await
        .assert_status_ok();

    let response = test_server.get("/api/annotations/export.csv").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("anon_dicom_path,num_ROI,ROI_coords,ROI_frames"));
    assert!(body.contains("exported.dcm"));
    assert!(body.contains("\"[[1,2,3,4]]\""));
    assert!(body.contains("\"[[1,2]]\""));
}
