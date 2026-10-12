//! What a thumbnail decoded at a reduced resolution holds, against what it
//! reserved (`pixels::thumbnail_reduction`, `DecodeWork::Thumbnail`).
//!
//! A reduced thumbnail reserves what a full one does, because the full
//! decode is its fallback under the same permit. So the rule to hold is
//! that no path of a reducible frame holds more than that reservation:
//! the reduced decode, the full decode of the same frame, and a reduced
//! attempt that fails and is followed by the full decode. Each is run for
//! JPEG frames at sizes just past a power of two, for the layouts the
//! decoder treats differently, and for frames with their data damaged, and
//! the heap of every thread of the runtime that worked on it is counted.
//!
//! JPEG 2000 is absent, for the reason `tests/codestream_cost` gives: its
//! decoder cannot run on the counting allocator of a debug build, and
//! takes most of its memory from the C allocator, which is not counted.
//! `tests/integration/reduced_thumbnails.rs` covers what its reduced path
//! serves, and `tests/codestream_cost` that it refuses a frame before the
//! decoder as a full decode does.

use super::bounds::MIB;
use super::codestream_files::{self as files, Layout};
use super::heap::CountedRuntime;
use dcmview::api::contracts::{ThumbnailSource, WindowMode};
use dcmview::pixels::{
    self, DecodeLimits, DecodeScheduler, DecodeWork, PixelResult, Redaction, ThumbnailCache,
    ThumbnailRequest, ThumbnailResponse,
};
use dcmview::types::FileEntry;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use std::sync::{Arc, Mutex};

/// What a request may hold beside its reservation: the tasks, handles and
/// messages of the request itself, whatever the frame.
const REQUEST_OVERHEAD: u64 = MIB;

/// Bytes that look like nothing in particular, the same on every run.
fn texture(count: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    (0..count)
        .map(|index| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            // A ramp under the noise, so the frame is a picture.
            ((index % 251) as u8 / 2).wrapping_add((state % 96) as u8)
        })
        .collect()
}

/// A baseline JPEG of `columns` by `rows`, gray or colour, written by the
/// encoder the viewer links.
fn baseline_jpeg(columns: u32, rows: u32, colour: bool) -> Vec<u8> {
    let samples = if colour { 3 } else { 1 };
    let pixels = texture((columns * rows * samples) as usize);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(
            &pixels,
            columns,
            rows,
            if colour {
                image::ExtendedColorType::Rgb8
            } else {
                image::ExtendedColorType::L8
            },
        )
        .expect("encode JPEG");
    jpeg
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(files::fixture("reduced-decode").join(name)).expect("the fixture")
}

/// The catalog entry of a JPEG Baseline file of one `frame` whose header
/// says `columns` by `rows`, with a stored window when it is gray.
fn listed(columns: u32, rows: u32, colour: bool, frame: Vec<u8>) -> (FileEntry, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("frame.dcm");
    let layout = if colour {
        Layout::color8(rows as u16, columns as u16, "YBR_FULL_422")
    } else {
        Layout::gray8(rows as u16, columns as u16)
    };
    let window = [
        DataElement::new(tags::WINDOW_CENTER, VR::DS, PrimitiveValue::from("128")),
        DataElement::new(tags::WINDOW_WIDTH, VR::DS, PrimitiveValue::from("256")),
    ];
    files::write_with(&path, files::JPEG_BASELINE, layout, &[frame], &window);
    let entry = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(files::list(&[path]))
        .remove(0);
    (entry, dir)
}

/// The thumbnail of `entry` at 128 pixels in `mode`, from a cold cache.
async fn thumbnail(
    entry: Arc<FileEntry>,
    mode: WindowMode,
    scheduler: Arc<DecodeScheduler>,
) -> PixelResult<ThumbnailResponse> {
    let cache = Arc::new(Mutex::new(ThumbnailCache::with_scheduler(
        pixels::THUMBNAIL_CACHE_MAX_BYTES,
        scheduler,
    )));
    let request = ThumbnailRequest {
        frame: 0,
        bucket: 128,
        window_mode: mode,
    };
    pixels::load_thumbnail(entry, cache, request, Redaction::default()).await
}

fn roomy_scheduler() -> Arc<DecodeScheduler> {
    DecodeScheduler::with_limits(
        4,
        DecodeLimits {
            memory_bytes: 1 << 50,
            ..DecodeLimits::DEFAULT
        },
    )
}

/// Runs the thumbnail of `entry` in `mode` and returns its outcome with the
/// heap it held, having checked that it reserved the thumbnail estimate and
/// held no more than the part of it that grows with the frame.
fn measured(
    runtime: &CountedRuntime,
    name: &str,
    entry: &Arc<FileEntry>,
    mode: WindowMode,
) -> (Option<ThumbnailSource>, u64) {
    let scheduler = roomy_scheduler();
    let (response, heap) = runtime.peak_during(thumbnail(entry.clone(), mode, scheduler.clone()));
    let load = runtime
        .peak_during(scheduler.load_when(|load| load.running == 0))
        .0;
    let estimate = pixels::decode_estimate(entry, DecodeWork::Thumbnail);
    assert_eq!(
        load.peak_reserved_bytes, estimate,
        "{name} {mode:?} reserves the thumbnail estimate"
    );
    let growing = estimate - pixels::DICOM_DECODE_BASE_BYTES;
    if std::env::var_os("RASTER_COST_REPORT").is_some() {
        eprintln!(
            "{name} {mode:?}: heap {heap}, {:.3} of the frame's part of {estimate} reserved, {:?}",
            heap as f64 / (growing + REQUEST_OVERHEAD) as f64,
            response.as_ref().map(|response| response.source).ok()
        );
    }
    assert!(
        heap <= growing + REQUEST_OVERHEAD,
        "{name} {mode:?}: {heap} bytes of heap held with {estimate} reserved, \
         {growing} of it for the frame"
    );
    (response.ok().map(|response| response.source), heap)
}

/// Honest frames: a reduced thumbnail holds no more than it reserved, and
/// less than the full decode of the same frame, which a `full_dynamic`
/// thumbnail is.
#[test]
fn a_reduced_thumbnail_holds_less_than_the_full_decode_it_reserved_for() {
    // (name, columns, rows, colour, frame)
    let frames: Vec<(&str, u32, u32, bool, Vec<u8>)> = vec![
        // Just past a power of two along one axis and along both.
        (
            "1025 x 1025",
            1025,
            1025,
            false,
            baseline_jpeg(1025, 1025, false),
        ),
        (
            "2049 x 1030",
            2049,
            1030,
            false,
            baseline_jpeg(2049, 1030, false),
        ),
        (
            "1030 x 2049",
            1030,
            2049,
            false,
            baseline_jpeg(1030, 2049, false),
        ),
        (
            "colour 1025 x 1031",
            1025,
            1031,
            true,
            baseline_jpeg(1025, 1031, true),
        ),
        (
            "progressive",
            1041,
            779,
            false,
            fixture("gray8-progressive-1041x779.jpg"),
        ),
        (
            "4:2:0 colour",
            1041,
            779,
            true,
            fixture("rgb8-420-1041x779.jpg"),
        ),
        (
            "progressive colour",
            1040,
            780,
            true,
            fixture("rgb8-444-progressive-1040x780.jpg"),
        ),
        (
            "restart intervals",
            2080,
            1560,
            false,
            fixture("gray8-restart-2080x1560.jpg"),
        ),
    ];
    std::thread::scope(|scope| {
        for (name, columns, rows, colour, frame) in frames {
            scope.spawn(move || {
                let (entry, _dir) = listed(columns, rows, colour, frame);
                let entry = Arc::new(entry);
                // Each file is measured on a runtime of its own, so they
                // can run side by side without sharing a count.
                let runtime = CountedRuntime::new();
                let (source, reduced) = measured(&runtime, name, &entry, WindowMode::Default);
                assert_eq!(source, Some(ThumbnailSource::ReducedDecode), "{name}");
                // Colour has no window, so every thumbnail of it is
                // reduced; a gray frame's full dynamic one is not.
                if colour {
                    return;
                }
                let (source, full) = measured(&runtime, name, &entry, WindowMode::FullDynamic);
                assert_eq!(source, Some(ThumbnailSource::FullDecode), "{name}");
                assert!(
                    reduced < full,
                    "{name}: a reduced thumbnail held {reduced} bytes, a full one {full}"
                );
            });
        }
    });
}

/// Frames a reduced decode cannot finish, and frames it is not tried on:
/// whatever happens to them, a failed attempt and the full decode after it
/// are never held together, so the thumbnail holds no more than it
/// reserved.
#[test]
fn a_reduced_attempt_that_fails_holds_no_more_than_the_thumbnail_reserved() {
    let honest = baseline_jpeg(1025, 1025, false);
    let progressive = fixture("gray8-progressive-1041x779.jpg");
    let cut =
        |frame: &[u8], fraction: f64| frame[..(frame.len() as f64 * fraction) as usize].to_vec();
    let zeroed = |frame: &[u8], from: f64, to: f64| {
        let at = |fraction: f64| (frame.len() as f64 * fraction) as usize;
        let mut damaged = frame.to_vec();
        damaged[at(from)..at(to)].fill(0);
        damaged
    };
    // (name, columns, rows, frame)
    let frames: Vec<(&str, u32, u32, Vec<u8>)> = vec![
        ("cut in half", 1025, 1025, cut(&honest, 0.5)),
        ("cut near the end", 1025, 1025, cut(&honest, 0.98)),
        ("zeros in the scan", 1025, 1025, zeroed(&honest, 0.4, 0.6)),
        ("zeros at the end", 1025, 1025, zeroed(&honest, 0.9, 1.0)),
        (
            "progressive, cut in half",
            1041,
            779,
            cut(&progressive, 0.5),
        ),
        (
            "progressive, zeros in a scan",
            1041,
            779,
            zeroed(&progressive, 0.5, 0.7),
        ),
        // A lossless frame under the baseline syntax: the reduction that
        // is asked for cannot be applied, and the frame is decoded in
        // full.
        (
            "lossless process",
            512,
            384,
            fixture("gray8-lossless-512x384.jpg"),
        ),
    ];
    std::thread::scope(|scope| {
        for (name, columns, rows, frame) in frames {
            scope.spawn(move || {
                let (entry, _dir) = listed(columns, rows, false, frame);
                let entry = Arc::new(entry);
                assert!(
                    pixels::thumbnail_reduction(&entry, 128, WindowMode::Default).is_some(),
                    "{name}: the thumbnail asks for a reduction"
                );
                let runtime = CountedRuntime::new();
                let (source, _) = measured(&runtime, name, &entry, WindowMode::Default);
                if name == "lossless process" {
                    assert_eq!(source, Some(ThumbnailSource::FullDecode), "{name}");
                }
            });
        }
    });
}
