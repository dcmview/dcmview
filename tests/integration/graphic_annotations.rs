//! Graphic annotations of the committed softcopy presentation state
//! fixtures, loaded through the production discovery path.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use serde_json::{json, Value};
use std::path::PathBuf;

const SINGLE: &str = "golden-gsps-target-u8.dcm";
const MULTIFRAME: &str = "golden-gsps-target-multiframe-u8.dcm";

/// A server over `names` and the index of each name, in the same order.
async fn serve(names: &[&str]) -> (TestServer, Vec<usize>) {
    let paths = names
        .iter()
        .map(|name| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name)
        })
        .collect::<Vec<_>>();
    let report = support::discover(
        &paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discover presentation state fixtures");
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

async fn annotations(server: &TestServer, image: usize, frame: u32, state: usize) -> Value {
    let response = server
        .get(&format!(
            "/api/file/{image}/frame/{frame}/graphic-annotations?state={state}"
        ))
        .await;
    response.assert_status_ok();
    response.json()
}

async fn context(server: &TestServer, state: usize) -> Value {
    let response = server
        .get(&format!("/api/file/{state}/semantic-context"))
        .await;
    response.assert_status_ok();
    let body = response.json::<Value>();
    assert_eq!(body["context"]["kind"], "presentation_state");
    body["context"].clone()
}

fn graphic_types(body: &Value) -> Vec<&str> {
    body["graphics"]
        .as_array()
        .expect("graphics")
        .iter()
        .map(|graphic| graphic["graphic_type"].as_str().expect("graphic type"))
        .collect()
}

fn texts(body: &Value) -> Vec<&str> {
    body["texts"]
        .as_array()
        .expect("texts")
        .iter()
        .map(|text| text["text"].as_str().expect("text"))
        .collect()
}

#[tokio::test]
async fn conforming_state_reports_every_graphic_type_in_image_pixels() {
    let (server, indices) = serve(&[SINGLE, "golden-gsps-conforming.dcm"]).await;
    let body = annotations(&server, indices[0], 0, indices[1]).await;

    assert_eq!(
        graphic_types(&body),
        [
            "ellipse",
            "ellipse",
            "circle",
            "polyline",
            "polyline",
            "interpolated",
            "circle",
            "point",
            "polyline",
            "polyline",
        ]
    );
    let graphics = body["graphics"].as_array().expect("graphics");
    // The rotated ellipse keeps its axis order: major endpoints, then minor.
    assert_eq!(
        graphics[1]["points"],
        json!([[106.0, 27.0], [154.0, 63.0], [139.0, 33.0], [121.0, 57.0]])
    );
    assert_eq!(graphics[1]["item"], 1);
    assert_eq!(graphics[2]["points"], json!([[200.0, 40.0], [222.0, 40.0]]));
    assert_eq!(graphics[2]["filled"], false);
    assert_eq!(graphics[6]["filled"], true);
    assert_eq!(graphics[7]["points"], json!([[120.5, 150.5]]));
    // The border runs from the image's first corner to Columns\Rows.
    assert_eq!(
        graphics[9]["points"],
        json!([
            [0.0, 0.0],
            [240.0, 0.0],
            [240.0, 160.0],
            [0.0, 160.0],
            [0.0, 0.0]
        ])
    );
    assert_eq!(graphics[9]["layer"], "MARKS");

    assert_eq!(
        texts(&body),
        ["Rotated ellipse", "Disc", "Point\n120.5, 150.5"]
    );
    let disc = &body["texts"][1];
    assert_eq!(disc["bounding_box"], json!([185.0, 66.0, 215.0, 76.0]));
    assert_eq!(disc["justification"], "center");
    assert_eq!(disc["anchor"], json!([200.0, 62.0]));
    assert_eq!(disc["anchor_visible"], true);
    let point = &body["texts"][2];
    assert_eq!(point["bounding_box"], Value::Null);
    assert_eq!(point["anchor_visible"], false);

    let layers = body["layers"].as_array().expect("layers");
    assert_eq!(
        layers
            .iter()
            .map(|layer| layer["name"].as_str().expect("layer name"))
            .collect::<Vec<_>>(),
        ["SHAPES", "LABELS", "MARKS"]
    );
    let yellow = layers[0]["color"].as_array().expect("CIELab layer color");
    assert!(yellow[0].as_u64() > Some(200) && yellow[2].as_u64() < Some(120));
    assert_eq!(layers[1]["color"], json!([255, 255, 255]));
    assert_eq!(layers[2]["color"], Value::Null);
    assert_eq!(
        body["skipped"],
        json!({ "display_units": 0, "matrix_units": 0, "malformed": 0, "masked_text": 0 })
    );
}

#[tokio::test]
async fn frame_scoped_items_are_drawn_on_their_own_frame_only() {
    let (server, indices) = serve(&[MULTIFRAME, "golden-gsps-frames.dcm"]).await;

    for frame in 0..3 {
        let body = annotations(&server, indices[0], frame, indices[1]).await;
        assert_eq!(graphic_types(&body), ["circle", "polyline"]);
        let name = format!("Frame {} only", frame + 1);
        assert_eq!(texts(&body), [name.as_str(), "All frames"]);
        assert_eq!(body["graphics"][0]["item"], frame);
        assert_eq!(body["graphics"][1]["item"], 3);
    }

    let context = context(&server, indices[1]).await;
    let items = context["items"].as_array().expect("items");
    assert_eq!(items.len(), 4);
    assert_eq!(items[1]["first_frame"]["frame_index"], 1);
    assert_eq!(items[1]["frame_count"], 1);
    assert_eq!(items[1]["scoped"], true);
    assert_eq!(items[3]["first_frame"]["frame_index"], 0);
    assert_eq!(items[3]["frame_count"], 3);
    assert_eq!(
        context["annotated_frames"]
            .as_array()
            .expect("annotated frames")
            .iter()
            .map(|frame| (
                frame["file_index"].as_u64().expect("file") as usize,
                frame["frame_index"].as_u64().expect("frame")
            ))
            .collect::<Vec<_>>(),
        [(indices[0], 0), (indices[0], 1), (indices[0], 2)]
    );
    assert_eq!(context["content_label"], "FRAMES");
}

#[tokio::test]
async fn unscoped_item_is_drawn_on_every_referenced_image_and_frame() {
    let (server, indices) = serve(&[SINGLE, MULTIFRAME, "golden-gsps-unscoped.dcm"]).await;

    for (image, frame) in [(indices[0], 0), (indices[1], 0), (indices[1], 2)] {
        let body = annotations(&server, image, frame, indices[2]).await;
        assert_eq!(graphic_types(&body), ["polyline", "polyline"]);
        assert_eq!(texts(&body), ["Every referenced image"]);
    }

    let context = context(&server, indices[2]).await;
    let item = &context["items"][0];
    assert_eq!(item["scoped"], false);
    assert_eq!(item["frame_count"], 4);
    assert_eq!(
        context["annotated_frames"]
            .as_array()
            .expect("annotated frames")
            .len(),
        4
    );
}

#[tokio::test]
async fn a_state_draws_nothing_on_an_image_it_does_not_reference() {
    let (server, indices) = serve(&[SINGLE, MULTIFRAME, "golden-gsps-conforming.dcm"]).await;
    let body = annotations(&server, indices[1], 0, indices[2]).await;

    assert_eq!(body["graphics"], json!([]));
    assert_eq!(body["texts"], json!([]));
    // The layers describe the state, whichever frame is asked for.
    assert_eq!(body["layers"].as_array().expect("layers").len(), 3);

    let context = context(&server, indices[2]).await;
    assert!(context["annotated_frames"]
        .as_array()
        .expect("annotated frames")
        .iter()
        .all(|frame| frame["file_index"] == indices[0]));
}

#[tokio::test]
async fn display_unit_objects_are_counted_and_not_drawn() {
    let (server, indices) = serve(&[SINGLE, "golden-gsps-display-units.dcm"]).await;
    let body = annotations(&server, indices[0], 0, indices[1]).await;

    assert_eq!(graphic_types(&body), ["circle"]);
    assert_eq!(body["texts"], json!([]));
    assert_eq!(
        body["skipped"],
        json!({ "display_units": 2, "matrix_units": 0, "malformed": 0, "masked_text": 0 })
    );

    let context = context(&server, indices[1]).await;
    assert_eq!(context["skipped"]["display_units"], 2);
    // The item holding only DISPLAY-unit objects has nothing to draw.
    assert_eq!(context["items"][0]["graphic_types"], json!([]));
    assert_eq!(context["items"][1]["graphic_types"], json!(["circle"]));
}

#[tokio::test]
async fn graphic_annotation_requests_are_validated() {
    let (server, indices) = serve(&[SINGLE, "golden-gsps-conforming.dcm"]).await;
    let (image, state) = (indices[0], indices[1]);
    let status = |path: String| {
        let server = &server;
        async move {
            let response = server.get(&path).await;
            assert!(
                response.json::<Value>()["code"].is_string(),
                "{path} should use the JSON error envelope"
            );
            response.status_code()
        }
    };

    assert_eq!(
        status(format!("/api/file/{image}/frame/0/graphic-annotations")).await,
        StatusCode::BAD_REQUEST
    );
    // An image is not a presentation state.
    assert_eq!(
        status(format!(
            "/api/file/{image}/frame/0/graphic-annotations?state={image}"
        ))
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(format!(
            "/api/file/{image}/frame/0/graphic-annotations?state=99"
        ))
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(format!(
            "/api/file/{image}/frame/5/graphic-annotations?state={state}"
        ))
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn presentation_state_references_resolve_to_its_images() {
    let (server, indices) = serve(&[SINGLE, MULTIFRAME, "golden-gsps-unscoped.dcm"]).await;
    let response = server
        .get(&format!("/api/file/{}/references", indices[2]))
        .await;
    response.assert_status_ok();
    let body = response.json::<Value>();
    let matched = body["references"]
        .as_array()
        .expect("references")
        .iter()
        .map(|reference| {
            reference["matches"][0]["file_index"]
                .as_u64()
                .expect("match") as usize
        })
        .collect::<Vec<_>>();

    assert_eq!(matched, [indices[0], indices[1]]);
}
