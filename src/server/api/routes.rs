use super::error;
use super::handlers;
use super::state::AppState;
use crate::api::contracts::{endpoints, API_PREFIX};
use crate::server::web;
use crate::server::RequestActivity;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
#[cfg(feature = "debug-api")]
use tower_http::cors::CorsLayer;

pub(crate) fn router(state: AppState) -> Router {
    let activity = state.activity().clone();
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
            get(handlers::segmentation_overlay),
        )
        .route(endpoints::FILE_WSI_CONTEXT.path, get(handlers::wsi_context))
        .route(endpoints::FILE_FRAME.path, get(handlers::frame))
        .route(endpoints::FILE_RAW_FRAME.path, get(handlers::raw_frame))
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
        .layer(middleware::from_fn_with_state(
            activity,
            track_request_activity,
        ))
        .with_state(state);

    #[cfg(feature = "debug-api")]
    let router = router.layer(CorsLayer::permissive());

    router
}

async fn track_request_activity(
    State(activity): State<RequestActivity>,
    request: Request,
    next: Next,
) -> Response {
    let _request = activity.request_started();
    next.run(request).await
}
