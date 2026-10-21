//! Opt-in measurements of gallery thumbnails: the delay they add to the
//! viewer (`pixels::INTERACTIVE_LATENCY_TARGET`), and how much faster a
//! thumbnail decoded at a reduced resolution is than one decoded in full
//! (`pixels::thumbnail_reduction`). They need a release build to mean
//! anything:
//!
//! ```text
//! cargo test --release --test integration thumbnail_timing -- --ignored --nocapture
//! ```
//!
//! Each always prints what it measured. The latency target fails only in a
//! release build on a host with at least four cores, which is where it is
//! defined; the reduced decode only in a release build.

use super::codestream_files as files;
use super::support;
use axum_test::TestServer;
use dcmview::pixels::INTERACTIVE_LATENCY_TARGET;
use dcmview::server;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const EDGE: u16 = 2048;
const VIEWER_FILES: usize = 4;
/// Display frames timed in each phase.
const SAMPLES: usize = 120;
/// Thumbnail requests kept in flight: four for every core, so the decode
/// pool is always oversubscribed and a thumbnail is always waiting when a
/// display frame arrives. The gallery's own scheduler keeps four in flight,
/// which is this much on a one-core host and less on any other. Capped to
/// bound the temporary files (one 8 MB file per request in flight).
fn gallery_requests_in_flight() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    (4 * cores).clamp(4, 32)
}

/// A server over 2048x2048 16-bit frames of noise over a ramp, which give
/// the windowing, PNG and JPEG steps real work.
fn serve(dir: &std::path::Path) -> TestServer {
    let mut noise = 0x2545_f491_u32;
    let files = (0..VIEWER_FILES + gallery_requests_in_flight())
        .map(|index| {
            let path = dir.join(format!("timing-{index}.dcm"));
            let samples = (0..usize::from(EDGE) * usize::from(EDGE))
                .map(|position| {
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    ((position % usize::from(EDGE)) as u16) + (noise % 512) as u16
                })
                .collect();
            support::write_uncompressed_u16_dicom(
                &path,
                "1.2.840.10008.1.2.1",
                EDGE,
                EDGE,
                samples,
                None,
                None,
            );
            let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
            entry.index = index;
            entry.rows = u32::from(EDGE);
            entry.columns = u32::from(EDGE);
            entry
        })
        .collect();
    TestServer::new(server::router(support::app_state(files)))
}

/// Times `SAMPLES` display frames no cache holds: each asks for a window
/// nothing asked for before, over samples the raw cache already has.
async fn time_display_frames(server: &TestServer, first_window: usize) -> Vec<Duration> {
    let mut latencies = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let file = sample % VIEWER_FILES;
        let center = 1000 + first_window + sample;
        let started = Instant::now();
        let response = server
            .get(&format!("/api/file/{file}/frame/0?wc={center}&ww=2000"))
            .await;
        latencies.push(started.elapsed());
        response.assert_status_ok();
        assert_eq!(response.header("x-cache"), "MISS");
    }
    latencies
}

fn percentile(latencies: &[Duration], percent: usize) -> Duration {
    let mut sorted = latencies.to_vec();
    sorted.sort();
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

/// Keeps cold thumbnail requests coming until `stop`: every bucket and
/// window mode of this worker's own file, made cold again by moving the
/// file's redaction box. Returns how long each thumbnail took.
async fn load_thumbnails(
    server: Arc<TestServer>,
    worker: usize,
    stop: Arc<AtomicBool>,
) -> Vec<Duration> {
    let mut rendered = Vec::new();
    let file = VIEWER_FILES + worker;
    for pass in 0.. {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let boxes = json!({
            "num_roi": 1,
            "roi_coords": [[0, 0, 1, 1 + pass % 1000]],
            "roi_frames": [],
        });
        server
            .put(&format!("/api/file/{file}/redactions"))
            .json(&boxes)
            .await
            .assert_status_ok();
        for size in [128, 256, 512, 1024] {
            for mode in ["default", "full_dynamic"] {
                if stop.load(Ordering::Relaxed) {
                    return rendered;
                }
                let started = Instant::now();
                let response = server
                    .get(&format!(
                        "/api/file/{file}/frame/0/thumbnail?size={size}&window_mode={mode}"
                    ))
                    .await;
                rendered.push(started.elapsed());
                response.assert_status_ok();
                assert_eq!(response.header("x-cache"), "MISS");
            }
        }
    }
    rendered
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing measurement; run in a release build with --ignored --nocapture"]
async fn interactive_frames_stay_within_the_latency_target_while_thumbnails_load() {
    let dir = tempfile::tempdir().expect("temp dir");
    let server = Arc::new(serve(dir.path()));
    for file in 0..VIEWER_FILES {
        server
            .get(&format!("/api/file/{file}/frame/0/raw"))
            .await
            .assert_status_ok();
    }

    let idle = time_display_frames(&server, 0).await;

    let stop = Arc::new(AtomicBool::new(false));
    let gallery = (0..gallery_requests_in_flight())
        .map(|worker| tokio::spawn(load_thumbnails(server.clone(), worker, stop.clone())))
        .collect::<Vec<_>>();
    // Let the gallery reach its steady state before timing the viewer.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let loaded = time_display_frames(&server, SAMPLES).await;
    stop.store(true, Ordering::Relaxed);
    let mut thumbnails = Vec::new();
    for worker in gallery {
        thumbnails.extend(worker.await.expect("gallery worker"));
    }
    assert!(!thumbnails.is_empty(), "the gallery rendered nothing");

    let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
    let extra = percentile(&loaded, 95).saturating_sub(percentile(&idle, 95));
    println!(
        "thumbnail timing: {cores} cores, {} build, {} thumbnails rendered under load (p50 {:?} each)",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        thumbnails.len(),
        percentile(&thumbnails, 50),
    );
    for (phase, latencies) in [("idle", &idle), ("under thumbnail load", &loaded)] {
        println!(
            "  display frame {phase}: p50 {:?}, p95 {:?}, max {:?}",
            percentile(latencies, 50),
            percentile(latencies, 95),
            percentile(latencies, 100),
        );
    }
    println!("  extra p95: {extra:?} (target {INTERACTIVE_LATENCY_TARGET:?})");

    if cfg!(debug_assertions) || cores < 4 {
        println!("  not enforced: the target is for a release build on four cores or more");
        return;
    }
    assert!(
        extra <= INTERACTIVE_LATENCY_TARGET,
        "thumbnails delayed the viewer by {extra:?} at p95"
    );
}

/// Frames timed each way by the reduced-decode measurement.
const REDUCED_FRAMES: usize = 12;

/// The time of each of `REDUCED_FRAMES` cold thumbnails at 128 pixels of a
/// file whose frames are all `codestream`, 2080 x 1560 with a stored window:
/// first decoded at a reduced resolution (the default window), then in full
/// (`full_dynamic`, which spans the frame's own extremes and so needs every
/// sample).
async fn time_reduced_and_full(
    transfer_syntax: &str,
    bits: u16,
    codestream: &str,
) -> (Vec<Duration>, Vec<Duration>) {
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::tags;

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("frames.dcm");
    let frame =
        std::fs::read(files::fixture("reduced-decode").join(codestream)).expect("the fixture");
    let full_scale = 1_u32 << bits;
    files::write_with(
        &path,
        transfer_syntax,
        files::Layout::gray(1560, 2080, if bits > 8 { 16 } else { 8 }, bits),
        &vec![frame; REDUCED_FRAMES],
        &[
            DataElement::new(
                tags::WINDOW_CENTER,
                VR::DS,
                PrimitiveValue::from((full_scale / 2).to_string()),
            ),
            DataElement::new(
                tags::WINDOW_WIDTH,
                VR::DS,
                PrimitiveValue::from(full_scale.to_string()),
            ),
        ],
    );
    let server = files::serve(files::list(&[path]).await);
    let mut timed = Vec::new();
    for (mode, source) in [
        ("default", "reduced_decode"),
        ("full_dynamic", "full_decode"),
    ] {
        let mut latencies = Vec::with_capacity(REDUCED_FRAMES);
        for frame in 0..REDUCED_FRAMES {
            let started = Instant::now();
            let response = server
                .get(&format!(
                    "/api/file/0/frame/{frame}/thumbnail?size=128&window_mode={mode}"
                ))
                .await;
            latencies.push(started.elapsed());
            response.assert_status_ok();
            assert_eq!(response.header("x-cache"), "MISS");
            assert_eq!(response.header("x-thumbnail-source"), source, "{mode}");
        }
        timed.push(latencies);
    }
    let full = timed.pop().expect("full");
    (timed.pop().expect("reduced"), full)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing measurement; run in a release build with --ignored --nocapture"]
async fn reduced_thumbnails_are_faster_than_full_decodes() {
    println!(
        "reduced thumbnail timing: {} build, {REDUCED_FRAMES} cold thumbnails of 2080 x 1560 \
         frames each way, one at a time",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    let mut ratios = Vec::new();
    for (codec, transfer_syntax, bits, codestream) in [
        (
            "JPEG 2000, five levels, an eighth of the size",
            files::JPEG_2000,
            12,
            "gray12-2080x1560.j2k",
        ),
        (
            "JPEG, an eighth of the size",
            files::JPEG_BASELINE,
            8,
            "gray8-restart-2080x1560.jpg",
        ),
    ] {
        let (reduced, full) = time_reduced_and_full(transfer_syntax, bits, codestream).await;
        let (reduced, full) = (percentile(&reduced, 50), percentile(&full, 50));
        let ratio = full.as_secs_f64() / reduced.as_secs_f64().max(1e-9);
        println!("  {codec}: reduced p50 {reduced:?}, full p50 {full:?}, {ratio:.1} times faster");
        ratios.push(ratio);
    }

    if cfg!(debug_assertions) {
        println!("  not enforced: the comparison is for a release build");
        return;
    }
    assert!(
        ratios[0] >= 2.0,
        "a JPEG 2000 thumbnail decoded at an eighth of the size was {:.1} times faster than \
         one decoded in full",
        ratios[0]
    );
}
