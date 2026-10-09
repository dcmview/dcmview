//! What a request reserves of the decode memory budget against the heap its
//! work holds (`pixels::decode_estimate`, `pixels::DecodeScheduler::admit`).
//!
//! The reservation is computed from the catalog entry before anything is
//! read, and the budget only bounds memory if no path holds more than it
//! reserved. So each path the pixel service takes for a frame (raw, display,
//! preview, thumbnail, presentation layer, each with and without redaction
//! boxes) is run for the files the other tests of this binary measure, the
//! hostile ones included, and the heap of every thread that worked on it is
//! compared with the bytes the scheduler says were reserved. Then many are
//! run at once under a small budget, and the heap of all of them together is
//! compared with the budget.
//!
//! The service decodes on the blocking pool, so the heap is counted across
//! the threads of a runtime each test owns (`heap::CountedRuntime`).

use super::bounds::{hostile_files, list, noise, Listed, MIB};
use super::heap::CountedRuntime;
use super::raster_files::{self as files, TiffPage, TiffValue};
use super::scale::stand_ins;
use dcmview::annotations::AnnotationStore;
use dcmview::api::contracts::WindowMode;
use dcmview::pixels::{
    self, DecodeLimits, DecodeScheduler, DecodeWork, FrameCache, FrameRequest, PixelResult,
    RawFrameCache, RawFrameRequest, Redaction, ThumbnailCache, ThumbnailRequest,
};
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::FileEntry;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use std::sync::{Arc, Mutex};

/// What a request may hold beside its reservation: the tasks, handles and
/// messages of the request itself, whatever the frame. A fixed megabyte.
const REQUEST_OVERHEAD: u64 = MIB;

/// One way a frame is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Raw,
    RawRedacted,
    Display,
    DisplayFullDynamic,
    DisplayWindowed,
    DisplayRedacted,
    Preview,
    Thumbnail,
    ThumbnailRedacted,
    PresentationLayer,
}

impl Path {
    const ALL: [Self; 10] = [
        Self::Raw,
        Self::RawRedacted,
        Self::Display,
        Self::DisplayFullDynamic,
        Self::DisplayWindowed,
        Self::DisplayRedacted,
        Self::Preview,
        Self::Thumbnail,
        Self::ThumbnailRedacted,
        Self::PresentationLayer,
    ];

    /// The most a request on this path reserves at once: each kind of work
    /// it does is admitted on its own, one after another.
    fn reserves(self, entry: &FileEntry) -> u64 {
        let estimate = |work| pixels::decode_estimate(entry, work);
        match self {
            Self::Raw => estimate(DecodeWork::RawFrame),
            Self::RawRedacted => {
                estimate(DecodeWork::RawFrame).max(estimate(DecodeWork::RawRedaction))
            }
            Self::Display
            | Self::DisplayFullDynamic
            | Self::DisplayWindowed
            | Self::DisplayRedacted
            | Self::Preview => estimate(DecodeWork::DisplayFrame),
            Self::Thumbnail | Self::ThumbnailRedacted => estimate(DecodeWork::Thumbnail),
            Self::PresentationLayer => estimate(DecodeWork::PresentationLayer),
        }
    }
}

/// Boxes over part of a frame of any size.
fn boxes(entry: &FileEntry) -> Redaction {
    Redaction {
        revision: 1,
        boxes: vec![[0, 0, entry.rows.div_ceil(2), entry.columns.div_ceil(2)]],
    }
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

/// Asks for frame 0 of `entry` by `path`, with caches of its own that
/// `scheduler` admits for, and says whether a frame came back. Everything
/// it allocated is freed before it returns.
async fn ask(entry: Arc<FileEntry>, path: Path, scheduler: Arc<DecodeScheduler>) -> bool {
    let frames = || {
        Arc::new(Mutex::new(FrameCache::with_scheduler(
            pixels::FRAME_CACHE_MAX_BYTES,
            scheduler.clone(),
        )))
    };
    let raws = || {
        Arc::new(Mutex::new(RawFrameCache::with_scheduler(
            pixels::RAW_CACHE_MAX_BYTES,
            scheduler.clone(),
        )))
    };
    let display = |request: FrameRequest, redaction: Redaction| {
        let (entry, frames, raws) = (entry.clone(), frames(), raws());
        async move {
            pixels::load_redacted_frame(entry, frames, raws, request, redaction)
                .await
                .is_ok()
        }
    };
    let window = |center: Option<f64>, mode: WindowMode, preview: bool| FrameRequest {
        frame: 0,
        window_center: center,
        window_width: center.map(|_| 100.0),
        window_mode: mode,
        real_world: None,
        preview,
    };
    let thumbnail = |redaction: Redaction| {
        let entry = entry.clone();
        let cache = Arc::new(Mutex::new(ThumbnailCache::with_scheduler(
            pixels::THUMBNAIL_CACHE_MAX_BYTES,
            scheduler.clone(),
        )));
        async move {
            let request = ThumbnailRequest {
                frame: 0,
                bucket: 1024,
                window_mode: WindowMode::Default,
            };
            pixels::load_thumbnail(entry, cache, request, redaction)
                .await
                .is_ok()
        }
    };
    let ok = |result: PixelResult<pixels::RawFrameResponse>| result.is_ok();
    match path {
        Path::Raw => {
            ok(pixels::load_raw_frame(entry.clone(), raws(), RawFrameRequest { frame: 0 }).await)
        }
        Path::RawRedacted => ok(pixels::load_redacted_raw_frame(
            entry.clone(),
            raws(),
            RawFrameRequest { frame: 0 },
            &boxes(&entry),
        )
        .await),
        Path::Display => {
            display(
                window(None, WindowMode::Default, false),
                Redaction::default(),
            )
            .await
        }
        Path::DisplayFullDynamic => {
            display(
                window(None, WindowMode::FullDynamic, false),
                Redaction::default(),
            )
            .await
        }
        Path::DisplayWindowed => {
            display(
                window(Some(50.0), WindowMode::Default, false),
                Redaction::default(),
            )
            .await
        }
        Path::DisplayRedacted => {
            display(window(None, WindowMode::Default, false), boxes(&entry)).await
        }
        Path::Preview => {
            display(
                window(Some(60.0), WindowMode::Default, true),
                Redaction::default(),
            )
            .await
        }
        Path::Thumbnail => thumbnail(Redaction::default()).await,
        Path::ThumbnailRedacted => thumbnail(boxes(&entry)).await,
        Path::PresentationLayer => {
            let mut file = FileEntry::clone(&entry);
            file.index = 0;
            let state = AppState::new(
                FileRegistry::from_files(vec![file]),
                AnnotationStore::empty(),
            )
            .with_decode_scheduler(scheduler.clone());
            let server = axum_test::TestServer::new(server::router(state));
            server
                .get("/api/file/0/frame/0/presentation-layer")
                .await
                .status_code()
                .is_success()
        }
    }
}

/// Runs every path for `entry` and holds each to its reservation. Returns
/// how many paths served a frame.
///
/// `fixed` is the part of a decode's estimate that does not grow with the
/// frame. For a frame large enough to measure it is left out of what the
/// path may hold, so the part that does grow with the frame is shown to
/// cover the work on its own, as it must for a frame a hundred times
/// larger.
fn assert_reservations_cover(
    runtime: &CountedRuntime,
    name: &str,
    entry: &FileEntry,
    fixed: u64,
) -> usize {
    let entry = Arc::new(entry.clone());
    let mut served = 0;
    for path in Path::ALL {
        let scheduler = roomy_scheduler();
        let (ok, heap) = runtime.peak_during(ask(entry.clone(), path, scheduler.clone()));
        let load = runtime
            .peak_during(scheduler.load_when(|load| load.running == 0))
            .0;
        let reserved = load.peak_reserved_bytes;
        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!(
                "{name} {path:?}: heap {heap}, reserved {reserved} ({:.2} of it), {}",
                heap as f64 / reserved.max(1) as f64,
                if ok { "served" } else { "refused" }
            );
        }
        // The layer is drawn without a decode, so its estimate has no
        // such part.
        let growing = match path {
            Path::PresentationLayer => reserved,
            _ => reserved.saturating_sub(fixed),
        };
        assert!(
            heap <= growing + REQUEST_OVERHEAD,
            "{name} {path:?}: {heap} bytes of heap held with {reserved} reserved, \
             {growing} of it for the frame"
        );
        if reserved > 0 {
            assert_eq!(
                reserved,
                path.reserves(&entry),
                "{name} {path:?} reserves its documented estimate"
            );
        }
        if ok {
            assert!(reserved > 0, "{name} {path:?} was served without a permit");
            served += 1;
        }
    }
    served
}

/// The files `scale.rs` measures and more layouts whose display path works
/// on every sample: gray and alpha, 16-bit gray, and 32-bit integers and
/// floats, stored plainly and as tiles that deflate to almost nothing (a
/// small file reserves nothing for its length).
fn frames_to_measure() -> Vec<(&'static str, Vec<u8>)> {
    const SIDE: u32 = 1248;
    let pixels = (SIDE * SIDE) as usize;
    let mut cases = stand_ins();
    cases.push((
        "gray-alpha.png",
        files::png_of(
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Sixteen,
            (SIDE, SIDE),
            &noise(pixels * 4),
            |_| {},
        ),
    ));
    cases.push((
        "gray16.png",
        files::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            (SIDE, SIDE),
            &noise(pixels * 2),
            |_| {},
        ),
    ));
    let wide = TiffPage::strip((SIDE, SIDE), &[32], 1, noise(pixels * 4));
    cases.push(("wide.tif", files::tiff_file(false, &[wide]).0));
    let signed = TiffPage::strip((SIDE, SIDE), &[32], 0, noise(pixels * 4))
        .with(339, TiffValue::Short(vec![2]));
    cases.push(("signed.tif", files::tiff_file(true, &[signed]).0));
    let tile = files::zlib(&vec![0; 256 * 256 * 4]);
    let tiles = (SIDE as usize).div_ceil(256).pow(2);
    for (name, format) in [("wide-tiles.tif", 1), ("real-tiles.tif", 3)] {
        let page = TiffPage {
            tags: vec![
                (256, TiffValue::Long(vec![SIDE])),
                (257, TiffValue::Long(vec![SIDE])),
                (258, TiffValue::Short(vec![32])),
                (259, TiffValue::Short(vec![8])),
                (262, TiffValue::Short(vec![1])),
                (277, TiffValue::Short(vec![1])),
                (322, TiffValue::Long(vec![256])),
                (323, TiffValue::Long(vec![256])),
                (339, TiffValue::Short(vec![format])),
            ],
            chunks: vec![tile.clone(); tiles],
            tiled: true,
        };
        cases.push((name, files::tiff_file(false, &[page]).0));
    }
    cases
}

fn listed(runtime: &tokio::runtime::Runtime, files: &[(&str, Vec<u8>)]) -> Listed {
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (*name, bytes.as_slice()))
        .collect();
    runtime.block_on(list(&files))
}

/// Every path holds no more heap than it reserved, for frames large enough
/// that the frame, not the fixed part of the estimate, is what is measured.
#[test]
fn a_request_holds_no_more_heap_than_it_reserved() {
    let plain = tokio::runtime::Runtime::new().expect("runtime");
    let cases = frames_to_measure();
    let scan = listed(&plain, &cases);
    for (name, _) in &cases {
        let frame = pixels::raster_frame_bytes(scan.entry(name)).expect("frame size");
        assert!(Path::Raw.reserves(scan.entry(name)) >= 6 * frame, "{name}");
    }
    // Each file is measured on a runtime of its own, so they can run side
    // by side without sharing a count.
    std::thread::scope(|scope| {
        for (name, _) in &cases {
            let entry = scan.entry(name);
            scope.spawn(move || {
                let frame = pixels::raster_frame_bytes(entry).expect("frame size");
                assert!(frame >= MIB, "{name}");
                let served = assert_reservations_cover(
                    &CountedRuntime::new(),
                    name,
                    entry,
                    pixels::RASTER_DECODE_HEAP_BASE_BYTES,
                );
                // Two of the files end early and decode to nothing.
                let ends_early = ["progressive.jpg", "lossy.webp"].contains(name);
                assert!(served > 0 || ends_early, "{name}");
            });
        }
    });
}

/// The same for the hostile and damaged files: what a file declares never
/// makes a path hold more than the entry reserved for it.
#[test]
fn a_hostile_file_holds_no_more_heap_than_its_entry_reserved() {
    let plain = tokio::runtime::Runtime::new().expect("runtime");
    let cases = hostile_files();
    let on_disk: Vec<_> = cases.iter().filter_map(|case| case.on_disk()).collect();
    assert!(
        on_disk.len() >= 30,
        "only {} cases can be written",
        on_disk.len()
    );
    let names: Vec<String> = on_disk
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let format = case.name.split(' ').next().expect("format");
            format!("{index}.{format}")
        })
        .collect();
    let files: Vec<(&str, Vec<u8>)> = names
        .iter()
        .zip(&on_disk)
        .map(|(file, case)| (file.as_str(), case.listed.to_vec()))
        .collect();
    let scan = listed(&plain, &files);
    let runtime = CountedRuntime::new();
    for (file, case) in names.iter().zip(&on_disk) {
        let entry = scan.entry(file);
        if let Some(replaced) = case.replaced {
            std::fs::write(&entry.path, replaced).expect("replace the listed file");
        }
        assert_reservations_cover(&runtime, case.name, entry, 0);
    }
}

/// A native single-frame DICOM image of `side` x `side` gray samples of
/// `bits` bits, holding `samples`.
fn native_dicom(side: u16, bits: u16, samples: Vec<u8>) -> Vec<u8> {
    Native::gray(side, bits).file(samples)
}

/// What a native single-frame DICOM image of `side` x `side` pixels says
/// about itself.
struct Native {
    side: u16,
    bits: u16,
    samples_per_pixel: u16,
    transfer_syntax: &'static str,
}

impl Native {
    fn gray(side: u16, bits: u16) -> Self {
        Self {
            side,
            bits,
            samples_per_pixel: 1,
            transfer_syntax: uids::EXPLICIT_VR_LITTLE_ENDIAN,
        }
    }

    fn in_syntax(self, transfer_syntax: &'static str) -> Self {
        Self {
            transfer_syntax,
            ..self
        }
    }

    /// The bytes of one frame.
    fn frame_bytes(&self) -> usize {
        usize::from(self.side)
            * usize::from(self.side)
            * usize::from(self.samples_per_pixel)
            * usize::from(self.bits / 8)
    }

    /// The file, holding `samples` as its pixel data.
    fn file(&self, samples: Vec<u8>) -> Vec<u8> {
        let Self { side, bits, .. } = *self;
        let unsigned = |value: u16| PrimitiveValue::from(value);
        let uid = format!("2.25.{}{bits}{side}", self.samples_per_pixel);
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, uid.as_str()),
            DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("OT")),
            DataElement::new(tags::ROWS, VR::US, unsigned(side)),
            DataElement::new(tags::COLUMNS, VR::US, unsigned(side)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, unsigned(bits)),
            DataElement::new(tags::BITS_STORED, VR::US, unsigned(bits)),
            DataElement::new(tags::HIGH_BIT, VR::US, unsigned(bits - 1)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, unsigned(0)),
            DataElement::new(
                tags::SAMPLES_PER_PIXEL,
                VR::US,
                unsigned(self.samples_per_pixel),
            ),
            DataElement::new(
                tags::PHOTOMETRIC_INTERPRETATION,
                VR::CS,
                PrimitiveValue::from(if self.samples_per_pixel == 3 {
                    "RGB"
                } else {
                    "MONOCHROME2"
                }),
            ),
            DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::from(samples)),
        ]);
        if self.samples_per_pixel == 3 {
            object.put(DataElement::new(
                tags::PLANAR_CONFIGURATION,
                VR::US,
                unsigned(0),
            ));
        }
        let mut bytes = Vec::new();
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(self.transfer_syntax)
                    .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid(uid.as_str()),
            )
            .expect("file meta")
            .write_all(&mut bytes)
            .expect("write DICOM");
        bytes
    }
}

/// Where the data set of the DICOM file `bytes` starts: after the preamble,
/// the magic code and the file meta group, whose first element is its
/// length.
fn data_set_start(bytes: &[u8]) -> usize {
    let length = u32::from_le_bytes(bytes[140..144].try_into().expect("group length"));
    144 + length as usize
}

/// A gray image as a Deflated Explicit VR Little Endian file whose deflated
/// stream ends `kept` bytes into the pixel data. The header and the pixel
/// element still state the whole frame.
fn deflated_dicom_cut_short(side: u16, bits: u16, kept: usize) -> Vec<u8> {
    use std::io::Write;

    let image = Native::gray(side, bits);
    let frame = image.frame_bytes();
    let plain = image.file(vec![0x5a; frame]);
    let data_set = &plain[data_set_start(&plain)..plain.len() - frame + kept];
    let deflated = Native::gray(side, bits)
        .in_syntax(uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN)
        .file(Vec::new());
    let mut file = deflated[..data_set_start(&deflated)].to_vec();
    let mut encoder = flate2::write::DeflateEncoder::new(&mut file, flate2::Compression::fast());
    encoder.write_all(data_set).expect("deflate");
    encoder.finish().expect("deflate");
    file
}

/// DICOM frames reserve by rule, not by a limit their decoders are held to;
/// for the committed fixtures the rule covers what each path holds. JPEG
/// 2000 is left out: its decoder frees with layouts this binary's allocator
/// check rejects (see `main.rs`).
#[test]
fn a_dicom_fixture_holds_no_more_heap_than_it_reserved() {
    let plain = tokio::runtime::Runtime::new().expect("runtime");
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files = Vec::new();
    for name in [
        "golden-uncompressed-u16-multiframe.dcm",
        "golden-jpeg-baseline-large-single-frame.dcm",
        "golden-jpeg-baseline-multiframe-bot.dcm",
        "golden-jpeg-lossless-u16-single-frame.dcm",
        "golden-jpeg-lossless-ybr-full-u8-single-frame.dcm",
        "golden-jpegxl-lossless-ybr-rct-u8-single-frame.dcm",
        "golden-rle-ybr-full-422-u8-single-frame.dcm",
        "golden-shutter-circular-cielab-rgb-u8.dcm",
        "golden-shutter-bitmap-u8.dcm",
        "golden-seg-binary.dcm",
        "golden-gsps-target-multiframe-u8.dcm",
    ] {
        files.push((
            name,
            std::fs::read(fixtures.join(name)).expect("read fixture"),
        ));
    }
    // Frames large enough for the frame, not the fixed part, to be what is
    // measured: 16-bit samples are windowed through a table, wider ones one
    // sample at a time.
    const SIDE: u16 = 1248;
    let pixels = usize::from(SIDE) * usize::from(SIDE);
    files.push(("native16.dcm", native_dicom(SIDE, 16, noise(pixels * 2))));
    files.push(("native32.dcm", native_dicom(SIDE, 32, noise(pixels * 4))));
    let scan = listed(&plain, &files);
    let runtime = CountedRuntime::new();
    let mut served = 0;
    for (name, _) in &files {
        // The generated frames are large enough to leave the fixed part out.
        let fixed = if name.starts_with("native") {
            pixels::DICOM_DECODE_BASE_BYTES
        } else {
            0
        };
        served += assert_reservations_cover(&runtime, name, scan.entry(name), fixed);
    }
    assert!(
        served >= 6 * files.len(),
        "only {served} paths served a frame"
    );
}

/// A deflated data set is not sized by its length on disk, and its pixel
/// element's declared length is only a claim. Every path that reads a frame
/// of one reserves the entry's frame and holds no more than that, and a
/// stream that ends before the frame does is refused at the cost of what it
/// supplied, not of the frame it announced.
#[test]
fn a_deflated_data_set_costs_what_it_supplies() {
    const SIDE: u16 = 2048;
    let frame = u64::from(SIDE) * u64::from(SIDE) * 2;
    let plain = tokio::runtime::Runtime::new().expect("runtime");
    let whole = Native::gray(SIDE, 16)
        .in_syntax(uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN)
        .file(noise(frame as usize));
    // (file, bytes, whether its frame is there to be served)
    let cases = [
        ("whole.dcm", whole, true),
        (
            "no-pixels.dcm",
            deflated_dicom_cut_short(SIDE, 16, 0),
            false,
        ),
        (
            "few-pixels.dcm",
            deflated_dicom_cut_short(SIDE, 16, 4096),
            false,
        ),
    ];
    let files: Vec<(&str, Vec<u8>)> = cases
        .iter()
        .map(|(name, bytes, _)| (*name, bytes.clone()))
        .collect();
    let scan = listed(&plain, &files);
    let runtime = CountedRuntime::new();
    for (name, bytes, complete) in &cases {
        let entry = Arc::new(scan.entry(name).clone());
        assert_eq!(
            (entry.rows, entry.columns, entry.bits_allocated),
            (u32::from(SIDE), u32::from(SIDE), 16),
            "{name} is listed with the frame its header states"
        );
        assert_eq!(
            entry.transfer_syntax_uid,
            uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN,
            "{name}"
        );
        if !complete {
            assert!((bytes.len() as u64) < MIB / 4, "{name} is a small file");
        }
        for path in Path::ALL {
            // The layer is drawn from the header alone and reads no frame.
            if path == Path::PresentationLayer {
                continue;
            }
            let scheduler = roomy_scheduler();
            let (served, heap) = runtime.peak_during(ask(entry.clone(), path, scheduler.clone()));
            let reserved = runtime
                .peak_during(scheduler.load_when(|load| load.running == 0))
                .0
                .peak_reserved_bytes;
            if std::env::var_os("RASTER_COST_REPORT").is_some() {
                eprintln!("{name} {path:?}: heap {heap}, reserved {reserved}");
            }
            assert_eq!(served, *complete, "{name} {path:?}");
            assert_eq!(
                reserved,
                path.reserves(&entry),
                "{name} {path:?} reserves its documented estimate"
            );
            assert!(reserved >= 3 * frame, "{name} {path:?}");
            let may_hold = if *complete {
                reserved - pixels::DICOM_DECODE_BASE_BYTES + REQUEST_OVERHEAD
            } else {
                REQUEST_OVERHEAD
            };
            assert!(
                heap <= may_hold,
                "{name} {path:?}: {heap} bytes of heap held for a frame of {frame}, \
                 {reserved} reserved"
            );
        }
    }
}

/// What many requests at once may hold beside the budget, whatever their
/// frames: the tasks and messages of all of them. A fixed 16 MiB.
const CONCURRENT_OVERHEAD: u64 = 16 * MIB;

/// Many requests at once, under a budget that admits the largest of them
/// alone, hold no more heap between them than that budget and a fixed
/// overhead. The scheduler has sixteen permits, so without admission by
/// bytes the same requests run sixteen at a time and hold several times
/// the budget.
#[test]
fn concurrent_requests_hold_no_more_heap_than_the_budget() {
    const PERMITS: usize = 16;
    let plain = tokio::runtime::Runtime::new().expect("runtime");
    let cases: Vec<_> = frames_to_measure()
        .into_iter()
        .filter(|(name, _)| {
            [
                "deep.png",
                "deep.tif",
                "floats.tif",
                "animation.webp",
                "wide.tif",
                "gray-alpha.png",
            ]
            .contains(name)
        })
        .collect();
    let scan = listed(&plain, &cases);
    let paths = [
        Path::Raw,
        Path::Display,
        Path::DisplayRedacted,
        Path::Preview,
    ];
    let mut requests = Vec::new();
    for (name, _) in &cases {
        for path in paths {
            for _ in 0..2 {
                requests.push((Arc::new(scan.entry(name).clone()), path));
            }
        }
    }
    let budget = requests
        .iter()
        .map(|(entry, path)| path.reserves(entry))
        .max()
        .expect("requests");
    let scheduler = DecodeScheduler::with_limits(
        PERMITS,
        DecodeLimits {
            memory_bytes: budget,
            ..DecodeLimits::DEFAULT
        },
    );
    let in_flight = requests.len() as u64;

    let runtime = CountedRuntime::new();
    let (served, heap) = runtime.peak_during({
        let scheduler = scheduler.clone();
        async move {
            let tasks: Vec<_> = requests
                .into_iter()
                .map(|(entry, path)| tokio::spawn(ask(entry, path, scheduler.clone())))
                .collect();
            let mut served = 0;
            for task in tasks {
                served += usize::from(task.await.expect("request task"));
            }
            served
        }
    });
    let load = scheduler.load();
    if std::env::var_os("RASTER_COST_REPORT").is_some() {
        eprintln!(
            "{in_flight} requests, {served} served: heap {heap}, budget {budget}, most reserved {}",
            load.peak_reserved_bytes
        );
    }
    assert_eq!(
        served as u64, in_flight,
        "every request is served in the end"
    );
    assert!(load.peak_reserved_bytes <= budget);
    assert!(
        load.peak_reserved_bytes > budget / 2,
        "the budget was what bound the decodes"
    );
    assert!(
        heap <= budget + CONCURRENT_OVERHEAD,
        "{heap} bytes of heap held under a budget of {budget}"
    );
}
