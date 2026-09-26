use crate::api::contracts::WindowPreset;
use crate::types::{
    DicomLut, FileEntry, LoadReport, NativePixelDataKind, NativePixelMetadata, OverlayPlane,
    PatientOrientation, PatientPosition, PresentationMetadata, RectangularDisplayShutter,
    SeriesMetadata,
};
use anyhow::{Context, Result};
use dicom_core::header::HasLength;
use dicom_dictionary_std::{tags, uids};
use dicom_encoding::text::SpecificCharacterSet;
use dicom_encoding::{TransferSyntax, TransferSyntaxIndex};
use dicom_object::{FileMetaTable, OpenFileOptions};
use dicom_parser::dataset::read::DataSetReader;
use dicom_parser::dataset::DataToken;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use rayon::prelude::*;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task;
use walkdir::WalkDir;

const DISCOVERY_SEND_RETRY_INTERVAL: Duration = Duration::from_millis(1);
const DISCOVERY_COLLECT_CAPACITY: usize = 64;

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    pub recursive: bool,
    pub filters: Vec<ScanFilter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFilterField {
    PatientId,
    PatientName,
    StudyDescription,
    StudyDate,
    StudyUid,
    SeriesDescription,
    SeriesNumber,
    SeriesUid,
    Modality,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFilter {
    pub field: ScanFilterField,
    pub value: String,
}

impl ScanFilter {
    pub const VALID_FIELDS: &'static [&'static str] = &[
        "patient_id",
        "patient_name",
        "study_description",
        "study_date",
        "study_uid",
        "series_description",
        "series_number",
        "series_uid",
        "modality",
    ];

    pub fn matches(&self, entry: &FileEntry) -> bool {
        let haystack = match self.field {
            ScanFilterField::PatientId => &entry.patient_id,
            ScanFilterField::PatientName => &entry.patient_name,
            ScanFilterField::StudyDescription => &entry.study_description,
            ScanFilterField::StudyDate => &entry.study_date,
            ScanFilterField::StudyUid => &entry.study_instance_uid,
            ScanFilterField::SeriesDescription => &entry.series_description,
            ScanFilterField::SeriesNumber => &entry.series_number,
            ScanFilterField::SeriesUid => &entry.series_instance_uid,
            ScanFilterField::Modality => &entry.modality,
        };
        haystack.to_lowercase().contains(&self.value.to_lowercase())
    }
}

impl std::fmt::Display for ScanFilter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}={}", self.field, self.value)
    }
}

impl std::fmt::Display for ScanFilterField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let field = match self {
            ScanFilterField::PatientId => "patient_id",
            ScanFilterField::PatientName => "patient_name",
            ScanFilterField::StudyDescription => "study_description",
            ScanFilterField::StudyDate => "study_date",
            ScanFilterField::StudyUid => "study_uid",
            ScanFilterField::SeriesDescription => "series_description",
            ScanFilterField::SeriesNumber => "series_number",
            ScanFilterField::SeriesUid => "series_uid",
            ScanFilterField::Modality => "modality",
        };
        formatter.write_str(field)
    }
}

impl FromStr for ScanFilter {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let (field, value) = raw
            .split_once('=')
            .ok_or_else(|| scan_filter_parse_error(raw))?;
        let field_name = field.trim().to_ascii_lowercase();
        let field = match field_name.as_str() {
            "patient_id" => ScanFilterField::PatientId,
            "patient_name" => ScanFilterField::PatientName,
            "study_description" => ScanFilterField::StudyDescription,
            "study_date" => ScanFilterField::StudyDate,
            "study_uid" => ScanFilterField::StudyUid,
            "series_description" => ScanFilterField::SeriesDescription,
            "series_number" => ScanFilterField::SeriesNumber,
            "series_uid" => ScanFilterField::SeriesUid,
            "modality" => ScanFilterField::Modality,
            _ => return Err(scan_filter_parse_error(raw)),
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(scan_filter_parse_error(raw));
        }
        Ok(Self {
            field,
            value: value.to_string(),
        })
    }
}

fn scan_filter_parse_error(raw: &str) -> String {
    format!(
        "invalid scan filter `{raw}`; expected FIELD=VALUE where FIELD is one of: {}",
        ScanFilter::VALID_FIELDS.join(", ")
    )
}

/// The outcome of inspecting one discovery candidate.
#[derive(Debug)]
pub enum DiscoveryEvent {
    Selected {
        file: Box<FileEntry>,
        record: DiscoveryRecord,
    },
    SkippedInput(DiscoveryRecord),
    FilteredInput(DiscoveryRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiscoveryDisposition {
    Selected,
    Skipped,
    Filtered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiscoveryReason {
    ValidDicom,
    InputPathUnavailable,
    DirectoryEntryUnreadable,
    MissingPart10Preamble,
    DicomParseFailed,
    UnsupportedMediaDirectory,
    InspectionFailed,
    FilterMismatch,
}

impl DiscoveryReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ValidDicom => "valid_dicom",
            Self::InputPathUnavailable => "input_path_unavailable",
            Self::DirectoryEntryUnreadable => "directory_entry_unreadable",
            Self::MissingPart10Preamble => "missing_part10_preamble",
            Self::DicomParseFailed => "dicom_parse_failed",
            Self::UnsupportedMediaDirectory => "unsupported_media_directory",
            Self::InspectionFailed => "inspection_failed",
            Self::FilterMismatch => "filter_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveryRecord {
    pub path: PathBuf,
    pub disposition: DiscoveryDisposition,
    pub reason: DiscoveryReason,
}

impl DiscoveryRecord {
    fn new(path: &Path, disposition: DiscoveryDisposition, reason: DiscoveryReason) -> Self {
        Self {
            path: normalize_input_path(path),
            disposition,
            reason,
        }
    }
}

enum EntryInspection {
    Selected(Box<FileEntry>),
    Skipped(DiscoveryReason),
}

#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    pub files_found: usize,
    pub skipped: usize,
    pub filtered: usize,
    pub searched_recursive: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryCancellation {
    cancelled: Arc<AtomicBool>,
}

impl DiscoveryCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCancellationReason {
    Requested,
    EventReceiverClosed,
}

impl std::fmt::Display for DiscoveryCancellationReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Requested => formatter.write_str("cancellation requested"),
            Self::EventReceiverClosed => formatter.write_str("event receiver closed"),
        }
    }
}

#[derive(Debug, Error)]
#[error("DICOM discovery cancelled: {reason}")]
pub struct DiscoveryCancelled {
    reason: DiscoveryCancellationReason,
}

impl DiscoveryCancelled {
    fn new(reason: DiscoveryCancellationReason) -> Self {
        Self { reason }
    }

    pub fn reason(&self) -> DiscoveryCancellationReason {
        self.reason
    }
}

pub fn discovery_cancellation_reason(error: &anyhow::Error) -> Option<DiscoveryCancellationReason> {
    error
        .downcast_ref::<DiscoveryCancelled>()
        .map(DiscoveryCancelled::reason)
}

/// Discover `paths` to completion and return the selected files sorted by
/// path, indexed in that order.
///
/// A collecting adapter over [`discover_progressive`] for callers that need
/// the whole result at once rather than the startup event stream.
pub async fn discover(paths: &[PathBuf], options: DiscoverOptions) -> Result<LoadReport> {
    let (events_tx, mut events_rx) = mpsc::channel(DISCOVERY_COLLECT_CAPACITY);
    let scan = discover_progressive(paths, options, events_tx, DiscoveryCancellation::new());
    let collect = async {
        let mut files = Vec::new();
        while let Some(event) = events_rx.recv().await {
            if let DiscoveryEvent::Selected { file, .. } = event {
                files.push(*file);
            }
        }
        files
    };
    let (report, mut files) = tokio::join!(scan, collect);
    let report = report?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    for (index, file) in files.iter_mut().enumerate() {
        file.index = index;
    }
    Ok(LoadReport {
        files,
        skipped: report.skipped,
        filtered: report.filtered,
        searched_recursive: report.searched_recursive,
    })
}

/// Inspect `paths` on blocking workers, streaming each outcome as an event.
///
/// Returns a [`DiscoveryCancelled`] error when `cancellation` is requested or
/// the event receiver closes; a full channel applies backpressure without
/// blocking cancellation.
pub async fn discover_progressive(
    paths: &[PathBuf],
    options: DiscoverOptions,
    events: mpsc::Sender<DiscoveryEvent>,
    cancellation: DiscoveryCancellation,
) -> Result<DiscoveryReport> {
    let paths = paths.to_vec();
    task::spawn_blocking(move || {
        discover_progressive_blocking(&paths, &options, events, &cancellation)
    })
    .await
    .context("loader worker panicked")?
}

fn collect_candidates(
    paths: &[PathBuf],
    options: &DiscoverOptions,
    events: &mpsc::Sender<DiscoveryEvent>,
    cancellation: &DiscoveryCancellation,
) -> std::result::Result<(Vec<PathBuf>, Vec<DiscoveryRecord>), DiscoveryCancelled> {
    let check_active = || ensure_discovery_active(events, cancellation);
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for path in paths {
        check_active()?;

        if path.is_file() {
            candidates.push(path.clone());
            continue;
        }

        if path.is_dir() {
            let mut walker = WalkDir::new(path).follow_links(false);
            if !options.recursive {
                walker = walker.max_depth(1);
            }

            let mut entries = walker.into_iter();
            loop {
                check_active()?;
                let Some(entry) = entries.next() else {
                    break;
                };
                match entry {
                    Ok(dir_entry) if dir_entry.path().is_file() => {
                        candidates.push(dir_entry.path().to_path_buf());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let skipped_path = error.path().unwrap_or(path);
                        skipped.push(DiscoveryRecord::new(
                            skipped_path,
                            DiscoveryDisposition::Skipped,
                            DiscoveryReason::DirectoryEntryUnreadable,
                        ));
                        eprintln!("dcmview: warning — could not read path entry: {error}");
                    }
                }
            }
            continue;
        }

        skipped.push(DiscoveryRecord::new(
            path,
            DiscoveryDisposition::Skipped,
            DiscoveryReason::InputPathUnavailable,
        ));
        eprintln!(
            "dcmview: warning — input path does not exist or is unsupported: {}",
            path.display()
        );
    }

    check_active()?;
    Ok((candidates, skipped))
}

fn discover_progressive_blocking(
    paths: &[PathBuf],
    options: &DiscoverOptions,
    events: mpsc::Sender<DiscoveryEvent>,
    cancellation: &DiscoveryCancellation,
) -> Result<DiscoveryReport> {
    let (candidates, initial_skipped) = collect_candidates(paths, options, &events, cancellation)?;
    for record in initial_skipped.iter().cloned() {
        send_discovery_event(&events, cancellation, DiscoveryEvent::SkippedInput(record))?;
    }

    let files_found = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(initial_skipped.len());
    let filtered = AtomicUsize::new(0);

    let processing_result: std::result::Result<(), DiscoveryCancelled> = candidates
        .par_iter()
        .try_for_each_with(events.clone(), |events, candidate| {
            ensure_discovery_active(events, cancellation)?;

            match build_entry(candidate) {
                Ok(EntryInspection::Selected(entry))
                    if matches_filters(&entry, &options.filters) =>
                {
                    let record = DiscoveryRecord::new(
                        candidate,
                        DiscoveryDisposition::Selected,
                        DiscoveryReason::ValidDicom,
                    );
                    send_discovery_event(
                        events,
                        cancellation,
                        DiscoveryEvent::Selected {
                            file: entry,
                            record,
                        },
                    )?;
                    files_found.fetch_add(1, Ordering::Relaxed);
                }
                Ok(EntryInspection::Selected(_)) => {
                    let record = DiscoveryRecord::new(
                        candidate,
                        DiscoveryDisposition::Filtered,
                        DiscoveryReason::FilterMismatch,
                    );
                    send_discovery_event(
                        events,
                        cancellation,
                        DiscoveryEvent::FilteredInput(record),
                    )?;
                    filtered.fetch_add(1, Ordering::Relaxed);
                }
                Ok(EntryInspection::Skipped(reason)) => {
                    let record =
                        DiscoveryRecord::new(candidate, DiscoveryDisposition::Skipped, reason);
                    send_discovery_event(
                        events,
                        cancellation,
                        DiscoveryEvent::SkippedInput(record),
                    )?;
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    let record = DiscoveryRecord::new(
                        candidate,
                        DiscoveryDisposition::Skipped,
                        DiscoveryReason::InspectionFailed,
                    );
                    send_discovery_event(
                        events,
                        cancellation,
                        DiscoveryEvent::SkippedInput(record),
                    )?;
                    skipped.fetch_add(1, Ordering::Relaxed);
                    eprintln!("dcmview: warning — failed to inspect DICOM: {error}");
                }
            }

            Ok(())
        });
    processing_result?;
    ensure_discovery_active(&events, cancellation)?;

    Ok(DiscoveryReport {
        files_found: files_found.load(Ordering::Relaxed),
        skipped: skipped.load(Ordering::Relaxed),
        filtered: filtered.load(Ordering::Relaxed),
        searched_recursive: options.recursive,
    })
}

fn ensure_discovery_active(
    events: &mpsc::Sender<DiscoveryEvent>,
    cancellation: &DiscoveryCancellation,
) -> std::result::Result<(), DiscoveryCancelled> {
    if cancellation.is_cancelled() {
        return Err(DiscoveryCancelled::new(
            DiscoveryCancellationReason::Requested,
        ));
    }
    if events.is_closed() {
        return Err(DiscoveryCancelled::new(
            DiscoveryCancellationReason::EventReceiverClosed,
        ));
    }
    Ok(())
}

fn send_discovery_event(
    events: &mpsc::Sender<DiscoveryEvent>,
    cancellation: &DiscoveryCancellation,
    mut event: DiscoveryEvent,
) -> std::result::Result<(), DiscoveryCancelled> {
    loop {
        ensure_discovery_active(events, cancellation)?;
        match events.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(unsent_event)) => {
                event = unsent_event;
                thread::park_timeout(DISCOVERY_SEND_RETRY_INTERVAL);
            }
            Err(TrySendError::Closed(_)) => {
                return Err(DiscoveryCancelled::new(
                    DiscoveryCancellationReason::EventReceiverClosed,
                ));
            }
        }
    }
}

fn matches_filters(entry: &FileEntry, filters: &[ScanFilter]) -> bool {
    filters.iter().all(|filter| filter.matches(entry))
}

fn build_entry(path: &Path) -> Result<EntryInspection> {
    if !has_dicm_preamble(path)? {
        return Ok(EntryInspection::Skipped(
            DiscoveryReason::MissingPart10Preamble,
        ));
    }

    let obj = match read_discovery_metadata(path) {
        Ok(obj) => obj,
        Err(_) => return Ok(EntryInspection::Skipped(DiscoveryReason::DicomParseFailed)),
    };
    if !valid_discovery_structure(&obj) {
        return Ok(EntryInspection::Skipped(DiscoveryReason::DicomParseFailed));
    }

    let transfer_syntax_uid = obj.meta().transfer_syntax().to_string();
    let patient_id = read_str(&obj, "PatientID").unwrap_or_default();
    let patient_name = read_str(&obj, "PatientName").unwrap_or_default();
    let modality = read_str(&obj, "Modality").unwrap_or_default();
    let sop_instance_uid = read_str(&obj, "SOPInstanceUID").unwrap_or_default();
    let sop_class_uid = read_str(&obj, "SOPClassUID").unwrap_or_default();
    if sop_class_uid == "1.2.840.10008.1.3.10" {
        return Ok(EntryInspection::Skipped(
            DiscoveryReason::UnsupportedMediaDirectory,
        ));
    }
    let study_instance_uid = read_str(&obj, "StudyInstanceUID").unwrap_or_default();
    let study_date = read_str(&obj, "StudyDate").unwrap_or_default();
    let study_description = read_str(&obj, "StudyDescription").unwrap_or_default();
    let series_instance_uid = read_str(&obj, "SeriesInstanceUID").unwrap_or_default();
    let series_number = read_str(&obj, "SeriesNumber").unwrap_or_default();
    let series_description = read_str(&obj, "SeriesDescription").unwrap_or_default();
    let instance_number = read_str(&obj, "InstanceNumber").unwrap_or_default();
    let frame_count = read_u32(&obj, "NumberOfFrames").unwrap_or(1);
    let frame_of_reference_uid = read_str(&obj, "FrameOfReferenceUID").unwrap_or_default();
    let image_position_patient = read_exact_f64s(&obj, "ImagePositionPatient");
    let image_orientation_patient = read_exact_f64s(&obj, "ImageOrientationPatient");
    let top_level_pixel_spacing = read_positive_f64_pair(&obj, "PixelSpacing");
    let (
        frame_image_positions_patient,
        frame_image_orientations_patient,
        frame_pixel_spacings,
        shared_pixel_spacing,
    ) = read_frame_patient_geometry(
        &obj,
        frame_count,
        image_position_patient,
        image_orientation_patient,
        top_level_pixel_spacing,
    );
    let concatenation_uid = read_str(&obj, "ConcatenationUID");
    let in_concatenation_number = read_u32(&obj, "InConcatenationNumber");
    let in_concatenation_total_number = read_u32(&obj, "InConcatenationTotalNumber");
    let concatenation_frame_offset_number = read_u32(&obj, "ConcatenationFrameOffsetNumber");
    let sop_instance_uid_of_concatenation_source =
        read_str(&obj, "SOPInstanceUIDOfConcatenationSource");
    let image_type = read_strings(&obj, "ImageType");
    let pyramid_uid = read_str(&obj, "PyramidUID");
    let dimension_organization_type = read_str(&obj, "DimensionOrganizationType");
    let dimension_organization_uids = read_sequence_strings(
        &obj,
        tags::DIMENSION_ORGANIZATION_SEQUENCE,
        tags::DIMENSION_ORGANIZATION_UID,
    );
    let image_orientation_slide = read_exact_f64s(&obj, "ImageOrientationSlide");
    let total_pixel_matrix_rows = read_u32(&obj, "TotalPixelMatrixRows");
    let total_pixel_matrix_columns = read_u32(&obj, "TotalPixelMatrixColumns");
    let total_pixel_matrix_focal_planes = read_u32(&obj, "TotalPixelMatrixFocalPlanes");
    let number_of_optical_paths = read_u32(&obj, "NumberOfOpticalPaths");
    let container_identifier = read_str(&obj, "ContainerIdentifier");
    let specimen_uids = read_sequence_strings(
        &obj,
        tags::SPECIMEN_DESCRIPTION_SEQUENCE,
        tags::SPECIMEN_UID,
    );
    let optical_path_identifiers = read_sequence_strings(
        &obj,
        tags::OPTICAL_PATH_SEQUENCE,
        tags::OPTICAL_PATH_IDENTIFIER,
    );
    let rows = read_u32(&obj, "Rows").unwrap_or(0);
    let columns = read_u32(&obj, "Columns").unwrap_or(0);
    let bits_allocated = read_u32(&obj, "BitsAllocated").unwrap_or(8);
    let planar_configuration = read_u32(&obj, "PlanarConfiguration");
    let bits_stored = read_u32(&obj, "BitsStored");
    let high_bit = read_u32(&obj, "HighBit");
    let pixel_spacing = top_level_pixel_spacing.or(shared_pixel_spacing);
    let pixel_aspect_ratio = read_positive_u32_pair(&obj, "PixelAspectRatio");
    let normalized_pixel_aspect = normalize_pixel_aspect(pixel_spacing, pixel_aspect_ratio);
    let modality_lut = read_lut_sequence(&obj, tags::MODALITY_LUT_SEQUENCE);
    let voi_lut = read_lut_sequence(&obj, tags::VOILUT_SEQUENCE);
    let presentation = read_presentation_metadata(&obj);
    let pixel_representation = read_u32(&obj, "PixelRepresentation").unwrap_or(0);
    let samples_per_pixel = read_u32(&obj, "SamplesPerPixel").unwrap_or(1).max(1);
    let photometric_interpretation =
        read_str(&obj, "PhotometricInterpretation").unwrap_or_else(|| "MONOCHROME2".to_string());
    let rescale_slope = read_f64(&obj, "RescaleSlope").unwrap_or(1.0);
    let rescale_intercept = read_f64(&obj, "RescaleIntercept").unwrap_or(0.0);
    let pixel_data_kind = find_native_pixel_data_kind(path, &transfer_syntax_uid)?;
    let has_pixels = pixel_data_kind.is_some();
    let default_window = match (
        read_f64(&obj, "WindowCenter"),
        read_f64(&obj, "WindowWidth"),
    ) {
        (Some(center), Some(width)) => Some(WindowPreset { center, width }),
        _ => None,
    };

    let fallback_label = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let label = build_label(&patient_id, &modality, &study_date, &fallback_label);

    Ok(EntryInspection::Selected(Box::new(FileEntry {
        index: 0,
        path: path.to_path_buf(),
        label,
        patient_id,
        patient_name,
        study_instance_uid,
        study_date,
        study_description,
        series_instance_uid,
        series_number,
        series_description,
        modality,
        instance_number,
        sop_instance_uid,
        sop_class_uid,
        series_metadata: Box::new(SeriesMetadata {
            native_pixel: NativePixelMetadata {
                planar_configuration,
                bits_stored,
                high_bit,
                pixel_data_kind,
                pixel_spacing,
                pixel_aspect_ratio,
                normalized_pixel_aspect,
                modality_lut,
                voi_lut,
            },
            presentation,
            frame_of_reference_uid,
            image_position_patient,
            image_orientation_patient,
            frame_image_positions_patient,
            frame_image_orientations_patient,
            frame_pixel_spacings,
            concatenation_uid,
            in_concatenation_number,
            in_concatenation_total_number,
            concatenation_frame_offset_number,
            sop_instance_uid_of_concatenation_source,
            image_type,
            pyramid_uid,
            dimension_organization_type,
            dimension_organization_uids,
            image_orientation_slide,
            total_pixel_matrix_rows,
            total_pixel_matrix_columns,
            total_pixel_matrix_focal_planes,
            number_of_optical_paths,
            container_identifier,
            specimen_uids,
            optical_path_identifiers,
        }),
        has_pixels,
        frame_count,
        rows,
        columns,
        bits_allocated,
        pixel_representation,
        samples_per_pixel,
        photometric_interpretation,
        rescale_slope,
        rescale_intercept,
        transfer_syntax_uid,
        default_window,
    })))
}

fn valid_discovery_structure(object: &dicom_object::DefaultDicomObject) -> bool {
    let character_set_valid = object
        .element(tags::SPECIFIC_CHARACTER_SET)
        .ok()
        .and_then(|element| element.to_str().ok())
        .map(|value| valid_specific_character_set(&value))
        .unwrap_or(true);
    character_set_valid && !has_odd_defined_item_length(object)
}

fn valid_specific_character_set(value: &str) -> bool {
    value.split('\\').all(|component| {
        let component = component.trim();
        component.is_empty() || SpecificCharacterSet::from_code(component).is_some()
    })
}

fn has_odd_defined_item_length(
    object: &dicom_object::InMemDicomObject<dicom_dictionary_std::StandardDataDictionary>,
) -> bool {
    object.iter().any(|element| {
        element.items().is_some_and(|items| {
            items.iter().any(|item| {
                item.length().get().is_some_and(|length| length % 2 != 0)
                    || has_odd_defined_item_length(item)
            })
        })
    })
}

fn read_discovery_metadata(path: &Path) -> Result<dicom_object::DefaultDicomObject> {
    Ok(OpenFileOptions::new()
        // Float and double-float pixel data precede the conventional Pixel Data
        // tag, so stopping there would materialize those payloads during scan.
        .read_until(tags::FLOAT_PIXEL_DATA)
        .open_file(path)?)
}

fn normalize_input_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    lexical_normalize(&absolute)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn has_dicm_preamble(path: &Path) -> Result<bool> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut preamble = [0_u8; 132];
    match file.read_exact(&mut preamble) {
        Ok(()) => Ok(&preamble[128..132] == b"DICM"),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// The kind of the data set's own pixel data element, if it has one.
///
/// Walks element headers without reading pixel values and counts only
/// top-level elements, so a pixel element nested in a sequence (an Icon Image
/// Sequence, for example) or pixel-tag bytes inside another value never make
/// a no-pixel object look like an image.
fn find_native_pixel_data_kind(
    path: &Path,
    transfer_syntax_uid: &str,
) -> Result<Option<NativePixelDataKind>> {
    let mut reader = io::BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    reader
        .seek(io::SeekFrom::Start(128))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    FileMetaTable::from_reader(&mut reader)
        .with_context(|| format!("failed to read file meta: {}", path.display()))?;
    let transfer_syntax = TransferSyntaxRegistry
        .get(transfer_syntax_uid)
        .with_context(|| format!("unknown transfer syntax {transfer_syntax_uid}"))?;
    let kind = if transfer_syntax_uid == uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN {
        top_level_pixel_data_kind(flate2::read::DeflateDecoder::new(reader), transfer_syntax)
    } else {
        top_level_pixel_data_kind(reader, transfer_syntax)
    };
    kind.with_context(|| format!("failed to parse {}", path.display()))
}

fn top_level_pixel_data_kind(
    source: impl Read,
    transfer_syntax: &TransferSyntax,
) -> Result<Option<NativePixelDataKind>> {
    let tokens = DataSetReader::new_with_ts(source, transfer_syntax)?;
    let mut depth = 0_usize;
    for token in tokens {
        match token? {
            DataToken::PixelSequenceStart if depth == 0 => {
                return Ok(Some(NativePixelDataKind::Integer));
            }
            DataToken::SequenceStart { .. } | DataToken::PixelSequenceStart => depth += 1,
            DataToken::SequenceEnd => depth = depth.saturating_sub(1),
            DataToken::ElementHeader(header) if depth == 0 => {
                let kind = match header.tag {
                    tags::PIXEL_DATA => NativePixelDataKind::Integer,
                    tags::FLOAT_PIXEL_DATA => NativePixelDataKind::Float32,
                    tags::DOUBLE_FLOAT_PIXEL_DATA => NativePixelDataKind::Float64,
                    _ => continue,
                };
                return Ok(Some(kind));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn read_str(obj: &dicom_object::DefaultDicomObject, name: &str) -> Option<String> {
    read_strings(obj, name).into_iter().next()
}

fn read_strings(obj: &dicom_object::DefaultDicomObject, name: &str) -> Vec<String> {
    obj.element_by_name(name)
        .ok()
        .and_then(|element| element.to_str().ok())
        .map(|value| split_dicom_values(value.as_ref()))
        .unwrap_or_default()
}

fn split_dicom_values(raw: &str) -> Vec<String> {
    raw.split('\\')
        .map(str::trim)
        .map(ToString::to_string)
        .collect()
}

fn read_u32(obj: &dicom_object::DefaultDicomObject, name: &str) -> Option<u32> {
    read_str(obj, name)?.parse::<u32>().ok()
}

fn read_i32(obj: &dicom_object::DefaultDicomObject, name: &str) -> Option<i32> {
    read_str(obj, name)?.parse::<i32>().ok()
}

fn read_f64(obj: &dicom_object::DefaultDicomObject, name: &str) -> Option<f64> {
    read_str(obj, name)?.parse::<f64>().ok()
}

fn read_positive_f64_pair(obj: &dicom_object::DefaultDicomObject, name: &str) -> Option<[f64; 2]> {
    let values = read_exact_f64s(obj, name)?;
    values.iter().all(|value| *value > 0.0).then_some(values)
}

fn read_positive_u32_pair(obj: &dicom_object::DefaultDicomObject, name: &str) -> Option<[u32; 2]> {
    let values = read_strings(obj, name)
        .into_iter()
        .map(|value| value.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    let values: [u32; 2] = values.try_into().ok()?;
    values.iter().all(|value| *value > 0).then_some(values)
}

fn normalize_pixel_aspect(
    pixel_spacing: Option<[f64; 2]>,
    pixel_aspect_ratio: Option<[u32; 2]>,
) -> Option<[f64; 2]> {
    let [row, column] = pixel_spacing.or_else(|| {
        pixel_aspect_ratio.map(|[vertical, horizontal]| [vertical as f64, horizontal as f64])
    })?;
    Some([row / column, 1.0])
}

fn read_lut_sequence(
    obj: &dicom_object::DefaultDicomObject,
    sequence_tag: dicom_core::Tag,
) -> Option<DicomLut> {
    let item = obj.element(sequence_tag).ok()?.items()?.first()?;
    let descriptor = item
        .element(tags::LUT_DESCRIPTOR)
        .ok()?
        .to_multi_int::<i32>()
        .ok()?;
    let [entry_count, first_mapped_value, bits_per_entry]: [i32; 3] = descriptor.try_into().ok()?;
    let entry_count = if entry_count == 0 {
        65_536
    } else {
        usize::try_from(entry_count).ok()?
    };
    let bits_per_entry = u16::try_from(bits_per_entry).ok()?;
    if !matches!(bits_per_entry, 8 | 16) {
        return None;
    }
    let data = item.element(tags::LUT_DATA).ok()?;
    let entries = if bits_per_entry == 8 {
        let bytes = data.to_bytes().ok()?;
        if bytes.len() >= entry_count && bytes.len() <= entry_count.saturating_add(1) {
            bytes
                .iter()
                .take(entry_count)
                .map(|value| u16::from(*value))
                .collect()
        } else {
            data.to_multi_int::<u16>()
                .ok()?
                .into_iter()
                .map(|value| value & 0x00FF)
                .collect()
        }
    } else {
        data.to_multi_int::<u16>().ok()?
    };
    if entries.len() < entry_count {
        return None;
    }
    Some(DicomLut {
        first_mapped_value,
        bits_per_entry,
        entries: entries.into_iter().take(entry_count).collect(),
    })
}

fn read_presentation_metadata(obj: &dicom_object::DefaultDicomObject) -> PresentationMetadata {
    PresentationMetadata {
        overlay_planes: read_overlay_planes(obj),
        rectangular_shutter: read_rectangular_display_shutter(obj),
    }
}

fn read_overlay_planes(obj: &dicom_object::DefaultDicomObject) -> Vec<OverlayPlane> {
    (0x6000_u16..=0x601e)
        .step_by(2)
        .filter_map(|group| read_overlay_plane(obj, group))
        .collect()
}

fn read_overlay_plane(obj: &dicom_object::DefaultDicomObject, group: u16) -> Option<OverlayPlane> {
    let rows = read_u32_tag(obj, dicom_core::Tag(group, 0x0010))?;
    let columns = read_u32_tag(obj, dicom_core::Tag(group, 0x0011))?;
    let origin = obj
        .element(dicom_core::Tag(group, 0x0050))
        .ok()?
        .to_multi_int::<i32>()
        .ok()?
        .try_into()
        .ok()?;
    if rows == 0
        || columns == 0
        || read_u32_tag(obj, dicom_core::Tag(group, 0x0100))? != 1
        || read_u32_tag(obj, dicom_core::Tag(group, 0x0102))? != 0
    {
        return None;
    }
    let number_of_frames = read_u32_tag(obj, dicom_core::Tag(group, 0x0015)).unwrap_or(1);
    let image_frame_origin = read_u32_tag(obj, dicom_core::Tag(group, 0x0051)).unwrap_or(1);
    if number_of_frames == 0 || image_frame_origin == 0 {
        return None;
    }
    let required_words = u64::from(rows)
        .checked_mul(u64::from(columns))?
        .checked_mul(u64::from(number_of_frames))?
        .checked_add(15)?
        / 16;
    let required_words = usize::try_from(required_words).ok()?;
    let mut data = obj
        .element(dicom_core::Tag(group, 0x3000))
        .ok()?
        .to_multi_int::<u16>()
        .ok()?;
    if data.len() < required_words {
        return None;
    }
    data.truncate(required_words);

    Some(OverlayPlane {
        group,
        rows,
        columns,
        origin,
        overlay_type: read_string_tag(obj, dicom_core::Tag(group, 0x0040)).unwrap_or_default(),
        number_of_frames,
        image_frame_origin,
        data,
    })
}

fn read_rectangular_display_shutter(
    obj: &dicom_object::DefaultDicomObject,
) -> Option<RectangularDisplayShutter> {
    if !read_strings(obj, "ShutterShape")
        .iter()
        .any(|shape| shape.eq_ignore_ascii_case("RECTANGULAR"))
    {
        return None;
    }
    let shutter = RectangularDisplayShutter {
        left_vertical_edge: read_i32(obj, "ShutterLeftVerticalEdge")?,
        right_vertical_edge: read_i32(obj, "ShutterRightVerticalEdge")?,
        upper_horizontal_edge: read_i32(obj, "ShutterUpperHorizontalEdge")?,
        lower_horizontal_edge: read_i32(obj, "ShutterLowerHorizontalEdge")?,
        presentation_value: u16::try_from(read_u32(obj, "ShutterPresentationValue")?).ok()?,
    };
    (shutter.left_vertical_edge <= shutter.right_vertical_edge
        && shutter.upper_horizontal_edge <= shutter.lower_horizontal_edge)
        .then_some(shutter)
}

fn read_u32_tag(obj: &dicom_object::DefaultDicomObject, tag: dicom_core::Tag) -> Option<u32> {
    obj.element(tag).ok()?.to_int::<u32>().ok()
}

fn read_string_tag(obj: &dicom_object::DefaultDicomObject, tag: dicom_core::Tag) -> Option<String> {
    obj.element(tag)
        .ok()?
        .to_str()
        .ok()
        .map(|value| value.trim().to_string())
}

fn read_exact_f64s<const N: usize>(
    obj: &dicom_object::DefaultDicomObject,
    name: &str,
) -> Option<[f64; N]> {
    let values = read_strings(obj, name)
        .into_iter()
        .map(|value| value.parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let values: [f64; N] = values.try_into().ok()?;
    values
        .iter()
        .all(|value| value.is_finite())
        .then_some(values)
}

fn read_sequence_strings(
    obj: &dicom_object::DefaultDicomObject,
    sequence_tag: dicom_core::Tag,
    value_tag: dicom_core::Tag,
) -> Vec<String> {
    let Some(items) = obj
        .element(sequence_tag)
        .ok()
        .and_then(|element| element.items())
    else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| item.element(value_tag).ok())
        .filter_map(|element| element.to_str().ok())
        .flat_map(|value| split_dicom_values(value.as_ref()))
        .filter(|value| !value.is_empty())
        .collect()
}

type FramePatientGeometry = (
    Vec<Option<PatientPosition>>,
    Vec<Option<PatientOrientation>>,
    Vec<Option<[f64; 2]>>,
    Option<[f64; 2]>,
);

fn read_frame_patient_geometry(
    obj: &dicom_object::DefaultDicomObject,
    frame_count: u32,
    top_level_position: Option<PatientPosition>,
    top_level_orientation: Option<PatientOrientation>,
    top_level_pixel_spacing: Option<[f64; 2]>,
) -> FramePatientGeometry {
    let shared_orientation = obj
        .element(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE)
        .ok()
        .and_then(|element| element.items())
        .and_then(|items| items.first())
        .and_then(|item| {
            read_nested_exact_f64s(
                item,
                tags::PLANE_ORIENTATION_SEQUENCE,
                tags::IMAGE_ORIENTATION_PATIENT,
            )
        });
    let shared_pixel_spacing = obj
        .element(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE)
        .ok()
        .and_then(|element| element.items())
        .and_then(|items| items.first())
        .and_then(|item| {
            read_nested_exact_f64s(item, tags::PIXEL_MEASURES_SEQUENCE, tags::PIXEL_SPACING)
        })
        .filter(|values: &[f64; 2]| values.iter().all(|value| value.is_finite() && *value > 0.0));
    let per_frame_items = obj
        .element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
        .ok()
        .and_then(|element| element.items());

    if per_frame_items.is_none() && shared_orientation.is_none() {
        return (Vec::new(), Vec::new(), Vec::new(), shared_pixel_spacing);
    }

    let frame_count = frame_count as usize;
    let mut positions = Vec::with_capacity(frame_count);
    let mut orientations = Vec::with_capacity(frame_count);
    let mut pixel_spacings = Vec::with_capacity(frame_count);
    for frame_index in 0..frame_count {
        let frame_item = per_frame_items.and_then(|items| items.get(frame_index));
        positions.push(
            frame_item
                .and_then(|item| {
                    read_nested_exact_f64s(
                        item,
                        tags::PLANE_POSITION_SEQUENCE,
                        tags::IMAGE_POSITION_PATIENT,
                    )
                })
                .or(top_level_position),
        );
        orientations.push(
            frame_item
                .and_then(|item| {
                    read_nested_exact_f64s(
                        item,
                        tags::PLANE_ORIENTATION_SEQUENCE,
                        tags::IMAGE_ORIENTATION_PATIENT,
                    )
                })
                .or(shared_orientation)
                .or(top_level_orientation),
        );
        pixel_spacings.push(
            frame_item
                .and_then(|item| {
                    read_nested_exact_f64s(item, tags::PIXEL_MEASURES_SEQUENCE, tags::PIXEL_SPACING)
                })
                .filter(|values: &[f64; 2]| {
                    values.iter().all(|value| value.is_finite() && *value > 0.0)
                })
                .or(shared_pixel_spacing)
                .or(top_level_pixel_spacing),
        );
    }
    (
        positions,
        orientations,
        pixel_spacings,
        shared_pixel_spacing,
    )
}

fn read_nested_exact_f64s<const N: usize>(
    item: &dicom_object::InMemDicomObject,
    sequence_tag: dicom_core::Tag,
    value_tag: dicom_core::Tag,
) -> Option<[f64; N]> {
    let value = item
        .element(sequence_tag)
        .ok()?
        .items()?
        .first()?
        .element(value_tag)
        .ok()?
        .to_str()
        .ok()?;
    let values = split_dicom_values(value.as_ref())
        .into_iter()
        .map(|value| value.parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let values: [f64; N] = values.try_into().ok()?;
    values
        .iter()
        .all(|value| value.is_finite())
        .then_some(values)
}

fn build_label(patient_id: &str, modality: &str, study_date: &str, fallback: &str) -> String {
    let mut fields = Vec::new();
    if !patient_id.is_empty() {
        fields.push(patient_id);
    }
    if !modality.is_empty() {
        fields.push(modality);
    }
    if !study_date.is_empty() {
        fields.push(study_date);
    }

    if fields.is_empty() {
        fallback.to_string()
    } else {
        fields.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_entry, read_discovery_metadata, split_dicom_values, valid_specific_character_set,
        EntryInspection,
    };
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
    use tempfile::tempdir;

    use crate::types::NativePixelDataKind;

    #[test]
    fn validates_each_component_of_multi_valued_character_sets() {
        assert!(valid_specific_character_set(
            "\\ISO 2022 IR 87\\ISO 2022 IR 13"
        ));
        assert!(!valid_specific_character_set(
            "ISO_IR 100\\DTS_UNKNOWN_CHARACTER_SET"
        ));
    }

    #[test]
    fn extracts_classic_enhanced_concatenation_and_wsi_identity() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("series-identity.dcm");
        write_series_identity_fixture(&path);

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let metadata = &file.series_metadata;

        assert_eq!(
            metadata.native_pixel.pixel_data_kind,
            Some(NativePixelDataKind::Integer)
        );
        assert_eq!(metadata.native_pixel.planar_configuration, Some(0));
        assert_eq!(metadata.native_pixel.bits_stored, Some(12));
        assert_eq!(metadata.native_pixel.high_bit, Some(11));
        assert_eq!(metadata.native_pixel.pixel_spacing, Some([0.6, 0.3]));
        assert_eq!(metadata.native_pixel.pixel_aspect_ratio, Some([3, 1]));
        assert_eq!(
            metadata.native_pixel.normalized_pixel_aspect,
            Some([2.0, 1.0])
        );
        assert_eq!(file.study_instance_uid, "2.25.100");
        assert_eq!(file.series_instance_uid, "2.25.200");
        assert_eq!(file.sop_instance_uid, "2.25.300");
        assert_eq!(file.instance_number, "7");
        assert_eq!(metadata.frame_of_reference_uid, "2.25.400");
        assert_eq!(metadata.image_position_patient, Some([10.0, 20.0, 30.0]));
        assert_eq!(
            metadata.image_orientation_patient,
            Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
        );
        assert_eq!(
            metadata.frame_image_positions_patient,
            vec![Some([0.0, 0.0, 1.0]), Some([0.0, 0.0, 2.0])]
        );
        assert_eq!(
            metadata.frame_image_orientations_patient,
            vec![
                Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                Some([0.0, 1.0, 0.0, 1.0, 0.0, 0.0]),
            ]
        );
        assert_eq!(
            metadata.frame_pixel_spacings,
            vec![Some([0.6, 0.3]), Some([0.6, 0.3])]
        );
        assert_eq!(metadata.concatenation_uid.as_deref(), Some("2.25.500"));
        assert_eq!(metadata.in_concatenation_number, Some(1));
        assert_eq!(metadata.in_concatenation_total_number, Some(2));
        assert_eq!(metadata.concatenation_frame_offset_number, Some(3));
        assert_eq!(
            metadata.sop_instance_uid_of_concatenation_source.as_deref(),
            Some("2.25.600")
        );
        assert_eq!(
            metadata.image_type,
            ["ORIGINAL", "PRIMARY", "VOLUME", "NONE"]
        );
        assert_eq!(metadata.pyramid_uid.as_deref(), Some("2.25.700"));
        assert_eq!(
            metadata.dimension_organization_type.as_deref(),
            Some("TILED_FULL")
        );
        assert_eq!(
            metadata.dimension_organization_uids,
            ["2.25.801", "2.25.802"]
        );
        assert_eq!(
            metadata.image_orientation_slide,
            Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
        );
        assert_eq!(metadata.total_pixel_matrix_rows, Some(8));
        assert_eq!(metadata.total_pixel_matrix_columns, Some(12));
        assert_eq!(metadata.total_pixel_matrix_focal_planes, Some(1));
        assert_eq!(metadata.number_of_optical_paths, Some(2));
        assert_eq!(metadata.container_identifier.as_deref(), Some("SLIDE-1"));
        assert_eq!(metadata.specimen_uids, ["2.25.901", "2.25.902"]);
        assert_eq!(metadata.optical_path_identifiers, ["RGB", "IHC"]);
    }

    #[test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    fn recognizes_prepared_deflated_image_frame_segmentation_metadata() {
        let root = std::env::var_os("DCMVIEW_PREPARED_CORPUS")
            .map(std::path::PathBuf::from)
            .expect("set DCMVIEW_PREPARED_CORPUS to the generated suite directory");
        let path = root
            .join("extended-deflate")
            .join("derived/seg/binary_multiframe_deflated_image_frame/instance.dcm");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect prepared SEG")
        else {
            panic!("prepared SEG should be selected");
        };

        assert_eq!(file.sop_class_uid, "1.2.840.10008.5.1.4.1.1.66.4");
        assert_eq!(file.transfer_syntax_uid, "1.2.840.10008.1.2.8.1");
        assert_eq!(file.modality, "SEG");
        assert_eq!(file.frame_count, 2);
        assert_eq!((file.rows, file.columns), (2, 2));
        assert_eq!(file.bits_allocated, 1);
        assert!(file.has_pixels);
    }

    #[test]
    fn rejects_partial_or_non_finite_geometry_values() {
        assert_eq!(split_dicom_values(" 1 \\ 2 \\ \\ 3 "), ["1", "2", "", "3"]);

        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("invalid-geometry.dcm");
        let object = base_object()
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        assert_eq!(file.series_metadata.image_position_patient, None);
        assert_eq!(file.series_metadata.image_orientation_patient, None);
        assert!(file
            .series_metadata
            .frame_image_positions_patient
            .is_empty());
        assert!(file
            .series_metadata
            .frame_image_orientations_patient
            .is_empty());
    }

    #[test]
    fn falls_back_to_valid_pixel_aspect_ratio() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("pixel-aspect-ratio.dcm");
        let mut object = base_object();
        object.put(DataElement::new(tags::PIXEL_SPACING, VR::DS, "0\\0.3"));
        object.put(DataElement::new(tags::PIXEL_ASPECT_RATIO, VR::IS, "2\\1"));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8; 4].into()),
        ));
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let metadata = &file.series_metadata.native_pixel;
        assert_eq!(metadata.pixel_spacing, None);
        assert_eq!(metadata.pixel_aspect_ratio, Some([2, 1]));
        assert_eq!(metadata.normalized_pixel_aspect, Some([2.0, 1.0]));
    }

    #[test]
    fn recognizes_float_and_double_float_pixel_elements() {
        let directory = tempdir().expect("temp directory");
        let cases = [
            (
                "float.dcm",
                DataElement::new(
                    Tag(0x7fe0, 0x0008),
                    VR::OF,
                    PrimitiveValue::F32(vec![0.0_f32, 1.0].into()),
                ),
                NativePixelDataKind::Float32,
            ),
            (
                "double.dcm",
                DataElement::new(
                    Tag(0x7fe0, 0x0009),
                    VR::OD,
                    PrimitiveValue::F64(vec![0.0_f64, 1.0].into()),
                ),
                NativePixelDataKind::Float64,
            ),
        ];

        for (name, pixel_element, expected_kind) in cases {
            let path = directory.path().join(name);
            let mut object = base_object();
            object.put(pixel_element);
            let object = object
                .with_meta(
                    FileMetaTableBuilder::new()
                        .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                        .media_storage_sop_class_uid(uids::PARAMETRIC_MAP_STORAGE)
                        .media_storage_sop_instance_uid("2.25.300"),
                )
                .expect("file meta");
            object.write_to_file(&path).expect("write fixture");

            let metadata = read_discovery_metadata(&path).expect("read discovery metadata");
            let pixel_tag = match expected_kind {
                NativePixelDataKind::Integer => tags::PIXEL_DATA,
                NativePixelDataKind::Float32 => tags::FLOAT_PIXEL_DATA,
                NativePixelDataKind::Float64 => tags::DOUBLE_FLOAT_PIXEL_DATA,
            };
            assert!(metadata.element(pixel_tag).is_err());

            let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture")
            else {
                panic!("fixture should be selected");
            };
            assert!(file.has_pixels);
            assert_eq!(
                file.series_metadata.native_pixel.pixel_data_kind,
                Some(expected_kind)
            );
        }
    }

    #[test]
    fn nested_icon_pixels_and_tag_bytes_do_not_count_as_pixel_data() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("icon-only.dcm");
        let icon = dicom_object::InMemDicomObject::from_element_iter([
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(
                tags::PIXEL_DATA,
                VR::OB,
                PrimitiveValue::from(vec![0_u8, 0]),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::ICON_IMAGE_SEQUENCE,
            VR::SQ,
            dicom_core::value::DataSetSequence::from(vec![icon]),
        ));
        // Little-endian (7FE0,0010) bytes inside an unrelated value.
        object.put(DataElement::new(
            Tag(0x0009, 0x1010),
            VR::OB,
            PrimitiveValue::from(vec![0xe0_u8, 0x7f, 0x10, 0x00]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.301"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        assert!(!file.has_pixels);
        assert_eq!(file.series_metadata.native_pixel.pixel_data_kind, None);
    }

    #[test]
    fn extracts_prepared_modality_lut_sequence() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("modality-lut.dcm");
        let lut_item = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::LUT_DESCRIPTOR,
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 16].into()),
            ),
            DataElement::new(
                tags::LUT_DATA,
                VR::OW,
                PrimitiveValue::U16(vec![0, 1024, 2048, 4095].into()),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::MODALITY_LUT_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![lut_item]),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8, 1, 2, 3].into()),
        ));
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let lut = file
            .series_metadata
            .native_pixel
            .modality_lut
            .as_ref()
            .expect("modality LUT");
        assert_eq!(lut.first_mapped_value, 0);
        assert_eq!(lut.bits_per_entry, 16);
        assert_eq!(lut.entries, [0, 1024, 2048, 4095]);
    }

    #[test]
    fn extracts_prepared_voi_lut_sequence() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("voi-lut.dcm");
        let lut_item = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::LUT_DESCRIPTOR,
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 16].into()),
            ),
            DataElement::new(
                tags::LUT_DATA,
                VR::OW,
                PrimitiveValue::U16(vec![0, 21_845, 43_690, 65_535].into()),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::VOILUT_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![lut_item]),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8, 1, 2, 3].into()),
        ));
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let lut = file
            .series_metadata
            .native_pixel
            .voi_lut
            .as_ref()
            .expect("VOI LUT");
        assert_eq!(lut.first_mapped_value, 0);
        assert_eq!(lut.bits_per_entry, 16);
        assert_eq!(lut.entries, [0, 21_845, 43_690, 65_535]);
    }

    #[test]
    fn extracts_eight_bit_lut_entries_from_byte_storage() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("voi-lut-u8.dcm");
        let lut_item = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::LUT_DESCRIPTOR,
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 8].into()),
            ),
            DataElement::new(
                tags::LUT_DATA,
                VR::OW,
                PrimitiveValue::U8(vec![0, 85, 170, 255].into()),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::VOILUT_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![lut_item]),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8, 1, 2, 3].into()),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.301"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let lut = file
            .series_metadata
            .native_pixel
            .voi_lut
            .as_ref()
            .expect("8-bit VOI LUT");
        assert_eq!(lut.bits_per_entry, 8);
        assert_eq!(lut.entries, [0, 85, 170, 255]);
    }

    #[test]
    fn extracts_overlay_plane_and_rectangular_shutter_metadata() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("presentation-metadata.dcm");
        let mut object = base_object();
        for element in [
            DataElement::new(Tag(0x6000, 0x0010), VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(Tag(0x6000, 0x0011), VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(Tag(0x6000, 0x0040), VR::CS, "G"),
            DataElement::new(
                Tag(0x6000, 0x0050),
                VR::SS,
                PrimitiveValue::I16(vec![1, 1].into()),
            ),
            DataElement::new(Tag(0x6000, 0x0100), VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(Tag(0x6000, 0x0102), VR::US, PrimitiveValue::from(0_u16)),
            DataElement::new(
                Tag(0x6000, 0x3000),
                VR::OW,
                PrimitiveValue::U16(vec![0x0009].into()),
            ),
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "RECTANGULAR"),
            DataElement::new(tags::SHUTTER_LEFT_VERTICAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_RIGHT_VERTICAL_EDGE, VR::IS, "2"),
            DataElement::new(tags::SHUTTER_UPPER_HORIZONTAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_LOWER_HORIZONTAL_EDGE, VR::IS, "2"),
            DataElement::new(
                tags::SHUTTER_PRESENTATION_VALUE,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
        ] {
            object.put(element);
        }
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; 8].into()),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let presentation = &file.series_metadata.presentation;
        assert_eq!(presentation.overlay_planes.len(), 1);
        let overlay = &presentation.overlay_planes[0];
        assert_eq!(overlay.group, 0x6000);
        assert_eq!((overlay.rows, overlay.columns), (2, 2));
        assert_eq!(overlay.origin, [1, 1]);
        assert_eq!(overlay.overlay_type, "G");
        assert_eq!(overlay.number_of_frames, 1);
        assert_eq!(overlay.image_frame_origin, 1);
        assert_eq!(overlay.data, [0x0009]);
        assert_eq!(
            presentation.rectangular_shutter,
            Some(crate::types::RectangularDisplayShutter {
                left_vertical_edge: 1,
                right_vertical_edge: 2,
                upper_horizontal_edge: 1,
                lower_horizontal_edge: 2,
                presentation_value: 0,
            })
        );
    }

    #[test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    fn prepared_overlay_and_shutter_metadata_match_locked_cases() {
        let root = std::env::var_os("DCMVIEW_PREPARED_CORPUS")
            .map(std::path::PathBuf::from)
            .expect("set DCMVIEW_PREPARED_CORPUS to the generated suite directory");
        let overlay_path = root
            .join("core")
            .join("classic/cr/overlay_modality_voi_explicit_le/instance.dcm");
        let shutter_path = root
            .join("core")
            .join("classic/dx/display_shutter_mono2_u16_explicit_le/instance.dcm");

        let EntryInspection::Selected(overlay_file) =
            build_entry(&overlay_path).expect("inspect prepared CR")
        else {
            panic!("prepared CR should be selected");
        };
        let overlay = &overlay_file.series_metadata.presentation.overlay_planes[0];
        assert_eq!((overlay.rows, overlay.columns), (2, 2));
        assert_eq!(overlay.origin, [1, 1]);
        assert_eq!(overlay.data, [0x0009]);

        let EntryInspection::Selected(shutter_file) =
            build_entry(&shutter_path).expect("inspect prepared DX")
        else {
            panic!("prepared DX should be selected");
        };
        assert_eq!(
            shutter_file
                .series_metadata
                .presentation
                .rectangular_shutter,
            Some(crate::types::RectangularDisplayShutter {
                left_vertical_edge: 1,
                right_vertical_edge: 2,
                upper_horizontal_edge: 1,
                lower_horizontal_edge: 2,
                presentation_value: 0,
            })
        );
    }

    fn write_series_identity_fixture(path: &std::path::Path) {
        let mut shared_orientation = nested_sequence_item(
            tags::PLANE_ORIENTATION_SEQUENCE,
            tags::IMAGE_ORIENTATION_PATIENT,
            "1\\0\\0\\0\\1\\0",
        );
        shared_orientation.put(DataElement::new(
            tags::PIXEL_MEASURES_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter([
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "0.6\\0.3"),
            ])]),
        ));
        let frame_one = nested_sequence_item(
            tags::PLANE_POSITION_SEQUENCE,
            tags::IMAGE_POSITION_PATIENT,
            "0\\0\\1",
        );
        let mut frame_two = nested_sequence_item(
            tags::PLANE_POSITION_SEQUENCE,
            tags::IMAGE_POSITION_PATIENT,
            "0\\0\\2",
        );
        frame_two.put(DataElement::new(
            tags::PLANE_ORIENTATION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter([
                DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "0\\1\\0\\1\\0\\0"),
            ])]),
        ));

        let dimension_items = ["2.25.801", "2.25.802"]
            .map(|uid| {
                InMemDicomObject::from_element_iter([DataElement::new(
                    tags::DIMENSION_ORGANIZATION_UID,
                    VR::UI,
                    uid,
                )])
            })
            .to_vec();
        let specimen_items = ["2.25.901", "2.25.902"]
            .map(|uid| {
                InMemDicomObject::from_element_iter([DataElement::new(
                    tags::SPECIMEN_UID,
                    VR::UI,
                    uid,
                )])
            })
            .to_vec();
        let optical_path_items = ["RGB", "IHC"]
            .map(|identifier| {
                InMemDicomObject::from_element_iter([DataElement::new(
                    tags::OPTICAL_PATH_IDENTIFIER,
                    VR::SH,
                    identifier,
                )])
            })
            .to_vec();

        let mut object = base_object();
        for element in [
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(12_u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(11_u16)),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(3_u16)),
            DataElement::new(
                tags::PLANAR_CONFIGURATION,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
            DataElement::new(tags::PIXEL_ASPECT_RATIO, VR::IS, "3\\1"),
            DataElement::new(tags::FRAME_OF_REFERENCE_UID, VR::UI, "2.25.400"),
            DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, "10\\20\\30"),
            DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
            DataElement::new(tags::CONCATENATION_UID, VR::UI, "2.25.500"),
            DataElement::new(
                tags::IN_CONCATENATION_NUMBER,
                VR::US,
                PrimitiveValue::from(1_u16),
            ),
            DataElement::new(
                tags::IN_CONCATENATION_TOTAL_NUMBER,
                VR::US,
                PrimitiveValue::from(2_u16),
            ),
            DataElement::new(
                tags::CONCATENATION_FRAME_OFFSET_NUMBER,
                VR::UL,
                PrimitiveValue::from(3_u32),
            ),
            DataElement::new(
                tags::SOP_INSTANCE_UID_OF_CONCATENATION_SOURCE,
                VR::UI,
                "2.25.600",
            ),
            DataElement::new(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\VOLUME\\NONE"),
            DataElement::new(tags::PYRAMID_UID, VR::UI, "2.25.700"),
            DataElement::new(tags::DIMENSION_ORGANIZATION_TYPE, VR::CS, "TILED_FULL"),
            DataElement::new(tags::IMAGE_ORIENTATION_SLIDE, VR::DS, "1\\0\\0\\0\\1\\0"),
            DataElement::new(
                tags::TOTAL_PIXEL_MATRIX_ROWS,
                VR::UL,
                PrimitiveValue::from(8_u32),
            ),
            DataElement::new(
                tags::TOTAL_PIXEL_MATRIX_COLUMNS,
                VR::UL,
                PrimitiveValue::from(12_u32),
            ),
            DataElement::new(
                tags::TOTAL_PIXEL_MATRIX_FOCAL_PLANES,
                VR::UL,
                PrimitiveValue::from(1_u32),
            ),
            DataElement::new(
                tags::NUMBER_OF_OPTICAL_PATHS,
                VR::UL,
                PrimitiveValue::from(2_u32),
            ),
            DataElement::new(tags::CONTAINER_IDENTIFIER, VR::LO, "SLIDE-1"),
        ] {
            object.put(element);
        }
        object.put(DataElement::new(
            tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![shared_orientation]),
        ));
        object.put(DataElement::new(
            tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![frame_one, frame_two]),
        ));
        object.put(DataElement::new(
            tags::DIMENSION_ORGANIZATION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(dimension_items),
        ));
        object.put(DataElement::new(
            tags::SPECIMEN_DESCRIPTION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(specimen_items),
        ));
        object.put(DataElement::new(
            tags::OPTICAL_PATH_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(optical_path_items),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; 8].into()),
        ));

        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::ENHANCED_CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(path).expect("write fixture");
    }

    fn base_object() -> InMemDicomObject {
        InMemDicomObject::from_element_iter([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::ENHANCED_CT_IMAGE_STORAGE),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.300"),
            DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, "2.25.100"),
            DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, "2.25.200"),
            DataElement::new(tags::INSTANCE_NUMBER, VR::IS, "7"),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
            DataElement::new(
                tags::PIXEL_REPRESENTATION,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, "1\\2"),
            DataElement::new(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                "NaN\\0\\0\\0\\1\\0",
            ),
        ])
    }

    fn nested_sequence_item(
        sequence_tag: dicom_core::Tag,
        value_tag: dicom_core::Tag,
        value: &str,
    ) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([DataElement::new(
            sequence_tag,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter([
                DataElement::new(value_tag, VR::DS, value),
            ])]),
        )])
    }
}
