use crate::api::contracts::{
    FileSummary, FrameRefSummary, SeriesCatalogResponse, SeriesStackSummary, SeriesSummary,
    SeriesWarningSummary,
};
use crate::loader::{DiscoveryDisposition, DiscoveryRecord};
use crate::masking::Masker;
use crate::series::{
    FrameOrderingInput, NavigationInput, NavigationKind, OrderingInput, SeriesCatalog,
    SeriesFileInput, SeriesGroup, SeriesStack, SeriesWarning,
};
use crate::types::FileEntry;
use bytes::Bytes;
use dicom_dictionary_std::uids;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tokio::sync::{futures::Notified, Notify};

pub const DISCOVERY_RESPONSE_MAX_RECORDS: usize = 256;

#[derive(Clone)]
pub struct FileRegistry {
    inner: Arc<RwLock<FileRegistryInner>>,
    notify: Arc<Notify>,
    /// The last series catalog as JSON, keyed by the file count and scan
    /// state it was built from; the viewer polls it every 500 ms during a
    /// scan, and a large catalog is costly to clone and serialize again.
    catalog: Arc<Mutex<Option<(usize, bool, Bytes)>>>,
    /// Present in a masked session: the catalog is masked as it is built.
    masker: Option<Arc<Masker>>,
}

/// Files and scan counters share one lock so every status read is a
/// consistent snapshot (never "0 files, scan complete" mid-update).
#[derive(Default)]
struct FileRegistryInner {
    files: Vec<Arc<FileEntry>>,
    summaries: Vec<FileSummary>,
    /// The most recent skipped and filtered records, bounded to what the API
    /// returns; accepted files are already listed as files.
    recent_discovery: VecDeque<DiscoveryRecord>,
    scanned: usize,
    skipped: usize,
    filtered: usize,
    scan_complete: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RegistryStatus {
    pub file_count: usize,
    pub scanned: usize,
    pub skipped: usize,
    pub filtered: usize,
    pub scan_complete: bool,
}

impl FileRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(FileRegistryInner::default())),
            notify: Arc::new(Notify::new()),
            catalog: Arc::new(Mutex::new(None)),
            masker: None,
        }
    }

    /// A registry whose catalog and series responses are masked.
    pub fn masked(masker: Arc<Masker>) -> Self {
        Self {
            masker: Some(masker),
            ..Self::new()
        }
    }

    pub fn masker(&self) -> Option<&Arc<Masker>> {
        self.masker.as_ref()
    }

    fn read(&self) -> RwLockReadGuard<'_, FileRegistryInner> {
        self.inner.read().expect("file registry lock poisoned")
    }

    fn write(&self) -> RwLockWriteGuard<'_, FileRegistryInner> {
        self.inner.write().expect("file registry lock poisoned")
    }

    pub fn from_files(files: Vec<FileEntry>) -> Self {
        let registry = Self::new();
        for file in files {
            registry.insert(file);
            registry.write().scanned += 1;
        }
        registry.mark_scan_complete();
        registry
    }

    pub fn insert(&self, mut file: FileEntry) -> usize {
        let mut inner = self.write();
        let index = inner.files.len();
        file.index = index;
        let summary = match &self.masker {
            Some(masker) => masker.summary(&file),
            None => FileSummary::from(&file),
        };
        inner.files.push(Arc::new(file));
        inner.summaries.push(summary);
        drop(inner);
        self.notify.notify_waiters();
        index
    }

    pub fn record_discovery(&self, record: DiscoveryRecord) {
        let mut inner = self.write();
        match record.disposition {
            DiscoveryDisposition::Selected => inner.scanned += 1,
            DiscoveryDisposition::Skipped => inner.skipped += 1,
            DiscoveryDisposition::Filtered => inner.filtered += 1,
        }
        if record.disposition != DiscoveryDisposition::Selected {
            if inner.recent_discovery.len() == DISCOVERY_RESPONSE_MAX_RECORDS {
                inner.recent_discovery.pop_front();
            }
            inner.recent_discovery.push_back(record);
        }
        drop(inner);
        self.notify.notify_waiters();
    }

    pub fn mark_scan_complete(&self) {
        self.write().scan_complete = true;
        self.notify.notify_waiters();
    }

    pub fn changed(&self) -> Notified<'_> {
        self.notify.notified()
    }

    /// A shared handle to the file at `index`; entries never change once registered.
    pub fn get(&self, index: usize) -> Option<Arc<FileEntry>> {
        self.read().files.get(index).cloned()
    }

    /// Shared handles to every registered file; cheap to take on each request.
    pub fn files_snapshot(&self) -> Vec<Arc<FileEntry>> {
        self.read().files.clone()
    }

    pub fn summaries_snapshot(&self) -> Vec<FileSummary> {
        self.read().summaries.clone()
    }

    /// The series catalog, serialized as its API response.
    pub fn series_catalog_json(&self) -> serde_json::Result<Bytes> {
        let (files, scan_complete) = {
            let inner = self.read();
            (inner.files.clone(), inner.scan_complete)
        };
        let key = (files.len(), scan_complete);
        if let Ok(cached) = self.catalog.lock() {
            if let Some((count, complete, json)) = cached.as_ref() {
                if (*count, *complete) == key {
                    return Ok(json.clone());
                }
            }
        }

        // A masked session groups and names series by hashed UIDs, so the
        // response and the identifiers derived from it carry no real UID.
        let inputs = files
            .iter()
            .map(|file| {
                let mut input = series_file_input(file);
                if let Some(masker) = &self.masker {
                    mask_series_uids(masker, &mut input);
                }
                input
            })
            .collect::<Vec<_>>();
        let mut frames_of_reference = HashMap::<(&str, &str), BTreeSet<&str>>::new();
        for input in &inputs {
            let uids = frames_of_reference
                .entry((&input.study_instance_uid, &input.series_instance_uid))
                .or_default();
            let uid = input.frame_of_reference_uid.as_str();
            if !uid.is_empty() {
                uids.insert(uid);
            }
        }
        let catalog = SeriesCatalog::build(inputs.iter().cloned());
        let response = SeriesCatalogResponse {
            series: catalog
                .series()
                .iter()
                .map(|group| {
                    let uids = frames_of_reference
                        .get(&(
                            group.id.study_instance_uid.as_str(),
                            group.id.series_instance_uid.as_str(),
                        ))
                        .map(|uids| uids.iter().map(|uid| uid.to_string()).collect())
                        .unwrap_or_default();
                    series_summary(group, uids)
                })
                .collect(),
            scan_complete,
        };
        let json = Bytes::from(serde_json::to_vec(&response)?);
        if let Ok(mut cached) = self.catalog.lock() {
            *cached = Some((key.0, key.1, json.clone()));
        }
        Ok(json)
    }

    /// The most recent skipped and filtered records, sorted by path.
    pub fn discovery_response_snapshot(&self) -> Vec<DiscoveryRecord> {
        let mut records = Vec::from(self.read().recent_discovery.clone());
        records.sort();
        records
    }

    pub fn status(&self) -> RegistryStatus {
        let inner = self.read();
        RegistryStatus {
            file_count: inner.files.len(),
            scanned: inner.scanned,
            skipped: inner.skipped,
            filtered: inner.filtered,
            scan_complete: inner.scan_complete,
        }
    }
}

fn series_file_input(file: &FileEntry) -> SeriesFileInput {
    let metadata = &file.series_metadata;
    let has_per_frame_geometry = !metadata.frame_image_positions_patient.is_empty()
        || !metadata.frame_image_orientations_patient.is_empty();
    let per_frame_ordering = if has_per_frame_geometry {
        (0..file.frame_count)
            .map(|frame_index| FrameOrderingInput {
                frame_index,
                ordering: OrderingInput {
                    image_position_patient: file.frame_image_position_patient(frame_index),
                    image_orientation_patient: file.frame_image_orientation_patient(frame_index),
                },
            })
            .collect()
    } else {
        Vec::new()
    };
    let navigation = if let Some(concatenation_uid) = metadata
        .concatenation_uid
        .as_ref()
        .filter(|uid| !uid.is_empty())
    {
        NavigationInput::Concatenation {
            concatenation_uid: concatenation_uid.clone(),
            concatenation_frame_offset_number: metadata.concatenation_frame_offset_number,
            in_concatenation_number: metadata.in_concatenation_number,
        }
    } else if file.sop_class_uid == uids::VL_WHOLE_SLIDE_MICROSCOPY_IMAGE_STORAGE {
        NavigationInput::Wsi {
            pyramid_uid: metadata.pyramid_uid.clone().filter(|uid| !uid.is_empty()),
            image_type_role: metadata
                .image_type
                .get(2)
                .cloned()
                .filter(|role| !role.is_empty()),
            total_pixel_matrix_rows: metadata.total_pixel_matrix_rows,
            total_pixel_matrix_columns: metadata.total_pixel_matrix_columns,
        }
    } else {
        NavigationInput::Ordinary
    };

    SeriesFileInput {
        file_index: file.index,
        path: file.path.clone(),
        study_instance_uid: file.study_instance_uid.clone(),
        series_instance_uid: file.series_instance_uid.clone(),
        frame_of_reference_uid: metadata.frame_of_reference_uid.clone(),
        sop_instance_uid: file.sop_instance_uid.clone(),
        frame_count: file.frame_count,
        instance_number: file.instance_number.trim().parse().ok(),
        ordering: OrderingInput {
            image_position_patient: metadata.image_position_patient,
            image_orientation_patient: metadata.image_orientation_patient,
        },
        per_frame_ordering,
        navigation,
    }
}

fn mask_series_uids(masker: &Masker, input: &mut SeriesFileInput) {
    for uid in [
        &mut input.study_instance_uid,
        &mut input.series_instance_uid,
        &mut input.frame_of_reference_uid,
        &mut input.sop_instance_uid,
    ] {
        *uid = masker.uid(uid);
    }
    match &mut input.navigation {
        NavigationInput::Ordinary => {}
        NavigationInput::Concatenation {
            concatenation_uid, ..
        } => *concatenation_uid = masker.uid(concatenation_uid),
        NavigationInput::Wsi { pyramid_uid, .. } => {
            if let Some(uid) = pyramid_uid {
                *uid = masker.uid(uid);
            }
        }
    }
}

fn series_summary(group: &SeriesGroup, frame_of_reference_uids: Vec<String>) -> SeriesSummary {
    let id = series_id(&group.id.study_instance_uid, &group.id.series_instance_uid);
    SeriesSummary {
        id: id.clone(),
        study_instance_uid: group.id.study_instance_uid.clone(),
        series_instance_uid: group.id.series_instance_uid.clone(),
        frame_of_reference_uids,
        stacks: group
            .stacks
            .iter()
            .map(|stack| stack_summary(&id, stack))
            .collect(),
    }
}

fn series_id(study_instance_uid: &str, series_instance_uid: &str) -> String {
    format!("study:{study_instance_uid}|series:{series_instance_uid}")
}

fn stack_summary(series_id: &str, stack: &SeriesStack) -> SeriesStackSummary {
    let (kind, identity, concatenation_uid, pyramid_uid, image_type_role, rows, columns) =
        match &stack.kind {
            NavigationKind::Ordinary => (
                "ordinary",
                "ordinary".to_string(),
                None,
                None,
                None,
                None,
                None,
            ),
            NavigationKind::Concatenation { concatenation_uid } => (
                "concatenation",
                format!("concatenation:{concatenation_uid}"),
                Some(concatenation_uid.clone()),
                None,
                None,
                None,
                None,
            ),
            NavigationKind::WsiPyramidLevel {
                pyramid_uid,
                image_type_role,
                total_pixel_matrix_rows,
                total_pixel_matrix_columns,
            } => (
                "wsi_pyramid_level",
                format!(
                    "pyramid:{pyramid_uid}|role:{}|matrix:{}x{}",
                    image_type_role.as_deref().unwrap_or(""),
                    total_pixel_matrix_rows.map_or_else(String::new, |value| value.to_string()),
                    total_pixel_matrix_columns.map_or_else(String::new, |value| value.to_string())
                ),
                None,
                Some(pyramid_uid.clone()),
                image_type_role.clone(),
                *total_pixel_matrix_rows,
                *total_pixel_matrix_columns,
            ),
            NavigationKind::WsiCompanion {
                sop_instance_uid,
                image_type_role,
            } => (
                "wsi_companion",
                format!("companion:{sop_instance_uid}"),
                None,
                None,
                image_type_role.clone(),
                None,
                None,
            ),
        };
    SeriesStackSummary {
        id: format!("{series_id}|stack:{identity}"),
        kind: kind.to_string(),
        concatenation_uid,
        pyramid_uid,
        image_type_role,
        total_pixel_matrix_rows: rows,
        total_pixel_matrix_columns: columns,
        frames: stack
            .frames
            .iter()
            .map(|frame| FrameRefSummary {
                virtual_index: frame.virtual_index,
                file_index: frame.file_index,
                frame_index: frame.frame_index,
                sop_instance_uid: frame.sop_instance_uid.clone(),
                instance_number: frame
                    .instance_number
                    .and_then(|value| value.try_into().ok()),
                position_along_normal_mm: frame.position_along_normal_mm,
            })
            .collect(),
        warnings: stack.warnings.iter().map(warning_summary).collect(),
    }
}

fn warning_summary(warning: &SeriesWarning) -> SeriesWarningSummary {
    let (code, message, file_indices) = match warning {
        SeriesWarning::MissingPositions { frames } => (
            "missing_positions",
            format!(
                "{} frame source(s) have no Image Position Patient",
                frames.len()
            ),
            frame_file_indices(frames.iter().map(|frame| frame.file_index)),
        ),
        SeriesWarning::DuplicatePositions { groups } => (
            "duplicate_positions",
            format!("{} duplicate projected position group(s)", groups.len()),
            frame_file_indices(
                groups
                    .iter()
                    .flat_map(|group| group.frames.iter().map(|frame| frame.file_index)),
            ),
        ),
        SeriesWarning::NonuniformSpacing {
            adjacent_spacing_mm,
        } => (
            "nonuniform_spacing",
            format!("adjacent projected spacing is {adjacent_spacing_mm:?} mm"),
            Vec::new(),
        ),
        SeriesWarning::InconsistentOrientation { frames } => (
            "inconsistent_orientation",
            format!(
                "{} frame source(s) differ from the reference orientation",
                frames.len()
            ),
            frame_file_indices(frames.iter().map(|frame| frame.file_index)),
        ),
        SeriesWarning::GantryTilt {
            frames,
            max_lateral_shift_mm,
        } => (
            "gantry_tilt",
            format!("maximum in-plane position shift is {max_lateral_shift_mm:.6} mm"),
            frame_file_indices(frames.iter().map(|frame| frame.file_index)),
        ),
    };
    SeriesWarningSummary {
        code: code.to_string(),
        message,
        file_indices,
    }
}

fn frame_file_indices(indices: impl IntoIterator<Item = usize>) -> Vec<usize> {
    indices
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl Default for FileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileRegistry, DISCOVERY_RESPONSE_MAX_RECORDS};
    use crate::loader::{DiscoveryDisposition, DiscoveryReason, DiscoveryRecord};
    use std::path::PathBuf;

    #[test]
    fn discovery_ledger_keeps_skipped_and_filtered_records_and_counts_all() {
        let registry = FileRegistry::new();
        registry.record_discovery(DiscoveryRecord {
            path: PathBuf::from("/scan/z-invalid.bin"),
            disposition: DiscoveryDisposition::Skipped,
            reason: DiscoveryReason::UnrecognizedFormat,
        });
        registry.record_discovery(DiscoveryRecord {
            path: PathBuf::from("/scan/a-selected.dcm"),
            disposition: DiscoveryDisposition::Selected,
            reason: DiscoveryReason::ValidDicom,
        });
        registry.record_discovery(DiscoveryRecord {
            path: PathBuf::from("/scan/m-filtered.dcm"),
            disposition: DiscoveryDisposition::Filtered,
            reason: DiscoveryReason::FilterMismatch,
        });

        let records = registry.discovery_response_snapshot();
        assert_eq!(
            records
                .iter()
                .map(|record| record.path.as_path())
                .collect::<Vec<_>>(),
            vec![
                std::path::Path::new("/scan/m-filtered.dcm"),
                std::path::Path::new("/scan/z-invalid.bin"),
            ]
        );
        assert_eq!(records[0].reason.code(), "filter_mismatch");
        assert_eq!(records[1].reason.code(), "unrecognized_format");

        let status = registry.status();
        assert_eq!(status.scanned, 1);
        assert_eq!(status.skipped, 1);
        assert_eq!(status.filtered, 1);

        let clone = registry.clone();
        assert_eq!(clone.discovery_response_snapshot(), records);
        assert!(FileRegistry::new().discovery_response_snapshot().is_empty());
    }

    #[test]
    fn discovery_response_snapshot_is_bounded_to_recent_records() {
        let registry = FileRegistry::new();
        for index in 0..DISCOVERY_RESPONSE_MAX_RECORDS + 2 {
            registry.record_discovery(DiscoveryRecord {
                path: PathBuf::from(format!("/scan/{index:04}.dcm")),
                disposition: DiscoveryDisposition::Skipped,
                reason: DiscoveryReason::DicomParseFailed,
            });
        }

        let response = registry.discovery_response_snapshot();
        assert_eq!(response.len(), DISCOVERY_RESPONSE_MAX_RECORDS);
        assert_eq!(response[0].path, PathBuf::from("/scan/0002.dcm"));
        assert_eq!(
            response.last().expect("last bounded record").path,
            PathBuf::from(format!(
                "/scan/{:04}.dcm",
                DISCOVERY_RESPONSE_MAX_RECORDS + 1
            ))
        );
        assert_eq!(
            registry.status().skipped,
            DISCOVERY_RESPONSE_MAX_RECORDS + 2,
            "counts cover every record even though only recent ones are kept"
        );
    }
}
