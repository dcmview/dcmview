//! Gallery thumbnails over the committed fixtures, loaded through the
//! production discovery path. The masking fixtures are 320x240 images with
//! bright block text in a 40-row banner above a textured wedge.

use super::support;
use axum::http::StatusCode;
use axum_test::{TestResponse, TestServer};
use dcmview::loader::DiscoverOptions;
use dcmview::masking::Masker;
use dcmview::server::{self, FileRegistry};
use image::RgbImage;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

const BANNER_IMAGE: &str = "golden-masking-patient-a-us-1.dcm";
const OTHER_IMAGE: &str = "golden-masking-patient-b-us.dcm";
const SLIDE_LABEL: &str = "golden-masking-wsi-label.dcm";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A server over `paths`, served in that order.
async fn serve_paths(paths: &[PathBuf], registry: FileRegistry) -> TestServer {
    let report = support::discover(
        paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discover fixtures");
    for path in paths {
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

async fn serve(names: &[&str]) -> TestServer {
    let paths = names
        .iter()
        .map(|name| fixtures().join(name))
        .collect::<Vec<_>>();
    serve_paths(&paths, FileRegistry::new()).await
}

fn header(response: &TestResponse, name: &str) -> String {
    response
        .header(name)
        .to_str()
        .expect("header text")
        .to_string()
}

/// A successful thumbnail: its pixels, `X-Cache` and `X-Thumbnail-Source`.
async fn thumbnail(server: &TestServer, path: &str) -> (RgbImage, String, String) {
    let response = server.get(path).await;
    assert_eq!(response.status_code(), StatusCode::OK, "{path}");
    assert_eq!(header(&response, "content-type"), "image/jpeg", "{path}");
    assert_eq!(header(&response, "cache-control"), "no-store", "{path}");
    let image = image::load_from_memory_with_format(response.as_bytes(), image::ImageFormat::Jpeg)
        .unwrap_or_else(|error| panic!("{path} is not a JPEG: {error}"))
        .into_rgb8();
    (
        image,
        header(&response, "x-cache"),
        header(&response, "x-thumbnail-source"),
    )
}

/// The brightest channel value in rows `rows` of `image`.
fn brightest(image: &RgbImage, rows: std::ops::Range<u32>) -> u8 {
    rows.flat_map(|row| (0..image.width()).map(move |column| (column, row)))
        .flat_map(|(column, row)| image.get_pixel(column, row).0)
        .max()
        .expect("rows inside the image")
}

/// Mean absolute difference per channel of two images of the same size.
fn mean_difference(left: &RgbImage, right: &RgbImage) -> f64 {
    assert_eq!(left.dimensions(), right.dimensions());
    let total: u64 = left
        .as_raw()
        .iter()
        .zip(right.as_raw())
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    total as f64 / left.as_raw().len() as f64
}

#[tokio::test]
async fn a_thumbnail_is_the_whole_frame_fitted_inside_its_size_bucket() {
    let server = serve(&[BANNER_IMAGE]).await;

    // (query, bucket, width and height of the 320x240 frame's thumbnail)
    let cases = [
        ("", 256, (256, 192)),
        ("?size=1", 128, (128, 96)),
        ("?size=128", 128, (128, 96)),
        ("?size=129", 256, (256, 192)),
        ("?size=300", 512, (320, 240)),
        ("?size=1024", 1024, (320, 240)),
        ("?size=128&window_mode=full_dynamic", 0, (128, 96)),
    ];
    let mut rendered = Vec::new();
    for (query, bucket, dimensions) in cases {
        let path = format!("/api/file/0/frame/0/thumbnail{query}");
        let (image, cache, source) = thumbnail(&server, &path).await;
        assert_eq!(image.dimensions(), dimensions, "{path}");
        // Sizes in one bucket are one cached thumbnail.
        let expected = if rendered.contains(&bucket) {
            ("HIT", "thumbnail_cache")
        } else {
            ("MISS", "full_decode")
        };
        assert_eq!((cache.as_str(), source.as_str()), expected, "{path}");
        rendered.push(bucket);

        let (again, cache, source) = thumbnail(&server, &path).await;
        assert_eq!(
            (cache.as_str(), source.as_str()),
            ("HIT", "thumbnail_cache"),
            "{path}"
        );
        assert_eq!(again, image, "{path}");
    }
}

#[tokio::test]
async fn a_thumbnail_has_the_physical_shape_of_non_square_pixels_in_the_stored_grid() {
    // 300 rows of 600 pixels that are twice as high as wide: a square. Only
    // the top-left quarter is bright.
    let (rows, columns) = (300_u16, 600_u16);
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("non-square.dcm");
    let samples = (0..rows)
        .flat_map(|row| {
            (0..columns).map(move |column| {
                if row < rows / 2 && column < columns / 2 {
                    3000
                } else {
                    0
                }
            })
        })
        .collect();
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        rows,
        columns,
        samples,
        Some("1500"),
        Some("3000"),
    );
    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = u32::from(rows);
    entry.columns = u32::from(columns);
    entry.series_metadata.native_pixel.normalized_pixel_aspect = Some([2.0, 1.0]);
    let server = TestServer::new(server::router(support::app_state(vec![entry])));

    // No axis is enlarged: at most 300 pixels along each.
    for (size, expected) in [(128, (128, 128)), (512, (300, 300))] {
        let path = format!("/api/file/0/frame/0/thumbnail?size={size}");
        let (image, _, _) = thumbnail(&server, &path).await;
        assert_eq!(image.dimensions(), expected, "{path}");
        let (width, height) = image.dimensions();
        let quarter = |column: u32, row: u32| image.get_pixel(column, row).0[0];
        assert!(quarter(width / 4, height / 4) > 200, "{path} top left");
        assert!(quarter(3 * width / 4, height / 4) < 50, "{path} top right");
        assert!(
            quarter(width / 4, 3 * height / 4) < 50,
            "{path} bottom left"
        );
        assert!(
            quarter(3 * width / 4, 3 * height / 4) < 50,
            "{path} bottom right"
        );
    }
}

/// Every committed fixture the viewer renders, without a display shutter or
/// overlay planes (a thumbnail omits those): its thumbnail is its default
/// display frame shrunk with an area filter, within what JPEG loses.
#[tokio::test]
async fn a_thumbnail_shows_the_default_display_frame_shrunk() {
    let mut paths = std::fs::read_dir(fixtures())
        .expect("fixture directory")
        .map(|entry| entry.expect("fixture entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "dcm"))
        .collect::<Vec<_>>();
    paths.sort();
    let server = serve_paths(&paths, FileRegistry::new()).await;
    let catalog: Value = server.get("/api/files").await.json();

    let mut compared = 0;
    for file in catalog["files"].as_array().expect("files") {
        if file["support_state"] != "renderable"
            || file["has_pixels"] != true
            || file["presentation_layer"] == true
        {
            continue;
        }
        let name = file["path"].as_str().expect("path");
        let index = file["index"].as_u64().expect("index");
        let frames = file["frame_count"].as_u64().expect("frame count");
        let (rows, columns) = (
            file["rows"].as_u64().expect("rows") as u32,
            file["columns"].as_u64().expect("columns") as u32,
        );
        let expected_size = dcmview::pixels::thumbnail_dimensions(
            rows,
            columns,
            file["pixel_aspect_ratio"].as_f64(),
            128,
        );
        // The last frame, so a multi-frame file proves the frame is honoured.
        let frame = frames - 1;
        for mode in ["default", "full_dynamic"] {
            let display = server
                .get(&format!("/api/file/{index}/frame/{frame}?mode={mode}"))
                .await;
            assert_eq!(display.status_code(), StatusCode::OK, "{name}");
            let display = image::load_from_memory(display.as_bytes())
                .expect("decode display PNG")
                .into_rgb8();
            let shrunk = image::imageops::thumbnail(&display, expected_size.0, expected_size.1);

            let (image, _, _) = thumbnail(
                &server,
                &format!("/api/file/{index}/frame/{frame}/thumbnail?size=128&window_mode={mode}"),
            )
            .await;
            assert_eq!(image.dimensions(), expected_size, "{name} {mode}");
            let difference = mean_difference(&image, &shrunk);
            assert!(
                difference <= 12.0,
                "{name} {mode}: thumbnail differs from the shrunk display frame by {difference:.2}"
            );
        }
        compared += 1;
    }
    assert!(compared >= 25, "only {compared} fixtures were compared");
}

#[tokio::test]
async fn a_thumbnail_of_a_redacted_frame_is_blanked_and_follows_the_boxes() {
    let server = serve(&[BANNER_IMAGE, OTHER_IMAGE]).await;
    // At the 128 bucket the 320x240 frame is 128x96 and its 40-row banner
    // is exactly the top 16 rows.
    let path = "/api/file/0/frame/0/thumbnail?size=128";
    let banner = 0..16;
    let below = 24..96;
    let whole_banner = json!({ "num_roi": 1, "roi_coords": [[0, 0, 40, 320]], "roi_frames": [] });
    let no_boxes = json!({ "num_roi": 0, "roi_coords": [], "roi_frames": [] });
    let put = |boxes: Value| {
        let server = &server;
        async move {
            server
                .put("/api/file/0/redactions")
                .json(&boxes)
                .await
                .assert_status_ok();
        }
    };

    let (before, cache, _) = thumbnail(&server, path).await;
    assert_eq!(cache, "MISS");
    assert!(
        brightest(&before, banner.clone()) > 128,
        "the banner holds text"
    );
    assert!(brightest(&before, below.clone()) > 128, "the wedge shows");

    put(whole_banner.clone()).await;
    let (redacted, cache, source) = thumbnail(&server, path).await;
    assert_eq!(
        (cache.as_str(), source.as_str()),
        ("MISS", "full_decode"),
        "the thumbnail cached before the box is not served"
    );
    assert!(
        brightest(&redacted, banner.clone()) <= 8,
        "the redacted banner shows through: {}",
        brightest(&redacted, banner.clone())
    );
    // The rows clear of the box are the same picture.
    let rows = |image: &RgbImage| {
        image::imageops::crop_imm(image, 0, below.start, 128, below.end - below.start).to_image()
    };
    assert!(mean_difference(&rows(&redacted), &rows(&before)) <= 1.0);
    assert_eq!(thumbnail(&server, path).await.1, "HIT");

    // Every bucket and window mode of the frame is redacted.
    for other in [
        "/api/file/0/frame/0/thumbnail?size=512",
        "/api/file/0/frame/0/thumbnail?size=128&window_mode=full_dynamic",
    ] {
        let (image, _, _) = thumbnail(&server, other).await;
        let banner_rows = 0..image.height() / 6;
        assert!(brightest(&image, banner_rows) <= 8, "{other}");
    }

    // A smaller box is a change too: the left half of the banner returns.
    put(json!({ "num_roi": 1, "roi_coords": [[0, 160, 40, 320]], "roi_frames": [] })).await;
    let (half, cache, _) = thumbnail(&server, path).await;
    assert_eq!(cache, "MISS", "a changed box is a new thumbnail");
    let side = |image: &RgbImage, columns: std::ops::Range<u32>| {
        columns
            .flat_map(|column| (0..16).map(move |row| image.get_pixel(column, row).0[0]))
            .max()
            .expect("columns inside the image")
    };
    assert!(side(&half, 0..56) > 128, "the unredacted half shows text");
    assert!(side(&half, 72..128) <= 8, "the redacted half is black");

    // Files without boxes are untouched, and removing the boxes shows the
    // frame again.
    let (other_file, _, _) = thumbnail(&server, "/api/file/1/frame/0/thumbnail?size=128").await;
    assert!(brightest(&other_file, banner.clone()) > 128);
    put(no_boxes).await;
    let (shown, _, _) = thumbnail(&server, path).await;
    assert!(brightest(&shown, banner) > 128);
}

#[tokio::test]
async fn a_masked_session_withholds_the_thumbnails_of_label_images() {
    let paths = [BANNER_IMAGE, SLIDE_LABEL].map(|name| fixtures().join(name));
    let server = serve_paths(&paths, FileRegistry::masked(Arc::new(Masker::new()))).await;

    thumbnail(&server, "/api/file/0/frame/0/thumbnail").await;
    let display = server.get("/api/file/1/frame/0").await;
    let label = server.get("/api/file/1/frame/0/thumbnail").await;
    assert_eq!(label.status_code(), display.status_code());
    assert_eq!(label.status_code(), StatusCode::FORBIDDEN);
    assert_eq!(label.json::<Value>()["code"], "masked");
}

#[tokio::test]
async fn thumbnails_leave_the_viewer_caches_alone() {
    let names = [
        BANNER_IMAGE,
        "golden-jpeg-baseline-single-frame.dcm",
        "golden-jpeg2000-lossless-u8-single-frame.dcm",
        "golden-uncompressed-u16-multiframe.dcm",
    ];
    let server = serve(&names).await;

    for (index, name) in names.iter().enumerate() {
        thumbnail(&server, &format!("/api/file/{index}/frame/0/thumbnail")).await;
        // Raw first: a display frame fills the raw cache itself.
        for viewer_frame in ["frame/0/raw", "frame/0"] {
            let response = server
                .get(&format!("/api/file/{index}/{viewer_frame}"))
                .await;
            response.assert_status_ok();
            assert_eq!(
                header(&response, "x-cache"),
                "MISS",
                "{name}: a thumbnail filled the cache of {viewer_frame}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_concurrent_requests_render_one_thumbnail() {
    let server = serve(&[BANNER_IMAGE]).await;
    let path = "/api/file/0/frame/0/thumbnail?size=512";

    let responses =
        futures::future::join_all((0..8).map(|_| async { server.get(path).await })).await;
    let misses = responses
        .iter()
        .filter(|response| header(response, "x-cache") == "MISS")
        .count();
    assert_eq!(misses, 1, "one request renders, the others share it");
    for response in &responses {
        response.assert_status_ok();
        assert_eq!(response.as_bytes(), responses[0].as_bytes());
    }
}

#[tokio::test]
async fn thumbnail_requests_that_cannot_be_served_use_the_error_envelope() {
    let server = serve(&[BANNER_IMAGE, "golden-no-pixels-sr.dcm"]).await;

    // (path, status, code)
    let cases = [
        (
            "/api/file/9/frame/0/thumbnail",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "/api/file/0/frame/7/thumbnail",
            StatusCode::NOT_FOUND,
            "frame_out_of_range",
        ),
        (
            "/api/file/1/frame/0/thumbnail",
            StatusCode::NOT_FOUND,
            "no_pixel_data",
        ),
        (
            "/api/file/0/frame/0/thumbnail?size=0",
            StatusCode::BAD_REQUEST,
            "invalid_query",
        ),
        (
            "/api/file/0/frame/0/thumbnail?size=1025",
            StatusCode::BAD_REQUEST,
            "invalid_query",
        ),
        (
            "/api/file/0/frame/0/thumbnail?size=large",
            StatusCode::BAD_REQUEST,
            "invalid_query",
        ),
        (
            "/api/file/0/frame/0/thumbnail?window_mode=sharp",
            StatusCode::BAD_REQUEST,
            "invalid_query",
        ),
    ];
    for (path, status, code) in cases {
        let response = server.get(path).await;
        assert_eq!(response.status_code(), status, "{path}");
        assert_eq!(response.json::<Value>()["code"], code, "{path}");
    }
}
