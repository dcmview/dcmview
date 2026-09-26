use super::support;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use image::{DynamicImage, ImageFormat};
use std::path::PathBuf;

/// Mid-gray value every grayscale shutter fixture displays.
const GRAY: [u8; 1] = [128];
/// Color of every pixel of the RGB shutter fixtures.
const RGB: [u8; 3] = [40, 80, 160];

/// The circle centered on row 4, column 5 with radius 3.
const CIRCLE: &str = "
    ....#...
    ..#####.
    ..#####.
    .#######
    ..#####.
    ..#####.
    ....#...
    ........
";

/// The triangle with vertices (1,1), (1,8), (6,1).
const TRIANGLE: &str = "
    ########
    ######..
    #####...
    ###.....
    ##......
    #.......
    ........
    ........
";

/// Discovers one committed 8x8 shutter fixture and returns one frame's
/// decoded display PNG and raw samples.
async fn shutter_fixture_frame(name: &str, frame: u32) -> (DynamicImage, Vec<u8>) {
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
    let display = test_server.get(&format!("/api/file/0/frame/{frame}")).await;
    display.assert_status_ok();
    let image = image::load_from_memory_with_format(display.as_bytes().as_ref(), ImageFormat::Png)
        .expect("shutter display PNG");
    assert_eq!((image.width(), image.height()), (8, 8), "{name}");
    let raw = test_server
        .get(&format!("/api/file/0/frame/{frame}/raw"))
        .await;
    raw.assert_status_ok();
    (image, raw.as_bytes().to_vec())
}

/// Expands a row-per-line picture of the frame into display samples: `#` is
/// the unshuttered `image` pixel, `.` the `shutter` fill, and `o` a visible
/// overlay pixel (255).
fn expected(picture: &str, image: &[u8], shutter: &[u8]) -> Vec<u8> {
    picture
        .chars()
        .filter(|pixel| !pixel.is_whitespace())
        .flat_map(|pixel| match pixel {
            '#' => image.to_vec(),
            '.' => shutter.to_vec(),
            'o' => vec![255],
            other => panic!("unexpected pixel {other}"),
        })
        .collect()
}

async fn assert_gray_shutter_fixture(name: &str, picture: &str, shutter_value: u8) {
    let (display, raw) = shutter_fixture_frame(name, 0).await;
    assert_eq!(
        display.to_luma8().into_raw(),
        expected(picture, &GRAY, &[shutter_value]),
        "{name}"
    );
    // The shutter is presentation only: raw samples stay untouched.
    assert_eq!(raw, [128_u8; 64], "{name}");
}

async fn assert_rgb_shutter_fixture(name: &str, picture: &str, shutter_color: [u8; 3]) {
    let (display, raw) = shutter_fixture_frame(name, 0).await;
    assert_eq!(
        display.to_rgb8().into_raw(),
        expected(picture, &RGB, &shutter_color),
        "{name}"
    );
    assert_eq!(raw, RGB.repeat(64), "{name}");
}

#[tokio::test]
async fn circular_shutter_centers_on_row_then_column() {
    assert_gray_shutter_fixture("golden-shutter-circular-u8.dcm", CIRCLE, 0).await;
}

#[tokio::test]
async fn polygonal_shutter_keeps_the_triangle_and_its_edges() {
    assert_gray_shutter_fixture("golden-shutter-polygonal-u8.dcm", TRIANGLE, 0).await;
}

#[tokio::test]
async fn combined_shutter_shows_only_the_intersection_in_white() {
    assert_gray_shutter_fixture(
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
    assert_gray_shutter_fixture(
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

#[tokio::test]
async fn native_color_shutter_uses_the_cielab_presentation_color() {
    // The fixture's CIELab value is sRGB red; its gray value is black.
    assert_rgb_shutter_fixture(
        "golden-shutter-circular-cielab-rgb-u8.dcm",
        CIRCLE,
        [255, 0, 0],
    )
    .await;
}

#[tokio::test]
async fn rle_color_shutter_falls_back_to_the_gray_presentation_value() {
    assert_rgb_shutter_fixture(
        "golden-shutter-polygonal-rle-rgb-u8.dcm",
        TRIANGLE,
        [255, 255, 255],
    )
    .await;
}
