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
use dcmview::server;
use dcmview::types::FileEntry;
use serde_json::{json, Value};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    let cases: [(&str, u32, u32, u64, u64, u64, u64); 16] = [
        // Native, 16-bit gray: F = 2 P; decode 3 F.
        (EXPLICIT_LE, 1, 16, 6 * p, 0, p, 2 * p),
        ("1.2.840.10008.1.2", 1, 8, 3 * p, 0, p, p),
        // One bit a sample is served a byte a sample.
        (EXPLICIT_LE, 1, 1, 3 * p, 0, p, p),
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

/// A thumbnail or a preview that is dropped while it waits starts no work
/// and reserves nothing; a display frame that is dropped while it waits is
/// still decoded and cached, as before.
#[tokio::test]
async fn a_request_dropped_while_it_waits_reserves_nothing() {
    finishes(async {
        let dir = tempdir().expect("temp dir");
        let entry = listed_raster(dir.path(), "image.png", &gray_png(64)).await;
        let display = pixels::decode_estimate(&entry, DecodeWork::DisplayFrame);
        let scheduler = default_scheduler(1);
        let server = serve(vec![entry], &scheduler);
        let held = granted(&scheduler, Interactive, 0).await;

        for (path, queues) in [
            ("/api/file/0/frame/0/thumbnail", (0, 1)),
            ("/api/file/0/frame/0?preview=true&wc=10&ww=20", (1, 0)),
        ] {
            let dropped = get(&server, path);
            waiting(&scheduler, queues.0, queues.1).await;
            dropped.abort();
            assert!(eventually("the abort", dropped).await.is_err());
            let load = scheduler.load();
            assert_eq!(
                (load.waiting_interactive, load.waiting_background),
                (0, 0),
                "{path}"
            );
        }
        let kept = get(&server, "/api/file/0/frame/0");
        waiting(&scheduler, 1, 0).await;
        kept.abort();
        assert!(eventually("the abort", kept).await.is_err());
        assert_eq!(
            scheduler.load().waiting_interactive,
            1,
            "a display frame stays queued"
        );

        drop(held);
        let load = idle(&scheduler).await;
        assert_eq!(
            load.peak_reserved_bytes, display,
            "only the display frame was decoded"
        );
        for (path, cache) in [
            ("/api/file/0/frame/0", "HIT"),
            ("/api/file/0/frame/0/thumbnail", "MISS"),
        ] {
            let response = server.get(path).await;
            assert_eq!(response.header("x-cache"), cache, "{path}");
        }
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
            assert_eq!(response.json::<Value>()["code"], "pixel_decode_failed");
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
