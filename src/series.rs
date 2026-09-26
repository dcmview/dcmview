//! Server-owned logical series and virtual-frame ordering.
//!
//! This module deliberately accepts a small metadata input model rather than
//! depending on the loader's `FileEntry`. The loader/API integration layer can
//! therefore evolve independently while series grouping and ordering remain a
//! deterministic, testable domain operation.

use crate::geometry::{cross, dot, magnitude, normalized, scale, subtract};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeriesId {
    pub study_instance_uid: String,
    pub series_instance_uid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OrderingInput {
    pub image_position_patient: Option<[f64; 3]>,
    pub image_orientation_patient: Option<[f64; 6]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOrderingInput {
    pub frame_index: u32,
    pub ordering: OrderingInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NavigationInput {
    #[default]
    Ordinary,
    Concatenation {
        concatenation_uid: String,
        concatenation_frame_offset_number: Option<u32>,
        in_concatenation_number: Option<u32>,
    },
    Wsi {
        pyramid_uid: Option<String>,
        image_type_role: Option<String>,
        total_pixel_matrix_rows: Option<u32>,
        total_pixel_matrix_columns: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesFileInput {
    pub file_index: usize,
    pub path: PathBuf,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub frame_of_reference_uid: String,
    pub sop_instance_uid: String,
    pub frame_count: u32,
    pub instance_number: Option<i64>,
    pub ordering: OrderingInput,
    pub per_frame_ordering: Vec<FrameOrderingInput>,
    pub navigation: NavigationInput,
}

impl SeriesFileInput {
    pub fn series_id(&self) -> SeriesId {
        SeriesId {
            study_instance_uid: self.study_instance_uid.clone(),
            series_instance_uid: self.series_instance_uid.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FrameLocation {
    pub file_index: usize,
    pub frame_index: u32,
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DuplicatePosition {
    pub position_along_normal_mm: f64,
    pub frames: Vec<FrameLocation>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SeriesWarning {
    MissingPositions {
        frames: Vec<FrameLocation>,
    },
    DuplicatePositions {
        groups: Vec<DuplicatePosition>,
    },
    NonuniformSpacing {
        adjacent_spacing_mm: Vec<f64>,
    },
    InconsistentOrientation {
        frames: Vec<FrameLocation>,
    },
    GantryTilt {
        frames: Vec<FrameLocation>,
        max_lateral_shift_mm: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameRef {
    pub virtual_index: usize,
    pub file_index: usize,
    pub frame_index: u32,
    pub source_path: PathBuf,
    pub sop_instance_uid: String,
    pub instance_number: Option<i64>,
    pub position_along_normal_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesStack {
    pub kind: NavigationKind,
    pub frames: Vec<FrameRef>,
    pub warnings: Vec<SeriesWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NavigationKind {
    Ordinary,
    Concatenation {
        concatenation_uid: String,
    },
    WsiPyramidLevel {
        pyramid_uid: String,
        image_type_role: Option<String>,
        total_pixel_matrix_rows: Option<u32>,
        total_pixel_matrix_columns: Option<u32>,
    },
    WsiCompanion {
        sop_instance_uid: String,
        image_type_role: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesGroup {
    pub id: SeriesId,
    pub stacks: Vec<SeriesStack>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SeriesCatalog {
    series: Vec<SeriesGroup>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeriesCatalogOptions {
    pub position_tolerance_mm: f64,
    pub spacing_tolerance_mm: f64,
    pub orientation_tolerance: f64,
}

impl Default for SeriesCatalogOptions {
    fn default() -> Self {
        Self {
            position_tolerance_mm: 1.0e-5,
            spacing_tolerance_mm: 1.0e-5,
            orientation_tolerance: 1.0e-5,
        }
    }
}

impl SeriesCatalog {
    pub fn build(files: impl IntoIterator<Item = SeriesFileInput>) -> Self {
        let options = SeriesCatalogOptions::default();
        let mut grouped = BTreeMap::<SeriesId, Vec<SeriesFileInput>>::new();
        for file in files {
            let id = file.series_id();
            if id.study_instance_uid.is_empty() || id.series_instance_uid.is_empty() {
                continue;
            }
            grouped.entry(id).or_default().push(file);
        }

        let series = grouped
            .into_iter()
            .map(|(id, files)| build_series_group(id, files, options))
            .collect();
        Self { series }
    }

    pub fn series(&self) -> &[SeriesGroup] {
        &self.series
    }
}

#[derive(Debug, Clone)]
struct CandidateFrame {
    location: FrameLocation,
    sop_instance_uid: String,
    instance_number: Option<i64>,
    ordering: OrderingInput,
    frame_specific_ordering: bool,
    position_along_normal_mm: Option<f64>,
    concatenation_frame_offset_number: Option<u32>,
    in_concatenation_number: Option<u32>,
}

fn build_series_group(
    id: SeriesId,
    files: Vec<SeriesFileInput>,
    options: SeriesCatalogOptions,
) -> SeriesGroup {
    let mut grouped = BTreeMap::<NavigationKind, Vec<SeriesFileInput>>::new();
    for file in files {
        grouped
            .entry(navigation_kind(&file))
            .or_default()
            .push(file);
    }
    let mut stacks = grouped
        .into_iter()
        .map(|(kind, files)| build_stack(kind, files, options))
        .collect::<Vec<_>>();
    stacks.sort_by(|left, right| compare_navigation_kinds(&left.kind, &right.kind));
    SeriesGroup { id, stacks }
}

fn compare_navigation_kinds(left: &NavigationKind, right: &NavigationKind) -> std::cmp::Ordering {
    navigation_rank(left)
        .cmp(&navigation_rank(right))
        .then_with(|| match (left, right) {
            (
                NavigationKind::WsiPyramidLevel {
                    pyramid_uid: left_uid,
                    total_pixel_matrix_rows: left_rows,
                    total_pixel_matrix_columns: left_columns,
                    image_type_role: left_role,
                },
                NavigationKind::WsiPyramidLevel {
                    pyramid_uid: right_uid,
                    total_pixel_matrix_rows: right_rows,
                    total_pixel_matrix_columns: right_columns,
                    image_type_role: right_role,
                },
            ) => left_uid
                .cmp(right_uid)
                .then_with(|| right_rows.cmp(left_rows))
                .then_with(|| right_columns.cmp(left_columns))
                .then_with(|| left_role.cmp(right_role)),
            _ => left.cmp(right),
        })
}

fn navigation_rank(kind: &NavigationKind) -> u8 {
    match kind {
        NavigationKind::Ordinary => 0,
        NavigationKind::Concatenation { .. } => 1,
        NavigationKind::WsiPyramidLevel { .. } => 2,
        NavigationKind::WsiCompanion { .. } => 3,
    }
}

fn navigation_kind(file: &SeriesFileInput) -> NavigationKind {
    match &file.navigation {
        NavigationInput::Ordinary => NavigationKind::Ordinary,
        NavigationInput::Concatenation {
            concatenation_uid, ..
        } => NavigationKind::Concatenation {
            concatenation_uid: concatenation_uid.clone(),
        },
        NavigationInput::Wsi {
            pyramid_uid: Some(pyramid_uid),
            image_type_role,
            total_pixel_matrix_rows,
            total_pixel_matrix_columns,
        } => NavigationKind::WsiPyramidLevel {
            pyramid_uid: pyramid_uid.clone(),
            image_type_role: image_type_role.clone(),
            total_pixel_matrix_rows: *total_pixel_matrix_rows,
            total_pixel_matrix_columns: *total_pixel_matrix_columns,
        },
        NavigationInput::Wsi {
            pyramid_uid: None,
            image_type_role,
            ..
        } => NavigationKind::WsiCompanion {
            sop_instance_uid: file.sop_instance_uid.clone(),
            image_type_role: image_type_role.clone(),
        },
    }
}

fn build_stack(
    kind: NavigationKind,
    mut files: Vec<SeriesFileInput>,
    options: SeriesCatalogOptions,
) -> SeriesStack {
    files.sort_by(|left, right| {
        compare_instance_numbers(left.instance_number, right.instance_number)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.file_index.cmp(&right.file_index))
    });
    let reference_orientation = files
        .iter()
        .flat_map(effective_orderings)
        .find_map(|(_, ordering, _)| valid_orientation(ordering.image_orientation_patient));
    let reference_normal = reference_orientation.and_then(slice_normal);

    let mut candidates = files
        .iter()
        .flat_map(|file| candidate_frames(file, reference_normal))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| compare_candidates(left, right, &kind));

    let warnings = if matches!(
        kind,
        NavigationKind::WsiPyramidLevel { .. } | NavigationKind::WsiCompanion { .. }
    ) {
        Vec::new()
    } else {
        analyze_geometry(&files, reference_orientation, reference_normal, options)
    };
    let frames = candidates
        .into_iter()
        .enumerate()
        .map(|(virtual_index, candidate)| FrameRef {
            virtual_index,
            file_index: candidate.location.file_index,
            frame_index: candidate.location.frame_index,
            source_path: candidate.location.source_path,
            sop_instance_uid: candidate.sop_instance_uid,
            instance_number: candidate.instance_number,
            position_along_normal_mm: candidate.position_along_normal_mm,
        })
        .collect();

    SeriesStack {
        kind,
        frames,
        warnings,
    }
}

fn effective_orderings(
    file: &SeriesFileInput,
) -> impl Iterator<Item = (u32, OrderingInput, bool)> + '_ {
    let overrides = file
        .per_frame_ordering
        .iter()
        .map(|input| (input.frame_index, input.ordering))
        .collect::<BTreeMap<_, _>>();
    (0..file.frame_count).map(move |frame_index| {
        overrides
            .get(&frame_index)
            .copied()
            .map(|ordering| (frame_index, ordering, true))
            .unwrap_or((frame_index, file.ordering, false))
    })
}

fn candidate_frames(
    file: &SeriesFileInput,
    reference_normal: Option<[f64; 3]>,
) -> impl Iterator<Item = CandidateFrame> + '_ {
    effective_orderings(file).map(move |(frame_index, ordering, frame_specific_ordering)| {
        let position_along_normal_mm = reference_normal
            .zip(ordering.image_position_patient)
            .and_then(|(normal, position)| {
                valid_orientation(ordering.image_orientation_patient).map(|_| dot(position, normal))
            });
        CandidateFrame {
            location: FrameLocation {
                file_index: file.file_index,
                frame_index,
                source_path: file.path.clone(),
            },
            sop_instance_uid: file.sop_instance_uid.clone(),
            instance_number: file.instance_number,
            ordering,
            frame_specific_ordering,
            position_along_normal_mm,
            concatenation_frame_offset_number: match &file.navigation {
                NavigationInput::Concatenation {
                    concatenation_frame_offset_number,
                    ..
                } => *concatenation_frame_offset_number,
                _ => None,
            },
            in_concatenation_number: match &file.navigation {
                NavigationInput::Concatenation {
                    in_concatenation_number,
                    ..
                } => *in_concatenation_number,
                _ => None,
            },
        }
    })
}

fn compare_candidates(
    left: &CandidateFrame,
    right: &CandidateFrame,
    kind: &NavigationKind,
) -> std::cmp::Ordering {
    if matches!(kind, NavigationKind::Concatenation { .. }) {
        return match (
            left.concatenation_frame_offset_number,
            right.concatenation_frame_offset_number,
        ) {
            (Some(left_offset), Some(right_offset)) => (left_offset as u64
                + left.location.frame_index as u64)
                .cmp(&(right_offset as u64 + right.location.frame_index as u64)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| {
            compare_optional_u32(left.in_concatenation_number, right.in_concatenation_number)
        })
        .then_with(|| left.location.frame_index.cmp(&right.location.frame_index))
        .then_with(|| compare_geometry_candidates(left, right));
    }
    compare_geometry_candidates(left, right)
}

fn compare_geometry_candidates(
    left: &CandidateFrame,
    right: &CandidateFrame,
) -> std::cmp::Ordering {
    match (
        left.position_along_normal_mm,
        right.position_along_normal_mm,
    ) {
        (Some(left_position), Some(right_position)) => left_position
            .total_cmp(&right_position)
            .then_with(|| compare_fallback(left, right)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => compare_fallback(left, right),
    }
}

fn compare_optional_u32(left: Option<u32>, right: Option<u32>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn compare_fallback(left: &CandidateFrame, right: &CandidateFrame) -> std::cmp::Ordering {
    compare_instance_numbers(left.instance_number, right.instance_number)
        .then_with(|| left.location.source_path.cmp(&right.location.source_path))
        .then_with(|| left.location.file_index.cmp(&right.location.file_index))
        .then_with(|| left.location.frame_index.cmp(&right.location.frame_index))
}

fn compare_instance_numbers(left: Option<i64>, right: Option<i64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn analyze_geometry(
    files: &[SeriesFileInput],
    reference_orientation: Option<[f64; 6]>,
    reference_normal: Option<[f64; 3]>,
    options: SeriesCatalogOptions,
) -> Vec<SeriesWarning> {
    let Some(normal) = reference_normal else {
        let analysis = files
            .iter()
            .flat_map(analysis_candidates)
            .collect::<Vec<_>>();
        let missing_positions = analysis
            .iter()
            .filter(|candidate| candidate.ordering.image_position_patient.is_none())
            .map(|candidate| candidate.location.clone())
            .collect::<Vec<_>>();
        let inconsistent_orientation = analysis
            .iter()
            .filter(|candidate| {
                valid_orientation(candidate.ordering.image_orientation_patient).is_none()
            })
            .map(|candidate| candidate.location.clone())
            .collect::<Vec<_>>();
        let mut warnings = Vec::new();
        if !missing_positions.is_empty() {
            warnings.push(SeriesWarning::MissingPositions {
                frames: missing_positions,
            });
        }
        if !inconsistent_orientation.is_empty() {
            warnings.push(SeriesWarning::InconsistentOrientation {
                frames: inconsistent_orientation,
            });
        }
        return warnings;
    };

    let mut analysis = files
        .iter()
        .flat_map(|file| candidate_frames(file, Some(normal)))
        .filter(|candidate| {
            candidate.frame_specific_ordering || candidate.location.frame_index == 0
        })
        .collect::<Vec<_>>();
    analysis.sort_by(compare_geometry_candidates);

    let missing_positions = analysis
        .iter()
        .filter(|candidate| candidate.ordering.image_position_patient.is_none())
        .map(|candidate| candidate.location.clone())
        .collect::<Vec<_>>();
    let inconsistent_orientation = analysis
        .iter()
        .filter(|candidate| {
            candidate
                .ordering
                .image_orientation_patient
                .and_then(|orientation| valid_orientation(Some(orientation)))
                .zip(reference_orientation)
                .map(|(orientation, reference)| {
                    !orientation_matches(orientation, reference, options.orientation_tolerance)
                })
                .unwrap_or(true)
        })
        .map(|candidate| candidate.location.clone())
        .collect::<Vec<_>>();

    let positioned = analysis
        .iter()
        .filter_map(|candidate| {
            candidate
                .position_along_normal_mm
                .map(|position| (position, candidate))
        })
        .collect::<Vec<_>>();
    let position_groups = group_positions(&positioned, options.position_tolerance_mm);
    let duplicate_groups = position_groups
        .iter()
        .filter(|group| group.len() > 1)
        .map(|group| DuplicatePosition {
            position_along_normal_mm: group[0].0,
            frames: group
                .iter()
                .map(|(_, candidate)| candidate.location.clone())
                .collect(),
        })
        .collect::<Vec<_>>();
    let unique_positions = position_groups
        .iter()
        .map(|group| group[0].0)
        .collect::<Vec<_>>();
    let adjacent_spacing_mm = unique_positions
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let nonuniform = spacing_is_nonuniform(&adjacent_spacing_mm, options.spacing_tolerance_mm);

    let (tilted_frames, max_lateral_shift_mm) = gantry_tilt(&positioned, normal, options);

    let mut warnings = Vec::new();
    if !missing_positions.is_empty() {
        warnings.push(SeriesWarning::MissingPositions {
            frames: missing_positions,
        });
    }
    if !duplicate_groups.is_empty() {
        warnings.push(SeriesWarning::DuplicatePositions {
            groups: duplicate_groups,
        });
    }
    if nonuniform {
        warnings.push(SeriesWarning::NonuniformSpacing {
            adjacent_spacing_mm,
        });
    }
    if !inconsistent_orientation.is_empty() {
        warnings.push(SeriesWarning::InconsistentOrientation {
            frames: inconsistent_orientation,
        });
    }
    if !tilted_frames.is_empty() {
        warnings.push(SeriesWarning::GantryTilt {
            frames: tilted_frames,
            max_lateral_shift_mm,
        });
    }
    warnings
}

fn analysis_candidates(file: &SeriesFileInput) -> impl Iterator<Item = CandidateFrame> + '_ {
    candidate_frames(file, None).filter(|candidate| {
        candidate.frame_specific_ordering || candidate.location.frame_index == 0
    })
}

fn group_positions<'a>(
    positioned: &'a [(f64, &'a CandidateFrame)],
    tolerance: f64,
) -> Vec<Vec<(f64, &'a CandidateFrame)>> {
    let mut groups: Vec<Vec<(f64, &CandidateFrame)>> = Vec::new();
    for &(position, candidate) in positioned {
        if let Some(group) = groups.last_mut() {
            if (position - group[0].0).abs() <= tolerance {
                group.push((position, candidate));
                continue;
            }
        }
        groups.push(vec![(position, candidate)]);
    }
    groups
}

fn spacing_is_nonuniform(spacing: &[f64], tolerance: f64) -> bool {
    let Some((&first, rest)) = spacing.split_first() else {
        return false;
    };
    rest.iter().any(|value| (value - first).abs() > tolerance)
}

fn gantry_tilt(
    positioned: &[(f64, &CandidateFrame)],
    normal: [f64; 3],
    options: SeriesCatalogOptions,
) -> (Vec<FrameLocation>, f64) {
    let orthogonal_positions = positioned
        .iter()
        .filter_map(|(_, candidate)| {
            candidate
                .ordering
                .image_position_patient
                .map(|position| (candidate.location.clone(), reject(position, normal)))
        })
        .collect::<Vec<_>>();
    let Some((_, origin)) = orthogonal_positions.first() else {
        return (Vec::new(), 0.0);
    };

    let mut tilted = BTreeSet::new();
    let mut max_shift = 0.0_f64;
    for (location, position) in &orthogonal_positions {
        let shift = magnitude(subtract(*position, *origin));
        max_shift = max_shift.max(shift);
        if shift > options.position_tolerance_mm {
            tilted.insert(location.clone());
        }
    }
    if !tilted.is_empty() {
        tilted.insert(orthogonal_positions[0].0.clone());
    }
    (tilted.into_iter().collect(), max_shift)
}

fn valid_orientation(orientation: Option<[f64; 6]>) -> Option<[f64; 6]> {
    let orientation = orientation?;
    orientation
        .iter()
        .all(|value| value.is_finite())
        .then_some(orientation)
        .filter(|orientation| slice_normal(*orientation).is_some())
}

fn orientation_matches(left: [f64; 6], right: [f64; 6], tolerance: f64) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(left, right)| (left - right).abs() <= tolerance)
}

fn slice_normal(orientation: [f64; 6]) -> Option<[f64; 3]> {
    let row = [orientation[0], orientation[1], orientation[2]];
    let column = [orientation[3], orientation[4], orientation[5]];
    normalized(cross(row, column))
}

/// The component of `vector` orthogonal to the unit `normal`.
fn reject(vector: [f64; 3], normal: [f64; 3]) -> [f64; 3] {
    subtract(vector, scale(normal, dot(vector, normal)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AXIAL: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    fn input(
        file_index: usize,
        study: &str,
        series: &str,
        frame_of_reference: &str,
        path: &str,
        instance_number: Option<i64>,
        position: Option<[f64; 3]>,
    ) -> SeriesFileInput {
        SeriesFileInput {
            file_index,
            path: PathBuf::from(path),
            study_instance_uid: study.to_string(),
            series_instance_uid: series.to_string(),
            frame_of_reference_uid: frame_of_reference.to_string(),
            sop_instance_uid: format!("sop-{file_index}"),
            frame_count: 1,
            instance_number,
            ordering: OrderingInput {
                image_position_patient: position,
                image_orientation_patient: Some(AXIAL),
            },
            per_frame_ordering: Vec::new(),
            navigation: NavigationInput::Ordinary,
        }
    }

    fn only_stack(catalog: &SeriesCatalog) -> &SeriesStack {
        assert_eq!(catalog.series().len(), 1);
        assert_eq!(catalog.series()[0].stacks.len(), 1);
        &catalog.series()[0].stacks[0]
    }

    #[test]
    fn groups_by_study_and_series_never_frame_of_reference() {
        let catalog = SeriesCatalog::build([
            input(0, "study", "series-a", "shared-for", "a.dcm", Some(1), None),
            input(1, "study", "series-b", "shared-for", "b.dcm", Some(1), None),
            input(
                2,
                "other-study",
                "series-a",
                "shared-for",
                "c.dcm",
                Some(1),
                None,
            ),
        ]);

        assert_eq!(catalog.series().len(), 3);
        assert_eq!(catalog.series()[0].id.study_instance_uid, "other-study");
        assert_eq!(catalog.series()[1].id.series_instance_uid, "series-a");
        assert_eq!(catalog.series()[2].id.series_instance_uid, "series-b");
    }

    #[test]
    fn omits_files_without_complete_series_identity() {
        let catalog = SeriesCatalog::build([
            input(0, "", "series", "for", "missing-study.dcm", Some(1), None),
            input(1, "study", "", "for", "missing-series.dcm", Some(1), None),
            input(2, "study", "series", "for", "complete.dcm", Some(1), None),
        ]);

        assert_eq!(catalog.series().len(), 1);
        assert_eq!(catalog.series()[0].id.study_instance_uid, "study");
        assert_eq!(catalog.series()[0].id.series_instance_uid, "series");
        assert_eq!(catalog.series()[0].stacks[0].frames[0].file_index, 2);
    }

    #[test]
    fn geometry_projection_overrides_conflicting_instance_numbers() {
        let oblique = [
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
            0.0,
            0.0,
            1.0,
        ];
        let normal = slice_normal(oblique).expect("oblique slice normal");
        let mut files = [
            input(0, "study", "series", "for", "z.dcm", Some(30), None),
            input(1, "study", "series", "for", "a.dcm", Some(10), None),
            input(2, "study", "series", "for", "m.dcm", Some(20), None),
        ];
        for (file, distance) in files.iter_mut().zip([0.0, 5.0, 10.0]) {
            file.ordering = OrderingInput {
                image_position_patient: Some([
                    normal[0] * distance,
                    normal[1] * distance,
                    normal[2] * distance,
                ]),
                image_orientation_patient: Some(oblique),
            };
        }

        let catalog = SeriesCatalog::build(files);
        let stack = only_stack(&catalog);
        assert_eq!(
            stack
                .frames
                .iter()
                .map(|frame| frame.file_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let positions = stack
            .frames
            .iter()
            .map(|frame| frame.position_along_normal_mm.expect("projected position"))
            .collect::<Vec<_>>();
        for (actual, expected) in positions.iter().zip([0.0, 5.0, 10.0]) {
            assert!((actual - expected).abs() < 1.0e-9);
        }
    }

    #[test]
    fn missing_geometry_falls_back_to_instance_then_path() {
        let catalog = SeriesCatalog::build([
            input(0, "study", "series", "for", "z.dcm", None, None),
            input(1, "study", "series", "for", "b.dcm", Some(2), None),
            input(2, "study", "series", "for", "a.dcm", Some(2), None),
            input(3, "study", "series", "for", "c.dcm", Some(1), None),
        ]);
        let stack = only_stack(&catalog);

        assert_eq!(
            stack
                .frames
                .iter()
                .map(|frame| frame.file_index)
                .collect::<Vec<_>>(),
            vec![3, 2, 1, 0]
        );
        assert!(matches!(
            &stack.warnings[0],
            SeriesWarning::MissingPositions { frames } if frames.len() == 4
        ));
    }

    #[test]
    fn flattens_multiframe_sources_and_uses_per_frame_geometry() {
        let mut multiframe = input(
            4,
            "study",
            "series",
            "for",
            "multi.dcm",
            Some(1),
            Some([0.0, 0.0, 0.0]),
        );
        multiframe.frame_count = 2;
        multiframe.per_frame_ordering = vec![
            FrameOrderingInput {
                frame_index: 0,
                ordering: OrderingInput {
                    image_position_patient: Some([0.0, 0.0, 0.0]),
                    image_orientation_patient: Some(AXIAL),
                },
            },
            FrameOrderingInput {
                frame_index: 1,
                ordering: OrderingInput {
                    image_position_patient: Some([0.0, 0.0, 5.0]),
                    image_orientation_patient: Some(AXIAL),
                },
            },
        ];
        let last = input(
            9,
            "study",
            "series",
            "for",
            "last.dcm",
            Some(2),
            Some([0.0, 0.0, 10.0]),
        );

        let catalog = SeriesCatalog::build([last, multiframe]);
        let stack = only_stack(&catalog);
        assert_eq!(
            stack
                .frames
                .iter()
                .map(|frame| (frame.virtual_index, frame.file_index, frame.frame_index))
                .collect::<Vec<_>>(),
            vec![(0, 4, 0), (1, 4, 1), (2, 9, 0)]
        );
    }

    #[test]
    fn reports_geometry_quality_warnings() {
        let mut duplicate = input(
            1,
            "study",
            "series",
            "for",
            "duplicate.dcm",
            Some(2),
            Some([0.0, 0.0, 0.0]),
        );
        duplicate.ordering.image_orientation_patient = Some([0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
        let files = [
            input(
                0,
                "study",
                "series",
                "for",
                "first.dcm",
                Some(1),
                Some([0.0, 0.0, 0.0]),
            ),
            duplicate,
            input(
                2,
                "study",
                "series",
                "for",
                "tilted.dcm",
                Some(3),
                Some([0.0, -1.0, 5.0]),
            ),
            input(
                3,
                "study",
                "series",
                "for",
                "last.dcm",
                Some(4),
                Some([0.0, -2.0, 12.0]),
            ),
        ];

        let catalog = SeriesCatalog::build(files);
        let warnings = &only_stack(&catalog).warnings;
        assert!(warnings
            .iter()
            .any(|warning| matches!(warning, SeriesWarning::DuplicatePositions { .. })));
        assert!(warnings
            .iter()
            .any(|warning| matches!(warning, SeriesWarning::NonuniformSpacing { .. })));
        assert!(warnings
            .iter()
            .any(|warning| matches!(warning, SeriesWarning::InconsistentOrientation { .. })));
        assert!(warnings.iter().any(|warning| matches!(
            warning,
            SeriesWarning::GantryTilt {
                max_lateral_shift_mm,
                ..
            } if (*max_lateral_shift_mm - 2.0).abs() < 1.0e-9
        )));
    }

    #[test]
    fn concatenation_offsets_order_parts_independent_of_path() {
        let mut first = input(
            7,
            "study",
            "series",
            "for",
            "z-part-one.dcm",
            Some(20),
            None,
        );
        first.frame_count = 2;
        first.navigation = NavigationInput::Concatenation {
            concatenation_uid: "concat".to_string(),
            concatenation_frame_offset_number: Some(0),
            in_concatenation_number: Some(1),
        };
        let mut second = input(
            3,
            "study",
            "series",
            "for",
            "a-part-two.dcm",
            Some(10),
            None,
        );
        second.navigation = NavigationInput::Concatenation {
            concatenation_uid: "concat".to_string(),
            concatenation_frame_offset_number: Some(2),
            in_concatenation_number: Some(2),
        };

        let catalog = SeriesCatalog::build([second, first]);
        let stack = only_stack(&catalog);
        assert_eq!(
            stack
                .frames
                .iter()
                .map(|frame| (frame.file_index, frame.frame_index))
                .collect::<Vec<_>>(),
            vec![(7, 0), (7, 1), (3, 0)]
        );
        assert!(matches!(
            &stack.kind,
            NavigationKind::Concatenation { concatenation_uid } if concatenation_uid == "concat"
        ));
    }

    #[test]
    fn wsi_pyramid_levels_and_nonmember_companions_are_separate_stacks() {
        let mut volume = input(0, "study", "series", "for", "volume.dcm", Some(1), None);
        volume.frame_count = 4;
        volume.navigation = NavigationInput::Wsi {
            pyramid_uid: Some("pyramid".to_string()),
            image_type_role: Some("VOLUME".to_string()),
            total_pixel_matrix_rows: Some(4),
            total_pixel_matrix_columns: Some(4),
        };
        let mut thumbnail = input(1, "study", "series", "for", "thumbnail.dcm", Some(2), None);
        thumbnail.navigation = NavigationInput::Wsi {
            pyramid_uid: Some("pyramid".to_string()),
            image_type_role: Some("THUMBNAIL".to_string()),
            total_pixel_matrix_rows: Some(2),
            total_pixel_matrix_columns: Some(2),
        };
        let mut label = input(2, "study", "series", "for", "label.dcm", Some(3), None);
        label.navigation = NavigationInput::Wsi {
            pyramid_uid: None,
            image_type_role: Some("LABEL".to_string()),
            total_pixel_matrix_rows: Some(2),
            total_pixel_matrix_columns: Some(2),
        };

        let catalog = SeriesCatalog::build([label, thumbnail, volume]);
        let group = &catalog.series()[0];
        assert_eq!(group.stacks.len(), 3);
        assert_eq!(
            group
                .stacks
                .iter()
                .map(|stack| stack.frames.len())
                .collect::<Vec<_>>(),
            vec![4, 1, 1]
        );
        assert!(matches!(
            &group.stacks[0].kind,
            NavigationKind::WsiPyramidLevel {
                image_type_role: Some(role),
                ..
            } if role == "VOLUME"
        ));
        assert!(matches!(
            &group.stacks[1].kind,
            NavigationKind::WsiPyramidLevel {
                image_type_role: Some(role),
                ..
            } if role == "THUMBNAIL"
        ));
        assert!(matches!(
            &group.stacks[2].kind,
            NavigationKind::WsiCompanion {
                image_type_role: Some(role),
                ..
            } if role == "LABEL"
        ));
    }
}
