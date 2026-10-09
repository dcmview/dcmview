use super::auth;
use super::error;
use super::handlers;
use super::overlays;
use super::state::AppState;
use crate::api::contracts::{endpoints, API_PREFIX, SERVER_INSTANCE_HEADER};
use crate::server::web;
use crate::server::RequestActivity;
use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{any, get, put};
use axum::Router;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::{CompressionLayer, CompressionLevel};
#[cfg(feature = "debug-api")]
use tower_http::cors::CorsLayer;

pub(crate) fn router(state: AppState) -> Router {
    let request_log = RequestLog {
        activity: state.activity().clone(),
        masked: state.registry().masker().is_some(),
    };
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
            endpoints::FILE_GRAPHIC_ANNOTATIONS.path,
            get(handlers::graphic_annotations),
        )
        .route(
            endpoints::FILE_VALUE_MAPPING.path,
            get(handlers::value_mapping),
        )
        .route(endpoints::FILE_WSI_CONTEXT.path, get(handlers::wsi_context))
        .route(endpoints::FILE_FRAME.path, get(handlers::frame))
        .route(endpoints::FILE_RAW_FRAME.path, get(handlers::raw_frame))
        .route(endpoints::FILE_RAW_PIXEL.path, get(handlers::raw_pixel))
        .route(endpoints::FILE_THUMBNAIL.path, get(handlers::thumbnail))
        .route(endpoints::FILE_TAGS.path, get(handlers::tags))
        .route(endpoints::FILE_TAG_SELECT.path, get(handlers::select_tag))
        .route(
            endpoints::FILE_ANNOTATIONS_GET.path,
            get(handlers::annotations).put(handlers::update_annotations),
        )
        .route(
            endpoints::FILE_REDACTIONS_GET.path,
            get(handlers::redactions).put(handlers::update_redactions),
        )
        .route(
            endpoints::FILE_REDACTIONS_APPLY_TO_SERIES.path,
            put(handlers::apply_redactions_to_series),
        )
        .route(
            endpoints::ANNOTATIONS_EXPORT.path,
            get(handlers::export_annotations),
        )
        .fallback(error::not_found_handler)
        .method_not_allowed_fallback(error::method_not_allowed_handler);
    // The token check wraps the whole API router, fallbacks included, so an
    // unauthenticated caller cannot tell a declared route from a missing one.
    let guard = |router: Router<AppState>| match state.access_token() {
        Some(token) => router.layer(middleware::from_fn_with_state(
            token.clone(),
            auth::require_bearer,
        )),
        None => router,
    };
    let api = guard(api);
    // `nest` does not match the prefix with a trailing slash, which would
    // otherwise fall through to the public page fallback.
    let api_root =
        guard(Router::new().route(&format!("{API_PREFIX}/"), any(error::not_found_handler)));

    let router = Router::new()
        .route("/", get(web::index))
        .route("/assets/{*path}", get(web::asset))
        .nest(API_PREFIX, api)
        .merge(api_root)
        .fallback(error::page_not_found_handler)
        .method_not_allowed_fallback(error::method_not_allowed_handler)
        .layer(middleware::from_fn_with_state(
            request_log,
            track_request_activity,
        ))
        .layer(middleware::from_fn_with_state(instance, identify_server))
        .layer(middleware::map_response(forbid_mime_sniffing))
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

/// What the request logger needs of the viewer.
#[derive(Clone)]
struct RequestLog {
    activity: RequestActivity,
    /// A `--mask` session logs nothing a file holds.
    masked: bool,
}

async fn track_request_activity(
    State(RequestLog { activity, masked }): State<RequestLog>,
    request: Request,
    next: Next,
) -> Response {
    let _request = if request
        .headers()
        .get(crate::api::contracts::BACKGROUND_REQUEST_HEADER)
        .is_some_and(|value| value == "1")
    {
        activity.background_request_started()
    } else {
        activity.request_started()
    };
    let (method, uri) = (request.method().clone(), request.uri().clone());
    let response = next.run(request).await;
    if let Some(error::ServerErrorMessage(message)) = response.extensions().get() {
        tracing::warn!(%method, %uri, status = response.status().as_u16(), "{message}");
    }
    // The cause is a library's or a parser's text and may quote the file:
    // debug level, escaped, and never in a masked session.
    if let (false, Some(error::ServerErrorDetail(detail))) = (masked, response.extensions().get()) {
        tracing::debug!(%method, %uri, "cause: {}", detail.escape_debug());
    }
    response
}

/// Browsers must use each response's declared type: raw frames return DICOM
/// sample bytes verbatim, and tag JSON and the annotation CSV quote values
/// from files, so none of them may be sniffed into a renderable document.
async fn forbid_mime_sniffing(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
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
