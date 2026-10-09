//! Overlays drawn on a displayed image frame: one SEG frame painted on its
//! resolved source frame, and value overlays (an RT Dose grid or Parametric
//! Map colorized in real-world units and resampled onto the frame) with the
//! legend that describes their colors.
//!
//! Semantic context decides eligibility from metadata; this module decodes
//! the overlay's frames through the raw-frame cache, applies each frame's
//! real-world mapping, and caches the encoded PNG per overlay and displayed
//! frame, which is what every overlay endpoint's `X-Cache` reports. A value
//! overlay can also be sent as its resampled values, for readouts.

use super::error::{self, ApiError};
use super::handlers::{registered_file, semantic_context_for, value_mappings_for};
use super::state::AppState;
use crate::api::contracts::{
    DoseOverlayQuery, OverlayEligibility, OverlayLegend, ParametricMapOverlayQuery,
    ResolvedSegmentSourceFrame, SemanticContext, SemanticContextResponse, CACHE_HEADER, CACHE_HIT,
    CACHE_MISS, OCTET_STREAM_MEDIA_TYPE, PNG_MEDIA_TYPE,
};
use crate::geometry::frame_geometry;
use crate::pixels::{
    self, ColorScale, ColorwashRequest, CoveredFrames, DecodeWork, PixelError, PixelResult,
    ValueRange, COLORMAP_NAME, COLORMAP_STOPS,
};
use crate::plane_stack::{PlaneStack, StackSampleError};
use crate::types::{
    FileEntry, NativePixelDataKind, OverlayCacheKey, OverlayEncoding, ValueRangeCacheKey,
};
use crate::value_mapping::{map_value, FileValueMappings};
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use bytes::Bytes;
use dicom_dictionary_std::uids;
use std::sync::Arc;
use tokio::task;

pub(super) async fn segmentation_overlay(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    use crate::semantic::SegmentationOverlayError;

    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let segmentation = registered_file(&state, index, "file")?;
    let files = state.registry().files_snapshot();
    let plan = async {
        crate::semantic::check_segmentation_frame(&segmentation, frame)?;
        let context = semantic_context_for(&state, segmentation.clone(), files.clone())
            .await
            .map_err(SegmentationOverlayError::Metadata)?;
        crate::semantic::segmentation_overlay_plan(&segmentation, frame, &context, &files)
    }
    .await
    .map_err(|error| match error {
        SegmentationOverlayError::NotSegmentation => ApiError::bad_request(error.to_string()),
        SegmentationOverlayError::FrameOutOfRange(error) => error::pixel_error(error),
        SegmentationOverlayError::Unavailable(_) => {
            ApiError::semantic_mapping_unavailable(error.to_string())
        }
        SegmentationOverlayError::Metadata(_) => ApiError::internal(error.to_string()),
    })?;
    let target = files
        .iter()
        .find(|file| file.index == plan.source_file_index)
        .cloned()
        .ok_or_else(|| ApiError::not_found("resolved source file is unavailable"))?;

    // The plan resolves the source frame against the current file set, so
    // the key names it rather than trusting an earlier resolution.
    let key = OverlayCacheKey {
        overlay_file_index: segmentation.index,
        overlay_frame: Some(frame),
        target_file_index: plan.source_file_index,
        target_frame: plan.source_frame_index,
        encoding: OverlayEncoding::Png,
        file_set: None,
    };
    let work = DecodeWork::SegmentationOverlay {
        target_rows: target.rows,
        target_columns: target.columns,
    };
    let (png, cache_hit) = pixels::compute_from_frames(
        segmentation,
        work,
        state.overlay_cache(),
        key,
        state.raw_cache(),
        |frames| async move {
            let raw = frames.raw(frame).await?;
            task::spawn_blocking(move || {
                pixels::encode_segmentation_overlay_png(
                    &raw.body,
                    &raw.metadata,
                    &plan,
                    target.rows,
                    target.columns,
                )
            })
            .await
            .map_err(|error| {
                PixelError::frame_decode(anyhow::anyhow!(
                    "SEG overlay encoding task failed: {error}"
                ))
            })?
        },
    )
    .await
    .map_err(error::pixel_error)?;
    Ok(overlay_response(png, cache_hit, OverlayEncoding::Png))
}

/// The frame's own shutter and overlay graphics, for a frame windowed in the
/// browser.
pub(super) async fn presentation_layer(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let file = registered_file(&state, index, "file")?;
    if !file.has_pixels {
        return Err(error::pixel_error(PixelError::NoPixelData {
            file_index: index,
        }));
    }
    PixelError::ensure_frame(frame, file.frame_count).map_err(error::pixel_error)?;
    // The layer is sized by the entry's rows and columns: a raster the
    // viewer does not decode (too large, for one) gets none.
    if file.format.is_raster() {
        if let Some(reason) = pixels::classify_pixel_support(&file).reason_id() {
            return Err(error::pixel_error(PixelError::UnsupportedLayout(
                reason.to_string(),
            )));
        }
    }
    // A file's own frame as overlay and target: no SEG is its own source.
    let key = OverlayCacheKey {
        overlay_file_index: index,
        overlay_frame: Some(frame),
        target_file_index: index,
        target_frame: frame,
        encoding: OverlayEncoding::Png,
        file_set: None,
    };
    let redaction = state
        .redactions()
        .for_frame(index, frame)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    // A layer with redaction boxes changes with them and is drawn each time.
    let cacheable = redaction.is_empty();
    if let Some(png) = state.cached_overlay(&key).filter(|_| cacheable) {
        return Ok(overlay_response(png, true, OverlayEncoding::Png));
    }
    // The layer is four bytes a pixel whatever the file holds, so it is
    // drawn under a permit like a decode.
    let drawn = file.clone();
    let png = pixels::draw_presentation_layer(&state.decode_scheduler(), &file, move || {
        pixels::encode_presentation_layer_png(&drawn, frame, &redaction.boxes)
    })
    .await
    .map_err(error::pixel_error)?
    .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    if cacheable {
        state.cache_overlay(key, png.clone());
    }
    Ok(overlay_response(png, false, OverlayEncoding::Png))
}

pub(super) async fn dose_overlay(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<DoseOverlayQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    dose_value_overlay(state, path, query, OverlayEncoding::Png).await
}

pub(super) async fn dose_overlay_values(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<DoseOverlayQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    dose_value_overlay(state, path, query, OverlayEncoding::Values).await
}

pub(super) async fn parametric_map_overlay(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<ParametricMapOverlayQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    parametric_map_value_overlay(state, path, query, OverlayEncoding::Png).await
}

pub(super) async fn parametric_map_overlay_values(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<ParametricMapOverlayQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    parametric_map_value_overlay(state, path, query, OverlayEncoding::Values).await
}

async fn dose_value_overlay(
    state: AppState,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<DoseOverlayQuery>, QueryRejection>,
    encoding: OverlayEncoding,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let target = registered_file(&state, index, "file")?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let dose = registered_file(&state, query.dose, "dose file")?;
    if dose.sop_class_uid != uids::RT_DOSE_STORAGE {
        return Err(ApiError::bad_request("dose must select an RT Dose object"));
    }
    value_overlay(&state, dose, target, frame, encoding).await
}

async fn parametric_map_value_overlay(
    state: AppState,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<ParametricMapOverlayQuery>, QueryRejection>,
    encoding: OverlayEncoding,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let target = registered_file(&state, index, "file")?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let map = registered_file(&state, query.map, "parametric map file")?;
    if map.sop_class_uid != uids::PARAMETRIC_MAP_STORAGE {
        return Err(ApiError::bad_request(
            "map must select a Parametric Map object",
        ));
    }
    value_overlay(&state, map, target, frame, encoding).await
}

/// Resample `overlay`'s planes onto one displayed frame of `target`, sent
/// as a colorwash PNG or as the values themselves.
async fn value_overlay(
    state: &AppState,
    overlay: Arc<FileEntry>,
    target: Arc<FileEntry>,
    frame: u32,
    encoding: OverlayEncoding,
) -> Result<Response, ApiError> {
    PixelError::ensure_frame(frame, target.frame_count).map_err(error::pixel_error)?;
    if target.index == overlay.index {
        return Err(ApiError::semantic_mapping_unavailable(
            "an overlay is drawn on image frames, not on its own frames",
        ));
    }
    // The legend is read against this file set, so the overlay drawn with
    // it is kept for this file set.
    let files = state.registry().files_snapshot();
    let file_set = files.len();
    let context = semantic_context_for(state, overlay.clone(), files)
        .await
        .map_err(error::context_failure)?;
    let (stack, legend) = overlay_plan(&overlay, &context)?;
    let frame_of_reference = &overlay.series_metadata.frame_of_reference_uid;
    if target.series_metadata.frame_of_reference_uid != *frame_of_reference {
        return Err(ApiError::semantic_mapping_unavailable(
            "the displayed frame is not in the overlay's Frame of Reference",
        ));
    }
    let geometry = frame_geometry(&target, frame).ok_or_else(|| {
        ApiError::semantic_mapping_unavailable("displayed frame geometry is incomplete")
    })?;
    let sample = stack.sample(geometry).map_err(|error| match error {
        StackSampleError::NotCovered => ApiError::overlay_not_covering_frame(error.to_string()),
        StackSampleError::InvalidGeometry => {
            ApiError::semantic_mapping_unavailable(error.to_string())
        }
    })?;

    let key = OverlayCacheKey {
        overlay_file_index: overlay.index,
        overlay_frame: None,
        target_file_index: target.index,
        target_frame: frame,
        encoding,
        file_set: Some(file_set),
    };
    if let Some(body) = state.cached_overlay(&key) {
        return Ok(overlay_response(body, true, encoding));
    }
    let mappings = value_mappings_for(state, overlay.clone())
        .await
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    let plane_frames = sample.frames();
    let work = DecodeWork::ValueOverlay {
        target_rows: target.rows,
        target_columns: target.columns,
        planes: u32::try_from(plane_frames.len()).unwrap_or(u32::MAX),
        encoding,
    };
    let scale = ColorScale {
        min: legend.min_value,
        max: legend.max_value,
        transparent_at_or_below: legend.transparent_at_or_below,
    };
    let (body, cache_hit) = pixels::compute_from_frames(
        overlay,
        work,
        state.overlay_cache(),
        key,
        state.raw_cache(),
        |frames| async move {
            let mut planes = Vec::with_capacity(plane_frames.len());
            for plane_frame in plane_frames {
                planes.push(mapped_frame_values(&frames, &mappings, plane_frame).await?);
            }
            task::spawn_blocking(move || {
                let values = sample.resample(&planes).ok_or_else(|| {
                    PixelError::UnsupportedLayout(
                        "overlay plane values do not match the plane grid".into(),
                    )
                })?;
                match encoding {
                    OverlayEncoding::Png => pixels::encode_colorwash_png(ColorwashRequest {
                        values: &values,
                        target_rows: target.rows,
                        target_columns: target.columns,
                        scale,
                    }),
                    OverlayEncoding::Values => Ok(encode_f32_values(&values)),
                }
            })
            .await
            .map_err(|error| {
                PixelError::frame_decode(anyhow::anyhow!("overlay encoding task failed: {error}"))
            })?
        },
    )
    .await
    .map_err(error::pixel_error)?;
    Ok(overlay_response(body, cache_hit, encoding))
}

/// Resampled values as little-endian `f32`s; NaN stays NaN.
fn encode_f32_values(values: &[f64]) -> Bytes {
    values
        .iter()
        .flat_map(|value| (*value as f32).to_le_bytes())
        .collect::<Vec<u8>>()
        .into()
}

/// The overlay's plane stack and legend, when its context is eligible.
fn overlay_plan(
    overlay: &FileEntry,
    context: &SemanticContextResponse,
) -> Result<(PlaneStack, OverlayLegend), ApiError> {
    let (eligibility, legend, stack) = match &context.context {
        SemanticContext::RtDose(dose) => (
            &dose.overlay,
            &dose.legend,
            PlaneStack::from_dose_grid(
                overlay.rows,
                overlay.columns,
                overlay.frame_count,
                &dose.geometry,
            ),
        ),
        SemanticContext::ParametricMap(map) => {
            (&map.overlay, &map.legend, PlaneStack::from_frames(overlay))
        }
        _ => {
            return Err(ApiError::bad_request(
                "value overlays are defined for RT Dose and Parametric Map objects",
            ))
        }
    };
    if !eligibility.eligible {
        return Err(ApiError::semantic_mapping_unavailable(
            eligibility.reason.clone(),
        ));
    }
    let legend = legend.clone().ok_or_else(|| {
        ApiError::semantic_mapping_unavailable("the overlay legend is unavailable")
    })?;
    let stack = stack.map_err(ApiError::semantic_mapping_unavailable)?;
    Ok((stack, legend))
}

/// How an overlay kind turns the value range of its frames into a legend.
#[derive(Clone, Copy)]
enum LegendScale {
    /// RT Dose: `0..max` of the whole grid, zero dose transparent.
    PositiveDose,
    /// Parametric Map: `min..max` of every frame's mapped values.
    MappedRange,
}

/// Complete an eligible RT Dose or Parametric Map context with its legend,
/// which spans the values of every frame so all displayed frames share one
/// color scale. Frames that cannot be decoded, or hold no usable value,
/// make the overlay ineligible instead.
///
/// `Err(PixelError::DecodeBusy)` when a frame could not be decoded only
/// because the viewer was busy: that says nothing about the overlay, so the
/// context is left incomplete for the caller to discard, not to cache.
pub(super) async fn add_overlay_legend(
    state: &AppState,
    file: &Arc<FileEntry>,
    file_set: usize,
    context: &mut SemanticContextResponse,
) -> PixelResult<()> {
    let (eligibility, source_frames, legend, scale): (
        &mut OverlayEligibility,
        &mut Vec<ResolvedSegmentSourceFrame>,
        &mut Option<OverlayLegend>,
        LegendScale,
    ) = match &mut context.context {
        SemanticContext::RtDose(dose) => (
            &mut dose.overlay,
            &mut dose.overlay_source_frames,
            &mut dose.legend,
            LegendScale::PositiveDose,
        ),
        SemanticContext::ParametricMap(map) => (
            &mut map.overlay,
            &mut map.overlay_source_frames,
            &mut map.legend,
            LegendScale::MappedRange,
        ),
        _ => return Ok(()),
    };
    if !eligibility.eligible {
        return Ok(());
    }
    match value_legend(state, file, file_set, scale).await {
        Ok(value) => *legend = Some(value),
        Err(LegendFailure::Busy) => return Err(PixelError::DecodeBusy),
        Err(LegendFailure::Unavailable(reason)) => {
            *eligibility = crate::semantic::ineligible(&reason);
            source_frames.clear();
        }
    }
    Ok(())
}

/// Why an overlay has no legend.
enum LegendFailure {
    /// A frame was not decoded because the viewer was busy; ask again.
    Busy,
    /// The overlay cannot be drawn, for this reason.
    Unavailable(String),
}

impl From<&str> for LegendFailure {
    fn from(reason: &str) -> Self {
        Self::Unavailable(reason.to_string())
    }
}

async fn value_legend(
    state: &AppState,
    file: &Arc<FileEntry>,
    file_set: usize,
    scale: LegendScale,
) -> Result<OverlayLegend, LegendFailure> {
    let mappings = value_mappings_for(state, file.clone())
        .await
        .map_err(|error| {
            LegendFailure::Unavailable(format!("overlay metadata could not be read: {error:#}"))
        })?;
    let map = mappings
        .real_world(0)
        .next()
        .ok_or("the overlay has no real-world value mapping")?
        .clone();
    let key = ValueRangeCacheKey {
        file_index: file.index,
        file_set,
    };
    let (range, _) = pixels::compute_from_frames(
        file.clone(),
        DecodeWork::ValueLegend,
        state.value_range_cache(),
        key,
        state.raw_cache(),
        |frames| async move {
            let mut range = ValueRange::EMPTY;
            for frame in 0..frames.file().frame_count {
                let values = mapped_frame_values(&frames, &mappings, frame).await?;
                range = task::spawn_blocking(move || {
                    values.into_iter().fold(range, ValueRange::including)
                })
                .await
                .map_err(|error| {
                    PixelError::raw_decode(anyhow::anyhow!("value range task failed: {error}"))
                })?;
            }
            Ok(range)
        },
    )
    .await
    .map_err(|error| match error {
        PixelError::DecodeBusy => LegendFailure::Busy,
        error => {
            LegendFailure::Unavailable(format!("overlay frames could not be decoded: {error}"))
        }
    })?;
    let (min, max) = (range.min, range.max);
    let (min_value, transparent_at_or_below) = match scale {
        LegendScale::PositiveDose if max > 0.0 => (0.0, Some(0.0)),
        LegendScale::PositiveDose => return Err("the dose grid holds no positive dose".into()),
        LegendScale::MappedRange if min <= max => (min, None),
        LegendScale::MappedRange => return Err("no sample has a mapped value".into()),
    };
    Ok(OverlayLegend {
        unit_label: map.unit_label,
        units: map.units,
        min_value,
        max_value: max,
        transparent_at_or_below,
        colormap: COLORMAP_NAME.to_string(),
        color_stops: COLORMAP_STOPS.to_vec(),
    })
}

/// One frame's samples in the units of its preferred real-world mapping;
/// samples outside the mapped range are NaN.
async fn mapped_frame_values(
    frames: &CoveredFrames,
    mappings: &FileValueMappings,
    frame: u32,
) -> PixelResult<Vec<f64>> {
    let map = mappings.real_world(frame).next().cloned().ok_or_else(|| {
        PixelError::UnsupportedLayout(format!("frame {frame} has no real-world value mapping"))
    })?;
    let kind = frames
        .file()
        .series_metadata
        .native_pixel
        .pixel_data_kind
        .unwrap_or(NativePixelDataKind::Integer);
    let raw = frames.raw(frame).await?;
    task::spawn_blocking(move || {
        let stored = pixels::raw_frame_values(&raw.body, &raw.metadata, kind)?;
        Ok(stored
            .into_iter()
            .map(|value| map_value(&map, value).unwrap_or(f64::NAN))
            .collect())
    })
    .await
    .map_err(|error| {
        PixelError::raw_decode(anyhow::anyhow!("value mapping task failed: {error}"))
    })?
}

fn overlay_response(body: Bytes, cache_hit: bool, encoding: OverlayEncoding) -> Response {
    let mut response = Response::new(axum::body::Body::from(body));
    response.headers_mut().insert(
        CACHE_HEADER,
        HeaderValue::from_static(if cache_hit { CACHE_HIT } else { CACHE_MISS }),
    );
    let media_type = match encoding {
        OverlayEncoding::Png => PNG_MEDIA_TYPE,
        OverlayEncoding::Values => OCTET_STREAM_MEDIA_TYPE,
    };
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    response
}
