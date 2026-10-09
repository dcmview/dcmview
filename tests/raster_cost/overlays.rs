//! What overlay and legend work reserves of the decode memory budget against
//! the heap it holds (`pixels::decode_estimate`, the rows of
//! `DecodeWork::ValueLegend`, `SegmentationOverlay` and `ValueOverlay`).
//!
//! An overlay is sized by two things, the object it is made from and the
//! displayed frame it is drawn on, and holds much more than a decode: the
//! planes it samples as real-world values, the resampled frame, an RGBA
//! image and its PNG. So each kind is drawn through the API for objects and
//! displayed frames large enough that the frame, not the fixed part of an
//! estimate, is what is measured, holding samples that do not compress, and
//! the heap of every thread that worked on it is compared with the bytes the
//! scheduler says were reserved. Then many are drawn at once under a small
//! budget.
//!
//! The viewers here keep no frame and no overlay (`NO_CACHES`), so what is
//! measured is the work and not a cache beside it, which the budget does not
//! cover.

use super::bounds::{list, noise, MIB};
use super::heap::CountedRuntime;
use dcmview::annotations::AnnotationStore;
use dcmview::pixels::{self, CacheBudget, DecodeLimits, DecodeScheduler, DecodeWork};
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::{FileEntry, OverlayEncoding};
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;
use std::sync::Arc;

/// What a request may hold beside its reservation: its tasks, handles and
/// messages, and the object's metadata while it is read. A fixed megabyte.
const REQUEST_OVERHEAD: u64 = MIB;

/// Rows and columns of every object and displayed frame measured here. At
/// this size the PNG of an image that does not compress is held at its
/// worst: just past a doubling of the buffers it is written through.
const SIDE: u16 = 1344;
const PIXELS: u64 = SIDE as u64 * SIDE as u64;

/// Planes of the dose grid. A displayed frame that cuts through the grid
/// samples all of them.
const DOSE_PLANES: u32 = 6;

/// A viewer that keeps nothing it decodes or draws.
const NO_CACHES: CacheBudget = CacheBudget {
    frame_bytes: 0,
    raw_bytes: 0,
    overlay_bytes: 0,
    thumbnail_bytes: 0,
};

/// The committed fixture `name` with `SIDE` rows and columns and
/// `pixel_data`, and whatever else `edit` changes. Its geometry is kept: a
/// larger grid of the same spacing from the same origin.
fn enlarged(name: &str, pixel_data: Vec<u8>, edit: impl FnOnce(&mut InMemDicomObject)) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let mut object = dicom_object::open_file(path).expect("open fixture");
    let vr = object.element(tags::PIXEL_DATA).expect("pixel data").vr();
    for tag in [tags::ROWS, tags::COLUMNS] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(SIDE)));
    }
    object.put(DataElement::new(
        tags::PIXEL_DATA,
        vr,
        PrimitiveValue::from(pixel_data),
    ));
    edit(&mut object);
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).expect("write DICOM");
    bytes
}

/// `frames` frames of 16-bit samples that do not compress, each sample's
/// high byte masked by `mask`.
fn samples(frames: u32, mask: u8) -> Vec<u8> {
    let mut bytes = noise((PIXELS * u64::from(frames) * 2) as usize);
    for sample in bytes.chunks_exact_mut(2) {
        sample[1] &= mask;
    }
    bytes
}

/// The objects and displayed frames measured, by file name.
///
/// - `map.dcm`: a Parametric Map of two planes at z = 0 and 2, whose
///   samples all lie in its mapped range. `map-on-plane.dcm` is displayed
///   on the first plane, `map-between.dcm` halfway between the two.
/// - `dose.dcm`: an RT Dose grid of `DOSE_PLANES` planes 4 mm apart.
///   `dose-on-plane.dcm` is displayed on the first, `dose-between.dcm`
///   between the second and third, and `dose-across.dcm` is a sagittal
///   frame that cuts through every plane.
/// - `seg-binary.dcm` and `seg-fractional.dcm`: one-bit and eight-bit SEG
///   objects on the frames of their sources.
fn files() -> Vec<(&'static str, Vec<u8>)> {
    let text = |tag, vr, value: &str| DataElement::new(tag, vr, value);
    vec![
        (
            "map.dcm",
            enlarged(
                "golden-parametric-map-u16-linear.dcm",
                samples(2, 0x0f),
                |_| {},
            ),
        ),
        (
            "map-on-plane.dcm",
            enlarged(
                "golden-parametric-map-mr-source-z0.dcm",
                samples(1, 0xff),
                |_| {},
            ),
        ),
        (
            "map-between.dcm",
            enlarged(
                "golden-parametric-map-mr-source-z1.dcm",
                samples(1, 0xff),
                |_| {},
            ),
        ),
        (
            "dose.dcm",
            enlarged(
                "golden-rtdose-u16-grid.dcm",
                samples(DOSE_PLANES, 0xff),
                |object| {
                    object.put(text(
                        tags::NUMBER_OF_FRAMES,
                        VR::IS,
                        &DOSE_PLANES.to_string(),
                    ));
                    let offsets: Vec<String> = (0..DOSE_PLANES)
                        .map(|plane| (4 * plane).to_string())
                        .collect();
                    object.put(text(
                        tags::GRID_FRAME_OFFSET_VECTOR,
                        VR::DS,
                        &offsets.join("\\"),
                    ));
                },
            ),
        ),
        (
            "dose-on-plane.dcm",
            enlarged("golden-rtdose-ct-source-z0.dcm", samples(1, 0xff), |_| {}),
        ),
        (
            "dose-between.dcm",
            enlarged("golden-rtdose-ct-source-z6.dcm", samples(1, 0xff), |_| {}),
        ),
        (
            "dose-across.dcm",
            enlarged(
                "golden-rtdose-ct-source-z20.dcm",
                samples(1, 0xff),
                |object| {
                    // Rows advance along z, 0.016 mm apart, from z = 0:
                    // 21.5 mm in all, past the last plane at z = 20.
                    object.put(text(tags::IMAGE_POSITION_PATIENT, VR::DS, "100\\0\\0"));
                    object.put(text(
                        tags::IMAGE_ORIENTATION_PATIENT,
                        VR::DS,
                        "0\\1\\0\\0\\0\\1",
                    ));
                    object.put(text(tags::PIXEL_SPACING, VR::DS, "0.016\\4"));
                },
            ),
        ),
        (
            "seg-binary.dcm",
            enlarged(
                "golden-seg-binary.dcm",
                noise((PIXELS * 2 / 8) as usize),
                |_| {},
            ),
        ),
        (
            "seg-binary-source.dcm",
            enlarged("golden-seg-binary-source.dcm", samples(2, 0xff), |_| {}),
        ),
        (
            "seg-fractional.dcm",
            enlarged(
                "golden-seg-fractional.dcm",
                noise((PIXELS * 2) as usize),
                |_| {},
            ),
        ),
        (
            "seg-fractional-source.dcm",
            enlarged("golden-seg-fractional-source.dcm", samples(2, 0xff), |_| {}),
        ),
    ]
}

/// The loader's entries for [`files`], indexed in that order, with the
/// directory they name.
struct Listed {
    names: Vec<&'static str>,
    entries: Vec<FileEntry>,
    _scan: super::bounds::Listed,
}

impl Listed {
    fn new() -> Self {
        let files = files();
        let borrowed: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(name, bytes)| (*name, bytes.as_slice()))
            .collect();
        let scan = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(list(&borrowed));
        let names: Vec<_> = files.iter().map(|(name, _)| *name).collect();
        let entries = names
            .iter()
            .map(|name| {
                let entry = scan.entry(name).clone();
                assert_eq!(
                    (entry.rows, entry.columns),
                    (u32::from(SIDE), u32::from(SIDE)),
                    "{name}"
                );
                entry
            })
            .collect();
        Self {
            names,
            entries,
            _scan: scan,
        }
    }

    fn index(&self, name: &str) -> usize {
        self.names
            .iter()
            .position(|listed| *listed == name)
            .unwrap_or_else(|| panic!("{name} is not measured"))
    }

    fn entry(&self, name: &str) -> &FileEntry {
        &self.entries[self.index(name)]
    }

    /// A viewer over the files that keeps nothing.
    fn state(&self) -> AppState {
        AppState::new(
            FileRegistry::from_files(self.entries.clone()),
            AnnotationStore::empty(),
        )
        .with_cache_budget(NO_CACHES)
    }
}

/// One overlay request: what it is called, its path, the work it reserves
/// for, the entry that work is estimated from, and the context whose legend
/// it is drawn with, if it has one.
struct Overlay {
    name: String,
    path: String,
    work: DecodeWork,
    object: FileEntry,
    context: Option<String>,
}

impl Overlay {
    fn estimate(&self) -> u64 {
        pixels::decode_estimate(&self.object, self.work)
    }
}

fn context_path(listed: &Listed, object: &str) -> String {
    format!("/api/file/{}/semantic-context", listed.index(object))
}

fn overlays(listed: &Listed) -> Vec<Overlay> {
    let side = u32::from(SIDE);
    let value = |object: &str, target: &str, planes: u32, encoding: OverlayEncoding| {
        let (endpoint, parameter) = if object == "dose.dcm" {
            ("dose-overlay", "dose")
        } else {
            ("parametric-map-overlay", "map")
        };
        let suffix = match encoding {
            OverlayEncoding::Png => "",
            OverlayEncoding::Values => "/values",
        };
        Overlay {
            name: format!("{object} on {target} as {encoding:?}"),
            path: format!(
                "/api/file/{}/frame/0/{endpoint}{suffix}?{parameter}={}",
                listed.index(target),
                listed.index(object)
            ),
            work: DecodeWork::ValueOverlay {
                target_rows: side,
                target_columns: side,
                planes,
                encoding,
            },
            object: listed.entry(object).clone(),
            context: Some(context_path(listed, object)),
        }
    };
    let painted = |segmentation: &str| Overlay {
        name: segmentation.to_string(),
        path: format!(
            "/api/file/{}/frame/0/segmentation-overlay",
            listed.index(segmentation)
        ),
        work: DecodeWork::SegmentationOverlay {
            target_rows: side,
            target_columns: side,
        },
        object: listed.entry(segmentation).clone(),
        context: None,
    };
    use OverlayEncoding::{Png, Values};
    vec![
        value("map.dcm", "map-on-plane.dcm", 1, Png),
        value("map.dcm", "map-between.dcm", 2, Png),
        value("map.dcm", "map-between.dcm", 2, Values),
        value("dose.dcm", "dose-on-plane.dcm", 1, Png),
        value("dose.dcm", "dose-between.dcm", 2, Png),
        value("dose.dcm", "dose-across.dcm", DOSE_PLANES, Png),
        value("dose.dcm", "dose-across.dcm", DOSE_PLANES, Values),
        painted("seg-binary.dcm"),
        painted("seg-fractional.dcm"),
    ]
}

/// A scheduler that reserves what it is asked for and never has to refuse.
fn roomy_scheduler() -> Arc<DecodeScheduler> {
    DecodeScheduler::with_limits(
        4,
        DecodeLimits {
            memory_bytes: 1 << 50,
            ..DecodeLimits::DEFAULT
        },
    )
}

/// Asks a viewer over `state`, admitted by a scheduler of its own, for
/// `path`, and returns the heap the request held and the most the scheduler
/// reserved for it. Panics unless the answer is 200.
fn measured(runtime: &CountedRuntime, state: &AppState, path: &str) -> (u64, u64) {
    let scheduler = roomy_scheduler();
    let state = state.clone().with_decode_scheduler(scheduler.clone());
    let (status, heap) = runtime.peak_during(async {
        let server = axum_test::TestServer::new(server::router(state));
        let response = server.get(path).await;
        (response.status_code(), response.text())
    });
    assert_eq!(status.0, 200, "{path}: {}", status.1);
    let load = runtime
        .peak_during(scheduler.load_when(|load| load.running == 0))
        .0;
    (heap, load.peak_reserved_bytes)
}

fn report(name: &str, heap: u64, reserved: u64) {
    if std::env::var_os("RASTER_COST_REPORT").is_some() {
        eprintln!(
            "{name}: heap {heap} ({:.2} bytes a pixel), reserved {reserved} ({:.2} of it)",
            heap as f64 / PIXELS as f64,
            heap as f64 / reserved.max(1) as f64,
        );
    }
}

/// A legend and every kind of overlay hold no more heap than they reserved,
/// and reserve their documented estimate.
///
/// The part of an estimate that does not grow with the frames is left out
/// of what the work may hold, so the part that does is shown to cover the
/// work on its own, as it must for frames a hundred times larger.
#[test]
fn overlay_and_legend_work_holds_no_more_heap_than_it_reserved() {
    let listed = Listed::new();
    // The estimates first: nothing below means anything without them.
    let requests = overlays(&listed);
    let legends = ["map.dcm", "dose.dcm"].map(|object| {
        let entry = listed.entry(object);
        (
            object,
            context_path(&listed, object),
            pixels::decode_estimate(entry, DecodeWork::ValueLegend),
        )
    });
    for request in &requests {
        assert!(request.estimate() > 24 * PIXELS, "{}", request.name);
    }

    let runtime = CountedRuntime::new();
    // One viewer for all of it: its semantic contexts are kept from one
    // request to the next, whichever scheduler admits them.
    let state = listed.state();

    for (object, path, estimate) in &legends {
        let (heap, reserved) = measured(&runtime, &state, path);
        report(&format!("the legend of {object}"), heap, reserved);
        assert_eq!(
            reserved, *estimate,
            "the legend of {object} reserves its estimate"
        );
        assert!(
            heap <= reserved - pixels::DICOM_DECODE_BASE_BYTES + REQUEST_OVERHEAD,
            "the legend of {object}: {heap} bytes of heap held with {reserved} reserved"
        );
    }

    for request in &requests {
        // The context and its legend are kept by now, so the overlay is
        // the only work the request does.
        let (heap, reserved) = measured(&runtime, &state, &request.path);
        report(&request.name, heap, reserved);
        assert_eq!(
            reserved,
            request.estimate(),
            "{} reserves its estimate",
            request.name
        );
        // A SEG overlay's estimate is its decode and the image; a value
        // overlay's is the larger of its decode and the displayed frame,
        // which at this size is the displayed frame.
        let fixed = match request.work {
            DecodeWork::SegmentationOverlay { .. } => pixels::DICOM_DECODE_BASE_BYTES,
            _ => 0,
        };
        assert!(
            heap <= reserved - fixed + REQUEST_OVERHEAD,
            "{}: {heap} bytes of heap held with {reserved} reserved",
            request.name
        );
        // The measurement is of the whole work: every overlay holds at
        // least its image or its planes.
        assert!(heap >= 12 * PIXELS, "{}: only {heap} bytes", request.name);
    }
}

/// What many requests at once may hold beside the budget, whatever their
/// frames: the tasks and messages of all of them. A fixed 16 MiB.
const CONCURRENT_OVERHEAD: u64 = 16 * MIB;

/// Many overlays at once, under a budget that admits the largest of them
/// alone, hold no more heap between them than that budget and a fixed
/// overhead. The scheduler has sixteen permits, so without their own
/// reservations the same overlays are drawn sixteen at a time and hold
/// several times the budget.
#[test]
fn concurrent_overlays_hold_no_more_heap_than_the_budget() {
    const PERMITS: usize = 16;
    let listed = Listed::new();
    let requests = overlays(&listed);
    let budget = requests
        .iter()
        .map(Overlay::estimate)
        .max()
        .expect("requests");
    let scheduler = DecodeScheduler::with_limits(
        PERMITS,
        DecodeLimits {
            memory_bytes: budget,
            ..DecodeLimits::DEFAULT
        },
    );
    let state = listed.state().with_decode_scheduler(scheduler.clone());
    // Each overlay twice over, and their contexts with them, all at once.
    let mut paths: Vec<String> = Vec::new();
    for request in &requests {
        paths.extend(request.context.clone());
        paths.push(request.path.clone());
        paths.push(request.path.clone());
    }
    let asked = paths.len();

    let runtime = CountedRuntime::new();
    let (served, heap) = runtime.peak_during(async move {
        let server = Arc::new(axum_test::TestServer::new(server::router(state)));
        let tasks: Vec<_> = paths
            .into_iter()
            .map(|path| {
                let server = server.clone();
                tokio::spawn(async move { server.get(&path).await.status_code() == 200 })
            })
            .collect();
        let mut served = 0;
        for task in tasks {
            served += usize::from(task.await.expect("request task"));
        }
        served
    });
    let load = scheduler.load();
    if std::env::var_os("RASTER_COST_REPORT").is_some() {
        eprintln!(
            "{asked} requests, {served} served: heap {heap}, budget {budget}, most reserved {}",
            load.peak_reserved_bytes
        );
    }
    assert_eq!(served, asked, "every request is served in the end");
    assert!(load.peak_reserved_bytes <= budget);
    assert!(
        load.peak_reserved_bytes > budget / 2,
        "the budget was what bound the overlays"
    );
    assert!(
        heap <= budget + CONCURRENT_OVERHEAD,
        "{heap} bytes of heap held under a budget of {budget}"
    );
}
