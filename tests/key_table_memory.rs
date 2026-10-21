//! What the key table costs in memory for the files of an ordinary scan.
//!
//! Nearly every file a viewer loads is a DICOM file whose SOP Instance UID
//! and path no other loaded file has. `KeyTable` promises that such a file
//! costs a fixed number of bytes whatever the length of its UID and path,
//! and no allocation of its own (`src/keys/table.rs`, "Cost"): a session
//! over 100,000 files must not carry a second copy of every path and UID,
//! or a `sop:` key string per file, for keys that are never asked for.
//!
//! This is its own test binary because it replaces the global allocator
//! with one that counts. The counts are kept per thread, so the test
//! harness's other threads do not show in them, and they are exact: the
//! assertions are bounds on bytes and blocks, not on time.

use dcmview::keys::{FileIdentity, KeyRef, KeyTable};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;

struct Counting;

thread_local! {
    /// Bytes this thread has allocated and not freed.
    static LIVE_BYTES: Cell<isize> = const { Cell::new(0) };
    /// Blocks this thread has allocated and not freed.
    static LIVE_BLOCKS: Cell<isize> = const { Cell::new(0) };
}

fn count(bytes: isize, blocks: isize) {
    // `try_with`: a thread that is being torn down still allocates.
    let _ = LIVE_BYTES.try_with(|live| live.set(live.get() + bytes));
    let _ = LIVE_BLOCKS.try_with(|live| live.set(live.get() + blocks));
}

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counters are plain thread-local cells that allocate nothing.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size() as isize, 1);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count(-(layout.size() as isize), -1);
        // SAFETY: `pointer` came from `System` with this layout.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size() as isize, 1);
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size as isize - layout.size() as isize, 0);
        // SAFETY: `pointer` came from `System` with this layout.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What this thread holds now, as `(bytes, blocks)`.
fn live() -> (isize, isize) {
    (LIVE_BYTES.with(Cell::get), LIVE_BLOCKS.with(Cell::get))
}

/// The most bytes the table may hold per file whose UID and path are its
/// own, at any number of files, when a clone of the registered value is one
/// pointer. The table's vectors and hash maps double as they grow, so the
/// figure at a given count swings between about 95 and 125 bytes; the
/// bound is above the top of that swing and far below one copy of a path.
const BYTES_PER_FILE: isize = 160;

/// The most blocks the table may hold however many such files it has: its
/// vectors and maps, and nothing per file.
const BLOCKS: isize = 16;

/// A UID and a path of realistic length: about 60 and 90 bytes. A table
/// that copied either would be over the bound from that alone.
fn ordinary_file(number: usize) -> Arc<FileIdentity> {
    Arc::new(FileIdentity {
        sop_instance_uid: format!("1.2.826.0.1.3680043.10.511.20240131.143000.{number:06}.1.2.3.4"),
        size_bytes: 524_288 + (number % 7) as u64,
        path: PathBuf::from(format!(
            "/data/cohorts/screening-2024/patient_{:05}/study_01/series_02/image_{number:06}.dcm",
            number / 200
        )),
    })
}

#[test]
fn an_ordinary_file_costs_the_key_table_a_fixed_small_amount_and_no_block() {
    // Counts at which the table's storage has just grown and counts at
    // which it is about to: the bound holds at each, not on average.
    for files in [1_000_usize, 8_193, 10_000, 14_400, 16_384, 30_000] {
        let identities = (0..files).map(ordinary_file).collect::<Vec<_>>();
        let before = live();
        let mut table = KeyTable::new();
        for identity in &identities {
            // The result is dropped at once, as the catalog drops it.
            table.register(identity.clone());
        }
        let after = live();
        let (bytes, blocks) = (after.0 - before.0, after.1 - before.1);
        // Shown with `--nocapture`, for whoever wants the figure itself.
        println!(
            "{files} files: {bytes} bytes ({} per file) in {blocks} blocks",
            bytes / files as isize
        );
        assert!(
            bytes <= BYTES_PER_FILE * files as isize,
            "{files} files: the table holds {bytes} bytes, {} per file",
            bytes / files as isize
        );
        assert!(
            blocks <= BLOCKS,
            "{files} files: the table holds {blocks} blocks"
        );

        // Reading what a catalog entry and a frame header need of every
        // file builds nothing: the key is the UID the caller already holds.
        let before = live();
        let mut shown = 0;
        for (index, identity) in identities.iter().enumerate() {
            let view = table.view(index).expect("registered");
            assert_eq!(view.alias_of, None);
            if view.key == Some(KeyRef::Sop(&identity.sop_instance_uid)) {
                shown += 1;
            }
            assert!(!table.served(index));
        }
        assert_eq!(shown, files);
        assert_eq!(live(), before, "{files} files: reading a key allocated");

        // Serving a frame of each, which is what viewing does, adds nothing
        // either: none of these files is ever hashed.
        for index in 0..files {
            assert!(table.frame_sent(index).wanted.is_empty());
        }
        let after = live();
        assert!(
            after.0 - before.0 == 0 && after.1 - before.1 == 0,
            "{files} files: serving frames left {} bytes in {} blocks",
            after.0 - before.0,
            after.1 - before.1
        );
        drop(table);
    }
}
