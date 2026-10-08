//! A compressed frame is decoded only as the image its file's header
//! declares (`pixels::codestream`): honest files keep decoding, and a frame
//! whose own header says something else is a decode error at every endpoint
//! that decodes it, while the file stays listed and its other frames decode.

use super::codestream_files::{self as files, hex, Layout};
use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::pixels::codestream::CODESTREAM_MISMATCH;
use serde_json::Value;
use tempfile::tempdir;

struct Case {
    name: &'static str,
    transfer_syntax: &'static str,
    layout: Layout,
    frame: Vec<u8>,
}

fn case(name: &'static str, transfer_syntax: &'static str, layout: Layout, frame: Vec<u8>) -> Case {
    Case {
        name,
        transfer_syntax,
        layout,
        frame,
    }
}

/// Files that differ from their header only in ways their codec and the
/// standard allow, from encoders other than the ones the viewer links.
fn honest() -> Vec<Case> {
    let gray8 = Layout::gray8(16, 16);
    let gray12 = Layout::gray(16, 16, 16, 12);
    vec![
        case("j2k", files::JPEG_2000, gray8, hex(files::J2K_GRAY8)),
        case(
            "j2k, one resolution",
            files::JPEG_2000,
            gray8,
            hex(files::J2K_GRAY8_ONE_RESOLUTION),
        ),
        case(
            "j2k, four tiles",
            files::JPEG_2000,
            gray8,
            hex(files::J2K_GRAY8_TILED),
        ),
        case(
            "j2k, three layers",
            files::JPEG_2000,
            gray8,
            hex(files::J2K_GRAY8_LAYERS),
        ),
        case(
            "j2k, precincts",
            files::JPEG_2000,
            Layout::gray8(64, 64),
            hex(files::J2K_GRAY8_PRECINCTS),
        ),
        case(
            "j2k, tile-parts",
            files::JPEG_2000,
            Layout::gray8(64, 64),
            hex(files::J2K_GRAY8_TILE_PARTS),
        ),
        case(
            "j2k in a jp2 file",
            files::JPEG_2000,
            gray8,
            hex(files::J2K_GRAY8_JP2),
        ),
        case(
            "j2k, reversible colour transform",
            files::JPEG_2000,
            Layout::color8(16, 16, "YBR_RCT"),
            hex(files::J2K_RGB8_RCT),
        ),
        case(
            "j2k, no colour transform",
            files::JPEG_2000,
            Layout::color8(16, 16, "RGB"),
            hex(files::J2K_RGB8_NO_MCT),
        ),
        case(
            "j2k, 16 bits",
            files::JPEG_2000,
            Layout::gray(16, 16, 16, 16),
            hex(files::J2K_GRAY16),
        ),
        case(
            "j2k, 12 bits in 16",
            files::JPEG_2000,
            gray12,
            hex(files::J2K_GRAY12),
        ),
        case(
            "j2k, 16-bit codestream under 12 bits stored",
            files::JPEG_2000,
            gray12,
            hex(files::J2K_GRAY16),
        ),
        case(
            "j2k, 8-bit codestream under 16 bits allocated",
            files::JPEG_2000,
            Layout::gray(16, 16, 16, 8),
            hex(files::J2K_GRAY8),
        ),
        case(
            "j2k, odd size",
            files::JPEG_2000,
            Layout::gray8(9, 15),
            hex(files::J2K_GRAY8_ODD),
        ),
        case(
            "jpeg baseline",
            files::JPEG_BASELINE,
            gray8,
            support::grayscale_jpeg_fragment_16x16(7),
        ),
        case(
            "jpeg, restart markers",
            files::JPEG_BASELINE,
            gray8,
            hex(files::JPEG_GRAY8_RESTART),
        ),
        case(
            "jpeg, progressive",
            files::JPEG_BASELINE,
            gray8,
            hex(files::JPEG_GRAY8_PROGRESSIVE),
        ),
        case(
            "jpeg, progressive colour",
            files::JPEG_BASELINE,
            Layout::color8(16, 16, "YBR_FULL_422"),
            hex(files::JPEG_RGB8_PROGRESSIVE),
        ),
        case(
            "jpeg, subsampled chroma and odd size",
            files::JPEG_BASELINE,
            Layout::color8(9, 15, "YBR_FULL_422"),
            hex(files::JPEG_RGB8_420_ODD),
        ),
        case(
            "jpeg, profile and comment before the frame header",
            files::JPEG_BASELINE,
            gray8,
            hex(files::JPEG_GRAY8_WITH_PROFILE),
        ),
        case(
            "jpeg, as many segments before the frame header as are read",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_with_segments(&hex(files::JPEG_GRAY8_RESTART), 1000),
        ),
        case(
            "jpeg lossless",
            files::JPEG_LOSSLESS_SV1,
            gray8,
            hex(files::JPEG_LOSSLESS_GRAY8),
        ),
        case(
            "jpeg lossless, 16-bit codestream under 12 bits stored",
            files::JPEG_LOSSLESS_SV1,
            gray12,
            hex(files::JPEG_LOSSLESS_GRAY12),
        ),
        case(
            "jpeg lossless, point transform",
            files::JPEG_LOSSLESS,
            gray12,
            hex(files::JPEG_LOSSLESS_GRAY12_POINT_TRANSFORM),
        ),
        case(
            "jpeg lossless, selection value 7",
            files::JPEG_LOSSLESS,
            gray12,
            hex(files::JPEG_LOSSLESS_GRAY12_SV7),
        ),
        case("jpeg-ls", files::JPEG_LS, gray8, hex(files::JPEGLS_GRAY8)),
        case(
            "jpeg-ls, 16-bit codestream under 12 bits stored",
            files::JPEG_LS,
            gray12,
            hex(files::JPEGLS_GRAY12),
        ),
        case("jpeg xl", files::JPEG_XL, gray8, hex(files::JXL_GRAY8)),
        case(
            "jpeg xl, colour",
            files::JPEG_XL,
            Layout::color8(16, 16, "RGB"),
            hex(files::JXL_RGB8),
        ),
        case(
            "jpeg xl, 16 bits in the box container",
            files::JPEG_XL,
            Layout::gray(16, 16, 16, 16),
            hex(files::JXL_GRAY16),
        ),
        case(
            "jpeg xl in the box container",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_GRAY8_CONTAINER),
        ),
    ]
}

/// One frame of each compressed kind that declares another image than a
/// 16 x 16, 8-bit, single-sample header, and the honest frame it was made
/// from.
fn lying() -> Vec<(Case, Vec<u8>)> {
    let gray8 = Layout::gray8(16, 16);
    let jpeg = hex(files::JPEG_GRAY8_RESTART);
    let jpeg_ls = hex(files::JPEGLS_GRAY8);
    let j2k = hex(files::J2K_GRAY8);
    vec![
        (
            case(
                "jpeg",
                files::JPEG_BASELINE,
                gray8,
                files::jpeg_sized(&jpeg, 2048, 2048),
            ),
            jpeg,
        ),
        (
            case(
                "jpeg-ls",
                files::JPEG_LS,
                gray8,
                files::jpeg_sized(&jpeg_ls, 2048, 2048),
            ),
            jpeg_ls,
        ),
        (
            case(
                "jpeg 2000",
                files::JPEG_2000,
                gray8,
                files::j2k_sized(&j2k, 2048, 2048),
            ),
            j2k,
        ),
        (
            case(
                "jpeg xl",
                files::JPEG_XL,
                gray8,
                hex(files::JXL_GRAY8_2048X2048),
            ),
            hex(files::JXL_GRAY8),
        ),
    ]
}

fn assert_refused(context: &str, response: &axum_test::TestResponse) {
    assert_refused_as(
        context,
        response,
        StatusCode::INTERNAL_SERVER_ERROR,
        "pixel_decode_failed",
    );
}

fn assert_refused_as(
    context: &str,
    response: &axum_test::TestResponse,
    status: StatusCode,
    code: &str,
) {
    assert_eq!(
        response.status_code(),
        status,
        "{context}: {}",
        response.text()
    );
    let body: Value = response.json();
    assert_eq!(body["code"], code, "{context}");
    let message = body["error"].as_str().expect("an error message");
    assert!(
        message.contains(CODESTREAM_MISMATCH),
        "{context}: the message says the pixel data disagrees with the header: {message}"
    );
}

async fn png_size(server: &TestServer, path: &str, context: &str) -> (u32, u32) {
    let response = server.get(path).await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "{context}: {}",
        response.text()
    );
    image::load_from_memory(response.as_bytes().as_ref())
        .unwrap_or_else(|error| panic!("{context}: a PNG: {error}"))
        .to_rgb8()
        .dimensions()
}

#[tokio::test]
async fn honest_compressed_frames_decode_as_their_header_describes_them() {
    let dir = tempdir().expect("temp dir");
    for (number, case) in honest().into_iter().enumerate() {
        let path = dir.path().join(format!("honest-{number}.dcm"));
        files::write(&path, case.transfer_syntax, case.layout, &[case.frame]);
        let server = files::serve(files::list(&[path]).await);
        let size = png_size(&server, "/api/file/0/frame/0", case.name).await;
        assert_eq!(
            size,
            (u32::from(case.layout.columns), u32::from(case.layout.rows)),
            "{}",
            case.name
        );
        if case.layout.samples == 1 {
            let raw = server.get("/api/file/0/frame/0/raw").await;
            assert_eq!(
                raw.status_code(),
                StatusCode::OK,
                "{}: {}",
                case.name,
                raw.text()
            );
        }
    }
}

#[tokio::test]
async fn a_frame_that_declares_another_image_is_refused_wherever_it_is_decoded() {
    let dir = tempdir().expect("temp dir");
    for (number, (case, _)) in lying().into_iter().enumerate() {
        let path = dir.path().join(format!("lying-{number}.dcm"));
        files::write(&path, case.transfer_syntax, case.layout, &[case.frame]);
        let server = files::serve(files::list(&[path]).await);
        for endpoint in [
            "/api/file/0/frame/0",
            "/api/file/0/frame/0?window_mode=full_dynamic",
            "/api/file/0/frame/0/raw",
            "/api/file/0/frame/0/raw/pixel?row=0&column=0",
        ] {
            let context = format!("{} at {endpoint}", case.name);
            assert_refused(&context, &server.get(endpoint).await);
            // A refusal is not cached as a frame.
            assert_refused(&context, &server.get(endpoint).await);
        }
        // The file stays listed, described by its header.
        let listed: Value = server.get("/api/files").await.json();
        assert_eq!(
            listed["files"].as_array().map(Vec::len),
            Some(1),
            "{}",
            case.name
        );
        let info: Value = server.get("/api/file/0/info").await.json();
        assert_eq!(info["rows"], 16, "{}", case.name);
        assert_eq!(info["columns"], 16, "{}", case.name);
        // Nothing is decoded for the frame's own graphics layer.
        server
            .get("/api/file/0/frame/0/presentation-layer")
            .await
            .assert_status_ok();
    }
}

#[tokio::test]
async fn one_lying_frame_does_not_take_its_file_s_other_frames_with_it() {
    let dir = tempdir().expect("temp dir");
    for (number, (case, honest)) in lying().into_iter().enumerate() {
        let path = dir.path().join(format!("mixed-{number}.dcm"));
        files::write(
            &path,
            case.transfer_syntax,
            case.layout,
            &[honest.clone(), case.frame, honest],
        );
        let server = files::serve(files::list(&[path]).await);
        for frame in [0, 2] {
            let context = format!("{} frame {frame}", case.name);
            let display = format!("/api/file/0/frame/{frame}");
            assert_eq!(png_size(&server, &display, &context).await, (16, 16));
            server
                .get(&format!("{display}/raw"))
                .await
                .assert_status_ok();
        }
        for endpoint in ["/api/file/0/frame/1", "/api/file/0/frame/1/raw"] {
            assert_refused(
                &format!("{} at {endpoint}", case.name),
                &server.get(endpoint).await,
            );
        }
    }
}

/// A Deflated Image Frame has no header of its own: its frame must inflate
/// to exactly what the entry gives.
#[tokio::test]
async fn a_deflated_frame_is_inflated_to_its_header_s_size_and_no_further() {
    let dir = tempdir().expect("temp dir");
    let layout = Layout::gray(16, 16, 1, 1);
    for (name, length, decodes) in [
        ("the frame's 32 bytes", 32, true),
        ("a byte short", 31, false),
        ("a byte over", 33, false),
        ("64 MiB", 64 * 1024 * 1024, false),
    ] {
        let path = dir.path().join("deflated.dcm");
        files::write(
            &path,
            files::DEFLATED_FRAME,
            layout,
            &[files::deflated_zeros(length)],
        );
        let server = files::serve(files::list(&[path]).await);
        for endpoint in ["/api/file/0/frame/0", "/api/file/0/frame/0/raw"] {
            let response = server.get(endpoint).await;
            if decodes {
                assert_eq!(response.status_code(), StatusCode::OK, "{name}");
            } else {
                assert_refused(&format!("{name} at {endpoint}"), &response);
            }
        }
    }
}

/// Overlays decode another file's frames, and discovery decodes a
/// fractional segmentation's frames to learn whether it is binary; both go
/// through the same check.
#[tokio::test]
async fn overlay_objects_with_lying_frames_are_listed_and_refused() {
    let dir = tempdir().expect("temp dir");
    let lying_ls = files::jpeg_sized(&hex(files::JPEGLS_GRAY8), 2048, 2048);
    let lying_j2k = files::j2k_sized(&hex(files::J2K_GRAY16), 2048, 2048);

    let segmentation = dir.path().join("seg.dcm");
    files::rewrite(
        &files::fixture("golden-seg-binary-valued-fractional.dcm"),
        &segmentation,
        files::JPEG_LS,
        &[lying_ls.clone(), lying_ls],
    );
    let listed = files::list(&[
        segmentation,
        files::fixture("golden-seg-binary-valued-fractional-source.dcm"),
    ])
    .await;
    let server = files::serve(listed);
    assert_refused(
        "segmentation overlay",
        &server.get("/api/file/0/frame/0/segmentation-overlay").await,
    );

    let map = dir.path().join("map.dcm");
    files::rewrite(
        &files::fixture("golden-parametric-map-u16-linear.dcm"),
        &map,
        files::JPEG_2000,
        &[lying_j2k.clone(), lying_j2k],
    );
    let listed = files::list(&[
        map,
        files::fixture("golden-parametric-map-mr-source-z0.dcm"),
        files::fixture("golden-parametric-map-mr-source-z1.dcm"),
    ])
    .await;
    let server = files::serve(listed);
    for endpoint in [
        "/api/file/2/frame/0/parametric-map-overlay?map=0",
        "/api/file/2/frame/0/parametric-map-overlay/values?map=0",
    ] {
        // A value overlay reports an undecodable map as its own error.
        assert_refused_as(
            endpoint,
            &server.get(endpoint).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "semantic_mapping_unavailable",
        );
    }
    // The map's legend is computed from its frames; without them the
    // context is still served.
    assert_ne!(
        server
            .get("/api/file/0/semantic-context")
            .await
            .status_code(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
