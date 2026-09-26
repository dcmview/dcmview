//! Selected-frame Whole Slide Microscopy positioning metadata.
//!
//! The output describes one decoded tile only. It never decodes neighbors,
//! stitches a mosaic, or represents metadata as reconstructed slide pixels.

use crate::api::contracts::{
    ReferenceMatchSummary, ReferenceSummary, ReferenceTargetSummary, WsiCompanionSummary,
    WsiFocalPlane, WsiFrameContextResponse, WsiOpticalPath, WsiTileRectangle, WsiTotalPixelMatrix,
};
use crate::dicom_values::{read_number, read_string, sequence_item, sequence_items};
use crate::object_kind::{classify_sop_class, ObjectKind};
use crate::references::{self, ReferenceCandidate, ResolvedReferenceEdge};
use crate::types::FileEntry;
use anyhow::{Context, Result};
use dicom_dictionary_std::{tags, StandardDataDictionary};
use dicom_object::{InMemDicomObject, OpenFileOptions};
use std::collections::BTreeSet;
use std::sync::Arc;

const MAX_COMPANIONS: usize = 64;
const MAX_RELATIONSHIPS: usize = 128;
const MAX_FOCAL_METADATA_SCAN: usize = 4_096;

pub fn frame_context(
    source: &FileEntry,
    frame: u32,
    files: &[Arc<FileEntry>],
) -> Result<WsiFrameContextResponse> {
    let object = OpenFileOptions::new()
        .read_until(tags::PIXEL_DATA)
        .open_file(&source.path)
        .with_context(|| format!("failed to open WSI metadata: {}", source.path.display()))?;
    Ok(frame_context_from_object(source, frame, files, &object))
}

fn frame_context_from_object(
    source: &FileEntry,
    frame: u32,
    files: &[Arc<FileEntry>],
    object: &InMemDicomObject<StandardDataDictionary>,
) -> WsiFrameContextResponse {
    let metadata = &source.series_metadata;
    let matrix = match (
        metadata.total_pixel_matrix_rows.filter(|value| *value > 0),
        metadata
            .total_pixel_matrix_columns
            .filter(|value| *value > 0),
    ) {
        (Some(rows), Some(columns)) => Some(WsiTotalPixelMatrix {
            rows: rows.into(),
            columns: columns.into(),
        }),
        _ => None,
    };
    let tiling_status = match metadata.dimension_organization_type.as_deref() {
        Some("TILED_FULL") => "full",
        Some("TILED_SPARSE") => "sparse",
        Some(_) => "unknown",
        None => "unknown",
    };
    let mut warnings = Vec::new();
    let placement = if tiling_status == "full" {
        tiled_full_placement(source, frame, matrix.as_ref(), &mut warnings)
    } else {
        sparse_placement(source, frame, object, matrix.as_ref(), &mut warnings)
    };
    let (companions, companions_truncated) = companions(source, files);
    let (relationships, relationships_truncated) = relationships(source, object, files);

    WsiFrameContextResponse {
        source_file_index: source.index,
        frame_index: frame,
        tile_frame_path: format!("/api/file/{}/frame/{frame}", source.index),
        positioning_status: if placement.rectangle.is_some() {
            "positioned"
        } else {
            "unavailable"
        }
        .to_string(),
        position_source: placement.source.to_string(),
        tiling_status: tiling_status.to_string(),
        total_pixel_matrix: matrix,
        tile_rectangle: placement.rectangle,
        tile_row: placement.tile_row,
        tile_column: placement.tile_column,
        pyramid_uid: metadata.pyramid_uid.clone(),
        pyramid_level: pyramid_level(source, files),
        optical_path: placement.optical_path,
        focal_plane: placement.focal_plane,
        image_type_role: metadata.image_type.get(2).cloned(),
        companions,
        companions_truncated,
        relationships,
        relationships_truncated,
        reconstruction_claimed: false,
        warnings,
    }
}

struct Placement {
    source: &'static str,
    rectangle: Option<WsiTileRectangle>,
    tile_row: Option<u64>,
    tile_column: Option<u64>,
    optical_path: Option<WsiOpticalPath>,
    focal_plane: Option<WsiFocalPlane>,
}

fn tiled_full_placement(
    file: &FileEntry,
    frame: u32,
    matrix: Option<&WsiTotalPixelMatrix>,
    warnings: &mut Vec<String>,
) -> Placement {
    let metadata = &file.series_metadata;
    let Some(matrix) = matrix else {
        warnings.push("Total Pixel Matrix dimensions are missing or invalid".to_string());
        return unavailable("unavailable");
    };
    let (tile_rows, tile_columns) = (u64::from(file.rows), u64::from(file.columns));
    let Some((tiles_down, tiles_across)) = nonzero_grid(matrix, tile_rows, tile_columns) else {
        warnings.push("tile dimensions are missing or invalid".to_string());
        return unavailable("unavailable");
    };
    let optical_count = metadata.number_of_optical_paths.filter(|value| *value > 0);
    let focal_count = metadata
        .total_pixel_matrix_focal_planes
        .filter(|value| *value > 0);
    let (Some(optical_count), Some(focal_count)) = (optical_count, focal_count) else {
        warnings.push(
            "TILED_FULL positioning requires declared optical-path and focal-plane counts"
                .to_string(),
        );
        return unavailable("unavailable");
    };
    let tiles_per_plane = match tiles_down.checked_mul(tiles_across) {
        Some(value) if value > 0 => value,
        _ => return unavailable("unavailable"),
    };
    let declared_frames = tiles_per_plane
        .checked_mul(u64::from(optical_count))
        .and_then(|value| value.checked_mul(u64::from(focal_count)));
    if declared_frames.is_none_or(|count| u64::from(frame) >= count) {
        warnings.push("selected frame exceeds the declared TILED_FULL organization".to_string());
        return unavailable("unavailable");
    }

    // DICOM TILED_FULL uses implicit raster order within an optical/focal plane.
    let frame = u64::from(frame);
    let spatial = frame % tiles_per_plane;
    let tile_row = spatial / tiles_across;
    let tile_column = spatial % tiles_across;
    let optical_index = ((frame / tiles_per_plane) % u64::from(optical_count)) as u32;
    let focal_index = (frame / (tiles_per_plane * u64::from(optical_count))) as u32;
    Placement {
        source: "dicom_tiled_full_raster",
        rectangle: rectangle(matrix, tile_row, tile_column, tile_rows, tile_columns),
        tile_row: Some(tile_row),
        tile_column: Some(tile_column),
        optical_path: Some(WsiOpticalPath {
            index: Some(optical_index),
            identifier: metadata
                .optical_path_identifiers
                .get(optical_index as usize)
                .cloned(),
        }),
        focal_plane: Some(WsiFocalPlane {
            index: Some(focal_index),
            z_offset_slide: None,
        }),
    }
}

fn sparse_placement(
    file: &FileEntry,
    frame: u32,
    object: &InMemDicomObject<StandardDataDictionary>,
    matrix: Option<&WsiTotalPixelMatrix>,
    warnings: &mut Vec<String>,
) -> Placement {
    let Some(matrix) = matrix else {
        warnings.push("Total Pixel Matrix dimensions are missing or invalid".to_string());
        return unavailable("unavailable");
    };
    let Some(frame_group) = sequence_item(
        object,
        tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
        frame as usize,
    ) else {
        warnings.push("selected frame has no Per-Frame Functional Group metadata".to_string());
        return unavailable("unavailable");
    };
    let Some(position) = sequence_items(frame_group, tags::PLANE_POSITION_SLIDE_SEQUENCE).first()
    else {
        warnings.push("selected frame has no declared slide position".to_string());
        return unavailable("unavailable");
    };
    let row = read_number::<i64>(position, tags::ROW_POSITION_IN_TOTAL_IMAGE_PIXEL_MATRIX);
    let column = read_number::<i64>(position, tags::COLUMN_POSITION_IN_TOTAL_IMAGE_PIXEL_MATRIX);
    let (Some(row), Some(column)) = (row, column) else {
        warnings.push("selected frame slide position is malformed".to_string());
        return unavailable("unavailable");
    };
    let (Some(y), Some(x)) = (row.checked_sub(1), column.checked_sub(1)) else {
        warnings.push("slide positions must be positive one-based values".to_string());
        return unavailable("unavailable");
    };
    let (Ok(y), Ok(x)) = (u64::try_from(y), u64::try_from(x)) else {
        warnings.push("slide positions must be positive one-based values".to_string());
        return unavailable("unavailable");
    };
    let tile_rows = u64::from(file.rows);
    let tile_columns = u64::from(file.columns);
    if tile_rows == 0 || tile_columns == 0 || x >= matrix.columns || y >= matrix.rows {
        warnings.push("selected frame rectangle lies outside the Total Pixel Matrix".to_string());
        return unavailable("unavailable");
    }
    let optical_item =
        sequence_items(frame_group, tags::OPTICAL_PATH_IDENTIFICATION_SEQUENCE).first();
    let optical_identifier =
        optical_item.and_then(|item| read_string(item, tags::OPTICAL_PATH_IDENTIFIER));
    let optical_index = optical_identifier.as_ref().and_then(|identifier| {
        file.series_metadata
            .optical_path_identifiers
            .iter()
            .position(|value| value == identifier)
            .and_then(|index| index.try_into().ok())
    });
    let z_offset_slide = read_number(position, tags::Z_OFFSET_IN_SLIDE_COORDINATE_SYSTEM);
    let focal_index = if file.frame_count as usize > MAX_FOCAL_METADATA_SCAN {
        warnings.push(format!(
            "focal-plane index omitted because metadata scan is bounded to {MAX_FOCAL_METADATA_SCAN} frames"
        ));
        None
    } else {
        z_offset_slide.and_then(|z| sparse_focal_index(object, z))
    };
    Placement {
        source: "declared_per_frame",
        rectangle: Some(WsiTileRectangle {
            x,
            y,
            width: tile_columns.min(matrix.columns - x),
            height: tile_rows.min(matrix.rows - y),
        }),
        tile_row: (y % tile_rows == 0).then_some(y / tile_rows),
        tile_column: (x % tile_columns == 0).then_some(x / tile_columns),
        optical_path: Some(WsiOpticalPath {
            index: optical_index,
            identifier: optical_identifier,
        }),
        focal_plane: Some(WsiFocalPlane {
            index: focal_index,
            z_offset_slide,
        }),
    }
}

fn sparse_focal_index(
    object: &InMemDicomObject<StandardDataDictionary>,
    selected_z: f64,
) -> Option<u32> {
    if !selected_z.is_finite() {
        return None;
    }
    let groups = object
        .element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
        .ok()?
        .items()?;
    let mut offsets = groups
        .iter()
        .take(MAX_FOCAL_METADATA_SCAN)
        .filter_map(|group| {
            let position = sequence_items(group, tags::PLANE_POSITION_SLIDE_SEQUENCE).first()?;
            read_number::<f64>(position, tags::Z_OFFSET_IN_SLIDE_COORDINATE_SYSTEM)
                .filter(|value| value.is_finite())
        })
        .collect::<Vec<_>>();
    offsets.sort_by(f64::total_cmp);
    offsets.dedup_by(|left, right| (*left - *right).abs() <= 1e-9);
    offsets
        .iter()
        .position(|value| (*value - selected_z).abs() <= 1e-9)
        .and_then(|index| index.try_into().ok())
}

fn nonzero_grid(
    matrix: &WsiTotalPixelMatrix,
    tile_rows: u64,
    tile_columns: u64,
) -> Option<(u64, u64)> {
    if tile_rows == 0 || tile_columns == 0 {
        return None;
    }
    Some((
        matrix.rows.div_ceil(tile_rows),
        matrix.columns.div_ceil(tile_columns),
    ))
}

fn rectangle(
    matrix: &WsiTotalPixelMatrix,
    row: u64,
    column: u64,
    tile_rows: u64,
    tile_columns: u64,
) -> Option<WsiTileRectangle> {
    let y = row.checked_mul(tile_rows)?;
    let x = column.checked_mul(tile_columns)?;
    (x < matrix.columns && y < matrix.rows).then(|| WsiTileRectangle {
        x,
        y,
        width: tile_columns.min(matrix.columns - x),
        height: tile_rows.min(matrix.rows - y),
    })
}

fn unavailable(source: &'static str) -> Placement {
    Placement {
        source,
        rectangle: None,
        tile_row: None,
        tile_column: None,
        optical_path: None,
        focal_plane: None,
    }
}

fn pyramid_level(source: &FileEntry, files: &[Arc<FileEntry>]) -> Option<u32> {
    let pyramid_uid = source.series_metadata.pyramid_uid.as_ref()?;
    let dimensions = (
        source.series_metadata.total_pixel_matrix_rows?,
        source.series_metadata.total_pixel_matrix_columns?,
    );
    let mut levels = BTreeSet::new();
    for file in files.iter().filter(|file| {
        file.series_metadata.pyramid_uid.as_ref() == Some(pyramid_uid)
            && classify_sop_class(&file.sop_class_uid) == ObjectKind::WholeSlideMicroscopy
    }) {
        if let (Some(rows), Some(columns)) = (
            file.series_metadata.total_pixel_matrix_rows,
            file.series_metadata.total_pixel_matrix_columns,
        ) {
            levels.insert((u64::from(rows) * u64::from(columns), rows, columns));
        }
    }
    levels
        .iter()
        .rev()
        .position(|(_, rows, columns)| (*rows, *columns) == dimensions)
        .and_then(|level| level.try_into().ok())
}

fn companions(source: &FileEntry, files: &[Arc<FileEntry>]) -> (Vec<WsiCompanionSummary>, bool) {
    let mut matches = Vec::new();
    for file in files
        .iter()
        .filter(|file| file.index != source.index)
        .filter(|file| classify_sop_class(&file.sop_class_uid) == ObjectKind::WholeSlideMicroscopy)
        .filter(|file| {
            source.series_metadata.pyramid_uid.is_some()
                && source.series_metadata.pyramid_uid == file.series_metadata.pyramid_uid
                || (!source.study_instance_uid.is_empty()
                    && source.study_instance_uid == file.study_instance_uid
                    && source.series_metadata.container_identifier.is_some()
                    && source.series_metadata.container_identifier
                        == file.series_metadata.container_identifier)
        })
    {
        matches.push(WsiCompanionSummary {
            file_index: file.index,
            sop_instance_uid: file.sop_instance_uid.clone(),
            image_type_role: file.series_metadata.image_type.get(2).cloned(),
            pyramid_uid: file.series_metadata.pyramid_uid.clone(),
        });
        if matches.len() > MAX_COMPANIONS {
            break;
        }
    }
    matches.sort_by_key(|item| item.file_index);
    let truncated = matches.len() > MAX_COMPANIONS;
    matches.truncate(MAX_COMPANIONS);
    (matches, truncated)
}

fn relationships(
    source: &FileEntry,
    object: &InMemDicomObject<StandardDataDictionary>,
    files: &[Arc<FileEntry>],
) -> (Vec<ReferenceSummary>, bool) {
    let candidates = files
        .iter()
        .map(|file| ReferenceCandidate::from_file(file))
        .collect::<Vec<_>>();
    let edges = references::extract_reference_edges_from_object(object);
    let mut resolved = references::resolve_reference_edges(&edges, &candidates);
    let truncated = resolved.len() > MAX_RELATIONSHIPS;
    resolved.truncate(MAX_RELATIONSHIPS);
    let _ = source;
    (resolved.iter().map(reference_summary).collect(), truncated)
}

fn reference_summary(edge: &ResolvedReferenceEdge) -> ReferenceSummary {
    ReferenceSummary {
        relationship: edge.relationship.as_str().to_string(),
        target: ReferenceTargetSummary {
            sop_class_uid: edge.target.sop_class_uid.clone(),
            sop_instance_uid: edge.target.sop_instance_uid.clone(),
            series_instance_uid: edge.target.series_instance_uid.clone(),
            frame_numbers: edge.target.frame_numbers.clone(),
            segment_numbers: edge.target.segment_numbers.clone(),
        },
        matches: edge
            .matches
            .iter()
            .map(|target| ReferenceMatchSummary {
                file_index: target.file_index,
                path: target.path.display().to_string(),
                sop_instance_uid: target.sop_instance_uid.clone(),
                frame_indices: target.frame_indices.clone(),
            })
            .collect(),
    }
}
