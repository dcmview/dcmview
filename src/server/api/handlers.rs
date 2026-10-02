use super::error::{self, ApiError};
use super::overlays;
use super::state::AppState;
use crate::api::contracts::{
    DiscoveryResult, EmbedRoiAnnotations, FileSummary, FilesResponse, FrameInfo, FrameQuery,
    GraphicAnnotationsQuery, GraphicAnnotationsResponse, HealthResponse, PixelQuery,
    ReferenceCatalogResponse, SemanticContextResponse, TagNode, TagQuery, ViewerIdentity,
    CACHE_HEADER, CACHE_HIT, CACHE_MISS, CSV_MEDIA_TYPE, DISPLAY_FRAME_HEADER_WINDOW_APPLIED,
    DISPLAY_FRAME_HEADER_WINDOW_CENTER, DISPLAY_FRAME_HEADER_WINDOW_WIDTH,
    EXPORT_CONTENT_DISPOSITION_HEADER, EXPORT_CONTENT_DISPOSITION_VALUE, OCTET_STREAM_MEDIA_TYPE,
    RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_COLUMNS, RAW_FRAME_HEADER_DEFAULT_WC,
    RAW_FRAME_HEADER_DEFAULT_WW, RAW_FRAME_HEADER_PADDING_HIGH, RAW_FRAME_HEADER_PADDING_LOW,
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, RAW_FRAME_HEADER_PIXEL_REPRESENTATION,
    RAW_FRAME_HEADER_RESCALE_INTERCEPT, RAW_FRAME_HEADER_RESCALE_SLOPE, RAW_FRAME_HEADER_ROWS,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
};
use crate::pixels::{self, FrameRequest, RawFrameRequest};
use crate::references::{self, ReferenceCandidate};
use crate::server::tags;
use crate::types::{FileEntry, WindowMode, WindowRequest};
use crate::value_mapping::FileValueMappings;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;
use tokio::task;

pub(super) async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let status = state.registry().status();
    Json(HealthResponse {
        status: "ok",
        viewer: ViewerIdentity::current(),
        file_count: status.file_count,
        server_start_ms: state.server_start_ms(),
        masked: state.registry().masker().is_some(),
    })
}

/// A JSON response whose UIDs are hashed in a masked session.
fn uid_masked_json<T: serde::Serialize>(state: &AppState, body: T) -> Result<Response, ApiError> {
    let Some(masker) = state.registry().masker() else {
        return Ok(Json(body).into_response());
    };
    let mut value = serde_json::to_value(body)
        .map_err(|error| ApiError::internal(format!("response serialization failed: {error}")))?;
    masker.uids_in_json(&mut value);
    Ok(Json(value).into_response())
}

/// Refuses the frames a masked session withholds.
fn ensure_pixels_shown(state: &AppState, file: &FileEntry) -> Result<(), ApiError> {
    if state.registry().masker().is_some() && crate::masking::hides_pixels(file) {
        return Err(ApiError::masked(
            "slide label and overview images are not shown in a masked session",
        ));
    }
    Ok(())
}

pub(super) async fn files(State(state): State<AppState>) -> Json<FilesResponse> {
    let status = state.registry().status();
    Json(FilesResponse {
        files: state.registry().summaries_snapshot(),
        discovery: state
            .registry()
            .discovery_response_snapshot()
            .into_iter()
            .map(|record| DiscoveryResult {
                path: record.path.display().to_string(),
                disposition: match record.disposition {
                    crate::loader::DiscoveryDisposition::Selected => "selected",
                    crate::loader::DiscoveryDisposition::Skipped => "skipped",
                    crate::loader::DiscoveryDisposition::Filtered => "filtered",
                }
                .to_string(),
                reason: record.reason.code().to_string(),
            })
            .collect(),
        server_start_ms: state.server_start_ms(),
        scan_complete: status.scan_complete,
        scanned: status.scanned,
        skipped: status.skipped,
        filtered: status.filtered,
    })
}

/// The registered file at `index`, or a 404 naming the index and how many
/// files are loaded. `role` names what the index selects.
pub(super) fn registered_file(
    state: &AppState,
    index: usize,
    role: &str,
) -> Result<Arc<FileEntry>, ApiError> {
    let registry = state.registry();
    registry.get(index).ok_or_else(|| {
        let count = registry.status().file_count;
        ApiError::not_found(format!(
            "{role} index {index} is out of range: {count} file(s) are loaded"
        ))
    })
}

pub(super) async fn series(State(state): State<AppState>) -> Result<Response, ApiError> {
    let json = state.registry().series_catalog_json().map_err(|error| {
        ApiError::internal(format!("series catalog serialization failed: {error}"))
    })?;
    Ok(([(header::CONTENT_TYPE, "application/json")], json).into_response())
}

pub(super) async fn info(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Json<FrameInfo>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let file = registered_file(&state, index, "file")?;
    let summary = FileSummary::from(&*file);
    Ok(Json(FrameInfo {
        frame_count: file.frame_count,
        rows: file.rows,
        columns: file.columns,
        transfer_syntax_uid: file.transfer_syntax_uid.clone(),
        has_pixels: file.has_pixels,
        sop_class_uid: summary.sop_class_uid,
        object_kind: summary.object_kind,
        support_state: summary.support_state,
        support_reason: summary.support_reason,
        default_window: file.default_window,
    }))
}

pub(super) async fn references(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let source = registered_file(&state, index, "file")?;
    let source_path = source.path.clone();
    let edges = task::spawn_blocking(move || references::extract_reference_edges(&source_path))
        .await
        .map_err(|error| ApiError::internal(format!("reference extraction task failed: {error}")))?
        .map_err(|error| error::gone_or(&source.path, ApiError::internal(error.to_string())))?;
    let candidates = state
        .registry()
        .files_snapshot()
        .into_iter()
        .map(|file| ReferenceCandidate::from_file(&file))
        .collect::<Vec<_>>();
    let resolved = references::resolve_reference_edges(&edges, &candidates)
        .iter()
        .map(references::ResolvedReferenceEdge::summary)
        .collect();
    uid_masked_json(
        &state,
        ReferenceCatalogResponse {
            source_file_index: index,
            source_sop_instance_uid: source.sop_instance_uid.clone(),
            references: resolved,
        },
    )
}

pub(super) async fn semantic_context(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let source = registered_file(&state, index, "file")?;
    let files = state.registry().files_snapshot();
    let path = source.path.clone();
    let context = semantic_context_for(&state, source.clone(), files)
        .await
        .map_err(|failure| error::gone_or(&path, ApiError::internal(format!("{failure:#}"))))?;
    let mut context = SemanticContextResponse::clone(&context);
    if let Some(masker) = state.registry().masker() {
        masker.semantic_context(&source, &mut context);
    }
    uid_masked_json(&state, context)
}

/// The source's semantic context against `files`, built at most once per
/// file set and kept in a small LRU.
pub(super) async fn semantic_context_for(
    state: &AppState,
    source: Arc<FileEntry>,
    files: Vec<Arc<FileEntry>>,
) -> anyhow::Result<Arc<SemanticContextResponse>> {
    let key = (source.index, files.len());
    if let Some(context) = state.cached_semantic_context(key) {
        return Ok(context);
    }
    let object = source.clone();
    let mut context =
        task::spawn_blocking(move || crate::semantic::semantic_context(&object, &files))
            .await
            .map_err(|error| anyhow::anyhow!("semantic context task failed: {error}"))??;
    overlays::add_overlay_legend(state, &source, &mut context).await;
    let context = Arc::new(context);
    state.cache_semantic_context(key, context.clone());
    Ok(context)
}

/// The annotations a softcopy presentation state draws on one image frame.
pub(super) async fn graphic_annotations(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<GraphicAnnotationsQuery>, QueryRejection>,
) -> Result<Json<GraphicAnnotationsResponse>, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let target = registered_file(&state, index, "file")?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let presentation_state = registered_file(&state, query.state, "presentation state file")?;
    if !crate::presentation_state::has_graphic_annotations(&presentation_state.sop_class_uid) {
        return Err(ApiError::bad_request(
            "state must select a Grayscale or Color Softcopy Presentation State",
        ));
    }
    crate::pixels::PixelError::ensure_frame(frame, target.frame_count)
        .map_err(error::pixel_error)?;
    let path = presentation_state.path.clone();
    let mut annotations = task::spawn_blocking(move || {
        crate::presentation_state::graphic_annotations(&presentation_state, &target, frame)
    })
    .await
    .map_err(|error| ApiError::internal(format!("graphic annotation task failed: {error}")))?
    .map_err(|failure| error::gone_or(&path, ApiError::internal(format!("{failure:#}"))))?;
    if let Some(masker) = state.registry().masker() {
        masker.graphic_annotations(&mut annotations);
    }
    Ok(Json(annotations))
}

pub(super) async fn value_mapping(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let file = registered_file(&state, index, "file")?;
    crate::pixels::PixelError::ensure_frame(frame, file.frame_count).map_err(error::pixel_error)?;
    let path = file.path.clone();
    let mappings = value_mappings_for(&state, file)
        .await
        .map_err(|failure| error::gone_or(&path, ApiError::internal(format!("{failure:#}"))))?;
    uid_masked_json(&state, mappings.frame(index, frame))
}

/// The file's parsed value mappings, with those of the RWVM instances that
/// reference it, read at most once per file set while cached.
pub(super) async fn value_mappings_for(
    state: &AppState,
    file: Arc<FileEntry>,
) -> anyhow::Result<Arc<FileValueMappings>> {
    let files = state.registry().files_snapshot();
    let key = (file.index, files.len());
    if let Some(mappings) = state.cached_value_mappings(key) {
        return Ok(mappings);
    }
    let mappings = task::spawn_blocking(move || FileValueMappings::read(&file, &files))
        .await
        .map_err(|error| anyhow::anyhow!("value mapping task failed: {error}"))??;
    let mappings = Arc::new(mappings);
    state.cache_value_mappings(key, mappings.clone());
    Ok(mappings)
}

pub(super) async fn wsi_context(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let source = registered_file(&state, index, "file")?;
    crate::pixels::PixelError::ensure_frame(frame, source.frame_count)
        .map_err(error::pixel_error)?;
    if crate::object_kind::classify_sop_class(&source.sop_class_uid)
        != crate::object_kind::ObjectKind::WholeSlideMicroscopy
    {
        return Err(ApiError::bad_request(
            "WSI context is only available for Whole Slide Microscopy objects",
        ));
    }
    let files = state.registry().files_snapshot();
    let context = task::spawn_blocking(move || crate::wsi::frame_context(&source, frame, &files))
        .await
        .map_err(|error| ApiError::internal(format!("WSI context task failed: {error}")))?
        .map_err(|error| ApiError::internal(error.to_string()))?;
    uid_masked_json(&state, context)
}

pub(super) async fn annotations(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Json<EmbedRoiAnnotations>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    registered_file(&state, index, "file")?;

    state
        .annotations()
        .wait_until_ready()
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let annotations = state
        .annotations()
        .get(index)
        .map_err(|error| ApiError::internal(error.to_string()))?;

    Ok(Json(annotations))
}

pub(super) async fn update_annotations(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
    payload: Result<Json<EmbedRoiAnnotations>, JsonRejection>,
) -> Result<Json<EmbedRoiAnnotations>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let Json(annotations) = payload.map_err(error::json_rejection)?;
    let file = registered_file(&state, index, "file")?;

    let canonical = state
        .annotations()
        .replace_for_file(&file, annotations)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;

    Ok(Json(canonical))
}

pub(super) async fn export_annotations(
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    state
        .annotations()
        .wait_until_ready()
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let files = state.registry().files_snapshot();
    let csv = state
        .annotations()
        .export_embed_csv(files.as_slice())
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut response = Response::new(axum::body::Body::from(csv));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(CSV_MEDIA_TYPE),
    );
    headers.insert(
        EXPORT_CONTENT_DISPOSITION_HEADER,
        HeaderValue::from_static(EXPORT_CONTENT_DISPOSITION_VALUE),
    );
    Ok(response)
}

pub(super) async fn frame(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<FrameQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let file = registered_file(&state, index, "file")?;
    ensure_pixels_shown(&state, &file)?;
    let source = file.path.clone();
    let window_mode = query.mode.unwrap_or_default();

    // A window in a real-world unit applies to the frame's preferred mapping
    // (the one the viewer's raw renderer windows) when it has that unit;
    // otherwise the frame shows its default window.
    let mut window = (query.wc, query.ww, None);
    if let (Some(unit), WindowMode::Default) = (query.unit, window_mode) {
        let invalid =
            |message: String| error::pixel_error(pixels::PixelError::InvalidWindow(message));
        if query.wc.is_none() || query.ww.is_none() {
            return Err(invalid("a window unit requires wc and ww".to_string()));
        }
        WindowRequest::new(query.wc, query.ww, window_mode)
            .map_err(|failure| invalid(failure.to_string()))?;
        let mappings = value_mappings_for(&state, file.clone())
            .await
            .map_err(|failure| {
                error::gone_or(&source, ApiError::internal(format!("{failure:#}")))
            })?;
        let preferred = mappings.real_world(frame).next();
        window = match preferred.filter(|map| map.unit_label == unit) {
            Some(map) => (query.wc, query.ww, Some(map.clone())),
            None => (None, None, None),
        };
    }
    let (window_center, window_width, real_world) = window;

    let frame_response = pixels::load_frame(
        file,
        state.pixel_cache(),
        state.raw_cache(),
        FrameRequest {
            frame,
            window_center,
            window_width,
            window_mode,
            real_world,
            preview: query.preview.unwrap_or(false),
        },
    )
    .await
    .map_err(|failure| error::gone_or(&source, error::pixel_error(failure)))?;

    let mut response = Response::new(axum::body::Body::from(frame_response.body));
    let cache_header = if frame_response.cache_hit {
        CACHE_HIT
    } else {
        CACHE_MISS
    };
    response
        .headers_mut()
        .insert(CACHE_HEADER, HeaderValue::from_static(cache_header));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(frame_response.content_type),
    );
    if let Some(kind) = frame_response.window.kind() {
        response.headers_mut().insert(
            DISPLAY_FRAME_HEADER_WINDOW_APPLIED,
            HeaderValue::from_static(kind.as_str()),
        );
    }
    if let pixels::AppliedWindow::Linear(window) = frame_response.window {
        let headers = response.headers_mut();
        insert_header_if_valid(
            headers,
            DISPLAY_FRAME_HEADER_WINDOW_CENTER,
            window.center.to_string(),
        );
        insert_header_if_valid(
            headers,
            DISPLAY_FRAME_HEADER_WINDOW_WIDTH,
            window.width.to_string(),
        );
    }
    Ok(response)
}

pub(super) async fn raw_frame(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let file = registered_file(&state, index, "file")?;
    ensure_pixels_shown(&state, &file)?;
    let source = file.path.clone();

    let raw_response = pixels::load_raw_frame(file, state.raw_cache(), RawFrameRequest { frame })
        .await
        .map_err(|failure| error::gone_or(&source, error::pixel_error(failure)))?;

    Ok(raw_response_with_headers(
        raw_response.body,
        &raw_response.metadata,
        raw_response.cache_hit,
    ))
}

pub(super) async fn raw_pixel(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<PixelQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let file = registered_file(&state, index, "file")?;
    ensure_pixels_shown(&state, &file)?;
    let source = file.path.clone();
    let raw = pixels::load_raw_frame(file.clone(), state.raw_cache(), RawFrameRequest { frame })
        .await
        .map_err(|failure| error::gone_or(&source, error::pixel_error(failure)))?;
    let (body, metadata) =
        pixels::raw_pixel(&file, &raw, query.row, query.column).ok_or_else(|| {
            ApiError::bad_request(format!(
                "pixel (row {}, column {}) is outside the {}x{} frame",
                query.row, query.column, raw.metadata.rows, raw.metadata.columns
            ))
        })?;
    Ok(raw_response_with_headers(body, &metadata, raw.cache_hit))
}

/// A raw-frame response: the samples, `X-Cache`, and the metadata headers.
fn raw_response_with_headers(
    body: bytes::Bytes,
    meta: &crate::api::contracts::RawFrameMetadata,
    cache_hit: bool,
) -> Response {
    let cache_header = if cache_hit { CACHE_HIT } else { CACHE_MISS };

    let mut response = Response::new(axum::body::Body::from(body));
    let headers = response.headers_mut();
    headers.insert(CACHE_HEADER, HeaderValue::from_static(cache_header));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(OCTET_STREAM_MEDIA_TYPE),
    );
    insert_header_if_valid(headers, RAW_FRAME_HEADER_ROWS, meta.rows.to_string());
    insert_header_if_valid(headers, RAW_FRAME_HEADER_COLUMNS, meta.columns.to_string());
    insert_header_if_valid(
        headers,
        RAW_FRAME_HEADER_BITS_ALLOCATED,
        meta.bits_allocated.to_string(),
    );
    insert_header_if_valid(
        headers,
        RAW_FRAME_HEADER_PIXEL_REPRESENTATION,
        meta.pixel_representation.to_string(),
    );
    insert_header_if_valid(
        headers,
        RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
        meta.samples_per_pixel.to_string(),
    );
    insert_header_if_valid(
        headers,
        RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION,
        meta.photometric_interpretation.clone(),
    );
    insert_header_if_valid(
        headers,
        RAW_FRAME_HEADER_RESCALE_SLOPE,
        meta.rescale_slope.to_string(),
    );
    insert_header_if_valid(
        headers,
        RAW_FRAME_HEADER_RESCALE_INTERCEPT,
        meta.rescale_intercept.to_string(),
    );
    if let Some(wc) = meta.default_wc {
        insert_header_if_valid(headers, RAW_FRAME_HEADER_DEFAULT_WC, wc.to_string());
    }
    if let Some(ww) = meta.default_ww {
        insert_header_if_valid(headers, RAW_FRAME_HEADER_DEFAULT_WW, ww.to_string());
    }
    if let (Some(low), Some(high)) = (meta.padding_low, meta.padding_high) {
        insert_header_if_valid(headers, RAW_FRAME_HEADER_PADDING_LOW, low.to_string());
        insert_header_if_valid(headers, RAW_FRAME_HEADER_PADDING_HIGH, high.to_string());
    }

    response
}

pub(super) async fn tags(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Json<Vec<TagNode>>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let file = registered_file(&state, index, "file")?;

    if let Some(nodes) = state.cached_tags(index) {
        return Ok(Json(nodes));
    }

    let path = file.path.clone();
    let mut nodes = tokio::task::spawn_blocking(move || tags::build_tag_tree(&path))
        .await
        .map_err(|error| ApiError::internal(format!("tag serialization task failed: {error}")))?
        .map_err(|failure| {
            error::gone_or(
                &file.path,
                ApiError::internal(format!("tag serialization failed: {failure}")),
            )
        })?;

    // The cache holds what the session shows: the mode never changes.
    if let Some(masker) = state.registry().masker() {
        masker.tags(&file, &mut nodes);
    }
    state.cache_tags(index, nodes.clone());
    Ok(Json(nodes))
}

pub(super) async fn select_tag(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
    query: Result<Query<TagQuery>, QueryRejection>,
) -> Result<Json<TagNode>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let file = registered_file(&state, index, "file")?;
    let path = file.path.clone();
    let selector = query.path.clone();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(tags::TAG_SELECT_DEFAULT_LIMIT);
    let mut node = tokio::task::spawn_blocking(move || {
        tags::build_selected_tag(&path, &selector, offset, limit)
    })
    .await
    .map_err(|error| ApiError::internal(format!("tag selection task failed: {error}")))?
    .map_err(|error| match error {
        tags::TagSelectError::Invalid(_) => ApiError::bad_request(error.to_string()),
        tags::TagSelectError::Read(_) => {
            error::gone_or(&file.path, ApiError::internal(error.to_string()))
        }
    })?;
    if let Some(masker) = state.registry().masker() {
        masker.selected_tag(&file, &query.path, &mut node);
    }
    Ok(Json(node))
}

fn insert_header_if_valid(headers: &mut HeaderMap, name: &'static str, value: String) {
    if let Ok(parsed) = HeaderValue::from_str(&value) {
        headers.insert(name, parsed);
    }
}
