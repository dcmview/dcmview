use dcmview::annotations::{AnnotationSource, AnnotationStore};
use dcmview::loader;
use dcmview::server::FileRegistry;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const DISCOVERY_EVENT_CAPACITY: usize = 64;

pub(super) struct DiscoveryInputs {
    pub(super) input_paths: Vec<PathBuf>,
    pub(super) recursive: bool,
    pub(super) filters: Vec<loader::ScanFilter>,
    pub(super) formats: loader::FormatSelection,
    pub(super) annotation_source: Option<AnnotationSource>,
    pub(super) registry: FileRegistry,
    pub(super) annotation_store: AnnotationStore,
    pub(super) shutdown: CancellationToken,
    /// Announce a completed scan on stdout for `--startup-json` readers.
    pub(super) startup_json: bool,
}

/// How discovery ended. Only `Failed` makes the process exit non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiscoveryOutcome {
    Completed,
    /// The server stopped first and requested cancellation.
    Cancelled,
    /// The scan failed or found no files, and the server was told to stop.
    Failed,
}

/// The owned discovery task: the loader scan, registry updates, and then
/// annotation loading.
pub(super) struct DiscoveryHandle {
    cancellation: loader::DiscoveryCancellation,
    task: JoinHandle<DiscoveryOutcome>,
}

impl DiscoveryHandle {
    pub(super) fn spawn(inputs: DiscoveryInputs) -> Self {
        let cancellation = loader::DiscoveryCancellation::new();
        let task = tokio::spawn(run_discovery(inputs, cancellation.clone()));
        Self { cancellation, task }
    }

    /// Request cancellation, then wait for the task, including the loader's
    /// blocking workers and any annotation loading.
    pub(super) async fn cancel_and_wait(mut self) -> DiscoveryOutcome {
        self.cancellation.cancel();
        match (&mut self.task).await {
            Ok(outcome) => outcome,
            Err(error) => {
                eprintln!("dcmview: discovery task failed: {error}");
                DiscoveryOutcome::Failed
            }
        }
    }
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn run_discovery(
    inputs: DiscoveryInputs,
    cancellation: loader::DiscoveryCancellation,
) -> DiscoveryOutcome {
    let outcome = {
        let mut finish = ScanFinish {
            registry: inputs.registry.clone(),
            shutdown: inputs.shutdown.clone(),
            failed: true,
        };
        let outcome = scan(&inputs, cancellation.clone()).await;
        finish.failed = outcome == DiscoveryOutcome::Failed;
        outcome
    };
    if outcome == DiscoveryOutcome::Completed && inputs.startup_json {
        // The server announces its URL before discovery ends, and a scan that
        // finds nothing then exits non-zero; this line tells a launcher (the
        // Python wrapper) which of the two happened.
        let file_count = inputs.registry.status().file_count;
        dcmview::status_line!(
            "{}",
            serde_json::json!({ "type": "scan_complete", "file_count": file_count })
        );
    }

    if outcome == DiscoveryOutcome::Completed {
        if let Some(source) = inputs.annotation_source {
            load_annotations(
                source,
                inputs.registry.files_snapshot(),
                inputs.annotation_store,
                cancellation,
            )
            .await;
        }
    }
    outcome
}

/// Marks the scan complete when dropped, and stops the server if the scan
/// failed, so both happen on every exit path, including a panic.
struct ScanFinish {
    registry: FileRegistry,
    shutdown: CancellationToken,
    failed: bool,
}

impl Drop for ScanFinish {
    fn drop(&mut self) {
        self.registry.mark_scan_complete();
        if self.failed {
            self.shutdown.cancel();
        }
    }
}

/// Run the loader while recording its events; the recorder ends when the
/// loader's workers drop their senders.
async fn scan(
    inputs: &DiscoveryInputs,
    cancellation: loader::DiscoveryCancellation,
) -> DiscoveryOutcome {
    let (events_tx, mut events_rx) = mpsc::channel(DISCOVERY_EVENT_CAPACITY);
    let options = loader::DiscoverOptions {
        recursive: inputs.recursive,
        filters: inputs.filters.clone(),
        formats: inputs.formats,
    };
    let discover =
        loader::discover_progressive(&inputs.input_paths, options, events_tx, cancellation);
    let record = async {
        while let Some(event) = events_rx.recv().await {
            record_event(event, &inputs.registry);
        }
    };
    let (result, ()) = tokio::join!(discover, record);
    finish_scan(
        result,
        &inputs.registry,
        &inputs.filters,
        &inputs.input_paths,
    )
}

fn record_event(event: loader::DiscoveryEvent, registry: &FileRegistry) {
    match event {
        loader::DiscoveryEvent::Selected { file, record } => {
            registry.record_discovery(record);
            registry.insert(*file);
        }
        loader::DiscoveryEvent::SkippedInput(record)
        | loader::DiscoveryEvent::FilteredInput(record) => registry.record_discovery(record),
    }
}

async fn load_annotations(
    source: AnnotationSource,
    files: Vec<std::sync::Arc<dcmview::types::FileEntry>>,
    store: AnnotationStore,
    cancellation: loader::DiscoveryCancellation,
) {
    let scan_cancellation = cancellation.clone();
    let result = tokio::task::spawn_blocking(move || {
        source.load_for_files_with_check(&files, || {
            if scan_cancellation.is_cancelled() {
                anyhow::bail!("annotation loading cancelled");
            }
            Ok(())
        })
    })
    .await;

    match result {
        Ok(Ok((annotations, report))) => {
            if let Err(error) = store.commit_csv_if_unedited(annotations) {
                eprintln!("dcmview: warning — failed to commit annotations: {error:#}");
                let _ = store.fail_loading(error.to_string());
                return;
            }
            if report.unmatched_rows > 0 {
                eprintln!(
                    "dcmview: warning — {} annotation row(s) did not match discovered DICOM files",
                    report.unmatched_rows
                );
            }
        }
        Ok(Err(error)) => {
            if !cancellation.is_cancelled() {
                eprintln!("dcmview: warning — failed to load annotations: {error:#}");
            }
            let _ = store.fail_loading(error.to_string());
        }
        Err(error) => {
            eprintln!("dcmview: warning — annotation loader failed: {error}");
            let _ = store.fail_loading(format!("annotation loader failed: {error}"));
        }
    }
}

fn finish_scan(
    result: anyhow::Result<loader::DiscoveryReport>,
    registry: &FileRegistry,
    filters: &[loader::ScanFilter],
    input_paths: &[PathBuf],
) -> DiscoveryOutcome {
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            if loader::discovery_cancellation_reason(&error)
                == Some(loader::DiscoveryCancellationReason::Requested)
            {
                return DiscoveryOutcome::Cancelled;
            }
            eprintln!("failed to discover DICOM files: {error:#}");
            return DiscoveryOutcome::Failed;
        }
    };

    let file_count = registry.status().file_count;
    if file_count == 0 {
        if report.filtered > 0 {
            eprintln!(
                "dcmview: no DICOM files matched active filters ({})",
                format_scan_filters(filters)
            );
        } else if report.skipped > 0 {
            eprintln!(
                "dcmview: no valid DICOM files found ({})",
                skip_breakdown(&report)
            );
        } else {
            eprintln!("dcmview: no valid DICOM files found");
        }
        return DiscoveryOutcome::Failed;
    }

    print_progressive_load_summary(file_count, &report, filters, input_paths);
    DiscoveryOutcome::Completed
}

/// "3 skipped: 2 not a DICOM or image file, 1 unparsable DICOM".
fn skip_breakdown(report: &loader::DiscoveryReport) -> String {
    let reasons = report
        .skipped_by_reason
        .iter()
        .map(|(reason, count)| format!("{count} {}", reason.summary()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} skipped: {reasons}", report.skipped)
}

fn print_progressive_load_summary(
    file_count: usize,
    report: &loader::DiscoveryReport,
    filters: &[loader::ScanFilter],
    input_paths: &[PathBuf],
) {
    let (skipped, filtered) = (report.skipped, report.filtered);
    let recursive_note = if report.searched_recursive {
        "searched recursively"
    } else {
        "searched top-level only"
    };
    let path_label = if input_paths.len() == 1 {
        input_paths[0].display().to_string()
    } else {
        format!("{} path(s)", input_paths.len())
    };

    let mut notes = Vec::new();
    if skipped > 0 {
        notes.push(skip_breakdown(report));
    }
    if filtered > 0 {
        notes.push(format!("{filtered} filtered"));
    }
    if !filters.is_empty() {
        notes.push(format!("filters: {}", format_scan_filters(filters)));
    }
    notes.push(recursive_note.to_string());
    let note = notes.join(", ");

    if file_count == 1 && skipped == 0 && filtered == 0 && filters.is_empty() {
        dcmview::status_line!("dcmview: loaded 1 DICOM file");
    } else {
        dcmview::status_line!(
            "dcmview: loaded {file_count} DICOM file(s) from {path_label} ({note})"
        );
    }
}

fn format_scan_filters(filters: &[loader::ScanFilter]) -> String {
    filters
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    /// Copy a committed single-frame fixture into `dir` for the real loader.
    fn fixture_copy(dir: &Path) -> PathBuf {
        let path = dir.join("scan.dcm");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/golden-jpeg-baseline-single-frame.dcm"),
            &path,
        )
        .expect("copy fixture");
        path
    }

    fn discovery_inputs(
        input_path: PathBuf,
        filters: Vec<loader::ScanFilter>,
        annotation_source: Option<AnnotationSource>,
    ) -> (
        DiscoveryInputs,
        FileRegistry,
        AnnotationStore,
        CancellationToken,
    ) {
        let registry = FileRegistry::new();
        let annotation_store = if annotation_source.is_some() {
            AnnotationStore::loading()
        } else {
            AnnotationStore::empty()
        };
        let shutdown = CancellationToken::new();
        (
            DiscoveryInputs {
                input_paths: vec![input_path],
                recursive: true,
                filters,
                formats: loader::FormatSelection::all(),
                annotation_source,
                registry: registry.clone(),
                annotation_store: annotation_store.clone(),
                shutdown: shutdown.clone(),
                startup_json: false,
            },
            registry,
            annotation_store,
            shutdown,
        )
    }

    async fn wait_for_task(handle: &DiscoveryHandle) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !handle.task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("discovery task should finish");
    }

    #[tokio::test]
    async fn cancellation_awaits_the_scan_and_completes_the_registry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (inputs, registry, _, shutdown) =
            discovery_inputs(fixture_copy(temp.path()), Vec::new(), None);
        let handle = DiscoveryHandle::spawn(inputs);

        // The current-thread test runtime has not polled the task yet, so the
        // loader observes the cancellation before inspecting any file.
        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Cancelled);
        assert!(registry.status().scan_complete);
        assert_eq!(registry.status().file_count, 0);
        assert!(!shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn completed_scan_returns_normal_outcome_and_marks_registry_complete() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (inputs, registry, _, shutdown) =
            discovery_inputs(fixture_copy(temp.path()), Vec::new(), None);
        let handle = DiscoveryHandle::spawn(inputs);
        wait_for_task(&handle).await;

        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Completed);
        assert!(registry.status().scan_complete);
        assert_eq!(registry.status().file_count, 1);
        assert_eq!(registry.status().scanned, 1);
        assert!(
            registry.discovery_response_snapshot().is_empty(),
            "accepted files are counted, not listed as discovery records"
        );
        assert!(!shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn annotation_failure_keeps_viewer_running_and_marks_store_failed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = fixture_copy(temp.path());
        let csv_path = temp.path().join("annotations.csv");
        fs::write(
            &csv_path,
            format!(
                "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{},1,\"[[1,2,3,4]]\",\"[[2]]\"\n",
                file_path.display()
            ),
        )
        .expect("annotation CSV");
        let source = AnnotationSource::from_path(&csv_path).expect("annotation source");
        let (inputs, registry, store, shutdown) =
            discovery_inputs(file_path, Vec::new(), Some(source));
        let handle = DiscoveryHandle::spawn(inputs);
        wait_for_task(&handle).await;

        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Completed);
        assert!(store
            .wait_until_ready()
            .await
            .expect_err("invalid matched annotations should fail")
            .to_string()
            .contains("contains frame 2"));
        assert!(!shutdown.is_cancelled());
        assert!(registry.status().scan_complete);
    }

    #[tokio::test]
    async fn zero_files_fails_and_stops_the_server() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("not-dicom.bin"), b"not a dicom file").expect("invalid file");
        let (inputs, registry, _, shutdown) =
            discovery_inputs(temp.path().to_path_buf(), Vec::new(), None);
        let handle = DiscoveryHandle::spawn(inputs);

        tokio::time::timeout(Duration::from_secs(5), shutdown.cancelled())
            .await
            .expect("empty discovery should stop the server");
        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Failed);
        let status = registry.status();
        assert!(status.scan_complete);
        assert_eq!(status.file_count, 0);
        assert_eq!(status.skipped, 1);
    }

    #[tokio::test]
    async fn all_files_filtered_fails_and_counts_the_filtered_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        fixture_copy(temp.path());
        let filters = vec!["modality=NOT-A-MODALITY".parse().expect("filter parses")];
        let (inputs, registry, _, shutdown) =
            discovery_inputs(temp.path().to_path_buf(), filters, None);
        let handle = DiscoveryHandle::spawn(inputs);

        tokio::time::timeout(Duration::from_secs(5), shutdown.cancelled())
            .await
            .expect("fully filtered discovery should stop the server");
        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Failed);
        let status = registry.status();
        assert!(status.scan_complete);
        assert_eq!(status.file_count, 0);
        assert_eq!(status.filtered, 1);
    }

    #[test]
    fn filter_summary_preserves_cli_order() {
        let filters = vec![
            "modality=CT".parse().expect("modality filter"),
            "patient_id=phantom".parse().expect("patient filter"),
        ];

        assert_eq!(
            format_scan_filters(&filters),
            "modality=CT, patient_id=phantom"
        );
    }
}
