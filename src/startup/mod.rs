mod discovery;

use anyhow::{Context, Result};
use dcmview::annotations::{AnnotationSource, AnnotationStore};
use dcmview::api::contracts::TOKEN_ENV_VAR;
use dcmview::loader;
use dcmview::masking::Masker;
use dcmview::server::{AccessToken, AppState, BoundServer, FileRegistry, ServerConfig};
use discovery::{DiscoveryHandle, DiscoveryInputs, DiscoveryOutcome};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(crate) struct LocalViewerOptions {
    pub(crate) input_paths: Vec<PathBuf>,
    pub(crate) recursive: bool,
    pub(crate) filters: Vec<loader::ScanFilter>,
    pub(crate) annotation_path: Option<PathBuf>,
    pub(crate) mask: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) unix_socket: Option<PathBuf>,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) open_browser: bool,
    pub(crate) startup_json: bool,
    pub(crate) no_token: bool,
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
    let fixed_token = std::env::var_os(TOKEN_ENV_VAR);
    anyhow::ensure!(
        !(options.no_token && fixed_token.is_some()),
        "--no-token cannot be used while DCMVIEW_TOKEN is set"
    );
    let access_token = if options.no_token {
        eprintln!(
            "dcmview: warning — --no-token: the API is open to anything that can reach the listener"
        );
        None
    } else {
        Some(match fixed_token {
            Some(value) => AccessToken::fixed(
                value
                    .to_str()
                    .context("DCMVIEW_TOKEN must contain only A-Z a-z 0-9 - . _ ~")?,
            )?,
            None => AccessToken::generate()?,
        })
    };
    let annotation_source = options
        .annotation_path
        .as_ref()
        .map(|path| {
            AnnotationSource::from_path(path)
                .with_context(|| format!("failed to load annotations from {}", path.display()))
        })
        .transpose()?;
    let registry = if options.mask {
        FileRegistry::masked(Arc::new(Masker::new()))
    } else {
        FileRegistry::new()
    };
    let annotation_store = if annotation_source.is_some() {
        AnnotationStore::loading()
    } else {
        AnnotationStore::empty()
    };
    let mut state = AppState::new(registry.clone(), annotation_store.clone());
    if let Some(token) = access_token {
        state = state.with_access_token(token);
    }
    let shutdown = CancellationToken::new();
    let config = ServerConfig {
        host: options.host,
        port: options.port,
        unix_socket: options.unix_socket,
        timeout_seconds: options.timeout_seconds,
        open_browser: options.open_browser,
        startup_json: options.startup_json,
        shutdown: shutdown.clone(),
    };

    let bound = BoundServer::bind(&config)
        .await
        .map_err(|error| friendly_bind_error(error, config.port))?;
    let discovery = DiscoveryHandle::spawn(DiscoveryInputs {
        input_paths: options.input_paths,
        recursive: options.recursive,
        filters: options.filters,
        annotation_source,
        registry,
        annotation_store,
        shutdown,
        startup_json: config.startup_json,
    });

    let server_result = bound.serve(config, state).await;
    let discovery_outcome = discovery.cancel_and_wait().await;
    server_result?;

    if discovery_outcome == DiscoveryOutcome::Failed {
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
    use tokio::net::TcpListener;

    fn options(port: u16) -> LocalViewerOptions {
        LocalViewerOptions {
            input_paths: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")],
            recursive: true,
            filters: Vec::new(),
            annotation_path: None,
            mask: false,
            host: "127.0.0.1".to_string(),
            port,
            unix_socket: None,
            timeout_seconds: Some(0),
            open_browser: false,
            startup_json: false,
            no_token: false,
        }
    }

    #[tokio::test]
    async fn occupied_port_fails_with_the_cli_hint() {
        let occupied = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("occupy listener");
        let port = occupied.local_addr().expect("occupied address").port();

        let error = run_local_viewer(options(port))
            .await
            .expect_err("occupied port should fail");

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

        let error = run_local_viewer(unavailable)
            .await
            .expect_err("unassigned address should fail");

        assert!(!error.to_string().contains("already in use"), "{error:#}");
        assert!(format!("{error:#}").contains("192.0.2.1:8765"), "{error:#}");
    }

    #[tokio::test]
    async fn normal_server_exit_cancels_and_awaits_discovery() {
        // A zero idle timeout stops the server as soon as the first file is
        // registered, usually while the rest of the fixtures are still being
        // scanned; either way the outcome is a normal exit.
        let outcome = run_local_viewer(options(0))
            .await
            .expect("local viewer exits normally");

        assert_eq!(outcome, LocalViewerOutcome::Completed);
    }

    #[test]
    fn local_viewer_outcomes_have_stable_exit_codes() {
        assert_eq!(LocalViewerOutcome::Completed.exit_code(), 0);
        assert_eq!(LocalViewerOutcome::DiscoveryFailed.exit_code(), 1);
    }
}
