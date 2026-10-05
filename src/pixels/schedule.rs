//! Decode classes: who gets a core when the viewer and the gallery both want
//! one.
//!
//! Every decode holds a [`DecodePermit`] while it runs on the blocking pool.
//! The pool of permits is the host's core count, as before; what is new is
//! that a request names its [`DecodeClass`], and background work (gallery
//! thumbnails) yields to interactive work (the frame the user is looking
//! at). A decode that holds a permit runs to completion, so this bounds the
//! delay the gallery adds to the viewer rather than removing it; the bound
//! is [`INTERACTIVE_LATENCY_TARGET`].
//!
//! The class is a property of the endpoint, not of the request: thumbnails
//! are background work, every other decode is interactive. It is unrelated
//! to the `X-Dcmview-Background` request header, which only keeps a request
//! off the idle clock and never changes how it is served.

use std::sync::Arc;
use std::time::Duration;

/// How urgent a decode is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecodeClass {
    /// Work the user is waiting on: display frames, raw frames, previews
    /// and the frames overlays are resampled from.
    Interactive,
    /// Work nobody is blocked on: gallery thumbnails, and any later
    /// non-interactive pixel work.
    Background,
}

/// The delay the gallery may add to the viewer: while thumbnails load, the
/// 95th percentile of the extra time an interactive display frame takes,
/// compared with the same frames on an otherwise idle server, on a host with
/// four cores or more.
///
/// "Extra" is end to end at the HTTP boundary, so it includes the wait for a
/// permit and the slowdown from sharing cores, memory and disk. The opt-in
/// measurement `integration::thumbnail_timing` reports it and, in a release
/// build on a host with at least four cores, fails above this value. On a
/// one-core host the bound is one background decode that had already
/// started; see [`ONE_CORE_IDLE_WINDOW`].
pub const INTERACTIVE_LATENCY_TARGET: Duration = Duration::from_millis(100);

/// On a one-core host no permit can be reserved for interactive work, so a
/// background decode starts only when no interactive request has asked for a
/// permit for this long.
pub const ONE_CORE_IDLE_WINDOW: Duration = Duration::from_secs(1);

/// The most permits background work may hold at once out of `permits`:
/// half of them, at least one, and never all of them when there is more than
/// one, so interactive work always has a permit it does not share.
pub fn background_limit(permits: usize) -> usize {
    match permits {
        0 | 1 => 1,
        permits => (permits / 2).max(1).min(permits - 1),
    }
}

/// Grants decode permits by class.
///
/// With `permits` permits in total:
///
/// - An interactive request is granted as soon as a permit is free.
///   Interactive requests are granted in arrival order.
/// - A background request is granted when a permit is free, background work
///   holds fewer than [`background_limit`] permits, and no interactive
///   request is waiting. Background requests are granted in arrival order.
/// - With one permit, a background request additionally waits until
///   [`ONE_CORE_IDLE_WINDOW`] has passed since the last interactive request
///   arrived (called [`Self::acquire`]); a later interactive arrival starts
///   the window again. Without any interactive request so far there is
///   nothing to wait for. The window is measured with `tokio::time`.
/// - When a permit is released, waiting interactive requests are considered
///   before waiting background ones.
/// - A request that stops waiting (its future is dropped before it is
///   granted) leaves the queue: it takes no permit, and it no longer counts
///   as a waiting interactive request.
pub struct DecodeScheduler {
    permits: usize,
}

/// One running decode's share of the pool, returned to the scheduler when
/// dropped. It owns its scheduler, so it can move into the task that runs
/// the decode.
#[must_use = "a decode permit is released when dropped"]
pub struct DecodePermit {
    _scheduler: Arc<DecodeScheduler>,
}

impl DecodeScheduler {
    /// A scheduler with `permits` permits (at least one).
    pub fn new(permits: usize) -> Arc<Self> {
        Arc::new(Self {
            permits: permits.max(1),
        })
    }

    /// The total number of permits.
    pub fn permits(&self) -> usize {
        self.permits
    }

    /// Waits for a permit of `class`, by the rules on [`DecodeScheduler`].
    ///
    /// Cancel safe: dropping the future before it resolves gives up the
    /// place in the queue and takes no permit. A thumbnail request awaits
    /// this inside its request future, so a client that aborts before the
    /// grant causes no decode at all.
    pub async fn acquire(self: &Arc<Self>, class: DecodeClass) -> DecodePermit {
        let _ = class;
        todo!("GAL1: grant decode permits by class")
    }
}

/// The process's scheduler, sized to the host's available parallelism (four
/// when that is unknown). Every decode of the pixel service takes its permit
/// here; it replaces the first-come-first-served `DECODE_PERMITS` semaphore.
pub fn decode_scheduler() -> &'static Arc<DecodeScheduler> {
    static SCHEDULER: std::sync::LazyLock<Arc<DecodeScheduler>> = std::sync::LazyLock::new(|| {
        DecodeScheduler::new(std::thread::available_parallelism().map_or(4, |cores| cores.get()))
    });
    &SCHEDULER
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::poll;
    use std::future::Future;
    use std::pin::{pin, Pin};
    use std::task::Poll;

    /// Lets wake-ups and timers that are already due run.
    async fn settle() {
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }

    async fn granted(waiter: &mut Pin<&mut impl Future<Output = DecodePermit>>) -> bool {
        settle().await;
        poll!(waiter.as_mut()).is_ready()
    }

    async fn take(waiter: &mut Pin<&mut impl Future<Output = DecodePermit>>) -> DecodePermit {
        settle().await;
        match poll!(waiter.as_mut()) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("the permit was not granted"),
        }
    }

    #[test]
    fn background_work_never_takes_every_permit_of_a_multi_core_host() {
        for (permits, limit) in [(1, 1), (2, 1), (3, 1), (4, 2), (8, 4), (10, 5)] {
            assert_eq!(background_limit(permits), limit, "{permits} permits");
        }
    }

    #[tokio::test]
    async fn background_decodes_are_capped_and_yield_to_interactive_ones() {
        use DecodeClass::{Background, Interactive};
        let scheduler = DecodeScheduler::new(4);

        // Background work holds at most half of four permits.
        let background_1 = scheduler.acquire(Background).await;
        let _background_2 = scheduler.acquire(Background).await;
        let mut background_3 = pin!(scheduler.acquire(Background));
        assert!(!granted(&mut background_3).await, "a third background");

        // The other half stays free for interactive work.
        let interactive_1 = scheduler.acquire(Interactive).await;
        let interactive_2 = scheduler.acquire(Interactive).await;

        // A background permit comes back: the waiting background takes it.
        drop(background_1);
        let _background_3 = take(&mut background_3).await;

        // The pool is full. A background request queues first, then an
        // interactive one; the next free permit is the interactive one's.
        drop(_background_2);
        let interactive_3 = scheduler.acquire(Interactive).await;
        let mut background_4 = pin!(scheduler.acquire(Background));
        let mut interactive_4 = pin!(scheduler.acquire(Interactive));
        assert!(!granted(&mut background_4).await);
        assert!(!granted(&mut interactive_4).await);
        drop(interactive_1);
        let interactive_4 = take(&mut interactive_4).await;
        assert!(
            !granted(&mut background_4).await,
            "background overtook a waiting interactive request"
        );

        // With nobody interactive waiting, the next permit is background's.
        drop(interactive_2);
        let _background_4 = take(&mut background_4).await;
        drop((interactive_3, interactive_4));
    }

    #[tokio::test]
    async fn a_request_that_stops_waiting_leaves_the_queue() {
        use DecodeClass::{Background, Interactive};
        let scheduler = DecodeScheduler::new(2);
        let held = scheduler.acquire(Interactive).await;
        let _also_held = scheduler.acquire(Interactive).await;

        {
            let mut abandoned = pin!(scheduler.acquire(Interactive));
            assert!(!granted(&mut abandoned).await);
        }
        let mut background = pin!(scheduler.acquire(Background));
        assert!(!granted(&mut background).await);

        // The abandoned request neither takes the permit nor keeps
        // background work waiting behind it.
        drop(held);
        let _background = take(&mut background).await;

        let mut abandoned = Box::pin(scheduler.acquire(Background));
        assert!(!granted(&mut abandoned.as_mut()).await);
        drop(abandoned);
        drop(_background);
        let _interactive = scheduler.acquire(Interactive).await;
    }

    #[tokio::test(start_paused = true)]
    async fn on_one_core_background_decodes_wait_for_the_viewer_to_go_idle() {
        use DecodeClass::{Background, Interactive};
        let scheduler = DecodeScheduler::new(1);
        let just_short = ONE_CORE_IDLE_WINDOW - Duration::from_millis(1);

        // Nothing interactive has happened: background work starts at once.
        drop(scheduler.acquire(Background).await);

        drop(scheduler.acquire(Interactive).await);
        let mut background = pin!(scheduler.acquire(Background));
        assert!(!granted(&mut background).await);
        tokio::time::advance(just_short).await;
        assert!(!granted(&mut background).await);

        // Another interactive request starts the idle window again.
        drop(scheduler.acquire(Interactive).await);
        tokio::time::advance(just_short).await;
        assert!(!granted(&mut background).await);
        tokio::time::advance(Duration::from_millis(1)).await;
        let _background = take(&mut background).await;
    }
}
