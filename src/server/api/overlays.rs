//! Value overlays: an RT Dose grid colorized in real-world units and
//! resampled onto a displayed image frame, and the legend that describes it.
//!
//! Semantic context decides eligibility from metadata; this module decodes
//! the overlay's frames through the raw-frame cache, applies each frame's
//! real-world mapping, and caches the encoded PNG per displayed frame.

use super::error::{self, ApiError};
use super::handlers::{semantic_context_for, value_mappings_for};
use super::state::AppState;
use crate::api::contracts::{
    DoseOverlayQuery, OverlayLegend, SemanticContext, SemanticContextResponse, CACHE_HEADER,
    CACHE_HIT, CACHE_MISS, PNG_MEDIA_TYPE,
};
use crate::geometry::frame_geometry;
use crate::pixels::{
    self, ColorScale, ColorwashRequest, PixelError, PixelResult, RawFrameRequest, WeightedPlane,
    COLORMAP_NAME, COLORMAP_STOPS,
};
use crate::plane_stack::{PlaneStack, StackSampleError};
use crate::types::{FileEntry, NativePixelDataKind, OverlayCacheKey};
use crate::value_mapping::{map_value, FileValueMappings};
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use bytes::Bytes;
use dicom_dictionary_std::uids;
use tokio::task;

pub(super) async fn dose_overlay(
    State(state): State<AppState>,
    path: Result<Path<(usize, u32)>, PathRejection>,
    query: Result<Query<DoseOverlayQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Path((index, frame)) = path.map_err(error::path_rejection)?;
    let target = state
        .registry()
        .get(index)
        .ok_or_else(|| ApiError::not_found("file index out of range"))?;
    let Query(query) = query.map_err(error::query_rejection)?;
    let dose = state
        .registry()
        .get(query.dose)
        .ok_or_else(|| ApiError::not_found("dose file index out of range"))?;
    if dose.sop_class_uid != uids::RT_DOSE_STORAGE {
        return Err(ApiError::bad_request("dose must select an RT Dose object"));
    }
    value_overlay(&state, dose, target, frame).await
}

/// Colorize `overlay`'s planes onto one displayed frame of `target`.
async fn value_overlay(
    state: &AppState,
    overlay: FileEntry,
    target: FileEntry,
    frame: u32,
) -> Result<Response, ApiError> {
    if frame >= target.frame_count {
        return Err(error::pixel_error(PixelError::FrameOutOfRange));
    }
    if target.index == overlay.index {
        return Err(ApiError::semantic_mapping_unavailable(
            "an overlay is drawn on image frames, not on its own frames",
        ));
    }
    let context = semantic_context_for(state, overlay.clone(), state.registry().files_snapshot())
        .await
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
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
        StackSampleError::NotParallel => ApiError::semantic_mapping_unavailable(error.to_string()),
    })?;

    let key = OverlayCacheKey {
        overlay_file_index: overlay.index,
        target_file_index: target.index,
        target_frame: frame,
    };
    if let Some(png) = state.cached_overlay(&key) {
        return Ok(png_response(png, true));
    }
    let mappings = value_mappings_for(state, overlay.clone())
        .await
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    let mut planes = Vec::with_capacity(sample.planes.len());
    for (plane_frame, weight) in sample.planes {
        let values = mapped_frame_values(state, &overlay, &mappings, plane_frame)
            .await
            .map_err(error::pixel_error)?;
        planes.push(WeightedPlane { values, weight });
    }
    let scale = ColorScale {
        min: legend.min_value,
        max: legend.max_value,
        transparent_at_or_below: legend.transparent_at_or_below,
    };
    let png = task::spawn_blocking(move || {
        pixels::encode_colorwash_png(ColorwashRequest {
            planes: &planes,
            plane_rows: stack.plane_rows(),
            plane_columns: stack.plane_columns(),
            transform: sample.transform,
            target_rows: target.rows,
            target_columns: target.columns,
            scale,
        })
    })
    .await
    .map_err(|error| ApiError::internal(format!("overlay encoding task failed: {error}")))?
    .map_err(error::pixel_error)?;
    state.cache_overlay(key, png.clone());
    Ok(png_response(png, false))
}

/// The overlay's plane stack and legend, when its context is eligible.
fn overlay_plan(
    overlay: &FileEntry,
    context: &SemanticContextResponse,
) -> Result<(PlaneStack, OverlayLegend), ApiError> {
    let SemanticContext::RtDose(dose) = &context.context else {
        return Err(ApiError::bad_request(
            "value overlays are defined for RT Dose objects",
        ));
    };
    if !dose.overlay.eligible {
        return Err(ApiError::semantic_mapping_unavailable(
            dose.overlay.reason.clone(),
        ));
    }
    let legend = dose
        .legend
        .clone()
        .ok_or_else(|| ApiError::semantic_mapping_unavailable("the dose legend is unavailable"))?;
    let stack = PlaneStack::from_dose_grid(
        overlay.rows,
        overlay.columns,
        overlay.frame_count,
        &dose.geometry,
    )
    .map_err(ApiError::semantic_mapping_unavailable)?;
    Ok((stack, legend))
}

/// Complete an eligible RT Dose context with its legend, which needs the
/// maximum dose of the whole grid so every slice shares one color scale.
/// A grid that cannot be decoded or holds no positive dose makes the
/// overlay ineligible instead.
pub(super) async fn add_overlay_legend(
    state: &AppState,
    file: &FileEntry,
    context: &mut SemanticContextResponse,
) {
    let SemanticContext::RtDose(dose) = &mut context.context else {
        return;
    };
    if !dose.overlay.eligible {
        return;
    }
    match dose_legend(state, file).await {
        Ok(legend) => dose.legend = Some(legend),
        Err(reason) => {
            dose.overlay = crate::semantic::ineligible(&reason);
            dose.overlay_source_frames.clear();
        }
    }
}

async fn dose_legend(state: &AppState, dose: &FileEntry) -> Result<OverlayLegend, String> {
    let mappings = value_mappings_for(state, dose.clone())
        .await
        .map_err(|error| format!("dose metadata could not be read: {error:#}"))?;
    let map = mappings
        .real_world(0)
        .first()
        .ok_or("Dose Grid Scaling is missing or malformed")?
        .clone();
    let mut max = f64::NEG_INFINITY;
    for frame in 0..dose.frame_count {
        let values = mapped_frame_values(state, dose, &mappings, frame)
            .await
            .map_err(|error| format!("dose grid could not be decoded: {error}"))?;
        max = values
            .into_iter()
            .filter(|value| value.is_finite())
            .fold(max, f64::max);
    }
    if max.is_nan() || max <= 0.0 {
        return Err("the dose grid holds no positive dose".to_string());
    }
    Ok(OverlayLegend {
        unit_label: map.unit_label,
        units: map.units,
        min_value: 0.0,
        max_value: max,
        transparent_at_or_below: Some(0.0),
        colormap: COLORMAP_NAME.to_string(),
        color_stops: COLORMAP_STOPS.to_vec(),
    })
}

/// One frame's samples in the units of its preferred real-world mapping;
/// samples outside the mapped range are NaN.
async fn mapped_frame_values(
    state: &AppState,
    file: &FileEntry,
    mappings: &FileValueMappings,
    frame: u32,
) -> PixelResult<Vec<f64>> {
    let map = mappings.real_world(frame).first().cloned().ok_or_else(|| {
        PixelError::UnsupportedLayout(format!("frame {frame} has no real-world value mapping"))
    })?;
    let kind = file
        .series_metadata
        .native_pixel
        .pixel_data_kind
        .unwrap_or(NativePixelDataKind::Integer);
    let raw =
        pixels::load_raw_frame(file.clone(), state.raw_cache(), RawFrameRequest { frame }).await?;
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

fn png_response(png: Bytes, cache_hit: bool) -> Response {
    let mut response = Response::new(axum::body::Body::from(png));
    response.headers_mut().insert(
        CACHE_HEADER,
        HeaderValue::from_static(if cache_hit { CACHE_HIT } else { CACHE_MISS }),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PNG_MEDIA_TYPE),
    );
    response
}
