//! The heap each test thread holds, counted at the allocator, so a test can
//! state how much memory a call may take instead of how long it may run.
//!
//! The counts are per thread: a call measured with [`peak_during`] is charged
//! what it allocates on the calling thread and nothing another test does
//! meanwhile. Code that hands its work to other threads is not measured.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Bytes allocated minus bytes freed on this thread. Negative when the
    /// thread frees what another allocated.
    static HELD: Cell<i64> = const { Cell::new(0) };
    /// The highest `HELD` since the last reset.
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

pub struct CountingAllocator;

fn charge(bytes: i64) {
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
