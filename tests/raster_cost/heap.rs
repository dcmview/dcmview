//! The heap each test thread holds, counted at the allocator, so a test can
//! state how much memory a call may take instead of how long it may run.
//!
//! The counts are per thread: a call measured with [`peak_during`] is charged
//! what it allocates on the calling thread and nothing another test does
//! meanwhile. Code that hands its work to other threads is not measured.
//!
//! Work that does run on other threads (the pixel service decodes on the
//! blocking pool) is measured on a [`CountedRuntime`]: every thread of that
//! runtime adds to one shared count, and no thread of another test does.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;
use std::sync::atomic::{AtomicI64, Ordering};

thread_local! {
    /// Bytes allocated minus bytes freed on this thread. Negative when the
    /// thread frees what another allocated.
    static HELD: Cell<i64> = const { Cell::new(0) };
    /// The highest `HELD` since the last reset.
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

/// What the threads of one [`CountedRuntime`] hold between them.
#[derive(Default)]
struct Shared {
    held: AtomicI64,
    peak: AtomicI64,
}

thread_local! {
    /// The count this thread also adds to, when it belongs to a
    /// [`CountedRuntime`] or is driving one.
    static SHARED: Cell<Option<&'static Shared>> = const { Cell::new(None) };
}

pub struct CountingAllocator;

fn charge(bytes: i64) {
    if let Ok(Some(shared)) = SHARED.try_with(Cell::get) {
        let now = shared.held.fetch_add(bytes, Ordering::SeqCst) + bytes;
        shared.peak.fetch_max(now, Ordering::SeqCst);
    }
    // A thread that is shutting down has no counters left; it is not one a
    // measurement runs on.
    let _ = HELD.try_with(|held| {
        let now = held.get() + bytes;
        held.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

// SAFETY: every request is passed to the system allocator unchanged; the
// counters are plain thread-local integers and never allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            charge(layout.size() as i64);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc_zeroed(layout);
        if !pointer.is_null() {
            charge(layout.size() as i64);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
        charge(-(layout.size() as i64));
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let grown = System.realloc(pointer, layout, new_size);
        if !grown.is_null() {
            // Both blocks can exist while the contents move.
            charge(new_size as i64);
            charge(-(layout.size() as i64));
        }
        grown
    }
}

/// Runs `call` and returns its result with the most bytes the calling thread
/// held at once during it, over what it held when the call began.
pub fn peak_during<T>(call: impl FnOnce() -> T) -> (T, u64) {
    let start = HELD.with(Cell::get);
    PEAK.with(|peak| peak.set(start));
    let result = call();
    let peak = PEAK.with(Cell::get);
    (result, (peak - start).max(0) as u64)
}

/// What the calling thread has allocated less what it has freed, so far.
/// Memory another thread allocated and this one frees lowers it: the
/// difference across a `drop` is what the drop gave back.
pub fn held() -> i64 {
    HELD.with(Cell::get)
}

/// A runtime whose threads (its workers and its blocking pool) are counted
/// together, so the heap of work the pixel service spreads over tasks and
/// blocking threads can be measured whole, apart from whatever other tests
/// allocate meanwhile.
pub struct CountedRuntime {
    runtime: tokio::runtime::Runtime,
    shared: &'static Shared,
}

impl CountedRuntime {
    pub fn new() -> Self {
        // One small count for the life of the test binary.
        let shared: &'static Shared = Box::leak(Box::default());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(64)
            .enable_all()
            .on_thread_start(move || SHARED.with(|slot| slot.set(Some(shared))))
            .build()
            .expect("counted runtime");
        Self { runtime, shared }
    }

    /// Runs `work` to completion on this runtime and returns its result
    /// with the most bytes the runtime's threads, and the calling thread
    /// while it drives them, held at once during it, over what they held
    /// when it began.
    ///
    /// `work` must free only what it allocated: memory from before the call
    /// that is freed during it lowers the count. Work it leaves running
    /// when it returns is charged to whatever is measured next.
    pub fn peak_during<F: Future>(&self, work: F) -> (F::Output, u64) {
        let outer = SHARED.with(|slot| slot.replace(Some(self.shared)));
        let start = self.shared.held.load(Ordering::SeqCst);
        self.shared.peak.store(start, Ordering::SeqCst);
        let result = self.runtime.block_on(work);
        let peak = self.shared.peak.load(Ordering::SeqCst);
        SHARED.with(|slot| slot.set(outer));
        (result, (peak - start).max(0) as u64)
    }
}
