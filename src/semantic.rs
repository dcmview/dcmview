//! Conservative semantic metadata for objects whose pixels have a domain meaning.
//!
//! This module never changes frame bytes. It reports declared mappings and only
//! marks overlays eligible when identity and patient geometry resolve uniquely.

use crate::api::contracts::{
    CodedConceptSummary, DoseGridGeometry, OverlayEligibility, ParametricMapContext,
    RealWorldValueMappingSummary, ResolvedSegmentSourceFrame, RtDoseContext, SegmentFrameMapping,
    SegmentSummary, SegmentationContext, SemanticContext, SemanticContextResponse,
};
use crate::dicom_values::{read_number, read_numbers, read_string, sequence_items};
use crate::geometry::{
    frame_geometry, grids_overlap, target_to_source_transform, GeometryTolerances,
    PixelAffineTransform,
};
use crate::object_kind::{classify_sop_class, ObjectKind};
use crate::pixels::open_header;
use crate::plane_stack::PlaneStack;
use crate::references::{self, ReferenceCandidate, ReferenceRelationship, ResolvedReferenceEdge};
use crate::types::FileEntry;
use crate::value_mapping::{stored_value_type, FileValueMappings};
use anyhow::{Context, Result};
use dicom_core::Tag;
use dicom_dictionary_std::{tags, uids, StandardDataDictionary};
use dicom_object::InMemDicomObject;
use std::collections::BTreeSet;
use std::sync::Arc;

const MAX_SEQUENCE_ITEMS: usize = 4_096;
const MAX_LUT_VALUES: usize = 4_096;
const MAX_OVERLAY_SOURCE_FRAMES: usize = 4_096;

/// The frame endpoints window stored values (after any Modality rescale):
/// neither Real World Value Mapping nor Dose Grid Scaling is applied to the
/// displayed pixels, so the value kind they show is `stored` for every object.
const DISPLAYED_VALUE_KIND: &str = "stored";

#[derive(Debug, thiserror::Error)]
pub enum SegmentationOverlayError {
    #[error("semantic overlay is only available for segmentation objects")]
    NotSegmentation,
    #[error("segmentation frame is out of range")]
    FrameOutOfRange,
    #[error("segmentation overlay unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Metadata(#[from] anyhow::Error),
}

#[derive(Debug, Clone)]
pub struct SegmentationOverlayPlan {
    pub segmentation_file_index: usize,
    pub segmentation_frame_index: u32,
    pub source_file_index: usize,
    pub source_frame_index: u32,
    pub target_to_segmentation: PixelAffineTransform,
    pub segmentation_type: String,
    pub maximum_fractional_value: Option<u32>,
    pub color: [u8; 3],
}

/// Reject a non-segmentation object or an out-of-range frame before any
/// metadata is read.
pub fn check_segmentation_frame(
    source: &FileEntry,
    frame: u32,
) -> Result<(), SegmentationOverlayError> {
    if classify_sop_class(&source.sop_class_uid) != ObjectKind::Segmentation {
        return Err(SegmentationOverlayError::NotSegmentation);
    }
    if frame >= source.frame_count {
        return Err(SegmentationOverlayError::FrameOutOfRange);
    }
    Ok(())
}

/// Plan one frame's overlay from the segmentation's semantic context, which
/// callers compute once per segmentation rather than once per frame.
pub fn segmentation_overlay_plan(
    source: &FileEntry,
    frame: u32,
    response: &SemanticContextResponse,
    files: &[Arc<FileEntry>],
) -> Result<SegmentationOverlayPlan, SegmentationOverlayError> {
    check_segmentation_frame(source, frame)?;
    let SemanticContext::Segmentation(context) = &response.context else {
        return Err(SegmentationOverlayError::NotSegmentation);
    };
    let mapping = context
        .frame_mappings
        .iter()
        .find(|mapping| mapping.frame_index == frame)
        .ok_or_else(|| {
            SegmentationOverlayError::Unavailable("frame mapping is missing".to_string())
        })?;
    if mapping.mapping_status != "resolved" || mapping.source_frames.len() != 1 {
        return Err(SegmentationOverlayError::Unavailable(
            mapping.mapping_reason.clone(),
        ));
    }
    let resolved_source = &mapping.source_frames[0];
    let target = files
        .iter()
        .find(|file| file.index == resolved_source.file_index)
        .ok_or_else(|| {
            SegmentationOverlayError::Unavailable(
                "the resolved source frame is not available".to_string(),
            )
        })?;
    let segmentation_geometry = frame_geometry(source, frame).ok_or_else(|| {
        SegmentationOverlayError::Unavailable(
            "segmentation frame geometry is incomplete".to_string(),
        )
    })?;
    let target_geometry = frame_geometry(target, resolved_source.frame_index).ok_or_else(|| {
        SegmentationOverlayError::Unavailable("source frame geometry is incomplete".to_string())
    })?;
    let target_to_segmentation = target_to_source_transform(
        segmentation_geometry,
        target_geometry,
        GeometryTolerances::default(),
    )
    .filter(|transform| grids_overlap(segmentation_geometry, target_geometry, *transform))
    .ok_or_else(|| {
        SegmentationOverlayError::Unavailable(
            "source and segmentation grids are not compatibly coplanar".to_string(),
        )
    })?;
    let segmentation_type = context
        .segmentation_type
        .clone()
        .unwrap_or_else(|| "UNKNOWN".to_string());
    if !matches!(segmentation_type.as_str(), "BINARY" | "FRACTIONAL") {
        return Err(SegmentationOverlayError::Unavailable(format!(
            "segmentation type {segmentation_type} is not supported for overlays"
        )));
    }
    let color = mapping
        .segment_number
        .and_then(|number| {
            context
                .segments
                .iter()
                .find(|segment| segment.number == number)
        })
        .map(|segment| segment.display_color)
        .unwrap_or_else(|| fallback_segment_color(mapping.segment_number.unwrap_or(1)));
    Ok(SegmentationOverlayPlan {
        segmentation_file_index: source.index,
        segmentation_frame_index: frame,
        source_file_index: resolved_source.file_index,
        source_frame_index: resolved_source.frame_index,
        target_to_segmentation,
        segmentation_type,
        maximum_fractional_value: context.maximum_fractional_value,
        color,
    })
}

/// The overlay color of one segment and where it comes from: the
/// Recommended Display CIELab Value when it holds three PCS-values, else the
/// Recommended Display Grayscale Value, else a fixed palette cycled by
/// segment number. The grayscale value is a P-Value from 0 (black) to FFFFH
/// (white), shown as the proportional 8-bit gray level.
fn segment_display_color(
    number: u16,
    cielab: Option<&[u16]>,
    grayscale: Option<u16>,
) -> ([u8; 3], &'static str) {
    if let Some(&[l, a, b]) = cielab {
        return (
            crate::pixels::cielab_to_srgb8([l, a, b]),
            "recommended_cielab",
        );
    }
    if let Some(gray) = grayscale {
        let level = (f64::from(gray) * 255.0 / 65_535.0).round() as u8;
        return ([level; 3], "recommended_grayscale");
    }
    (fallback_segment_color(number), "palette")
}

fn fallback_segment_color(segment_number: u16) -> [u8; 3] {
    const COLORS: [[u8; 3]; 6] = [
        [255, 79, 132],
        [42, 211, 199],
        [255, 190, 92],
        [136, 132, 255],
        [114, 218, 111],
        [255, 126, 92],
    ];
    COLORS[usize::from(segment_number.saturating_sub(1)) % COLORS.len()]
}

pub fn semantic_context(
    source: &FileEntry,
    files: &[Arc<FileEntry>],
) -> Result<SemanticContextResponse> {
    // Float Pixel Data precedes Pixel Data, so stopping at the first pixel
    // element keeps a float Parametric Map's samples out of memory.
    let object = open_header(&source.path).context("failed to open semantic metadata")?;
    let candidates = files
        .iter()
        .map(|file| ReferenceCandidate::from_file(file))
        .collect::<Vec<_>>();
    let edges = references::extract_reference_edges_from_object(&object);
    let resolved = references::resolve_reference_edges(&edges, &candidates);

    let context = match classify_sop_class(&source.sop_class_uid) {
        ObjectKind::Segmentation => {
            SemanticContext::Segmentation(segmentation_context(source, &object, files, &resolved))
        }
        ObjectKind::ParametricMap => SemanticContext::ParametricMap(Box::new(
            parametric_map_context(source, &object, files, &resolved),
        )),
        ObjectKind::RadiationTherapy if source.sop_class_uid == uids::RT_DOSE_STORAGE => {
            SemanticContext::RtDose(Box::new(rt_dose_context(source, &object, files, &resolved)))
        }
        _ => SemanticContext::NotApplicable {
            reason: "semantic context is only defined for SEG, Parametric Map, and RT Dose"
                .to_string(),
        },
    };
    Ok(SemanticContextResponse {
        source_file_index: source.index,
        default_mode: "pixel_preview".to_string(),
        pixel_preview_preserves_stored_values: true,
        context,
    })
}

fn segmentation_context(
    source: &FileEntry,
    object: &InMemDicomObject<StandardDataDictionary>,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> SegmentationContext {
    let segments = sequence_items(object, tags::SEGMENT_SEQUENCE)
        .iter()
        .take(MAX_SEQUENCE_ITEMS)
        .filter_map(|item| {
            let number = read_number::<u16>(item, tags::SEGMENT_NUMBER)?;
            let recommended_display_cielab =
                optional_numbers(item, tags::RECOMMENDED_DISPLAY_CIE_LAB_VALUE);
            let recommended_display_grayscale =
                read_number(item, tags::RECOMMENDED_DISPLAY_GRAYSCALE_VALUE);
            let (display_color, display_color_source) = segment_display_color(
                number,
                recommended_display_cielab.as_deref(),
                recommended_display_grayscale,
            );
            Some(SegmentSummary {
                number,
                label: read_string(item, tags::SEGMENT_LABEL),
                description: read_string(item, tags::SEGMENT_DESCRIPTION),
                property_category: read_code(item, tags::SEGMENTED_PROPERTY_CATEGORY_CODE_SEQUENCE),
                property_type: read_code(item, tags::SEGMENTED_PROPERTY_TYPE_CODE_SEQUENCE),
                algorithm_type: read_string(item, tags::SEGMENT_ALGORITHM_TYPE),
                algorithm_name: read_string(item, tags::SEGMENT_ALGORITHM_NAME),
                recommended_display_cielab,
                recommended_display_grayscale,
                display_color,
                display_color_source: display_color_source.to_string(),
            })
        })
        .collect::<Vec<_>>();

    let mut frame_mappings = Vec::new();
    let shared_group = sequence_items(object, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE).first();
    let declared_sources = sequence_items(object, tags::SOURCE_IMAGE_SEQUENCE);
    for (frame_index, frame_group) in
        sequence_items(object, tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
            .iter()
            .take(source.frame_count as usize)
            .enumerate()
    {
        let segment_number = referenced_segment_number(frame_group, shared_group);
        let explicit_source_items = sequence_items(frame_group, tags::DERIVATION_IMAGE_SEQUENCE)
            .iter()
            .flat_map(|item| sequence_items(item, tags::SOURCE_IMAGE_SEQUENCE))
            .collect::<Vec<_>>();
        let (source_frames, mapping_method, mapping_status, mapping_reason) =
            if explicit_source_items.is_empty() {
                resolve_geometry_sources(source, frame_index as u32, files, declared_sources)
            } else {
                resolve_explicit_sources(source, frame_index as u32, files, &explicit_source_items)
            };
        let source_sop_instance_uids = source_frames
            .iter()
            .map(|mapping| mapping.sop_instance_uid.clone())
            .collect::<BTreeSet<_>>();
        let source_sop_instance_uid = (source_sop_instance_uids.len() == 1)
            .then(|| source_sop_instance_uids.into_iter().next())
            .flatten();
        let source_frame_numbers = source_frames
            .iter()
            .map(|mapping| mapping.frame_index + 1)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let source_file_indices = source_frames
            .iter()
            .map(|mapping| mapping.file_index)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        frame_mappings.push(SegmentFrameMapping {
            frame_index: frame_index as u32,
            segment_number,
            source_sop_instance_uid,
            source_frame_numbers,
            source_file_indices,
            source_frames,
            mapping_method,
            mapping_status,
            mapping_reason,
        });
    }

    let declared_segments = segments
        .iter()
        .map(|segment| segment.number)
        .collect::<Vec<_>>();
    let segment_closure_valid = frame_mappings.iter().all(|mapping| {
        mapping
            .segment_number
            .is_some_and(|number| declared_segments.contains(&number))
    });
    let overlay = segmentation_overlay(
        source,
        files,
        resolved,
        &frame_mappings,
        segment_closure_valid,
    );
    SegmentationContext {
        segmentation_type: read_string(object, tags::SEGMENTATION_TYPE),
        segmentation_fractional_type: read_string(object, tags::SEGMENTATION_FRACTIONAL_TYPE),
        maximum_fractional_value: read_number(object, tags::MAXIMUM_FRACTIONAL_VALUE),
        segments,
        frame_mappings,
        references: resolved
            .iter()
            .map(ResolvedReferenceEdge::summary)
            .collect(),
        overlay,
    }
}

fn referenced_segment_number(
    frame_group: &InMemDicomObject<StandardDataDictionary>,
    shared_group: Option<&InMemDicomObject<StandardDataDictionary>>,
) -> Option<u16> {
    [Some(frame_group), shared_group]
        .into_iter()
        .flatten()
        .find_map(|group| {
            sequence_items(group, tags::SEGMENT_IDENTIFICATION_SEQUENCE)
                .first()
                .and_then(|item| read_number(item, tags::REFERENCED_SEGMENT_NUMBER))
        })
}

fn segmentation_overlay(
    source: &FileEntry,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
    frame_mappings: &[SegmentFrameMapping],
    segment_closure_valid: bool,
) -> OverlayEligibility {
    if !segment_closure_valid {
        return ineligible("a frame references a missing or undeclared segment number");
    }
    if frame_mappings.len() != source.frame_count as usize || frame_mappings.is_empty() {
        return ineligible("per-frame segment/source mapping is incomplete");
    }
    if frame_mappings
        .iter()
        .any(|mapping| mapping.mapping_status != "resolved" || mapping.source_frames.len() != 1)
    {
        return ineligible("one or more segmentation frames lack a unique source-frame mapping");
    }
    if frame_mappings.iter().any(|mapping| {
        let resolved_source = &mapping.source_frames[0];
        files
            .iter()
            .find(|file| file.index == resolved_source.file_index)
            .is_none_or(|target| {
                !frame_geometrically_compatible(
                    source,
                    mapping.frame_index,
                    target,
                    resolved_source.frame_index,
                )
            })
    }) {
        return ineligible("one or more source frames have incompatible patient geometry");
    }
    let source_indices = frame_mappings
        .iter()
        .map(|mapping| mapping.source_frames[0].file_index)
        .collect::<BTreeSet<_>>();
    let Some(&first_target_index) = source_indices.first() else {
        return ineligible("no frame-level source image mapping is declared");
    };
    let references_close_all_mappings = frame_mappings.iter().all(|mapping| {
        let source_frame = &mapping.source_frames[0];
        resolved.iter().any(|edge| {
            matches!(
                edge.relationship,
                ReferenceRelationship::SourceImage
                    | ReferenceRelationship::SourceImageForSegmentation
            ) && edge
                .matches
                .iter()
                .any(|candidate| candidate.file_index == source_frame.file_index)
        })
    });
    if !references_close_all_mappings {
        return ineligible("frame mapping is not closed by a declared source reference");
    }
    OverlayEligibility {
        eligible: true,
        reason: "every segmentation frame has a unique declared, geometry-compatible source frame"
            .to_string(),
        source_file_index: (source_indices.len() == 1).then_some(first_target_index),
        mapped_source_count: source_indices.len(),
    }
}

fn resolve_explicit_sources(
    segmentation: &FileEntry,
    segmentation_frame: u32,
    files: &[Arc<FileEntry>],
    source_items: &[&InMemDicomObject<StandardDataDictionary>],
) -> (
    Vec<ResolvedSegmentSourceFrame>,
    Option<String>,
    String,
    String,
) {
    let mut mappings = Vec::new();
    for item in source_items {
        let Some(uid) = read_string(item, tags::REFERENCED_SOP_INSTANCE_UID) else {
            continue;
        };
        let declared_frames = read_numbers::<u32>(item, tags::REFERENCED_FRAME_NUMBER);
        for file in files.iter().filter(|file| file.sop_instance_uid == uid) {
            let candidate_frames = if declared_frames.is_empty() {
                (0..file.frame_count).collect::<Vec<_>>()
            } else {
                declared_frames
                    .iter()
                    .filter_map(|number| number.checked_sub(1))
                    .filter(|frame| *frame < file.frame_count)
                    .collect()
            };
            let compatible = candidate_frames
                .iter()
                .copied()
                .filter(|source_frame| {
                    frame_geometrically_compatible(
                        segmentation,
                        segmentation_frame,
                        file,
                        *source_frame,
                    )
                })
                .collect::<Vec<_>>();
            let selected = if compatible.is_empty() && candidate_frames.len() == 1 {
                candidate_frames
            } else {
                compatible
            };
            mappings.extend(
                selected
                    .into_iter()
                    .map(|frame_index| ResolvedSegmentSourceFrame {
                        file_index: file.index,
                        frame_index,
                        sop_instance_uid: file.sop_instance_uid.clone(),
                    }),
            );
        }
    }
    finish_frame_mapping(mappings, "explicit_derivation")
}

fn resolve_geometry_sources(
    segmentation: &FileEntry,
    segmentation_frame: u32,
    files: &[Arc<FileEntry>],
    source_items: &[InMemDicomObject<StandardDataDictionary>],
) -> (
    Vec<ResolvedSegmentSourceFrame>,
    Option<String>,
    String,
    String,
) {
    let declared_uids = source_items
        .iter()
        .filter_map(|item| read_string(item, tags::REFERENCED_SOP_INSTANCE_UID))
        .collect::<BTreeSet<_>>();
    if declared_uids.is_empty() {
        return (
            Vec::new(),
            None,
            "missing".to_string(),
            "no per-frame derivation or top-level source images are declared".to_string(),
        );
    }
    let mappings = files
        .iter()
        .filter(|file| declared_uids.contains(&file.sop_instance_uid))
        .flat_map(|file| {
            (0..file.frame_count)
                .filter(move |frame_index| {
                    frame_geometrically_compatible(
                        segmentation,
                        segmentation_frame,
                        file,
                        *frame_index,
                    )
                })
                .map(|frame_index| ResolvedSegmentSourceFrame {
                    file_index: file.index,
                    frame_index,
                    sop_instance_uid: file.sop_instance_uid.clone(),
                })
        })
        .collect();
    finish_frame_mapping(mappings, "declared_source_geometry")
}

fn finish_frame_mapping(
    mut mappings: Vec<ResolvedSegmentSourceFrame>,
    method: &str,
) -> (
    Vec<ResolvedSegmentSourceFrame>,
    Option<String>,
    String,
    String,
) {
    mappings.sort_by_key(|mapping| (mapping.file_index, mapping.frame_index));
    mappings.dedup_by_key(|mapping| (mapping.file_index, mapping.frame_index));
    let (status, reason) = match mappings.len() {
        0 => (
            "missing",
            "no local source frame satisfies the declared mapping",
        ),
        1 => ("resolved", "one local source frame is uniquely resolved"),
        _ => (
            "ambiguous",
            "multiple local source frames satisfy the declared mapping",
        ),
    };
    (
        mappings,
        Some(method.to_string()),
        status.to_string(),
        reason.to_string(),
    )
}

fn frame_geometrically_compatible(
    segmentation: &FileEntry,
    segmentation_frame: u32,
    source: &FileEntry,
    source_frame: u32,
) -> bool {
    if segmentation
        .series_metadata
        .frame_of_reference_uid
        .is_empty()
        || segmentation.series_metadata.frame_of_reference_uid
            != source.series_metadata.frame_of_reference_uid
    {
        return false;
    }
    let Some(segmentation_geometry) = frame_geometry(segmentation, segmentation_frame) else {
        return false;
    };
    let Some(source_geometry) = frame_geometry(source, source_frame) else {
        return false;
    };
    let Some(transform) = target_to_source_transform(
        segmentation_geometry,
        source_geometry,
        GeometryTolerances::default(),
    ) else {
        return false;
    };
    grids_overlap(segmentation_geometry, source_geometry, transform)
}

fn parametric_map_context(
    source: &FileEntry,
    object: &InMemDicomObject<StandardDataDictionary>,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> ParametricMapContext {
    let mut mappings = Vec::new();
    collect_rwvm_mappings(object, "embedded", None, &mut mappings);
    let mut warnings = Vec::new();
    for reference in referenced_rwvm_instances(object) {
        let matches = files
            .iter()
            .filter(|file| file.sop_instance_uid == reference)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [file] => match open_header(&file.path) {
                Ok(mapping_object) => {
                    let before = mappings.len();
                    collect_rwvm_mappings(
                        &mapping_object,
                        "referenced",
                        Some(reference.as_str()),
                        &mut mappings,
                    );
                    if mappings.len() == before {
                        warnings.push(format!(
                            "referenced RWVM {reference} contains no usable mapping"
                        ));
                    }
                }
                Err(_) => warnings.push(format!("referenced RWVM {reference} could not be read")),
            },
            [] => warnings.push(format!("referenced RWVM {reference} is missing")),
            _ => warnings.push(format!("referenced RWVM {reference} resolves ambiguously")),
        }
    }
    let mapping_status = if mappings.is_empty() {
        "unmapped"
    } else if mappings.iter().any(valid_mapping) {
        "mapping_available"
    } else {
        "incompatible_mapping"
    };
    let (overlay, overlay_source_frames) = parametric_map_overlay(source, object, files, resolved);
    ParametricMapContext {
        stored_value_type: stored_value_type(source).to_string(),
        displayed_value_kind: DISPLAYED_VALUE_KIND.to_string(),
        mappings,
        mapping_status: mapping_status.to_string(),
        source_references: resolved
            .iter()
            .map(ResolvedReferenceEdge::summary)
            .collect(),
        warnings,
        overlay,
        overlay_source_frames,
        // The legend spans the mapped values of every frame; the server
        // fills it in from decoded frames once the overlay is eligible.
        legend: None,
    }
}

/// A Parametric Map overlays its source images when every frame carries a
/// real-world mapping in one unit and the frames form a plane stack.
fn parametric_map_overlay(
    map: &FileEntry,
    object: &InMemDicomObject<StandardDataDictionary>,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> (OverlayEligibility, Vec<ResolvedSegmentSourceFrame>) {
    if !map.has_pixels {
        return (
            ineligible("the parametric map has no pixel data"),
            Vec::new(),
        );
    }
    let mappings = FileValueMappings::from_object(map, object);
    let Some(units) = mappings.real_world(0).next().map(|map| &map.unit_label) else {
        return (
            ineligible("the first frame has no usable Real World Value Mapping"),
            Vec::new(),
        );
    };
    if (1..map.frame_count).any(|frame| {
        mappings
            .real_world(frame)
            .next()
            .is_none_or(|mapping| mapping.unit_label != *units)
    }) {
        return (
            ineligible("frames lack a Real World Value Mapping or map to different units"),
            Vec::new(),
        );
    }
    match PlaneStack::from_frames(map) {
        Ok(stack) => value_overlay_sources(map, &stack, files, resolved),
        Err(reason) => (ineligible(&reason), Vec::new()),
    }
}

fn rt_dose_context(
    source: &FileEntry,
    object: &InMemDicomObject<StandardDataDictionary>,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> RtDoseContext {
    let scaling = read_number::<f64>(object, tags::DOSE_GRID_SCALING)
        .filter(|value| value.is_finite() && *value > 0.0);
    let geometry = DoseGridGeometry {
        frame_of_reference_uid: read_string(object, tags::FRAME_OF_REFERENCE_UID),
        image_position_patient: fixed_numbers(object, tags::IMAGE_POSITION_PATIENT),
        image_orientation_patient: fixed_numbers(object, tags::IMAGE_ORIENTATION_PATIENT),
        pixel_spacing: fixed_numbers(object, tags::PIXEL_SPACING),
        grid_frame_offsets: read_numbers(object, tags::GRID_FRAME_OFFSET_VECTOR),
    };
    let (overlay, overlay_source_frames) =
        rt_dose_overlay(source, scaling, &geometry, files, resolved);
    RtDoseContext {
        dose_grid_scaling: scaling,
        scaling_status: if scaling.is_some() { "available" } else { "missing_or_malformed" }
            .to_string(),
        displayed_value_kind: DISPLAYED_VALUE_KIND.to_string(),
        dose_units: read_string(object, tags::DOSE_UNITS),
        dose_type: read_string(object, tags::DOSE_TYPE),
        dose_summation_type: read_string(object, tags::DOSE_SUMMATION_TYPE),
        geometry,
        references: resolved.iter().map(ResolvedReferenceEdge::summary).collect(),
        overlay,
        overlay_source_frames,
        // The legend needs the whole grid's maximum; the server fills it in
        // from decoded frames once the overlay is known to be eligible.
        legend: None,
        clinical_use_warning:
            "Semantic context does not establish prescription correctness or clinical acceptability."
                .to_string(),
    }
}

fn rt_dose_overlay(
    dose: &FileEntry,
    scaling: Option<f64>,
    geometry: &DoseGridGeometry,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> (OverlayEligibility, Vec<ResolvedSegmentSourceFrame>) {
    if scaling.is_none() {
        return (
            ineligible("Dose Grid Scaling is missing or malformed"),
            Vec::new(),
        );
    }
    if !dose.has_pixels {
        return (ineligible("the dose object has no pixel data"), Vec::new());
    }
    match PlaneStack::from_dose_grid(dose.rows, dose.columns, dose.frame_count, geometry) {
        Ok(stack) => value_overlay_sources(dose, &stack, files, resolved),
        Err(reason) => (ineligible(&reason), Vec::new()),
    }
}

/// The local image frames a value overlay (dose grid or Parametric Map
/// planes) covers: frames of other image objects in the overlay's Frame of
/// Reference that its plane stack samples. Eligible when any is covered.
fn value_overlay_sources(
    overlay: &FileEntry,
    stack: &PlaneStack,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> (OverlayEligibility, Vec<ResolvedSegmentSourceFrame>) {
    let frame_of_reference = &overlay.series_metadata.frame_of_reference_uid;
    if frame_of_reference.is_empty() {
        return (
            ineligible("the overlay object declares no Frame of Reference"),
            Vec::new(),
        );
    }
    let mut frames = Vec::new();
    let mut covered_files = BTreeSet::new();
    for file in files.iter().filter(|file| {
        file.index != overlay.index
            && file.has_pixels
            && file.sop_class_uid != overlay.sop_class_uid
            && file.series_metadata.frame_of_reference_uid == *frame_of_reference
    }) {
        for frame_index in 0..file.frame_count {
            if frame_geometry(file, frame_index).is_some_and(|target| stack.sample(target).is_ok())
            {
                covered_files.insert(file.index);
                if frames.len() < MAX_OVERLAY_SOURCE_FRAMES {
                    frames.push(ResolvedSegmentSourceFrame {
                        file_index: file.index,
                        frame_index,
                        sop_instance_uid: file.sop_instance_uid.clone(),
                    });
                }
            }
        }
    }
    let Some(&only_file) = covered_files.first() else {
        return (
            ineligible("no local image frame in the overlay's Frame of Reference lies within it"),
            Vec::new(),
        );
    };
    let declared_sources = resolved
        .iter()
        .filter(|edge| edge.relationship == ReferenceRelationship::SourceImage)
        .flat_map(|edge| edge.matches.iter())
        .map(|candidate| candidate.file_index)
        .filter(|index| covered_files.contains(index))
        .collect::<BTreeSet<_>>();
    let source_file_index = match (declared_sources.len(), covered_files.len()) {
        (1, _) => declared_sources.first().copied(),
        (_, 1) => Some(only_file),
        _ => None,
    };
    let covered_frames = frames.len();
    (
        OverlayEligibility {
            eligible: true,
            reason: format!(
                "{covered_frames} local image frame(s) in the overlay's Frame of Reference lie within it"
            ),
            source_file_index,
            mapped_source_count: covered_files.len(),
        },
        frames,
    )
}

fn collect_rwvm_mappings(
    object: &InMemDicomObject<StandardDataDictionary>,
    source: &str,
    source_sop_instance_uid: Option<&str>,
    output: &mut Vec<RealWorldValueMappingSummary>,
) {
    for element in object.iter() {
        let Some(items) = element.items() else {
            continue;
        };
        if element.header().tag == tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE {
            output.extend(
                items
                    .iter()
                    .take(MAX_SEQUENCE_ITEMS)
                    .map(|item| rwvm_mapping(item, source, source_sop_instance_uid)),
            );
        } else {
            for item in items.iter().take(MAX_SEQUENCE_ITEMS) {
                collect_rwvm_mappings(item, source, source_sop_instance_uid, output);
            }
        }
    }
}

pub(crate) fn rwvm_mapping(
    item: &InMemDicomObject<StandardDataDictionary>,
    source: &str,
    source_sop_instance_uid: Option<&str>,
) -> RealWorldValueMappingSummary {
    let mut lut_data = read_numbers(item, tags::REAL_WORLD_VALUE_LUT_DATA);
    let lut_data_truncated = lut_data.len() > MAX_LUT_VALUES;
    lut_data.truncate(MAX_LUT_VALUES);
    // Float and double-float pixel data declare their range with the
    // double-float bounds instead of the US/SS ones.
    RealWorldValueMappingSummary {
        source: source.to_string(),
        source_sop_instance_uid: source_sop_instance_uid.map(str::to_string),
        label: read_string(item, tags::LUT_LABEL),
        first_value_mapped: read_number(item, tags::REAL_WORLD_VALUE_FIRST_VALUE_MAPPED)
            .or_else(|| read_number(item, tags::DOUBLE_FLOAT_REAL_WORLD_VALUE_FIRST_VALUE_MAPPED)),
        last_value_mapped: read_number(item, tags::REAL_WORLD_VALUE_LAST_VALUE_MAPPED)
            .or_else(|| read_number(item, tags::DOUBLE_FLOAT_REAL_WORLD_VALUE_LAST_VALUE_MAPPED)),
        slope: read_number(item, tags::REAL_WORLD_VALUE_SLOPE),
        intercept: read_number(item, tags::REAL_WORLD_VALUE_INTERCEPT),
        lut_data,
        lut_data_truncated,
        units: read_code(item, tags::MEASUREMENT_UNITS_CODE_SEQUENCE),
        quantity: read_quantity_code(item),
        derivation: read_code(item, tags::DERIVATION_CODE_SEQUENCE),
    }
}

fn read_quantity_code(
    object: &InMemDicomObject<StandardDataDictionary>,
) -> Option<CodedConceptSummary> {
    sequence_items(object, tags::QUANTITY_DEFINITION_SEQUENCE)
        .iter()
        .find_map(|item| {
            read_code(item, tags::CONCEPT_CODE_SEQUENCE).or_else(|| read_direct_code(item))
        })
}

pub(crate) fn valid_mapping(mapping: &RealWorldValueMappingSummary) -> bool {
    let range_valid = matches!(
        (mapping.first_value_mapped, mapping.last_value_mapped),
        (Some(first), Some(last)) if first.is_finite() && last.is_finite() && first <= last
    );
    let linear = matches!(
        (mapping.slope, mapping.intercept),
        (Some(slope), Some(intercept)) if slope.is_finite() && intercept.is_finite()
    );
    range_valid && (linear || !mapping.lut_data.is_empty()) && mapping.units.is_some()
}

fn referenced_rwvm_instances(object: &InMemDicomObject<StandardDataDictionary>) -> Vec<String> {
    sequence_items(
        object,
        tags::REFERENCED_REAL_WORLD_VALUE_MAPPING_INSTANCE_SEQUENCE,
    )
    .iter()
    .filter_map(|item| read_string(item, tags::REFERENCED_SOP_INSTANCE_UID))
    .collect()
}

pub(crate) fn ineligible(reason: &str) -> OverlayEligibility {
    OverlayEligibility {
        eligible: false,
        reason: reason.to_string(),
        source_file_index: None,
        mapped_source_count: 0,
    }
}

fn read_code(
    object: &InMemDicomObject<StandardDataDictionary>,
    tag: Tag,
) -> Option<CodedConceptSummary> {
    read_direct_code(sequence_items(object, tag).first()?)
}

fn read_direct_code(
    item: &InMemDicomObject<StandardDataDictionary>,
) -> Option<CodedConceptSummary> {
    let value = read_string(item, tags::CODE_VALUE)
        .or_else(|| read_string(item, tags::LONG_CODE_VALUE))
        .or_else(|| read_string(item, tags::URN_CODE_VALUE))?;
    Some(CodedConceptSummary {
        value,
        scheme: read_string(item, tags::CODING_SCHEME_DESIGNATOR).unwrap_or_default(),
        meaning: read_string(item, tags::CODE_MEANING).unwrap_or_default(),
    })
}

fn optional_numbers<T>(
    object: &InMemDicomObject<StandardDataDictionary>,
    tag: Tag,
) -> Option<Vec<T>>
where
    T: std::str::FromStr,
{
    let values = read_numbers(object, tag);
    (!values.is_empty()).then_some(values)
}

fn fixed_numbers<const N: usize>(
    object: &InMemDicomObject<StandardDataDictionary>,
    tag: Tag,
) -> Option<[f64; N]> {
    read_numbers::<f64>(object, tag).try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::{referenced_segment_number, segment_display_color};
    use dicom_core::{value::DataSetSequence, DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::tags;
    use dicom_object::InMemDicomObject;

    fn group(segment_number: u16) -> InMemDicomObject {
        let identification = InMemDicomObject::from_element_iter([DataElement::new(
            tags::REFERENCED_SEGMENT_NUMBER,
            VR::US,
            PrimitiveValue::U16(vec![segment_number].into()),
        )]);
        InMemDicomObject::from_element_iter([DataElement::new(
            tags::SEGMENT_IDENTIFICATION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![identification]),
        )])
    }

    #[test]
    fn per_frame_segment_number_overrides_shared_functional_group() {
        let per_frame = group(2);
        let shared = group(1);
        assert_eq!(
            referenced_segment_number(&per_frame, Some(&shared)),
            Some(2)
        );
    }

    #[test]
    fn shared_segment_number_applies_when_per_frame_group_omits_it() {
        let per_frame = InMemDicomObject::new_empty();
        let shared = group(1);
        assert_eq!(
            referenced_segment_number(&per_frame, Some(&shared)),
            Some(1)
        );
    }

    #[test]
    fn segment_color_prefers_cielab_then_grayscale_then_palette() {
        let white = [0xFFFF, 0x8080, 0x8080];
        assert_eq!(
            segment_display_color(2, Some(&white), Some(0)),
            ([255, 255, 255], "recommended_cielab")
        );
        // A malformed CIELab value falls through to the grayscale value.
        assert_eq!(
            segment_display_color(2, Some(&[1, 2]), Some(0x8000)),
            ([128, 128, 128], "recommended_grayscale")
        );
        assert_eq!(
            segment_display_color(2, None, None),
            ([42, 211, 199], "palette")
        );
        assert_eq!(segment_display_color(7, None, None).0, [255, 79, 132]);
    }
}
