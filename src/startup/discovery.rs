use dcmview::annotations::{AnnotationSource, AnnotationStore};
use dcmview::loader;
use dcmview::server::FileRegistry;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;

const DISCOVERY_EVENT_CAPACITY: usize = 64;

pub(super) struct DiscoveryInputs {
    pub(super) input_paths: Vec<PathBuf>,
    pub(super) recursive: bool,
    pub(super) filters: Vec<loader::ScanFilter>,
    pub(super) annotation_source: Option<AnnotationSource>,
    pub(super) registry: FileRegistry,
    pub(super) annotation_store: AnnotationStore,
    pub(super) shutdown: Arc<Notify>,
}

pub(super) struct ScanRequest {
    input_paths: Vec<PathBuf>,
    options: loader::DiscoverOptions,
}

pub(super) type DiscoveryFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<loader::DiscoveryReport>> + Send + 'static>>;

pub(super) trait DiscoverySpawner: Send + Sync {
    fn spawn(
        &self,
        request: ScanRequest,
        events: mpsc::Sender<loader::DiscoveryEvent>,
        cancellation: loader::DiscoveryCancellation,
    ) -> DiscoveryFuture;
}

pub(super) struct LoaderDiscoverySpawner;

impl DiscoverySpawner for LoaderDiscoverySpawner {
    fn spawn(
        &self,
        request: ScanRequest,
        events: mpsc::Sender<loader::DiscoveryEvent>,
        cancellation: loader::DiscoveryCancellation,
    ) -> DiscoveryFuture {
        Box::pin(async move {
            loader::discover_progressive_with_cancellation(
                &request.input_paths,
                request.options,
                events,
                cancellation,
            )
            .await
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiscoveryFailure {
    Scan,
    NoFiles,
    Coordinator,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiscoveryOutcome {
    Completed,
    Cancelled(loader::DiscoveryCancellationReason),
    Failed(DiscoveryFailure),
}

impl DiscoveryOutcome {
    pub(super) fn is_failure(self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

enum ScanCompletion {
    Completed(loader::DiscoveryReport),
    Cancelled(loader::DiscoveryCancellationReason),
    Failed(String),
}

pub(super) struct DiscoveryHandle {
    cancellation: loader::DiscoveryCancellation,
    coordinator: Option<JoinHandle<DiscoveryOutcome>>,
    scan: Option<JoinHandle<()>>,
}

impl DiscoveryHandle {
    pub(super) fn spawn(inputs: DiscoveryInputs, spawner: &dyn DiscoverySpawner) -> Self {
        let (events_tx, events_rx) = mpsc::channel(DISCOVERY_EVENT_CAPACITY);
        let (completion_tx, completion_rx) = oneshot::channel();
        let cancellation = loader::DiscoveryCancellation::new();
        let scan_request = ScanRequest {
            input_paths: inputs.input_paths.clone(),
            options: loader::DiscoverOptions {
                recursive: inputs.recursive,
                filters: inputs.filters.clone(),
            },
        };
        let scan_future = spawner.spawn(scan_request, events_tx, cancellation.clone());
        let scan = tokio::spawn(async move {
            let completion = classify_scan_result(scan_future.await);
            let _ = completion_tx.send(completion);
        });
        let coordinator_cancellation = cancellation.clone();
        let coordinator = tokio::spawn(run_coordinator(
            inputs,
            events_rx,
            completion_rx,
            coordinator_cancellation,
        ));

        Self {
            cancellation,
            coordinator: Some(coordinator),
            scan: Some(scan),
        }
    }

    pub(super) async fn cancel_and_wait(mut self) -> DiscoveryOutcome {
        self.cancellation.cancel();
        let coordinator_result = self
            .coordinator
            .take()
            .expect("discovery coordinator handle missing")
            .await;
        let scan_result = self
            .scan
            .take()
            .expect("discovery scan handle missing")
            .await;

        let mut outcome = match coordinator_result {
            Ok(outcome) => outcome,
            Err(error) => {
                eprintln!("dcmview: discovery coordinator failed: {error}");
                DiscoveryOutcome::Failed(DiscoveryFailure::Coordinator)
            }
        };
        if let Err(error) = scan_result {
            eprintln!("dcmview: loader worker panicked: {error}");
            outcome = DiscoveryOutcome::Failed(DiscoveryFailure::Worker);
        }
        outcome
    }
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

fn classify_scan_result(result: anyhow::Result<loader::DiscoveryReport>) -> ScanCompletion {
    match result {
        Ok(report) => ScanCompletion::Completed(report),
        Err(error) => match loader::discovery_cancellation_reason(&error) {
            Some(reason) => ScanCompletion::Cancelled(reason),
            None => ScanCompletion::Failed(format!("{error:#}")),
        },
    }
}

async fn run_coordinator(
    inputs: DiscoveryInputs,
    mut events: mpsc::Receiver<loader::DiscoveryEvent>,
    completion: oneshot::Receiver<ScanCompletion>,
    cancellation: loader::DiscoveryCancellation,
) -> DiscoveryOutcome {
    let mut guard = CoordinatorGuard::new(
        cancellation.clone(),
        inputs.registry.clone(),
        inputs.shutdown.clone(),
    );
    while let Some(event) = events.recv().await {
        process_event(event, &inputs.registry);
    }

    let scan_completion = match completion.await {
        Ok(completion) => completion,
        Err(_) => ScanCompletion::Failed("loader worker ended without a result".to_string()),
    };

    let outcome = finish_scan(
        scan_completion,
        &inputs.registry,
        &inputs.filters,
        &inputs.input_paths,
    );
    if outcome == DiscoveryOutcome::Completed {
        inputs.registry.mark_scan_complete();
        if let Some(source) = inputs.annotation_source {
            load_annotations(
                source,
                inputs.registry.files_snapshot(),
                inputs.annotation_store,
                cancellation.clone(),
            )
            .await;
        }
    }
    guard.finish(outcome)
}

fn process_event(event: loader::DiscoveryEvent, registry: &FileRegistry) {
    match event {
        loader::DiscoveryEvent::File(file) => {
            registry.record_scanned();
            registry.insert(*file);
        }
        loader::DiscoveryEvent::Skipped => registry.record_skipped(),
        loader::DiscoveryEvent::Filtered => registry.record_filtered(),
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
    files: Vec<dcmview::types::FileEntry>,
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
    completion: ScanCompletion,
    registry: &FileRegistry,
    filters: &[loader::ScanFilter],
    input_paths: &[PathBuf],
) -> DiscoveryOutcome {
    match completion {
        ScanCompletion::Completed(report) => {
            let files = registry.files_snapshot();
            if files.is_empty() {
                if report.filtered > 0 {
                    eprintln!(
                        "dcmview: no DICOM files matched active filters ({})",
                        format_scan_filters(filters)
                    );
                } else {
                    eprintln!("dcmview: no valid DICOM files found");
                }
                return DiscoveryOutcome::Failed(DiscoveryFailure::NoFiles);
            }

            print_progressive_load_summary(
                files.len(),
                report.skipped,
                report.filtered,
                report.searched_recursive,
                filters,
                input_paths,
            );
            DiscoveryOutcome::Completed
        }
        ScanCompletion::Cancelled(loader::DiscoveryCancellationReason::Requested) => {
            DiscoveryOutcome::Cancelled(loader::DiscoveryCancellationReason::Requested)
        }
        ScanCompletion::Cancelled(
            reason @ loader::DiscoveryCancellationReason::EventReceiverClosed,
        ) => {
            eprintln!("failed to discover DICOM files: DICOM discovery cancelled: {reason}");
            DiscoveryOutcome::Failed(DiscoveryFailure::Scan)
        }
        ScanCompletion::Failed(error) => {
            eprintln!("failed to discover DICOM files: {error}");
            DiscoveryOutcome::Failed(DiscoveryFailure::Scan)
        }
    }
}

struct CoordinatorGuard {
    cancellation: loader::DiscoveryCancellation,
    registry: FileRegistry,
    shutdown: Arc<Notify>,
    armed: bool,
}

impl CoordinatorGuard {
    fn new(
        cancellation: loader::DiscoveryCancellation,
        registry: FileRegistry,
        shutdown: Arc<Notify>,
    ) -> Self {
        Self {
            cancellation,
            registry,
            shutdown,
            armed: true,
        }
    }

    fn finish(&mut self, outcome: DiscoveryOutcome) -> DiscoveryOutcome {
        self.registry.mark_scan_complete();
        if outcome.is_failure() {
            self.shutdown.notify_one();
        }
        self.armed = false;
        outcome
    }
}

impl Drop for CoordinatorGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cancellation.cancel();
            self.registry.mark_scan_complete();
            self.shutdown.notify_one();
        }
    }
}

fn print_progressive_load_summary(
    file_count: usize,
    skipped: usize,
    filtered: usize,
    searched_recursive: bool,
    filters: &[loader::ScanFilter],
    input_paths: &[PathBuf],
) {
    let recursive_note = if searched_recursive {
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
        notes.push(format!("{skipped} skipped — not valid DICOM"));
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
        println!("dcmview: loaded 1 DICOM file");
    } else {
        println!("dcmview: loaded {file_count} DICOM file(s) from {path_label} ({note})");
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
    use dcmview::types::FileEntry;
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Clone)]
    enum TestBehavior {
        Complete(FileEntry),
        CompleteEmpty,
        WaitForCancellation(FileEntry),
        Fail,
    }

    struct TestSpawner {
        behavior: TestBehavior,
        spawn_count: Arc<AtomicUsize>,
        finished: Arc<AtomicBool>,
    }

    impl TestSpawner {
        fn new(behavior: TestBehavior) -> Self {
            Self {
                behavior,
                spawn_count: Arc::new(AtomicUsize::new(0)),
                finished: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl DiscoverySpawner for TestSpawner {
        fn spawn(
            &self,
            request: ScanRequest,
            events: mpsc::Sender<loader::DiscoveryEvent>,
            cancellation: loader::DiscoveryCancellation,
        ) -> DiscoveryFuture {
            self.spawn_count.fetch_add(1, Ordering::Relaxed);
            let behavior = self.behavior.clone();
            let finished = self.finished.clone();
            Box::pin(async move {
                let result = match behavior {
                    TestBehavior::Complete(file) => {
                        events
                            .send(selected_event(file))
                            .await
                            .map_err(|_| anyhow::anyhow!("synthetic event receiver closed"))?;
                        Ok(loader::DiscoveryReport {
                            files_found: 1,
                            skipped: 0,
                            filtered: 0,
                            searched_recursive: request.options.recursive,
                        })
                    }
                    TestBehavior::CompleteEmpty => Ok(loader::DiscoveryReport {
                        files_found: 0,
                        skipped: 0,
                        filtered: 0,
                        searched_recursive: request.options.recursive,
                    }),
                    TestBehavior::WaitForCancellation(file) => {
                        events
                            .send(selected_event(file))
                            .await
                            .map_err(|_| anyhow::anyhow!("synthetic event receiver closed"))?;
                        while !cancellation.is_cancelled() {
                            tokio::task::yield_now().await;
                        }
                        loader::discover_progressive_with_cancellation(
                            &[],
                            request.options,
                            events,
                            cancellation,
                        )
                        .await
                    }
                    TestBehavior::Fail => Err(anyhow::anyhow!("synthetic scan failure")),
                };
                finished.store(true, Ordering::Release);
                result
            })
        }
    }

    fn synthetic_file(path: PathBuf) -> FileEntry {
        FileEntry {
            index: 0,
            path,
            label: "synthetic".to_string(),
            patient_id: "PATIENT".to_string(),
            patient_name: "Test^Patient".to_string(),
            study_instance_uid: "1.2.3".to_string(),
            study_date: "20260101".to_string(),
            study_description: "Study".to_string(),
            series_instance_uid: "1.2.3.4".to_string(),
            series_number: "1".to_string(),
            series_description: "Series".to_string(),
            modality: "CT".to_string(),
            instance_number: "1".to_string(),
            sop_instance_uid: "1.2.3.4.5".to_string(),
            sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
            series_metadata: Default::default(),
            has_pixels: true,
            frame_count: 1,
            rows: 16,
            columns: 16,
            bits_allocated: 16,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
            default_window: None,
        }
    }

    fn selected_event(file: FileEntry) -> loader::DiscoveryEvent {
        let path = file.path.clone();
        loader::DiscoveryEvent::Selected {
            file: Box::new(file),
            record: loader::DiscoveryRecord {
                path,
                disposition: loader::DiscoveryDisposition::Selected,
                reason: loader::DiscoveryReason::ValidDicom,
            },
        }
    }

    fn discovery_inputs(
        file_path: PathBuf,
        annotation_source: Option<AnnotationSource>,
    ) -> (DiscoveryInputs, FileRegistry, AnnotationStore, Arc<Notify>) {
        let registry = FileRegistry::new();
        let annotation_store = if annotation_source.is_some() {
            AnnotationStore::loading()
        } else {
            AnnotationStore::empty()
        };
        let shutdown = Arc::new(Notify::new());
        (
            DiscoveryInputs {
                input_paths: vec![file_path],
                recursive: true,
                filters: Vec::new(),
                annotation_source,
                registry: registry.clone(),
                annotation_store: annotation_store.clone(),
                shutdown: shutdown.clone(),
            },
            registry,
            annotation_store,
            shutdown,
        )
    }

    async fn wait_for_registry_file(registry: &FileRegistry) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.status().file_count == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("synthetic file should be inserted");
    }

    #[tokio::test]
    async fn cancellation_awaits_the_owned_scan_worker() {
        let path = PathBuf::from("/synthetic/scan.dcm");
        let spawner = TestSpawner::new(TestBehavior::WaitForCancellation(synthetic_file(
            path.clone(),
        )));
        let (inputs, registry, _, _) = discovery_inputs(path, None);
        let handle = DiscoveryHandle::spawn(inputs, &spawner);
        wait_for_registry_file(&registry).await;

        let outcome = handle.cancel_and_wait().await;

        assert_eq!(
            outcome,
            DiscoveryOutcome::Cancelled(loader::DiscoveryCancellationReason::Requested)
        );
        assert!(spawner.finished.load(Ordering::Acquire));
        assert!(registry.status().scan_complete);
    }

    #[tokio::test]
    async fn completed_scan_returns_normal_outcome_and_marks_registry_complete() {
        let path = PathBuf::from("/synthetic/scan.dcm");
        let spawner = TestSpawner::new(TestBehavior::Complete(synthetic_file(path.clone())));
        let (inputs, registry, _, _) = discovery_inputs(path, None);
        let handle = DiscoveryHandle::spawn(inputs, &spawner);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !registry.status().scan_complete {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("synthetic scan should complete");

        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Completed);
        assert!(spawner.finished.load(Ordering::Acquire));
        assert_eq!(registry.status().file_count, 1);
        assert_eq!(
            registry.discovery_response_snapshot(),
            vec![loader::DiscoveryRecord {
                path: PathBuf::from("/synthetic/scan.dcm"),
                disposition: loader::DiscoveryDisposition::Selected,
                reason: loader::DiscoveryReason::ValidDicom,
            }]
        );
    }

    #[tokio::test]
    async fn annotation_failure_keeps_viewer_running_and_marks_store_failed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = temp.path().join("scan.dcm");
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
        let spawner = TestSpawner::new(TestBehavior::Complete(synthetic_file(file_path.clone())));
        let (inputs, registry, store, shutdown) = discovery_inputs(file_path, Some(source));
        let handle = DiscoveryHandle::spawn(inputs, &spawner);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handle
                .coordinator
                .as_ref()
                .expect("coordinator handle")
                .is_finished()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("annotation failure coordinator should finish");
        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Completed);
        assert!(store
            .wait_until_ready()
            .await
            .expect_err("invalid matched annotations should fail")
            .to_string()
            .contains("contains frame 2"));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), shutdown.notified())
                .await
                .is_err()
        );
        assert!(spawner.finished.load(Ordering::Acquire));
        assert!(registry.status().scan_complete);
    }

    #[tokio::test]
    async fn scan_failure_marks_failure_and_notifies_shutdown() {
        let path = PathBuf::from("/synthetic/scan.dcm");
        let spawner = TestSpawner::new(TestBehavior::Fail);
        let (inputs, registry, _, shutdown) = discovery_inputs(path, None);
        let handle = DiscoveryHandle::spawn(inputs, &spawner);

        tokio::time::timeout(Duration::from_secs(1), shutdown.notified())
            .await
            .expect("scan failure should notify shutdown");
        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Failed(DiscoveryFailure::Scan));
        assert!(spawner.finished.load(Ordering::Acquire));
        assert!(registry.status().scan_complete);
    }

    #[tokio::test]
    async fn zero_files_returns_failure_after_worker_completion() {
        let path = PathBuf::from("/synthetic/scan.dcm");
        let spawner = TestSpawner::new(TestBehavior::CompleteEmpty);
        let (inputs, registry, _, shutdown) = discovery_inputs(path, None);
        let handle = DiscoveryHandle::spawn(inputs, &spawner);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !handle
                .coordinator
                .as_ref()
                .expect("coordinator handle")
                .is_finished()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("empty discovery should finish");
        tokio::time::timeout(Duration::from_millis(20), shutdown.notified())
            .await
            .expect("empty discovery should durably notify");
        let outcome = handle.cancel_and_wait().await;

        assert_eq!(outcome, DiscoveryOutcome::Failed(DiscoveryFailure::NoFiles));
        assert!(spawner.finished.load(Ordering::Acquire));
        assert!(registry.status().scan_complete);
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
