use super::support;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use image::ImageFormat;
use std::path::PathBuf;

/// Discovers one committed 8x8 shutter fixture and returns its display frame
/// and raw samples.
async fn shutter_fixture_frames(name: &str) -> (Vec<u8>, Vec<u8>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let report = support::discover(
        &[path],
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover shutter fixture");
    // Client raw rendering cannot draw the shutter, so the viewer must keep
    // the server presentation.
    let summary = dcmview::types::FileSummary::from(&report.files[0]);
    assert!(!summary.raw_windowing_compatible, "{name}");

    let test_server = TestServer::new(server::router(support::app_state(report.files)));
    let display = test_server.get("/api/file/0/frame/0").await;
    display.assert_status_ok();
    let pixels = image::load_from_memory_with_format(display.as_bytes().as_ref(), ImageFormat::Png)
        .expect("shutter display PNG")
        .to_luma8();
    assert_eq!(pixels.dimensions(), (8, 8), "{name}");
    let raw = test_server.get("/api/file/0/frame/0/raw").await;
    raw.assert_status_ok();
    (pixels.into_raw(), raw.as_bytes().to_vec())
}

/// Expands a row-per-line picture of the frame: `#` displays the mid-gray image (128),
/// `.` the shutter value, and `o` a visible overlay pixel (255).
fn expected(picture: &str, shutter_value: u8) -> Vec<u8> {
    picture
        .chars()
        .filter(|pixel| !pixel.is_whitespace())
        .map(|pixel| match pixel {
            '#' => 128,
            '.' => shutter_value,
            'o' => 255,
            other => panic!("unexpected pixel {other}"),
        })
        .collect()
}

async fn assert_shutter_fixture(name: &str, picture: &str, shutter_value: u8) {
    let (display, raw) = shutter_fixture_frames(name).await;
    assert_eq!(display, expected(picture, shutter_value), "{name}");
    // The shutter is presentation only: raw samples stay untouched.
    assert_eq!(raw, [128_u8; 64], "{name}");
}

#[tokio::test]
async fn circular_shutter_centers_on_row_then_column() {
    assert_shutter_fixture(
        "golden-shutter-circular-u8.dcm",
        "
            ....#...
            ..#####.
            ..#####.
            .#######
            ..#####.
            ..#####.
            ....#...
            ........
        ",
        0,
    )
    .await;
}

#[tokio::test]
async fn polygonal_shutter_keeps_the_triangle_and_its_edges() {
    assert_shutter_fixture(
        "golden-shutter-polygonal-u8.dcm",
        "
            ########
            ######..
            #####...
            ###.....
            ##......
            #.......
            ........
            ........
        ",
        0,
    )
    .await;
}

#[tokio::test]
async fn combined_shutter_shows_only_the_intersection_in_white() {
    assert_shutter_fixture(
        "golden-shutter-rectangular-circular-u8.dcm",
        "
            ...#....
            .#####..
            .#####..
            .######.
            .#####..
            ........
            ........
            ........
        ",
        255,
    )
    .await;
}

#[tokio::test]
async fn bitmap_shutter_masks_its_overlay_without_drawing_it() {
    // Overlay 6000 is the shutter mask, so its set bits take the shutter
    // value rather than the overlay white; overlay 6002 is still drawn.
    assert_shutter_fixture(
        "golden-shutter-bitmap-u8.dcm",
        "
            ........
            ........
            #######.
            #######.
            ###o###.
            #######.
            #######.
            #######.
        ",
        0,
    )
    .await;
}
