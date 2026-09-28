//! Runs the shared windowing oracle (`tests/windowing-cases.json`) through the
//! real server path: a DICOM file written with the case's tags, the loader,
//! `AppState`, and the HTTP display and raw endpoints. The frontend renderer
//! consumes the same cases in `frontend/src/lib/rawWindowing.test.ts`.

use super::support;
use axum_test::{TestResponse, TestServer};
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;
use image::ImageFormat;
use serde::Deserialize;
use std::path::Path;
use tempfile::tempdir;

#[derive(Deserialize)]
struct Oracle {
    cases: Vec<WindowingCase>,
}

#[derive(Deserialize)]
struct WindowingCase {
    name: String,
    stored: Vec<u16>,
    rescale_slope: f64,
    rescale_intercept: f64,
    photometric_interpretation: String,
    dicom_window: Option<Window>,
    padding: Option<Padding>,
    mode: String,
    wc: Option<f64>,
    ww: Option<f64>,
    real_world: Option<RealWorld>,
    shutter: Option<Shutter>,
    overlay: Option<Vec<u8>>,
    layer: Option<Vec<Option<u8>>>,
    expected: Vec<u8>,
}

/// A rectangular shutter opening over one-based columns of the one-row frame.
#[derive(Deserialize)]
struct Shutter {
    left: u16,
    right: u16,
    presentation_value: u16,
}

/// A Real World Value Mapping item the window is in the unit of.
#[derive(Deserialize)]
struct RealWorld {
    unit: String,
    first_value_mapped: u16,
    last_value_mapped: u16,
    lut: Option<Vec<f64>>,
    slope: Option<f64>,
    intercept: Option<f64>,
}

#[derive(Deserialize)]
struct Window {
    center: f64,
    width: f64,
}

#[derive(Deserialize)]
struct Padding {
    value: u16,
    range_limit: Option<u16>,
}

fn load_oracle() -> Oracle {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/windowing-cases.json");
    let text = std::fs::read_to_string(&path).expect("read windowing oracle");
    serde_json::from_str(&text).expect("parse windowing oracle")
}

fn write_case_dicom(path: &Path, case: &WindowingCase) {
    let center = case.dicom_window.as_ref().map(|w| w.center.to_string());
    let width = case.dicom_window.as_ref().map(|w| w.width.to_string());
    support::write_uncompressed_u16_dicom_with_photometric(
        path,
        "1.2.840.10008.1.2.1",
        (1, case.stored.len() as u16),
        case.stored.clone(),
        &case.photometric_interpretation,
        center.as_deref(),
        width.as_deref(),
    );

    let mut object = dicom_object::open_file(path).expect("reopen oracle DICOM");
    object.put(DataElement::new(
        tags::RESCALE_SLOPE,
        VR::DS,
        PrimitiveValue::from(case.rescale_slope.to_string()),
    ));
    object.put(DataElement::new(
        tags::RESCALE_INTERCEPT,
        VR::DS,
        PrimitiveValue::from(case.rescale_intercept.to_string()),
    ));
    if let Some(padding) = &case.padding {
        object.put(DataElement::new(
            tags::PIXEL_PADDING_VALUE,
            VR::US,
            PrimitiveValue::from(padding.value),
        ));
        if let Some(limit) = padding.range_limit {
            object.put(DataElement::new(
                tags::PIXEL_PADDING_RANGE_LIMIT,
                VR::US,
                PrimitiveValue::from(limit),
            ));
        }
    }
    if let Some(shutter) = &case.shutter {
        for (tag, value) in [
            (tags::SHUTTER_LEFT_VERTICAL_EDGE, shutter.left),
            (tags::SHUTTER_RIGHT_VERTICAL_EDGE, shutter.right),
            (tags::SHUTTER_UPPER_HORIZONTAL_EDGE, 1),
            (tags::SHUTTER_LOWER_HORIZONTAL_EDGE, 1),
        ] {
            object.put(DataElement::new(tag, VR::IS, value.to_string()));
        }
        object.put(DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "RECTANGULAR"));
        object.put(DataElement::new(
            tags::SHUTTER_PRESENTATION_VALUE,
            VR::US,
            PrimitiveValue::from(shutter.presentation_value),
        ));
    }
    if let Some(bits) = &case.overlay {
        let overlay = |element: u16| dicom_core::Tag(0x6000, element);
        let mut words = vec![0_u16; bits.len().div_ceil(16)];
        for (index, _) in bits.iter().enumerate().filter(|(_, bit)| **bit != 0) {
            words[index / 16] |= 1 << (index % 16);
        }
        object.put(DataElement::new(
            overlay(0x0010),
            VR::US,
            PrimitiveValue::from(1_u16),
        ));
        object.put(DataElement::new(
            overlay(0x0011),
            VR::US,
            PrimitiveValue::from(bits.len() as u16),
        ));
        object.put(DataElement::new(overlay(0x0040), VR::CS, "G"));
        object.put(DataElement::new(
            overlay(0x0050),
            VR::SS,
            PrimitiveValue::from([1_i16, 1]),
        ));
        object.put(DataElement::new(
            overlay(0x0100),
            VR::US,
            PrimitiveValue::from(1_u16),
        ));
        object.put(DataElement::new(
            overlay(0x0102),
            VR::US,
            PrimitiveValue::from(0_u16),
        ));
        object.put(DataElement::new(
            overlay(0x3000),
            VR::OW,
            PrimitiveValue::U16(words.into()),
        ));
    }
    if let Some(mapping) = &case.real_world {
        object.put(DataElement::new(
            tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![real_world_item(mapping)]),
        ));
    }
    object.write_to_file(path).expect("write oracle DICOM");
}

fn real_world_item(mapping: &RealWorld) -> InMemDicomObject {
    let units = InMemDicomObject::from_element_iter([
        DataElement::new(tags::CODE_VALUE, VR::SH, mapping.unit.as_str()),
        DataElement::new(tags::CODING_SCHEME_DESIGNATOR, VR::SH, "UCUM"),
        DataElement::new(tags::CODE_MEANING, VR::LO, mapping.unit.as_str()),
    ]);
    let mut item = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::REAL_WORLD_VALUE_FIRST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(mapping.first_value_mapped),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_LAST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(mapping.last_value_mapped),
        ),
        DataElement::new(
            tags::MEASUREMENT_UNITS_CODE_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![units]),
        ),
    ]);
    if let Some(lut) = &mapping.lut {
        item.put(DataElement::new(
            tags::REAL_WORLD_VALUE_LUT_DATA,
            VR::FD,
            PrimitiveValue::F64(lut.iter().copied().collect()),
        ));
    }
    if let (Some(slope), Some(intercept)) = (mapping.slope, mapping.intercept) {
        item.put(DataElement::new(
            tags::REAL_WORLD_VALUE_SLOPE,
            VR::FD,
            PrimitiveValue::from(slope),
        ));
        item.put(DataElement::new(
            tags::REAL_WORLD_VALUE_INTERCEPT,
            VR::FD,
            PrimitiveValue::from(intercept),
        ));
    }
    item
}

fn display_url(case: &WindowingCase) -> String {
    let mut query = Vec::new();
    if case.mode != "default" {
        query.push(format!("mode={}", case.mode));
    }
    if let Some(wc) = case.wc {
        query.push(format!("wc={wc}"));
    }
    if let Some(ww) = case.ww {
        query.push(format!("ww={ww}"));
    }
    if let Some(mapping) = &case.real_world {
        query.push(format!("unit={}", mapping.unit));
    }
    if query.is_empty() {
        "/api/file/0/frame/0".to_string()
    } else {
        format!("/api/file/0/frame/0?{}", query.join("&"))
    }
}

fn header_f64(response: &TestResponse, name: &str) -> Option<f64> {
    response
        .maybe_header(name)
        .map(|value| value.to_str().expect("header utf-8").parse().expect("f64"))
}

/// The raw endpoint must carry exactly the inputs the client renderer is given
/// by the oracle, so both consumers window the same stored samples.
fn assert_raw_transport(case: &WindowingCase, raw: &TestResponse) {
    let name = &case.name;
    let stored_le = case
        .stored
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    assert_eq!(raw.as_bytes().as_ref(), stored_le, "{name}: raw samples");
    assert_eq!(
        raw.header("X-Frame-Photometric-Interpretation"),
        case.photometric_interpretation.as_str(),
        "{name}: photometric interpretation"
    );
    assert_eq!(
        header_f64(raw, "X-Frame-Rescale-Slope"),
        Some(case.rescale_slope),
        "{name}: rescale slope"
    );
    assert_eq!(
        header_f64(raw, "X-Frame-Rescale-Intercept"),
        Some(case.rescale_intercept),
        "{name}: rescale intercept"
    );
    assert_eq!(
        (
            header_f64(raw, "X-Frame-Default-Wc"),
            header_f64(raw, "X-Frame-Default-Ww")
        ),
        (
            case.dicom_window.as_ref().map(|w| w.center),
            case.dicom_window.as_ref().map(|w| w.width)
        ),
        "{name}: DICOM window"
    );
    let padding = case.padding.as_ref().map(|padding| {
        let limit = padding.range_limit.unwrap_or(padding.value);
        (
            f64::from(padding.value.min(limit)),
            f64::from(padding.value.max(limit)),
        )
    });
    assert_eq!(
        (
            header_f64(raw, "X-Frame-Padding-Low"),
            header_f64(raw, "X-Frame-Padding-High")
        ),
        (padding.map(|p| p.0), padding.map(|p| p.1)),
        "{name}: padding range"
    );
}

#[tokio::test]
async fn loader_driven_display_frames_match_the_shared_windowing_oracle() {
    let oracle = load_oracle();
    assert!(!oracle.cases.is_empty());

    for case in &oracle.cases {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("oracle.dcm");
        write_case_dicom(&path, case);

        let report = support::discover(
            &[path],
            DiscoverOptions {
                recursive: false,
                filters: Vec::new(),
            },
        )
        .await
        .expect("discover oracle DICOM");
        assert_eq!(report.files.len(), 1, "{}: discovered files", case.name);
        let test_server = TestServer::new(server::router(support::app_state(report.files)));

        let display = test_server.get(&display_url(case)).await;
        display.assert_status_ok();
        let pixels =
            image::load_from_memory_with_format(display.as_bytes().as_ref(), ImageFormat::Png)
                .expect("display PNG")
                .to_luma8()
                .into_raw();
        assert_eq!(pixels, case.expected, "{}: display pixels", case.name);

        let raw = test_server.get("/api/file/0/frame/0/raw").await;
        raw.assert_status_ok();
        assert_raw_transport(case, &raw);

        // The layer the client composites over its raw render: gray and
        // opaque where drawn, transparent elsewhere.
        let layer = test_server
            .get("/api/file/0/frame/0/presentation-layer")
            .await;
        layer.assert_status_ok();
        let layer =
            image::load_from_memory_with_format(layer.as_bytes().as_ref(), ImageFormat::Png)
                .expect("presentation layer PNG")
                .to_rgba8()
                .into_raw();
        let expected_layer = case
            .layer
            .clone()
            .unwrap_or_else(|| vec![None; case.stored.len()])
            .into_iter()
            .flat_map(|gray| gray.map_or([0; 4], |gray| [gray, gray, gray, 255]))
            .collect::<Vec<_>>();
        assert_eq!(layer, expected_layer, "{}: presentation layer", case.name);
    }
}

#[tokio::test]
async fn a_window_in_another_unit_shows_the_default_window() {
    let oracle = load_oracle();
    let case = oracle
        .cases
        .iter()
        .find(|case| case.real_world.is_some())
        .expect("a real-world oracle case");
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("oracle.dcm");
    write_case_dicom(&path, case);
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover oracle DICOM");
    let test_server = TestServer::new(server::router(support::app_state(report.files)));

    let default = test_server.get("/api/file/0/frame/0").await;
    let other_unit = test_server
        .get("/api/file/0/frame/0?wc=45&ww=70&unit=Gy")
        .await;
    other_unit.assert_status_ok();
    assert_eq!(other_unit.as_bytes(), default.as_bytes());

    let without_window = test_server.get("/api/file/0/frame/0?unit=SUV").await;
    without_window.assert_status_bad_request();
    assert_eq!(
        without_window.json::<serde_json::Value>()["code"],
        "invalid_window"
    );
}
