//! RT Dose and Parametric Map value overlays over the committed fixtures,
//! loaded through the production discovery path.

use super::support;
use axum::http::{header, StatusCode};
use axum_test::{TestResponse, TestServer};
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use image::RgbaImage;
use serde_json::Value;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A server over `names` and the index of each name, in the same order.
async fn serve(names: &[&str]) -> (TestServer, Vec<usize>) {
    let paths = names.iter().map(|name| fixture(name)).collect::<Vec<_>>();
    let report = support::discover(
        &paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover overlay fixtures");
    assert_eq!(report.files.len(), names.len());
    let indices = paths
        .iter()
        .map(|path| {
            report
                .files
                .iter()
                .find(|file| file.path == *path)
                .expect("fixture was selected")
                .index
        })
        .collect();
    (
        TestServer::new(server::router(support::app_state(report.files))),
        indices,
    )
}

/// The legend's color for `value`, interpolated as the legend documents.
fn legend_color(legend: &Value, value: f64) -> [u8; 3] {
    let min = legend["min_value"].as_f64().expect("min");
    let max = legend["max_value"].as_f64().expect("max");
    let stops = legend["color_stops"]
        .as_array()
        .expect("stops")
        .iter()
        .map(|stop| {
            let channels = stop.as_array().expect("stop channels");
            [0, 1, 2].map(|channel| channels[channel].as_f64().expect("channel"))
        })
        .collect::<Vec<_>>();
    let scaled = ((value - min) / (max - min)).clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let low = (scaled.floor() as usize).min(stops.len() - 1);
    let high = (low + 1).min(stops.len() - 1);
    let fraction = scaled - low as f64;
    [0, 1, 2].map(|channel| {
        (stops[low][channel] + (stops[high][channel] - stops[low][channel]) * fraction).round()
            as u8
    })
}

fn assert_color(image: &RgbaImage, x: u32, y: u32, expected: [u8; 3]) {
    let pixel = image.get_pixel(x, y).0;
    assert_eq!(pixel[3], 255, "pixel ({x}, {y}) should be opaque");
    for channel in 0..3 {
        assert!(
            pixel[channel].abs_diff(expected[channel]) <= 1,
            "pixel ({x}, {y}) is {pixel:?}, expected {expected:?}"
        );
    }
}

fn overlay_image(response: &TestResponse) -> RgbaImage {
    response.assert_status_ok();
    response.assert_header(header::CONTENT_TYPE, "image/png");
    image::load_from_memory(response.as_bytes().as_ref())
        .expect("decode overlay PNG")
        .to_rgba8()
}

fn assert_error(response: &TestResponse, status: StatusCode, code: &str) {
    assert_eq!(response.status_code(), status);
    assert_eq!(response.json::<Value>()["code"], code);
}

#[tokio::test]
async fn rt_dose_overlay_resamples_the_grid_onto_covered_slices() {
    let (server, indices) = serve(&[
        "golden-rtdose-u16-grid.dcm",
        "golden-rtdose-ct-source-z0.dcm",
        "golden-rtdose-ct-source-z6.dcm",
        "golden-rtdose-ct-source-z20.dcm",
    ])
    .await;
    let [dose, z0, z6, z20] = indices[..] else {
        unreachable!()
    };

    let context: Value = server
        .get(&format!("/api/file/{dose}/semantic-context"))
        .await
        .json();
    let context = &context["context"];
    assert_eq!(context["kind"], "rt_dose");
    assert_eq!(
        context["overlay"]["eligible"], true,
        "{}",
        context["overlay"]
    );
    assert_eq!(context["overlay"]["mapped_source_count"], 2);
    let covered = context["overlay_source_frames"]
        .as_array()
        .expect("covered frames")
        .iter()
        .map(|frame| {
            (
                frame["file_index"].as_u64().unwrap(),
                frame["frame_index"].clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut expected = vec![(z0 as u64, Value::from(0)), (z6 as u64, Value::from(0))];
    expected.sort_by_key(|(index, _)| *index);
    assert_eq!(covered, expected);
    let legend = &context["legend"];
    assert_eq!(legend["unit_label"], "Gy");
    assert_eq!(legend["min_value"], 0.0);
    // The largest stored value, 2000 + 300 + 30, scaled by 0.01 Gy.
    assert!((legend["max_value"].as_f64().unwrap() - 23.3).abs() < 1e-9);
    assert_eq!(legend["transparent_at_or_below"], 0.0);
    assert_eq!(legend["colormap"], "viridis");
    assert_eq!(legend["color_stops"].as_array().unwrap().len(), 9);

    let url = format!("/api/file/{z6}/frame/0/dose-overlay?dose={dose}");
    let first = server.get(&url).await;
    first.assert_header("X-Cache", "MISS");
    let overlay = overlay_image(&first);
    assert_eq!(overlay.dimensions(), (10, 10));
    // z = 6 is halfway between planes 1 and 2. CT pixel (row 2, column 4)
    // is dose voxel (1, 2): (1120 + 2120) / 2 stored, 16.2 Gy.
    assert_color(&overlay, 4, 2, legend_color(legend, 16.2));
    // CT pixel (row 3, column 1) is between voxels: rows 1 and 2, columns
    // 0 and 1, so (1500 + 150 + 5) stored, 16.55 Gy.
    assert_color(&overlay, 1, 3, legend_color(legend, 16.55));
    // Columns 8 and 9 lie beyond the grid's last half voxel.
    assert_eq!(overlay.get_pixel(8, 0).0[3], 0);
    let repeat = server.get(&url).await;
    repeat.assert_header("X-Cache", "HIT");
    assert_eq!(repeat.as_bytes(), first.as_bytes());

    let on_plane = overlay_image(
        &server
            .get(&format!("/api/file/{z0}/frame/0/dose-overlay?dose={dose}"))
            .await,
    );
    // Zero dose is transparent; the next voxel holds 10 stored, 0.1 Gy.
    assert_eq!(on_plane.get_pixel(0, 0).0[3], 0);
    assert_color(&on_plane, 2, 0, legend_color(legend, 0.1));

    assert_error(
        &server
            .get(&format!("/api/file/{z20}/frame/0/dose-overlay?dose={dose}"))
            .await,
        StatusCode::NOT_FOUND,
        "overlay_not_covering_frame",
    );
    assert_error(
        &server
            .get(&format!("/api/file/{z6}/frame/0/dose-overlay?dose={z0}"))
            .await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );
    assert_error(
        &server
            .get(&format!("/api/file/{z6}/frame/0/dose-overlay?dose=99"))
            .await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_error(
        &server
            .get(&format!("/api/file/{z6}/frame/1/dose-overlay?dose={dose}"))
            .await,
        StatusCode::NOT_FOUND,
        "frame_out_of_range",
    );
    assert_error(
        &server
            .get(&format!("/api/file/{z6}/frame/0/dose-overlay"))
            .await,
        StatusCode::BAD_REQUEST,
        "invalid_query",
    );
}

#[tokio::test]
async fn rt_dose_value_mapping_reports_dose_grid_scaling() {
    let (server, indices) = serve(&["golden-rtdose-u16-grid.dcm"]).await;
    let mapping: Value = server
        .get(&format!("/api/file/{}/frame/2/value-mapping", indices[0]))
        .await
        .json();
    assert_eq!(mapping["stored_value_type"], "integer");
    let dose = &mapping["real_world"][0];
    assert_eq!(dose["source"], "dose_grid_scaling");
    assert_eq!(dose["unit_label"], "Gy");
    assert_eq!(
        dose["transform"],
        serde_json::json!({"kind": "linear", "slope": 0.01, "intercept": 0.0})
    );
}

#[tokio::test]
async fn parametric_map_overlay_colors_mapped_values_on_its_sources() {
    let (server, indices) = serve(&[
        "golden-parametric-map-u16-linear.dcm",
        "golden-parametric-map-mr-source-z0.dcm",
        "golden-parametric-map-mr-source-z1.dcm",
        "golden-rtdose-ct-source-z0.dcm",
    ])
    .await;
    let [map, z0, z1, other_frame_of_reference] = indices[..] else {
        unreachable!()
    };

    let context: Value = server
        .get(&format!("/api/file/{map}/semantic-context"))
        .await
        .json();
    let context = &context["context"];
    assert_eq!(context["kind"], "parametric_map");
    assert_eq!(context["displayed_value_kind"], "stored");
    assert_eq!(
        context["overlay"]["eligible"], true,
        "{}",
        context["overlay"]
    );
    assert_eq!(context["overlay"]["mapped_source_count"], 2);
    let mut covered = context["overlay_source_frames"]
        .as_array()
        .expect("covered frames")
        .iter()
        .map(|frame| frame["file_index"].as_u64().unwrap() as usize)
        .collect::<Vec<_>>();
    covered.sort_unstable();
    let mut expected = vec![z0, z1];
    expected.sort_unstable();
    assert_eq!(covered, expected);
    let legend = &context["legend"];
    assert_eq!(legend["unit_label"], "um2/s");
    assert_eq!(legend["units"]["scheme"], "UCUM");
    // Mapped `0.5 * stored - 10` over stored 20..=1350.
    assert_eq!(legend["min_value"], 0.0);
    assert_eq!(legend["max_value"], 665.0);
    assert!(legend["transparent_at_or_below"].is_null());

    let url = format!("/api/file/{z1}/frame/0/parametric-map-overlay?map={map}");
    let first = server.get(&url).await;
    first.assert_header("X-Cache", "MISS");
    let overlay = overlay_image(&first);
    assert_eq!(overlay.dimensions(), (4, 4));
    // z = 1 lies halfway between the frames at z = 0 and 2: mapped
    // `250 + 50 * row + 5 * column`.
    assert_color(&overlay, 0, 0, legend_color(legend, 250.0));
    assert_color(&overlay, 3, 2, legend_color(legend, 365.0));
    server.get(&url).await.assert_header("X-Cache", "HIT");

    let on_frame = overlay_image(
        &server
            .get(&format!(
                "/api/file/{z0}/frame/0/parametric-map-overlay?map={map}"
            ))
            .await,
    );
    // Without a transparency floor the minimum is drawn, not hidden.
    assert_color(&on_frame, 0, 0, legend_color(legend, 0.0));
    assert_color(&on_frame, 1, 2, legend_color(legend, 105.0));

    assert_error(
        &server
            .get(&format!(
                "/api/file/{other_frame_of_reference}/frame/0/parametric-map-overlay?map={map}"
            ))
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "semantic_mapping_unavailable",
    );
    assert_error(
        &server
            .get(&format!(
                "/api/file/{z1}/frame/0/parametric-map-overlay?map={z0}"
            ))
            .await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );

    let mapping: Value = server
        .get(&format!("/api/file/{map}/frame/1/value-mapping"))
        .await
        .json();
    let adc = &mapping["real_world"][0];
    assert_eq!(adc["source"], "real_world_value_mapping");
    assert_eq!(adc["label"], "ADC");
    assert_eq!(adc["unit_label"], "um2/s");
    assert_eq!(adc["quantity"]["meaning"], "Apparent Diffusion Coefficient");
    assert_eq!(
        adc["transform"],
        serde_json::json!({"kind": "linear", "slope": 0.5, "intercept": -10.0})
    );
}
