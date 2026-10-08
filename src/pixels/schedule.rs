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
//!
//! A permit also carries the decode's share of the **decode memory budget**
//! (`docs/design/image-formats.md` section 2.3, "Byte-based decode
//! admission"): the bytes the decode may hold while it runs, estimated from
//! the catalog entry by `admission::decode_estimate` before anything is
//! read. Permit and bytes are granted together and returned together, so
//! the bytes reserved are always the bytes of decodes that are running, and
//! their sum never exceeds the budget. [`DecodeScheduler::admit`] states the
//! rules; [`DecodeLimits`] holds the numbers.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

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

/// The decode memory budget when `--decode-memory` is not given: 4 GiB.
///
/// It bounds what running decodes hold, not the process: the frame caches
/// (`--cache-budget`) and the bodies of responses being sent are beside it.
pub const DECODE_MEMORY_DEFAULT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The smallest decode memory budget `--decode-memory` accepts: 256 MiB.
/// Below it frames of ordinary size are refused, and the answer to that is
/// a larger budget, not a smaller one.
pub const DECODE_MEMORY_MIN_BYTES: u64 = 256 * 1024 * 1024;

/// The most interactive requests that may wait for a permit at once. The
/// next is refused with [`DecodeRefusal::Busy`].
///
/// A waiting request holds no decode memory, so this bounds the tasks and
/// the delay behind a full budget, not bytes. It is far above what one
/// viewer produces (a drag along a long stack leaves a few hundred decodes
/// waiting), so a viewer only meets the refusal when the budget is held by
/// large frames and requests keep arriving.
pub const DECODE_QUEUE_INTERACTIVE: usize = 1024;

/// The most background requests that may wait for a permit at once: the
/// same number as [`DECODE_QUEUE_INTERACTIVE`]. The next is refused with
/// [`DecodeRefusal::Busy`].
///
/// A gallery asks for every tile it shows at once and abandons the ones it
/// scrolls past, so a folder of several hundred images puts that many
/// thumbnails in the queue in one step. They hold no decode memory while
/// they wait, and the viewer's frames do not wait behind them.
pub const DECODE_QUEUE_BACKGROUND: usize = 1024;

/// The share of a decode memory budget of `memory_bytes` that background
/// decodes may reserve between them: half, rounded down. The other half is
/// never reserved by a thumbnail, so the viewer always has it.
pub fn background_memory_limit(memory_bytes: u64) -> u64 {
    memory_bytes / 2
}

/// What bounds the decodes of one [`DecodeScheduler`] beside its permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    /// The decode memory budget: the most bytes the running decodes may
    /// have reserved between them.
    pub memory_bytes: u64,
    /// The most interactive requests that may wait at once.
    pub interactive_queue: usize,
    /// The most background requests that may wait at once.
    pub background_queue: usize,
}

impl DecodeLimits {
    /// What a viewer started without `--decode-memory` runs with.
    pub const DEFAULT: Self = Self {
        memory_bytes: DECODE_MEMORY_DEFAULT_BYTES,
        interactive_queue: DECODE_QUEUE_INTERACTIVE,
        background_queue: DECODE_QUEUE_BACKGROUND,
    };

    /// No memory budget and no queue limit: decodes are bounded by the
    /// permits alone. [`DecodeScheduler::new`] builds this; the viewer never
    /// runs with it.
    pub const UNLIMITED: Self = Self {
        memory_bytes: u64::MAX,
        interactive_queue: usize::MAX,
        background_queue: usize::MAX,
    };

    /// [`Self::DEFAULT`] with a budget of `memory_bytes` (`--decode-memory`).
    /// A budget below [`DECODE_MEMORY_MIN_BYTES`] is an error that names the
    /// minimum.
    pub fn with_memory(memory_bytes: u64) -> Result<Self, String> {
        if memory_bytes < DECODE_MEMORY_MIN_BYTES {
            return Err(format!(
                "decode memory must be at least {DECODE_MEMORY_MIN_BYTES} bytes (256MiB)"
            ));
        }
        Ok(Self {
            memory_bytes,
            ..Self::DEFAULT
        })
    }
}

/// Why [`DecodeScheduler::admit`] did not grant a permit. Neither is a
/// property of the file: the catalog's `support_state` never depends on the
/// budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeRefusal {
    /// The request needs more bytes than its class may ever reserve, so
    /// waiting cannot help. `limit_bytes` is the whole budget for an
    /// interactive request and [`background_memory_limit`] of it for a
    /// background one.
    TooLarge {
        needed_bytes: u64,
        limit_bytes: u64,
        class: DecodeClass,
    },
    /// The request would have to wait, and its class's queue is full.
    Busy,
}

/// What a scheduler is doing now, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecodeLoad {
    /// Permits held.
    pub running: usize,
    /// Permits held by background decodes.
    pub background_running: usize,
    /// Bytes reserved by the permits held.
    pub reserved_bytes: u64,
    /// Bytes reserved by the background permits held.
    pub background_reserved_bytes: u64,
    /// Interactive requests waiting.
    pub waiting_interactive: usize,
    /// Background requests waiting.
    pub waiting_background: usize,
    /// The highest `reserved_bytes` has been since the scheduler was built.
    pub peak_reserved_bytes: u64,
}

/// The most permits background work may hold at once out of `permits`:
/// half of them, at least one, and never all of them when there is more than
/// one, so interactive work always has a permit it does not share.
pub fn background_limit(permits: usize) -> usize {
    match permits {
        0 | 1 => 1,
        permits => (permits / 2).max(1).min(permits - 1),
    }
}

/// Grants decode permits by class and by memory.
///
/// [`Self::admit`] is what the pixel service calls and states the rules for
/// memory. The rules for permits, which [`Self::acquire`] applies alone, are
/// these. With `permits` permits in total:
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
    limits: DecodeLimits,
    state: Mutex<ScheduleState>,
    changed: Notify,
}

/// One running decode's share of the pool and of the decode memory budget,
/// both returned to the scheduler when it is dropped: when the decode ends,
/// when it panics, and when the task that holds it is aborted. It owns its
/// scheduler, so it can move into the task that runs the decode.
#[must_use = "a decode permit is released when dropped"]
pub struct DecodePermit {
    scheduler: Arc<DecodeScheduler>,
    class: DecodeClass,
    /// The bytes this permit reserved; 0 for one from [`DecodeScheduler::acquire`].
    bytes: u64,
}

impl DecodePermit {
    /// The bytes of the decode memory budget this permit holds.
    pub fn reserved_bytes(&self) -> u64 {
        self.bytes
    }
}

impl DecodeScheduler {
    /// A scheduler with `permits` permits (at least one) and
    /// [`DecodeLimits::UNLIMITED`]: no memory budget.
    pub fn new(permits: usize) -> Arc<Self> {
        Self::with_limits(permits, DecodeLimits::UNLIMITED)
    }

    /// A scheduler with `permits` permits (at least one) that admits by
    /// `limits`.
    pub fn with_limits(permits: usize, limits: DecodeLimits) -> Arc<Self> {
        Arc::new(Self {
            permits: permits.max(1),
            limits,
            state: Mutex::new(ScheduleState::default()),
            changed: Notify::new(),
        })
    }

    /// A new scheduler for one viewer in this process: one permit per core
    /// the host makes available (four when that is unknown) and
    /// [`DecodeLimits::DEFAULT`]. `AppState::new` builds its own with this,
    /// so two viewers in one process (two tests) do not share a budget.
    pub fn for_host() -> Arc<Self> {
        Self::with_limits(host_permits(), DecodeLimits::DEFAULT)
    }

    /// The total number of permits.
    pub fn permits(&self) -> usize {
        self.permits
    }

    /// The limits this scheduler admits by.
    pub fn limits(&self) -> DecodeLimits {
        self.limits
    }

    /// What the scheduler is doing now.
    pub fn load(&self) -> DecodeLoad {
        let state = self.state.lock().expect("decode scheduler lock poisoned");
        DecodeLoad {
            running: state.running,
            background_running: state.background_running,
            reserved_bytes: state.reserved_bytes,
            background_reserved_bytes: state.background_reserved_bytes,
            waiting_interactive: state.interactive.len(),
            waiting_background: state.background.len(),
            peak_reserved_bytes: state.peak_reserved_bytes,
        }
    }

    /// Waits until [`Self::load`] satisfies `reached` and returns that load.
    /// Every change to what `load` reports wakes it, so a test can wait for
    /// a request to start waiting or for a decode to end without sleeping.
    pub async fn load_when(&self, reached: impl Fn(&DecodeLoad) -> bool) -> DecodeLoad {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let load = self.load();
            if reached(&load) {
                return load;
            }
            changed.await;
        }
    }

    /// Waits for a permit of `class` that reserves `bytes` of the decode
    /// memory budget, or refuses at once. This is the only way the pixel
    /// service starts a decode.
    ///
    /// With [`DecodeLimits::UNLIMITED`] it is [`Self::acquire`]: nothing is
    /// reserved and nothing is refused. Otherwise, with `limits` and
    /// `budget = limits.memory_bytes`:
    ///
    /// **Refused at once, without waiting** (checked in this order, when the
    /// call is first polled):
    ///
    /// 1. [`DecodeRefusal::TooLarge`] when `bytes` exceeds what the class
    ///    may ever reserve: `budget` for an interactive request,
    ///    [`background_memory_limit`]`(budget)` for a background one. The
    ///    refusal carries `bytes` and that limit.
    /// 2. [`DecodeRefusal::Busy`] when the request cannot be granted now
    ///    and its class already has `limits.interactive_queue` (or
    ///    `limits.background_queue`) requests waiting. A request that can be
    ///    granted now is granted whatever the queue limit, 0 included.
    ///    Requests waiting in [`Self::acquire`] count as waiting.
    ///
    /// **Granted** when all of these hold; until then the request waits in
    /// its class's queue:
    ///
    /// - it is at the front of its class's queue (arrival order within a
    ///   class; a large request at the front is not overtaken by smaller
    ///   ones behind it, so it cannot starve);
    /// - a permit is free, by the rules on [`DecodeScheduler`], the
    ///   background cap and the one-core idle window included;
    /// - `reserved + bytes <= budget`, where `reserved` is the sum of the
    ///   bytes of the permits now held;
    /// - for a background request, also `background_reserved + bytes <=
    ///   background_memory_limit(budget)`, and no interactive request is
    ///   waiting (for a permit or for bytes).
    ///
    /// The permit and the bytes are taken in one step under the scheduler's
    /// lock and returned in one step when the [`DecodePermit`] is dropped,
    /// so `load().reserved_bytes` never exceeds `budget`, and
    /// `background_reserved_bytes` never exceeds its share, at any instant.
    /// `peak_reserved_bytes` is raised in the same step. Sums that could
    /// overflow saturate.
    ///
    /// Every grant, release, arrival in a queue and departure from one
    /// wakes the waiters ([`Self::load_when`] included). When a permit is
    /// released, waiting interactive requests are considered before
    /// waiting background ones.
    ///
    /// Cancel safe, as [`Self::acquire`]: dropping the future before it
    /// resolves leaves the queue, reserves nothing and takes no permit. A
    /// request of zero bytes reserves nothing and is otherwise admitted by
    /// the same rules.
    pub async fn admit(
        self: &Arc<Self>,
        class: DecodeClass,
        bytes: u64,
    ) -> Result<DecodePermit, DecodeRefusal> {
        if self.limits == DecodeLimits::UNLIMITED {
            return Ok(self.acquire(class).await);
        }
        self.wait_for_permit(class, bytes, true).await
    }

    /// Waits for a permit of `class`, by the rules on [`DecodeScheduler`].
    ///
    /// Cancel safe: dropping the future before it resolves gives up the
    /// place in the queue and takes no permit. A thumbnail request awaits
    /// this inside its request future, so a client that aborts before the
    /// grant causes no decode at all.
    pub async fn acquire(self: &Arc<Self>, class: DecodeClass) -> DecodePermit {
        self.wait_for_permit(class, 0, false)
            .await
            .expect("unlimited acquisition cannot be refused")
    }

    async fn wait_for_permit(
        self: &Arc<Self>,
        class: DecodeClass,
        bytes: u64,
        limited: bool,
    ) -> Result<DecodePermit, DecodeRefusal> {
        let id = {
            let mut state = self.state.lock().expect("decode scheduler lock poisoned");
            let limit_bytes = match class {
                DecodeClass::Interactive => self.limits.memory_bytes,
                DecodeClass::Background => background_memory_limit(self.limits.memory_bytes),
            };
            if limited && bytes > limit_bytes {
                return Err(DecodeRefusal::TooLarge {
                    needed_bytes: bytes,
                    limit_bytes,
                    class,
                });
            }
            if class == DecodeClass::Interactive {
                state.last_interactive = Some(Instant::now());
            }
            if state.queue(class).is_empty() && self.can_grant(&state, class, bytes, limited) {
                return Ok(self.grant(&mut state, class, bytes));
            }
            let queue_limit = match class {
                DecodeClass::Interactive => self.limits.interactive_queue,
                DecodeClass::Background => self.limits.background_queue,
            };
            if limited && state.queue(class).len() >= queue_limit {
                return Err(DecodeRefusal::Busy);
            }
            let id = state.next_id;
            state.next_id += 1;
            state.queue(class).push_back(id);
            id
        };
        let mut waiting = WaitingDecode {
            scheduler: self,
            class,
            id,
            queued: true,
        };
        self.changed.notify_waiters();
        loop {
            // Register before checking the state, so a release between the
            // check and the await cannot be lost.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let idle_until = {
                let mut state = self.state.lock().expect("decode scheduler lock poisoned");
                if state.queue(class).front() == Some(&id)
                    && self.can_grant(&state, class, bytes, limited)
                {
                    state.queue(class).pop_front();
                    waiting.queued = false;
                    return Ok(self.grant(&mut state, class, bytes));
                }
                self.idle_until(&state, class)
            };
            if let Some(deadline) = idle_until {
                tokio::select! {
                    _ = changed => {},
                    _ = tokio::time::sleep_until(deadline) => {},
                }
            } else {
                changed.await;
            }
        }
    }

    fn idle_until(&self, state: &ScheduleState, class: DecodeClass) -> Option<Instant> {
        (self.permits == 1 && class == DecodeClass::Background)
            .then_some(state.last_interactive)
            .flatten()
            .map(|last| last + ONE_CORE_IDLE_WINDOW)
            .filter(|deadline| *deadline > Instant::now())
    }

    // Queue order is checked by the caller, for both arrivals and waiters.
    fn can_grant(
        &self,
        state: &ScheduleState,
        class: DecodeClass,
        bytes: u64,
        limited: bool,
    ) -> bool {
        state.running < self.permits
            && (!limited || state.reserved_bytes.saturating_add(bytes) <= self.limits.memory_bytes)
            && (class == DecodeClass::Interactive
                || (state.interactive.is_empty()
                    && state.background_running < background_limit(self.permits)
                    && self.idle_until(state, class).is_none()
                    && (!limited
                        || state.background_reserved_bytes.saturating_add(bytes)
                            <= background_memory_limit(self.limits.memory_bytes))))
    }

    fn grant(
        self: &Arc<Self>,
        state: &mut ScheduleState,
        class: DecodeClass,
        bytes: u64,
    ) -> DecodePermit {
        state.running += 1;
        state.reserved_bytes = state.reserved_bytes.saturating_add(bytes);
        state.peak_reserved_bytes = state.peak_reserved_bytes.max(state.reserved_bytes);
        if class == DecodeClass::Background {
            state.background_running += 1;
            state.background_reserved_bytes = state.background_reserved_bytes.saturating_add(bytes);
        }
        // A grant also lets the next waiter use another free permit.
        self.changed.notify_waiters();
        DecodePermit {
            scheduler: Arc::clone(self),
            class,
            bytes,
        }
    }
}

#[derive(Default)]
struct ScheduleState {
    running: usize,
    background_running: usize,
    /// The sum of the bytes of the permits held, and of the background ones.
    reserved_bytes: u64,
    background_reserved_bytes: u64,
    peak_reserved_bytes: u64,
    next_id: u64,
    interactive: VecDeque<u64>,
    background: VecDeque<u64>,
    last_interactive: Option<Instant>,
}

impl ScheduleState {
    fn queue(&mut self, class: DecodeClass) -> &mut VecDeque<u64> {
        match class {
            DecodeClass::Interactive => &mut self.interactive,
            DecodeClass::Background => &mut self.background,
        }
    }
}

// Queue membership belongs to the acquire future, not a detached task.
// Grants happen on polling that future, with no suspension before returning
// the owning permit; cancellation therefore cannot strand a granted slot.
struct WaitingDecode<'a> {
    scheduler: &'a DecodeScheduler,
    class: DecodeClass,
    id: u64,
    queued: bool,
}

impl Drop for WaitingDecode<'_> {
    fn drop(&mut self) {
        if self.queued {
            self.scheduler
                .state
                .lock()
                .expect("decode scheduler lock poisoned")
                .queue(self.class)
                .retain(|id| *id != self.id);
            self.scheduler.changed.notify_waiters();
        }
    }
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        let mut state = self
            .scheduler
            .state
            .lock()
            .expect("decode scheduler lock poisoned");
        state.running -= 1;
        state.reserved_bytes = state.reserved_bytes.saturating_sub(self.bytes);
        if self.class == DecodeClass::Background {
            state.background_running -= 1;
            state.background_reserved_bytes =
                state.background_reserved_bytes.saturating_sub(self.bytes);
        }
        drop(state);
        self.scheduler.changed.notify_waiters();
    }
}

/// One permit per core the host makes available; four when that is unknown.
pub fn host_permits() -> usize {
    std::thread::available_parallelism().map_or(4, |cores| cores.get())
}

/// A scheduler shared by everything in the process that was not given one:
/// the caches `pixels::new_cache` and its siblings build. It is
/// [`DecodeScheduler::for_host`], built once. A viewer's `AppState` has its
/// own.
pub fn decode_scheduler() -> &'static Arc<DecodeScheduler> {
    static SCHEDULER: std::sync::LazyLock<Arc<DecodeScheduler>> =
        std::sync::LazyLock::new(DecodeScheduler::for_host);
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
