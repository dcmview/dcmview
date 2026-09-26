//! Bound listener ownership and graceful server resource management.

use super::lifecycle::{wait_for_shutdown, ExternalShutdown, ShutdownReason};
use super::{is_non_loopback_bind, router, AppState, FileRegistry};
use crate::signals::StopSignals;
use anyhow::{Context, Result};
use serde::Serialize;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Notify};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub timeout_seconds: Option<u64>,
    pub open_browser: bool,
    pub startup_json: bool,
    pub shutdown: Option<Arc<Notify>>,
}

pub struct BoundServer {
    listener: TcpListener,
    local_addr: SocketAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerExit {
    pub local_addr: SocketAddr,
    pub reason: ShutdownReason,
}

#[derive(Debug, Serialize)]
struct StartupEvent<'a> {
    r#type: &'a str,
    url: &'a str,
    host: &'a str,
    port: u16,
}

impl BoundServer {
    pub async fn bind(config: &ServerConfig) -> Result<Self> {
        let bind_addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(&bind_addr)
            .await
            .with_context(|| format!("failed to bind to {bind_addr}"))?;
        let local_addr = listener
            .local_addr()
            .context("failed to read local bind address")?;
        Ok(Self {
            listener,
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn url(&self) -> String {
        socket_url(self.local_addr)
    }

    pub async fn serve(self, config: ServerConfig, state: AppState) -> Result<ServerExit> {
        // Listen before announcing the URL: a wrapper may stop the process as
        // soon as it reads it, and an unhandled signal would skip the graceful
        // shutdown below.
        let mut stop_signals = StopSignals::listen();
        let external_shutdown = ExternalShutdown::new(config.shutdown.clone());
        let server_url = self.url();

        println!(
            "dcmview: (on a remote server? run on your local machine: ssh -L {0}:localhost:{0} user@host)",
            self.local_addr.port()
        );

        if is_non_loopback_bind(self.local_addr.ip()) {
            eprintln!(
                "dcmview: warning — server bound to non-loopback address {}; endpoints are unauthenticated and may expose sensitive DICOM data",
                self.local_addr.ip()
            );
            eprintln!(
                "dcmview: warning — prefer --host 127.0.0.1 (or ::1) and use SSH port forwarding for remote access"
            );
        }

        let activity = state.activity().clone();
        let registry = state.registry().clone();
        let app = router(state);
        let mut browser_task = BrowserTask::new(
            config
                .open_browser
                .then(|| spawn_browser_opener(server_url.clone(), registry.clone())),
        );

        let timeout = config.timeout_seconds.map(Duration::from_secs);
        let (reason_tx, reason_rx) = oneshot::channel();
        let shutdown = async move {
            let reason =
                wait_for_shutdown(activity, registry, timeout, external_shutdown, async move {
                    stop_signals.recv().await;
                    ShutdownReason::OsSignal
                })
                .await;
            let _ = reason_tx.send(reason);
        };

        if config.startup_json {
            println!(
                "{}",
                startup_event_json(&server_url, &config.host, self.local_addr.port())
                    .context("failed to serialize startup event")?
            );
        }
        println!("dcmview: server running at {server_url}");
        println!("dcmview: press Ctrl+C to stop");

        let serve_result = axum::serve(self.listener, app)
            .with_graceful_shutdown(shutdown)
            .await;

        browser_task.abort();
        serve_result.context("server failed")?;
        let reason = reason_rx
            .await
            .context("server stopped without a shutdown reason")?;
        println!("dcmview: shutting down...");

        Ok(ServerExit {
            local_addr: self.local_addr,
            reason,
        })
    }
}

pub async fn run(config: ServerConfig, state: AppState) -> Result<()> {
    BoundServer::bind(&config)
        .await?
        .serve(config, state)
        .await
        .map(|_| ())
}

pub fn startup_event_json(server_url: &str, host: &str, port: u16) -> serde_json::Result<String> {
    serde_json::to_string(&StartupEvent {
        r#type: "server_started",
        url: server_url,
        host,
        port,
    })
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
                if let Err(error) = open::that(&server_url) {
                    eprintln!("dcmview: warning — failed to open browser: {error}");
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
