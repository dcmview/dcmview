//! Monotonic request activity and graceful-shutdown policy.

use super::FileRegistry;
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::futures::Notified;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct RequestActivity {
    inner: Arc<RequestActivityInner>,
}

struct RequestActivityInner {
    state: Mutex<RequestActivityState>,
    changed: Notify,
}

#[derive(Debug, Clone, Copy)]
struct RequestActivityState {
    ready: bool,
    last_activity: Instant,
    in_flight: usize,
}

#[must_use = "the guard keeps a request marked as in flight until it is dropped"]
pub struct RequestActivityGuard {
    activity: RequestActivity,
}

impl RequestActivity {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RequestActivityInner {
                state: Mutex::new(RequestActivityState {
                    ready: false,
                    last_activity: Instant::now(),
                    in_flight: 0,
                }),
                changed: Notify::new(),
            }),
        }
    }

    pub fn request_started(&self) -> RequestActivityGuard {
        {
            let mut state = self.state();
            state.in_flight = state.in_flight.saturating_add(1);
            state.last_activity = Instant::now();
        }
        self.inner.changed.notify_waiters();
        RequestActivityGuard {
            activity: self.clone(),
        }
    }

    pub fn in_flight(&self) -> usize {
        self.snapshot().in_flight
    }

    fn state(&self) -> MutexGuard<'_, RequestActivityState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn snapshot(&self) -> RequestActivityState {
        *self.state()
    }

    fn mark_ready(&self) {
        let changed = {
            let mut state = self.state();
            if state.ready {
                false
            } else {
                state.ready = true;
                state.last_activity = Instant::now();
                true
            }
        };
        if changed {
            self.inner.changed.notify_waiters();
        }
    }

    fn changed(&self) -> Notified<'_> {
        self.inner.changed.notified()
    }
}

impl Default for RequestActivity {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RequestActivityGuard {
    fn drop(&mut self) {
        {
            let mut state = self.activity.state();
            state.in_flight = state.in_flight.saturating_sub(1);
            state.last_activity = Instant::now();
        }
        self.activity.inner.changed.notify_waiters();
    }
}

/// Resolve when the server should stop: on a stop signal, when `external` is
/// cancelled, or after `timeout` of idleness once the scan has finished.
pub(crate) async fn wait_for_shutdown(
    activity: RequestActivity,
    registry: FileRegistry,
    timeout: Option<Duration>,
    external: CancellationToken,
    stop_signal: impl Future<Output = ()>,
) {
    let idle = async move {
        match timeout {
            Some(timeout) => idle_timeout(activity, registry, timeout).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        () = stop_signal => {}
        () = external.cancelled() => {}
        () = idle => {}
    }
}

async fn idle_timeout(activity: RequestActivity, registry: FileRegistry, timeout: Duration) {
    wait_until_scan_complete(&activity, &registry).await;

    loop {
        let changed = activity.changed();
        let mut changed = pin!(changed);
        changed.as_mut().enable();
        let snapshot = activity.snapshot();

        if snapshot.in_flight > 0 {
            changed.await;
            continue;
        }

        let deadline = snapshot.last_activity + timeout;
        if Instant::now() >= deadline {
            return;
        }

        tokio::select! {
            _ = &mut changed => {}
            _ = tokio::time::sleep_until(deadline) => {
                let current = activity.snapshot();
                if current.in_flight == 0 && Instant::now() >= current.last_activity + timeout {
                    return;
                }
            }
        }
    }
}

/// Discovery progress is not request activity, so the idle clock starts only
/// when the scan has finished; a long scan never counts as idleness.
async fn wait_until_scan_complete(activity: &RequestActivity, registry: &FileRegistry) {
    loop {
        let changed = registry.changed();
        let mut changed = pin!(changed);
        changed.as_mut().enable();
        if registry.status().scan_complete {
            activity.mark_ready();
            return;
        }
        changed.await;
    }
}

#[cfg(test)]
mod tests {
    use super::{idle_timeout, RequestActivity};
    use crate::server::FileRegistry;
    use std::time::Duration;

    fn ready_registry() -> FileRegistry {
        let registry = FileRegistry::new();
        registry.mark_scan_complete();
        registry
    }

    async fn settle() {
        tokio::task::yield_now().await;
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_fires_at_the_exact_deadline() {
        let activity = RequestActivity::new();
        let task = tokio::spawn(idle_timeout(
            activity,
            ready_registry(),
            Duration::from_secs(5),
        ));
        settle().await;

        tokio::time::advance(Duration::from_millis(4_999)).await;
        settle().await;
        assert!(!task.is_finished());

        tokio::time::advance(Duration::from_millis(1)).await;
        task.await.expect("idle task");
    }

    #[tokio::test(start_paused = true)]
    async fn request_activity_resets_the_idle_deadline() {
        let activity = RequestActivity::new();
        let task = tokio::spawn(idle_timeout(
            activity.clone(),
            ready_registry(),
            Duration::from_secs(5),
        ));
        settle().await;

        tokio::time::advance(Duration::from_secs(4)).await;
        drop(activity.request_started());
        tokio::time::advance(Duration::from_secs(4)).await;
        settle().await;
        assert!(!task.is_finished());

        tokio::time::advance(Duration::from_secs(1)).await;
        task.await.expect("idle task");
    }

    #[tokio::test(start_paused = true)]
    async fn incomplete_empty_registry_suppresses_idle_timeout() {
        let task = tokio::spawn(idle_timeout(
            RequestActivity::new(),
            FileRegistry::new(),
            Duration::from_secs(5),
        ));
        settle().await;

        tokio::time::advance(Duration::from_secs(60)).await;
        settle().await;
        assert!(!task.is_finished());
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn scan_in_progress_suppresses_idle_timeout_after_files_arrive() {
        let registry = FileRegistry::new();
        let task = tokio::spawn(idle_timeout(
            RequestActivity::new(),
            registry.clone(),
            Duration::from_secs(5),
        ));
        settle().await;

        registry.insert(crate::loader::test_entry(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm"),
        ));
        settle().await;
        tokio::time::advance(Duration::from_secs(60)).await;
        settle().await;
        assert!(!task.is_finished());
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn scan_completion_starts_a_fresh_idle_baseline() {
        let registry = FileRegistry::new();
        let task = tokio::spawn(idle_timeout(
            RequestActivity::new(),
            registry.clone(),
            Duration::from_secs(5),
        ));
        settle().await;

        tokio::time::advance(Duration::from_secs(60)).await;
        registry.mark_scan_complete();
        settle().await;
        tokio::time::advance(Duration::from_millis(4_999)).await;
        settle().await;
        assert!(!task.is_finished());

        tokio::time::advance(Duration::from_millis(1)).await;
        task.await.expect("idle task");
    }

    #[tokio::test(start_paused = true)]
    async fn in_flight_request_suppresses_timeout_until_completion() {
        let activity = RequestActivity::new();
        let guard = activity.request_started();
        let task = tokio::spawn(idle_timeout(
            activity,
            ready_registry(),
            Duration::from_secs(5),
        ));
        settle().await;

        tokio::time::advance(Duration::from_secs(60)).await;
        settle().await;
        assert!(!task.is_finished());

        drop(guard);
        tokio::time::advance(Duration::from_secs(5)).await;
        task.await.expect("idle task");
    }

    #[tokio::test(start_paused = true)]
    async fn zero_timeout_exits_as_soon_as_the_scan_completes() {
        idle_timeout(RequestActivity::new(), ready_registry(), Duration::ZERO).await;
    }
}
