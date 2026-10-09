use super::error::{self, ApiError};
use super::overlays;
use super::state::AppState;
use crate::api::contracts::RedactionSeriesResponse;
use crate::api::contracts::{
    DiscoveryResult, EmbedRoiAnnotations, FileSummary, FilesQuery, FilesResponse, FrameInfo,
    FrameQuery, GraphicAnnotationsQuery, GraphicAnnotationsResponse, HealthResponse, PixelQuery,
    ReferenceCatalogResponse, SemanticContextResponse, TagNode, TagQuery, ThumbnailQuery,
    ViewerIdentity, CACHE_HEADER, CACHE_HIT, CACHE_MISS, CSV_MEDIA_TYPE,
    DISPLAY_FRAME_HEADER_WINDOW_APPLIED, DISPLAY_FRAME_HEADER_WINDOW_CENTER,
    DISPLAY_FRAME_HEADER_WINDOW_WIDTH, EXPORT_CONTENT_DISPOSITION_HEADER,
    EXPORT_CONTENT_DISPOSITION_VALUE, FILE_KEY_HEADER, JPEG_MEDIA_TYPE, OCTET_STREAM_MEDIA_TYPE,
    RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_COLUMNS, RAW_FRAME_HEADER_DEFAULT_WC,
    RAW_FRAME_HEADER_DEFAULT_WW, RAW_FRAME_HEADER_PADDING_HIGH, RAW_FRAME_HEADER_PADDING_LOW,
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, RAW_FRAME_HEADER_PIXEL_REPRESENTATION,
    RAW_FRAME_HEADER_RESCALE_INTERCEPT, RAW_FRAME_HEADER_RESCALE_SLOPE, RAW_FRAME_HEADER_ROWS,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL, THUMBNAIL_CACHE_CONTROL, THUMBNAIL_HEADER_SOURCE,
    THUMBNAIL_SIZE_BUCKETS,
};
use crate::pixels::{self, FrameRequest, RawFrameRequest, ThumbnailRequest};
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

pub(super) async fn files(
    State(state): State<AppState>,
    query: Result<Query<FilesQuery>, QueryRejection>,
) -> Result<Json<FilesResponse>, ApiError> {
    let Query(query) = query.map_err(error::query_rejection)?;
    if query.limit == Some(0) {
        return Err(ApiError::invalid_query("limit must be at least 1"));
    }
    // One read of the registry: the entries, the scan state and the
    // counters in this response are of the same moment. A second read here
    // could report a completed scan beside a list that predates its end.
    let page = state.registry().files_page(
        query.since,
        query
            .limit
            .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX)),
    );
    Ok(Json(FilesResponse {
        files: page.files,
        discovery: page
            .discovery
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
        masked: state.registry().masker().is_some(),
        scan_complete: page.status.scan_complete,
        scanned: page.status.scanned,
        skipped: page.status.skipped,
        filtered: page.status.filtered,
        revision: page.revision,
        reset: page.reset,
        more: page.more,
        keys_hashing: page.keys_hashing,
        rekeys: page.rekeys,
    }))
}

/// What every display and raw frame response does for file keys: sends the
/// file's key as the session shows it in [`FILE_KEY_HEADER`] when it has
/// one, and notes that a frame was served, which starts the hashing of a
/// file that has none.
fn note_frame_served(state: &AppState, index: usize, headers: &mut HeaderMap) {
    let registry = state.registry();
    if let Some(key) = registry.shown_file_key(index) {
        insert_header_if_valid(headers, FILE_KEY_HEADER, key);
    }
    registry.frame_sent(index);
}

/// The registered file at `index`, or a 404 naming the index and how many
/// files are loaded. `role` names what the index selects.
pub(super) fn registered_file(
    state: &AppState,
    index: usize,
    role: &str,
) -> Result<Arc<FileEntry>, ApiError> {
    state.registry().get_or_count(index).map_err(|count| {
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
    if source.format.is_raster() {
        return uid_masked_json(
            &state,
            ReferenceCatalogResponse {
                source_file_index: index,
                source_sop_instance_uid: String::new(),
                references: Vec::new(),
            },
        );
    }
    let source_path = source.path.clone();
    let edges = task::spawn_blocking(move || references::extract_reference_edges(&source_path))
        .await
        .map_err(|error| ApiError::internal(format!("reference extraction task failed: {error}")))?
        .map_err(|error| error::gone_or(&source.path, ApiError::internal(error.to_string())))?;
    let candidates = state
        .registry()
        .files_snapshot()
        .into_iter()
        .filter(|file| !file.format.is_raster())
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
        .map_err(|failure| error::gone_or(&path, error::context_failure(failure)))?;
    let mut context = SemanticContextResponse::clone(&context);
    if let Some(masker) = state.registry().masker() {
        masker.semantic_context(&source, &mut context);
    }
    uid_masked_json(&state, context)
}

/// The source's semantic context against `files`, built at most once per
/// file set and kept in a small LRU.
///
/// The legend of an RT Dose or Parametric Map decodes its frames. When one
/// is refused because the viewer is busy, the error is that
/// `PixelError::DecodeBusy` and nothing is kept, so the next request builds
/// the context again; `error::context_failure` reports it.
pub(super) async fn semantic_context_for(
    state: &AppState,
    source: Arc<FileEntry>,
    files: Vec<Arc<FileEntry>>,
) -> anyhow::Result<Arc<SemanticContextResponse>> {
    if source.format.is_raster() {
        return Ok(Arc::new(SemanticContextResponse {
            source_file_index: source.index,
            default_mode: "pixel_preview".to_string(),
            pixel_preview_preserves_stored_values: true,
            context: crate::api::contracts::SemanticContext::NotApplicable {
                reason: "semantic context is not available for image files".to_string(),
            },
        }));
    }
    let key = (source.index, files.len());
    if let Some(context) = state.cached_semantic_context(key) {
        return Ok(context);
    }
    let object = source.clone();
    let file_set = files.len();
    let mut context =
        task::spawn_blocking(move || crate::semantic::semantic_context(&object, &files))
            .await
            .map_err(|error| anyhow::anyhow!("semantic context task failed: {error}"))??;
    overlays::add_overlay_legend(state, &source, file_set, &mut context).await?;
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
    if file.format.is_raster() {
        return Ok(Arc::new(FileValueMappings::identity(&file)));
    }
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

pub(super) async fn redactions(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Json<EmbedRoiAnnotations>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    registered_file(&state, index, "file")?;
    let boxes = state
        .redactions()
        .get(index)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(boxes))
}

pub(super) async fn update_redactions(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
    payload: Result<Json<EmbedRoiAnnotations>, JsonRejection>,
) -> Result<Json<EmbedRoiAnnotations>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let Json(boxes) = payload.map_err(error::json_rejection)?;
    let file = registered_file(&state, index, "file")?;
    let canonical = state
        .redactions()
        .replace_for_file(&file, boxes)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(canonical))
}

/// Copies a file's redaction boxes to the files of its series that share its
/// image size: a banner sits at the same place in each. A copy keeps the
/// boxes' frames only when the file has as many frames as the source;
/// otherwise its boxes cover every frame.
pub(super) async fn apply_redactions_to_series(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
) -> Result<Json<RedactionSeriesResponse>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let source = registered_file(&state, index, "file")?;
    if source.format.is_raster() {
        return Ok(Json(RedactionSeriesResponse {
            file_indices: Vec::new(),
        }));
    }
    let boxes = state
        .redactions()
        .get(index)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut file_indices = Vec::new();
    for file in state.registry().files_snapshot() {
        let same_series = !file.format.is_raster()
            && file.index != source.index
            && file.study_instance_uid == source.study_instance_uid
            && file.series_instance_uid == source.series_instance_uid
            && (file.rows, file.columns) == (source.rows, source.columns);
        if !same_series {
            continue;
        }
        let mut copy = boxes.clone();
        if file.frame_count != source.frame_count {
            copy.roi_frames.clear();
        }
        state
            .redactions()
            .replace_for_file(&file, copy)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        file_indices.push(file.index);
    }
    Ok(Json(RedactionSeriesResponse { file_indices }))
}

/// The redaction boxes of one frame.
fn frame_redaction(
    state: &AppState,
    index: usize,
    frame: u32,
) -> Result<pixels::Redaction, ApiError> {
    state
        .redactions()
        .for_frame(index, frame)
        .map_err(|error| ApiError::internal(error.to_string()))
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
                error::gone_or(
                    &source,
                    ApiError::failed("the frame's value mappings could not be read", failure),
                )
            })?;
        let preferred = mappings.real_world(frame).next();
        window = match preferred.filter(|map| map.unit_label == unit) {
            Some(map) => (query.wc, query.ww, Some(map.clone())),
            None => (None, None, None),
        };
    }
    let (window_center, window_width, real_world) = window;

    let redaction = frame_redaction(&state, index, frame)?;
    let frame_response = pixels::load_redacted_frame(
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
        redaction,
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
    note_frame_served(&state, index, response.headers_mut());
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

    let raw_response = pixels::load_redacted_raw_frame(
        file,
        state.raw_cache(),
        RawFrameRequest { frame },
        &frame_redaction(&state, index, frame)?,
    )
    .await
    .map_err(|failure| error::gone_or(&source, error::pixel_error(failure)))?;

    let mut response = raw_response_with_headers(
        raw_response.body,
        &raw_response.metadata,
        raw_response.cache_hit,
    );
    note_frame_served(&state, index, response.headers_mut());
    Ok(response)
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
    let raw = pixels::load_redacted_raw_frame(
        file.clone(),
        state.raw_cache(),
        RawFrameRequest { frame },
        &frame_redaction(&state, index, frame)?,
    )
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

/// The gallery thumbnail of one frame. It is refused and redacted exactly
/// where the display frame is: a masked session withholds it for label and
/// overview images, and the frame's redaction boxes are painted before it is
/// encoded and are part of its cache key.
pub(super) async fn thumbnail(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<ThumbnailQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let file = registered_file(&state, index, "file")?;
    ensure_pixels_shown(&state, &file)?;
    let bucket = pixels::thumbnail_bucket(query.size).ok_or_else(|| {
        ApiError::invalid_query(format!(
            "size must be between 1 and {}",
            THUMBNAIL_SIZE_BUCKETS[THUMBNAIL_SIZE_BUCKETS.len() - 1]
        ))
    })?;
    let source = file.path.clone();

    let thumbnail = pixels::load_thumbnail(
        file,
        state.thumbnail_cache(),
        ThumbnailRequest {
            frame,
            bucket,
            window_mode: query.window_mode.unwrap_or_default(),
        },
        frame_redaction(&state, index, frame)?,
    )
    .await
    .map_err(|failure| error::gone_or(&source, error::pixel_error(failure)))?;

    let cache_header = if thumbnail.cache_hit {
        CACHE_HIT
    } else {
        CACHE_MISS
    };
    let mut response = Response::new(axum::body::Body::from(thumbnail.body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(JPEG_MEDIA_TYPE),
    );
    headers.insert(CACHE_HEADER, HeaderValue::from_static(cache_header));
    headers.insert(
        THUMBNAIL_HEADER_SOURCE,
        HeaderValue::from_static(thumbnail.source.as_str()),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(THUMBNAIL_CACHE_CONTROL),
    );
    Ok(response)
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

    if file.format.is_raster() {
        return Ok(Json(raster_tags(&state, index, file).await?));
    }
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

/// The metadata tree of a raster image file as this session shows it,
/// from the tag cache or read now. Reading takes no decode permit: it is
/// bounded by `pixels::read_raster_tags` and decodes no pixels. The error
/// says nothing the file holds.
async fn raster_tags(
    state: &AppState,
    index: usize,
    file: Arc<FileEntry>,
) -> Result<Vec<TagNode>, ApiError> {
    if let Some(nodes) = state.cached_tags(index) {
        return Ok(nodes);
    }
    let read = file.clone();
    let mut nodes = tokio::task::spawn_blocking(move || pixels::raster_tag_nodes(&read))
        .await
        .map_err(|error| ApiError::internal(format!("metadata task failed: {error}")))?
        .map_err(|_| {
            error::gone_or(
                &file.path,
                ApiError::internal("the image file could not be opened to read its metadata"),
            )
        })?;
    // The cache holds what the session shows: the mode never changes.
    if let Some(masker) = state.registry().masker() {
        masker.raster_tags(&mut nodes);
    }
    state.cache_tags(index, nodes.clone());
    Ok(nodes)
}

pub(super) async fn select_tag(
    State(state): State<AppState>,
    path: Result<Path<usize>, PathRejection>,
    query: Result<Query<TagQuery>, QueryRejection>,
) -> Result<Json<TagNode>, ApiError> {
    let Path(index) = path.map_err(error::path_rejection)?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let file = registered_file(&state, index, "file")?;
    if file.format.is_raster() {
        // Selected from the tree `/tags` answers with, masked as it is.
        let nodes = raster_tags(&state, index, file).await?;
        let node = tags::select_raster_node(
            &nodes,
            &query.path,
            query.offset.unwrap_or(0),
            query.limit.unwrap_or(tags::TAG_SELECT_DEFAULT_LIMIT),
        )
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
        return Ok(Json(node));
    }
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

#[cfg(test)]
mod tests {
    use crate::annotations::AnnotationStore;
    use crate::server::{router, AppState, FileRegistry};
    use axum_test::TestServer;
    use serde_json::Value;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// The catalog a client polls must be one moment of the registry. The
    /// scan is made to find its last files and finish at the one point where
    /// a response assembled from two reads would differ from one assembled
    /// from a single read: after the entries were read. A response that then
    /// reports the finished scan lists fewer files than the scan found, and
    /// the page stops polling on `scan_complete`.
    #[tokio::test]
    async fn a_catalog_response_is_one_moment_of_the_scan() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let template = crate::loader::test_entry(&fixture);
        let entry = move |number: usize| {
            let mut file = template.clone();
            file.path = PathBuf::from(format!("/scan/{number}.dcm"));
            file.sop_instance_uid = format!("1.2.826.0.1.3680043.10.515.{number}");
            file
        };
        // (query, how the request is paged): the plain listing and the cursor
        // take the same snapshot.
        for query in ["", "?since=0", "?since=0&limit=1000"] {
            let registry = FileRegistry::new();
            registry.insert(entry(0));
            let scan = registry.clone();
            let entry = entry.clone();
            let finished = Arc::new(AtomicBool::new(false));
            let registry = registry.with_after_page(Arc::new({
                let finished = finished.clone();
                move || {
                    if !finished.swap(true, Ordering::AcqRel) {
                        scan.insert(entry(1));
                        scan.insert(entry(2));
                        scan.mark_scan_complete();
                    }
                }
            }));
            let server = TestServer::new(router(AppState::new(
                registry.clone(),
                AnnotationStore::empty(),
            )));

            let during: Value = server.get(&format!("/api/files{query}")).await.json();
            assert!(
                finished.load(Ordering::Acquire),
                "{query}: the scan finished while the request was answered"
            );
            let listed = during["files"].as_array().expect("files").len();
            assert_eq!(
                (listed, &during["scan_complete"]),
                (1, &Value::Bool(false)),
                "{query}: the response is the catalog as it was when its entries were read"
            );
            assert_eq!(during["revision"], 1, "{query}");

            let after: Value = server.get(&format!("/api/files{query}")).await.json();
            assert_eq!(
                (
                    after["files"].as_array().expect("files").len(),
                    &after["scan_complete"]
                ),
                (3, &Value::Bool(true)),
                "{query}"
            );
        }
    }
}
