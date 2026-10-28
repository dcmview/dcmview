//! Overlay Data draws the same plane whether a file stores it as VR OW or,
//! as files written before the attribute became OW alone do, as VR OB, in
//! either byte order.
//!
//! The fixtures under `tests/fixtures/overlay-data/` hold one 5x7 plane each
//! way: as 16-bit words in the byte order of the transfer syntax, and as a
//! byte stream whose first bit is the least significant bit of its first
//! byte.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server::{self, FileRegistry};
use std::path::PathBuf;

const ROWS: usize = 5;
const COLUMNS: usize = 7;

/// Whether bit `index` of the plane is set, as the generator writes it.
fn overlay_bit(index: usize) -> bool {
    index.is_multiple_of(3) || index % 7 == 2
}

const NAMES: [&str; 4] = ["le-ow", "le-ob", "be-ow", "be-ob"];

/// A server over the four fixtures, registered in the order of `NAMES`.
async fn fixture_server() -> TestServer {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/overlay-data");
    let paths = NAMES
        .iter()
        .map(|name| directory.join(format!("golden-overlay-data-5x7-{name}.dcm")))
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
    .expect("discover fixtures");
    let registry = FileRegistry::new();
    for path in &paths {
        let file = report
            .files
            .iter()
            .find(|file| &file.path == path)
            .unwrap_or_else(|| panic!("{} was not discovered", path.display()))
            .clone();
        registry.insert(file);
    }
    registry.mark_scan_complete();
    TestServer::new(server::router(support::app_state_with_registry(registry)))
}

fn png(bytes: &[u8]) -> image::DynamicImage {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png).expect("a PNG")
}

#[tokio::test]
async fn the_presentation_layer_draws_overlay_data_of_either_vr_and_byte_order() {
    let server = fixture_server().await;
    let written = (0..ROWS * COLUMNS).map(overlay_bit).collect::<Vec<_>>();
    assert_eq!(written.iter().filter(|set| **set).count(), 15);
    for (index, name) in NAMES.iter().enumerate() {
        let response = server
            .get(&format!("/api/file/{index}/frame/0/presentation-layer"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK, "{name}");
        let layer =
            image::load_from_memory_with_format(response.as_bytes(), image::ImageFormat::Png)
                .unwrap_or_else(|error| panic!("{name} layer is not a PNG: {error}"))
                .into_rgba8();
        assert_eq!(layer.dimensions(), (COLUMNS as u32, ROWS as u32), "{name}");
        // The layer is transparent except where an overlay bit is set.
        let drawn = layer
            .pixels()
            .map(|pixel| pixel.0[3] != 0)
            .collect::<Vec<_>>();
        assert_eq!(drawn, written, "{name}");
    }
}

#[tokio::test]
async fn overlay_planes_can_be_left_off_frames_and_drawn_alone() {
    let server = fixture_server().await;
    let files = server.get("/api/files").await.json::<serde_json::Value>();
    assert_eq!(files["files"][0]["overlay_planes"], 1);

    let layer = |query: &'static str| {
        let server = &server;
        async move {
            let response = server
                .get(&format!("/api/file/0/frame/0/presentation-layer{query}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::OK, "{query}");
            png(response.as_bytes()).into_rgba8()
        }
    };
    let written = (0..ROWS * COLUMNS).map(overlay_bit).collect::<Vec<_>>();
    let drawn = |image: &image::RgbaImage| {
        image
            .pixels()
            .map(|pixel| pixel.0[3] != 0)
            .collect::<Vec<_>>()
    };
    assert_eq!(drawn(&layer("?content=overlays").await), written);
    // The fixtures have no shutter, so their shutter layer is empty.
    assert!(drawn(&layer("?content=shutter").await)
        .iter()
        .all(|set| !set));
    assert!(server
        .get("/api/file/0/frame/0/presentation-layer?content=bogus")
        .await
        .status_code()
        .is_client_error());

    let frame = |query: &'static str| {
        let server = &server;
        async move {
            let response = server.get(&format!("/api/file/0/frame/0{query}")).await;
            assert_eq!(response.status_code(), StatusCode::OK, "{query}");
            png(response.as_bytes()).into_luma8().into_raw()
        }
    };
    let with_overlays = frame("").await;
    let without = frame("?overlays=false").await;
    // Overlay bits are drawn at full white; elsewhere the two frames agree.
    for (index, set) in written.iter().enumerate() {
        if *set {
            assert_eq!(with_overlays[index], 255, "pixel {index}");
        } else {
            assert_eq!(with_overlays[index], without[index], "pixel {index}");
        }
    }
    assert_ne!(with_overlays, without, "the frame without overlays differs");
    // Each is cached under its own key: asking again gives the same frame.
    assert_eq!(frame("").await, with_overlays);
    assert_eq!(frame("?overlays=false").await, without);
}
