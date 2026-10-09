//! The heap the whole process holds, counted at the allocator, so a test can
//! state how much memory a decode may take instead of how long it may run.
//!
//! The counts are for every thread, because the decoders hand their work to
//! blocking threads. A measurement therefore holds [`alone`], and every test
//! of this binary takes it for its whole body.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Bytes allocated and not yet freed.
static HELD: AtomicI64 = AtomicI64::new(0);
/// The highest `HELD` since the last reset.
static PEAK: AtomicI64 = AtomicI64::new(0);
static ALONE: Mutex<()> = Mutex::new(());

pub struct CountingAllocator;

fn charge(bytes: i64) {
    let now = HELD.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

// SAFETY: every request is passed to the system allocator unchanged; the
// counters are plain atomics and never allocate.
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

/// Held by a test for as long as it runs, so that no other test of this
/// binary allocates during its measurements.
pub fn alone() -> MutexGuard<'static, ()> {
    ALONE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `call` and returns its result with the most bytes the process held
/// at once during it, over what it held when the call began.
pub fn peak_during<T>(call: impl FnOnce() -> T) -> (T, u64) {
    let start = HELD.load(Ordering::Relaxed);
    PEAK.store(start, Ordering::Relaxed);
    let result = call();
    let peak = PEAK.load(Ordering::Relaxed);
    (result, (peak - start).max(0) as u64)
}
