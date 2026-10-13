//! Pixel digests: what `px:` is a digest of, which frames have one, when
//! one is computed and what it costs, and what documents say and read of
//! them (`docs/design/annotation-model.md` 1.3 and 1.6).
//!
//! Every file is written from sample values chosen here and listed by the
//! production loader. No expected digest is one the viewer returned: each
//! is BLAKE3 of a header and samples laid out in the test, and the ones
//! written out in hexadecimal were computed with a BLAKE3 written from its
//! specification, apart from the linked crate.

use super::codestream_files::{self as coded, Layout};
use super::raster_cases::{be16, le16, packed};
use super::raster_files::{self as rasters, TiffPage, TiffValue};
use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::annotations::{AnnotationStore, NativeSource};
use dcmview::loader::{DiscoverOptions, FormatSelection};
use dcmview::masking::Masker;
use dcmview::pixels::{
    exact_samples, pixel_digest, CacheBudget, DecodeLimits, PixelDigestStats, PIXEL_DIGEST_CAPACITY,
};
use dcmview::server::annotations::{import_native, AnnotationError};
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::FileEntry;
use dcmview_annotation::{PixelLayout, SampleFormat};
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";
const IMPLICIT_LE: &str = "1.2.840.10008.1.2";
const EXPLICIT_BE: &str = "1.2.840.10008.1.2.2";
const DEFLATED_LE: &str = "1.2.840.10008.1.2.1.99";
const RLE: &str = "1.2.840.10008.1.2.5";

// ---------------------------------------------------------------------------
// The digest, worked out here

/// `px:` and BLAKE3 of the six header numbers and `samples`, which are in
/// the order and width the digest is defined over.
fn px(
    (rows, columns): (u32, u32),
    samples_per_pixel: u32,
    bits: u32,
    format: u32,
    samples: &[u8],
) -> String {
    let mut hasher = blake3::Hasher::new();
    for number in [rows, columns, samples_per_pixel, bits, format, 0] {
        hasher.update(&number.to_le_bytes());
    }
    hasher.update(samples);
    text(hasher.finalize().as_bytes())
}

fn text(digest: &[u8; 32]) -> String {
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("px:{hex}")
}

/// Sixteen 16-bit samples, no two alike and none a palindrome of bytes.
fn wide_samples() -> Vec<u16> {
    (0..16_u32)
        .map(|i| ((i * 4099 + 17) % 65536) as u16)
        .collect()
}

/// The digest of [`wide_samples`] as a 4 x 4 frame of unsigned samples.
const WIDE: &str = "px:f445c7b7728e36d588413ced93e634b78c79bf73110dd0d5ff65e8fab4aa0c6e";
/// The same bytes as signed samples.
const WIDE_SIGNED: &str = "px:4a30d93af786473260f250b710115074f64aafa3eb6b817adb8330832324fcef";
/// 16 x 16 of `coded::jxl_pattern`, one sample a pixel.
const PATTERN_GRAY: &str = "px:c6af44b4f2bfdce2650f7a9fd65ff020a3271741ea5cdcd7207a4e03f3af33c7";
/// 16 x 16 of `coded::jxl_pattern`, three samples a pixel.
const PATTERN_RGB: &str = "px:14737793c3eb531e1379ada9e40ed5c97557a8aff72aa3ffb7a2791ac2504168";

/// 16 x 16 of `coded::jxl_pattern` with `channels` samples a pixel, colour
/// by pixel.
fn pattern(channels: usize) -> Vec<u8> {
    let mut samples = Vec::new();
    for row in 0..16 {
        for column in 0..16 {
            for channel in 0..channels {
                samples.push(coded::jxl_pattern(row, column, channel));
            }
        }
    }
    samples
}

/// The same samples colour by plane.
fn planes(interleaved: &[u8], channels: usize) -> Vec<u8> {
    (0..channels)
        .flat_map(|channel| interleaved.iter().skip(channel).step_by(channels).copied())
        .collect()
}

// ---------------------------------------------------------------------------
// Files

/// What an uncompressed DICOM file written by [`write_native`] says of its
/// frames.
#[derive(Clone, Copy)]
struct Native {
    transfer_syntax: &'static str,
    uid: &'static str,
    rows: u16,
    columns: u16,
    bits_allocated: u16,
    bits_stored: u16,
    signed: bool,
    samples: u16,
    planar: u16,
    photometric: &'static str,
    frames: u32,
}

impl Native {
    fn gray(uid: &'static str, (rows, columns): (u16, u16), bits: u16) -> Self {
        Self {
            transfer_syntax: EXPLICIT_LE,
            uid,
            rows,
            columns,
            bits_allocated: bits,
            bits_stored: bits,
            signed: false,
            samples: 1,
            planar: 0,
            photometric: "MONOCHROME2",
            frames: 1,
        }
    }
}

/// An uncompressed DICOM file of `pixel_data`, which holds every frame as
/// the transfer syntax stores it.
fn write_native(path: &Path, native: Native, pixel_data: Vec<u8>) {
    let mut object = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, native.uid),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("PIXELS")),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("OT")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(native.rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(native.columns)),
        DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(native.bits_allocated),
        ),
        DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(native.bits_stored),
        ),
        DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(native.bits_stored - 1),
        ),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(u16::from(native.signed)),
        ),
        DataElement::new(
            tags::SAMPLES_PER_PIXEL,
            VR::US,
            PrimitiveValue::from(native.samples),
        ),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from(native.photometric),
        ),
        DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            PrimitiveValue::from(native.frames.to_string()),
        ),
    ]);
    if native.samples > 1 {
        object.put(DataElement::new(
            tags::PLANAR_CONFIGURATION,
            VR::US,
            PrimitiveValue::from(native.planar),
        ));
    }
    object.put(DataElement::new(
        tags::PIXEL_DATA,
        VR::OW,
        PrimitiveValue::from(pixel_data),
    ));
    object
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(native.transfer_syntax)
                .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                .media_storage_sop_instance_uid(native.uid),
        )
        .expect("file meta")
        .write_to_file(path)
        .expect("write DICOM");
}

/// One RLE frame of `planes`, each a byte plane in the order the standard
/// stores them, written as literal runs.
fn rle_frame(planes: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![0_u8; 64];
    header[0..4].copy_from_slice(&(planes.len() as u32).to_le_bytes());
    let mut body = Vec::new();
    for (index, plane) in planes.iter().enumerate() {
        let offset = (64 + body.len()) as u32;
        header[4 + index * 4..8 + index * 4].copy_from_slice(&offset.to_le_bytes());
        for run in plane.chunks(128) {
            body.push((run.len() - 1) as u8);
            body.extend_from_slice(run);
        }
        if body.len() % 2 == 1 {
            body.push(0x80);
        }
    }
    header.extend(body);
    header
}

/// A single-strip TIFF of one page.
fn tiff(
    big_endian: bool,
    size: (u32, u32),
    bits: &[u16],
    photometric: u16,
    data: Vec<u8>,
) -> Vec<u8> {
    rasters::tiff_file(
        big_endian,
        &[TiffPage::strip(size, bits, photometric, data)],
    )
    .0
}

// ---------------------------------------------------------------------------
// A session

/// One session over the files of a directory, listed by the production
/// loader in the order of their paths.
struct Viewer {
    registry: FileRegistry,
    store: AnnotationStore,
    state: AppState,
    server: TestServer,
    files: Vec<FileEntry>,
}

impl Viewer {
    async fn listed(directory: &Path) -> Vec<FileEntry> {
        support::discover(
            &[directory.to_path_buf()],
            DiscoverOptions {
                recursive: true,
                filters: Vec::new(),
                formats: FormatSelection::all(),
            },
        )
        .await
        .expect("discovery")
        .files
    }

    fn over(registry: FileRegistry, store: AnnotationStore) -> Self {
        let files = registry
            .files_snapshot()
            .iter()
            .map(|file| (**file).clone())
            .collect();
        let state = AppState::new(registry.clone(), store.clone());
        let server = TestServer::new(server::router(state.clone()));
        Self {
            registry,
            store,
            state,
            server,
            files,
        }
    }

    /// A session without `--annotations`.
    async fn of(directory: &Path) -> Self {
        let files = Self::listed(directory).await;
        Self::over(FileRegistry::from_files(files), AnnotationStore::empty())
    }

    /// A session whose `--annotations` import has not ended.
    async fn importing(directory: &Path) -> Self {
        let files = Self::listed(directory).await;
        Self::over(FileRegistry::from_files(files), AnnotationStore::loading())
    }

    /// A session of `--mask`.
    async fn masked(directory: &Path) -> Self {
        let registry = FileRegistry::masked(Arc::new(Masker::new()));
        for file in Self::listed(directory).await {
            registry.insert(file);
        }
        registry.mark_scan_complete();
        Self::over(registry, AnnotationStore::empty())
    }

    fn index(&self, name: &str) -> usize {
        self.files
            .iter()
            .find(|file| file.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} was not listed"))
            .index
    }

    fn stats(&self) -> PixelDigestStats {
        self.registry.pixel_digests().stats()
    }

    /// The digest the session holds of one frame, as a document writes it.
    fn held(&self, name: &str, frame: u32) -> Option<String> {
        self.registry
            .pixel_digests()
            .get(self.index(name), frame)
            .map(|digest| text(&digest))
    }

    async fn get(&self, path: &str) -> axum_test::TestResponse {
        let response = self.server.get(path).await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{path}: {}",
            response.text()
        );
        response
    }

    /// Asks for the raw samples of one frame and returns them.
    async fn raw(&self, name: &str, frame: u32) -> Vec<u8> {
        let index = self.index(name);
        self.get(&format!("/api/file/{index}/frame/{frame}/raw"))
            .await
            .as_bytes()
            .to_vec()
    }

    /// The key the session sends for a file, settled.
    async fn key(&self, name: &str) -> String {
        let key = self
            .registry
            .ensure_key(self.index(name))
            .await
            .expect("key");
        self.registry.shown_key_of(&key)
    }

    /// Draws a rectangle on a file.
    async fn annotate(&self, name: &str, n: u32) {
        let key = self.key(name).await;
        let response = self
            .server
            .post(&format!("/api/annotations/ops?file={}", self.index(name)))
            .json(&json!({
                "op_id": dcmview_annotation::new_id(), "actor": "user:test",
                "ts": "2026-10-10T08:00:00.000Z",
                "op": { "type": "create_annotation", "annotation": record(n, &key) },
            }))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
        assert_eq!(response.json::<Value>()["result"]["status"], "ok", "{name}");
    }

    async fn document(&self) -> Value {
        serde_json::from_str(&self.get("/api/annotations/export.json").await.text())
            .expect("document")
    }

    /// Ends the session's import with the document in `text`.
    async fn import(&self, directory: &Path, text: &str) -> Value {
        let path = directory.join(format!("document-{}.json", dcmview_annotation::new_id()));
        std::fs::write(&path, text).expect("write document");
        let source = NativeSource::from_path(&path).expect("document source");
        let report = import_native(&self.registry, &self.store, &source, Arc::new(|| true))
            .await
            .expect("import");
        serde_json::to_value(report).expect("report")
    }

    /// How many ROIs the EMBED view shows on a file.
    async fn rois(&self, name: &str) -> u64 {
        let index = self.index(name);
        self.get(&format!("/api/file/{index}/annotations"))
            .await
            .json::<Value>()["num_roi"]
            .as_u64()
            .expect("num_roi")
    }
}

fn record(n: u32, key: &str) -> Value {
    json!({
        "id": format!("0199c0de-0000-7000-8000-{n:012x}"), "file": key, "frames": "all",
        "layer": "default", "class": "roi",
        "geometry": { "type": "rect", "x0": 0, "y0": 0, "x1": 1, "y1": 1 },
        "attributes": {}, "extensions": {},
        "rev": 1, "created_by": "user:someone", "created_at": "2001-01-01T00:00:00.000Z",
        "modified_by": "user:someone", "modified_at": "2001-01-01T00:00:00.000Z",
    })
}

/// The file reference of a document whose path ends with `name`.
fn reference<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|file| {
            file["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(name))
        })
        .unwrap_or_else(|| panic!("{name} is not in {}", document["files"]))
}

/// How many times an import's report warns with `code`.
fn warned(report: &Value, code: &str) -> u64 {
    report["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning["code"] == code)
        .map_or(0, |warning| warning["count"].as_u64().expect("count"))
}

// ---------------------------------------------------------------------------
// What a digest is

/// The definition, held to digests computed apart from the viewer and from
/// the BLAKE3 crate it links.
#[test]
fn a_digest_is_blake3_of_six_numbers_and_the_samples() {
    let layout = |rows, columns, samples_per_pixel, bits_allocated, sample_format| PixelLayout {
        rows,
        columns,
        samples_per_pixel,
        bits_allocated,
        sample_format,
    };
    let floats: Vec<u8> = [1.5_f32, -2.25]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let cases: Vec<(&str, PixelLayout, Vec<u8>, &str)> = vec![
        (
            "2 x 3 of 8 bits",
            layout(2, 3, 1, 8, SampleFormat::Unsigned),
            (0..6_u32).map(|i| ((i * 7 + 3) % 251) as u8).collect(),
            "px:cc6aca40ff12ee55a1c6772cd4c70364040d2392b1e3e32d1ace8715b56de630",
        ),
        (
            "4 x 4 of 16 bits, unsigned",
            layout(4, 4, 1, 16, SampleFormat::Unsigned),
            le16(&wide_samples()),
            WIDE,
        ),
        (
            "4 x 4 of 16 bits, signed",
            layout(4, 4, 1, 16, SampleFormat::Signed),
            le16(&wide_samples()),
            WIDE_SIGNED,
        ),
        (
            "16 x 16 of one byte",
            layout(16, 16, 1, 8, SampleFormat::Unsigned),
            pattern(1),
            PATTERN_GRAY,
        ),
        (
            "16 x 16 of three bytes",
            layout(16, 16, 3, 8, SampleFormat::Unsigned),
            pattern(3),
            PATTERN_RGB,
        ),
        (
            "1 x 2 of 32-bit floats",
            layout(1, 2, 1, 32, SampleFormat::Float),
            floats,
            "px:9c247bf6f656594e4f051d4acb60962d6417b4fb0dff36c923e2c7f23c23e134",
        ),
        (
            // Longer than one BLAKE3 chunk.
            "40 x 40 of 8 bits",
            layout(40, 40, 1, 8, SampleFormat::Unsigned),
            (0..1600_u32).map(|i| ((i * 31 + 5) % 256) as u8).collect(),
            "px:5d8844148b13bddd9b61d72d10be8759f0be171fd439f54453f58f391787b834",
        ),
        (
            "one pixel",
            layout(1, 1, 1, 8, SampleFormat::Unsigned),
            vec![0],
            "px:43d6f549989d78f07765437ac3b23ab042f2a2864a47c6ec8c8973df086d9f4e",
        ),
    ];
    for (name, layout, samples, expected) in &cases {
        let digest = pixel_digest(layout, samples).unwrap_or_else(|| panic!("{name}: no digest"));
        assert_eq!(text(&digest), *expected, "{name}");
        assert_eq!(
            dcmview_annotation::pixel_digest_text(&digest),
            *expected,
            "{name}"
        );
        // The helper the other tests work digests out with agrees.
        let format = layout.sample_format as u32;
        assert_eq!(
            px(
                (layout.rows, layout.columns),
                layout.samples_per_pixel,
                layout.bits_allocated,
                format,
                samples
            ),
            *expected,
            "{name}"
        );
        // Samples that are not the frame's length are no frame.
        assert_eq!(pixel_digest(layout, &samples[1..]), None, "{name}");
        let mut longer = samples.clone();
        longer.push(0);
        assert_eq!(pixel_digest(layout, &longer), None, "{name}");
    }
}

/// The same samples have one digest in every lossless encoding of them, in
/// DICOM and in image files, and the digest is the one worked out from the
/// values the files were written from.
#[tokio::test]
async fn the_same_samples_have_one_digest_however_they_are_stored() {
    let dir = tempdir().expect("temp dir");
    let at = |name: &str| dir.path().join(name);
    let wide = wide_samples();
    let high: Vec<u8> = wide.iter().map(|sample| (sample >> 8) as u8).collect();
    let low: Vec<u8> = wide.iter().map(|sample| *sample as u8).collect();

    // Sixteen bits: either byte order, deflated, RLE, PNG, TIFF.
    for (name, syntax) in [
        ("wide-explicit.dcm", EXPLICIT_LE),
        ("wide-implicit.dcm", IMPLICIT_LE),
        ("wide-deflated.dcm", DEFLATED_LE),
    ] {
        write_native(
            &at(name),
            Native {
                transfer_syntax: syntax,
                ..Native::gray("2.25.5101", (4, 4), 16)
            },
            le16(&wide),
        );
    }
    write_native(
        &at("wide-big-endian.dcm"),
        Native {
            transfer_syntax: EXPLICIT_BE,
            ..Native::gray("2.25.5101", (4, 4), 16)
        },
        be16(&wide),
    );
    coded::write(
        &at("wide-rle.dcm"),
        RLE,
        Layout::gray(4, 4, 16, 16),
        &[rle_frame(&[high, low])],
    );
    std::fs::write(
        at("wide.png"),
        rasters::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            (4, 4),
            &be16(&wide),
            |_| {},
        ),
    )
    .expect("write");
    std::fs::write(
        at("wide-ii.tif"),
        tiff(false, (4, 4), &[16], 1, le16(&wide)),
    )
    .expect("write");
    std::fs::write(at("wide-mm.tif"), tiff(true, (4, 4), &[16], 1, be16(&wide))).expect("write");

    // Eight bits: uncompressed, RLE, lossless JPEG XL, PNG, TIFF.
    let gray = pattern(1);
    write_native(
        &at("gray-native.dcm"),
        Native::gray("2.25.5102", (16, 16), 8),
        gray.clone(),
    );
    coded::write(
        &at("gray-rle.dcm"),
        RLE,
        Layout::gray8(16, 16),
        &[rle_frame(std::slice::from_ref(&gray))],
    );
    coded::write(
        &at("gray-jxl.dcm"),
        coded::JPEG_XL,
        Layout::gray8(16, 16),
        &[coded::hex(coded::JXL_GRAY8_PATTERN)],
    );
    std::fs::write(
        at("gray.png"),
        rasters::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            (16, 16),
            &gray,
            |_| {},
        ),
    )
    .expect("write");
    std::fs::write(at("gray.tif"), tiff(false, (16, 16), &[8], 1, gray.clone())).expect("write");

    // Colour: by pixel, by plane, lossless JPEG XL, PNG, TIFF.
    let colour = pattern(3);
    let rgb = Native {
        samples: 3,
        photometric: "RGB",
        ..Native::gray("2.25.5103", (16, 16), 8)
    };
    write_native(&at("colour-by-pixel.dcm"), rgb, colour.clone());
    write_native(
        &at("colour-by-plane.dcm"),
        Native { planar: 1, ..rgb },
        planes(&colour, 3),
    );
    coded::write(
        &at("colour-jxl.dcm"),
        coded::JPEG_XL,
        Layout::color8(16, 16, "RGB"),
        &[coded::hex(coded::JXL_RGB8_PATTERN)],
    );
    std::fs::write(
        at("colour.png"),
        rasters::png_of(
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            (16, 16),
            &colour,
            |_| {},
        ),
    )
    .expect("write");
    std::fs::write(
        at("colour.tif"),
        tiff(false, (16, 16), &[8, 8, 8], 2, colour.clone()),
    )
    .expect("write");

    let viewer = Viewer::of(dir.path()).await;
    let groups: [(&str, &[&str]); 3] = [
        (
            WIDE,
            &[
                "wide-explicit.dcm",
                "wide-implicit.dcm",
                "wide-deflated.dcm",
                "wide-big-endian.dcm",
                "wide-rle.dcm",
                "wide.png",
                "wide-ii.tif",
                "wide-mm.tif",
            ],
        ),
        (
            PATTERN_GRAY,
            &[
                "gray-native.dcm",
                "gray-rle.dcm",
                "gray-jxl.dcm",
                "gray.png",
                "gray.tif",
            ],
        ),
        (
            PATTERN_RGB,
            &[
                "colour-by-pixel.dcm",
                "colour-by-plane.dcm",
                "colour-jxl.dcm",
                "colour.png",
                "colour.tif",
            ],
        ),
    ];
    assert_eq!(
        viewer.files.len(),
        groups.iter().map(|(_, names)| names.len()).sum::<usize>()
    );
    assert_eq!(viewer.stats(), PixelDigestStats::default());
    let mut hashed = 0;
    for (expected, names) in groups {
        for name in names {
            viewer.raw(name, 0).await;
            assert_eq!(viewer.held(name, 0).as_deref(), Some(expected), "{name}");
            hashed += 1;
        }
    }
    assert_eq!(viewer.stats().frames_hashed, hashed);
    assert_eq!(viewer.stats().held, hashed as usize);
}

/// What is part of a digest and what is not, each as two files that differ
/// in one thing.
#[tokio::test]
async fn a_digest_follows_the_samples_and_their_format_and_nothing_else() {
    let dir = tempdir().expect("temp dir");
    let at = |name: &str| dir.path().join(name);
    let wide = wide_samples();
    let gray = Native::gray("2.25.5201", (4, 4), 16);
    write_native(&at("plain.dcm"), gray, le16(&wide));
    // The same bytes as signed numbers.
    write_native(
        &at("signed.dcm"),
        Native {
            signed: true,
            uid: "2.25.5202",
            ..gray
        },
        le16(&wide),
    );
    // The same bytes shown inverted.
    write_native(
        &at("inverted.dcm"),
        Native {
            photometric: "MONOCHROME1",
            uid: "2.25.5203",
            ..gray
        },
        le16(&wide),
    );
    // Twelve bits of each sample, once with the other four clear and once
    // with them set: they are not pixel data.
    let twelve: Vec<u16> = wide.iter().map(|sample| sample & 0x0fff).collect();
    let dirty: Vec<u16> = twelve.iter().map(|sample| sample | 0xf000).collect();
    for (name, uid, samples) in [
        ("twelve.dcm", "2.25.5204", &twelve),
        ("twelve-dirty.dcm", "2.25.5205", &dirty),
    ] {
        write_native(
            &at(name),
            Native {
                bits_stored: 12,
                uid,
                ..gray
            },
            le16(samples),
        );
    }
    // The low bytes of the samples as an image of 8-bit samples, and the
    // same values in 16 bits.
    let narrow: Vec<u8> = wide.iter().map(|sample| *sample as u8).collect();
    let widened: Vec<u16> = narrow.iter().map(|sample| u16::from(*sample)).collect();
    write_native(
        &at("narrow.dcm"),
        Native::gray("2.25.5206", (4, 4), 8),
        narrow.clone(),
    );
    write_native(
        &at("widened.dcm"),
        Native {
            uid: "2.25.5207",
            ..gray
        },
        le16(&widened),
    );
    // Another shape of the same sixteen samples.
    write_native(
        &at("reshaped.dcm"),
        Native {
            rows: 2,
            columns: 8,
            uid: "2.25.5208",
            ..gray
        },
        le16(&wide),
    );
    // One-bit samples packed eight to a byte, and the same values a byte
    // each.
    let bits: Vec<u8> = (0..16_u8).map(|i| (i * 5 + i / 3) % 2).collect();
    std::fs::write(
        at("bits.png"),
        rasters::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::One,
            (8, 2),
            &packed(&bits, 8, 1),
            |_| {},
        ),
    )
    .expect("write");
    std::fs::write(
        at("bytes.png"),
        rasters::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            (8, 2),
            &bits,
            |_| {},
        ),
    )
    .expect("write");
    // Indices into a palette, and the colours they name.
    let palette = [10_u8, 20, 30, 200, 100, 50, 7, 77, 177];
    let indices = [0_u8, 1, 2, 1];
    let colours: Vec<u8> = indices
        .iter()
        .flat_map(|index| palette[usize::from(*index) * 3..][..3].to_vec())
        .collect();
    std::fs::write(
        at("indexed.png"),
        rasters::png_of(
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            (2, 2),
            &indices,
            |encoder| encoder.set_palette(palette.to_vec()),
        ),
    )
    .expect("write");
    // A TIFF page that says white is zero, as stored.
    std::fs::write(
        at("white-is-zero.tif"),
        tiff(false, (4, 4), &[16], 0, le16(&wide)),
    )
    .expect("write");
    // Signed and floating-point samples of an image file.
    std::fs::write(
        at("signed.tif"),
        rasters::tiff_file(
            false,
            &[TiffPage::strip((4, 4), &[16], 1, le16(&wide)).with(339, TiffValue::Short(vec![2]))],
        )
        .0,
    )
    .expect("write");
    let reals: Vec<u8> = [1.5_f32, -2.25]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    std::fs::write(
        at("reals.tif"),
        rasters::tiff_file(
            false,
            &[TiffPage::strip((2, 1), &[32], 1, reals).with(339, TiffValue::Short(vec![3]))],
        )
        .0,
    )
    .expect("write");
    // One-bit samples of a DICOM frame, packed from the low bit: a byte
    // each once decoded.
    let packed_low_first: Vec<u8> = bits
        .chunks(8)
        .map(|byte| {
            byte.iter()
                .enumerate()
                .fold(0, |packed, (bit, value)| packed | value << bit)
        })
        .collect();
    write_native(
        &at("bits.dcm"),
        Native {
            bits_stored: 1,
            ..Native::gray("2.25.5209", (2, 8), 1)
        },
        packed_low_first,
    );
    // 64-bit floating-point samples.
    let doubles: Vec<u8> = [0.5_f64, -3.25, 1e300, 7.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    std::fs::write(
        at("doubles.tif"),
        rasters::tiff_file(
            false,
            &[TiffPage::strip((2, 2), &[64], 1, doubles.clone())
                .with(339, TiffValue::Short(vec![3]))],
        )
        .0,
    )
    .expect("write");
    // A header that says colour-by-plane over a coding that is not stored
    // that way: only an uncompressed frame is put in order.
    let by_plane = Native {
        samples: 3,
        planar: 1,
        photometric: "RGB",
        ..Native::gray("2.25.5210", (16, 16), 8)
    };
    write_native(&at("planes.dcm"), by_plane, planes(&pattern(3), 3));
    coded::rewrite(
        &at("planes.dcm"),
        &at("planes-jxl.dcm"),
        coded::JPEG_XL,
        &[coded::hex(coded::JXL_RGB8_PATTERN)],
    );
    let colour_planes: Vec<Vec<u8>> = (0..3)
        .map(|channel| {
            pattern(3)
                .iter()
                .skip(channel)
                .step_by(3)
                .copied()
                .collect()
        })
        .collect();
    coded::rewrite(
        &at("planes.dcm"),
        &at("planes-rle.dcm"),
        RLE,
        &[rle_frame(&colour_planes)],
    );
    // Unassociated alpha is a sample like the others.
    let rgba: Vec<u8> = (0..16_u8).map(|i| i * 13 + 5).collect();
    std::fs::write(
        at("alpha.tif"),
        rasters::tiff_file(
            false,
            &[TiffPage::strip((2, 2), &[8, 8, 8, 8], 2, rgba.clone())
                .with(338, TiffValue::Short(vec![2]))],
        )
        .0,
    )
    .expect("write");

    let viewer = Viewer::of(dir.path()).await;
    let expected: Vec<(&str, String)> = vec![
        ("plain.dcm", WIDE.to_string()),
        ("signed.dcm", WIDE_SIGNED.to_string()),
        ("inverted.dcm", WIDE.to_string()),
        ("twelve.dcm", px((4, 4), 1, 16, 0, &le16(&twelve))),
        ("twelve-dirty.dcm", px((4, 4), 1, 16, 0, &le16(&twelve))),
        ("narrow.dcm", px((4, 4), 1, 8, 0, &narrow)),
        ("widened.dcm", px((4, 4), 1, 16, 0, &le16(&widened))),
        ("reshaped.dcm", px((2, 8), 1, 16, 0, &le16(&wide))),
        ("bits.png", px((2, 8), 1, 8, 0, &bits)),
        ("bytes.png", px((2, 8), 1, 8, 0, &bits)),
        ("indexed.png", px((2, 2), 3, 8, 0, &colours)),
        ("white-is-zero.tif", WIDE.to_string()),
        ("signed.tif", WIDE_SIGNED.to_string()),
        (
            "reals.tif",
            "px:9c247bf6f656594e4f051d4acb60962d6417b4fb0dff36c923e2c7f23c23e134".to_string(),
        ),
        ("bits.dcm", px((2, 8), 1, 8, 0, &bits)),
        ("doubles.tif", px((2, 2), 1, 64, 2, &doubles)),
        ("planes.dcm", PATTERN_RGB.to_string()),
        ("planes-jxl.dcm", PATTERN_RGB.to_string()),
        ("planes-rle.dcm", PATTERN_RGB.to_string()),
        ("alpha.tif", px((2, 2), 4, 8, 0, &rgba)),
    ];
    assert_eq!(viewer.files.len(), expected.len());
    for (name, digest) in &expected {
        viewer.raw(name, 0).await;
        assert_eq!(viewer.held(name, 0).as_ref(), Some(digest), "{name}");
    }
    // The pairs that must differ do.
    let of = |name: &str| {
        &expected
            .iter()
            .find(|(file, _)| *file == name)
            .expect("case")
            .1
    };
    for (one, other) in [
        ("plain.dcm", "signed.dcm"),
        ("narrow.dcm", "widened.dcm"),
        ("plain.dcm", "reshaped.dcm"),
        ("plain.dcm", "twelve.dcm"),
    ] {
        assert_ne!(of(one), of(other), "{one} and {other}");
    }
}

/// The cells of a 16 x 16 frame of 12-bit samples allocated 16 bits: the
/// sample in bits 0 to 11, and in bits 12 to 15 nothing (`junk` false) or
/// bits that are not pixel data.
fn stored_cells(junk: bool) -> Vec<u16> {
    (0..256_u32)
        .map(|i| {
            let sample = (i * 37 + 11) & 0x0fff;
            let above = if junk { ((i * 5 + 3) & 0xf) << 12 } else { 0 };
            (sample | above) as u16
        })
        .collect()
}

/// [`stored_cells`] (unsigned, bits 12 to 15 set to other things), coded as 16 bits: `dcmcjpeg +e1` (DCMTK 3.7.0).
const JPEG_LOSSLESS_U_JUNK: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b100010001001011100ffc4001600010101000000\
    000000000000000000000f0a0cffda0008010100010000300a50255025502550255025502550255025502550\
    2550255025502550255025a50502550255025502550255025502550255025502550255025502550255025a50\
    502550255025502550255025502550255025502550255025502550255025a505025502550255025502550255\
    02550255025502550255025502550255025a5050255025502550255025502550255025502550255025502550\
    2550255025a50502550255025502550255025502550255025502550255025502550255025a50502550255025\
    502550255025502550255025502550255025502550254025c49ea04aa04aa04aa04aa04aa04aa04aa04aa04a\
    a04aa04aa04aa04aa04aa04b4a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04b4\
    a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04b4a0a04aa04aa04aa04aa04aa04\
    aa04aa04aa04aa04aa04aa04aa04aa04aa04b4a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04a\
    a04aa04aa04b4a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04b4a0a04aa04aa0\
    4aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04a804aa04b893d4095409540954095409540954095409540\
    9540954095409540954095409694140954095409540954095409540954095409540954095409540954095409\
    7fffffd9\
";

/// [`stored_cells`] (unsigned, bits 12 to 15 set to other things), coded as 16 bits: `dcmcjpls` (DCMTK 3.7.0).
const JPEG_LS_U_JUNK: &str = "\
    ffd8fff7000b100010001001011100fff8000d01ffff0012004301140040ffda000801010000000000000041\
    50000000000892604730229810d4082a03f501ea80ed4072a037501aa80cd4062a02f5016a5024f24fc0049c\
    49d80126926e004984964998024a25024924824bc01239256009189270048a4884918048649e48448248f804\
    8048d8047e48b8047c47a4898047849c47647448780472485804704838046e46c481808d49348d08cc8ff011\
    911f6023123dc046045e479808b89308b48b08ef011591d6022a239c04524504718089c076903d9c0080072f\
    802001cb60080072b80402001ca600010013611391a60226233c044a4658089088c8c7022264c221220230c0\
    87c8bf021e22ec08748708b7021b24b21a21922cc08608af021722ac08588548a7021424a213212228c08448\
    9f0210226c083c838897020d24920c20b224c082888f0209222c082081c8870206248205204220c080c87f02\
    0221ec080480087700080300103ffc020037cc04006f780800deb0101001bcc0000000000047ff691ca1c804\
    8670200218c1ff485f07fcff242d83fc2460ffd9\
";

/// [`stored_cells`] (signed, bits 12 to 15 clear), coded as 16 bits: `dcmcjpeg +e1` (DCMTK 3.7.0).
const JPEG_LOSSLESS_S_CLEAN: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b100010001001011100ffc4001700010101010000\
    00000000000000000000060a0c0fffda0008010100010000e0014952a54a952a54a952a54a952a5a504a952a\
    54a952a54a952a54a952d28254a952a54a952a54a952a54a969412a54a952a54a952a54a952a54b4a0952a54\
    a952a54a952a54a952a5a504a952a54a952a54a952a54a952d28254a952a54a952a54a952a54b8093127a54a\
    952a54a952a54a952a54a969412a54a952a54a952a54a952a54b4a0952a54a952a54a952a54a952a5a504a95\
    2a54a952a54a952a54a952d28254a952a54a952a54a952a54a969412a54a952a54a952a54a952a54b4a0952a\
    54a952a54a952a54a9701225c49e952a54a952a54a952a54a952a5a504a952a54a952a54a952a54a952fffd9\
";

/// [`stored_cells`] (signed, bits 12 to 15 clear), coded as 16 bits: `dcmcjpls` (DCMTK 3.7.0).
const JPEG_LS_S_CLEAN: &str = "\
    ffd8fff7000b100010001001011100fff8000d01ffff0012004301140040ffda000801010000000041589312\
    624c4791643a0cfe24c4791643a0cfe7a94112a2448d2250a813e9e4ea716d696561129282bab2aaa29a928a\
    827a7a72d5ab112127ac58ad5aa54a952952a54a850919272850a13a74e9d3a74c993264c91126a64c9932aa\
    aaaaaaaaaa8a28493145145145144d34d34d34c000000037e07698010040100804020101010101010100127d\
    28269a69a69a69a6924924922164c5294a5294a5294a4a510125a5294a5294a5294a5287c929294a5294aaaa\
    aaaa1e2495554444444444447492111111111111100000000000047ffa0001dc602020408102040810204080\
    127a3849c82828282828282828282828282828d8ffd9\
";

/// [`stored_cells`] (signed, bits 12 to 15 set to other things), coded as 16 bits: `dcmcjpeg +e1` (DCMTK 3.7.0).
const JPEG_LOSSLESS_S_JUNK: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b100010001001011100ffc4001600010101000000\
    000000000000000000000f0a0cffda0008010100010000300a50255025502550255025502550255025502550\
    2550255025502550255025a50502550255025502550255025502550255025502550255025502550255025a50\
    502550255025502550255025502550255025502550255025502550255025a505025502550255025502550255\
    02550255025502550255025502550255025a5050255025502550255025502550255025502550255025502550\
    2550255025a50502550255025502550255025502550255025502550255025502550255025a50502550255025\
    502550255025502550255025502550255025502550254025c49ea04aa04aa04aa04aa04aa04aa04aa04aa04a\
    a04aa04aa04aa04aa04aa04b4a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04b4\
    a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04b4a0a04aa04aa04aa04aa04aa04\
    aa04aa04aa04aa04aa04aa04aa04aa04aa04b4a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04a\
    a04aa04aa04b4a0a04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04b4a0a04aa04aa0\
    4aa04aa04aa04aa04aa04aa04aa04aa04aa04aa04a804aa04b893d4095409540954095409540954095409540\
    9540954095409540954095409694140954095409540954095409540954095409540954095409540954095409\
    7fffffd9\
";

/// [`stored_cells`] (signed, bits 12 to 15 set to other things), coded as 16 bits: `dcmcjpls` (DCMTK 3.7.0).
const JPEG_LS_S_JUNK: &str = "\
    ffd8fff7000b100010001001011100fff8000d01ffff0012004301140040ffda000801010000000000000041\
    50000000000892604730229810d4082a03f501ea80ed4072a037501aa80cd4062a02f5016a5024f24fc0049c\
    49d80126926e004984964998024a25024924824bc01239256009189270048a4884918048649e48448248f804\
    8048d8047e48b8047c47a4898047849c47647448780472485804704838046e46c481808d49348d08cc8ff011\
    911f6023123dc046045e479808b89308b48b08ef011591d6022a239c04524504718089c076903d9c0080072f\
    802001cb60080072b80402001ca600010013611391a60226233c044a4658089088c8c7022264c221220230c0\
    87c8bf021e22ec08748708b7021b24b21a21922cc08608af021722ac08588548a7021424a213212228c08448\
    9f0210226c083c838897020d24920c20b224c082888f0209222c082081c8870206248205204220c080c87f02\
    0221ec080480087700080300103ffc020037cc04006f780800deb0101001bcc0000000000047ff691ca1c804\
    8670200218c1ff485f07fcff242d83fc2460ffd9\
";

/// [`stored_cells`] (unsigned, bits 12 to 15 set to other things), coded as 16 bits:
/// `opj_compress -F 16,16,1,16,u -n 3` (OpenJPEG 2.5.4) of the cells as little-endian words
/// (`.rawl`).
const J2K_U_JUNK: &str = "\
    ff4fff5100290000000000100000001000000000000000000000001000000010000000000000000000010f01\
    01ff52000c00000001000204040001ff5c000a4080888890888890ff64002500014372656174656420627920\
    4f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000c40001ff93c7fe0c3e1c52e988\
    92164d695c8e4212328a478a6fc38fb23ff1a3e8662f21e5ab83abcffc487a07f811803f60801c2642767276\
    b085e2b9a4ccb1e2a37b81ee4ae91b8a5c14242668f38f4f00cf325adc0fb6122c896826418106b28f2033c2\
    de2a2e2e36cffc489607f81980ff008017227d3676dbba4e9e1e4e1b2c41027f2b17c24a65c6ff6d74bae40f\
    ff5e08bc89fd53c9f800d3120f2bb4f108b4c300f3a2c9735c0da2afcf9247746b6137eb051c265fff7fffd9\
";

/// [`stored_cells`] (unsigned, bits 12 to 15 set to other things), coded as 16 bits:
/// `cjxl -d 0` (libjxl 0.11.2) of the cells as a 16-bit PGM.
const JXL_U_JUNK: &str = "\
    0000000c4a584c200d0a870a00000014667479706a786c20000000006a786c20000000096a786c6c0a000001\
    096a786c63ff0a43f00314370208040100cc034b386a18d2e3fb2b6cefafa060222323fa458cc1cd88c10c2b\
    c660cd182362307a037d19ff7c33ff0f14e0eb4f0225b0fd4aa0043ebdb75f0994c0f60bf22f81122881ed57\
    029f7825108542a15028140a8542a15028140a8542a15028146aef078542a15028140a8542a15028140a8542\
    a15028140a8542a15028140a8542a1242b5028140a8542a15028d4b56b2236d7ae5d7b35bf06000a8542a17e\
    bd5128140a8542a15028140a8542a15028140a8542a15028140a854255bda05028140a8542a15028140a8542\
    a15028140a8542a15028140a8542a150c753d7ae5d2357b876ed1a6c110028140a8542a15028140a8502\
";

/// Bits outside the stored field are not pixel data, whichever decoder
/// hands them over: a frame whose cells carry other things there, and a
/// signed frame whose cells leave them clear, have the digest of their
/// values in every lossless coding, the one the uncompressed file has.
#[tokio::test]
async fn bits_outside_the_stored_field_reach_no_digest_in_any_coding() {
    let dir = tempdir().expect("temp dir");
    let at = |name: &str| dir.path().join(name);
    let twelve = Native {
        bits_stored: 12,
        ..Native::gray("2.25.5251", (16, 16), 16)
    };
    let signed = Native {
        signed: true,
        ..twelve
    };
    // The values: unsigned with the bits above clear, signed with the sign
    // carried up.
    let values: Vec<u16> = stored_cells(false);
    let signed_values: Vec<u16> = values
        .iter()
        .map(|value| {
            if value & 0x0800 != 0 {
                value | 0xf000
            } else {
                *value
            }
        })
        .collect();
    assert!(signed_values.iter().any(|value| value & 0x8000 != 0));
    let unsigned_digest = px((16, 16), 1, 16, 0, &le16(&values));
    let signed_digest = px((16, 16), 1, 16, 1, &le16(&signed_values));
    assert_ne!(unsigned_digest, signed_digest);

    // Each kind uncompressed, then the same cells in each coding.
    let kinds: [(&str, Native, bool, &String); 3] = [
        ("unsigned-junk", twelve, true, &unsigned_digest),
        ("signed-clean", signed, false, &signed_digest),
        ("signed-junk", signed, true, &signed_digest),
    ];
    let coded_frames: [(&str, &str, &str, &str); 8] = [
        (
            "unsigned-junk",
            "jpeg-lossless",
            coded::JPEG_LOSSLESS_SV1,
            JPEG_LOSSLESS_U_JUNK,
        ),
        (
            "signed-clean",
            "jpeg-lossless",
            coded::JPEG_LOSSLESS_SV1,
            JPEG_LOSSLESS_S_CLEAN,
        ),
        (
            "signed-junk",
            "jpeg-lossless",
            coded::JPEG_LOSSLESS_SV1,
            JPEG_LOSSLESS_S_JUNK,
        ),
        ("unsigned-junk", "jpeg-ls", coded::JPEG_LS, JPEG_LS_U_JUNK),
        ("signed-clean", "jpeg-ls", coded::JPEG_LS, JPEG_LS_S_CLEAN),
        ("signed-junk", "jpeg-ls", coded::JPEG_LS, JPEG_LS_S_JUNK),
        ("unsigned-junk", "jpeg-2000", coded::JPEG_2000, J2K_U_JUNK),
        ("unsigned-junk", "jpeg-xl", coded::JPEG_XL, JXL_U_JUNK),
    ];
    let mut expected: Vec<(String, &String)> = Vec::new();
    for (kind, native, junk, digest) in kinds {
        let source = at(&format!("{kind}.dcm"));
        write_native(&source, native, le16(&stored_cells(junk)));
        expected.push((format!("{kind}.dcm"), digest));
        // RLE: the high bytes, then the low bytes.
        let cells = stored_cells(junk);
        let planes = [
            cells.iter().map(|cell| (cell >> 8) as u8).collect(),
            cells.iter().map(|cell| *cell as u8).collect(),
        ];
        let name = format!("{kind}-rle.dcm");
        coded::rewrite(&source, &at(&name), RLE, &[rle_frame(&planes)]);
        expected.push((name, digest));
        for (_, coding, syntax, frame) in coded_frames.iter().filter(|frame| frame.0 == kind) {
            let name = format!("{kind}-{coding}.dcm");
            coded::rewrite(&source, &at(&name), syntax, &[coded::hex(frame)]);
            expected.push((name, digest));
        }
    }
    assert_eq!(expected.len(), 14);

    let viewer = Viewer::of(dir.path()).await;
    assert_eq!(viewer.files.len(), expected.len());
    for (name, digest) in &expected {
        viewer.raw(name, 0).await;
        assert_eq!(viewer.held(name, 0).as_ref(), Some(*digest), "{name}");
    }
}

/// A frame has a digest only when decoding it gives exactly the samples
/// the file stores. The others are served as before and are never hashed.
#[tokio::test]
async fn a_frame_decoded_inexactly_has_no_digest() {
    let dir = tempdir().expect("temp dir");
    let at = |name: &str| dir.path().join(name);
    support::write_encapsulated_dicom(
        &at("baseline.dcm"),
        coded::JPEG_BASELINE,
        vec![support::grayscale_jpeg_fragment_16x16(3)],
    );
    std::fs::write(
        at("photo.jpg"),
        rasters::baseline_jpeg(image::ExtendedColorType::L8, (16, 16), &pattern(1)),
    )
    .expect("write");
    std::fs::write(at("lossy.webp"), rasters::lossy_webp()).expect("write");
    std::fs::write(
        at("lossless.webp"),
        rasters::lossless_webp(image::ExtendedColorType::Rgb8, (16, 16), &pattern(3)),
    )
    .expect("write");
    // Colour that was multiplied by its alpha is divided again on decode.
    let rgba: Vec<u8> = (0..16_u8).map(|i| i * 13 + 5).collect();
    std::fs::write(
        at("associated.tif"),
        rasters::tiff_file(
            false,
            &[
                TiffPage::strip((2, 2), &[8, 8, 8, 8], 2, rgba)
                    .with(338, TiffValue::Short(vec![1])),
            ],
        )
        .0,
    )
    .expect("write");
    // Chrominance at half resolution: two samples a pixel.
    write_native(
        &at("subsampled.dcm"),
        Native {
            samples: 3,
            photometric: "YBR_FULL_422",
            ..Native::gray("2.25.5301", (2, 4), 8)
        },
        (0..16_u8).map(|i| i * 9 + 40).collect(),
    );
    // And one that has a digest, so the test would see hashing at all.
    write_native(
        &at("exact.dcm"),
        Native::gray("2.25.5302", (4, 4), 16),
        le16(&wide_samples()),
    );

    let viewer = Viewer::of(dir.path()).await;
    let inexact = [
        "baseline.dcm",
        "photo.jpg",
        "lossy.webp",
        "lossless.webp",
        "associated.tif",
        "subsampled.dcm",
    ];
    assert_eq!(viewer.files.len(), inexact.len() + 1);
    for name in inexact {
        let file = &viewer.files[viewer.index(name)];
        assert_eq!(
            exact_samples(file),
            name == "subsampled.dcm",
            "{name}: only the half-resolution frame is ruled out by its samples"
        );
        assert!(!viewer.raw(name, 0).await.is_empty(), "{name}");
        // The display tier too, which for one sample a pixel reads the
        // raw tier.
        let index = viewer.index(name);
        viewer.get(&format!("/api/file/{index}/frame/0")).await;
        assert_eq!(viewer.held(name, 0), None, "{name}");
    }
    assert_eq!(viewer.stats(), PixelDigestStats::default());

    assert!(exact_samples(&viewer.files[viewer.index("exact.dcm")]));
    viewer.raw("exact.dcm", 0).await;
    assert_eq!(viewer.held("exact.dcm", 0).as_deref(), Some(WIDE));
    assert_eq!(
        viewer.stats(),
        PixelDigestStats {
            frames_hashed: 1,
            bytes_hashed: 24 + 32,
            held: 1
        }
    );
}

// ---------------------------------------------------------------------------
// When a digest is computed, and what it costs

/// Three frames of 4 x 4 16-bit samples, each frame its own values.
fn three_frames() -> Vec<Vec<u16>> {
    (0..3_u16)
        .map(|frame| {
            wide_samples()
                .iter()
                .map(|sample| sample.wrapping_mul(frame * 2 + 1))
                .collect()
        })
        .collect()
}

fn write_three_frames(path: &Path, uid: &'static str) {
    write_native(
        path,
        Native {
            frames: 3,
            ..Native::gray(uid, (4, 4), 16)
        },
        three_frames()
            .iter()
            .flat_map(|frame| le16(frame))
            .collect(),
    );
}

fn frame_digest(frame: usize) -> String {
    px((4, 4), 1, 16, 0, &le16(&three_frames()[frame]))
}

/// One digest for each raw decode, in the table by the time the frame is
/// answered, and none for anything that is not a raw decode.
#[tokio::test]
async fn a_digest_is_computed_once_by_the_decode_of_its_frame() {
    let dir = tempdir().expect("temp dir");
    write_three_frames(&dir.path().join("volume.dcm"), "2.25.5401");
    let viewer = Viewer::of(dir.path()).await;
    let frame_bytes = 24 + 32;
    let stats = |frames: u64| PixelDigestStats {
        frames_hashed: frames,
        bytes_hashed: frames * frame_bytes,
        held: frames as usize,
    };

    // Listing the files and reading about them decodes nothing.
    for path in [
        "/api/files",
        "/api/file/0/info",
        "/api/file/0/tags",
        "/api/series",
    ] {
        viewer.get(path).await;
    }
    assert_eq!(viewer.stats(), stats(0));
    // A thumbnail is not a raw decode.
    viewer.get("/api/file/0/frame/0/thumbnail").await;
    assert_eq!(viewer.stats(), stats(0));

    // The raw samples of frame 1: its digest, and only its.
    let samples = viewer.raw("volume.dcm", 1).await;
    assert_eq!(samples, le16(&three_frames()[1]));
    assert_eq!(viewer.held("volume.dcm", 1), Some(frame_digest(1)));
    assert_eq!(viewer.held("volume.dcm", 0), None);
    assert_eq!(viewer.held("volume.dcm", 2), None);
    assert_eq!(viewer.stats(), stats(1));

    // Again, from the cache: nothing is hashed.
    viewer.raw("volume.dcm", 1).await;
    viewer
        .get("/api/file/0/frame/1/raw/pixel?row=0&column=0")
        .await;
    assert_eq!(viewer.stats(), stats(1));

    // A display frame of one sample a pixel decodes through the raw tier,
    // once, whatever windows it is then shown with.
    viewer.get("/api/file/0/frame/2").await;
    assert_eq!(viewer.held("volume.dcm", 2), Some(frame_digest(2)));
    assert_eq!(viewer.stats(), stats(2));
    viewer.get("/api/file/0/frame/2?wc=100&ww=50").await;
    viewer.get("/api/file/0/frame/2?mode=full_dynamic").await;
    viewer.raw("volume.dcm", 2).await;
    assert_eq!(viewer.stats(), stats(2));

    // No decode is waiting or running, and none ran for a digest alone:
    // the scheduler never held more than the decodes above reserved.
    let load = viewer.state.decode_scheduler().load();
    assert_eq!(
        (
            load.running,
            load.waiting_interactive,
            load.waiting_background
        ),
        (0, 0, 0)
    );
}

/// A session records digests however its caches and its decode memory
/// were sized at startup: each of those builds the raw cache anew.
#[tokio::test]
async fn a_session_records_digests_however_its_caches_were_sized() {
    let dir = tempdir().expect("temp dir");
    write_three_frames(&dir.path().join("volume.dcm"), "2.25.5451");
    type Build = fn(AppState) -> AppState;
    let builds: [(&str, Build); 4] = [
        ("as built", |state| state),
        ("with decode limits", |state| {
            state.with_decode_limits(DecodeLimits::DEFAULT)
        }),
        ("with a cache budget", |state| {
            state.with_cache_budget(CacheBudget::DEFAULT)
        }),
        ("with both, as startup builds it", |state| {
            state
                .with_decode_limits(DecodeLimits::DEFAULT)
                .with_cache_budget(CacheBudget::DEFAULT)
        }),
    ];
    for (name, build) in builds {
        let registry = FileRegistry::from_files(Viewer::listed(dir.path()).await);
        let state = build(AppState::new(registry.clone(), AnnotationStore::empty()));
        let server = TestServer::new(server::router(state));
        server
            .get("/api/file/0/frame/1/raw")
            .await
            .assert_status_ok();
        assert_eq!(
            registry
                .pixel_digests()
                .get(0, 1)
                .map(|digest| text(&digest)),
            Some(frame_digest(1)),
            "{name}"
        );
        assert_eq!(registry.pixel_digests().stats().frames_hashed, 1, "{name}");
    }
}

/// Redaction boxes and windows are applied to what is sent, never to what
/// is hashed.
#[tokio::test]
async fn boxes_and_windows_do_not_change_a_digest() {
    let dir = tempdir().expect("temp dir");
    write_three_frames(&dir.path().join("volume.dcm"), "2.25.5501");
    let viewer = Viewer::of(dir.path()).await;
    viewer
        .server
        .put("/api/file/0/redactions")
        .json(&json!({ "num_roi": 1, "roi_coords": [[0, 0, 2, 4]], "roi_frames": [] }))
        .await
        .assert_status_ok();

    // The samples that are sent have the box filled; the digest is of the
    // samples the file stores.
    let sent = viewer.raw("volume.dcm", 0).await;
    assert_ne!(sent, le16(&three_frames()[0]));
    assert_eq!(viewer.held("volume.dcm", 0), Some(frame_digest(0)));
    viewer.get("/api/file/0/frame/1?wc=1000&ww=10").await;
    assert_eq!(viewer.held("volume.dcm", 1), Some(frame_digest(1)));

    // Taking the boxes away hashes nothing again and changes nothing.
    viewer
        .server
        .put("/api/file/0/redactions")
        .json(&json!({ "num_roi": 0, "roi_coords": [], "roi_frames": [] }))
        .await
        .assert_status_ok();
    assert_eq!(viewer.raw("volume.dcm", 0).await, le16(&three_frames()[0]));
    assert_eq!(viewer.held("volume.dcm", 0), Some(frame_digest(0)));
    assert_eq!(viewer.stats().frames_hashed, 2);
}

// ---------------------------------------------------------------------------
// Documents

/// A document states the digests of the frames that were decoded, `null`
/// for the others, and nothing for a file none of whose frames was. An
/// export reads the table: it decodes nothing, hashes nothing and reads no
/// file for it.
#[tokio::test]
async fn a_document_states_the_digests_of_the_frames_that_were_decoded() {
    let dir = tempdir().expect("temp dir");
    write_three_frames(&dir.path().join("seen.dcm"), "2.25.5601");
    write_three_frames(&dir.path().join("unseen.dcm"), "2.25.5602");
    let viewer = Viewer::of(dir.path()).await;
    viewer.annotate("seen.dcm", 1).await;
    viewer.annotate("unseen.dcm", 2).await;
    viewer.raw("seen.dcm", 2).await;
    viewer.raw("seen.dcm", 0).await;
    let before = viewer.stats();
    assert_eq!(before.frames_hashed, 2);

    // The files are gone: nothing an export states of them is read now.
    std::fs::remove_file(dir.path().join("seen.dcm")).expect("remove");
    std::fs::remove_file(dir.path().join("unseen.dcm")).expect("remove");

    let document = viewer.document().await;
    assert_eq!(
        reference(&document, "seen.dcm")["pixel_digest"],
        json!([frame_digest(0), null, frame_digest(2)])
    );
    assert_eq!(
        reference(&document, "unseen.dcm")["pixel_digest"],
        json!([])
    );
    // A DICOM file has no pages.
    assert_eq!(
        reference(&document, "seen.dcm")["frame_source"],
        Value::Null
    );

    // JSON Lines says the same, and so does either through the
    // compression layer.
    let lines = viewer.get("/api/annotations/export.jsonl").await.text();
    let seen = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("line"))
        .find(|line| {
            line["file"]["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("seen.dcm"))
        })
        .expect("the file's line");
    assert_eq!(
        seen["file"]["pixel_digest"],
        json!([frame_digest(0), null, frame_digest(2)])
    );
    let packed = viewer
        .server
        .get("/api/annotations/export.json")
        .add_header("accept-encoding", "gzip")
        .await;
    packed.assert_status_ok();
    assert_eq!(packed.header("content-encoding"), "gzip");
    let mut unpacked = String::new();
    flate2::read::GzDecoder::new(packed.as_bytes().as_ref())
        .read_to_string(&mut unpacked)
        .expect("gzip");
    let unpacked: Value = serde_json::from_str(&unpacked).expect("document");
    assert_eq!(unpacked["files"], document["files"]);

    // Three exports later nothing more was hashed and no decode ran.
    assert_eq!(viewer.stats(), before);
    assert_eq!(viewer.state.decode_scheduler().load().running, 0);

    // What an export wrote, an import reads: the document is valid.
    let other = tempdir().expect("temp dir");
    write_three_frames(&other.path().join("seen.dcm"), "2.25.5601");
    write_three_frames(&other.path().join("unseen.dcm"), "2.25.5602");
    let reader = Viewer::importing(other.path()).await;
    let report = reader.import(other.path(), &document.to_string()).await;
    assert_eq!(report["counts"]["files"], 2, "{report}");
    assert_eq!(report["counts"]["annotations_out"], 2, "{report}");
    // The reader decoded nothing, so it states nothing, whatever the
    // document it read stated: a document's digests are not the session's.
    assert_eq!(reader.stats(), PixelDigestStats::default());
    let again = reader.document().await;
    assert_eq!(reference(&again, "seen.dcm")["pixel_digest"], json!([]));
}

/// Files with the same bytes are one image: a frame decoded on any copy is
/// stated in the one reference they share.
#[tokio::test]
async fn copies_share_the_digests_of_the_frames_decoded_on_any_of_them() {
    let dir = tempdir().expect("temp dir");
    for folder in ["a", "b"] {
        std::fs::create_dir(dir.path().join(folder)).expect("folder");
        write_three_frames(&dir.path().join(folder).join("copy.dcm"), "2.25.5701");
    }
    // The same UID over other pixels is another image, with another key.
    std::fs::create_dir(dir.path().join("c")).expect("folder");
    write_native(
        &dir.path().join("c").join("other.dcm"),
        Native {
            frames: 3,
            ..Native::gray("2.25.5701", (4, 4), 16)
        },
        le16(&[7; 48]),
    );
    let viewer = Viewer::of(dir.path()).await;
    assert_eq!(viewer.files.len(), 3);
    viewer.annotate("b/copy.dcm", 1).await;
    viewer.annotate("c/other.dcm", 2).await;
    assert_eq!(
        viewer.key("a/copy.dcm").await,
        viewer.key("b/copy.dcm").await
    );
    viewer.raw("b/copy.dcm", 1).await;
    viewer.raw("a/copy.dcm", 0).await;
    viewer.raw("c/other.dcm", 2).await;
    assert_eq!(viewer.stats().frames_hashed, 3);

    let document = viewer.document().await;
    assert_eq!(document["files"].as_array().expect("files").len(), 2);
    assert_eq!(
        reference(&document, "a/copy.dcm")["pixel_digest"],
        json!([frame_digest(0), frame_digest(1), null])
    );
    assert_eq!(
        reference(&document, "c/other.dcm")["pixel_digest"],
        json!([null, null, px((4, 4), 1, 16, 0, &le16(&[7; 16]))])
    );
}

/// An image file's reference names the page of each frame, and each page
/// has its own digest.
#[tokio::test]
async fn a_raster_reference_names_the_page_and_the_digest_of_each_frame() {
    let dir = tempdir().expect("temp dir");
    let first: Vec<u8> = (0..16_u8).map(|i| i * 3 + 1).collect();
    let second: Vec<u8> = (0..16_u8).map(|i| 250 - i * 5).collect();
    // A reduced-resolution page between two pages of the image: it is not
    // a frame, and the frames keep the numbers of their pages.
    let pages = [
        TiffPage::strip((4, 4), &[8], 1, first.clone()),
        TiffPage::strip((2, 2), &[8], 1, vec![1, 2, 3, 4]).with(254, TiffValue::Long(vec![1])),
        TiffPage::strip((4, 4), &[8], 1, second.clone()),
    ];
    std::fs::write(
        dir.path().join("pages.tif"),
        rasters::tiff_file(false, &pages).0,
    )
    .expect("write");
    std::fs::write(
        dir.path().join("single.png"),
        rasters::png_of(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            (4, 4),
            &first,
            |_| {},
        ),
    )
    .expect("write");
    let viewer = Viewer::of(dir.path()).await;
    viewer.annotate("pages.tif", 1).await;
    viewer.annotate("single.png", 2).await;
    viewer.raw("pages.tif", 1).await;
    assert_eq!(viewer.stats().frames_hashed, 1);

    let document = viewer.document().await;
    let pages = reference(&document, "pages.tif");
    assert_eq!(pages["frames"], 2);
    assert_eq!(pages["frame_source"], json!([0, 2]));
    assert_eq!(
        pages["pixel_digest"],
        json!([null, px((4, 4), 1, 8, 0, &second)])
    );
    let single = reference(&document, "single.png");
    assert_eq!(single["frame_source"], json!([0]));
    assert_eq!(single["pixel_digest"], json!([]));

    viewer.raw("pages.tif", 0).await;
    viewer.raw("single.png", 0).await;
    let document = viewer.document().await;
    // The first page of the TIFF and the PNG hold the same samples.
    let same = px((4, 4), 1, 8, 0, &first);
    assert_eq!(
        reference(&document, "pages.tif")["pixel_digest"],
        json!([same, px((4, 4), 1, 8, 0, &second)])
    );
    assert_eq!(
        reference(&document, "single.png")["pixel_digest"],
        json!([same])
    );
}

/// A masked session's document states pixel digests as any other does:
/// they say no more of a file than the digest of its bytes, which it
/// already names its files by.
#[tokio::test]
async fn a_masked_document_states_pixel_digests() {
    let dir = tempdir().expect("temp dir");
    write_three_frames(&dir.path().join("volume.dcm"), "2.25.5801");
    std::fs::write(
        dir.path().join("pages.tif"),
        tiff(false, (4, 4), &[16], 1, le16(&wide_samples())),
    )
    .expect("write");
    let viewer = Viewer::masked(dir.path()).await;
    viewer.annotate("volume.dcm", 1).await;
    viewer.annotate("pages.tif", 2).await;
    viewer.raw("volume.dcm", 1).await;
    viewer.raw("pages.tif", 0).await;
    assert_eq!(viewer.stats().frames_hashed, 2);

    let document = viewer.document().await;
    assert_eq!(document["extensions"]["dcmview"]["masked"], true);
    let files = document["files"].as_array().expect("files");
    assert_eq!(files.len(), 2);
    let stated: Vec<&Value> = files.iter().map(|file| &file["pixel_digest"]).collect();
    assert!(
        stated.contains(&&json!([null, frame_digest(1), null])),
        "{stated:?}"
    );
    assert!(stated.contains(&&json!([WIDE])), "{stated:?}");
    let sources: Vec<&Value> = files.iter().map(|file| &file["frame_source"]).collect();
    assert!(sources.contains(&&json!([0])) && sources.contains(&&Value::Null));
    // And still nothing that names a file but its content.
    let text = document.to_string();
    for hidden in ["2.25.5801", "volume.dcm", "pages.tif", "PIXELS"] {
        assert!(!text.contains(hidden), "{hidden} is in a masked document");
    }
}

/// A reference to a file of this module's three frames, by its UID.
fn by_uid(path: &Path, uid: &str, pixel_digest: Value) -> Value {
    json!({
        "key": format!("sop:{uid}"), "kind": "dicom", "sop_instance_uid": uid,
        "path": path.file_name().and_then(|name| name.to_str()).expect("name"),
        "size_bytes": std::fs::metadata(path).expect("file").len(),
        "rows": 4, "columns": 4, "frames": 3, "format": "dicom",
        "pixel_digest": pixel_digest,
    })
}

fn document_of(file: Value) -> String {
    let key = file["key"].as_str().expect("key").to_string();
    json!({
        "format": "dcmview.annotations", "version": "1.0",
        "files": [file], "layers": [], "annotations": [record(1, &key)],
    })
    .to_string()
}

/// Where the bytes of a file were not compared with the document, the
/// pixel digests both sides have are: one that differs and the records
/// stay off the file. A digest only one side has is no evidence, bytes
/// that were compared decide alone, and an import never decodes or hashes
/// to find out.
#[tokio::test]
async fn an_import_holds_a_file_matched_by_its_uid_to_the_pixels_both_sides_know() {
    const UID: &str = "2.25.5901";
    let wrong = format!("px:{}", "0".repeat(64));
    struct Case {
        name: &'static str,
        /// The frames decoded here before the import.
        decoded: &'static [u32],
        /// What the document states of each frame.
        stated: [Option<String>; 3],
        /// Whether the document also states the digest of the file's bytes.
        with_bytes: bool,
        placed: bool,
    }
    let cases = [
        Case {
            name: "the frame both know agrees",
            decoded: &[0],
            stated: [Some(frame_digest(0)), None, None],
            with_bytes: false,
            placed: true,
        },
        Case {
            name: "the frame both know differs",
            decoded: &[0],
            stated: [Some(wrong.clone()), None, None],
            with_bytes: false,
            placed: false,
        },
        Case {
            name: "one of two frames both know differs",
            decoded: &[0, 2],
            stated: [Some(frame_digest(0)), None, Some(wrong.clone())],
            with_bytes: false,
            placed: false,
        },
        Case {
            name: "nothing was decoded here",
            decoded: &[],
            stated: [
                Some(wrong.clone()),
                Some(wrong.clone()),
                Some(wrong.clone()),
            ],
            with_bytes: false,
            placed: true,
        },
        Case {
            name: "the frames that differ were not decoded here",
            decoded: &[1],
            stated: [
                Some(wrong.clone()),
                Some(frame_digest(1)),
                Some(wrong.clone()),
            ],
            with_bytes: false,
            placed: true,
        },
        Case {
            name: "the document states none",
            decoded: &[0, 1, 2],
            stated: [None, None, None],
            with_bytes: false,
            placed: true,
        },
        Case {
            name: "the bytes were compared and are the same",
            decoded: &[0],
            stated: [Some(wrong.clone()), None, None],
            with_bytes: true,
            placed: true,
        },
    ];
    for case in cases {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("volume.dcm");
        write_three_frames(&path, UID);
        let viewer = Viewer::importing(dir.path()).await;
        for frame in case.decoded {
            viewer.raw("volume.dcm", *frame).await;
        }
        let before = viewer.stats();
        assert_eq!(
            before.frames_hashed,
            case.decoded.len() as u64,
            "{}",
            case.name
        );

        let mut file = by_uid(&path, UID, json!(case.stated));
        if case.with_bytes {
            let bytes = std::fs::read(&path).expect("read");
            file["digest"] = json!(format!("b3:{}", blake3::hash(&bytes).to_hex()));
        }
        let report = viewer.import(dir.path(), &document_of(file)).await;
        let placed = u64::from(case.placed);
        assert_eq!(report["counts"]["files"], placed, "{}: {report}", case.name);
        assert_eq!(
            report["counts"]["annotations_out"], placed,
            "{}: {report}",
            case.name
        );
        assert_eq!(
            report["counts"]["refused"],
            1 - placed,
            "{}: {report}",
            case.name
        );
        assert_eq!(
            warned(&report, "evidence_conflict"),
            1 - placed,
            "{}: {report}",
            case.name
        );
        assert_eq!(viewer.rois("volume.dcm").await, placed, "{}", case.name);
        // The import hashed nothing and put nothing in the table.
        assert_eq!(viewer.stats(), before, "{}", case.name);
        assert_eq!(
            viewer.state.decode_scheduler().load().running,
            0,
            "{}",
            case.name
        );
    }
}

/// Files with the same bytes are one image for an import too: what was
/// decoded on one copy is held against a reference whichever copy it is
/// compared with, so no record lands on a key whose pixels the session
/// knows to be others.
#[tokio::test]
async fn an_import_holds_a_reference_to_the_pixels_decoded_on_any_copy() {
    const UID: &str = "2.25.5951";
    let wrong = format!("px:{}", "3".repeat(64));
    for (stated, placed) in [(frame_digest(1), 1), (wrong, 0)] {
        let dir = tempdir().expect("temp dir");
        for folder in ["a", "b"] {
            std::fs::create_dir(dir.path().join(folder)).expect("folder");
            write_three_frames(&dir.path().join(folder).join("copy.dcm"), UID);
        }
        let viewer = Viewer::importing(dir.path()).await;
        // Decoded on the copy the reference is not compared with first.
        viewer.raw("b/copy.dcm", 1).await;
        let before = viewer.stats();
        let file = by_uid(
            &dir.path().join("a").join("copy.dcm"),
            UID,
            json!([null, stated, null]),
        );
        let report = viewer.import(dir.path(), &document_of(file)).await;
        assert_eq!(report["counts"]["files"], placed, "{report}");
        assert_eq!(warned(&report, "evidence_conflict"), 1 - placed, "{report}");
        for copy in ["a/copy.dcm", "b/copy.dcm"] {
            assert_eq!(viewer.rois(copy).await, placed, "{copy}");
        }
        assert_eq!(viewer.stats(), before);
    }
}

/// A masked session shows masked UIDs, and its import finds the copies of
/// a file under the UID it shows: what was decoded on one copy is held
/// against a reference there too.
#[tokio::test]
async fn a_masked_import_holds_a_reference_to_the_pixels_decoded_on_any_copy() {
    const UID: &str = "2.25.5961";
    let wrong = format!("px:{}", "4".repeat(64));
    for (stated, placed) in [(frame_digest(1), 1), (wrong, 0)] {
        let dir = tempdir().expect("temp dir");
        for folder in ["a", "b"] {
            std::fs::create_dir(dir.path().join(folder)).expect("folder");
            write_three_frames(&dir.path().join(folder).join("copy.dcm"), UID);
        }
        let registry = FileRegistry::masked(Arc::new(Masker::new()));
        for file in Viewer::listed(dir.path()).await {
            registry.insert(file);
        }
        registry.mark_scan_complete();
        let viewer = Viewer::over(registry, AnnotationStore::loading());
        viewer.raw("b/copy.dcm", 1).await;
        let before = viewer.stats();
        assert_eq!(before.frames_hashed, 1);
        // The reference as this session shows the file: a key and a UID
        // that are not the file's own.
        let key = viewer.key("a/copy.dcm").await;
        let shown = key.strip_prefix("sop:").expect("a UID key");
        assert_ne!(shown, UID);
        let mut file = by_uid(
            &dir.path().join("a").join("copy.dcm"),
            shown,
            json!([null, stated, null]),
        );
        file["path"] = json!("a/copy.dcm");
        let report = viewer.import(dir.path(), &document_of(file)).await;
        assert_eq!(report["counts"]["files"], placed, "{report}");
        assert_eq!(warned(&report, "evidence_conflict"), 1 - placed, "{report}");
        assert_eq!(viewer.rois("a/copy.dcm").await, placed);
        assert_eq!(viewer.stats(), before);
    }
}

/// Only files with the reference's key are its copies. A file that carries
/// the same UID over other bytes has a key of its own, and what was decoded
/// on it says nothing of the file the reference names.
#[tokio::test]
async fn pixels_decoded_on_a_file_with_another_key_are_not_held_against_a_reference() {
    const UID: &str = "2.25.5971";
    let dir = tempdir().expect("temp dir");
    for folder in ["a", "b"] {
        std::fs::create_dir(dir.path().join(folder)).expect("folder");
    }
    let named = dir.path().join("a").join("volume.dcm");
    write_three_frames(&named, UID);
    // The same UID, size and shape over other pixels.
    write_native(
        &dir.path().join("b").join("volume.dcm"),
        Native {
            frames: 3,
            ..Native::gray(UID, (4, 4), 16)
        },
        le16(&[7; 48]),
    );
    // The first file's key is relied on before the second is found, so it
    // keeps its UID key and the second is keyed by its content.
    let mut files = Viewer::listed(dir.path()).await.into_iter();
    let registry = FileRegistry::new();
    registry.insert(files.next().expect("first"));
    let key = registry.ensure_key(0).await.expect("key");
    assert_eq!(key.as_str(), format!("sop:{UID}"));
    registry.insert(files.next().expect("second"));
    registry.mark_scan_complete();
    let viewer = Viewer::over(registry, AnnotationStore::loading());
    viewer.raw("b/volume.dcm", 0).await;
    assert_eq!(
        viewer.held("b/volume.dcm", 0),
        Some(px((4, 4), 1, 16, 0, &le16(&[7; 16])))
    );

    // The other file's key is settled too, and is not the UID's.
    assert_ne!(viewer.key("b/volume.dcm").await, format!("sop:{UID}"));

    // What the document states of frame 0 is true of the file it names and
    // differs from what was decoded on the other.
    let mut file = by_uid(&named, UID, json!([frame_digest(0), null, null]));
    file["path"] = json!("a/volume.dcm");
    let report = viewer.import(dir.path(), &document_of(file)).await;
    assert_eq!(report["counts"]["files"], 1, "{report}");
    assert_eq!(warned(&report, "evidence_conflict"), 0, "{report}");
    assert_eq!(viewer.rois("a/volume.dcm").await, 1);
    assert_eq!(viewer.rois("b/volume.dcm").await, 0);
}

/// A reference by content is compared by its bytes, which decide: its
/// pixel digests are not looked at, and a file whose bytes changed is not
/// found by the pixels it kept.
#[tokio::test]
async fn pixel_digests_find_no_file_and_overrule_no_bytes() {
    let dir = tempdir().expect("temp dir");
    let samples = le16(&wide_samples());
    let page = |software: &str| {
        rasters::tiff_file(
            false,
            &[TiffPage::strip((4, 4), &[16], 1, samples.clone())
                .with(305, TiffValue::Bytes(software.as_bytes().to_vec()))],
        )
        .0
    };
    let original = page("first writer");
    let resaved = page("other writer");
    assert_eq!(original.len(), resaved.len());
    std::fs::write(dir.path().join("scan.tif"), &resaved).expect("write");
    let content = |bytes: &[u8]| format!("b3:{}", blake3::hash(bytes).to_hex());
    let stated = |bytes: &[u8], pixels: &str| {
        json!({
            "key": content(bytes), "kind": "raster", "digest": content(bytes),
            "path": "scan.tif", "size_bytes": bytes.len(), "rows": 4, "columns": 4,
            "frames": 1, "format": "tiff", "pixel_digest": [pixels],
        })
    };
    let wrong = format!("px:{}", "1".repeat(64));

    // The file's own bytes with pixels it does not have: the bytes decide.
    let viewer = Viewer::importing(dir.path()).await;
    viewer.raw("scan.tif", 0).await;
    assert_eq!(viewer.held("scan.tif", 0).as_deref(), Some(WIDE));
    let report = viewer
        .import(dir.path(), &document_of(stated(&resaved, &wrong)))
        .await;
    assert_eq!(report["counts"]["files"], 1, "{report}");
    assert_eq!(viewer.rois("scan.tif").await, 1);

    // Other bytes with the pixels the file has: not this file.
    let viewer = Viewer::importing(dir.path()).await;
    viewer.raw("scan.tif", 0).await;
    let before = viewer.stats();
    let report = viewer
        .import(dir.path(), &document_of(stated(&original, WIDE)))
        .await;
    assert_eq!(report["counts"]["files"], 0, "{report}");
    assert_eq!(warned(&report, "evidence_conflict"), 1, "{report}");
    assert_eq!(viewer.rois("scan.tif").await, 0);
    assert_eq!(viewer.stats(), before);
}

/// However many digests a document states, an import computes none: the
/// length of the list drives no decode, and a digest that is not one is
/// refused before any file is looked at.
#[tokio::test]
async fn the_digests_a_document_states_cost_no_decode() {
    const FRAMES: u32 = 4096;
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("long.dcm");
    write_native(
        &path,
        Native {
            frames: FRAMES,
            ..Native::gray("2.25.6001", (1, 2), 8)
        },
        (0..FRAMES * 2).map(|i| (i % 251) as u8).collect(),
    );
    let reference = |digests: Value| {
        json!({
            "key": "sop:2.25.6001", "kind": "dicom", "sop_instance_uid": "2.25.6001",
            "path": "long.dcm", "size_bytes": std::fs::metadata(&path).expect("file").len(),
            "rows": 1, "columns": 2, "frames": FRAMES, "format": "dicom",
            "pixel_digest": digests,
        })
    };
    let wrong = format!("px:{}", "2".repeat(64));

    // Every frame stated, none decoded here: placed, and nothing decoded.
    let viewer = Viewer::importing(dir.path()).await;
    let report = viewer
        .import(
            dir.path(),
            &document_of(reference(json!(vec![wrong.clone(); FRAMES as usize]))),
        )
        .await;
    assert_eq!(report["counts"]["files"], 1, "{report}");
    assert_eq!(viewer.stats(), PixelDigestStats::default());
    assert_eq!(
        viewer.state.decode_scheduler().load().peak_reserved_bytes,
        0
    );

    // A list of another length than the file's frames, and an entry that
    // is not a pixel digest, fail the import as before.
    for (name, digests) in [
        ("too short", json!([wrong.clone()])),
        ("not a digest", {
            let mut digests = vec![json!(null); FRAMES as usize];
            digests[7] = json!("px:1234");
            json!(digests)
        }),
        ("a file digest", {
            let mut digests = vec![json!(null); FRAMES as usize];
            digests[0] = json!(format!("b3:{}", "2".repeat(64)));
            json!(digests)
        }),
    ] {
        let viewer = Viewer::importing(dir.path()).await;
        let document = dir.path().join(format!("{}.json", name.replace(' ', "-")));
        std::fs::write(&document, document_of(reference(digests))).expect("write");
        let source = NativeSource::from_path(&document).expect("source");
        let failed = import_native(&viewer.registry, &viewer.store, &source, Arc::new(|| true))
            .await
            .expect_err(name);
        assert!(
            matches!(failed, AnnotationError::ImportFailed(_)),
            "{name}: {failed}"
        );
        assert_eq!(viewer.stats(), PixelDigestStats::default(), "{name}");
    }
}

/// The table is a session's: a frame recorded past its capacity forgets
/// the one recorded longest ago, and what a document states of a file is
/// what the table still holds.
#[tokio::test]
async fn a_document_states_what_the_table_still_holds() {
    let dir = tempdir().expect("temp dir");
    write_three_frames(&dir.path().join("volume.dcm"), "2.25.6101");
    let viewer = Viewer::of(dir.path()).await;
    viewer.annotate("volume.dcm", 1).await;
    viewer.raw("volume.dcm", 0).await;
    viewer.raw("volume.dcm", 1).await;
    // As many frames of other files as the table holds, less one: the
    // first frame recorded is the one that has to go.
    let digests = viewer.registry.pixel_digests();
    for other in 0..PIXEL_DIGEST_CAPACITY - 1 {
        digests.record(1 + other / 1000, (other % 1000) as u32, [9; 32], 56);
    }
    assert_eq!(viewer.stats().held, PIXEL_DIGEST_CAPACITY);
    assert_eq!(viewer.held("volume.dcm", 0), None);
    assert_eq!(
        reference(&viewer.document().await, "volume.dcm")["pixel_digest"],
        json!([null, frame_digest(1), null])
    );
}
