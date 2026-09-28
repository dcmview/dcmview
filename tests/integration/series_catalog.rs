use super::support;
use axum_test::TestServer;
use dcmview::server;
use serde_json::Value;

const AXIAL: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
const WSI_SOP_CLASS: &str = "1.2.840.10008.5.1.4.1.1.77.1.6";

#[tokio::test]
async fn series_api_orders_geometry_and_keeps_shared_frame_of_reference_series_separate() {
    let mut files = Vec::new();
    for (path, series_uid, instance, z) in [
        ("series-a/slice-001.dcm", "series-a", "30", 0.0),
        ("series-a/slice-002.dcm", "series-a", "10", 5.0),
        ("series-a/slice-003.dcm", "series-a", "20", 10.0),
        ("series-b/slice-001.dcm", "series-b", "1", 0.0),
        ("series-b/slice-002.dcm", "series-b", "2", 5.0),
    ] {
        let mut file = support::file_entry(path.into(), "1.2.840.10008.1.2.1", 1);
        file.study_instance_uid = "study".to_string();
        file.series_instance_uid = series_uid.to_string();
        file.instance_number = instance.to_string();
        file.sop_instance_uid = format!("sop-{series_uid}-{path}");
        file.series_metadata.frame_of_reference_uid = "shared-for".to_string();
        file.series_metadata.image_position_patient = Some([0.0, 0.0, z]);
        file.series_metadata.image_orientation_patient = Some(AXIAL);
        files.push(file);
    }

    let response: Value = TestServer::new(server::router(support::app_state(files)))
        .get("/api/series")
        .await
        .json();
    assert_eq!(response["scan_complete"], true);
    let series = response["series"].as_array().expect("series array");
    assert_eq!(series.len(), 2);
    assert_eq!(series[0]["series_instance_uid"], "series-a");
    assert_eq!(series[1]["series_instance_uid"], "series-b");
    assert_eq!(
        series[0]["frame_of_reference_uids"],
        serde_json::json!(["shared-for"])
    );
    assert_eq!(
        series[1]["frame_of_reference_uids"],
        serde_json::json!(["shared-for"])
    );
    assert_eq!(
        series[0]["stacks"][0]["frames"]
            .as_array()
            .expect("frame array")
            .iter()
            .map(|frame| frame["sop_instance_uid"]
                .as_str()
                .expect("SOP instance UID"))
            .collect::<Vec<_>>(),
        vec![
            "sop-series-a-series-a/slice-001.dcm",
            "sop-series-a-series-a/slice-002.dcm",
            "sop-series-a-series-a/slice-003.dcm",
        ]
    );
}

#[tokio::test]
async fn series_api_models_concatenations_and_wsi_levels_without_cross_level_flattening() {
    let mut first = support::file_entry("concat/part-001.dcm".into(), "1.2.840.10008.1.2.1", 2);
    first.study_instance_uid = "study".to_string();
    first.series_instance_uid = "concat-series".to_string();
    first.sop_instance_uid = "concat-1".to_string();
    first.series_metadata.concatenation_uid = Some("concat".to_string());
    first.series_metadata.concatenation_frame_offset_number = Some(0);
    first.series_metadata.in_concatenation_number = Some(1);

    let mut second = support::file_entry("concat/part-002.dcm".into(), "1.2.840.10008.1.2.1", 1);
    second.study_instance_uid = "study".to_string();
    second.series_instance_uid = "concat-series".to_string();
    second.sop_instance_uid = "concat-2".to_string();
    second.series_metadata.concatenation_uid = Some("concat".to_string());
    second.series_metadata.concatenation_frame_offset_number = Some(2);
    second.series_metadata.in_concatenation_number = Some(2);

    let mut wsi_files = Vec::new();
    for (path, sop, role, pyramid_uid, matrix, frames) in [
        (
            "wsi/volume.dcm",
            "wsi-volume",
            "VOLUME",
            Some("pyramid"),
            4,
            4,
        ),
        (
            "wsi/thumbnail.dcm",
            "wsi-thumbnail",
            "THUMBNAIL",
            Some("pyramid"),
            2,
            1,
        ),
        ("wsi/label.dcm", "wsi-label", "LABEL", None, 2, 1),
    ] {
        let mut file = support::file_entry(path.into(), "1.2.840.10008.1.2.1", frames);
        file.study_instance_uid = "study".to_string();
        file.series_instance_uid = "wsi-series".to_string();
        file.sop_instance_uid = sop.to_string();
        file.sop_class_uid = WSI_SOP_CLASS.to_string();
        file.series_metadata.image_type = vec![
            "ORIGINAL".to_string(),
            "PRIMARY".to_string(),
            role.to_string(),
            "NONE".to_string(),
        ];
        file.series_metadata.pyramid_uid = pyramid_uid.map(ToString::to_string);
        file.series_metadata.total_pixel_matrix_rows = Some(matrix);
        file.series_metadata.total_pixel_matrix_columns = Some(matrix);
        wsi_files.push(file);
    }

    let mut files = vec![second, first];
    files.extend(wsi_files);
    let response: Value = TestServer::new(server::router(support::app_state(files)))
        .get("/api/series")
        .await
        .json();
    let series = response["series"].as_array().expect("series array");
    let concatenation = series
        .iter()
        .find(|item| item["series_instance_uid"] == "concat-series")
        .expect("concatenation series");
    assert_eq!(concatenation["stacks"][0]["kind"], "concatenation");
    assert_eq!(
        concatenation["stacks"][0]["frames"]
            .as_array()
            .expect("concatenation frames")
            .iter()
            .map(|frame| frame["sop_instance_uid"].as_str().expect("SOP UID"))
            .collect::<Vec<_>>(),
        vec!["concat-1", "concat-1", "concat-2"]
    );

    let wsi = series
        .iter()
        .find(|item| item["series_instance_uid"] == "wsi-series")
        .expect("WSI series");
    assert_eq!(
        wsi["stacks"]
            .as_array()
            .expect("WSI stacks")
            .iter()
            .map(|stack| stack["kind"].as_str().expect("stack kind"))
            .collect::<Vec<_>>(),
        vec!["wsi_pyramid_level", "wsi_pyramid_level", "wsi_companion"]
    );
    assert_eq!(wsi["stacks"][0]["frames"].as_array().unwrap().len(), 4);
    assert_eq!(wsi["stacks"][1]["frames"].as_array().unwrap().len(), 1);
    assert_eq!(wsi["stacks"][2]["frames"].as_array().unwrap().len(), 1);
}
