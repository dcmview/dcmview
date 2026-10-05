//! Raster image files (PNG, JPEG, TIFF, WebP) in discovery and the catalog
//! (`docs/design/image-formats.md` sections 3 to 5 and 11).
//!
//! Every case writes its files into a temporary directory and runs the
//! production loader over it, then reads the catalog through the router, so
//! what is asserted is what a client of `/api/files` sees. The files are
//! built here with the linked encoders: the expected values are the ones the
//! files were written with, never values a decoder returned.
//!
//! Decoding raster pixels and listing raster metadata as tags are later work.
//! Until then a raster is listed, described, and reported as not decodable,
//! and no endpoint answers a server error for it.

use super::support;
use axum_test::TestServer;
use dcmview::api::contracts::endpoints;
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
const NOT_DECODABLE: &str = "raster.decode_not_available";

// ---------------------------------------------------------------------------
// Files

fn png(color: ExtendedColorType, width: u32, height: u32) -> Vec<u8> {
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
fn png_with(
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

fn jpeg(color: ExtendedColorType, width: u32, height: u32) -> Vec<u8> {
    let bytes = usize::from(color.bits_per_pixel()) / 8 * (width * height) as usize;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&vec![0x40; bytes], width, height, color)
        .expect("encode JPEG");
    out
}

/// `jpeg` with an EXIF `APP1` segment, holding only the orientation tag,
/// inserted straight after the start-of-image marker.
fn with_exif_orientation(mut jpeg: Vec<u8>, orientation: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0".to_vec();
    // Little-endian TIFF header, then IFD0 at offset 8 with one SHORT entry.
    exif.extend_from_slice(b"II\x2a\0\x08\0\0\0");
    exif.extend_from_slice(&1_u16.to_le_bytes());
    exif.extend_from_slice(&0x0112_u16.to_le_bytes());
    exif.extend_from_slice(&3_u16.to_le_bytes());
    exif.extend_from_slice(&1_u32.to_le_bytes());
    exif.extend_from_slice(&orientation.to_le_bytes());
    exif.extend_from_slice(&[0, 0]);
    exif.extend_from_slice(&0_u32.to_le_bytes());
    let mut segment = vec![0xff, 0xe1];
    segment.extend_from_slice(&(exif.len() as u16 + 2).to_be_bytes());
    segment.extend_from_slice(&exif);
    assert_eq!(&jpeg[..2], [0xff, 0xd8], "JPEG starts with SOI");
    jpeg.splice(2..2, segment);
    jpeg
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
fn tiff_page<C>(big: bool, data: &[C::Inner], tags: &[(Tag, u16)]) -> Vec<u8>
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

fn webp_rgba(width: u32, height: u32) -> Vec<u8> {
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

fn write_dicom(path: &Path) {
    support::write_uncompressed_u16_dicom(path, EXPLICIT_LE, 2, 2, vec![1, 2, 3, 4], None, None);
}

// ---------------------------------------------------------------------------
// Harness

/// One completed discovery, served: the catalog as `/api/files` reports it.
struct Scan {
    server: TestServer,
    report: loader::DiscoveryReport,
    /// The loader's own entries, for the sample layout a decoder will read.
    entries: Vec<FileEntry>,
    catalog: Value,
}

impl Scan {
    /// The catalog entry of the file named `name`.
    fn file(&self, name: &str) -> &Value {
        self.catalog["files"]
            .as_array()
            .expect("files array")
            .iter()
            .find(|file| file_name(file["path"].as_str().expect("path")) == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog: {}", self.catalog["files"]))
    }

    fn index(&self, name: &str) -> String {
        self.file(name)["index"].to_string()
    }

    fn entry(&self, name: &str) -> &FileEntry {
        self.entries
            .iter()
            .find(|entry| entry.path.file_name().and_then(|name| name.to_str()) == Some(name))
            .unwrap_or_else(|| panic!("{name} was not selected"))
    }

    /// File names in the catalog, sorted.
    fn loaded(&self) -> Vec<String> {
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
    fn not_loaded(&self) -> Vec<(String, String)> {
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

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn options(formats: FormatSelection, filters: &[&str]) -> DiscoverOptions {
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
async fn scan_into(paths: &[PathBuf], options: DiscoverOptions, registry: FileRegistry) -> Scan {
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

async fn scan(paths: &[PathBuf], options: DiscoverOptions) -> Scan {
    scan_into(paths, options, FileRegistry::new()).await
}

async fn scan_dir(dir: &Path) -> Scan {
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
    /// photometric_interpretation)` of the raw frames a decoder will serve.
    layout: (u32, u32, u32, &'static str),
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
        },
        Expected {
            name: "gray16.png",
            bytes: png(ExtendedColorType::L16, 3, 2),
            format: "png",
            size: (2, 3),
            frames: 1,
            raster: json!({ "bit_depth": 16 }),
            layout: (1, 16, 0, "MONOCHROME2"),
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
        },
        Expected {
            name: "rgba.png",
            bytes: png(ExtendedColorType::Rgba8, 3, 2),
            format: "png",
            size: (2, 3),
            frames: 1,
            raster: json!({ "color_type": "rgba", "has_alpha": true }),
            layout: (4, 8, 0, "RGBA"),
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
        },
        Expected {
            name: "profile.png",
            bytes: png_with_icc_profile(),
            format: "png",
            size: (2, 2),
            frames: 1,
            raster: json!({ "color_type": "rgb", "has_icc": true }),
            layout: (3, 8, 0, "RGB"),
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
        },
        Expected {
            name: "gray.jpg",
            bytes: jpeg(ExtendedColorType::L8, 8, 6),
            format: "jpeg",
            size: (6, 8),
            frames: 1,
            raster: json!({}),
            layout: gray,
        },
        // Orientation is reported and never applied: 8 wide, 6 high as stored.
        Expected {
            name: "rotated.jpg",
            bytes: with_exif_orientation(jpeg(ExtendedColorType::Rgb8, 8, 6), 6),
            format: "jpeg",
            size: (6, 8),
            frames: 1,
            raster: json!({ "color_type": "rgb", "orientation": 6 }),
            layout: (3, 8, 0, "RGB"),
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
            }),
            layout: (1, 16, 0, "MONOCHROME2"),
        },
        Expected {
            name: "float-big.tif",
            bytes: tiff_page::<colortype::Gray32Float>(true, &[0.5; 4], &[]),
            format: "tiff",
            size: (2, 2),
            frames: 1,
            raster: json!({ "bit_depth": 32, "sample_format": "float" }),
            layout: (1, 32, 0, "MONOCHROME2"),
        },
        Expected {
            name: "signed.tif",
            bytes: tiff_page::<colortype::GrayI16>(false, &[-5; 4], &[]),
            format: "tiff",
            size: (2, 2),
            frames: 1,
            raster: json!({ "bit_depth": 16, "sample_format": "int" }),
            layout: (1, 16, 1, "MONOCHROME2"),
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
        },
        Expected {
            name: "lossless.webp",
            bytes: webp_rgba(3, 2),
            format: "webp",
            size: (2, 3),
            frames: 1,
            raster: json!({ "color_type": "rgba", "has_alpha": true }),
            layout: (4, 8, 0, "RGBA"),
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

        // Nothing decodes it yet, and the catalog says so.
        assert_eq!(file["support_state"], "unsupported", "{name}");
        assert_eq!(file["support_reason"], NOT_DECODABLE, "{name}");

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

#[tokio::test]
async fn every_endpoint_answers_for_a_raster_without_a_server_error() {
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

    // Whatever an endpoint answers for a raster, it is never a server error.
    for endpoint in endpoints::ALL {
        let response = get(endpoint).await;
        let status = response.status_code().as_u16();
        assert!(status < 500, "{} answered {status}", endpoint.id);
        if status >= 400 {
            let body: Value = response.json();
            assert!(
                body["code"].is_string() && body["error"].is_string(),
                "{} must use the JSON error envelope: {body}",
                endpoint.id
            );
        }
    }

    // No decoder yet: the frame endpoints say so with the catalog's reason.
    for endpoint in [
        &endpoints::FILE_FRAME,
        &endpoints::FILE_RAW_FRAME,
        &endpoints::FILE_RAW_PIXEL,
    ] {
        let response = get(endpoint).await;
        assert_eq!(response.status_code().as_u16(), 422, "{}", endpoint.id);
        let body: Value = response.json();
        assert_eq!(body["code"], "unsupported_pixel_layout", "{}", endpoint.id);
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|error| error.contains(NOT_DECODABLE)),
            "{}: {body}",
            endpoint.id
        );
    }

    let info: Value = get(&endpoints::FILE_INFO).await.json();
    assert_eq!(info["object_kind"], "image");
    assert_eq!(info["support_state"], "unsupported");
    assert_eq!(info["support_reason"], NOT_DECODABLE);
    assert_eq!(
        (info["rows"].as_u64(), info["columns"].as_u64()),
        (Some(2), Some(3))
    );

    // No metadata tree yet: an empty one, not an error.
    let tags = get(&endpoints::FILE_TAGS).await;
    tags.assert_status_ok();
    assert_eq!(tags.json::<Value>(), json!([]));
    assert_eq!(
        get(&endpoints::FILE_TAG_SELECT)
            .await
            .status_code()
            .as_u16(),
        400
    );

    // What a raster has for good: no references, no semantic context, and
    // stored values that are their own modality values.
    let references = get(&endpoints::FILE_REFERENCES).await;
    references.assert_status_ok();
    assert_eq!(references.json::<Value>()["references"], json!([]));

    let context = get(&endpoints::FILE_SEMANTIC_CONTEXT).await;
    context.assert_status_ok();
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
