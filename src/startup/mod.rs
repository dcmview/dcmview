mod discovery;

use anyhow::{Context, Result};
use dcmview::annotations::{AnnotationSource, AnnotationStore};
use dcmview::loader;
use dcmview::server::{AppState, BoundServer, FileRegistry, ServerConfig, TunnelConfig};
use discovery::{DiscoveryHandle, DiscoveryInputs, DiscoverySpawner, LoaderDiscoverySpawner};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Debug)]
pub(crate) struct LocalViewerOptions {
    pub(crate) input_paths: Vec<PathBuf>,
    pub(crate) recursive: bool,
    pub(crate) filters: Vec<loader::ScanFilter>,
    pub(crate) annotation_path: Option<PathBuf>,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) open_browser: bool,
    pub(crate) startup_json: bool,
    pub(crate) tunnel_enabled: bool,
    pub(crate) tunnel_host: Option<String>,
    pub(crate) tunnel_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalViewerOutcome {
    Completed,
    DiscoveryFailed,
}

impl LocalViewerOutcome {
    pub(crate) fn exit_code(self) -> i32 {
        match self {
            Self::Completed => 0,
            Self::DiscoveryFailed => 1,
        }
    }
}

pub(crate) async fn run_local_viewer(options: LocalViewerOptions) -> Result<LocalViewerOutcome> {
    run_local_viewer_with_spawner(options, &LoaderDiscoverySpawner).await
}

async fn run_local_viewer_with_spawner(
    options: LocalViewerOptions,
    spawner: &dyn DiscoverySpawner,
) -> Result<LocalViewerOutcome> {
    let tunnel = if options.tunnel_enabled {
        let host = options
            .tunnel_host
            .clone()
            .ok_or_else(|| anyhow::anyhow!("dcmview: --tunnel requires --tunnel-host"))?;
        Some(TunnelConfig {
            host,
            port: options.tunnel_port,
        })
    } else {
        None
    };
    let annotation_source = options
        .annotation_path
        .as_ref()
        .map(|path| {
            AnnotationSource::from_path(path)
                .with_context(|| format!("failed to load annotations from {}", path.display()))
        })
        .transpose()?;
    let registry = FileRegistry::new();
    let annotation_store = if annotation_source.is_some() {
        AnnotationStore::loading()
    } else {
        AnnotationStore::empty()
    };
    let state = AppState::new(registry.clone(), annotation_store.clone());
    let shutdown = Arc::new(Notify::new());
    let config = ServerConfig {
        host: options.host,
        port: options.port,
        timeout_seconds: options.timeout_seconds,
        open_browser: options.open_browser,
        startup_json: options.startup_json,
        tunnel,
        shutdown: Some(shutdown.clone()),
    };

    let bound = BoundServer::bind(&config)
        .await
        .map_err(|error| friendly_bind_error(error, config.port))?;
    let discovery = DiscoveryHandle::spawn(
        DiscoveryInputs {
            input_paths: options.input_paths,
            recursive: options.recursive,
            filters: options.filters,
            annotation_source,
            registry,
            annotation_store,
            shutdown,
        },
        spawner,
    );

    let server_result = bound.serve(config, state).await;
    let discovery_outcome = discovery.cancel_and_wait().await;
    server_result?;

    if discovery_outcome.is_failure() {
        Ok(LocalViewerOutcome::DiscoveryFailed)
    } else {
        Ok(LocalViewerOutcome::Completed)
    }
}

fn friendly_bind_error(error: anyhow::Error, port: u16) -> anyhow::Error {
    let address_in_use = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
    });
    if port != 0 && address_in_use {
        anyhow::anyhow!("dcmview: port {port} is already in use — try --port 0 for auto-assign")
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcmview::types::FileEntry;
    use discovery::{DiscoveryFuture, ScanRequest};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    struct CountingSpawner {
        count: AtomicUsize,
    }

    impl CountingSpawner {
        fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
            }
        }
    }

    impl DiscoverySpawner for CountingSpawner {
        fn spawn(
            &self,
            _request: ScanRequest,
            _events: mpsc::Sender<loader::DiscoveryEvent>,
            _cancellation: loader::DiscoveryCancellation,
        ) -> DiscoveryFuture {
            self.count.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Err(anyhow::anyhow!("unexpected synthetic discovery")) })
        }
    }

    struct WaitingSpawner {
        finished: Arc<AtomicBool>,
    }

    impl DiscoverySpawner for WaitingSpawner {
        fn spawn(
            &self,
            _request: ScanRequest,
            events: mpsc::Sender<loader::DiscoveryEvent>,
            cancellation: loader::DiscoveryCancellation,
        ) -> DiscoveryFuture {
            let finished = self.finished.clone();
            Box::pin(async move {
                events
                    .send(loader::DiscoveryEvent::File(Box::new(synthetic_file())))
                    .await
                    .map_err(|_| anyhow::anyhow!("startup event receiver closed"))?;
                while !cancellation.is_cancelled() {
                    tokio::task::yield_now().await;
                }
                let result = loader::discover_progressive_with_cancellation(
                    &[],
                    loader::DiscoverOptions {
                        recursive: true,
                        filters: Vec::new(),
                    },
                    events,
                    cancellation,
                )
                .await;
                finished.store(true, Ordering::Release);
                result
            })
        }
    }

    fn synthetic_file() -> FileEntry {
        FileEntry {
            index: 0,
            path: PathBuf::from("/synthetic/scan.dcm"),
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

    fn options(port: u16) -> LocalViewerOptions {
        LocalViewerOptions {
            input_paths: vec![PathBuf::from("/synthetic/scan.dcm")],
            recursive: true,
            filters: Vec::new(),
            annotation_path: None,
            host: "127.0.0.1".to_string(),
            port,
            timeout_seconds: Some(0),
            open_browser: false,
            startup_json: false,
            tunnel_enabled: false,
            tunnel_host: None,
            tunnel_port: 0,
        }
    }

    #[tokio::test]
    async fn bind_failure_happens_before_discovery_is_spawned() {
        let occupied = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupy listener");
        let port = occupied.local_addr().expect("occupied address").port();
        let spawner = CountingSpawner::new();

        let error = run_local_viewer_with_spawner(options(port), &spawner)
            .await
            .expect_err("occupied port should fail");

        assert_eq!(spawner.count.load(Ordering::Relaxed), 0);
        assert_eq!(
            error.to_string(),
            format!("dcmview: port {port} is already in use — try --port 0 for auto-assign")
        );
    }

    #[tokio::test]
    async fn bind_failures_other_than_address_in_use_keep_their_cause() {
        // TEST-NET-1 is never assigned to a local interface, so binding fails
        // with an address-unavailable error rather than an occupied port.
        let mut unavailable = options(8765);
        unavailable.host = "192.0.2.1".to_string();

        let error = run_local_viewer_with_spawner(unavailable, &CountingSpawner::new())
            .await
            .expect_err("unassigned address should fail");

        assert!(!error.to_string().contains("already in use"), "{error:#}");
        assert!(format!("{error:#}").contains("192.0.2.1:8765"), "{error:#}");
    }

    #[tokio::test]
    async fn normal_server_exit_cancels_and_awaits_incomplete_discovery() {
        let finished = Arc::new(AtomicBool::new(false));
        let spawner = WaitingSpawner {
            finished: finished.clone(),
        };

        let outcome = run_local_viewer_with_spawner(options(0), &spawner)
            .await
            .expect("local viewer exits normally");

        assert_eq!(outcome, LocalViewerOutcome::Completed);
        assert!(finished.load(Ordering::Acquire));
    }

    #[test]
    fn local_viewer_outcomes_have_stable_exit_codes() {
        assert_eq!(LocalViewerOutcome::Completed.exit_code(), 0);
        assert_eq!(LocalViewerOutcome::DiscoveryFailed.exit_code(), 1);
    }
}
