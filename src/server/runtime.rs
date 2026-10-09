//! Bound listener ownership and graceful server resource management.

use super::lifecycle::wait_for_shutdown;
use super::{is_non_loopback_bind, router, AppState, FileRegistry};
use crate::api::contracts::{StartupEvent, TOKEN_FRAGMENT_PARAM};
use crate::signals::StopSignals;
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub unix_socket: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
    pub open_browser: bool,
    pub startup_json: bool,
    /// Cancelled by the owner to stop the server, e.g. when discovery fails.
    pub shutdown: CancellationToken,
}

pub enum BoundServer {
    Tcp {
        listener: TcpListener,
        local_addr: SocketAddr,
    },
    #[cfg(unix)]
    Unix(super::unix_socket::UnixSocket),
}

impl BoundServer {
    pub async fn bind(config: &ServerConfig) -> Result<Self> {
        if let Some(path) = &config.unix_socket {
            #[cfg(unix)]
            return super::unix_socket::UnixSocket::bind(path)
                .await
                .map(Self::Unix);
            #[cfg(not(unix))]
            {
                let _ = path;
                anyhow::bail!("--unix-socket is supported only on Unix (Linux and macOS)");
            }
        }
        let bind_addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("failed to bind to {bind_addr}"))?;
        let local_addr = listener
            .local_addr()
            .context("failed to read local bind address")?;
        Ok(Self::Tcp {
            listener,
            local_addr,
        })
    }

    /// The bound TCP address.
    ///
    /// # Panics
    /// Panics for a Unix socket, which has no TCP address.
    pub fn local_addr(&self) -> SocketAddr {
        match self {
            Self::Tcp { local_addr, .. } => *local_addr,
            #[cfg(unix)]
            Self::Unix(_) => panic!("Unix socket listeners have no TCP address"),
        }
    }

    /// The TCP viewer URL.
    ///
    /// # Panics
    /// Panics for a Unix socket; its forwarded URL is chosen by the user.
    pub fn url(&self) -> String {
        socket_url(self.local_addr())
    }

    pub async fn serve(self, config: ServerConfig, state: AppState) -> Result<()> {
        // Listen before announcing the URL: a wrapper may stop the process as
        // soon as it reads it, and an unhandled signal would skip the graceful
        // shutdown below.
        let mut stop_signals = StopSignals::listen();
        let access_token = state.access_token().cloned();
        let token = access_token.as_ref().map(|token| token.expose());
        let fragment = token
            .map(|token| format!("#{TOKEN_FRAGMENT_PARAM}={token}"))
            .unwrap_or_default();
        let startup_event = match &self {
            Self::Tcp { local_addr, .. } => {
                crate::status_line!(
                    "dcmview: (on a remote server? run on your local machine: ssh -L {0}:localhost:{0} user@host)",
                    local_addr.port()
                );
                crate::status_line!(
                    "dcmview: then open http://localhost:{}/{fragment}",
                    local_addr.port()
                );
                if is_non_loopback_bind(local_addr.ip()) {
                    let exposure = if token.is_some() {
                        "plain HTTP does not encrypt the access token or sensitive DICOM data"
                    } else {
                        "endpoints are unauthenticated and may expose sensitive DICOM data over plain HTTP"
                    };
                    eprintln!(
                        "dcmview: warning — server bound to non-loopback address {}; {exposure}",
                        local_addr.ip()
                    );
                    eprintln!(
                        "dcmview: warning — prefer --host 127.0.0.1 (or ::1) and use SSH port forwarding for remote access"
                    );
                }
                StartupEvent::tcp(
                    &socket_url(*local_addr),
                    &config.host,
                    local_addr.port(),
                    token,
                )
            }
            #[cfg(unix)]
            Self::Unix(socket) => {
                StartupEvent::unix_socket(&socket.path().to_string_lossy(), token)
            }
        }
        .with_key_rules(crate::keys::KEY_RULES);

        if state.registry().masker().is_some() {
            crate::status_line!(
                "dcmview: display masking is on — identifiers are replaced on screen only; files are not modified and this is not de-identification"
            );
        }

        let activity = state.activity().clone();
        let registry = state.registry().clone();
        let decode_scheduler = state.decode_scheduler();
        let app = router(state);
        let mut browser_task = BrowserTask::new(
            startup_event
                .url
                .as_ref()
                .filter(|_| config.open_browser)
                .map(|url| spawn_browser_opener(url.clone(), registry.clone())),
        );

        let timeout = config.timeout_seconds.map(Duration::from_secs);
        let shutdown = wait_for_shutdown(
            activity,
            registry,
            timeout,
            config.shutdown.clone(),
            async move { stop_signals.recv().await },
        );
        // The graceful shutdown below waits for every request in flight.
        // One that is waiting for decode capacity would be decoded first,
        // behind everything queued before it, so the wait had no bound:
        // waiting requests are refused instead (`503 decode_busy`), and
        // the server waits only for the decodes that are running.
        let shutdown = async move {
            shutdown.await;
            decode_scheduler.refuse_waiting();
        };

        if config.startup_json {
            crate::status_line!(
                "{}",
                serde_json::to_string(&startup_event)
                    .context("failed to serialize startup event")?
            );
        }
        match &self {
            Self::Tcp { .. } => {
                let server_url = startup_event.url.as_deref().expect("TCP viewer URL");
                crate::status_line!("dcmview: server running at {server_url}");
            }
            #[cfg(unix)]
            Self::Unix(socket) => {
                let path = socket.path().display();
                crate::status_line!("dcmview: listening on {path}");
                crate::status_line!(
                    "dcmview: on your local machine run: ssh -L 8080:{path} user@host"
                );
                crate::status_line!("dcmview: then open http://localhost:8080/{fragment}");
            }
        }
        crate::status_line!("dcmview: press Ctrl+C to stop");

        let serve_result = match self {
            Self::Tcp { listener, .. } => {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown)
                    .await
            }
            #[cfg(unix)]
            Self::Unix(listener) => {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown)
                    .await
            }
        };

        browser_task.abort();
        serve_result.context("server failed")?;
        crate::status_line!("dcmview: shutting down...");
        Ok(())
    }
}

fn socket_url(local_addr: SocketAddr) -> String {
    format!("http://{local_addr}")
}

fn spawn_browser_opener(server_url: String, registry: FileRegistry) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let changed = registry.changed();
            let mut changed = pin!(changed);
            changed.as_mut().enable();
            let status = registry.status();
            if status.file_count > 0 {
                if open::that(&server_url).is_err() {
                    // An opener error can contain its command arguments, including the token.
                    eprintln!(
                        "dcmview: warning — failed to open browser; open the launch URL manually"
                    );
                }
                return;
            }
            if status.scan_complete {
                return;
            }
            changed.await;
        }
    })
}

struct BrowserTask {
    handle: Option<JoinHandle<()>>,
}

impl BrowserTask {
    fn new(handle: Option<JoinHandle<()>>) -> Self {
        Self { handle }
    }

    fn abort(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

impl Drop for BrowserTask {
    fn drop(&mut self) {
        self.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::socket_url;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn socket_urls_are_ipv4_and_ipv6_correct() {
        assert_eq!(
            socket_url(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42)),
            "http://127.0.0.1:42"
        );
        assert_eq!(
            socket_url(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 42)),
            "http://[::1]:42"
        );
    }
}
