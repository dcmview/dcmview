use super::entry::{build_entry_selected, EntryInspection};
use super::filter::{matches_filters, ScanFilter};
use super::format::FormatSelection;
use crate::types::FileEntry;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task;
use walkdir::WalkDir;

const DISCOVERY_SEND_RETRY_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    pub recursive: bool,
    pub filters: Vec<ScanFilter>,
    /// The formats loaded from walked directories (`--formats`).
    pub formats: FormatSelection,
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
    /// A selected raster image file.
    ValidImage,
    InputPathUnavailable,
    DirectoryEntryUnreadable,
    /// Neither a DICOM Part 10 file nor a raster format the viewer reads.
    /// Before raster support this was `MissingPart10Preamble`, code
    /// `missing_part10_preamble`; the owner signed the rename off
    /// (`docs/design/image-formats.md` decision 12.3).
    UnrecognizedFormat,
    /// A recognized format that `--formats` leaves out; the file's header
    /// was not parsed.
    FormatNotSelected,
    /// A raster signature whose header does not parse.
    RasterHeaderInvalid,
    DicomParseFailed,
    UnsupportedMediaDirectory,
    InspectionFailed,
    FilterMismatch,
}

impl DiscoveryReason {
    /// What a skip for this reason means, for the discovery summary.
    pub const fn summary(self) -> &'static str {
        match self {
            Self::ValidDicom => "valid DICOM",
            Self::ValidImage => "valid image",
            Self::InputPathUnavailable => "missing input path",
            Self::DirectoryEntryUnreadable => "unreadable directory entry",
            Self::UnrecognizedFormat => "not a DICOM or image file",
            Self::FormatNotSelected => "excluded by --formats",
            Self::RasterHeaderInvalid => "unreadable image header",
            Self::DicomParseFailed => "unparsable DICOM",
            Self::UnsupportedMediaDirectory => "DICOMDIR",
            Self::InspectionFailed => "unreadable file",
            Self::FilterMismatch => "filtered",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::ValidDicom => "valid_dicom",
            Self::ValidImage => "valid_image",
            Self::InputPathUnavailable => "input_path_unavailable",
            Self::DirectoryEntryUnreadable => "directory_entry_unreadable",
            Self::UnrecognizedFormat => "unrecognized_format",
            Self::FormatNotSelected => "format_not_selected",
            Self::RasterHeaderInvalid => "raster_header_invalid",
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

#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    /// Selected files of every format.
    pub files_found: usize,
    /// The raster image files among `files_found`.
    pub images_found: usize,
    pub skipped: usize,
    /// Skipped candidates by reason, so the summary can say why.
    pub skipped_by_reason: BTreeMap<DiscoveryReason, usize>,
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

/// One file to inspect, with the format selection that applies to it:
/// `--formats` narrows what a directory walk loads, while a file named as an
/// input path is loaded in whatever supported format it has.
struct Candidate {
    path: PathBuf,
    formats: FormatSelection,
}

fn collect_candidates(
    paths: &[PathBuf],
    options: &DiscoverOptions,
    events: &mpsc::Sender<DiscoveryEvent>,
    cancellation: &DiscoveryCancellation,
) -> std::result::Result<(Vec<Candidate>, Vec<DiscoveryRecord>), DiscoveryCancelled> {
    let check_active = || ensure_discovery_active(events, cancellation);
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for path in paths {
        check_active()?;

        if path.is_file() {
            candidates.push(Candidate {
                path: path.clone(),
                formats: FormatSelection::all(),
            });
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
                        candidates.push(Candidate {
                            path: dir_entry.path().to_path_buf(),
                            formats: options.formats,
                        });
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
    let skipped = Mutex::new(BTreeMap::<DiscoveryReason, usize>::new());
    let note_skip = |path: &Path, reason: DiscoveryReason| {
        tracing::debug!(path = %path.display(), reason = reason.code(), "skipped");
        if let Ok(mut counts) = skipped.lock() {
            *counts.entry(reason).or_default() += 1;
        }
    };
    for record in &initial_skipped {
        note_skip(&record.path, record.reason);
    }
    let images_found = AtomicUsize::new(0);
    let filtered = AtomicUsize::new(0);

    let processing_result: std::result::Result<(), DiscoveryCancelled> = candidates
        .par_iter()
        .try_for_each_with(events.clone(), |events, candidate| {
            ensure_discovery_active(events, cancellation)?;
            let Candidate {
                path: candidate,
                formats,
            } = candidate;

            let inspected = build_entry_selected(
                candidate,
                *formats,
                &|entry| matches_filters(entry, &options.filters),
                &|| ensure_discovery_active(events, cancellation).map_err(Into::into),
            );
            ensure_discovery_active(events, cancellation)?;
            match inspected {
                Ok(EntryInspection::Selected(entry))
                    if matches_filters(&entry, &options.filters) =>
                {
                    let is_image = entry.format.is_raster();
                    let record = DiscoveryRecord::new(
                        candidate,
                        DiscoveryDisposition::Selected,
                        if is_image {
                            DiscoveryReason::ValidImage
                        } else {
                            DiscoveryReason::ValidDicom
                        },
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
                    if is_image {
                        images_found.fetch_add(1, Ordering::Relaxed);
                    }
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
                    tracing::debug!(path = %candidate.display(), "filtered");
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
                    note_skip(candidate, reason);
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
                    note_skip(candidate, DiscoveryReason::InspectionFailed);
                    eprintln!("dcmview: warning — failed to inspect DICOM: {error}");
                }
            }

            Ok(())
        });
    processing_result?;
    ensure_discovery_active(&events, cancellation)?;

    let skipped_by_reason = skipped.into_inner().unwrap_or_default();
    Ok(DiscoveryReport {
        files_found: files_found.load(Ordering::Relaxed),
        images_found: images_found.load(Ordering::Relaxed),
        skipped: skipped_by_reason.values().sum(),
        skipped_by_reason,
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
