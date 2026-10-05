//! Process stop signals, listened for from the moment they are created.
//!
//! `tokio::signal::ctrl_c()` installs its handler only when first polled. A
//! wrapper that stops the process as soon as it reads the announced URL can
//! land its signal before that, and the OS default action then kills the
//! process without a graceful shutdown. Create [`StopSignals`] before printing
//! anything a wrapper may react to.

use std::future::Future;

/// Stop when the supervising parent's stdin pipe closes or cannot be read.
pub fn stop_on_stdin_eof(shutdown: tokio_util::sync::CancellationToken) -> std::io::Result<()> {
    // Tokio stdin and spawn_blocking use runtime-owned blocking workers that
    // cannot be cancelled during a read. A detached OS thread lets the runtime
    // and process exit even if another stop source fires with stdin still open.
    std::thread::Builder::new()
        .name("dcmview-parent-stdin".into())
        .spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            shutdown.cancel();
        })?;
    Ok(())
}

/// Ctrl+C and SIGTERM on Unix; Ctrl+C and Ctrl+Break on Windows.
pub struct StopSignals {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(windows)]
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
    #[cfg(windows)]
    ctrl_break: Option<tokio::signal::windows::CtrlBreak>,
}

impl StopSignals {
    /// Register the listeners now. Must run inside a Tokio runtime. A listener
    /// that cannot be registered is reported on stderr and never fires.
    pub fn listen() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Self {
                interrupt: installed("Ctrl+C", signal(SignalKind::interrupt())),
                terminate: installed("SIGTERM", signal(SignalKind::terminate())),
            }
        }

        #[cfg(windows)]
        {
            use tokio::signal::windows::{ctrl_break, ctrl_c};
            Self {
                ctrl_c: installed("Ctrl+C", ctrl_c()),
                ctrl_break: installed("Ctrl+Break", ctrl_break()),
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            Self {}
        }
    }

    /// Wait for the next stop signal.
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = next(self.interrupt.as_mut().map(|signal| signal.recv())) => {}
                _ = next(self.terminate.as_mut().map(|signal| signal.recv())) => {}
            }
        }

        #[cfg(windows)]
        {
            tokio::select! {
                _ = next(self.ctrl_c.as_mut().map(|signal| signal.recv())) => {}
                _ = next(self.ctrl_break.as_mut().map(|signal| signal.recv())) => {}
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            std::future::pending::<()>().await;
        }
    }
}

fn installed<T>(name: &str, listener: std::io::Result<T>) -> Option<T> {
    listener
        .map_err(|error| eprintln!("dcmview: warning — failed to install {name} handler: {error}"))
        .ok()
}

/// Await a registered listener; a missing one never fires.
async fn next<F: Future>(recv: Option<F>) {
    match recv {
        Some(recv) => {
            recv.await;
        }
        None => std::future::pending().await,
    }
}
