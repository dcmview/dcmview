//! Bounded extraction of typed references between DICOM instances.
//!
//! These edges describe identity and declared relationships only. They do not
//! imply that the referenced object is present or that either object has a
//! semantic renderer.

use crate::dicom_values::{read_numbers, read_string};
use anyhow::{Context, Result};
use dicom_core::Tag;
use dicom_dictionary_std::{tags, StandardDataDictionary};
use dicom_object::{InMemDicomObject, OpenFileOptions};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_SEQUENCE_DEPTH: usize = 16;
const MAX_SEQUENCE_ITEMS: usize = 4_096;
const MAX_CANDIDATES: usize = 4_096;

const REFERENCED_SERIES_SEQUENCE: Tag = Tag(0x0008, 0x1115);
const REFERENCED_IMAGE_SEQUENCE: Tag = Tag(0x0008, 0x1140);
const SOURCE_IMAGE_SEQUENCE: Tag = Tag(0x0008, 0x2112);
const DEFINITION_SOURCE_SEQUENCE: Tag = Tag(0x0008, 0x1156);
const CONTENT_SEQUENCE: Tag = Tag(0x0040, 0xA730);
const REAL_WORLD_VALUE_MAPPING_SEQUENCE: Tag = Tag(0x0040, 0x9096);
const DEFORMABLE_REGISTRATION_SEQUENCE: Tag = Tag(0x0064, 0x0002);
const REGISTRATION_SEQUENCE: Tag = Tag(0x0070, 0x0308);
const CONTOUR_IMAGE_SEQUENCE: Tag = Tag(0x3006, 0x0016);
const REFERENCED_RT_PLAN_SEQUENCE: Tag = Tag(0x300C, 0x0002);
const REFERENCED_STRUCTURE_SET_SEQUENCE: Tag = Tag(0x300C, 0x0060);
const REFERENCED_DOSE_SEQUENCE: Tag = Tag(0x300C, 0x0080);
const REFERENCED_RT_RADIATION_SEQUENCE: Tag = Tag(0x300A, 0x0630);

const SEGMENTATION_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.66.4";
const LABEL_MAP_SEGMENTATION_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.66.7";
const PARAMETRIC_MAP_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.30";
const RWVM_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.67";
const GRAYSCALE_PR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.11.1";
const COLOR_PR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.11.2";
const BLENDING_PR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.11.4";
const ADVANCED_BLENDING_PR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.11.8";
const SPATIAL_REGISTRATION_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.66.1";
const DEFORMABLE_REGISTRATION_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.66.3";
const BASIC_TEXT_SR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.88.11";
const COMPREHENSIVE_SR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.88.33";
const COMPREHENSIVE_3D_SR_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.88.34";
const KEY_OBJECT_SELECTION_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.88.59";
const RT_IMAGE_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.481.1";
const RT_DOSE_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.481.2";
const RT_STRUCTURE_SET_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.481.3";
const RT_PLAN_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.481.5";
const RT_RADIATION_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.481.12";
const RT_RADIATION_SET_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.481.13";
const WSI_STORAGE: &str = "1.2.840.10008.5.1.4.1.1.77.1.6";

/// Stable relationship names used by internal evidence and future resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceRelationship {
    SourceImage,
    SourceImageForSegmentation,
    BlendingInput,
    BlendingSource,
    RegisteredTarget,
    MovingSource,
    DeformationSource,
    SourceOfMeasurement,
    KeyObjectSegmentation,
    ReferencedSegment,
    DefinitionSource,
    SourceStructureSet,
    ReferencedStructureSet,
    ReferencedDose,
    ReferencedRtPlan,
    ReferencedRtRadiation,
    Unknown,
}

impl ReferenceRelationship {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceImage => "source_image",
            Self::SourceImageForSegmentation => "source_image_for_segmentation",
            Self::BlendingInput => "blending_input",
            Self::BlendingSource => "blending_source",
            Self::RegisteredTarget => "registered_target",
            Self::MovingSource => "moving_source",
            Self::DeformationSource => "deformation_source",
            Self::SourceOfMeasurement => "source_of_measurement",
            Self::KeyObjectSegmentation => "key_object_segmentation",
            Self::ReferencedSegment => "referenced_segment",
            Self::DefinitionSource => "definition_source",
            Self::SourceStructureSet => "source_structure_set",
            Self::ReferencedStructureSet => "referenced_structure_set",
            Self::ReferencedDose => "referenced_dose",
            Self::ReferencedRtPlan => "referenced_rt_plan",
            Self::ReferencedRtRadiation => "referenced_rt_radiation",
            Self::Unknown => "unknown",
        }
    }
}

/// Identity retained even when the referenced instance is not in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ReferenceIdentity {
    pub sop_class_uid: Option<String>,
    pub sop_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
    pub frame_numbers: Vec<u32>,
    pub segment_numbers: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReferenceEdge {
    pub relationship: ReferenceRelationship,
    pub target: ReferenceIdentity,
}

/// Minimal local identity used to resolve declared references without relying
/// on nondeterministic registry positions alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceCandidate {
    pub file_index: usize,
    pub path: PathBuf,
    pub sop_class_uid: String,
    pub sop_instance_uid: String,
    pub series_instance_uid: String,
    pub frame_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceMatch {
    pub file_index: usize,
    pub path: PathBuf,
    pub sop_instance_uid: String,
    /// Zero-based, in-range frames suitable for viewer navigation.
    pub frame_indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReferenceEdge {
    pub relationship: ReferenceRelationship,
    pub target: ReferenceIdentity,
    pub matches: Vec<ReferenceMatch>,
}

/// Resolve edges against the current ephemeral registry snapshot.
///
/// SOP Instance UID is the preferred key. A series-only declaration resolves
/// to every member of that series. Optional SOP Class and Series identities
/// further constrain a match, and invalid one-based frame numbers are retained
/// in the target identity but omitted from navigable zero-based frame indices.
pub fn resolve_reference_edges(
    edges: &[ReferenceEdge],
    candidates: &[ReferenceCandidate],
) -> Vec<ResolvedReferenceEdge> {
    edges
        .iter()
        .map(|edge| {
            let matching_candidates = candidates
                .iter()
                .filter(|candidate| reference_matches_candidate(&edge.target, candidate))
                .collect::<Vec<_>>();
            let mut target = edge.target.clone();
            if let [candidate] = matching_candidates.as_slice() {
                target
                    .sop_class_uid
                    .get_or_insert_with(|| candidate.sop_class_uid.clone());
                target
                    .series_instance_uid
                    .get_or_insert_with(|| candidate.series_instance_uid.clone());
                if edge.relationship == ReferenceRelationship::SourceImage
                    && target.frame_numbers.is_empty()
                    && candidate.frame_count > 1
                {
                    target.frame_numbers = (1..=candidate.frame_count).collect();
                }
            }
            let mut matches = matching_candidates
                .into_iter()
                .map(|candidate| ReferenceMatch {
                    file_index: candidate.file_index,
                    path: candidate.path.clone(),
                    sop_instance_uid: candidate.sop_instance_uid.clone(),
                    frame_indices: navigable_frame_indices(
                        &target.frame_numbers,
                        candidate.frame_count,
                    ),
                })
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| {
                left.file_index
                    .cmp(&right.file_index)
                    .then_with(|| left.path.cmp(&right.path))
            });
            ResolvedReferenceEdge {
                relationship: edge.relationship,
                target,
                matches,
            }
        })
        .collect()
}

fn reference_matches_candidate(target: &ReferenceIdentity, candidate: &ReferenceCandidate) -> bool {
    if target.sop_instance_uid.is_none() && target.series_instance_uid.is_none() {
        return false;
    }
    if target
        .sop_instance_uid
        .as_ref()
        .is_some_and(|uid| uid != &candidate.sop_instance_uid)
    {
        return false;
    }
    if target
        .series_instance_uid
        .as_ref()
        .is_some_and(|uid| uid != &candidate.series_instance_uid)
    {
        return false;
    }
    target
        .sop_class_uid
        .as_ref()
        .is_none_or(|uid| uid == &candidate.sop_class_uid)
}

fn navigable_frame_indices(frame_numbers: &[u32], frame_count: u32) -> Vec<u32> {
    if frame_numbers.is_empty() {
        return (0..frame_count).collect();
    }
    let mut indices = Vec::new();
    for frame_number in frame_numbers {
        let Some(index) = frame_number
            .checked_sub(1)
            .filter(|index| *index < frame_count)
        else {
            continue;
        };
        if !indices.contains(&index) {
            indices.push(index);
        }
    }
    indices
}

/// Open one Part 10 object and extract its declared semantic reference edges.
pub fn extract_reference_edges(path: &Path) -> Result<Vec<ReferenceEdge>> {
    let object = OpenFileOptions::new()
        .read_until(tags::PIXEL_DATA)
        .open_file(path)
        .with_context(|| format!("failed to open DICOM references: {}", path.display()))?;
    Ok(extract_reference_edges_from_object(&object))
}

/// Extract edges from an already decoded object without failing on malformed values.
pub fn extract_reference_edges_from_object(
    object: &InMemDicomObject<StandardDataDictionary>,
) -> Vec<ReferenceEdge> {
    let sop_class = read_string(object, tags::SOP_CLASS_UID).unwrap_or_default();
    let mut candidates = Vec::new();
    collect_candidates(object, &mut Vec::new(), None, 0, &mut candidates);

    let series_by_instance = candidates
        .iter()
        .filter_map(|candidate| {
            Some((
                candidate.sop_instance_uid.as_ref()?.clone(),
                candidate.series_instance_uid.as_ref()?.clone(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut frames_by_instance = HashMap::<String, Vec<u32>>::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| !candidate.frame_numbers.is_empty())
    {
        let Some(instance_uid) = candidate.sop_instance_uid.as_ref() else {
            continue;
        };
        let frames = frames_by_instance.entry(instance_uid.clone()).or_default();
        for frame in &candidate.frame_numbers {
            if !frames.contains(frame) {
                frames.push(*frame);
            }
        }
    }

    let selected = select_candidates(&sop_class, &candidates);
    let mut edges = selected
        .into_iter()
        .map(|(relationship, candidate)| {
            let mut identity = candidate.identity();
            if let Some(instance_uid) = identity.sop_instance_uid.as_ref() {
                if identity.series_instance_uid.is_none() {
                    identity.series_instance_uid = series_by_instance.get(instance_uid).cloned();
                }
                if identity.frame_numbers.is_empty() {
                    identity.frame_numbers = frames_by_instance
                        .get(instance_uid)
                        .cloned()
                        .unwrap_or_default();
                }
            }
            ReferenceEdge {
                relationship,
                target: identity,
            }
        })
        .collect::<Vec<_>>();

    // Preserve declaration order while removing only fully identical edges.
    let mut seen = HashSet::new();
    edges.retain(|edge| seen.insert(edge.clone()));
    edges
}

#[derive(Debug, Clone)]
struct Candidate {
    path: Vec<Tag>,
    sop_class_uid: Option<String>,
    sop_instance_uid: Option<String>,
    series_instance_uid: Option<String>,
    frame_numbers: Vec<u32>,
    segment_numbers: Vec<u16>,
}

impl Candidate {
    fn identity(&self) -> ReferenceIdentity {
        ReferenceIdentity {
            sop_class_uid: self.sop_class_uid.clone(),
            sop_instance_uid: self.sop_instance_uid.clone(),
            series_instance_uid: self.series_instance_uid.clone(),
            frame_numbers: self.frame_numbers.clone(),
            segment_numbers: self.segment_numbers.clone(),
        }
    }

    fn under(&self, tag: Tag) -> bool {
        self.path.contains(&tag)
    }

    fn starts_with(&self, tag: Tag) -> bool {
        self.path.first() == Some(&tag)
    }
}

fn collect_candidates(
    object: &InMemDicomObject<StandardDataDictionary>,
    path: &mut Vec<Tag>,
    inherited_series_uid: Option<String>,
    depth: usize,
    output: &mut Vec<Candidate>,
) {
    if depth > MAX_SEQUENCE_DEPTH || output.len() >= MAX_CANDIDATES {
        return;
    }
    let local_series_uid = if path.last() == Some(&REFERENCED_SERIES_SEQUENCE) {
        read_string(object, tags::SERIES_INSTANCE_UID).or(inherited_series_uid)
    } else {
        inherited_series_uid
    };
    let sop_class_uid = read_string(object, tags::REFERENCED_SOP_CLASS_UID);
    let sop_instance_uid = read_string(object, tags::REFERENCED_SOP_INSTANCE_UID);
    if sop_class_uid.is_some() || sop_instance_uid.is_some() {
        output.push(Candidate {
            path: path.clone(),
            sop_class_uid,
            sop_instance_uid,
            series_instance_uid: local_series_uid.clone(),
            frame_numbers: read_numbers::<u32>(object, tags::REFERENCED_FRAME_NUMBER),
            segment_numbers: read_numbers::<u16>(object, Tag(0x0062, 0x000B)),
        });
    }

    for element in object.iter() {
        let Some(items) = element.items() else {
            continue;
        };
        path.push(element.header().tag);
        for item in items.iter().take(MAX_SEQUENCE_ITEMS) {
            collect_candidates(item, path, local_series_uid.clone(), depth + 1, output);
            if output.len() >= MAX_CANDIDATES {
                break;
            }
        }
        path.pop();
    }
}

fn select_candidates<'a>(
    sop_class: &str,
    candidates: &'a [Candidate],
) -> Vec<(ReferenceRelationship, &'a Candidate)> {
    let matching = |predicate: &dyn Fn(&Candidate) -> bool| {
        candidates
            .iter()
            .filter(|candidate| predicate(candidate))
            .collect::<Vec<_>>()
    };
    match sop_class {
        PARAMETRIC_MAP_STORAGE => {
            matching(&|candidate| candidate.path.as_slice() == [SOURCE_IMAGE_SEQUENCE])
                .into_iter()
                .map(|candidate| (ReferenceRelationship::SourceImage, candidate))
                .collect()
        }
        RWVM_STORAGE => matching(&|candidate| candidate.under(REAL_WORLD_VALUE_MAPPING_SEQUENCE))
            .into_iter()
            .map(|candidate| (ReferenceRelationship::SourceImage, candidate))
            .collect(),
        SEGMENTATION_STORAGE | LABEL_MAP_SEGMENTATION_STORAGE => matching(&|candidate| {
            candidate.starts_with(REFERENCED_SERIES_SEQUENCE)
                || candidate.path.as_slice() == [SOURCE_IMAGE_SEQUENCE]
        })
        .into_iter()
        .map(|candidate| {
            let relationship = if candidate.sop_class_uid.as_deref() == Some(WSI_STORAGE) {
                ReferenceRelationship::SourceImageForSegmentation
            } else {
                ReferenceRelationship::SourceImage
            };
            (relationship, candidate)
        })
        .collect(),
        GRAYSCALE_PR_STORAGE | COLOR_PR_STORAGE => {
            matching(&|candidate| candidate.starts_with(REFERENCED_SERIES_SEQUENCE))
                .into_iter()
                .map(|candidate| (ReferenceRelationship::SourceImage, candidate))
                .collect()
        }
        BLENDING_PR_STORAGE => matching(&|candidate| {
            candidate.under(REFERENCED_IMAGE_SEQUENCE)
                && !candidate.starts_with(REFERENCED_SERIES_SEQUENCE)
        })
        .into_iter()
        .map(|candidate| (ReferenceRelationship::BlendingSource, candidate))
        .collect(),
        ADVANCED_BLENDING_PR_STORAGE => matching(&|candidate| {
            candidate.under(REFERENCED_IMAGE_SEQUENCE)
                && !candidate.starts_with(REFERENCED_SERIES_SEQUENCE)
        })
        .into_iter()
        .map(|candidate| (ReferenceRelationship::BlendingInput, candidate))
        .collect(),
        SPATIAL_REGISTRATION_STORAGE => matching(&|candidate| {
            candidate.under(REGISTRATION_SEQUENCE) && candidate.under(REFERENCED_IMAGE_SEQUENCE)
        })
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| {
            (
                if index == 0 {
                    ReferenceRelationship::RegisteredTarget
                } else {
                    ReferenceRelationship::MovingSource
                },
                candidate,
            )
        })
        .collect(),
        DEFORMABLE_REGISTRATION_STORAGE => {
            let mut output =
                matching(&|candidate| candidate.starts_with(REFERENCED_SERIES_SEQUENCE))
                    .into_iter()
                    .map(|candidate| (ReferenceRelationship::RegisteredTarget, candidate))
                    .collect::<Vec<_>>();
            output.extend(
                matching(&|candidate| candidate.under(DEFORMABLE_REGISTRATION_SEQUENCE))
                    .into_iter()
                    .map(|candidate| (ReferenceRelationship::DeformationSource, candidate)),
            );
            output
        }
        BASIC_TEXT_SR_STORAGE
        | COMPREHENSIVE_SR_STORAGE
        | COMPREHENSIVE_3D_SR_STORAGE
        | KEY_OBJECT_SELECTION_STORAGE => select_sr_candidates(sop_class, candidates),
        RT_IMAGE_STORAGE
        | RT_DOSE_STORAGE
        | RT_STRUCTURE_SET_STORAGE
        | RT_PLAN_STORAGE
        | RT_RADIATION_STORAGE
        | RT_RADIATION_SET_STORAGE => select_rt_candidates(sop_class, candidates),
        _ => matching(&|_| true)
            .into_iter()
            .map(|candidate| (ReferenceRelationship::Unknown, candidate))
            .collect(),
    }
}

fn select_sr_candidates<'a>(
    sop_class: &str,
    candidates: &'a [Candidate],
) -> Vec<(ReferenceRelationship, &'a Candidate)> {
    let content = candidates
        .iter()
        .filter(|candidate| candidate.under(CONTENT_SEQUENCE))
        .collect::<Vec<_>>();
    let selected = if content.is_empty() {
        candidates
            .iter()
            .filter(|candidate| candidate.starts_with(Tag(0x0040, 0xA375)))
            .collect::<Vec<_>>()
    } else {
        content
    };
    let tid1500 = sop_class == COMPREHENSIVE_3D_SR_STORAGE
        && selected.iter().any(|candidate| {
            candidate.sop_class_uid.as_deref() == Some(SEGMENTATION_STORAGE)
                || !candidate.segment_numbers.is_empty()
        });
    selected
        .into_iter()
        .map(|candidate| {
            let relationship = if sop_class == KEY_OBJECT_SELECTION_STORAGE {
                if candidate.sop_class_uid.as_deref() == Some(SEGMENTATION_STORAGE) {
                    ReferenceRelationship::KeyObjectSegmentation
                } else {
                    ReferenceRelationship::SourceImage
                }
            } else if tid1500 {
                if candidate.sop_class_uid.as_deref() == Some(SEGMENTATION_STORAGE) {
                    ReferenceRelationship::ReferencedSegment
                } else {
                    ReferenceRelationship::SourceImageForSegmentation
                }
            } else if sop_class == COMPREHENSIVE_3D_SR_STORAGE {
                ReferenceRelationship::SourceOfMeasurement
            } else {
                ReferenceRelationship::SourceImage
            };
            (relationship, candidate)
        })
        .collect()
}

fn select_rt_candidates<'a>(
    sop_class: &str,
    candidates: &'a [Candidate],
) -> Vec<(ReferenceRelationship, &'a Candidate)> {
    candidates
        .iter()
        .filter_map(|candidate| {
            let relationship = if candidate.under(DEFINITION_SOURCE_SEQUENCE) {
                ReferenceRelationship::DefinitionSource
            } else if candidate.under(REFERENCED_RT_RADIATION_SEQUENCE) {
                ReferenceRelationship::ReferencedRtRadiation
            } else if candidate.under(REFERENCED_RT_PLAN_SEQUENCE) {
                ReferenceRelationship::ReferencedRtPlan
            } else if candidate.under(REFERENCED_STRUCTURE_SET_SEQUENCE) {
                if sop_class == RT_DOSE_STORAGE {
                    ReferenceRelationship::SourceStructureSet
                } else {
                    ReferenceRelationship::ReferencedStructureSet
                }
            } else if candidate.under(REFERENCED_DOSE_SEQUENCE) {
                ReferenceRelationship::ReferencedDose
            } else if candidate.under(CONTOUR_IMAGE_SEQUENCE)
                || candidate.path.as_slice() == [REFERENCED_IMAGE_SEQUENCE]
            {
                ReferenceRelationship::SourceImage
            } else {
                return None;
            };
            Some((relationship, candidate))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        extract_reference_edges_from_object, ReferenceRelationship, CONTENT_SEQUENCE,
        SEGMENTATION_STORAGE,
    };
    use dicom_core::{value::DataSetSequence, DataElement, PrimitiveValue, Tag, VR};
    use dicom_dictionary_std::tags;
    use dicom_object::InMemDicomObject;

    fn item(elements: impl IntoIterator<Item = DataElement<InMemDicomObject>>) -> InMemDicomObject {
        InMemDicomObject::from_element_iter(elements)
    }

    fn referenced(uid: &str, frames: &[&str]) -> InMemDicomObject {
        item([
            DataElement::new(tags::REFERENCED_SOP_CLASS_UID, VR::UI, "1.2.3"),
            DataElement::new(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, uid),
            DataElement::new(
                tags::REFERENCED_FRAME_NUMBER,
                VR::IS,
                PrimitiveValue::Strs(frames.iter().map(|value| (*value).into()).collect()),
            ),
        ])
    }

    #[test]
    fn extracts_ordered_sr_content_references_and_exact_deduplicates() {
        let first = referenced("1.2.3.1", &["2", "4"]);
        let duplicate = first.clone();
        let second = referenced("1.2.3.2", &["1"]);
        let object = item([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.88.33"),
            DataElement::new(
                CONTENT_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![
                    item([DataElement::new(
                        Tag(0x0008, 0x1199),
                        VR::SQ,
                        DataSetSequence::from(vec![first]),
                    )]),
                    item([DataElement::new(
                        Tag(0x0008, 0x1199),
                        VR::SQ,
                        DataSetSequence::from(vec![duplicate]),
                    )]),
                    item([DataElement::new(
                        Tag(0x0008, 0x1199),
                        VR::SQ,
                        DataSetSequence::from(vec![second]),
                    )]),
                ]),
            ),
        ]);

        let edges = extract_reference_edges_from_object(&object);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].relationship, ReferenceRelationship::SourceImage);
        assert_eq!(edges[0].target.sop_instance_uid.as_deref(), Some("1.2.3.1"));
        assert_eq!(edges[0].target.frame_numbers, [2, 4]);
        assert_eq!(edges[1].target.sop_instance_uid.as_deref(), Some("1.2.3.2"));
    }

    #[test]
    fn retains_partial_unresolved_identity_and_ignores_malformed_numbers() {
        let partial = item([
            DataElement::new(tags::REFERENCED_SOP_CLASS_UID, VR::UI, "1.2.3"),
            DataElement::new(tags::REFERENCED_FRAME_NUMBER, VR::IS, "bad\\3"),
        ]);
        let object = item([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, "9.9.9"),
            DataElement::new(
                Tag(0x0008, 0x1199),
                VR::SQ,
                DataSetSequence::from(vec![partial]),
            ),
        ]);
        let edges = extract_reference_edges_from_object(&object);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].relationship, ReferenceRelationship::Unknown);
        assert_eq!(edges[0].target.sop_instance_uid, None);
        assert_eq!(edges[0].target.frame_numbers, [3]);
    }

    #[test]
    fn segmentation_uses_canonical_series_identity_and_per_frame_numbers() {
        let uid = "1.2.3.4";
        let evidence = item([
            DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, "1.2.3.series"),
            DataElement::new(
                Tag(0x0008, 0x114A),
                VR::SQ,
                DataSetSequence::from(vec![referenced(uid, &[])]),
            ),
        ]);
        let per_frame = |frame: &str| {
            item([DataElement::new(
                Tag(0x0008, 0x9124),
                VR::SQ,
                DataSetSequence::from(vec![item([DataElement::new(
                    Tag(0x0008, 0x2112),
                    VR::SQ,
                    DataSetSequence::from(vec![referenced(uid, &[frame])]),
                )])]),
            )])
        };
        let object = item([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, SEGMENTATION_STORAGE),
            DataElement::new(
                Tag(0x0008, 0x1115),
                VR::SQ,
                DataSetSequence::from(vec![evidence]),
            ),
            DataElement::new(
                Tag(0x5200, 0x9230),
                VR::SQ,
                DataSetSequence::from(vec![per_frame("1"), per_frame("2")]),
            ),
        ]);
        let edges = extract_reference_edges_from_object(&object);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].target.series_instance_uid.as_deref(),
            Some("1.2.3.series")
        );
        assert_eq!(edges[0].target.frame_numbers, [1, 2]);
    }

    #[test]
    fn segmentation_retains_top_level_source_images_without_common_instance_reference() {
        let first = "1.2.3.10";
        let second = "1.2.3.11";
        let object = item([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, SEGMENTATION_STORAGE),
            DataElement::new(
                tags::SOURCE_IMAGE_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![referenced(first, &[]), referenced(second, &[])]),
            ),
        ]);

        let edges = extract_reference_edges_from_object(&object);
        assert_eq!(edges.len(), 2);
        assert!(edges
            .iter()
            .all(|edge| edge.relationship == ReferenceRelationship::SourceImage));
        assert_eq!(
            edges
                .iter()
                .filter_map(|edge| edge.target.sop_instance_uid.as_deref())
                .collect::<Vec<_>>(),
            [first, second]
        );
    }

    #[test]
    fn presentation_state_retains_nested_frame_numbers() {
        let uid = "1.2.3.5";
        let referenced_series = item([
            DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, "1.2.3.series"),
            DataElement::new(
                Tag(0x0008, 0x1140),
                VR::SQ,
                DataSetSequence::from(vec![referenced(uid, &["1", "2"])]),
            ),
        ]);
        let object = item([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.11.1"),
            DataElement::new(
                Tag(0x0008, 0x1115),
                VR::SQ,
                DataSetSequence::from(vec![referenced_series]),
            ),
        ]);

        let edges = extract_reference_edges_from_object(&object);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target.sop_instance_uid.as_deref(), Some(uid));
        assert_eq!(edges[0].target.frame_numbers, [1, 2]);
        assert_eq!(
            edges[0].target.series_instance_uid.as_deref(),
            Some("1.2.3.series")
        );
    }

    #[test]
    fn resolves_instance_and_series_references_to_deterministic_local_frames() {
        let candidates = vec![
            super::ReferenceCandidate {
                file_index: 7,
                path: "/scan/b.dcm".into(),
                sop_class_uid: "1.2.class".into(),
                sop_instance_uid: "1.2.instance.b".into(),
                series_instance_uid: "1.2.series".into(),
                frame_count: 2,
            },
            super::ReferenceCandidate {
                file_index: 3,
                path: "/scan/a.dcm".into(),
                sop_class_uid: "1.2.class".into(),
                sop_instance_uid: "1.2.instance.a".into(),
                series_instance_uid: "1.2.series".into(),
                frame_count: 4,
            },
        ];
        let edges = vec![
            super::ReferenceEdge {
                relationship: super::ReferenceRelationship::SourceImage,
                target: super::ReferenceIdentity {
                    sop_class_uid: Some("1.2.class".into()),
                    sop_instance_uid: Some("1.2.instance.a".into()),
                    series_instance_uid: Some("1.2.series".into()),
                    frame_numbers: vec![1, 4, 5, 4],
                    segment_numbers: vec![],
                },
            },
            super::ReferenceEdge {
                relationship: super::ReferenceRelationship::BlendingInput,
                target: super::ReferenceIdentity {
                    series_instance_uid: Some("1.2.series".into()),
                    ..Default::default()
                },
            },
        ];

        let resolved = super::resolve_reference_edges(&edges, &candidates);
        assert_eq!(resolved[0].matches.len(), 1);
        assert_eq!(resolved[0].matches[0].file_index, 3);
        assert_eq!(resolved[0].matches[0].frame_indices, [0, 3]);
        assert_eq!(
            resolved[1]
                .matches
                .iter()
                .map(|target| target.file_index)
                .collect::<Vec<_>>(),
            [3, 7]
        );
        assert_eq!(resolved[1].matches[0].frame_indices, [0, 1, 2, 3]);
    }

    #[test]
    fn absent_frame_numbers_mean_every_frame_of_a_unique_instance() {
        let edge = super::ReferenceEdge {
            relationship: super::ReferenceRelationship::SourceImage,
            target: super::ReferenceIdentity {
                sop_instance_uid: Some("1.2.instance".into()),
                ..Default::default()
            },
        };
        let candidate = super::ReferenceCandidate {
            file_index: 2,
            path: "/scan/source.dcm".into(),
            sop_class_uid: "1.2.class".into(),
            sop_instance_uid: "1.2.instance".into(),
            series_instance_uid: "1.2.series".into(),
            frame_count: 3,
        };

        let resolved = super::resolve_reference_edges(&[edge], &[candidate]);
        assert_eq!(resolved[0].target.frame_numbers, [1, 2, 3]);
        assert_eq!(resolved[0].matches[0].frame_indices, [0, 1, 2]);
    }

    #[test]
    fn unresolved_or_class_mismatched_references_remain_visible() {
        let candidates = vec![super::ReferenceCandidate {
            file_index: 1,
            path: "/scan/source.dcm".into(),
            sop_class_uid: "1.2.class".into(),
            sop_instance_uid: "1.2.instance".into(),
            series_instance_uid: "1.2.series".into(),
            frame_count: 1,
        }];
        let edges = vec![super::ReferenceEdge {
            relationship: super::ReferenceRelationship::Unknown,
            target: super::ReferenceIdentity {
                sop_class_uid: Some("different.class".into()),
                sop_instance_uid: Some("1.2.instance".into()),
                ..Default::default()
            },
        }];

        let resolved = super::resolve_reference_edges(&edges, &candidates);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].matches.is_empty());
        assert_eq!(
            resolved[0].target.sop_instance_uid.as_deref(),
            Some("1.2.instance")
        );
    }

    #[test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    fn prepared_reference_corpus_matches_locked_relationship_inventory() {
        let root = std::env::var_os("DCMVIEW_PREPARED_CORPUS")
            .map(std::path::PathBuf::from)
            .expect("set DCMVIEW_PREPARED_CORPUS to the generated suite directory");
        let cases: &[(&str, &[&str])] = &[
            (
                "derived/parametric-map/float32_ct_derived_explicit_le/parametric-map.dcm",
                &["source_image", "source_image", "source_image"],
            ),
            (
                "derived/parametric-map/float64_ct_derived_explicit_le/parametric-map-float64.dcm",
                &["source_image", "source_image", "source_image"],
            ),
            (
                "derived/presentation-state/advanced_blending/instance.dcm",
                &[
                    "blending_input",
                    "blending_input",
                    "blending_input",
                    "blending_input",
                ],
            ),
            (
                "derived/presentation-state/blending/instance.dcm",
                &[
                    "blending_source",
                    "blending_source",
                    "blending_source",
                    "blending_source",
                ],
            ),
            (
                "derived/presentation-state/color_softcopy/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/presentation-state/grayscale_softcopy_ct_window_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/registration/deformable_ct_pair/instance.dcm",
                &["registered_target", "deformation_source"],
            ),
            (
                "derived/registration/spatial_ct_pair/instance.dcm",
                &["registered_target", "moving_source"],
            ),
            (
                "derived/rwvm/linear_ct_mapping_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/seg/binary_multiframe_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/seg/binary_multiframe_deflated_image_frame/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/seg/fractional_probability_multiframe_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/seg/labelmap_multiframe_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/seg/wsi_tile_reference/wsi-tile-segmentation.dcm",
                &["source_image_for_segmentation"],
            ),
            (
                "derived/sr/basic_text_observation_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/sr/comprehensive3d_scoord3d/scoord3d-report.dcm",
                &["source_of_measurement"],
            ),
            (
                "derived/sr/comprehensive_measurement_explicit_le/instance.dcm",
                &["source_image"],
            ),
            (
                "derived/sr/key_object_selection_explicit_le/instance.dcm",
                &["source_image", "key_object_segmentation"],
            ),
            (
                "derived/sr/tid1500_ct_measurement_report/measurement-report.dcm",
                &["referenced_segment", "source_image_for_segmentation"],
            ),
            (
                "non-image/rt/carm_photon_electron_radiation_minimal/instance.dcm",
                &["definition_source"],
            ),
            (
                "non-image/rt/dose_grid_u16_explicit_le/instance.dcm",
                &["source_image", "source_structure_set"],
            ),
            (
                "non-image/rt/image_linked/instance.dcm",
                &["referenced_rt_plan"],
            ),
            (
                "non-image/rt/plan_linked/instance.dcm",
                &["referenced_structure_set", "referenced_dose"],
            ),
            (
                "non-image/rt/radiation_set_minimal/instance.dcm",
                &["definition_source", "referenced_rt_radiation"],
            ),
            (
                "non-image/rt/structure_set_single_roi_explicit_le/instance.dcm",
                &["source_image"],
            ),
        ];

        let mut total = 0;
        for (relative_path, expected) in cases {
            let path = ["", "extended", "extended-deflate"]
                .into_iter()
                .map(|profile| {
                    if profile.is_empty() {
                        root.join(relative_path)
                    } else {
                        root.join(profile).join(relative_path)
                    }
                })
                .find(|path| path.is_file())
                .unwrap_or_else(|| root.join("extended").join(relative_path));
            let edges = super::extract_reference_edges(&path)
                .unwrap_or_else(|error| panic!("{relative_path}: {error:#}"));
            let actual = edges
                .iter()
                .map(|edge| edge.relationship.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, *expected, "{relative_path}: {edges:?}");
            total += actual.len();
        }
        assert_eq!(cases.len(), 25);
        assert_eq!(total, 42);
    }
}
