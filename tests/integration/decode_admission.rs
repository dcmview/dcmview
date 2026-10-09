//! Decode admission: what running decodes may reserve between them, who
//! waits, who is refused, and that nothing a request does leaks a permit
//! (`docs/design/image-formats.md` section 2.3, "Byte-based decode
//! admission").
//!
//! Nothing here is timed. A test learns that a request is waiting, or that a
//! decode has ended, from the scheduler's own account of itself
//! (`DecodeScheduler::load_when`), and holds a decode in place by holding a
//! permit or, on Unix, by putting a pipe nobody writes to where the file was.
//! The one duration in this file is `eventually`'s, which turns a wait that
//! would never end into a failure.

use super::raster_discovery::scan_dir;
use super::raster_files as files;
use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::api::contracts::{endpoints, Endpoint, DECODE_BUSY_RETRY_AFTER_SECONDS};
use dcmview::loader::DiscoverOptions;
use dcmview::pixels::DecodeClass::{Background, Interactive};
use dcmview::pixels::{
    self, DecodeClass, DecodeLimits, DecodeLoad, DecodePermit, DecodeRefusal, DecodeScheduler,
    DecodeWork,
};
use dcmview::server::{self, FileRegistry};
use dcmview::types::FileEntry;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use futures::poll;
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;
use tokio::task::JoinHandle;

const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";

/// Fails a test that would otherwise never end, because a request is waiting
/// for a permit nothing will free. It bounds nothing the viewer does.
async fn finishes(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(120), test)
        .await
        .expect("the test never finished: something is still waiting for a permit");
}

/// Fails a test whose wait would never end. It bounds nothing the viewer
/// does: every wait it wraps ends as soon as the scheduler changes.
async fn eventually<T>(what: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .unwrap_or_else(|_| panic!("{what}: never happened"))
}

fn limits(memory_bytes: u64, interactive_queue: usize, background_queue: usize) -> DecodeLimits {
    DecodeLimits {
        memory_bytes,
        interactive_queue,
        background_queue,
    }
}

type Admission = Result<DecodePermit, DecodeRefusal>;

/// A request for a permit, running on its own so the test can watch it wait.
fn request(
    scheduler: &Arc<DecodeScheduler>,
    class: DecodeClass,
    bytes: u64,
) -> JoinHandle<Admission> {
    let scheduler = scheduler.clone();
    tokio::spawn(async move { scheduler.admit(class, bytes).await })
}

async fn granted(scheduler: &Arc<DecodeScheduler>, class: DecodeClass, bytes: u64) -> DecodePermit {
    eventually("a grant", scheduler.admit(class, bytes))
        .await
        .unwrap_or_else(|refusal| panic!("{bytes} bytes were refused: {refusal:?}"))
}

async fn waiting(
    scheduler: &Arc<DecodeScheduler>,
    interactive: usize,
    background: usize,
) -> DecodeLoad {
    eventually(
        "requests waiting",
        scheduler.load_when(|load| {
            load.waiting_interactive == interactive && load.waiting_background == background
        }),
    )
    .await
}

async fn idle(scheduler: &Arc<DecodeScheduler>) -> DecodeLoad {
    eventually(
        "an idle scheduler",
        scheduler.load_when(|load| {
            load.running == 0
                && load.reserved_bytes == 0
                && load.background_reserved_bytes == 0
                && load.waiting_interactive == 0
                && load.waiting_background == 0
        }),
    )
    .await
}

async fn joined(handle: JoinHandle<Admission>) -> DecodePermit {
    eventually("a waiting request's grant", handle)
        .await
        .expect("request task")
        .expect("granted")
}

// ---------------------------------------------------------------------------
// The scheduler's rules

/// The bytes reserved are never more than the budget: the request that
/// would pass it waits, and takes the bytes a finished decode returns.
#[tokio::test]
async fn the_bytes_reserved_never_exceed_the_budget_and_the_next_request_waits() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(8, limits(100, 8, 8));

        let first = granted(&scheduler, Interactive, 40).await;
        let second = granted(&scheduler, Interactive, 40).await;
        assert_eq!(first.reserved_bytes(), 40);
        let third = request(&scheduler, Interactive, 40);
        let load = waiting(&scheduler, 1, 0).await;
        assert_eq!((load.running, load.reserved_bytes), (2, 80));

        // A finished decode's bytes admit the next.
        drop(first);
        let third = joined(third).await;
        assert_eq!(scheduler.load().reserved_bytes, 80);

        // The budget can be reserved to its last byte, and not one further.
        let exact = granted(&scheduler, Interactive, 20).await;
        assert_eq!(scheduler.load().reserved_bytes, 100);
        let over = request(&scheduler, Interactive, 1);
        waiting(&scheduler, 1, 0).await;
        drop(second);
        let over = joined(over).await;
        assert_eq!(scheduler.load().reserved_bytes, 61);

        drop((third, exact, over));
        let load = idle(&scheduler).await;
        assert_eq!(load.peak_reserved_bytes, 100, "the most ever reserved");
    })
    .await;
}

/// A request for more than its class may ever reserve is refused at once,
/// with what it needed and what the limit is; the largest that fits is not.
#[tokio::test]
async fn a_request_that_waiting_cannot_satisfy_is_refused_at_once() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(4, limits(1000, 8, 8));
        assert_eq!(pixels::background_memory_limit(1000), 500);
        assert_eq!(pixels::background_memory_limit(1001), 500);

        for (class, bytes, limit) in [(Interactive, 1001, 1000), (Background, 501, 500)] {
            // Refused whether or not anything is running.
            for held in [0, 300] {
                let _held = granted(&scheduler, Interactive, held).await;
                let refusal = eventually("a refusal", scheduler.admit(class, bytes))
                    .await
                    .err();
                assert_eq!(
                    refusal,
                    Some(DecodeRefusal::TooLarge {
                        needed_bytes: bytes,
                        limit_bytes: limit,
                        class,
                    }),
                    "{class:?} asking for {bytes}"
                );
            }
            drop(granted(&scheduler, class, limit).await);
        }
        assert_eq!(idle(&scheduler).await.peak_reserved_bytes, 1000);

        // The flag's own floor.
        assert!(DecodeLimits::with_memory(pixels::DECODE_MEMORY_MIN_BYTES - 1).is_err());
        assert_eq!(
            DecodeLimits::with_memory(pixels::DECODE_MEMORY_MIN_BYTES),
            Ok(DecodeLimits {
                memory_bytes: pixels::DECODE_MEMORY_MIN_BYTES,
                ..DecodeLimits::DEFAULT
            })
        );
        assert_eq!(
            DecodeLimits::DEFAULT,
            limits(
                pixels::DECODE_MEMORY_DEFAULT_BYTES,
                pixels::DECODE_QUEUE_INTERACTIVE,
                pixels::DECODE_QUEUE_BACKGROUND
            )
        );
    })
    .await;
}

/// A class's queue holds its limit of waiting requests and the next is
/// refused as busy; a request that need not wait is granted whatever the
/// queue limit.
#[tokio::test]
async fn a_request_that_would_wait_behind_a_full_queue_is_refused_as_busy() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(4, limits(100, 2, 1));
        let held = granted(&scheduler, Interactive, 100).await;

        let first = request(&scheduler, Interactive, 10);
        waiting(&scheduler, 1, 0).await;
        let second = request(&scheduler, Interactive, 10);
        waiting(&scheduler, 2, 0).await;
        let background = request(&scheduler, Background, 10);
        waiting(&scheduler, 2, 1).await;
        for class in [Interactive, Background] {
            let refusal = eventually("a refusal", scheduler.admit(class, 10)).await;
            assert_eq!(refusal.err(), Some(DecodeRefusal::Busy), "{class:?}");
        }
        // A refusal leaves no trace.
        let load = scheduler.load();
        assert_eq!((load.waiting_interactive, load.waiting_background), (2, 1));
        assert_eq!(load.reserved_bytes, 100);

        drop(held);
        drop((
            joined(first).await,
            joined(second).await,
            joined(background).await,
        ));
        idle(&scheduler).await;

        // With no room to wait at all, what fits is still granted.
        let scheduler = DecodeScheduler::with_limits(4, limits(100, 0, 0));
        let interactive = granted(&scheduler, Interactive, 60).await;
        let background = granted(&scheduler, Background, 40).await;
        for class in [Interactive, Background] {
            let refusal = eventually("a refusal", scheduler.admit(class, 1)).await;
            assert_eq!(refusal.err(), Some(DecodeRefusal::Busy), "{class:?}");
        }
        drop((interactive, background));
        idle(&scheduler).await;

        // The viewer's own limits: a gallery that asks for a thousand
        // thumbnails at once is made to wait, not refused, and so is the
        // viewer; the request after the 1,024th waiting one is refused.
        assert_eq!(pixels::DECODE_QUEUE_INTERACTIVE, 1024);
        assert_eq!(pixels::DECODE_QUEUE_BACKGROUND, 1024);
        let scheduler = default_scheduler(1);
        let held = granted(&scheduler, Interactive, 0).await;
        let mut requests = Vec::new();
        for (class, queued) in [(Background, (0, 1024)), (Interactive, (1024, 1024))] {
            requests.extend((0..1024).map(|_| request(&scheduler, class, 1)));
            waiting(&scheduler, queued.0, queued.1).await;
            let refusal = eventually("a refusal", scheduler.admit(class, 1)).await;
            assert_eq!(refusal.err(), Some(DecodeRefusal::Busy), "{class:?}");
        }
        // Dropped while they wait, they all leave their queues.
        requests.iter().for_each(JoinHandle::abort);
        waiting(&scheduler, 0, 0).await;
        drop(held);
    })
    .await;
}

/// A request that stops waiting tells the scheduler's other waiters: the
/// request behind it is granted what it was in the way of, and whoever
/// watches the load sees the queue shorten.
#[tokio::test]
async fn a_request_that_stops_waiting_wakes_those_behind_it() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(4, limits(100, 8, 8));
        let held = granted(&scheduler, Interactive, 60).await;

        // 50 bytes do not fit beside 60, and 40 would but are behind them.
        let front = request(&scheduler, Interactive, 50);
        waiting(&scheduler, 1, 0).await;
        let behind = request(&scheduler, Interactive, 40);
        waiting(&scheduler, 2, 0).await;
        front.abort();
        let behind = joined(behind).await;
        assert_eq!(scheduler.load().reserved_bytes, 100);

        // With nothing to grant, the departure is still a change of load.
        let alone = request(&scheduler, Interactive, 50);
        waiting(&scheduler, 1, 0).await;
        let mut emptied = Box::pin(scheduler.load_when(|load| load.waiting_interactive == 0));
        assert!(poll!(&mut emptied).is_pending());
        alone.abort();
        let load = eventually("the queue to empty", emptied).await;
        assert_eq!((load.running, load.reserved_bytes), (2, 100));
        drop((held, behind));
        idle(&scheduler).await;
    })
    .await;
}

/// Requests that take a permit alone (`acquire`) and requests admitted by
/// bytes share the permits and the queues of a scheduler with limits: a
/// permit-only request reserves nothing, counts as waiting, and keeps its
/// place in arrival order.
#[tokio::test]
async fn permit_only_requests_share_the_permits_and_queues_of_admitted_ones() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(1, limits(100, 2, 2));
        let held = eventually("a permit", scheduler.acquire(Interactive)).await;
        assert_eq!(held.reserved_bytes(), 0);
        let load = scheduler.load();
        assert_eq!((load.running, load.reserved_bytes), (1, 0));

        let permit_only = tokio::spawn({
            let scheduler = scheduler.clone();
            async move { scheduler.acquire(Interactive).await }
        });
        waiting(&scheduler, 1, 0).await;
        let admitted = request(&scheduler, Interactive, 10);
        waiting(&scheduler, 2, 0).await;
        // The permit-only request fills the queue like any other.
        let refusal = eventually("a refusal", scheduler.admit(Interactive, 10)).await;
        assert_eq!(refusal.err(), Some(DecodeRefusal::Busy));

        // Arrival order across both kinds.
        drop(held);
        let permit_only = eventually("the permit-only request", permit_only)
            .await
            .expect("request task");
        let load = waiting(&scheduler, 1, 0).await;
        assert_eq!((load.running, load.reserved_bytes), (1, 0));
        drop(permit_only);
        let admitted = joined(admitted).await;
        assert_eq!(scheduler.load().reserved_bytes, 10);

        // A permit held with bytes is the permit a permit-only request waits for.
        let permit_only = tokio::spawn({
            let scheduler = scheduler.clone();
            async move { scheduler.acquire(Interactive).await }
        });
        waiting(&scheduler, 1, 0).await;
        drop(admitted);
        drop(
            eventually("the permit-only request", permit_only)
                .await
                .expect("request task"),
        );
        assert_eq!(idle(&scheduler).await.peak_reserved_bytes, 10);
    })
    .await;
}

/// A scheduler told to stop making requests wait (a viewer shutting down)
/// refuses everything that is waiting, and everything that would wait from
/// then on, as busy. What is running keeps its permit, and what fits beside
/// it is still granted.
#[tokio::test]
async fn a_scheduler_that_stops_making_requests_wait_refuses_them_as_busy() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(2, limits(100, 8, 8));
        let running = granted(&scheduler, Interactive, 60).await;
        let interactive = [
            request(&scheduler, Interactive, 50),
            request(&scheduler, Interactive, 10),
        ];
        waiting(&scheduler, 2, 0).await;
        let background = request(&scheduler, Background, 10);
        waiting(&scheduler, 2, 1).await;

        scheduler.refuse_waiting();
        for refused in interactive.into_iter().chain([background]) {
            let refusal = eventually("a refusal", refused)
                .await
                .expect("request task");
            assert_eq!(refusal.err(), Some(DecodeRefusal::Busy));
        }
        let load = waiting(&scheduler, 0, 0).await;
        assert_eq!((load.running, load.reserved_bytes), (1, 60));

        // What fits beside the running decode is granted; what would wait
        // is refused, and what could never fit says so as before.
        let beside = granted(&scheduler, Interactive, 40).await;
        for class in [Interactive, Background] {
            let refusal = eventually("a refusal", scheduler.admit(class, 1)).await;
            assert_eq!(refusal.err(), Some(DecodeRefusal::Busy), "{class:?}");
        }
        assert!(matches!(
            scheduler.admit(Interactive, 101).await,
            Err(DecodeRefusal::TooLarge { .. })
        ));
        drop((running, beside));
        idle(&scheduler).await;
    })
    .await;
}

/// Background decodes share half the budget between them and wait for it
/// while the other half is free; the viewer's half is never theirs.
#[tokio::test]
async fn background_decodes_cannot_reserve_the_share_kept_for_the_viewer() {
    finishes(async {
        // Eight permits: four for background work, so bytes are what binds.
        let scheduler = DecodeScheduler::with_limits(8, limits(100, 8, 8));

        let first = granted(&scheduler, Background, 30).await;
        let second = granted(&scheduler, Background, 20).await;
        let third = request(&scheduler, Background, 1);
        let load = waiting(&scheduler, 0, 1).await;
        assert_eq!(
            (load.reserved_bytes, load.background_reserved_bytes),
            (50, 50)
        );
        assert_eq!(load.background_running, 2, "permits were free");

        // The viewer's half is free and is granted at once, past the waiting
        // thumbnail.
        let viewer = granted(&scheduler, Interactive, 50).await;
        assert_eq!(scheduler.load().reserved_bytes, 100);
        assert_eq!(scheduler.load().waiting_background, 1);

        // A background decode's bytes go to the waiting background request.
        drop(second);
        let third = joined(third).await;
        assert_eq!(scheduler.load().background_reserved_bytes, 31);

        // While the viewer waits for bytes, background work is granted none,
        // though its own share has room.
        let waiting_viewer = request(&scheduler, Interactive, 40);
        waiting(&scheduler, 1, 0).await;
        let behind = request(&scheduler, Background, 5);
        let load = waiting(&scheduler, 1, 1).await;
        assert_eq!(load.reserved_bytes, 81);
        drop(first);
        let waiting_viewer = joined(waiting_viewer).await;
        let behind = joined(behind).await;

        drop((viewer, third, waiting_viewer, behind));
        let load = idle(&scheduler).await;
        assert_eq!(load.peak_reserved_bytes, 100);
    })
    .await;
}

/// Requests of a class are granted in arrival order: a large one at the
/// front is not overtaken by small ones that would fit, so it cannot
/// starve.
#[tokio::test]
async fn a_large_request_at_the_front_of_its_queue_is_not_overtaken() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(8, limits(100, 8, 8));
        let held = granted(&scheduler, Interactive, 60).await;

        let large = request(&scheduler, Interactive, 50);
        waiting(&scheduler, 1, 0).await;
        let small = request(&scheduler, Interactive, 10);
        let load = waiting(&scheduler, 2, 0).await;
        assert_eq!(load.reserved_bytes, 60, "ten bytes would have fitted");

        drop(held);
        let large = joined(large).await;
        let small = joined(small).await;
        assert_eq!(scheduler.load().reserved_bytes, 60);
        drop((large, small));
        idle(&scheduler).await;
    })
    .await;
}

/// The permits still bound how many decodes run, whatever bytes are free.
#[tokio::test]
async fn a_request_waits_for_a_permit_when_its_bytes_are_free() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(2, limits(100, 8, 8));
        let first = granted(&scheduler, Interactive, 1).await;
        let _second = granted(&scheduler, Interactive, 1).await;
        let third = request(&scheduler, Interactive, 1);
        let load = waiting(&scheduler, 1, 0).await;
        assert_eq!((load.running, load.reserved_bytes), (2, 2));
        drop(first);
        drop(joined(third).await);

        // A scheduler without limits reserves nothing and refuses nothing.
        let unlimited = DecodeScheduler::new(2);
        assert_eq!(unlimited.limits(), DecodeLimits::UNLIMITED);
        let permit = granted(&unlimited, Background, u64::MAX).await;
        assert_eq!(permit.reserved_bytes(), 0);
        assert_eq!(unlimited.load().reserved_bytes, 0);
    })
    .await;
}

/// However the holder of a permit ends, its permit and its bytes come back,
/// and a request that stops waiting leaves nothing behind.
#[tokio::test]
async fn a_permit_and_its_bytes_come_back_however_their_holder_ends() {
    finishes(async {
        let scheduler = DecodeScheduler::with_limits(4, limits(100, 8, 8));

        // The decode ends.
        drop(granted(&scheduler, Interactive, 100).await);
        idle(&scheduler).await;

        // The decode panics.
        let panicking = {
            let scheduler = scheduler.clone();
            tokio::spawn(async move {
                let _permit = scheduler.admit(Background, 30).await.expect("granted");
                panic!("a decode panicked while it held a permit");
            })
        };
        assert!(eventually("the panic", panicking).await.is_err());
        idle(&scheduler).await;

        // The task that holds the permit is aborted.
        let aborted = {
            let scheduler = scheduler.clone();
            tokio::spawn(async move {
                let _permit = scheduler.admit(Interactive, 70).await.expect("granted");
                std::future::pending::<()>().await;
            })
        };
        let load = eventually("the grant", scheduler.load_when(|load| load.running == 1)).await;
        assert_eq!(load.reserved_bytes, 70);
        aborted.abort();
        assert!(eventually("the abort", aborted).await.is_err());
        idle(&scheduler).await;

        // A request stops waiting: it took nothing and takes nothing later.
        let held = granted(&scheduler, Interactive, 100).await;
        for class in [Interactive, Background] {
            let abandoned = request(&scheduler, class, 10);
            eventually(
                "the request to wait",
                scheduler.load_when(|load| load.waiting_interactive + load.waiting_background == 1),
            )
            .await;
            abandoned.abort();
            assert!(eventually("the abort", abandoned).await.is_err());
            let load = scheduler.load();
            assert_eq!((load.waiting_interactive, load.waiting_background), (0, 0));
        }
        drop(held);
        let load = idle(&scheduler).await;
        assert_eq!(load.peak_reserved_bytes, 100);
    })
    .await;
}

// ---------------------------------------------------------------------------
// The estimate

/// What a piece of work reserves is the documented formula of the catalog
/// entry, for every codec and sample layout, with each number written out
/// here: 100 x 200 pixels throughout.
#[test]
fn the_estimate_is_the_documented_formula_of_the_catalog_entry() {
    const MIB: u64 = 1024 * 1024;
    const PIXELS: u64 = 100 * 200;
    let entry = |syntax: &str, samples: u32, bits: u32| {
        let mut entry = support::file_entry(PathBuf::from("not-read.dcm"), syntax, 1);
        (entry.rows, entry.columns) = (100, 200);
        (entry.samples_per_pixel, entry.bits_allocated) = (samples, bits);
        entry
    };
    assert_eq!(pixels::DICOM_DECODE_BASE_BYTES, 16 * MIB);
    assert_eq!(pixels::DISPLAY_BASE_BYTES, MIB);
    assert_eq!(pixels::THUMBNAIL_BASE_BYTES, 8 * MIB);

    // (transfer syntax, samples per pixel, bits allocated,
    //  the decode beside its 16 MiB, V, D, F)
    let p = PIXELS;
    let cases: [(&str, u32, u32, u64, u64, u64, u64); 19] = [
        // Native, 16-bit gray: F = 2 P; decode 3 F.
        (EXPLICIT_LE, 1, 16, 6 * p, 0, p, 2 * p),
        ("1.2.840.10008.1.2", 1, 8, 3 * p, 0, p, p),
        // One bit a sample is served a byte a sample.
        (EXPLICIT_LE, 1, 1, 3 * p, 0, p, p),
        // Bits that do not fill a byte take the next whole one: 12 bits are
        // two bytes a sample, 17 are three. An entry that claims no bits is
        // still reserved a byte a sample.
        (EXPLICIT_LE, 1, 12, 6 * p, 0, p, 2 * p),
        (EXPLICIT_LE, 1, 17, 9 * p, 0, p, 3 * p),
        (EXPLICIT_LE, 1, 0, 3 * p, 0, p, p),
        // 8-bit colour: F = 3 P, D = 3 P. Deeper colour: D = 6 P.
        (EXPLICIT_LE, 3, 8, 9 * p, 0, 3 * p, 3 * p),
        (EXPLICIT_LE, 3, 16, 18 * p, 0, 6 * p, 6 * p),
        // 32- and 64-bit samples: V = 32 S.
        (EXPLICIT_LE, 1, 32, 12 * p, 32 * p, p, 4 * p),
        (EXPLICIT_LE, 1, 64, 24 * p, 32 * p, p, 8 * p),
        // RLE and the deflated frame: 3 F.
        ("1.2.840.10008.1.2.5", 1, 16, 6 * p, 0, p, 2 * p),
        ("1.2.840.10008.1.2.8.1", 1, 1, 3 * p, 0, p, p),
        // JPEG Baseline, JPEG Lossless (both syntaxes), JPEG-LS: 5 F.
        ("1.2.840.10008.1.2.4.50", 3, 8, 15 * p, 0, 3 * p, 3 * p),
        ("1.2.840.10008.1.2.4.57", 1, 16, 10 * p, 0, p, 2 * p),
        ("1.2.840.10008.1.2.4.70", 1, 16, 10 * p, 0, p, 2 * p),
        ("1.2.840.10008.1.2.4.80", 1, 16, 10 * p, 0, p, 2 * p),
        // JPEG 2000 and JPEG XL: 16 S + 2 F.
        ("1.2.840.10008.1.2.4.90", 1, 16, 16 * p + 4 * p, 0, p, 2 * p),
        (
            "1.2.840.10008.1.2.4.110",
            3,
            8,
            16 * 3 * p + 6 * p,
            0,
            3 * p,
            3 * p,
        ),
        // A syntax with no codec: as native.
        ("1.2.840.10008.1.2.4.91", 1, 16, 6 * p, 0, p, 2 * p),
    ];
    for (syntax, samples, bits, decode, wide, display, frame) in cases {
        let entry = entry(syntax, samples, bits);
        let decode = 16 * MIB + decode;
        let expected = [
            (DecodeWork::RawFrame, decode),
            (DecodeWork::DisplayFrame, decode + wide + 3 * display + MIB),
            (DecodeWork::Thumbnail, decode + wide + display + 8 * MIB),
            (DecodeWork::PresentationLayer, 9 * p + MIB),
            (DecodeWork::RawRedaction, frame),
        ];
        for (work, bytes) in expected {
            assert_eq!(
                pixels::decode_estimate(&entry, work),
                bytes,
                "{work:?} of {samples} x {bits}-bit samples in {syntax}"
            );
        }
    }

    // An entry nothing vouches for cannot overflow the sums.
    let mut vast = entry(EXPLICIT_LE, u32::MAX, 64);
    (vast.rows, vast.columns) = (u32::MAX, u32::MAX);
    for work in [
        DecodeWork::RawFrame,
        DecodeWork::DisplayFrame,
        DecodeWork::Thumbnail,
        DecodeWork::RawRedaction,
    ] {
        assert_eq!(pixels::decode_estimate(&vast, work), u64::MAX, "{work:?}");
    }
    assert_eq!(
        pixels::decode_estimate(&vast, DecodeWork::PresentationLayer),
        u64::MAX
    );
}

/// A raster reserves the heap limit its decoder is held to, at the length
/// the file had when it was listed, whatever the file has become since; one
/// with more pixels than the viewer decodes has no estimate to fit a budget.
#[tokio::test]
async fn a_raster_reserves_its_decode_limit_at_the_length_it_was_listed_with() {
    finishes(async {
        const MIB: u64 = 1024 * 1024;
        let dir = tempdir().expect("temp dir");
        let bytes = rgba_png(40);
        let entry = listed_raster(dir.path(), "image.png", &bytes).await;
        let (p, frame, length) = (40 * 40, 40 * 40 * 4, bytes.len() as u64);
        // 32 MiB and one 64 KiB read buffer, six frames, four files.
        let decode = 32 * MIB + 64 * 1024 + 6 * frame + 4 * length;
        assert_eq!(
            pixels::raster_decode_heap_limit(&entry, length),
            Some(decode)
        );
        std::fs::write(&entry.path, [0; 16]).expect("shrink the file");
        let expected = [
            (DecodeWork::RawFrame, decode),
            (DecodeWork::DisplayFrame, decode + 3 * 3 * p + MIB),
            (DecodeWork::Thumbnail, decode + 3 * p + 8 * MIB),
            (DecodeWork::PresentationLayer, 9 * p + MIB),
            (DecodeWork::RawRedaction, frame),
        ];
        for (work, bytes) in expected {
            assert_eq!(pixels::decode_estimate(&entry, work), bytes, "{work:?}");
        }

        // More pixels than the viewer decodes: listed, never admitted.
        let huge = files::png_from_chunks(
            (20_000, 20_000),
            8,
            0,
            false,
            &[files::png_chunk(b"IDAT", &[])],
        );
        let entry = listed_raster(dir.path(), "huge.png", &huge).await;
        assert_eq!(pixels::raster_frame_bytes(&entry), None);
        assert_eq!(
            pixels::decode_estimate(&entry, DecodeWork::RawFrame),
            u64::MAX
        );
        assert_eq!(
            pixels::decode_estimate(&entry, DecodeWork::Thumbnail),
            u64::MAX
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// Through the API

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The loader's entries for `paths`, in that order.
async fn listed(paths: &[PathBuf]) -> Vec<FileEntry> {
    let report = support::discover(
        paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discover");
    paths
        .iter()
        .map(|path| {
            report
                .files
                .iter()
                .find(|file| &file.path == path)
                .unwrap_or_else(|| panic!("{} was not listed", path.display()))
                .clone()
        })
        .collect()
}

/// The loader's entry for the one raster `bytes` written as `name` in `dir`.
async fn listed_raster(dir: &Path, name: &str, bytes: &[u8]) -> FileEntry {
    std::fs::write(dir.join(name), bytes).expect("write raster");
    let scan = scan_dir(dir).await;
    scan.entry(name).clone()
}

/// A viewer over `entries` whose decodes `scheduler` admits.
fn serve(entries: Vec<FileEntry>, scheduler: &Arc<DecodeScheduler>) -> Arc<TestServer> {
    let state = support::app_state(entries).with_decode_scheduler(scheduler.clone());
    Arc::new(TestServer::new(server::router(state)))
}

/// A scheduler with the default limits and `permits` permits.
fn default_scheduler(permits: usize) -> Arc<DecodeScheduler> {
    DecodeScheduler::with_limits(permits, DecodeLimits::DEFAULT)
}

fn gray_png(side: u32) -> Vec<u8> {
    let pixels: Vec<u8> = (0..side * side).map(|index| (index % 251) as u8).collect();
    files::png_of(
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        (side, side),
        &pixels,
        |_| {},
    )
}

fn rgba_png(side: u32) -> Vec<u8> {
    let pixels: Vec<u8> = (0..side * side * 4)
        .map(|index| (index % 241) as u8)
        .collect();
    files::png_of(
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        (side, side),
        &pixels,
        |_| {},
    )
}

/// One request's status, sent from its own task so the test can watch the
/// scheduler meanwhile, or drop the request by aborting the task.
fn get(server: &Arc<TestServer>, path: &str) -> JoinHandle<StatusCode> {
    let (server, path) = (server.clone(), path.to_string());
    tokio::spawn(async move { server.get(&path).await.status_code() })
}

async fn status(handle: JoinHandle<StatusCode>) -> StatusCode {
    eventually("a response", handle)
        .await
        .expect("request task")
}

/// The endpoints of `endpoints::DECODING` that `support::every_endpoint_state`
/// can answer and that answer both refusals.
fn decoding_endpoints() -> Vec<&'static Endpoint> {
    endpoints::DECODING
        .iter()
        .filter(|endpoint| {
            !support::NEEDS_LINKED_SOURCE.contains(endpoint)
                && **endpoint != endpoints::FILE_SEMANTIC_CONTEXT
        })
        .collect()
}

/// With the budget held and no room to wait, every endpoint that decodes
/// answers 503 with `Retry-After` and the error envelope, every other
/// endpoint answers as usual, and the same requests succeed once the budget
/// is free.
#[tokio::test]
async fn a_busy_viewer_answers_503_with_retry_after_on_the_decoding_endpoints_only() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let memory = 1 << 40;
        let scheduler = DecodeScheduler::with_limits(4, limits(memory, 0, 0));
        let state =
            support::every_endpoint_state(dir.path()).with_decode_scheduler(scheduler.clone());
        let server = TestServer::new(server::router(state));
        let file = server.get("/api/files").await.json::<Value>()["files"][0].clone();
        assert_eq!(file["support_state"], "renderable");
        assert!(decoding_endpoints().len() >= 5);

        let held = granted(&scheduler, Interactive, memory).await;
        for endpoint in endpoints::ALL {
            if support::NEEDS_LINKED_SOURCE.contains(endpoint) {
                continue;
            }
            let response = support::endpoint_request(&server, endpoint, "0").await;
            if !decoding_endpoints().contains(&endpoint) {
                assert_eq!(
                    response.status_code().as_u16(),
                    endpoint.success_status,
                    "{} does not decode",
                    endpoint.id
                );
                assert!(
                    response.maybe_header("retry-after").is_none(),
                    "{}",
                    endpoint.id
                );
                continue;
            }
            assert_eq!(
                response.status_code(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{}",
                endpoint.id
            );
            assert_eq!(
                response.header("retry-after"),
                DECODE_BUSY_RETRY_AFTER_SECONDS.to_string(),
                "{}",
                endpoint.id
            );
            assert_eq!(response.header("content-type"), "application/json");
            assert!(
                response.maybe_header("x-cache").is_none(),
                "{}",
                endpoint.id
            );
            let body: Value = response.json();
            assert_eq!(body["code"], "decode_busy", "{}", endpoint.id);
            assert!(body["error"].as_str().is_some_and(|text| !text.is_empty()));
            assert_eq!(body.as_object().map(|object| object.len()), Some(2));
        }
        // Nothing was started, queued or kept for the refused requests.
        let load = scheduler.load();
        assert_eq!((load.running, load.reserved_bytes), (1, memory));
        assert_eq!((load.waiting_interactive, load.waiting_background), (0, 0));

        drop(held);
        for endpoint in decoding_endpoints() {
            let response = support::endpoint_request(&server, endpoint, "0").await;
            assert_eq!(
                response.status_code().as_u16(),
                endpoint.success_status,
                "{} after the budget was freed: {}",
                endpoint.id,
                response.text()
            );
            assert!(response.maybe_header("retry-after").is_none());
        }
        idle(&scheduler).await;
    })
    .await;
}

/// A frame whose decode needs more than the budget is refused with 422 and
/// its own code, at once and every time, and the catalog goes on listing the
/// file as renderable: the budget is the host's, not the file's.
#[tokio::test]
async fn a_frame_too_large_for_the_budget_is_refused_for_the_request_and_stays_renderable() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let entry = listed_raster(dir.path(), "image.png", &gray_png(64)).await;
        let estimate = |work| pixels::decode_estimate(&entry, work);
        let (raw, display, thumbnail, layer) = (
            estimate(DecodeWork::RawFrame),
            estimate(DecodeWork::DisplayFrame),
            estimate(DecodeWork::Thumbnail),
            estimate(DecodeWork::PresentationLayer),
        );
        assert!(layer < raw && raw < display && display < thumbnail);

        let refused = |response: &axum_test::TestResponse, what: &str| {
            assert_eq!(
                response.status_code(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{what}: {}",
                response.text()
            );
            let body: Value = response.json();
            assert_eq!(body["code"], "decode_memory_exceeded", "{what}");
            assert!(response.maybe_header("retry-after").is_none(), "{what}");
        };
        let paths = [
            "/api/file/0/frame/0",
            "/api/file/0/frame/0?preview=true&wc=100&ww=50",
            "/api/file/0/frame/0/raw",
            "/api/file/0/frame/0/raw/pixel?row=1&column=1",
            "/api/file/0/frame/0/thumbnail",
            "/api/file/0/frame/0/presentation-layer",
        ];
        // (budget, the paths that fit it)
        let cases: [(u64, &[usize]); 4] = [
            // One byte short of the raw decode: only the layer is drawn.
            (raw - 1, &[5]),
            // The raw decode fits and the display frame does not.
            (display - 1, &[2, 3, 5]),
            // The viewer's frames fit; a thumbnail needs more than half.
            (display, &[0, 1, 2, 3, 5]),
            // Twice the thumbnail: its share of the budget is enough.
            (2 * thumbnail, &[0, 1, 2, 3, 4, 5]),
        ];
        for (budget, fitting) in cases {
            let scheduler = DecodeScheduler::with_limits(2, limits(budget, 8, 8));
            let server = serve(vec![entry.clone()], &scheduler);
            for round in 0..2 {
                for (index, path) in paths.iter().enumerate() {
                    let response = server.get(path).await;
                    let what = format!("{path} with a budget of {budget}, round {round}");
                    if fitting.contains(&index) {
                        assert_eq!(response.status_code(), 200, "{what}: {}", response.text());
                    } else {
                        refused(&response, &what);
                    }
                }
            }
            let file = server.get("/api/files").await.json::<Value>()["files"][0].clone();
            assert_eq!(file["support_state"], "renderable", "budget {budget}");
            assert_eq!(file["support_reason"], Value::Null, "budget {budget}");
            let load = idle(&scheduler).await;
            assert!(load.peak_reserved_bytes <= budget);
        }

        // The refusal names what was needed and the limit that was passed:
        // the budget for a frame, the share of it for a thumbnail.
        let budget = display - 1;
        let server = serve(
            vec![entry.clone()],
            &DecodeScheduler::with_limits(2, limits(budget, 8, 8)),
        );
        let explained = |path: &'static str| {
            let server = server.clone();
            async move { server.get(path).await.json::<Value>()["error"].to_string() }
        };
        let frame = explained(paths[0]).await;
        assert!(frame.contains(&display.to_string()), "{frame}");
        assert!(frame.contains(&budget.to_string()), "{frame}");
        assert!(!frame.contains("thumbnail"), "{frame}");
        let small = explained(paths[4]).await;
        assert!(small.contains(&thumbnail.to_string()), "{small}");
        assert!(small.contains(&(budget / 2).to_string()), "{small}");
        assert!(small.contains("thumbnail"), "{small}");
        for text in [&frame, &small] {
            assert!(text.contains("--decode-memory"), "{text}");
        }

        // A viewer nobody configured has the default budget: a frame that
        // would need more is refused before its file is opened.
        let path = dir.path().join("huge.dcm");
        std::fs::write(&path, b"not read").expect("write placeholder");
        let mut huge = support::file_entry(path, EXPLICIT_LE, 1);
        (huge.rows, huge.columns) = (60_000, 60_000);
        assert!(
            pixels::decode_estimate(&huge, DecodeWork::RawFrame)
                > pixels::DECODE_MEMORY_DEFAULT_BYTES
        );
        let state = support::app_state(vec![huge]);
        assert_eq!(state.decode_scheduler().limits(), DecodeLimits::DEFAULT);
        assert_eq!(state.decode_scheduler().permits(), pixels::host_permits());
        let server = TestServer::new(server::router(state));
        for path in ["/api/file/0/frame/0", "/api/file/0/frame/0/raw"] {
            refused(&server.get(path).await, path);
        }
    })
    .await;
}

/// What a request reserves is its documented estimate, once, for every
/// kind of file and endpoint: one permit covers a display frame's decode,
/// window, encoding and redaction.
#[tokio::test]
async fn each_request_reserves_its_documented_estimate_under_one_permit() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let mut entries = listed(&[
            fixture("golden-uncompressed-u16-multiframe.dcm"),
            fixture("golden-jpeg-baseline-single-frame.dcm"),
            fixture("golden-jpeg2000-lossless-u8-single-frame.dcm"),
            fixture("golden-rle-ybr-full-422-u8-single-frame.dcm"),
        ])
        .await;
        entries.push(listed_raster(dir.path(), "gray.png", &gray_png(48)).await);
        let colour = tempdir().expect("temp dir");
        entries.push(listed_raster(colour.path(), "rgba.png", &rgba_png(48)).await);

        let requests = [
            ("", DecodeWork::DisplayFrame),
            ("?mode=full_dynamic", DecodeWork::DisplayFrame),
            ("?preview=true&mode=full_dynamic", DecodeWork::DisplayFrame),
            ("/raw", DecodeWork::RawFrame),
            ("/thumbnail", DecodeWork::Thumbnail),
            ("/presentation-layer", DecodeWork::PresentationLayer),
        ];
        for entry in &entries {
            let name = entry.path.file_name().expect("name").to_string_lossy();
            for (suffix, work) in requests {
                let estimate = pixels::decode_estimate(entry, work);
                // One permit: a second, taken while the first was held, would
                // never be granted.
                let scheduler = default_scheduler(1);
                let server = serve(vec![entry.clone()], &scheduler);
                let path = format!("/api/file/0/frame/0{suffix}");
                let response = eventually("a response", async { server.get(&path).await }).await;
                assert_eq!(
                    response.status_code(),
                    200,
                    "{name}{suffix}: {}",
                    response.text()
                );
                let load = idle(&scheduler).await;
                assert_eq!(
                    load.peak_reserved_bytes, estimate,
                    "{name}{suffix} reserves the estimate of {work:?}"
                );
            }
        }
    })
    .await;
}

/// A frame the caches hold is served without a permit, so a busy viewer
/// still shows what it has; filling redaction boxes in a cached raw frame
/// is a copy of the frame and takes one.
#[tokio::test]
async fn cached_frames_are_served_while_the_budget_is_held() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let entry = listed_raster(dir.path(), "image.png", &gray_png(64)).await;
        let frame = pixels::decode_estimate(&entry, DecodeWork::RawRedaction);
        assert_eq!(frame, 64 * 64);
        let memory = 1 << 40;
        let scheduler = DecodeScheduler::with_limits(4, limits(memory, 0, 0));
        let server = serve(vec![entry.clone()], &scheduler);
        let cached = [
            "/api/file/0/frame/0",
            "/api/file/0/frame/0/raw",
            "/api/file/0/frame/0/thumbnail",
            "/api/file/0/frame/0/presentation-layer",
        ];
        for path in cached {
            server.get(path).await.assert_status_ok();
        }

        let held = granted(&scheduler, Interactive, memory).await;
        for path in cached {
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            assert_eq!(response.header("x-cache"), "HIT", "{path}");
        }
        // A preview of a window that is cached is served from the cache too.
        let preview = server.get("/api/file/0/frame/0?preview=true").await;
        assert_eq!(preview.status_code(), 200);
        assert_eq!(preview.header("x-cache"), "HIT");
        // Another window is new work.
        for path in [
            "/api/file/0/frame/0?wc=10&ww=20",
            "/api/file/0/frame/0?preview=true&wc=10&ww=20",
            "/api/file/0/frame/0/thumbnail?size=512",
        ] {
            assert_eq!(server.get(path).await.status_code(), 503, "{path}");
        }
        drop(held);

        // Boxes on the file: the raw frame is still cached, and the copy the
        // boxes are filled in reserves the frame's bytes.
        server
            .put("/api/file/0/redactions")
            .json(&json!({"num_roi": 1, "roi_coords": [[1, 1, 8, 8]], "roi_frames": []}))
            .await
            .assert_status_ok();
        let held = granted(&scheduler, Interactive, memory - frame + 1).await;
        for path in [
            "/api/file/0/frame/0/raw",
            "/api/file/0/frame/0/raw/pixel?row=2&column=2",
        ] {
            assert_eq!(server.get(path).await.status_code(), 503, "{path}");
        }
        drop(held);
        let held = granted(&scheduler, Interactive, memory - frame).await;
        for path in [
            "/api/file/0/frame/0/raw",
            "/api/file/0/frame/0/raw/pixel?row=2&column=2",
        ] {
            assert_eq!(server.get(path).await.status_code(), 200, "{path}");
        }
        drop(held);
        assert_eq!(idle(&scheduler).await.peak_reserved_bytes, memory);
    })
    .await;
}

/// A thumbnail waits for its share while the viewer's frames of the same
/// file are decoded past it, and is served once background decodes finish.
#[tokio::test]
async fn a_thumbnail_waits_for_its_share_while_the_viewer_is_served() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let entry = listed_raster(dir.path(), "image.png", &gray_png(64)).await;
        let thumbnail = pixels::decode_estimate(&entry, DecodeWork::Thumbnail);
        let memory = 4 * thumbnail;
        let scheduler = DecodeScheduler::with_limits(8, limits(memory, 8, 8));
        let server = serve(vec![entry], &scheduler);

        // Another thumbnail's decode holds all of the background share but one
        // byte less than this thumbnail needs.
        let other = granted(&scheduler, Background, memory / 2 - thumbnail + 1).await;
        let waiting_thumbnail = get(&server, "/api/file/0/frame/0/thumbnail");
        let load = waiting(&scheduler, 0, 1).await;
        assert_eq!(load.running, 1, "the thumbnail has not started");

        for path in ["/api/file/0/frame/0", "/api/file/0/frame/0/raw"] {
            let response = eventually("the viewer's frame", async { server.get(path).await }).await;
            assert_eq!(response.status_code(), 200, "{path}");
        }
        assert_eq!(scheduler.load().waiting_background, 1);

        drop(other);
        assert_eq!(status(waiting_thumbnail).await, 200);
        let load = idle(&scheduler).await;
        assert!(load.peak_reserved_bytes <= memory);
    })
    .await;
}

/// With one permit, raw and display requests for the same cold frames all
/// finish: a display frame never holds the permit its own raw decode, or
/// another request's, is waiting for.
#[tokio::test]
async fn one_permit_serves_raw_and_display_requests_for_the_same_cold_frames() {
    finishes(async {
        let entries = listed(&[fixture("golden-uncompressed-u16-multiframe.dcm")]).await;
        let frames = entries[0].frame_count;
        assert!(frames >= 2);
        let display = pixels::decode_estimate(&entries[0], DecodeWork::DisplayFrame);
        let scheduler = default_scheduler(1);
        let server = serve(entries, &scheduler);

        let mut requests = Vec::new();
        for frame in 0..frames {
            for suffix in [
                "/raw",
                "",
                "?mode=full_dynamic",
                "/thumbnail",
                "?preview=true&wc=9&ww=9",
            ] {
                requests.push(get(&server, &format!("/api/file/0/frame/{frame}{suffix}")));
            }
        }
        for request in requests {
            assert_eq!(status(request).await, 200);
        }
        let load = idle(&scheduler).await;
        assert!(
            load.peak_reserved_bytes
                <= display.max(pixels::decode_estimate(
                    &listed(&[fixture("golden-uncompressed-u16-multiframe.dcm")]).await[0],
                    DecodeWork::Thumbnail
                ))
        );
    })
    .await;
}

/// A request that is dropped while it waits starts no work and reserves
/// nothing, whatever it asked for: a thumbnail and a preview wait in the
/// request itself, and the decode of a display or raw frame leaves the
/// queue when the last request waiting for it is gone.
#[tokio::test]
async fn a_request_dropped_while_it_waits_reserves_nothing() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let entry = listed_raster(dir.path(), "image.png", &gray_png(64)).await;
        let scheduler = default_scheduler(1);
        let server = serve(vec![entry], &scheduler);
        let held = granted(&scheduler, Interactive, 0).await;

        let paths = [
            ("/api/file/0/frame/0/thumbnail", (0, 1)),
            ("/api/file/0/frame/0?preview=true&wc=10&ww=20", (1, 0)),
            ("/api/file/0/frame/0", (1, 0)),
            ("/api/file/0/frame/0/raw", (1, 0)),
        ];
        for (path, queues) in paths {
            let dropped = get(&server, path);
            waiting(&scheduler, queues.0, queues.1).await;
            dropped.abort();
            assert!(eventually("the abort", dropped).await.is_err());
            waiting(&scheduler, 0, 0).await;
        }

        drop(held);
        let load = idle(&scheduler).await;
        assert_eq!(load.peak_reserved_bytes, 0, "nothing was decoded");
        // Nothing was kept for them either: each is new work when asked again.
        for (path, _) in paths.iter().rev() {
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            assert_eq!(response.header("x-cache"), "MISS", "{path}");
        }
    })
    .await;
}

/// Display and raw frames whose requests were all dropped while their
/// decodes waited leave the queue: it has room again, the next request is
/// served as if they had never been asked for, and none of them is decoded.
#[tokio::test]
async fn decodes_whose_requests_were_all_dropped_leave_the_queue() {
    finishes(async {
        const QUEUED: usize = 12;
        let entries = listed(&[fixture("golden-uncompressed-u16-multiframe.dcm")]).await;
        assert!(entries[0].frame_count >= 2);
        let display = pixels::decode_estimate(&entries[0], DecodeWork::DisplayFrame);
        let scheduler =
            DecodeScheduler::with_limits(1, limits(pixels::DECODE_MEMORY_DEFAULT_BYTES, QUEUED, 0));
        let server = serve(entries, &scheduler);
        let held = granted(&scheduler, Interactive, 0).await;

        // Each window of a frame is a decode of its own, and so is its raw
        // frame: the queue is full of them.
        let window = |index: usize| format!("/api/file/0/frame/0?wc={}&ww=40", 100 + index);
        let mut abandoned: Vec<_> = (1..QUEUED)
            .map(|index| get(&server, &window(index)))
            .collect();
        abandoned.push(get(&server, "/api/file/0/frame/0/raw"));
        waiting(&scheduler, QUEUED, 0).await;
        assert_eq!(server.get("/api/file/0/frame/1").await.status_code(), 503);

        abandoned.iter().for_each(JoinHandle::abort);
        waiting(&scheduler, 0, 0).await;

        // A request that is still wanted finds the queue empty.
        let wanted = get(&server, "/api/file/0/frame/1");
        waiting(&scheduler, 1, 0).await;
        drop(held);
        assert_eq!(status(wanted).await, 200);
        let load = idle(&scheduler).await;
        assert_eq!(
            load.peak_reserved_bytes, display,
            "only the request that was still wanted was decoded"
        );

        // A frame that was given up is decoded when it is asked for again.
        for path in ["/api/file/0/frame/0/raw".to_string(), window(1)] {
            let response = server.get(&path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            assert_eq!(response.header("x-cache"), "MISS", "{path}");
        }
        idle(&scheduler).await;
    })
    .await;
}

/// A decode that two requests wait for stays queued when one of them is
/// dropped, and serves the other.
#[tokio::test]
async fn a_queued_decode_goes_on_while_one_of_its_requests_still_waits() {
    finishes(async {
        let entry = Arc::new(
            listed(&[fixture("golden-uncompressed-u16-multiframe.dcm")])
                .await
                .remove(0),
        );
        let scheduler = default_scheduler(1);
        let cache = Arc::new(Mutex::new(pixels::RawFrameCache::with_scheduler(
            1 << 24,
            scheduler.clone(),
        )));
        let raw_frame = || {
            Box::pin(pixels::load_raw_frame(
                entry.clone(),
                cache.clone(),
                pixels::RawFrameRequest { frame: 0 },
            ))
        };
        let held = granted(&scheduler, Interactive, 0).await;

        // Polled once, each request has announced the decode or joined it.
        let (mut first, mut second) = (raw_frame(), raw_frame());
        assert!(poll!(&mut first).is_pending());
        assert!(poll!(&mut second).is_pending());
        waiting(&scheduler, 1, 0).await;

        // The decode's task is given its turn (the test's runtime has one
        // thread) and has nothing to do: a request is still waiting.
        drop(first);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(scheduler.load().waiting_interactive, 1);

        drop(held);
        let served = eventually("the remaining request", second)
            .await
            .expect("the shared decode");
        assert!(served.cache_hit, "the second request shared the first's");
        let load = idle(&scheduler).await;
        assert_eq!(
            load.peak_reserved_bytes,
            pixels::decode_estimate(&entry, DecodeWork::RawFrame),
            "one decode served it"
        );
    })
    .await;
}

/// However a decode ends (a frame, a corrupt file, a decoder that panics, a
/// file that is gone), its permit and its bytes are returned.
#[tokio::test]
async fn every_way_a_decode_ends_returns_its_permit() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let good = gray_png(64);
        let cases: [(&str, Vec<u8>, u16); 4] = [
            ("good.png", good.clone(), 200),
            ("cut.png", good.clone(), 500),
            ("panics.jpg", files::jpeg_its_decoder_panics_on(), 500),
            ("gone.png", good.clone(), 404),
        ];
        for (name, bytes, expected) in cases {
            let dir = dir.path().join(name.replace('.', "-"));
            std::fs::create_dir(&dir).expect("case dir");
            let entry = listed_raster(&dir, name, &bytes).await;
            assert!(pixels::decode_estimate(&entry, DecodeWork::RawFrame) > 0);
            match name {
                "cut.png" => std::fs::write(&entry.path, &bytes[..bytes.len() / 2]).expect("cut"),
                "gone.png" => std::fs::remove_file(&entry.path).expect("remove"),
                _ => {}
            }
            for suffix in ["", "?preview=true&wc=10&ww=20", "/raw", "/thumbnail"] {
                let scheduler = default_scheduler(2);
                let server = serve(vec![entry.clone()], &scheduler);
                let response = server.get(&format!("/api/file/0/frame/0{suffix}")).await;
                assert_eq!(
                    response.status_code().as_u16(),
                    expected,
                    "{name}{suffix}: {}",
                    response.text()
                );
                let load = idle(&scheduler).await;
                assert!(load.peak_reserved_bytes > 0, "{name}{suffix} was admitted");
            }
        }
    })
    .await;
}

/// The legend of a dose overlay decodes the dose's frames. A viewer too
/// busy to decode them answers 503 and does not conclude that the overlay
/// cannot be drawn: the next request, with the budget free, finds it
/// eligible.
#[tokio::test]
async fn a_busy_viewer_does_not_mark_a_value_overlay_ineligible() {
    finishes(async {
        let entries = listed(&[
            fixture("golden-rtdose-u16-grid.dcm"),
            fixture("golden-rtdose-ct-source-z0.dcm"),
        ])
        .await;
        assert!(pixels::decode_estimate(&entries[0], DecodeWork::RawFrame) > 0);
        let memory = 1 << 40;
        let scheduler = DecodeScheduler::with_limits(4, limits(memory, 0, 0));
        let server = serve(entries, &scheduler);
        let paths = [
            "/api/file/0/semantic-context".to_string(),
            "/api/file/1/frame/0/dose-overlay?dose=0".to_string(),
            "/api/file/1/frame/0/dose-overlay/values?dose=0".to_string(),
        ];

        let held = granted(&scheduler, Interactive, memory).await;
        for path in &paths {
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 503, "{path}: {}", response.text());
            assert_eq!(response.json::<Value>()["code"], "decode_busy", "{path}");
            assert_eq!(
                response.header("retry-after"),
                DECODE_BUSY_RETRY_AFTER_SECONDS.to_string()
            );
        }
        drop(held);

        let context: Value = server.get(&paths[0]).await.json();
        assert_eq!(context["context"]["overlay"]["eligible"], true, "{context}");
        for path in &paths[1..] {
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
        }
        idle(&scheduler).await;
    })
    .await;
}

// ---------------------------------------------------------------------------
// Files too long to read, and files that grew

/// A PNG, JPEG or WebP longer than its read budget is listed as not decoded,
/// with its own reason, and its frames answer 422 naming it; at the budget
/// it is renderable. A TIFF, read a page at a time, may be any length.
#[tokio::test]
async fn a_raster_longer_than_its_read_budget_is_listed_as_not_decoded() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let tiff = files::tiff_file(
            false,
            &[files::TiffPage::strip((8, 8), &[8], 1, vec![7; 64])],
        )
        .0;
        let jpeg = files::baseline_jpeg(image::ExtendedColorType::L8, (8, 8), &[9; 64]);
        let webp = super::raster_discovery::webp_rgba(8, 8);
        // (name, bytes, bytes of one frame)
        let rasters: [(&str, Vec<u8>, u64); 4] = [
            ("image.png", gray_png(8), 64),
            ("image.jpg", jpeg, 64),
            ("image.webp", webp, 256),
            ("image.tif", tiff, 64),
        ];
        let lengthen = |name: &str, bytes: &[u8], length: u64| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).expect("write raster");
            // Zeros after the image; a sparse file where the system has them.
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open raster");
            file.set_len(length).expect("lengthen raster");
        };
        for (name, bytes, frame) in &rasters {
            let budget = pixels::RASTER_READ_BUDGET_BASE_BYTES
                + pixels::RASTER_READ_BUDGET_PER_DECODED_BYTE * frame;
            lengthen(&format!("at-budget-{name}"), bytes, budget);
            lengthen(&format!("too-long-{name}"), bytes, budget + 1);
        }
        let scan = scan_dir(dir.path()).await;

        for (name, _, _) in &rasters {
            let at_budget = scan.file(&format!("at-budget-{name}"));
            assert_eq!(at_budget["support_state"], "renderable", "{name}");
            assert_eq!(at_budget["support_reason"], Value::Null, "{name}");

            let too_long = scan.file(&format!("too-long-{name}"));
            if *name == "image.tif" {
                assert_eq!(too_long["support_state"], "renderable", "{name}");
                let index = scan.index(&format!("too-long-{name}"));
                let raw = scan
                    .server
                    .get(&format!("/api/file/{index}/frame/0/raw"))
                    .await;
                assert_eq!(raw.status_code(), 200, "{}", raw.text());
                assert_eq!(raw.as_bytes().as_ref(), [7_u8; 64].as_slice());
                continue;
            }
            assert_eq!(too_long["support_state"], "unsupported", "{name}");
            assert_eq!(
                too_long["support_reason"], "raster.file_too_large",
                "{name}"
            );
            assert_eq!(too_long["file_format"], at_budget["file_format"]);
            let index = scan.index(&format!("too-long-{name}"));
            for suffix in [
                "",
                "/raw",
                "/raw/pixel?row=0&column=0",
                "/thumbnail",
                "/presentation-layer",
            ] {
                let response = scan
                    .server
                    .get(&format!("/api/file/{index}/frame/0{suffix}"))
                    .await;
                assert_eq!(response.status_code(), 422, "{name}{suffix}");
                let body: Value = response.json();
                assert_eq!(body["code"], "unsupported_pixel_layout", "{name}{suffix}");
                assert!(
                    body["error"]
                        .as_str()
                        .is_some_and(|text| text.contains("raster.file_too_large")),
                    "{name}{suffix}: {body}"
                );
            }
        }
        // What is listed renderable at its budget decodes.
        let index = scan.index("at-budget-image.png");
        let raw = scan
            .server
            .get(&format!("/api/file/{index}/frame/0/raw"))
            .await;
        assert_eq!(raw.status_code(), 200, "{}", raw.text());
        assert_eq!(raw.as_bytes().len(), 64);
    })
    .await;
}

/// A decode's memory is reserved from the length the file had when it was
/// listed, so a file that has grown since is refused, not read.
#[tokio::test]
async fn a_raster_that_has_grown_since_it_was_listed_is_not_decoded() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let bytes = gray_png(16);
        let entry = listed_raster(dir.path(), "image.png", &bytes).await;
        assert_eq!(
            pixels::decode_estimate(&entry, DecodeWork::RawFrame),
            pixels::raster_decode_heap_limit(&entry, bytes.len() as u64).expect("limit")
        );
        let scheduler = default_scheduler(2);
        let server = serve(vec![entry.clone()], &scheduler);
        server
            .get("/api/file/0/frame/0/thumbnail")
            .await
            .assert_status_ok();

        let mut grown = bytes.clone();
        grown.extend_from_slice(&[0; 4096]);
        std::fs::write(&entry.path, &grown).expect("grow the file");
        for suffix in ["", "/raw"] {
            let response = server.get(&format!("/api/file/0/frame/0{suffix}")).await;
            assert_eq!(response.status_code(), 500, "{suffix}: {}", response.text());
            let body: Value = response.json();
            assert_eq!(body["code"], "pixel_decode_failed");
            // The viewer shows this text: it says what to do.
            let text = body["error"].as_str().expect("error text");
            assert!(
                text.contains("changed") && text.contains("reopen"),
                "{text}"
            );
        }
        // The same file back at its listed length decodes again.
        std::fs::write(&entry.path, &bytes).expect("restore the file");
        server
            .get("/api/file/0/frame/0/raw")
            .await
            .assert_status_ok();
        idle(&scheduler).await;
    })
    .await;
}

// ---------------------------------------------------------------------------
// Decodes held in place

/// A raster file replaced by a pipe nobody writes to: the decode that opens
/// it stops there, on its blocking thread, until [`HeldFile::release`].
#[cfg(unix)]
struct HeldFile {
    path: PathBuf,
    /// A second name for the pipe, so it can still be opened once the file
    /// is back under the first.
    pipe: PathBuf,
    original: Vec<u8>,
    released: bool,
}

#[cfg(unix)]
impl HeldFile {
    fn hold(entry: &FileEntry) -> Self {
        use std::os::unix::ffi::OsStrExt;
        let original = std::fs::read(&entry.path).expect("read raster");
        std::fs::remove_file(&entry.path).expect("remove raster");
        let name = std::ffi::CString::new(entry.path.as_os_str().as_bytes()).expect("path");
        // SAFETY: `name` is a valid NUL-terminated path for the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0, "mkfifo");
        let pipe = entry.path.with_extension("pipe");
        std::fs::hard_link(&entry.path, &pipe).expect("name the pipe twice");
        Self {
            path: entry.path.clone(),
            pipe,
            original,
            released: false,
        }
    }

    /// Lets the decode go on. The file is put back first, so whoever opens
    /// the path from now on finds it; then whoever is already waiting in
    /// the pipe's `open` is let through, to find the pipe empty. Nothing
    /// here waits: a writer opens the pipe at once if a reader is there and
    /// fails at once if none is.
    fn release(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        if std::mem::replace(&mut self.released, true) {
            return;
        }
        let restored = self.path.with_extension("restored");
        std::fs::write(&restored, &self.original).expect("write raster");
        std::fs::rename(&restored, &self.path).expect("restore raster");
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.pipe);
        drop(writer);
        let _ = std::fs::remove_file(&self.pipe);
    }
}

#[cfg(unix)]
impl Drop for HeldFile {
    fn drop(&mut self) {
        self.release();
    }
}

/// A request that is dropped once its decode has begun keeps its permit and
/// its bytes until the decode ends. That holds for a preview too: its
/// blocking work goes on after the request is gone, and it stays counted
/// for as long.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_dropped_after_its_decode_began_keeps_its_permit_until_the_decode_ends() {
    finishes(async {
        let requests = [
            ("?preview=true&wc=10&ww=20", DecodeWork::DisplayFrame),
            ("", DecodeWork::DisplayFrame),
            ("/raw", DecodeWork::RawFrame),
            ("/thumbnail", DecodeWork::Thumbnail),
        ];
        for (suffix, work) in requests {
            let dir = tempdir().expect("temp dir");
            let entry = listed_raster(dir.path(), "image.png", &gray_png(64)).await;
            let estimate = pixels::decode_estimate(&entry, work);
            let scheduler = default_scheduler(2);
            let server = serve(vec![entry.clone()], &scheduler);
            let mut held = HeldFile::hold(&entry);

            let dropped = get(&server, &format!("/api/file/0/frame/0{suffix}"));
            let load = eventually(
                "the decode to start",
                scheduler.load_when(|load| load.running == 1),
            )
            .await;
            assert_eq!(load.reserved_bytes, estimate, "{suffix}");
            dropped.abort();
            assert!(eventually("the abort", dropped).await.is_err());

            // The request is gone and its decode is not.
            let load = scheduler.load();
            assert_eq!(
                (load.running, load.reserved_bytes),
                (1, estimate),
                "{suffix}"
            );
            // One more decode fits beside it, and no more than the permits allow.
            let beside = granted(&scheduler, Interactive, 0).await;
            let third = request(&scheduler, Interactive, 0);
            let load = waiting(&scheduler, 1, 0).await;
            assert_eq!(
                load.running, 2,
                "{suffix}: the dropped request's permit is still held"
            );
            drop(beside);
            drop(joined(third).await);

            held.release();
            let load = idle(&scheduler).await;
            assert_eq!(load.peak_reserved_bytes, estimate, "{suffix}");
        }
    })
    .await;
}

/// A presentation layer whose request is dropped while it is drawn keeps
/// its permit and its bytes until the drawing ends: the permit belongs to
/// the blocking work, not to the request.
#[tokio::test]
async fn a_presentation_layer_dropped_while_it_is_drawn_keeps_its_permit() {
    finishes(async {
        let entry = listed(&[fixture("golden-uncompressed-u16-multiframe.dcm")])
            .await
            .remove(0);
        let estimate = pixels::decode_estimate(&entry, DecodeWork::PresentationLayer);
        let scheduler = default_scheduler(2);

        // The drawing stops until the test lets it go on.
        let (go_on, held) = std::sync::mpsc::channel::<()>();
        let dropped = tokio::spawn({
            let scheduler = scheduler.clone();
            async move {
                pixels::draw_presentation_layer(&scheduler, &entry, move || {
                    let _ = held.recv();
                })
                .await
            }
        });
        let load = eventually(
            "the drawing to start",
            scheduler.load_when(|load| load.running == 1),
        )
        .await;
        assert_eq!(load.reserved_bytes, estimate);
        dropped.abort();
        assert!(eventually("the abort", dropped).await.is_err());

        // The request is gone and its drawing is not.
        let load = scheduler.load();
        assert_eq!((load.running, load.reserved_bytes), (1, estimate));
        go_on.send(()).expect("the drawing is waiting");
        assert_eq!(idle(&scheduler).await.peak_reserved_bytes, estimate);
    })
    .await;
}

// ---------------------------------------------------------------------------
// Overlays and legends

use dcmview::types::OverlayEncoding;

/// The estimate of an overlay drawn on a displayed frame of `rows` x
/// `columns` from `planes` frames of `overlay`.
fn value_overlay(
    overlay: &FileEntry,
    (rows, columns): (u32, u32),
    planes: u32,
    encoding: OverlayEncoding,
) -> u64 {
    pixels::decode_estimate(
        overlay,
        DecodeWork::ValueOverlay {
            target_rows: rows,
            target_columns: columns,
            planes,
            encoding,
        },
    )
}

/// The estimate of a SEG frame of `segmentation` painted on `target`.
fn segmentation_overlay(segmentation: &FileEntry, target: &FileEntry) -> u64 {
    pixels::decode_estimate(
        segmentation,
        DecodeWork::SegmentationOverlay {
            target_rows: target.rows,
            target_columns: target.columns,
        },
    )
}

/// The committed overlay fixtures, in the order the tests name them by
/// index: the RT Dose grid (4 x 4, three planes at z = 0, 4 and 8), the CT
/// slices at z = 0 (on a plane) and z = 6 (between two), the Parametric Map
/// (4 x 4, planes at z = 0 and 2), its source at z = 1 (between them), the
/// binary SEG and its source.
async fn overlay_fixtures() -> Vec<FileEntry> {
    listed(&[
        fixture("golden-rtdose-u16-grid.dcm"),
        fixture("golden-rtdose-ct-source-z0.dcm"),
        fixture("golden-rtdose-ct-source-z6.dcm"),
        fixture("golden-parametric-map-u16-linear.dcm"),
        fixture("golden-parametric-map-mr-source-z1.dcm"),
        fixture("golden-seg-binary.dcm"),
        fixture("golden-seg-binary-source.dcm"),
    ])
    .await
}

const DOSE: usize = 0;
const DOSE_ON_A_PLANE: usize = 1;
const DOSE_BETWEEN_PLANES: usize = 2;
const MAP: usize = 3;
const MAP_BETWEEN_PLANES: usize = 4;
const SEG: usize = 5;
const SEG_SOURCE: usize = 6;

/// Overlay and legend work reserves the documented formula of the overlay
/// object's entry and the displayed frame it is drawn on.
#[test]
fn the_estimate_of_overlay_work_is_the_documented_formula() {
    const MIB: u64 = 1024 * 1024;
    // The overlay object: 100 x 200. The displayed frame: 300 x 500.
    let p = 100 * 200;
    let t = 300 * 500;
    let entry = |syntax: &str, bits: u32| {
        let mut entry = support::file_entry(PathBuf::from("not-read.dcm"), syntax, 4);
        (entry.rows, entry.columns) = (100, 200);
        entry.bits_allocated = bits;
        entry
    };
    let overlay = |entry: &FileEntry, planes, encoding| {
        pixels::decode_estimate(
            entry,
            DecodeWork::ValueOverlay {
                target_rows: 300,
                target_columns: 500,
                planes,
                encoding,
            },
        )
    };
    let painted = |entry: &FileEntry| {
        pixels::decode_estimate(
            entry,
            DecodeWork::SegmentationOverlay {
                target_rows: 300,
                target_columns: 500,
            },
        )
    };

    // (transfer syntax, bits allocated, the decode beside its 16 MiB)
    let cases = [
        (EXPLICIT_LE, 16, 6 * p),
        (EXPLICIT_LE, 1, 3 * p),
        (EXPLICIT_LE, 8, 3 * p),
        (EXPLICIT_LE, 32, 12 * p),
        (EXPLICIT_LE, 64, 24 * p),
        ("1.2.840.10008.1.2.4.70", 16, 10 * p),
        ("1.2.840.10008.1.2.4.90", 16, 20 * p),
    ];
    for (syntax, bits, decode) in cases {
        let entry = entry(syntax, bits);
        let decode = 16 * MIB + decode;
        let what = format!("{bits}-bit samples in {syntax}");
        assert_eq!(
            pixels::decode_estimate(&entry, DecodeWork::RawFrame),
            decode,
            "{what}"
        );
        // A legend: the decode and one frame of eight-byte values.
        assert_eq!(
            pixels::decode_estimate(&entry, DecodeWork::ValueLegend),
            decode + 8 * p,
            "{what}"
        );
        // A SEG frame painted on the displayed frame: 24 bytes a pixel of it.
        assert_eq!(painted(&entry), decode + 24 * t + MIB, "{what}");
        // A value overlay: its planes, and the larger of the decode and of
        // what the displayed frame takes (here the displayed frame, which
        // is larger than 16 MiB only as an image of many planes' worth).
        for planes in [0, 1, 2, 4] {
            let held = 8 * u64::from(planes) * p;
            assert_eq!(
                overlay(&entry, planes, OverlayEncoding::Png),
                held + decode.max(32 * t + MIB),
                "{what}, {planes} planes"
            );
            assert_eq!(
                overlay(&entry, planes, OverlayEncoding::Values),
                held + decode.max(12 * t + MIB),
                "{what}, {planes} planes"
            );
        }
    }
    // Both sides of the larger-of are met. On the displayed frame above
    // the decode is the larger for every layout, so those rows say nothing
    // of what a displayed pixel costs. On one of 3000 x 5000 the displayed
    // frame is the larger for every layout, and the numbers are written
    // out with no larger-of: 32 bytes a pixel of it as an image, 12 as
    // values, and a megabyte.
    let large = 3000 * 5000;
    for (syntax, bits, decode) in cases {
        let entry = entry(syntax, bits);
        let what = format!("{bits}-bit samples in {syntax} on a large frame");
        for (encoding, each) in [(OverlayEncoding::Png, 32), (OverlayEncoding::Values, 12)] {
            assert!(each * large + MIB > 16 * MIB + decode, "{what}");
            for planes in [0, 1, 2, 4] {
                let work = DecodeWork::ValueOverlay {
                    target_rows: 3000,
                    target_columns: 5000,
                    planes,
                    encoding,
                };
                assert_eq!(
                    pixels::decode_estimate(&entry, work),
                    8 * u64::from(planes) * p + each * large + MIB,
                    "{what}, {planes} planes as {encoding:?}"
                );
            }
        }
    }
    // And the decode side, written out the same way.
    let small = entry(EXPLICIT_LE, 16);
    assert_eq!(
        overlay(&small, 2, OverlayEncoding::Values),
        16 * p + 16 * MIB + 6 * p
    );

    // Neither an entry nor a displayed frame nothing vouches for can
    // overflow the sums.
    let mut vast = entry(EXPLICIT_LE, 64);
    (vast.rows, vast.columns) = (u32::MAX, u32::MAX);
    for work in [
        DecodeWork::ValueLegend,
        DecodeWork::SegmentationOverlay {
            target_rows: 1,
            target_columns: 1,
        },
        DecodeWork::ValueOverlay {
            target_rows: 1,
            target_columns: 1,
            planes: u32::MAX,
            encoding: OverlayEncoding::Png,
        },
    ] {
        assert_eq!(pixels::decode_estimate(&vast, work), u64::MAX, "{work:?}");
    }
    let vast_target = DecodeWork::ValueOverlay {
        target_rows: u32::MAX,
        target_columns: u32::MAX,
        planes: u32::MAX,
        encoding: OverlayEncoding::Png,
    };
    assert_eq!(pixels::decode_estimate(&small, vast_target), u64::MAX);
    let vast_image = DecodeWork::SegmentationOverlay {
        target_rows: u32::MAX,
        target_columns: u32::MAX,
    };
    assert_eq!(pixels::decode_estimate(&small, vast_image), u64::MAX);
}

/// The overlay requests of the fixtures with what each reserves: a path,
/// and the estimate of its work.
fn overlay_requests(entries: &[FileEntry]) -> Vec<(String, u64)> {
    let on = |target: usize| (entries[target].rows, entries[target].columns);
    let dose = |target: usize, planes, suffix: &str, encoding| {
        (
            format!("/api/file/{target}/frame/0/dose-overlay{suffix}?dose={DOSE}"),
            value_overlay(&entries[DOSE], on(target), planes, encoding),
        )
    };
    vec![
        dose(DOSE_ON_A_PLANE, 1, "", OverlayEncoding::Png),
        dose(DOSE_BETWEEN_PLANES, 2, "", OverlayEncoding::Png),
        dose(DOSE_BETWEEN_PLANES, 2, "/values", OverlayEncoding::Values),
        (
            format!("/api/file/{MAP_BETWEEN_PLANES}/frame/0/parametric-map-overlay?map={MAP}"),
            value_overlay(
                &entries[MAP],
                on(MAP_BETWEEN_PLANES),
                2,
                OverlayEncoding::Png,
            ),
        ),
        (
            format!(
                "/api/file/{MAP_BETWEEN_PLANES}/frame/0/parametric-map-overlay/values?map={MAP}"
            ),
            value_overlay(
                &entries[MAP],
                on(MAP_BETWEEN_PLANES),
                2,
                OverlayEncoding::Values,
            ),
        ),
        (
            format!("/api/file/{SEG}/frame/0/segmentation-overlay"),
            segmentation_overlay(&entries[SEG], &entries[SEG_SOURCE]),
        ),
    ]
}

/// An overlay and a legend each reserve their documented estimate, once:
/// the frames they decode run under the same permit, so a viewer with one
/// permit serves them.
#[tokio::test]
async fn each_overlay_and_legend_reserves_its_documented_estimate_under_one_permit() {
    finishes(async {
        let entries = overlay_fixtures().await;
        let legends = [DOSE, MAP].map(|object| {
            (
                format!("/api/file/{object}/semantic-context"),
                pixels::decode_estimate(&entries[object], DecodeWork::ValueLegend),
            )
        });
        let overlays = overlay_requests(&entries);

        // One permit: a second, taken while the first was held, would
        // never be granted.
        let state = support::app_state(entries.clone());
        let scheduler = default_scheduler(1);
        let server = TestServer::new(server::router(
            state.clone().with_decode_scheduler(scheduler.clone()),
        ));
        for (path, _) in &legends {
            let response = eventually("a legend", async { server.get(path).await }).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
            let context: Value = response.json();
            assert_eq!(context["context"]["overlay"]["eligible"], true, "{context}");
            assert!(context["context"]["legend"].is_object(), "{context}");
        }
        let load = idle(&scheduler).await;
        assert_eq!(
            load.peak_reserved_bytes,
            legends[0].1.max(legends[1].1),
            "a legend reserves its estimate"
        );

        // The contexts are kept with their legends, so an overlay is now
        // the only work its request does. Each is drawn by a viewer with a
        // scheduler of its own, whose account is that overlay's alone.
        for (path, estimate) in &overlays {
            let scheduler = default_scheduler(1);
            let server = TestServer::new(server::router(
                state.clone().with_decode_scheduler(scheduler.clone()),
            ));
            let response = eventually("an overlay", async { server.get(path).await }).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
            assert_eq!(response.header("x-cache"), "MISS", "{path}");
            let load = idle(&scheduler).await;
            assert_eq!(
                load.peak_reserved_bytes, *estimate,
                "{path} reserves its estimate"
            );
            // Kept: asked again, it is served without a permit.
            let held = granted(&scheduler, Interactive, 0).await;
            let again = eventually("a cached overlay", async { server.get(path).await }).await;
            assert_eq!(again.status_code(), 200, "{path}");
            assert_eq!(again.header("x-cache"), "HIT", "{path}");
            assert_eq!(again.as_bytes(), response.as_bytes(), "{path}");
            drop(held);
        }
    })
    .await;
}

/// Requests for one overlay share one computation, and so do requests for
/// one legend: the queue has room for a single piece of work, the requests
/// arrive while it waits there, and none is turned away.
#[tokio::test]
async fn identical_overlay_requests_share_one_computation() {
    finishes(async {
        const REQUESTS: usize = 6;
        let entries = overlay_fixtures().await;
        let legend = pixels::decode_estimate(&entries[DOSE], DecodeWork::ValueLegend);
        let overlays = overlay_requests(&entries);
        let state = support::app_state(entries);
        let room_for_one =
            || DecodeScheduler::with_limits(1, limits(pixels::DECODE_MEMORY_DEFAULT_BYTES, 1, 0));

        // A legend. Its requests read the object's metadata first, each on
        // its own, so they reach the legend one after another.
        let scheduler = room_for_one();
        let server = Arc::new(TestServer::new(server::router(
            state.clone().with_decode_scheduler(scheduler.clone()),
        )));
        let held = granted(&scheduler, Interactive, 0).await;
        let path = format!("/api/file/{DOSE}/semantic-context");
        let requests: Vec<_> = (0..REQUESTS).map(|_| get(&server, &path)).collect();
        waiting(&scheduler, 1, 0).await;
        drop(held);
        for request in requests {
            assert_eq!(status(request).await, 200, "{path}");
        }
        let load = idle(&scheduler).await;
        assert_eq!(load.peak_reserved_bytes, legend, "one legend served them");

        // Overlays. The context is kept, so each request goes straight to
        // its overlay: given their turn (the test's runtime has one
        // thread), all of them have asked for it before it is admitted.
        for (path, estimate) in &overlays {
            let scheduler = room_for_one();
            let server = Arc::new(TestServer::new(server::router(
                state.clone().with_decode_scheduler(scheduler.clone()),
            )));
            let held = granted(&scheduler, Interactive, 0).await;
            let requests: Vec<_> = (0..REQUESTS)
                .map(|_| {
                    let (server, path) = (server.clone(), path.clone());
                    tokio::spawn(async move {
                        let response = server.get(&path).await;
                        (
                            response.status_code(),
                            response
                                .maybe_header("x-cache")
                                .map(|value| value.to_str().expect("header text").to_string()),
                        )
                    })
                })
                .collect();
            waiting(&scheduler, 1, 0).await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert_eq!(scheduler.load().waiting_interactive, 1, "{path}");
            drop(held);

            let mut computed = 0;
            for request in requests {
                let (status, cache) = eventually("a response", request)
                    .await
                    .expect("request task");
                assert_eq!(status, 200, "{path}");
                match cache.as_deref() {
                    Some("MISS") => computed += 1,
                    Some("HIT") => {}
                    other => panic!("{path}: X-Cache {other:?}"),
                }
            }
            assert_eq!(computed, 1, "{path}: one request computed the overlay");
            let load = idle(&scheduler).await;
            assert_eq!(
                load.peak_reserved_bytes, *estimate,
                "{path}: one overlay served them"
            );
        }
    })
    .await;
}

/// An overlay that needs more than the budget is refused for the request
/// with 422 and the budget's code, at once and every time; a busy viewer
/// answers 503 with `Retry-After`; and neither changes what the catalog or
/// the semantic context say. A legend over the budget makes its overlay
/// ineligible, naming the budget.
#[tokio::test]
async fn an_overlay_over_the_budget_is_refused_and_a_busy_viewer_asks_to_retry() {
    finishes(async {
        let entries = overlay_fixtures().await;
        let overlays = overlay_requests(&entries);
        let legend = [DOSE, MAP]
            .map(|object| pixels::decode_estimate(&entries[object], DecodeWork::ValueLegend));
        async fn renderable(server: &TestServer) {
            let files: Value = server.get("/api/files").await.json();
            for file in files["files"].as_array().expect("files") {
                assert_eq!(file["support_state"], "renderable", "{file}");
            }
        }

        let mut refused = 0;
        for (path, estimate) in &overlays {
            // A value overlay is drawn with its legend, so the budget must
            // leave room for that: the overlays of two planes need more
            // than their legend, the one of a single plane does not.
            if !path.contains("segmentation") && *estimate <= legend[0].max(legend[1]) {
                continue;
            }
            refused += 1;
            for round in 0..2 {
                let scheduler = DecodeScheduler::with_limits(2, limits(estimate - 1, 8, 8));
                let server = serve(entries.clone(), &scheduler);
                let response = server.get(path).await;
                let what = format!("{path} one byte short, round {round}");
                assert_eq!(response.status_code(), 422, "{what}: {}", response.text());
                let body: Value = response.json();
                assert_eq!(body["code"], "decode_memory_exceeded", "{what}");
                let text = body["error"].to_string();
                assert!(text.contains(&estimate.to_string()), "{what}: {text}");
                assert!(text.contains("--decode-memory"), "{what}: {text}");
                assert!(response.maybe_header("retry-after").is_none(), "{what}");
                assert!(response.maybe_header("x-cache").is_none(), "{what}");
                renderable(&server).await;
                assert!(idle(&scheduler).await.peak_reserved_bytes < *estimate);
            }

            // With exactly its estimate it is drawn.
            let scheduler = DecodeScheduler::with_limits(2, limits(*estimate, 8, 8));
            let server = serve(entries.clone(), &scheduler);
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
            assert_eq!(idle(&scheduler).await.peak_reserved_bytes, *estimate);

            // Busy: the budget is held and nothing may wait.
            let memory = 1 << 40;
            let scheduler = DecodeScheduler::with_limits(4, limits(memory, 0, 0));
            let server = serve(entries.clone(), &scheduler);
            let held = granted(&scheduler, Interactive, memory).await;
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 503, "{path}: {}", response.text());
            assert_eq!(response.json::<Value>()["code"], "decode_busy", "{path}");
            assert_eq!(
                response.header("retry-after"),
                DECODE_BUSY_RETRY_AFTER_SECONDS.to_string(),
                "{path}"
            );
            assert!(response.maybe_header("x-cache").is_none(), "{path}");
            drop(held);
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
            assert_eq!(response.header("x-cache"), "MISS", "{path}");
            idle(&scheduler).await;
        }
        assert_eq!(refused, overlays.len() - 1);

        // A legend one byte short of its estimate: the context is served,
        // its overlay is not eligible and says why, and the overlay
        // endpoint reports that instead of drawing.
        for (object, target, endpoint) in [
            (DOSE, DOSE_BETWEEN_PLANES, "dose-overlay?dose"),
            (MAP, MAP_BETWEEN_PLANES, "parametric-map-overlay?map"),
        ] {
            let estimate = pixels::decode_estimate(&entries[object], DecodeWork::ValueLegend);
            let scheduler = DecodeScheduler::with_limits(2, limits(estimate - 1, 8, 8));
            let server = serve(entries.clone(), &scheduler);
            let response = server
                .get(&format!("/api/file/{object}/semantic-context"))
                .await;
            assert_eq!(response.status_code(), 200, "{}", response.text());
            let context: Value = response.json();
            let overlay = &context["context"]["overlay"];
            assert_eq!(overlay["eligible"], false, "{context}");
            let reason = overlay["reason"].to_string();
            assert!(reason.contains("--decode-memory"), "{reason}");
            assert!(reason.contains(&estimate.to_string()), "{reason}");
            assert!(context["context"]["legend"].is_null(), "{context}");
            let response = server
                .get(&format!("/api/file/{target}/frame/0/{endpoint}={object}"))
                .await;
            assert_eq!(response.status_code(), 422, "{}", response.text());
            assert_eq!(
                response.json::<Value>()["code"],
                "semantic_mapping_unavailable"
            );
            renderable(&server).await;
            assert!(idle(&scheduler).await.peak_reserved_bytes < estimate);
        }
    })
    .await;
}

/// An overlay whose requests were all dropped while it waited leaves the
/// queue and is never drawn; asked for again, it is new work.
#[tokio::test]
async fn an_overlay_nobody_waits_for_any_longer_leaves_the_queue() {
    finishes(async {
        let entries = overlay_fixtures().await;
        let overlays = overlay_requests(&entries);
        let state = support::app_state(entries);
        // The contexts and their legends first, on a viewer of their own.
        let warm = TestServer::new(server::router(state.clone()));
        for object in [DOSE, MAP] {
            let response = warm
                .get(&format!("/api/file/{object}/semantic-context"))
                .await;
            assert_eq!(response.status_code(), 200, "{}", response.text());
        }

        let scheduler = default_scheduler(1);
        let server = Arc::new(TestServer::new(server::router(
            state.with_decode_scheduler(scheduler.clone()),
        )));
        let held = granted(&scheduler, Interactive, 0).await;
        for (path, _) in &overlays {
            let dropped = [get(&server, path), get(&server, path)];
            waiting(&scheduler, 1, 0).await;
            for request in dropped {
                request.abort();
                assert!(eventually("the abort", request).await.is_err());
            }
            waiting(&scheduler, 0, 0).await;
        }
        drop(held);
        let load = idle(&scheduler).await;
        assert_eq!(load.peak_reserved_bytes, 0, "nothing was drawn");
        for (path, _) in &overlays {
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            assert_eq!(response.header("x-cache"), "MISS", "{path}");
        }
        idle(&scheduler).await;
    })
    .await;
}

/// A viewer over `entries`, admitted by `scheduler`, with the registry it
/// serves, so a test can add a file to the file set.
fn growing_viewer(
    entries: Vec<FileEntry>,
    scheduler: &Arc<DecodeScheduler>,
) -> (FileRegistry, TestServer) {
    let registry = FileRegistry::from_files(entries);
    let state =
        support::app_state_with_registry(registry.clone()).with_decode_scheduler(scheduler.clone());
    (registry, TestServer::new(server::router(state)))
}

/// A scheduler on which nothing waits: with `HELD_BYTES` held, whatever
/// needs a permit is answered 503 and whatever is kept is served.
fn nothing_waits() -> Arc<DecodeScheduler> {
    DecodeScheduler::with_limits(4, limits(HELD_BYTES, 0, 0))
}

const HELD_BYTES: u64 = 1 << 40;

/// Asserts that `path` is answered 503: the viewer had to do work for it.
async fn assert_busy(server: &TestServer, path: &str) {
    let response = server.get(path).await;
    assert_eq!(response.status_code(), 503, "{path}");
    assert_eq!(response.json::<Value>()["code"], "decode_busy", "{path}");
    assert!(response.maybe_header("x-cache").is_none(), "{path}");
}

/// The legend `path`'s semantic context carries, which must be there.
async fn legend_of(server: &TestServer, path: &str) -> Value {
    let response = server.get(path).await;
    assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
    let legend = response.json::<Value>()["context"]["legend"].clone();
    assert!(legend.is_object(), "{path}: no legend");
    legend
}

/// The value range a legend spans is kept for its object and its file set
/// and for nothing else: another object's legend, and the same object's
/// once a file has been added, are found by decoding again, while the
/// legend of an object whose context the viewer no longer keeps is served
/// with the budget held.
#[tokio::test]
async fn a_legend_range_is_kept_for_its_object_and_file_set() {
    finishes(async {
        let mut entries = overlay_fixtures().await;
        let context = |object: usize| format!("/api/file/{object}/semantic-context");
        // Each legend as a viewer that is asked for nothing else gives it.
        let mut alone = Vec::new();
        for object in [DOSE, MAP] {
            let server = serve(entries.clone(), &default_scheduler(1));
            alone.push(legend_of(&server, &context(object)).await);
        }
        assert_ne!(alone[0]["max_value"], alone[1]["max_value"]);

        // Many more files than a viewer keeps semantic contexts for.
        let others = entries.len()..entries.len() + 96;
        let other = entries[DOSE_ON_A_PLANE].clone();
        entries.extend(others.clone().map(|_| other.clone()));
        let scheduler = nothing_waits();
        let (registry, server) = growing_viewer(entries, &scheduler);

        // Another object: the dose's range does not answer for the map.
        assert_eq!(legend_of(&server, &context(DOSE)).await, alone[0]);
        let held = granted(&scheduler, Interactive, HELD_BYTES).await;
        assert_busy(&server, &context(MAP)).await;
        drop(held);
        assert_eq!(legend_of(&server, &context(MAP)).await, alone[1]);
        assert_eq!(legend_of(&server, &context(DOSE)).await, alone[0]);

        // The same object again, once its context is no longer kept: the
        // contexts of the other files have taken its place. The range is
        // still there, so the legend needs no permit.
        let held = granted(&scheduler, Interactive, HELD_BYTES).await;
        for object in others {
            let response = server.get(&context(object)).await;
            assert_eq!(response.status_code(), 200, "{}", response.text());
        }
        for (object, legend) in [(DOSE, &alone[0]), (MAP, &alone[1])] {
            assert_eq!(&legend_of(&server, &context(object)).await, legend);
        }

        // Another file set: a range found before the file was added is not
        // the range of the set with it.
        registry.insert(other);
        for object in [DOSE, MAP] {
            assert_busy(&server, &context(object)).await;
        }
        drop(held);
        for (object, legend) in [(DOSE, &alone[0]), (MAP, &alone[1])] {
            assert_eq!(&legend_of(&server, &context(object)).await, legend);
        }
        idle(&scheduler).await;
    })
    .await;
}

/// A refusal is the answer to the requests that shared it and to no later
/// one. On one viewer, an overlay refused because the viewer was busy is
/// drawn when it is asked for again with room, and kept from then on; and
/// after an overlay that can never fit its budget is refused, one that fits
/// is drawn and kept, while the first is refused as before.
#[tokio::test]
async fn a_refused_overlay_is_not_kept_as_its_answer() {
    finishes(async {
        let entries = overlay_fixtures().await;
        let overlays = overlay_requests(&entries);
        let contexts = [DOSE, MAP].map(|object| format!("/api/file/{object}/semantic-context"));
        // What each overlay is, from a viewer that refuses nothing.
        let undisturbed = serve(entries.clone(), &default_scheduler(1));
        let mut bodies = Vec::new();
        for (path, _) in &overlays {
            let response = undisturbed.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
            assert!(!response.as_bytes().is_empty(), "{path}");
            bodies.push(response.as_bytes().clone());
        }
        async fn drawn_then_kept(server: &TestServer, path: &str, body: &[u8]) {
            for cache in ["MISS", "HIT"] {
                let response = server.get(path).await;
                assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
                assert_eq!(response.header("x-cache"), cache, "{path}");
                assert_eq!(response.as_bytes().as_ref(), body, "{path}");
            }
        }

        // Busy. The legends are found first, so the overlay's own work is
        // what is refused.
        let scheduler = nothing_waits();
        let server = serve(entries.clone(), &scheduler);
        for path in &contexts {
            legend_of(&server, path).await;
        }
        for ((path, _), body) in overlays.iter().zip(&bodies) {
            let held = granted(&scheduler, Interactive, HELD_BYTES).await;
            assert_busy(&server, path).await;
            drop(held);
            drawn_then_kept(&server, path, body).await;
        }
        idle(&scheduler).await;

        // Over the budget. The overlay that reserves least fits every
        // budget here, and so do the legends.
        let (least, (fits, _)) = overlays
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, estimate))| *estimate)
            .expect("overlays");
        let mut refused = 0;
        for (path, estimate) in &overlays {
            if *estimate == overlays[least].1 {
                continue;
            }
            refused += 1;
            let scheduler = DecodeScheduler::with_limits(2, limits(estimate - 1, 8, 8));
            let server = serve(entries.clone(), &scheduler);
            for round in 0..2 {
                let response = server.get(path).await;
                assert_eq!(response.status_code(), 422, "{path}: {}", response.text());
                assert_eq!(
                    response.json::<Value>()["code"],
                    "decode_memory_exceeded",
                    "{path}, round {round}"
                );
                assert!(response.maybe_header("x-cache").is_none(), "{path}");
                if round == 0 {
                    drawn_then_kept(&server, fits, &bodies[least]).await;
                }
            }
            idle(&scheduler).await;
        }
        assert!(refused >= 4, "only {refused} overlays were over a budget");
    })
    .await;
}

/// The committed fixture `fixture` written to `dir` as `name` with `rows`
/// and `columns` and, if given, that many frames. Its geometry is kept: a
/// grid of the same spacing from the same origin. No sample is zero.
fn resized(
    dir: &Path,
    name: &str,
    fixture_name: &str,
    (rows, columns): (u16, u16),
    frames: Option<u32>,
) -> PathBuf {
    let mut object = dicom_object::open_file(fixture(fixture_name)).expect("open fixture");
    for (tag, value) in [(tags::ROWS, rows), (tags::COLUMNS, columns)] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    if let Some(frames) = frames {
        object.put(DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            frames.to_string().as_str(),
        ));
    }
    let number = |tag| {
        object
            .element_opt(tag)
            .expect("read element")
            .map_or(1, |element| element.to_int::<u64>().expect("a number"))
    };
    let bits = u64::from(rows)
        * u64::from(columns)
        * number(tags::NUMBER_OF_FRAMES)
        * number(tags::BITS_ALLOCATED);
    let length = bits.div_ceil(16) * 2;
    let vr = object.element(tags::PIXEL_DATA).expect("pixel data").vr();
    let samples: Vec<u8> = (0..length).map(|index| (index % 7) as u8 + 1).collect();
    object.put(DataElement::new(
        tags::PIXEL_DATA,
        vr,
        PrimitiveValue::from(samples),
    ));
    let path = dir.join(name);
    object.write_to_file(&path).expect("write DICOM");
    path
}

/// An overlay is sized by its own object and by the displayed frame it is
/// drawn on, each for its own part: with a small object on a large frame
/// and a large object on a small frame, each overlay reserves the
/// documented bytes of that pair, which are not those of the pair the
/// other way round.
#[tokio::test]
async fn an_overlay_reserves_for_its_object_and_for_the_displayed_frame() {
    const MIB: u64 = 1024 * 1024;
    const DOSE_FIXTURE: &str = "golden-rtdose-u16-grid.dcm";
    const SLICE_FIXTURE: &str = "golden-rtdose-ct-source-z0.dcm";
    const SEG_FIXTURE: &str = "golden-seg-binary.dcm";
    const SEG_SOURCE_FIXTURE: &str = "golden-seg-binary-source.dcm";
    // The documented rows, written out: `p` pixels of the object, `t` of
    // the displayed frame. A dose is 16-bit and a binary SEG one-bit, both
    // uncompressed, and a slice on a plane of the dose samples that plane.
    let dose_as_png = |p: u64, t: u64| 8 * p + (16 * MIB + 6 * p).max(32 * t + MIB);
    let dose_as_values = |p: u64, t: u64| 8 * p + (16 * MIB + 6 * p).max(12 * t + MIB);
    let painted = |p: u64, t: u64| 16 * MIB + 3 * p + 24 * t + MIB;
    type Row<'a> = (&'a str, &'a dyn Fn(u64, u64) -> u64);
    let dose: [Row; 2] = [
        ("/api/file/1/frame/0/dose-overlay?dose=0", &dose_as_png),
        (
            "/api/file/1/frame/0/dose-overlay/values?dose=0",
            &dose_as_values,
        ),
    ];
    let seg: [Row; 1] = [("/api/file/0/frame/0/segmentation-overlay", &painted)];
    // (the object's fixture and size, the displayed frame's, the requests)
    type Case<'a> = (&'a str, (u16, u16), &'a str, (u16, u16), &'a [Row<'a>]);
    let cases: [Case; 4] = [
        (DOSE_FIXTURE, (4, 4), SLICE_FIXTURE, (1200, 1300), &dose),
        (DOSE_FIXTURE, (600, 700), SLICE_FIXTURE, (10, 10), &dose),
        (SEG_FIXTURE, (2, 2), SEG_SOURCE_FIXTURE, (500, 600), &seg),
        (SEG_FIXTURE, (300, 400), SEG_SOURCE_FIXTURE, (2, 2), &seg),
    ];
    finishes(async {
        for (object, object_size, target, target_size, requests) in cases {
            let dir = tempdir().expect("temp dir");
            let entries = listed(&[
                resized(dir.path(), "object.dcm", object, object_size, None),
                resized(dir.path(), "target.dcm", target, target_size, None),
            ])
            .await;
            let pixels = |(rows, columns): (u16, u16)| u64::from(rows) * u64::from(columns);
            let (p, t) = (pixels(object_size), pixels(target_size));
            let what = format!("{object} of {object_size:?} on {target} of {target_size:?}");
            assert_eq!(
                (entries[0].rows, entries[0].columns),
                (object_size.0.into(), object_size.1.into()),
                "{what}"
            );
            assert_eq!(
                (entries[1].rows, entries[1].columns),
                (target_size.0.into(), target_size.1.into()),
                "{what}"
            );

            // The context and its legend first, on a viewer of their own.
            let state = support::app_state(entries);
            let warm = TestServer::new(server::router(state.clone()));
            let response = warm.get("/api/file/0/semantic-context").await;
            assert_eq!(response.status_code(), 200, "{what}: {}", response.text());

            for (path, documented) in requests {
                let estimate = documented(p, t);
                assert_ne!(
                    estimate,
                    documented(t, p),
                    "{what}: {path} tells the two apart"
                );
                let scheduler = default_scheduler(1);
                let server = TestServer::new(server::router(
                    state.clone().with_decode_scheduler(scheduler.clone()),
                ));
                let response = eventually("an overlay", async { server.get(path).await }).await;
                assert_eq!(
                    response.status_code(),
                    200,
                    "{what}, {path}: {}",
                    response.text()
                );
                assert_eq!(
                    idle(&scheduler).await.peak_reserved_bytes,
                    estimate,
                    "{what}: {path}"
                );
            }
        }
    })
    .await;
}

/// An overlay is kept for the frame it was drawn on and the frame it was
/// drawn from: a value overlay on another frame of the displayed file, and
/// another frame of a SEG on its own source frame, are drawn again and not
/// served from the first one's entry.
#[tokio::test]
async fn an_overlay_of_another_frame_is_drawn_again() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let entries = listed(&[
            fixture("golden-rtdose-u16-grid.dcm"),
            // The slice on the dose's first plane, as a file of two frames.
            resized(
                dir.path(),
                "two-frames.dcm",
                "golden-rtdose-ct-source-z0.dcm",
                (10, 10),
                Some(2),
            ),
            fixture("golden-seg-binary.dcm"),
            fixture("golden-seg-binary-source.dcm"),
        ])
        .await;
        assert_eq!(entries[1].frame_count, 2);
        // (an overlay, the same overlay of another frame)
        let cases = [
            (
                "/api/file/1/frame/0/dose-overlay?dose=0",
                "/api/file/1/frame/1/dose-overlay?dose=0",
            ),
            (
                "/api/file/1/frame/0/dose-overlay/values?dose=0",
                "/api/file/1/frame/1/dose-overlay/values?dose=0",
            ),
            (
                "/api/file/2/frame/0/segmentation-overlay",
                "/api/file/2/frame/1/segmentation-overlay",
            ),
        ];
        let scheduler = nothing_waits();
        let server = serve(entries, &scheduler);
        legend_of(&server, "/api/file/0/semantic-context").await;
        let answer = |path: &'static str| {
            let server = server.clone();
            async move {
                let response = server.get(path).await;
                assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
                response.header("x-cache")
            }
        };
        for (first, other) in cases {
            assert_eq!(answer(first).await, "MISS", "{first}");
            // With the budget held, only what is kept is served.
            let held = granted(&scheduler, Interactive, HELD_BYTES).await;
            assert_eq!(answer(first).await, "HIT", "{first}");
            assert_busy(&server, other).await;
            drop(held);
            assert_eq!(answer(other).await, "MISS", "{other}");
            for path in [first, other] {
                assert_eq!(answer(path).await, "HIT", "{path}");
            }
        }
        idle(&scheduler).await;
    })
    .await;
}
