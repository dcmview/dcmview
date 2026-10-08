//! Raster image files (PNG, JPEG, TIFF, WebP) in discovery and the catalog
//! (`docs/design/image-formats.md` sections 3 to 5 and 11).
//!
//! Every case writes its files into a temporary directory and runs the
//! production loader over it, then reads the catalog through the router, so
//! what is asserted is what a client of `/api/files` sees. The files are
//! built here with the linked encoders: the expected values are the ones the
//! files were written with, never values a decoder returned.
//!
//! What the frames of these files hold is in `raster_decode.rs`, and what
//! decoding one may cost in `raster_bounds.rs`. Listing raster metadata as
//! tags is later work.

use super::{raster_files, support};
use axum_test::TestServer;
use dcmview::api::contracts::{
    endpoints, RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_COLUMNS,
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, RAW_FRAME_HEADER_PIXEL_REPRESENTATION,
    RAW_FRAME_HEADER_RESCALE_INTERCEPT, RAW_FRAME_HEADER_RESCALE_SLOPE, RAW_FRAME_HEADER_ROWS,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
};
use dcmview::loader::{self, DiscoverOptions, FormatSelection};
use dcmview::masking::Masker;
use dcmview::server::{self, FileRegistry};
use dcmview::types::{FileEntry, FileFormat};
use image::{ExtendedColorType, ImageEncoder};
use serde_json::{json, Value};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::tempdir;
use tiff::encoder::{colortype, TiffEncoder};
use tiff::tags::Tag;
use tokio::sync::mpsc;

const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";

// ---------------------------------------------------------------------------
// Files

pub(super) fn png(color: ExtendedColorType, width: u32, height: u32) -> Vec<u8> {
    let bytes = usize::from(color.bits_per_pixel()).div_ceil(8) * (width * height) as usize;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&vec![0x40; bytes], width, height, color)
        .expect("encode PNG");
    out
}

fn png_with_icc_profile() -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut out);
    encoder
        .set_icc_profile(b"not a real profile, only present".to_vec())
        .expect("PNG carries a profile");
    encoder
        .write_image(&[0x40; 2 * 2 * 3], 2, 2, ExtendedColorType::Rgb8)
        .expect("encode PNG");
    out
}

/// A PNG written chunk by chunk through the `png` crate, for what `image`'s
/// encoder does not write: low bit depths, palettes and animation.
pub(super) fn png_with(
    color: png::ColorType,
    depth: png::BitDepth,
    (width, height): (u32, u32),
    row_bytes: usize,
    configure: impl FnOnce(&mut png::Encoder<'_, &mut Vec<u8>>),
    frames: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    configure(&mut encoder);
    let mut writer = encoder.write_header().expect("write PNG header");
    for _ in 0..frames {
        writer
            .write_image_data(&vec![0; row_bytes * height as usize])
            .expect("write PNG frame");
    }
    writer.finish().expect("finish PNG");
    out
}

pub(super) fn jpeg(color: ExtendedColorType, width: u32, height: u32) -> Vec<u8> {
    let bytes = usize::from(color.bits_per_pixel()) / 8 * (width * height) as usize;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&vec![0x40; bytes], width, height, color)
        .expect("encode JPEG");
    out
}

/// Four pages: 4x3 16-bit, a 2x2 16-bit thumbnail, 4x3 16-bit again, and a
/// 4x3 8-bit page. Pages 0 and 2 are the frames.
fn tiff_stack() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut out).expect("TIFF encoder");
    let page = |encoder: &mut TiffEncoder<_>, width: u32, height: u32| {
        encoder
            .write_image::<colortype::Gray16>(width, height, &vec![7; (width * height) as usize])
            .expect("write 16-bit page");
    };
    page(&mut encoder, 4, 3);
    page(&mut encoder, 2, 2);
    page(&mut encoder, 4, 3);
    encoder
        .write_image::<colortype::Gray8>(4, 3, &[7; 12])
        .expect("write 8-bit page");
    out.into_inner()
}

/// One page with extra tags written over the encoder's own.
pub(super) fn tiff_page<C>(big: bool, data: &[C::Inner], tags: &[(Tag, u16)]) -> Vec<u8>
where
    C: colortype::ColorType,
    [C::Inner]: tiff::encoder::TiffValue,
{
    fn write<C, K>(
        mut encoder: TiffEncoder<&mut Cursor<Vec<u8>>, K>,
        data: &[C::Inner],
        tags: &[(Tag, u16)],
    ) where
        C: colortype::ColorType,
        [C::Inner]: tiff::encoder::TiffValue,
        K: tiff::encoder::TiffKind,
    {
        let mut image = encoder.new_image::<C>(2, 2).expect("TIFF page");
        for (tag, value) in tags {
            image.encoder().write_tag(*tag, *value).expect("TIFF tag");
        }
        image.write_data(data).expect("TIFF samples");
    }
    let mut out = Cursor::new(Vec::new());
    if big {
        write::<C, _>(TiffEncoder::new_big(&mut out).expect("BigTIFF"), data, tags);
    } else {
        write::<C, _>(TiffEncoder::new(&mut out).expect("TIFF"), data, tags);
    }
    out.into_inner()
}

/// A classic little-endian TIFF written tag by tag, one IFD per page and no
/// pixel data, for layouts and page counts the encoder does not write. Every
/// value is a SHORT; a page's tags are given in ascending order.
pub(super) fn tiff_of(pages: &[&[(u16, &[u16])]]) -> Vec<u8> {
    let mut out = b"II\x2a\0\x08\0\0\0".to_vec();
    for (index, tags) in pages.iter().enumerate() {
        let mut data_at = out.len() + 2 + tags.len() * 12 + 4;
        let mut data = Vec::new();
        out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        for (tag, values) in tags.iter() {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&3_u16.to_le_bytes());
            out.extend_from_slice(&(values.len() as u32).to_le_bytes());
            let bytes = values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>();
            if bytes.len() <= 4 {
                out.extend_from_slice(&bytes);
                out.resize(out.len() + 4 - bytes.len(), 0);
            } else {
                out.extend_from_slice(&(data_at as u32).to_le_bytes());
                data_at += bytes.len();
                data.extend(bytes);
            }
        }
        let next = if index + 1 == pages.len() { 0 } else { data_at };
        out.extend_from_slice(&(next as u32).to_le_bytes());
        out.extend(data);
    }
    out
}

/// `count` 2x2 one-bit gray pages.
fn tiff_of_pages(count: usize) -> Vec<u8> {
    let page: &[(u16, &[u16])] = &[(256, &[2]), (257, &[2]), (262, &[1])];
    tiff_of(&vec![page; count])
}

pub(super) fn webp_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(
            &vec![0x40; (width * height * 4) as usize],
            width,
            height,
            ExtendedColorType::Rgba8,
        )
        .expect("encode WebP");
    out
}

pub(super) fn write_dicom(path: &Path) {
    support::write_uncompressed_u16_dicom(path, EXPLICIT_LE, 2, 2, vec![1, 2, 3, 4], None, None);
}

// ---------------------------------------------------------------------------
// Harness

/// One completed discovery, served: the catalog as `/api/files` reports it.
pub(super) struct Scan {
    pub(super) server: TestServer,
    pub(super) report: loader::DiscoveryReport,
    /// The loader's own entries, for the sample layout a decoder will read.
    pub(super) entries: Vec<FileEntry>,
    pub(super) catalog: Value,
}

impl Scan {
    /// The catalog entry of the file named `name`.
    pub(super) fn file(&self, name: &str) -> &Value {
        self.catalog["files"]
            .as_array()
            .expect("files array")
            .iter()
            .find(|file| file_name(file["path"].as_str().expect("path")) == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog: {}", self.catalog["files"]))
    }

    pub(super) fn index(&self, name: &str) -> String {
        self.file(name)["index"].to_string()
    }

    pub(super) fn entry(&self, name: &str) -> &FileEntry {
        self.entries
            .iter()
            .find(|entry| entry.path.file_name().and_then(|name| name.to_str()) == Some(name))
            .unwrap_or_else(|| panic!("{name} was not selected"))
    }

    /// File names in the catalog, sorted.
    pub(super) fn loaded(&self) -> Vec<String> {
        let mut names = self.catalog["files"]
            .as_array()
            .expect("files array")
            .iter()
            .map(|file| file_name(file["path"].as_str().expect("path")).to_string())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// `(file name, reason)` of every skipped or filtered path, sorted.
    pub(super) fn not_loaded(&self) -> Vec<(String, String)> {
        let mut records = self.catalog["discovery"]
            .as_array()
            .expect("discovery array")
            .iter()
            .map(|record| {
                (
                    file_name(record["path"].as_str().expect("path")).to_string(),
                    record["reason"].as_str().expect("reason").to_string(),
                )
            })
            .collect::<Vec<_>>();
        records.sort();
        records
    }
}

pub(super) fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

pub(super) fn options(formats: FormatSelection, filters: &[&str]) -> DiscoverOptions {
    DiscoverOptions {
        recursive: true,
        filters: filters
            .iter()
            .map(|filter| filter.parse().expect("filter parses"))
            .collect(),
        formats,
    }
}

/// Runs the production loader over `paths` into `registry` the way startup
/// does, and serves the result.
pub(super) async fn scan_into(
    paths: &[PathBuf],
    options: DiscoverOptions,
    registry: FileRegistry,
) -> Scan {
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let discover = loader::discover_progressive(
        paths,
        options,
        events_tx,
        loader::DiscoveryCancellation::new(),
    );
    let record = async {
        let mut entries = Vec::new();
        while let Some(event) = events_rx.recv().await {
            match event {
                loader::DiscoveryEvent::Selected { file, record } => {
                    registry.record_discovery(record);
                    entries.push((*file).clone());
                    registry.insert(*file);
                }
                loader::DiscoveryEvent::SkippedInput(record)
                | loader::DiscoveryEvent::FilteredInput(record) => {
                    registry.record_discovery(record)
                }
            }
        }
        entries
    };
    let (report, entries) = tokio::join!(discover, record);
    let report = report.expect("discovery completes");
    registry.mark_scan_complete();
    let server = TestServer::new(server::router(support::app_state_with_registry(registry)));
    let catalog = server.get("/api/files").await.json();
    Scan {
        server,
        report,
        entries,
        catalog,
    }
}

pub(super) async fn scan(paths: &[PathBuf], options: DiscoverOptions) -> Scan {
    scan_into(paths, options, FileRegistry::new()).await
}

pub(super) async fn scan_dir(dir: &Path) -> Scan {
    scan(&[dir.to_path_buf()], options(FormatSelection::all(), &[])).await
}

// ---------------------------------------------------------------------------
// Tests

/// What the catalog reports for one raster file.
struct Expected {
    name: &'static str,
    bytes: Vec<u8>,
    format: &'static str,
    /// `(rows, columns)` of the stored pixel grid.
    size: (u32, u32),
    frames: u32,
    /// `raster` fields that differ from [`raster_defaults`].
    raster: Value,
    /// `(samples_per_pixel, bits_allocated, pixel_representation,
    /// photometric_interpretation)` of the raw frames the file is served as.
    layout: (u32, u32, u32, &'static str),
    /// The default window, `(center, width)`: the whole stored range of
    /// gray samples of 8 bits or fewer, none for wider samples and colour.
    window: Option<(f64, f64)>,
}

/// The `raster` object of a single-page 8-bit gray file with nothing special.
fn raster_defaults() -> Value {
    json!({
        "color_type": "gray",
        "bit_depth": 8,
        "sample_format": "uint",
        "has_alpha": false,
        "orientation": 1,
        "has_icc": false,
        "pages_total": 1,
        "frame_pages": [0],
        "excluded_pages": [],
        "excluded_pages_total": 0,
        "animated": false,
        "significant_bits": null,
    })
}

fn raster_cases() -> Vec<Expected> {
    let gray = (1, 8, 0, "MONOCHROME2");
    vec![
        // 1x1: the whole file is shorter than a DICOM preamble.
        Expected {
            name: "tiny.png",
            bytes: png(ExtendedColorType::L8, 1, 1),
            format: "png",
            size: (1, 1),
            frames: 1,
            raster: json!({}),
            layout: gray,
            window: Some((128.0, 256.0)),
        },
        Expected {
            name: "gray16.png",
            bytes: png(ExtendedColorType::L16, 3, 2),
            format: "png",
            size: (2, 3),
            frames: 1,
            raster: json!({ "bit_depth": 16 }),
            layout: (1, 16, 0, "MONOCHROME2"),
            window: None,
        },
        // One-bit samples are served one byte each.
        Expected {
            name: "bilevel.png",
            bytes: png_with(
                png::ColorType::Grayscale,
                png::BitDepth::One,
                (8, 2),
                1,
                |_| {},
                1,
            ),
            format: "png",
            size: (2, 8),
            frames: 1,
            raster: json!({ "bit_depth": 1 }),
            layout: gray,
            window: Some((1.0, 2.0)),
        },
        Expected {
            name: "rgba.png",
            bytes: png(ExtendedColorType::Rgba8, 3, 2),
            format: "png",
            size: (2, 3),
            frames: 1,
            raster: json!({ "color_type": "rgba", "has_alpha": true }),
            layout: (4, 8, 0, "RGBA"),
            window: None,
        },
        // A palette is described as stored and served expanded.
        Expected {
            name: "palette.png",
            bytes: png_with(
                png::ColorType::Indexed,
                png::BitDepth::Two,
                (4, 2),
                1,
                |encoder| {
                    encoder.set_palette(vec![0, 0, 0, 255, 0, 0, 0, 255, 0]);
                    encoder.set_trns(vec![0, 255, 255]);
                },
                1,
            ),
            format: "png",
            size: (2, 4),
            frames: 1,
            raster: json!({ "color_type": "palette", "bit_depth": 2, "has_alpha": true }),
            layout: (4, 8, 0, "RGBA"),
            window: None,
        },
        Expected {
            name: "animated.png",
            bytes: png_with(
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                (2, 2),
                2,
                |encoder| encoder.set_animated(2, 0).expect("animated PNG"),
                2,
            ),
            format: "png",
            size: (2, 2),
            frames: 1,
            raster: json!({ "animated": true }),
            layout: gray,
            window: Some((128.0, 256.0)),
        },
        Expected {
            name: "profile.png",
            bytes: png_with_icc_profile(),
            format: "png",
            size: (2, 2),
            frames: 1,
            raster: json!({ "color_type": "rgb", "has_icc": true }),
            layout: (3, 8, 0, "RGB"),
            window: None,
        },
        // The format comes from the content, whatever the name says.
        Expected {
            name: "png-named.jpg",
            bytes: png(ExtendedColorType::Rgb8, 2, 2),
            format: "png",
            size: (2, 2),
            frames: 1,
            raster: json!({ "color_type": "rgb" }),
            layout: (3, 8, 0, "RGB"),
            window: None,
        },
        Expected {
            name: "gray.jpg",
            bytes: jpeg(ExtendedColorType::L8, 8, 6),
            format: "jpeg",
            size: (6, 8),
            frames: 1,
            raster: json!({}),
            layout: gray,
            window: Some((128.0, 256.0)),
        },
        // Orientation is reported and never applied: 8 wide, 6 high as stored.
        Expected {
            name: "rotated.jpg",
            bytes: raster_files::with_exif_orientation(jpeg(ExtendedColorType::Rgb8, 8, 6), 6),
            format: "jpeg",
            size: (6, 8),
            frames: 1,
            raster: json!({ "color_type": "rgb", "orientation": 6 }),
            layout: (3, 8, 0, "RGB"),
            window: None,
        },
        Expected {
            name: "stack.tif",
            bytes: tiff_stack(),
            format: "tiff",
            size: (3, 4),
            frames: 2,
            raster: json!({
                "bit_depth": 16,
                "pages_total": 4,
                "frame_pages": [0, 2],
                "excluded_pages": [
                    { "page": 1, "differs": "width" },
                    { "page": 3, "differs": "bits_per_sample" },
                ],
                "excluded_pages_total": 2,
            }),
            layout: (1, 16, 0, "MONOCHROME2"),
            window: None,
        },
        Expected {
            name: "float-big.tif",
            bytes: tiff_page::<colortype::Gray32Float>(true, &[0.5; 4], &[]),
            format: "tiff",
            size: (2, 2),
            frames: 1,
            raster: json!({ "bit_depth": 32, "sample_format": "float" }),
            layout: (1, 32, 0, "MONOCHROME2"),
            window: None,
        },
        Expected {
            name: "signed.tif",
            bytes: tiff_page::<colortype::GrayI16>(false, &[-5; 4], &[]),
            format: "tiff",
            size: (2, 2),
            frames: 1,
            raster: json!({ "bit_depth": 16, "sample_format": "int" }),
            layout: (1, 16, 1, "MONOCHROME2"),
            window: None,
        },
        // WhiteIsZero (photometric 0), rotated 180 degrees (orientation 3).
        Expected {
            name: "inverted.tif",
            bytes: tiff_page::<colortype::Gray8>(
                false,
                &[9; 4],
                &[(Tag::PhotometricInterpretation, 0), (Tag::Orientation, 3)],
            ),
            format: "tiff",
            size: (2, 2),
            frames: 1,
            raster: json!({ "orientation": 3 }),
            layout: (1, 8, 0, "MONOCHROME1"),
            window: Some((128.0, 256.0)),
        },
        Expected {
            name: "lossless.webp",
            bytes: webp_rgba(3, 2),
            format: "webp",
            size: (2, 3),
            frames: 1,
            raster: json!({ "color_type": "rgba", "has_alpha": true }),
            layout: (4, 8, 0, "RGBA"),
            window: None,
        },
    ]
}

#[tokio::test]
async fn rasters_are_listed_by_content_with_what_their_headers_declare() {
    let dir = tempdir().expect("temp dir");
    let cases = raster_cases();
    for case in &cases {
        fs::write(dir.path().join(case.name), &case.bytes).expect("write raster");
    }
    assert!(cases[0].bytes.len() < 132, "tiny.png fits in a preamble");

    let scan = scan_dir(dir.path()).await;

    assert_eq!(scan.report.files_found, cases.len());
    assert_eq!(scan.report.images_found, cases.len());
    for case in &cases {
        let name = case.name;
        let file = scan.file(name);
        assert_eq!(file["file_format"], case.format, "{name}");
        assert_eq!(file["object_kind"], "image", "{name}");
        assert_eq!(
            (file["rows"].as_u64(), file["columns"].as_u64()),
            (Some(case.size.0.into()), Some(case.size.1.into())),
            "{name}"
        );
        assert_eq!(file["frame_count"], case.frames, "{name}");
        assert_eq!(file["has_pixels"], true, "{name}");

        let mut raster = raster_defaults();
        for (field, value) in case.raster.as_object().expect("overrides") {
            raster[field] = value.clone();
        }
        assert_eq!(file["raster"], raster, "{name}");

        // A raster has no DICOM identity and no transfer syntax.
        for field in [
            "patient_id",
            "patient_name",
            "study_instance_uid",
            "series_instance_uid",
            "sop_instance_uid",
            "sop_class_uid",
            "modality",
            "transfer_syntax_uid",
        ] {
            assert_eq!(file[field], "", "{name} {field}");
        }
        assert_eq!(file["label"], name, "{name}");

        // Every one of these decodes, and the catalog says so.
        assert_eq!(file["support_state"], "renderable", "{name}");
        assert!(file["support_reason"].is_null(), "{name}");
        let window = case
            .window
            .map(|(center, width)| json!({ "center": center, "width": width }));
        assert_eq!(
            file["default_window"],
            window.unwrap_or(Value::Null),
            "{name}"
        );

        let entry = scan.entry(name);
        assert_eq!(
            (
                entry.samples_per_pixel,
                entry.bits_allocated,
                entry.pixel_representation,
                entry.photometric_interpretation.as_str(),
            ),
            case.layout,
            "{name}"
        );
    }
}

/// A PNG that compresses well is smaller on disk than one row of its
/// pixels, or than a text chunk it carries. Blank label masks are the
/// everyday case.
#[tokio::test]
async fn compressible_pngs_are_listed_whatever_their_size_on_disk() {
    let blank = |width: u32, height: u32| {
        png_with(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            (width, height),
            width as usize,
            |_| {},
            1,
        )
    };
    // (name, bytes, rows, columns)
    let cases = [
        ("mask-256.png", blank(256, 256), 256, 256),
        ("mask-512.png", blank(512, 512), 512, 512),
        ("strip.png", blank(8192, 8), 8, 8192),
        (
            "wide-rgba.png",
            png(ExtendedColorType::Rgba8, 30_000, 10),
            10,
            30_000,
        ),
        (
            "captioned.png",
            png_with(
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                (4, 4),
                4,
                |encoder| {
                    encoder
                        .add_text_chunk("Comment".to_string(), "a".repeat(200_000))
                        .expect("text chunk")
                },
                1,
            ),
            4,
            4,
        ),
    ];
    let dir = tempdir().expect("temp dir");
    for (name, bytes, _, _) in &cases {
        fs::write(dir.path().join(name), bytes).expect("write PNG");
    }

    let scan = scan_dir(dir.path()).await;

    assert_eq!(scan.not_loaded(), []);
    for (name, _, rows, columns) in cases {
        let file = scan.file(name);
        assert_eq!(file["file_format"], "png", "{name}");
        assert_eq!(
            (file["rows"].as_u64(), file["columns"].as_u64()),
            (Some(rows), Some(columns)),
            "{name}"
        );
    }
}

/// Files built to make a header walk long are skipped, each for what it is,
/// and a TIFF is listed up to exactly the page limit. What such a walk may
/// read is pinned beside the reader, in `src/loader/raster.rs`.
#[tokio::test]
async fn crafted_headers_are_skipped_and_the_tiff_page_limit_is_exact() {
    const PAGE_LIMIT: usize = 65_535;
    let mut fill = vec![0xff, 0xd8];
    fill.resize(512 * 1024, 0xff);
    let mut segments = vec![0xff, 0xd8];
    let mut png_chunks = png(ExtendedColorType::L8, 1, 1)[..33].to_vec();
    let mut webp_chunks = b"RIFF\0\0\0\0WEBP".to_vec();
    for _ in 0..2 * PAGE_LIMIT {
        segments.extend_from_slice(&[0xff, 0xe0, 0x00, 0x02]);
        png_chunks.extend_from_slice(b"\0\0\0\0tEXt\0\0\0\0");
        webp_chunks.extend_from_slice(b"JUNK\0\0\0\0");
    }
    let riff_length = webp_chunks.len() as u32 - 8;
    webp_chunks[4..8].copy_from_slice(&riff_length.to_le_bytes());

    let dir = tempdir().expect("temp dir");
    let skipped = [
        ("fill.jpg", fill),
        ("segments.jpg", segments),
        ("chunks.png", png_chunks),
        ("chunks.webp", webp_chunks),
        ("one-page-too-many.tif", tiff_of_pages(PAGE_LIMIT + 1)),
    ];
    for (name, bytes) in &skipped {
        fs::write(dir.path().join(name), bytes).expect("write crafted file");
    }
    fs::write(dir.path().join("most-pages.tif"), tiff_of_pages(PAGE_LIMIT)).expect("write TIFF");

    let scan = scan_dir(dir.path()).await;

    let mut expected = skipped
        .iter()
        .map(|(name, _)| (name.to_string(), "raster_header_invalid".to_string()))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(scan.not_loaded(), expected);
    assert_eq!(scan.loaded(), ["most-pages.tif"]);
    let most = scan.file("most-pages.tif");
    assert_eq!(most["frame_count"], PAGE_LIMIT);
    assert_eq!(most["raster"]["pages_total"], PAGE_LIMIT);
}

/// A TIFF page that holds any tag twice is not listed: readers disagree on
/// which entry counts, so what the catalog said of it would not be what a
/// decoder is given. As page 0 it makes the file unreadable; as a later
/// page it ends the walk, like any page that cannot be read. Entries in
/// descending order are no reason to refuse a file.
#[tokio::test]
async fn a_tiff_page_that_repeats_a_tag_is_not_listed() {
    use raster_files::{tiff_file, TiffPage, TiffValue};
    let page = || TiffPage::strip((8, 8), &[8], 1, vec![5; 64]);
    // A second entry for `tag`, after the one the page has.
    let twice = |mut page: TiffPage, tag: u16, value: TiffValue| {
        page.tags.push((tag, value));
        page
    };
    let skipped = [
        // A tag discovery reads, one only a decoder reads, and two that
        // nothing reads.
        (
            "photometric.tif",
            twice(page(), 262, TiffValue::Short(vec![0])),
        ),
        (
            "strip-rows.tif",
            twice(page(), 278, TiffValue::Long(vec![1])),
        ),
        (
            "description.tif",
            twice(
                page().with(270, TiffValue::Bytes(b"one\0".to_vec())),
                270,
                TiffValue::Bytes(b"two\0".to_vec()),
            ),
        ),
        (
            "private.tif",
            twice(
                page().with(40_000, TiffValue::Short(vec![1])),
                40_000,
                TiffValue::Short(vec![1]),
            ),
        ),
    ];
    let dir = tempdir().expect("temp dir");
    for (name, page) in &skipped {
        let bytes = tiff_file(false, std::slice::from_ref(page)).0;
        fs::write(dir.path().join(name), bytes).expect("write TIFF");
    }
    let later = tiff_file(
        false,
        &[
            page(),
            twice(page(), 262, TiffValue::Short(vec![0])),
            page(),
        ],
    )
    .0;
    fs::write(dir.path().join("later-page.tif"), later).expect("write TIFF");
    let (mut descending, pages) = tiff_file(true, &[page()]);
    let ifd = pages[0] as usize;
    let count = usize::from(u16::from_be_bytes([descending[ifd], descending[ifd + 1]]));
    let entries: Vec<u8> = descending[ifd + 2..ifd + 2 + 12 * count]
        .chunks(12)
        .rev()
        .flatten()
        .copied()
        .collect();
    descending[ifd + 2..ifd + 2 + 12 * count].copy_from_slice(&entries);
    fs::write(dir.path().join("descending.tif"), descending).expect("write TIFF");

    let scan = scan_dir(dir.path()).await;

    let mut expected = skipped
        .iter()
        .map(|(name, _)| (name.to_string(), "raster_header_invalid".to_string()))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(scan.not_loaded(), expected);
    assert_eq!(scan.loaded(), ["descending.tif", "later-page.tif"]);
    let later = scan.file("later-page.tif");
    assert_eq!(later["frame_count"], 1);
    assert_eq!(later["raster"]["pages_total"], 1);
    assert_eq!(scan.file("descending.tif")["frame_count"], 1);
}

/// A TIFF whose layout no decoder will take is still listed, and says why,
/// instead of vanishing as an unreadable file. A file with many excluded
/// pages counts them all and lists the first few.
#[tokio::test]
async fn tiff_layouts_without_a_decoder_are_listed_as_unsupported_and_exclusions_are_counted() {
    // (name, page 0, raster fields, support_reason)
    type Case<'a> = (&'a str, &'a [(u16, &'a [u16])], Value, &'a str);
    const SIZE: [(u16, &[u16]); 2] = [(256, &[2]), (257, &[2])];
    let cases: [Case<'_>; 4] = [
        // RGB with a fourth sample declared "unspecified", not alpha.
        (
            "extra-unspecified.tif",
            &[
                SIZE[0],
                SIZE[1],
                (258, &[8, 8, 8, 8]),
                (262, &[2]),
                (277, &[4]),
                (338, &[0]),
            ],
            json!({ "color_type": "other", "has_alpha": false }),
            "raster.unsupported_color",
        ),
        (
            "five-bands.tif",
            &[
                SIZE[0],
                SIZE[1],
                (258, &[8, 8, 8, 8, 8]),
                (262, &[1]),
                (277, &[5]),
                (338, &[0, 0, 0, 0]),
            ],
            json!({ "color_type": "other", "has_alpha": false }),
            "raster.unsupported_color",
        ),
        (
            "cielab.tif",
            &[
                SIZE[0],
                SIZE[1],
                (258, &[8, 8, 8]),
                (262, &[8]),
                (277, &[3]),
            ],
            json!({ "color_type": "other" }),
            "raster.unsupported_color",
        ),
        (
            "half-float.tif",
            &[SIZE[0], SIZE[1], (258, &[16]), (262, &[1]), (339, &[3])],
            json!({ "bit_depth": 16, "sample_format": "float" }),
            "raster.unsupported_sample_format",
        ),
    ];
    // Page 0, forty pages of another width, and a second frame.
    let frame: &[(u16, &[u16])] = &[SIZE[0], SIZE[1], (258, &[8]), (262, &[1])];
    let wider: &[(u16, &[u16])] = &[(256, &[3]), SIZE[1], (258, &[8]), (262, &[1])];
    let mut pages = vec![frame];
    pages.extend(vec![wider; 40]);
    pages.push(frame);

    let dir = tempdir().expect("temp dir");
    for (name, page, _, _) in &cases {
        fs::write(dir.path().join(name), tiff_of(&[page])).expect("write TIFF");
    }
    fs::write(dir.path().join("many-excluded.tif"), tiff_of(&pages)).expect("write TIFF");

    let scan = scan_dir(dir.path()).await;

    assert_eq!(scan.not_loaded(), []);
    for (name, _, fields, reason) in cases {
        let file = scan.file(name);
        assert_eq!(file["file_format"], "tiff", "{name}");
        assert_eq!(file["frame_count"], 1, "{name}");
        assert_eq!(
            (file["rows"].as_u64(), file["columns"].as_u64()),
            (Some(2), Some(2))
        );
        let mut raster = raster_defaults();
        for (field, value) in fields.as_object().expect("overrides") {
            raster[field] = value.clone();
        }
        assert_eq!(file["raster"], raster, "{name}");
        assert_eq!(file["support_state"], "unsupported", "{name}");
        assert_eq!(file["support_reason"], reason, "{name}");
    }

    let raster = &scan.file("many-excluded.tif")["raster"];
    assert_eq!(raster["pages_total"], 42);
    assert_eq!(raster["frame_pages"], json!([0, 41]));
    assert_eq!(raster["excluded_pages_total"], 40);
    let listed = raster["excluded_pages"].as_array().expect("excluded pages");
    assert_eq!(listed.len(), 16);
    assert_eq!(listed[0], json!({ "page": 1, "differs": "width" }));
    assert_eq!(listed[15], json!({ "page": 16, "differs": "width" }));
}

#[tokio::test]
async fn dicom_wins_over_a_raster_signature_and_unreadable_files_are_skipped_with_a_reason() {
    let dir = tempdir().expect("temp dir");
    // A DICOM file whose preamble is a TIFF header, as DICOM WSI
    // "dual-personality" files have, stays DICOM.
    let dual = dir.path().join("dual-personality.dcm");
    write_dicom(&dual);
    let mut bytes = fs::read(&dual).expect("read DICOM");
    bytes[..8].copy_from_slice(b"II\x2a\0\x08\0\0\0");
    fs::write(&dual, bytes).expect("write dual-personality file");

    let whole_png = png(ExtendedColorType::L8, 4, 4);
    let whole_jpeg = jpeg(ExtendedColorType::L8, 8, 8);
    let skipped: [(&str, &[u8], &str); 7] = [
        // A signature and nothing after it.
        ("cut.png", &whole_png[..20], "raster_header_invalid"),
        ("cut.jpg", &whole_jpeg[..4], "raster_header_invalid"),
        // The first IFD is declared beyond the end of the file.
        ("cut.tif", b"II\x2a\0\xff\xff\0\0", "raster_header_invalid"),
        ("cut.webp", b"RIFF\x24\0\0\0WEBP", "raster_header_invalid"),
        (
            "notes.png",
            b"plain text in a file named like an image",
            "unrecognized_format",
        ),
        (
            "animation.gif",
            b"GIF89a\x01\0\x01\0\0\0\0;",
            "unrecognized_format",
        ),
        ("empty.bin", b"", "unrecognized_format"),
    ];
    for (name, bytes, _) in skipped {
        fs::write(dir.path().join(name), bytes).expect("write file");
    }

    let scan = scan_dir(dir.path()).await;

    assert_eq!(scan.loaded(), ["dual-personality.dcm"]);
    assert_eq!(scan.file("dual-personality.dcm")["file_format"], "dicom");
    assert!(scan.file("dual-personality.dcm")["raster"].is_null());
    assert_eq!(scan.report.images_found, 0);
    let mut expected = skipped
        .iter()
        .map(|(name, _, reason)| (name.to_string(), reason.to_string()))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(scan.not_loaded(), expected);
    assert_eq!(scan.catalog["skipped"], skipped.len());
}

#[tokio::test]
async fn formats_narrow_what_a_directory_walk_loads_but_never_a_file_named_as_an_input() {
    use FileFormat::{Dicom, Jpeg, Png, Tiff, Webp};
    let dir = tempdir().expect("temp dir");
    write_dicom(&dir.path().join("a.dcm"));
    fs::write(dir.path().join("b.png"), png(ExtendedColorType::L8, 2, 2)).expect("write PNG");
    fs::write(dir.path().join("c.jpg"), jpeg(ExtendedColorType::L8, 8, 8)).expect("write JPEG");
    fs::write(dir.path().join("d.txt"), b"plain text").expect("write text");
    let folder = dir.path().to_path_buf();
    let png_file = dir.path().join("b.png");
    let dicom_file = dir.path().join("a.dcm");

    // (inputs, selection, loaded, excluded by the selection)
    type Case<'a> = (
        &'a [PathBuf],
        &'a [FileFormat],
        &'a [&'a str],
        &'a [&'a str],
    );
    let cases: [Case<'_>; 6] = [
        (
            std::slice::from_ref(&folder),
            &[Dicom, Png, Jpeg, Tiff, Webp],
            &["a.dcm", "b.png", "c.jpg"],
            &[],
        ),
        // Today's behaviour: only DICOM is loaded.
        (
            std::slice::from_ref(&folder),
            &[Dicom],
            &["a.dcm"],
            &["b.png", "c.jpg"],
        ),
        (
            std::slice::from_ref(&folder),
            &[Dicom, Png],
            &["a.dcm", "b.png"],
            &["c.jpg"],
        ),
        (
            std::slice::from_ref(&folder),
            &[Png, Jpeg],
            &["b.png", "c.jpg"],
            &["a.dcm"],
        ),
        // A file named as an input is loaded whatever the selection says.
        (std::slice::from_ref(&png_file), &[Dicom], &["b.png"], &[]),
        (std::slice::from_ref(&dicom_file), &[Png], &["a.dcm"], &[]),
    ];
    for (inputs, selection, loaded, excluded) in cases {
        let scan = scan(inputs, options(FormatSelection::only(selection), &[])).await;
        let context = format!("{selection:?} over {inputs:?}");

        assert_eq!(scan.loaded(), loaded, "{context}");
        let not_selected = scan
            .not_loaded()
            .into_iter()
            .filter(|(_, reason)| reason == "format_not_selected")
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert_eq!(not_selected, excluded, "{context}");
        // A file of no known format is unrecognized under every selection.
        if inputs == std::slice::from_ref(&folder) {
            assert!(
                scan.not_loaded()
                    .contains(&("d.txt".to_string(), "unrecognized_format".to_string())),
                "{context}"
            );
        }
    }
}

#[tokio::test]
async fn format_and_path_filters_select_files_and_dicom_filters_exclude_rasters() {
    let dir = tempdir().expect("temp dir");
    let scans = dir.path().join("scans");
    let exports = dir.path().join("exports");
    fs::create_dir_all(&scans).expect("scans dir");
    fs::create_dir_all(&exports).expect("exports dir");
    // `write_uncompressed_u16_dicom` writes Modality CT.
    write_dicom(&scans.join("a.dcm"));
    write_dicom(&exports.join("c.dcm"));
    fs::write(exports.join("b.png"), png(ExtendedColorType::L8, 2, 2)).expect("write PNG");
    fs::write(exports.join("d.jpg"), jpeg(ExtendedColorType::L8, 8, 8)).expect("write JPEG");

    let cases: [(&[&str], &[&str]); 8] = [
        (&["format=png"], &["b.png"]),
        (&["format=DICOM"], &["a.dcm", "c.dcm"]),
        // An equality on the format name, not a substring of it.
        (&["format=jpeg"], &["d.jpg"]),
        (&["path=exports"], &["b.png", "c.dcm", "d.jpg"]),
        (&["path=EXPORTS", "format=png"], &["b.png"]),
        (&["path=.dcm"], &["a.dcm", "c.dcm"]),
        // A raster has no Modality, so a DICOM filter leaves it out.
        (&["modality=CT"], &["a.dcm", "c.dcm"]),
        (&["modality=CT", "format=png"], &[]),
    ];
    for (filters, loaded) in cases {
        let scan = scan(
            &[dir.path().to_path_buf()],
            options(FormatSelection::all(), filters),
        )
        .await;
        assert_eq!(scan.loaded(), loaded, "{filters:?}");
        assert_eq!(
            scan.catalog["filtered"],
            4 - loaded.len(),
            "{filters:?}: every other file is filtered, not skipped"
        );
    }

    for rejected in ["format=gif", "format=p", "format=jpg"] {
        assert!(
            rejected.parse::<loader::ScanFilter>().is_err(),
            "{rejected} names no format"
        );
    }
}

/// What every endpoint of the contract answers for a raster that decodes:
/// its frames like any image's, the DICOM-only views with the answer they
/// give a file of the wrong kind, and never a server error.
#[tokio::test]
async fn every_endpoint_answers_for_a_decodable_raster() {
    let dir = tempdir().expect("temp dir");
    fs::write(
        dir.path().join("image.png"),
        png(ExtendedColorType::L16, 3, 2),
    )
    .expect("write PNG");
    fs::write(
        dir.path().join("same-size.png"),
        png(ExtendedColorType::L16, 3, 2),
    )
    .expect("write PNG");
    fs::write(
        dir.path().join("volume.tif"),
        tiff_page::<colortype::Gray32Float>(false, &[0.5; 4], &[]),
    )
    .expect("write TIFF");
    let scan = scan_dir(dir.path()).await;
    let index = scan.index("image.png");
    let get = |endpoint| support::endpoint_request(&scan.server, endpoint, &index);

    // (endpoint, status, media type of a success)
    let expected = [
        (&endpoints::HEALTH, 200, "application/json"),
        (&endpoints::FILES, 200, "application/json"),
        (&endpoints::SERIES, 200, "application/json"),
        (&endpoints::FILE_INFO, 200, "application/json"),
        (&endpoints::FILE_REFERENCES, 200, "application/json"),
        (&endpoints::FILE_SEMANTIC_CONTEXT, 200, "application/json"),
        // The DICOM-only views name a source object of a kind no raster is.
        (&endpoints::FILE_SEGMENTATION_OVERLAY, 400, ""),
        (&endpoints::FILE_DOSE_OVERLAY, 400, ""),
        (&endpoints::FILE_DOSE_OVERLAY_VALUES, 400, ""),
        (&endpoints::FILE_PARAMETRIC_MAP_OVERLAY, 400, ""),
        (&endpoints::FILE_PARAMETRIC_MAP_OVERLAY_VALUES, 400, ""),
        (&endpoints::FILE_GRAPHIC_ANNOTATIONS, 400, ""),
        (&endpoints::FILE_WSI_CONTEXT, 400, ""),
        (&endpoints::FILE_VALUE_MAPPING, 200, "application/json"),
        // The frame, in every form a DICOM frame is served in.
        (&endpoints::FILE_FRAME, 200, "image/png"),
        (&endpoints::FILE_RAW_FRAME, 200, "application/octet-stream"),
        (&endpoints::FILE_RAW_PIXEL, 200, "application/octet-stream"),
        (&endpoints::FILE_THUMBNAIL, 200, "image/jpeg"),
        (&endpoints::FILE_PRESENTATION_LAYER, 200, "image/png"),
        // No metadata tree yet: an empty one, and nothing to select from.
        (&endpoints::FILE_TAGS, 200, "application/json"),
        (&endpoints::FILE_TAG_SELECT, 400, ""),
        (&endpoints::FILE_ANNOTATIONS_GET, 200, "application/json"),
        (&endpoints::FILE_ANNOTATIONS_UPDATE, 200, "application/json"),
        (&endpoints::FILE_REDACTIONS_GET, 200, "application/json"),
        (&endpoints::FILE_REDACTIONS_UPDATE, 200, "application/json"),
        (
            &endpoints::FILE_REDACTIONS_APPLY_TO_SERIES,
            200,
            "application/json",
        ),
        (&endpoints::ANNOTATIONS_EXPORT, 200, "text/csv"),
    ];
    assert_eq!(
        expected.len(),
        endpoints::ALL.len(),
        "every endpoint of the contract has a row"
    );
    for (endpoint, status, media_type) in expected {
        let response = get(endpoint).await;
        assert_eq!(
            response.status_code().as_u16(),
            status,
            "{}: {}",
            endpoint.id,
            response.text()
        );
        if status == 200 {
            let content_type = response.header("content-type");
            assert!(
                content_type
                    .to_str()
                    .is_ok_and(|value| value.starts_with(media_type)),
                "{}: {content_type:?}",
                endpoint.id
            );
        } else {
            let body: Value = response.json();
            assert!(
                body["code"].is_string() && body["error"].is_string(),
                "{} must use the JSON error envelope: {body}",
                endpoint.id
            );
        }
    }

    let info: Value = get(&endpoints::FILE_INFO).await.json();
    assert_eq!(info["object_kind"], "image");
    assert_eq!(info["support_state"], "renderable");
    assert!(info["support_reason"].is_null());
    assert_eq!(
        (info["rows"].as_u64(), info["columns"].as_u64()),
        (Some(2), Some(3))
    );

    // The frame endpoints carry what they carry for DICOM: the cache state,
    // and for raw samples the layout the catalog declares.
    for endpoint in [&endpoints::FILE_FRAME, &endpoints::FILE_RAW_FRAME] {
        assert_eq!(
            get(endpoint).await.header("x-cache"),
            "HIT",
            "{}",
            endpoint.id
        );
    }
    let raw = get(&endpoints::FILE_RAW_FRAME).await;
    for (header, value) in [
        (RAW_FRAME_HEADER_ROWS, "2"),
        (RAW_FRAME_HEADER_COLUMNS, "3"),
        (RAW_FRAME_HEADER_BITS_ALLOCATED, "16"),
        (RAW_FRAME_HEADER_PIXEL_REPRESENTATION, "0"),
        (RAW_FRAME_HEADER_SAMPLES_PER_PIXEL, "1"),
        (RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, "MONOCHROME2"),
        (RAW_FRAME_HEADER_RESCALE_SLOPE, "1"),
        (RAW_FRAME_HEADER_RESCALE_INTERCEPT, "0"),
    ] {
        assert_eq!(raw.header(header), value, "{header}");
    }
    assert_eq!(raw.as_bytes().len(), 2 * 3 * 2);
    // `endpoint_request` asks for the pixel at row 1, column 0.
    let pixel = get(&endpoints::FILE_RAW_PIXEL).await;
    assert_eq!(pixel.as_bytes().as_ref(), [0x40, 0x40]);

    let tags = get(&endpoints::FILE_TAGS).await;
    assert_eq!(tags.json::<Value>(), json!([]));

    // What a raster has for good: no references, no semantic context, and
    // stored values that are their own modality values.
    let references = get(&endpoints::FILE_REFERENCES).await;
    assert_eq!(references.json::<Value>()["references"], json!([]));

    let context = get(&endpoints::FILE_SEMANTIC_CONTEXT).await;
    assert_eq!(context.json::<Value>()["context"]["kind"], "not_applicable");

    for (name, stored_value_type) in [("image.png", "integer"), ("volume.tif", "float32")] {
        let mapping = support::endpoint_request(
            &scan.server,
            &endpoints::FILE_VALUE_MAPPING,
            &scan.index(name),
        )
        .await;
        mapping.assert_status_ok();
        let mapping: Value = mapping.json();
        assert_eq!(mapping["stored_value_type"], stored_value_type, "{name}");
        assert_eq!(
            mapping["modality"],
            json!({
                "rescale_slope": 1.0,
                "rescale_intercept": 0.0,
                "rescale_type": null,
                "lut": null,
            }),
            "{name}"
        );
        assert_eq!(mapping["real_world"], json!([]), "{name}");
        assert!(mapping["voi_lut"].is_null(), "{name}");
    }

    // A raster belongs to no series: the catalog groups none, and one
    // image's redaction boxes are never copied to another of the same size.
    let series: Value = scan.server.get("/api/series").await.json();
    assert_eq!(series["series"], json!([]));
    scan.server
        .put(&format!("/api/file/{index}/redactions"))
        .json(&json!({ "num_roi": 1, "roi_coords": [[0, 0, 1, 1]], "roi_frames": [] }))
        .await
        .assert_status_ok();
    let copied = scan
        .server
        .put(&format!("/api/file/{index}/redactions/series"))
        .await;
    copied.assert_status_ok();
    assert_eq!(copied.json::<Value>(), json!({ "file_indices": [] }));
}

/// A raster the viewer lists and does not decode says why in the catalog,
/// and every endpoint that would produce its pixels answers with that
/// reason instead of trying.
#[tokio::test]
async fn rasters_the_viewer_does_not_decode_say_why_and_their_frames_are_refused() {
    const SIZE: [(u16, &[u16]); 2] = [(256, &[2]), (257, &[2])];
    let gray8: [(u16, &[u16]); 2] = [(258, &[8]), (262, &[1])];
    let rgb8: [(u16, &[u16]); 3] = [(258, &[8, 8, 8]), (262, &[2]), (277, &[3])];
    let tiff = |tags: &[(u16, &[u16])]| {
        let mut page = SIZE.to_vec();
        page.extend_from_slice(tags);
        page.sort_by_key(|(tag, _)| *tag);
        tiff_of(&[&page])
    };
    // A baseline JPEG whose frame header is rewritten to another process.
    let baseline = jpeg(ExtendedColorType::L8, 8, 8);
    let marker = raster_files::jpeg_frame_marker(&baseline);
    let jpeg_as = |process: u8, precision: u8| {
        let mut bytes = baseline.clone();
        bytes[marker] = process;
        bytes[marker + 3] = precision;
        bytes
    };
    // A header is all discovery reads, so a PNG of any declared size is a
    // few bytes.
    let png_declaring = |width: u32, height: u32| {
        raster_files::png_from_chunks(
            (width, height),
            8,
            0,
            false,
            &[raster_files::png_chunk(b"IDAT", &[])],
        )
    };
    let mut wide_canvas = raster_files::vp8x(0, (20_000, 20_000));
    wide_canvas.extend(raster_files::riff_chunk(b"VP8L", &[0x2f, 0x02, 0x40, 0, 0]));

    // (name, bytes, support_reason)
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        // TIFF compressions without a decoder here, JPEG among them.
        (
            "jpeg-compressed.tif",
            tiff(&[gray8[0], gray8[1], (259, &[7])]),
            "raster.unsupported_compression",
        ),
        (
            "ycbcr-jpeg.tif",
            tiff(&[(258, &[8, 8, 8]), (259, &[7]), (262, &[6]), (277, &[3])]),
            "raster.unsupported_compression",
        ),
        (
            "jpeg2000.tif",
            tiff(&[rgb8[0], rgb8[1], rgb8[2], (259, &[33005])]),
            "raster.unsupported_compression",
        ),
        // Colour layouts.
        (
            "palette.tif",
            tiff(&[(258, &[8]), (262, &[3])]),
            "raster.unsupported_color",
        ),
        (
            "cmyk.tif",
            tiff(&[(258, &[8, 8, 8, 8]), (262, &[5]), (277, &[4])]),
            "raster.unsupported_color",
        ),
        (
            "gray-alpha.tif",
            tiff(&[(258, &[8, 8]), (262, &[1]), (277, &[2]), (338, &[2])]),
            "raster.unsupported_color",
        ),
        (
            "planar-rgb.tif",
            tiff(&[rgb8[0], rgb8[1], rgb8[2], (284, &[2])]),
            "raster.unsupported_color",
        ),
        (
            "ycbcr.tif",
            tiff(&[(258, &[8, 8, 8]), (262, &[6]), (277, &[3])]),
            "raster.unsupported_color",
        ),
        // Sample formats and depths.
        (
            "bilevel.tif",
            tiff(&[(258, &[1]), (262, &[1])]),
            "raster.unsupported_sample_format",
        ),
        (
            "rgb32.tif",
            tiff(&[(258, &[32, 32, 32]), (262, &[2]), (277, &[3])]),
            "raster.unsupported_sample_format",
        ),
        (
            "int64.tif",
            tiff(&[(258, &[64]), (262, &[1]), (339, &[2])]),
            "raster.unsupported_sample_format",
        ),
        // The layout is named before the compression.
        (
            "bilevel-fax.tif",
            tiff(&[(258, &[1]), (259, &[4]), (262, &[0])]),
            "raster.unsupported_sample_format",
        ),
        // JPEG processes: lossless, arithmetic-coded, 12-bit.
        (
            "lossless.jpg",
            jpeg_as(0xc3, 8),
            "raster.jpeg_unsupported_process",
        ),
        (
            "arithmetic.jpg",
            jpeg_as(0xc9, 8),
            "raster.jpeg_unsupported_process",
        ),
        (
            "twelve-bit.jpg",
            jpeg_as(0xc1, 12),
            "raster.jpeg_unsupported_process",
        ),
        // More than 268,435,456 pixels in a frame.
        (
            "huge.png",
            png_declaring(20_000, 20_000),
            "raster.too_large",
        ),
        (
            "one-row-too-many.png",
            png_declaring(16_384, 16_385),
            "raster.too_large",
        ),
        (
            "huge.webp",
            raster_files::webp_from_chunks(&wide_canvas),
            "raster.too_large",
        ),
    ];
    let dir = tempdir().expect("temp dir");
    for (name, bytes, _) in &cases {
        fs::write(dir.path().join(name), bytes).expect("write raster");
    }
    // Exactly at the limit a file is not too large.
    fs::write(
        dir.path().join("largest.png"),
        png_declaring(16_384, 16_384),
    )
    .expect("write PNG");

    let scan = scan_dir(dir.path()).await;

    assert_eq!(scan.not_loaded(), []);
    assert_eq!(scan.file("largest.png")["support_state"], "renderable");
    for (name, _, reason) in &cases {
        let file = scan.file(name);
        assert_eq!(file["support_state"], "unsupported", "{name}");
        assert_eq!(file["support_reason"], *reason, "{name}");
        for endpoint in [
            &endpoints::FILE_FRAME,
            &endpoints::FILE_THUMBNAIL,
            &endpoints::FILE_RAW_FRAME,
            &endpoints::FILE_RAW_PIXEL,
            // Sized from the header's rows and columns, so refused too.
            &endpoints::FILE_PRESENTATION_LAYER,
        ] {
            let response =
                support::endpoint_request(&scan.server, endpoint, &scan.index(name)).await;
            assert_eq!(
                response.status_code().as_u16(),
                422,
                "{name} {}",
                endpoint.id
            );
            let body: Value = response.json();
            assert_eq!(
                body["code"], "unsupported_pixel_layout",
                "{name} {}",
                endpoint.id
            );
            assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|error| error.contains(reason)),
                "{name} {}: {body}",
                endpoint.id
            );
        }
    }
}

#[tokio::test]
async fn a_masked_session_gives_a_raster_no_patient() {
    let dir = tempdir().expect("temp dir");
    write_dicom(&dir.path().join("a.dcm"));
    fs::write(dir.path().join("b.png"), png(ExtendedColorType::L8, 2, 2)).expect("write PNG");

    let scan = scan_into(
        &[dir.path().to_path_buf()],
        options(FormatSelection::all(), &[]),
        FileRegistry::masked(Arc::new(Masker::new())),
    )
    .await;

    let image = scan.file("b.png");
    assert_eq!(image["patient_id"], "");
    assert_eq!(image["patient_name"], "");
    assert_eq!(image["study_date"], "");
    // The name it is shown under is the session's synthetic one, as for DICOM.
    assert_eq!(image["label"], image["display_name"]);
    assert_ne!(image["display_name"], "b.png");
    assert_ne!(scan.file("a.dcm")["patient_id"], "");
}

/// The EMBED export of a folder is the same bytes with and without a raster
/// beside the annotated DICOM file.
#[tokio::test]
async fn a_raster_beside_annotated_dicom_leaves_the_export_unchanged() {
    let dir = tempdir().expect("temp dir");
    write_dicom(&dir.path().join("a.dcm"));
    let roi = json!({ "num_roi": 1, "roi_coords": [[0, 0, 1, 1]], "roi_frames": [[0]] });
    let roi = &roi;
    let export = |scan: Scan| async move {
        let index = scan.index("a.dcm");
        scan.server
            .put(&format!("/api/file/{index}/annotations"))
            .json(roi)
            .await
            .assert_status_ok();
        scan.server.get("/api/annotations/export.csv").await.text()
    };

    let alone = export(scan_dir(dir.path()).await).await;
    fs::write(dir.path().join("b.png"), png(ExtendedColorType::L8, 2, 2)).expect("write PNG");
    fs::write(dir.path().join("c.jpg"), jpeg(ExtendedColorType::L8, 8, 8)).expect("write JPEG");
    let scan = scan_dir(dir.path()).await;
    assert_eq!(scan.loaded(), ["a.dcm", "b.png", "c.jpg"]);
    let beside_rasters = export(scan).await;

    assert_eq!(alone.lines().count(), 2, "{alone}");
    assert_eq!(beside_rasters, alone);
}
