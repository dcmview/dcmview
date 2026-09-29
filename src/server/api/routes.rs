use super::error;
use super::handlers;
use super::overlays;
use super::state::AppState;
use crate::api::contracts::{endpoints, API_PREFIX, SERVER_INSTANCE_HEADER};
use crate::server::web;
use crate::server::RequestActivity;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::{CompressionLayer, CompressionLevel};
#[cfg(feature = "debug-api")]
use tower_http::cors::CorsLayer;

pub(crate) fn router(state: AppState) -> Router {
    let activity = state.activity().clone();
    let instance = HeaderValue::from_str(&state.server_start_ms().to_string())
        .expect("integer server identity");
    // Methods here must match `endpoints::ALL`; tests/integration/api_contract.rs
    // requests every declared endpoint with its declared method.
    let api = Router::new()
        .route(endpoints::HEALTH.path, get(handlers::health))
        .route(endpoints::FILES.path, get(handlers::files))
        .route(endpoints::SERIES.path, get(handlers::series))
        .route(endpoints::FILE_INFO.path, get(handlers::info))
        .route(endpoints::FILE_REFERENCES.path, get(handlers::references))
        .route(
            endpoints::FILE_SEMANTIC_CONTEXT.path,
            get(handlers::semantic_context),
        )
        .route(
            endpoints::FILE_SEGMENTATION_OVERLAY.path,
            get(overlays::segmentation_overlay),
        )
        .route(
            endpoints::FILE_PRESENTATION_LAYER.path,
            get(overlays::presentation_layer),
        )
        .route(
            endpoints::FILE_DOSE_OVERLAY.path,
            get(overlays::dose_overlay),
        )
        .route(
            endpoints::FILE_DOSE_OVERLAY_VALUES.path,
            get(overlays::dose_overlay_values),
        )
        .route(
            endpoints::FILE_PARAMETRIC_MAP_OVERLAY.path,
            get(overlays::parametric_map_overlay),
        )
        .route(
            endpoints::FILE_PARAMETRIC_MAP_OVERLAY_VALUES.path,
            get(overlays::parametric_map_overlay_values),
        )
        .route(
            endpoints::FILE_VALUE_MAPPING.path,
            get(handlers::value_mapping),
        )
        .route(endpoints::FILE_WSI_CONTEXT.path, get(handlers::wsi_context))
        .route(endpoints::FILE_FRAME.path, get(handlers::frame))
        .route(endpoints::FILE_RAW_FRAME.path, get(handlers::raw_frame))
        .route(endpoints::FILE_RAW_PIXEL.path, get(handlers::raw_pixel))
        .route(endpoints::FILE_TAGS.path, get(handlers::tags))
        .route(endpoints::FILE_TAG_SELECT.path, get(handlers::select_tag))
        .route(
            endpoints::FILE_ANNOTATIONS_GET.path,
            get(handlers::annotations).put(handlers::update_annotations),
        )
        .route(
            endpoints::ANNOTATIONS_EXPORT.path,
            get(handlers::export_annotations),
        )
        .fallback(error::not_found_handler)
        .method_not_allowed_fallback(error::method_not_allowed_handler);

    let router = Router::new()
        .route("/", get(web::index))
        .route("/assets/{*path}", get(web::asset))
        .nest(API_PREFIX, api)
        .fallback(error::page_not_found_handler)
        .method_not_allowed_fallback(error::method_not_allowed_handler)
        .layer(middleware::from_fn_with_state(
            activity,
            track_request_activity,
        ))
        .layer(middleware::from_fn_with_state(instance, identify_server))
        .layer(compression())
        .with_state(state);

    #[cfg(feature = "debug-api")]
    let router = router.layer(CorsLayer::permissive());

    router
}

/// Gzip for JSON, CSV and the viewer's scripts and styles: the catalog of a
/// few thousand files is megabytes of JSON polled during a scan, often over
/// an SSH tunnel. PNG frames (skipped by the default predicate), raw frame
/// samples and fonts are already compressed or too costly to compress per
/// request.
fn compression() -> CompressionLayer<impl Predicate> {
    CompressionLayer::new()
        .gzip(true)
        .quality(CompressionLevel::Fastest)
        .compress_when(
            DefaultPredicate::new()
                .and(NotForContentType::const_new("application/octet-stream"))
                .and(NotForContentType::const_new("font/")),
        )
}

async fn track_request_activity(
    State(activity): State<RequestActivity>,
    request: Request,
    next: Next,
) -> Response {
    let _request = activity.request_started();
    let (method, uri) = (request.method().clone(), request.uri().clone());
    let response = next.run(request).await;
    if let Some(error::ServerErrorMessage(message)) = response.extensions().get() {
        tracing::warn!(%method, %uri, status = response.status().as_u16(), "{message}");
    }
    response
}

/// The outer layer also covers API fallbacks and extractor/method errors.
async fn identify_server(
    State(instance): State<HeaderValue>,
    request: Request,
    next: Next,
) -> Response {
    let is_api = request.uri().path() == API_PREFIX || request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if is_api {
        response.headers_mut().insert(
            HeaderName::from_bytes(SERVER_INSTANCE_HEADER.as_bytes()).expect("static header name"),
            instance,
        );
    }
    response
}
