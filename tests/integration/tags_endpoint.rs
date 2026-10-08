use super::support;
use axum_test::TestServer;
use dcmview::server;
use dcmview::types::FileEntry;
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use serde_json::Value;
use tempfile::tempdir;

#[tokio::test]
async fn serializes_sequences_and_binary_values_without_leaking_raw_pixel_data() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("tags-sequence.dcm");
    write_tag_fixture(&path);

    let file = FileEntry {
        format: Default::default(),
        raster: None,
        size_bytes: 0,
        index: 0,
        path: path.clone(),
        label: "tag fixture".to_string(),
        patient_id: String::new(),
        patient_name: String::new(),
        study_instance_uid: String::new(),
        study_date: String::new(),
        study_description: String::new(),
        series_instance_uid: String::new(),
        series_number: String::new(),
        series_description: String::new(),
        modality: String::new(),
        instance_number: String::new(),
        sop_instance_uid: String::new(),
        sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
        series_metadata: Default::default(),
        has_pixels: true,
        frame_count: 1,
        rows: 8,
        columns: 8,
        bits_allocated: 8,
        pixel_representation: 0,
        samples_per_pixel: 1,
        photometric_interpretation: "MONOCHROME2".to_string(),
        rescale_slope: 1.0,
        rescale_intercept: 0.0,
        transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
        default_window: None,
    };

    let app = server::router(support::app_state(vec![file]));
    let test_server = TestServer::new(app);

    let response = test_server.get("/api/file/0/tags").await;
    response.assert_status_ok();
    let payload: Value = response.json();
    let rows = payload.as_array().expect("tag list array");
    assert!(!rows.is_empty());

    let pixel_data_row = rows
        .iter()
        .find(|row| row["tag"] == "(7FE0,0010)")
        .expect("pixel data row");
    assert_eq!(pixel_data_row["value"]["type"], "binary");
    assert_eq!(pixel_data_row["value"]["length"], 512);

    let sequence_row = rows
        .iter()
        .find(|row| row["vr"] == "SQ")
        .expect("sequence row");
    assert_eq!(sequence_row["value"]["type"], "sequence");
    assert_eq!(
        sequence_row["value"]["items"]
            .as_array()
            .expect("sequence items")
            .len(),
        1
    );

    let tags = rows
        .iter()
        .map(|row| row["tag"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    let mut sorted = tags.clone();
    sorted.sort();
    assert_eq!(tags, sorted, "tags must be ordered by ascending tag number");
}

#[tokio::test]
async fn truncates_long_multibyte_tag_values_without_panicking() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("tags-multibyte.dcm");
    write_multibyte_tag_fixture(&path);

    let file = FileEntry {
        format: Default::default(),
        raster: None,
        size_bytes: 0,
        index: 0,
        path: path.clone(),
        label: "multibyte fixture".to_string(),
        patient_id: String::new(),
        patient_name: String::new(),
        study_instance_uid: String::new(),
        study_date: String::new(),
        study_description: String::new(),
        series_instance_uid: String::new(),
        series_number: String::new(),
        series_description: String::new(),
        modality: String::new(),
        instance_number: String::new(),
        sop_instance_uid: String::new(),
        sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
        series_metadata: Default::default(),
        has_pixels: false,
        frame_count: 1,
        rows: 0,
        columns: 0,
        bits_allocated: 8,
        pixel_representation: 0,
        samples_per_pixel: 1,
        photometric_interpretation: "MONOCHROME2".to_string(),
        rescale_slope: 1.0,
        rescale_intercept: 0.0,
        transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
        default_window: None,
    };

    let app = server::router(support::app_state(vec![file]));
    let test_server = TestServer::new(app);
    let response = test_server.get("/api/file/0/tags").await;
    response.assert_status_ok();

    let payload: Value = response.json();
    let rows = payload.as_array().expect("tag list array");
    let study_description_row = rows
        .iter()
        .find(|row| row["tag"] == "(0008,1030)")
        .expect("StudyDescription row");
    assert_eq!(study_description_row["value"]["type"], "string");
    let preview = study_description_row["value"]["value"]
        .as_str()
        .expect("string preview");
    assert!(
        preview.ends_with('…'),
        "long previews should be safely truncated"
    );
    assert!(
        !preview.is_empty(),
        "truncated preview should keep visible content"
    );
}

#[tokio::test]
async fn limits_large_numeric_arrays_and_sequences() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("tags-limits.dcm");
    write_limited_tag_fixture(&path);

    let file = FileEntry {
        format: Default::default(),
        raster: None,
        size_bytes: 0,
        index: 0,
        path: path.clone(),
        label: "limits fixture".to_string(),
        patient_id: String::new(),
        patient_name: String::new(),
        study_instance_uid: String::new(),
        study_date: String::new(),
        study_description: String::new(),
        series_instance_uid: String::new(),
        series_number: String::new(),
        series_description: String::new(),
        modality: String::new(),
        instance_number: String::new(),
        sop_instance_uid: String::new(),
        sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
        series_metadata: Default::default(),
        has_pixels: false,
        frame_count: 1,
        rows: 0,
        columns: 0,
        bits_allocated: 8,
        pixel_representation: 0,
        samples_per_pixel: 1,
        photometric_interpretation: "MONOCHROME2".to_string(),
        rescale_slope: 1.0,
        rescale_intercept: 0.0,
        transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
        default_window: None,
    };

    let app = server::router(support::app_state(vec![file]));
    let test_server = TestServer::new(app);
    let response = test_server.get("/api/file/0/tags").await;
    response.assert_status_ok();

    let payload: Value = response.json();
    let rows = payload.as_array().expect("tag list array");

    let window_center_row = rows
        .iter()
        .find(|row| row["tag"] == "(0028,1050)")
        .expect("WindowCenter row");
    assert_eq!(window_center_row["value"]["type"], "numbers");
    assert_eq!(
        window_center_row["value"]["value"]
            .as_array()
            .expect("limited numeric values")
            .len(),
        128
    );
    assert_eq!(window_center_row["value"]["truncated"], true);
    assert_eq!(window_center_row["value"]["total"], 150);

    let sequence_row = rows
        .iter()
        .find(|row| row["tag"] == "(0008,2218)")
        .expect("AnatomicRegionSequence row");
    assert_eq!(sequence_row["value"]["type"], "sequence");
    assert_eq!(
        sequence_row["value"]["items"]
            .as_array()
            .expect("limited sequence items")
            .len(),
        64
    );
    assert_eq!(sequence_row["value"]["truncated"], true);
    assert_eq!(sequence_row["value"]["total"], 70);

    let page = test_server
        .get("/api/file/0/tags/select?path=%280008%2C2218%29&offset=65&limit=3")
        .await;
    page.assert_status_ok();
    let page: Value = page.json();
    assert_eq!(page["tag"], "(0008,2218)");
    assert_eq!(page["value"]["items"].as_array().unwrap().len(), 3);
    assert_eq!(page["value"]["truncated"], true);
    assert_eq!(page["value"]["total"], 70);

    let nested = test_server
        .get("/api/file/0/tags/select?path=%280008%2C2218%29%2F69%2F%280008%2C0100%29")
        .await;
    nested.assert_status_ok();
    let nested: Value = nested.json();
    assert_eq!(nested["tag"], "(0008,0100)");
    assert_eq!(nested["value"]["value"], "T-00069");

    let invalid = test_server
        .get("/api/file/0/tags/select?path=%280008%2C2218%29%2Fmissing")
        .await;
    invalid.assert_status_bad_request();
    let invalid: Value = invalid.json();
    assert_eq!(invalid["code"], "bad_request");
}

fn write_multibyte_tag_fixture(path: &std::path::Path) {
    let long_text = "é".repeat(300);
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(
            tags::SOP_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from("2.25.333333333"),
        ),
        DataElement::new(
            tags::STUDY_DESCRIPTION,
            VR::LO,
            PrimitiveValue::from(long_text),
        ),
    ]);

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.333333333"),
        )
        .expect("build multibyte meta table");
    file_object
        .write_to_file(path)
        .expect("write multibyte tag fixture");
}

fn write_limited_tag_fixture(path: &std::path::Path) {
    let numeric_values = (0..150)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("\\");
    let sequence_items = (0..70)
        .map(|value| {
            InMemDicomObject::from_element_iter([DataElement::new(
                tags::CODE_VALUE,
                VR::SH,
                PrimitiveValue::from(format!("T-{value:05}")),
            )])
        })
        .collect::<Vec<_>>();

    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(
            tags::SOP_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from("2.25.444444444"),
        ),
        DataElement::new(
            tags::WINDOW_CENTER,
            VR::DS,
            PrimitiveValue::from(numeric_values),
        ),
    ]);

    obj.put(DataElement::new(
        tags::ANATOMIC_REGION_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(sequence_items),
    ));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.444444444"),
        )
        .expect("build limited tag meta table");
    file_object
        .write_to_file(path)
        .expect("write limited tag fixture");
}

fn write_tag_fixture(path: &std::path::Path) {
    let sequence_item = InMemDicomObject::from_element_iter([
        DataElement::new(tags::CODE_VALUE, VR::SH, PrimitiveValue::from("T-04000")),
        DataElement::new(tags::CODE_MEANING, VR::LO, PrimitiveValue::from("Breast")),
    ]);

    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(
            tags::SOP_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from("2.25.123123123"),
        ),
        DataElement::new(tags::PATIENT_NAME, VR::PN, PrimitiveValue::from("Doe^Jane")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::from(vec![7_u8; 512]),
        ),
    ]);

    obj.put(DataElement::new(
        tags::ANATOMIC_REGION_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(vec![sequence_item]),
    ));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.123123123"),
        )
        .expect("build meta table");
    file_object.write_to_file(path).expect("write tag fixture");
}
