use crate::api::contracts::{ApiErrorCode, ErrorResponse};
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

pub(crate) async fn index() -> impl IntoResponse {
    serve_asset("index.html").unwrap_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: ApiErrorCode::AssetNotFound,
                error: "frontend index asset missing".to_string(),
            }),
        )
            .into_response()
    })
}

pub(crate) async fn asset(Path(path): Path<String>) -> impl IntoResponse {
    let full_path = format!("assets/{}", path.trim_start_matches('/'));
    serve_asset(&full_path).unwrap_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: ApiErrorCode::AssetNotFound,
                error: format!("asset not found: {path}"),
            }),
        )
            .into_response()
    })
}

fn serve_asset(path: &str) -> Option<Response> {
    let normalized = path.trim_start_matches('/');
    let asset = FrontendAssets::get(normalized)?;
    let mime = match normalized.rsplit('.').next().unwrap_or_default() {
        "js" => "text/javascript",
        "css" => "text/css",
        "html" => "text/html",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };

    let mut response = Response::new(axum::body::Body::from(asset.data));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    // A content-hashed file never changes under its name; everything else
    // (index.html, the icon, licence texts) is revalidated.
    let cache_control = if is_content_hashed(normalized) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    Some(response)
}

/// Whether Vite named the file after its content: `name-HASH.ext`, where the
/// hash is the stem's last eight characters (letters, digits, `-` or `_`).
fn is_content_hashed(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let Some((stem, _extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    let bytes = stem.as_bytes();
    bytes.len() > 9
        && bytes[bytes.len() - 9] == b'-'
        && bytes[bytes.len() - 8..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::is_content_hashed;

    #[test]
    fn only_vite_hashed_names_are_immutable() {
        assert!(is_content_hashed("assets/index-DJ9oNGep.js"));
        assert!(is_content_hashed(
            "assets/inter-greek-wght-normal-CkhJZR-_.woff2"
        ));
        assert!(!is_content_hashed("assets/dcmview-icon.png"));
        assert!(!is_content_hashed("index.html"));
        assert!(!is_content_hashed("assets/licenses/Inter-OFL.txt"));
    }
}
