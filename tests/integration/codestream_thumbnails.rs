//! The gallery thumbnail decodes a frame through its codec's display
//! renderer, so it refuses a frame that declares another image than its
//! header as the display endpoint does.

use super::codestream_files::{self as files, hex, Layout};
use axum::http::StatusCode;
use dcmview::pixels::codestream::CODESTREAM_MISMATCH;
use serde_json::Value;
use tempfile::tempdir;

#[tokio::test]
async fn a_thumbnail_is_not_rendered_from_a_frame_that_declares_another_image() {
    let dir = tempdir().expect("temp dir");
    let gray8 = Layout::gray8(16, 16);
    let cases = [
        (
            "jpeg",
            files::JPEG_BASELINE,
            hex(files::JPEG_GRAY8_RESTART),
            files::jpeg_sized(&hex(files::JPEG_GRAY8_RESTART), 2048, 2048),
        ),
        (
            "jpeg-ls",
            files::JPEG_LS,
            hex(files::JPEGLS_GRAY8),
            files::jpeg_sized(&hex(files::JPEGLS_GRAY8), 2048, 2048),
        ),
        (
            "jpeg 2000",
            files::JPEG_2000,
            hex(files::J2K_GRAY8),
            files::j2k_sized(&hex(files::J2K_GRAY8), 2048, 2048),
        ),
        (
            "jpeg xl",
            files::JPEG_XL,
            hex(files::JXL_GRAY8),
            hex(files::JXL_GRAY8_2048X2048),
        ),
    ];
    for (number, (name, transfer_syntax, honest, lying)) in cases.into_iter().enumerate() {
        let path = dir.path().join(format!("thumbnail-{number}.dcm"));
        files::write(&path, transfer_syntax, gray8, &[honest, lying]);
        let server = files::serve(files::list(&[path]).await);
        server
            .get("/api/file/0/frame/0/thumbnail?size=128")
            .await
            .assert_status_ok();
        let refused = server.get("/api/file/0/frame/1/thumbnail?size=128").await;
        assert_eq!(
            refused.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{name}: {}",
            refused.text()
        );
        let body: Value = refused.json();
        assert_eq!(body["code"], "pixel_decode_failed", "{name}");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|message| message.contains(CODESTREAM_MISMATCH)),
            "{name}: {body}"
        );
    }
}
