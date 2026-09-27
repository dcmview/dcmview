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
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    Some(response)
}
