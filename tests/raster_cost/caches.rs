//! What the frame caches bill their entries against what those entries
//! hold (`pixels::BudgetedLru`, `--cache-budget`).
//!
//! A cache keeps to its budget by the bytes it bills, which is the length
//! of each body. The budget only bounds memory if a body holds no more than
//! its length, and a body is built in a buffer that grew: an encoder's
//! output has room for up to twice what was written to it. So each kind of
//! body the viewer caches is asked for through the API, and the heap the
//! viewer gives back when it is dropped is compared with what its caches
//! said they held.

use super::admission::{striped_shutter, Native};
use super::bounds::noise;
use super::heap::{self, CountedRuntime};
use super::overlays::{self, context_path, roomy_scheduler, Listed, NO_CACHES, SIDE};
use dcmview::pixels::CacheBudget;
use dcmview::server;

/// What a viewer's caches may hold beside the bodies they bill: a key, the
/// metadata of a raw frame and the list's own links for each entry. A fixed
/// 64 KiB for the few entries of one row.
const ENTRY_OVERHEAD: i64 = 64 * 1024;

/// Asks a viewer over `listed` with caches of `budget` for `paths`, one
/// after another, and returns what its caches bill their entries
/// afterwards and the heap the viewer gave back when it was dropped.
fn billed_and_held(
    runtime: &CountedRuntime,
    listed: &Listed,
    budget: CacheBudget,
    paths: &[String],
) -> (u64, i64) {
    let scheduler = roomy_scheduler();
    let state = listed
        .state_keeping(budget)
        .with_decode_scheduler(scheduler.clone());
    runtime.peak_during(async {
        let server = axum_test::TestServer::new(server::router(state.clone()));
        for path in paths {
            let response = server.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
        }
    });
    runtime.peak_during(scheduler.load_when(|load| load.running == 0));
    let billed = state.cached_bytes().total_bytes();
    let before = heap::held();
    drop(state);
    (billed, before - heap::held())
}

/// Every kind of body the viewer caches is billed at least what it holds:
/// display PNGs, raw frames, thumbnails, presentation layers, and overlays
/// as images and as values.
///
/// What the caches hold is the heap a viewer gives back less what the same
/// viewer gives back after the same requests when it caches nothing (its
/// catalog, and the contexts and legends of the objects it was asked
/// about).
#[test]
fn a_cache_bills_its_entries_what_they_hold() {
    let mut files = overlays::files();
    let layer = Native::gray(SIDE, 8).with(striped_shutter(SIDE));
    files.push(("layer.dcm", layer.file(noise(layer.frame_bytes()))));
    let listed = Listed::of(files, SIDE);

    let frame =
        |file: &str, rest: &str| vec![format!("/api/file/{}/frame/0{rest}", listed.index(file))];
    let overlay = |endpoint: &str, parameter: &str, target: &str, object: &str| {
        vec![
            context_path(&listed, object),
            format!(
                "/api/file/{}/frame/0/{endpoint}?{parameter}={}",
                listed.index(target),
                listed.index(object)
            ),
        ]
    };
    // (the body, the requests that leave it cached)
    let rows = [
        ("a display frame", frame("seg-binary-source.dcm", "")),
        (
            "a display frame, windowed",
            frame("layer.dcm", "?wc=100&ww=50"),
        ),
        ("a raw frame", frame("dose-on-plane.dcm", "/raw")),
        ("a thumbnail", frame("layer.dcm", "/thumbnail?size=1024")),
        (
            "a presentation layer",
            frame("layer.dcm", "/presentation-layer"),
        ),
        (
            "a segmentation overlay",
            frame("seg-fractional.dcm", "/segmentation-overlay"),
        ),
        (
            "a dose overlay",
            overlay("dose-overlay", "dose", "dose-on-plane.dcm", "dose.dcm"),
        ),
        (
            "a map overlay as values",
            overlay(
                "parametric-map-overlay/values",
                "map",
                "map-between.dcm",
                "map.dcm",
            ),
        ),
    ];

    let runtime = CountedRuntime::new();
    for (name, paths) in &rows {
        let (billed, with_caches) = billed_and_held(&runtime, &listed, CacheBudget::DEFAULT, paths);
        let (nothing, without_caches) = billed_and_held(&runtime, &listed, NO_CACHES, paths);
        assert_eq!(nothing, 0, "{name}: a viewer without caches keeps nothing");
        let held = with_caches - without_caches;
        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!(
                "{name}: billed {billed}, held {held} ({:.3} of it)",
                held as f64 / billed.max(1) as f64
            );
        }
        // The measurement is of a body: each row caches tens of kilobytes
        // at the least, and its caches hold all they bill.
        assert!(billed >= 32 * 1024, "{name}: only {billed} bytes cached");
        assert!(
            held >= billed as i64,
            "{name}: {held} bytes held with {billed} billed"
        );
        assert!(
            held <= billed as i64 + ENTRY_OVERHEAD,
            "{name}: the caches hold {held} bytes and bill {billed}"
        );
    }
}
