//! Thumbnails decoded at a reduced resolution (`pixels::thumbnail_reduction`):
//! JPEG 2000 frames stopped at a lower resolution level and JPEG frames
//! through the scaled inverse DCT.
//!
//! The frames are honest codestreams from `tests/fixtures/reduced-decode`
//! (how they were written is in `tests/fixtures/README.md`), wrapped here in
//! DICOM files and listed by the production loader. Each is a test card with
//! values over its whole range: eight vertical bands stepping from dark to
//! light, discs with hard edges, bright bars along the top eighth and a patch
//! of fine texture in the bottom right corner. A "mirrored" card is the same
//! one flipped left to right.

use super::codestream_files::{self as files, Layout};
use super::support;
use axum::http::StatusCode;
use axum_test::{TestResponse, TestServer};
use dcmview::api::contracts::WindowMode;
use dcmview::masking::Masker;
use dcmview::pixels::{
    self, thumbnail_dimensions, thumbnail_reduction, DecodeLimits, DecodeScheduler, DecodeWork,
    Reduction, JPEG_2000_MAX_REDUCTION_SHIFT, JPEG_MAX_REDUCTION_SHIFT,
};
use dcmview::server::{self, FileRegistry};
use dcmview::types::FileEntry;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;
use image::RgbImage;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

/// A reduced thumbnail against the frame's default display frame shrunk with
/// an area filter, per sample on a scale of 255: the mean difference over the
/// image, and the share of samples that differ by more than [`FAR`].
///
/// A reduced decode low-pass filters with its codec's own filter (a wavelet,
/// a block transform) where the full path averages stored pixels, so the two
/// differ along hard edges and nowhere else. The cards, which are mostly
/// edges, measure up to 3.7 and 0.9% that way, against 0.9 and none for a
/// full decode. A wrong window, sign or inversion, the wrong frame or a
/// mirrored card is several times these bounds.
const MEAN_TOLERANCE: f64 = 5.0;
const FAR: u8 = 48;
const FAR_SHARE_TOLERANCE: f64 = 0.02;
/// The mean of an outermost row or column of a thumbnail against the same
/// one of the shrunk display frame: a decode that leaves an edge of the
/// image unfilled is far outside it.
const EDGE_TOLERANCE: f64 = 12.0;

fn codestream(name: &str) -> Vec<u8> {
    std::fs::read(files::fixture("reduced-decode").join(name))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// One file to take thumbnails of.
#[derive(Clone)]
struct Case {
    name: &'static str,
    transfer_syntax: &'static str,
    layout: Layout,
    /// The codestream of each frame.
    frames: &'static [&'static str],
    /// Pixel Representation 1.
    signed: bool,
    /// Window Center and Width; `None` leaves the frame without a window.
    window: Option<(i32, i32)>,
    /// The buckets at which its thumbnail is decoded at a reduced
    /// resolution; at the others it is decoded in full.
    reduced_at: &'static [u32],
}

const fn gray(rows: u16, columns: u16, bits_allocated: u16, bits_stored: u16) -> Layout {
    Layout::gray(rows, columns, bits_allocated, bits_stored)
}

fn monochrome1(mut layout: Layout) -> Layout {
    layout.photometric = "MONOCHROME1";
    layout
}

const WINDOW_8: Option<(i32, i32)> = Some((128, 256));
const WINDOW_12: Option<(i32, i32)> = Some((2048, 4096));

fn case(
    name: &'static str,
    transfer_syntax: &'static str,
    layout: Layout,
    frames: &'static [&'static str],
    window: Option<(i32, i32)>,
    reduced_at: &'static [u32],
) -> Case {
    Case {
        name,
        transfer_syntax,
        layout,
        frames,
        signed: false,
        window,
        reduced_at,
    }
}

/// An 8-bit gray JPEG 2000 frame with a stored window: 512 x 384, five
/// decomposition levels.
fn plain_j2k() -> Case {
    case(
        "JPEG 2000, 8-bit gray",
        files::JPEG_2000,
        gray(384, 512, 8, 8),
        &["gray8-512x384.j2k"],
        WINDOW_8,
        &[128],
    )
}

/// An 8-bit gray baseline JPEG frame with a stored window: 1040 x 780.
fn plain_jpeg() -> Case {
    case(
        "JPEG, baseline gray",
        files::JPEG_BASELINE,
        gray(780, 1040, 8, 8),
        &["gray8-baseline-1040x780.jpg"],
        WINDOW_8,
        &[128, 256],
    )
}

/// Every layout the reduced paths decode. A frame is reduced when it is at
/// least four times its thumbnail along both axes: 512 x 384 at 128 pixels,
/// 1040 x 780 at 128 and 256, 2080 x 1560 at 128, 256 and 512.
fn reduced_cases() -> Vec<Case> {
    let j2k = files::JPEG_2000;
    let jpeg = files::JPEG_BASELINE;
    vec![
        plain_j2k(),
        case(
            "JPEG 2000 in a JP2 file",
            j2k,
            gray(384, 512, 8, 8),
            &["gray8-512x384.jp2"],
            WINDOW_8,
            &[128],
        ),
        case(
            "JPEG 2000, 12-bit gray, odd tiles, precincts, odd size",
            j2k,
            gray(389, 517, 16, 12),
            &["gray12-tiles-517x389.j2k"],
            WINDOW_12,
            &[128],
        ),
        case(
            "JPEG 2000, 12-bit MONOCHROME1",
            j2k,
            monochrome1(gray(389, 517, 16, 12)),
            &["gray12-tiles-517x389.j2k"],
            WINDOW_12,
            &[128],
        ),
        Case {
            signed: true,
            ..case(
                "JPEG 2000, 16-bit signed gray",
                j2k,
                gray(384, 512, 16, 16),
                &["gray16-signed-512x384.j2k"],
                Some((0, 32768)),
                &[128],
            )
        },
        case(
            "JPEG 2000, two frames",
            j2k,
            gray(384, 512, 8, 8),
            &["gray8-512x384.j2k", "gray8-512x384-mirrored.j2k"],
            WINDOW_8,
            &[128],
        ),
        case(
            "JPEG 2000, reversible colour transform",
            j2k,
            Layout::color8(384, 512, "YBR_RCT"),
            &["rgb8-rct-512x384.j2k"],
            None,
            &[128],
        ),
        case(
            "JPEG 2000, RGB without a colour transform",
            j2k,
            Layout::color8(384, 512, "RGB"),
            &["rgb8-plain-512x384.j2k"],
            None,
            &[128],
        ),
        case(
            "JPEG 2000, 12-bit colour",
            j2k,
            Layout {
                bits_allocated: 16,
                bits_stored: 12,
                ..Layout::color8(384, 512, "YBR_RCT")
            },
            &["rgb12-rct-512x384.j2k"],
            None,
            &[128],
        ),
        case(
            "JPEG 2000, tiles and a tile-part for each resolution",
            j2k,
            gray(780, 1040, 16, 12),
            &["gray12-tile-parts-1040x780.j2k"],
            WINDOW_12,
            &[128, 256],
        ),
        // Asked for two halvings at 128 pixels; it has one.
        case(
            "JPEG 2000 with one decomposition level",
            j2k,
            gray(780, 1040, 8, 8),
            &["gray8-one-level-1040x780.j2k"],
            WINDOW_8,
            &[128, 256],
        ),
        case(
            "JPEG 2000, three halvings",
            j2k,
            gray(1560, 2080, 16, 12),
            &["gray12-2080x1560.j2k"],
            WINDOW_12,
            &[128, 256, 512],
        ),
        plain_jpeg(),
        case(
            "JPEG, baseline gray, two frames",
            jpeg,
            gray(780, 1040, 8, 8),
            &[
                "gray8-baseline-1040x780.jpg",
                "gray8-baseline-1040x780-mirrored.jpg",
            ],
            WINDOW_8,
            &[128, 256],
        ),
        case(
            "JPEG, baseline MONOCHROME1",
            jpeg,
            monochrome1(gray(780, 1040, 8, 8)),
            &["gray8-baseline-1040x780.jpg"],
            WINDOW_8,
            &[128, 256],
        ),
        case(
            "JPEG, progressive gray, odd size",
            jpeg,
            gray(779, 1041, 8, 8),
            &["gray8-progressive-1041x779.jpg"],
            WINDOW_8,
            &[128, 256],
        ),
        case(
            "JPEG, restart intervals, an eighth of the size",
            jpeg,
            gray(1560, 2080, 8, 8),
            &["gray8-restart-2080x1560.jpg"],
            WINDOW_8,
            &[128, 256, 512],
        ),
        case(
            "JPEG, 4:2:0 colour, odd size",
            jpeg,
            Layout::color8(779, 1041, "YBR_FULL_422"),
            &["rgb8-420-1041x779.jpg"],
            None,
            &[128, 256],
        ),
        case(
            "JPEG, progressive 4:4:4 colour",
            jpeg,
            Layout::color8(780, 1040, "YBR_FULL"),
            &["rgb8-444-progressive-1040x780.jpg"],
            None,
            &[128, 256],
        ),
    ]
}

/// Writes `case` as `path`, with `frames` in place of its own when given
/// and `more` put into its header.
fn write_case(
    path: &Path,
    case: &Case,
    frames: Option<&[Vec<u8>]>,
    more: &[DataElement<InMemDicomObject>],
) {
    let own: Vec<Vec<u8>> = case.frames.iter().map(|name| codestream(name)).collect();
    let mut header = more.to_vec();
    if case.signed {
        header.push(DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(1_u16),
        ));
    }
    if let Some((center, width)) = case.window {
        header.push(DataElement::new(
            tags::WINDOW_CENTER,
            VR::DS,
            PrimitiveValue::from(center.to_string()),
        ));
        header.push(DataElement::new(
            tags::WINDOW_WIDTH,
            VR::DS,
            PrimitiveValue::from(width.to_string()),
        ));
    }
    files::write_with(
        path,
        case.transfer_syntax,
        case.layout,
        frames.unwrap_or(&own),
        &header,
    );
}

/// The catalog entry of `case`, written into `dir`.
async fn entry_of(dir: &Path, case: &Case) -> FileEntry {
    let path = dir.join("case.dcm");
    write_case(&path, case, None, &[]);
    files::list(&[path]).await.remove(0)
}

async fn serve_case(dir: &Path, case: &Case) -> (TestServer, FileEntry) {
    let entry = entry_of(dir, case).await;
    (files::serve(vec![entry.clone()]), entry)
}

fn header(response: &TestResponse, name: &str) -> String {
    response
        .header(name)
        .to_str()
        .expect("header text")
        .to_string()
}

/// A successful thumbnail: its pixels, `X-Cache` and `X-Thumbnail-Source`.
async fn thumbnail(server: &TestServer, path: &str) -> (RgbImage, String, String) {
    let response = server.get(path).await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "{path}: {}",
        response.text()
    );
    assert_eq!(header(&response, "content-type"), "image/jpeg", "{path}");
    assert_eq!(header(&response, "cache-control"), "no-store", "{path}");
    let image = image::load_from_memory_with_format(response.as_bytes(), image::ImageFormat::Jpeg)
        .unwrap_or_else(|error| panic!("{path} is not a JPEG: {error}"))
        .into_rgb8();
    (
        image,
        header(&response, "x-cache"),
        header(&response, "x-thumbnail-source"),
    )
}

/// The display frame of `frame` in `mode`, shrunk to `size` with the area
/// filter the thumbnail path uses.
async fn shrunk_display(server: &TestServer, frame: u32, mode: &str, size: (u32, u32)) -> RgbImage {
    let display = server
        .get(&format!("/api/file/0/frame/{frame}?mode={mode}"))
        .await;
    display.assert_status_ok();
    let display = image::load_from_memory(display.as_bytes())
        .expect("decode display PNG")
        .into_rgb8();
    image::imageops::thumbnail(&display, size.0, size.1)
}

/// The mean difference per sample of two images of one size, and the share
/// of samples further apart than [`FAR`].
fn difference(left: &RgbImage, right: &RgbImage) -> (f64, f64) {
    assert_eq!(left.dimensions(), right.dimensions());
    let (mut total, mut far) = (0_u64, 0_u64);
    for (a, b) in left.as_raw().iter().zip(right.as_raw()) {
        let apart = a.abs_diff(*b);
        total += u64::from(apart);
        far += u64::from(apart > FAR);
    }
    let samples = left.as_raw().len() as f64;
    (total as f64 / samples, far as f64 / samples)
}

/// The mean sample of each outermost row and column: top, bottom, left,
/// right.
fn edges(image: &RgbImage) -> [f64; 4] {
    let (width, height) = image.dimensions();
    let mean = |pixels: &mut dyn Iterator<Item = (u32, u32)>| {
        let (mut total, mut count) = (0_u64, 0_u64);
        for (column, row) in pixels {
            for sample in image.get_pixel(column, row).0 {
                total += u64::from(sample);
                count += 1;
            }
        }
        total as f64 / count as f64
    };
    [
        mean(&mut (0..width).map(|column| (column, 0))),
        mean(&mut (0..width).map(|column| (column, height - 1))),
        mean(&mut (0..height).map(|row| (0, row))),
        mean(&mut (0..height).map(|row| (width - 1, row))),
    ]
}

/// Holds `thumbnail` to the tolerances against `expected`.
fn assert_close(context: &str, thumbnail: &RgbImage, expected: &RgbImage) {
    let (mean, far) = difference(thumbnail, expected);
    if std::env::var_os("REDUCED_THUMBNAIL_REPORT").is_some() {
        eprintln!("{context}: mean {mean:.2}, far {:.2}%", far * 100.0);
    }
    assert!(
        mean <= MEAN_TOLERANCE && far <= FAR_SHARE_TOLERANCE,
        "{context}: the thumbnail differs from the shrunk display frame by {mean:.2} a sample, \
         and {:.2}% of its samples by more than {FAR}",
        far * 100.0
    );
    for (edge, (ours, theirs)) in ["top", "bottom", "left", "right"]
        .into_iter()
        .zip(edges(thumbnail).into_iter().zip(edges(expected)))
    {
        assert!(
            (ours - theirs).abs() <= EDGE_TOLERANCE,
            "{context}: the {edge} edge of the thumbnail averages {ours:.0}, \
             of the shrunk display frame {theirs:.0}"
        );
    }
}

fn source_at(case: &Case, bucket: u32) -> &'static str {
    if case.reduced_at.contains(&bucket) {
        "reduced_decode"
    } else {
        "full_decode"
    }
}

/// Every layout and every frame, at each bucket where the frame is reduced
/// and at the first where it is not: the thumbnail has the size the catalog
/// entry gives it, shows the default display frame shrunk, and says whether
/// it was decoded at a reduced resolution. A second request is served from
/// the thumbnail cache, byte for byte. The layouts are checked side by
/// side; each has a server of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_reduced_thumbnail_is_the_default_display_frame_shrunk() {
    let checks = reduced_cases().into_iter().map(|case| {
        tokio::spawn(async move {
            let dir = tempdir().expect("temp dir");
            let (server, entry) = serve_case(dir.path(), &case).await;
            assert_eq!(
                entry.frame_count as usize,
                case.frames.len(),
                "{}",
                case.name
            );
            let full = [128, 256, 512, 1024]
                .into_iter()
                .find(|bucket| !case.reduced_at.contains(bucket))
                .expect("a bucket the frame is not reduced at");
            for bucket in case.reduced_at.iter().copied().chain([full]) {
                let size = thumbnail_dimensions(entry.rows, entry.columns, None, bucket);
                for frame in 0..entry.frame_count {
                    let context = format!("{}, frame {frame}, {bucket} pixels", case.name);
                    let path = format!("/api/file/0/frame/{frame}/thumbnail?size={bucket}");
                    let (image, cache, source) = thumbnail(&server, &path).await;
                    assert_eq!(image.dimensions(), size, "{context}");
                    assert_eq!(
                        (cache.as_str(), source.as_str()),
                        ("MISS", source_at(&case, bucket)),
                        "{context}"
                    );
                    let expected = shrunk_display(&server, frame, "default", size).await;
                    assert_close(&context, &image, &expected);

                    let (again, cache, source) = thumbnail(&server, &path).await;
                    assert_eq!(
                        (cache.as_str(), source.as_str()),
                        ("HIT", "thumbnail_cache"),
                        "{context}"
                    );
                    assert_eq!(again, image, "{context}");
                }
            }
        })
    });
    for check in futures::future::join_all(checks).await {
        if let Err(failed) = check {
            std::panic::resume_unwind(failed.into_panic());
        }
    }
}

/// The frames of a file are different pictures, and each thumbnail is its
/// own frame's: a mirrored card is far from the card.
#[tokio::test]
async fn a_reduced_thumbnail_is_of_the_frame_that_was_asked_for() {
    for case in reduced_cases()
        .into_iter()
        .filter(|case| case.frames.len() > 1)
    {
        let dir = tempdir().expect("temp dir");
        let (server, _) = serve_case(dir.path(), &case).await;
        let (first, _, source) = thumbnail(&server, "/api/file/0/frame/0/thumbnail?size=128").await;
        assert_eq!(source, "reduced_decode", "{}", case.name);
        let (second, _, source) =
            thumbnail(&server, "/api/file/0/frame/1/thumbnail?size=128").await;
        assert_eq!(source, "reduced_decode", "{}", case.name);
        let (mean, _) = difference(&first, &second);
        if std::env::var_os("REDUCED_THUMBNAIL_REPORT").is_some() {
            eprintln!("{}: frames apart {mean:.2}", case.name);
        }
        assert!(
            mean > 4.0 * MEAN_TOLERANCE,
            "{}: the two frames' thumbnails differ by {mean:.2}",
            case.name
        );
        let mirrored = image::imageops::flip_horizontal(&second);
        let (mean, _) = difference(&first, &mirrored);
        if std::env::var_os("REDUCED_THUMBNAIL_REPORT").is_some() {
            eprintln!("{}: mirrored back {mean:.2}", case.name);
        }
        assert!(
            mean <= MEAN_TOLERANCE,
            "{}: frame 1 is frame 0 mirrored, but differs from it by {mean:.2} mirrored back",
            case.name
        );
    }
}

/// A thumbnail whose presentation depends on the frame's own samples is
/// decoded in full, so it has the window the display frame has: a frame
/// without a stored window (windowed from its histogram) and every
/// `full_dynamic` thumbnail (windowed over its extremes). So is a frame the
/// reduced decode does not apply to, and each is still the display frame
/// shrunk, within what JPEG loses.
#[tokio::test]
async fn a_thumbnail_that_cannot_be_reduced_is_decoded_in_full() {
    let without_window = |case: Case| Case {
        window: None,
        ..case
    };
    let lossless_frame = |name, transfer_syntax| {
        case(
            name,
            transfer_syntax,
            gray(384, 512, 8, 8),
            &["gray8-lossless-512x384.jpg"],
            WINDOW_8,
            &[],
        )
    };
    // (case, query after the bucket, display mode, why)
    let cases = [
        (
            plain_j2k(),
            "&window_mode=full_dynamic",
            "full_dynamic",
            "full dynamic",
        ),
        (
            plain_jpeg(),
            "&window_mode=full_dynamic",
            "full_dynamic",
            "full dynamic",
        ),
        (without_window(plain_j2k()), "", "default", "no window"),
        (without_window(plain_jpeg()), "", "default", "no window"),
        (
            case(
                "JPEG 2000 without decomposition levels",
                files::JPEG_2000,
                gray(384, 512, 8, 8),
                &["gray8-no-levels-512x384.j2k"],
                WINDOW_8,
                &[],
            ),
            "",
            "default",
            "no levels",
        ),
        (
            case(
                "JPEG 2000, image off the origin of its grid",
                files::JPEG_2000,
                gray(389, 517, 8, 8),
                &["gray8-offset-517x389.j2k"],
                WINDOW_8,
                &[],
            ),
            "",
            "default",
            "off the origin",
        ),
        (
            case(
                "JPEG 2000, image off the origin along its columns only",
                files::JPEG_2000,
                gray(389, 517, 8, 8),
                &["gray8-offset-x-517x389.j2k"],
                WINDOW_8,
                &[],
            ),
            "",
            "default",
            "off the origin in X",
        ),
        (
            case(
                "JPEG 2000, image off the origin along its rows only",
                files::JPEG_2000,
                gray(389, 517, 8, 8),
                &["gray8-offset-y-517x389.j2k"],
                WINDOW_8,
                &[],
            ),
            "",
            "default",
            "off the origin in Y",
        ),
        // The lossless process has no blocks to scale: under the lossless
        // syntax no reduction is asked for, and under the baseline syntax
        // the one asked for cannot be applied.
        (
            lossless_frame("JPEG Lossless", files::JPEG_LOSSLESS_SV1),
            "",
            "default",
            "not asked",
        ),
        (
            lossless_frame(
                "a lossless frame in a JPEG Baseline file",
                files::JPEG_BASELINE,
            ),
            "",
            "default",
            "lossless process",
        ),
    ];
    for (case, query, mode, why) in cases {
        let dir = tempdir().expect("temp dir");
        let (server, entry) = serve_case(dir.path(), &case).await;
        let context = format!("{} ({why})", case.name);
        let size = thumbnail_dimensions(entry.rows, entry.columns, None, 128);
        let (image, cache, source) = thumbnail(
            &server,
            &format!("/api/file/0/frame/0/thumbnail?size=128{query}"),
        )
        .await;
        assert_eq!(
            (cache.as_str(), source.as_str()),
            ("MISS", "full_decode"),
            "{context}"
        );
        assert_eq!(image.dimensions(), size, "{context}");
        let expected = shrunk_display(&server, 0, mode, size).await;
        let (mean, far) = difference(&image, &expected);
        assert!(
            mean <= 1.5 && far == 0.0,
            "{context}: a full decode differs from the shrunk display frame by {mean:.2}"
        );
    }
}

fn entry_like(case: &Case, transfer_syntax: &str) -> FileEntry {
    let mut entry = support::file_entry("unread.dcm".into(), transfer_syntax, 1);
    entry.rows = u32::from(case.layout.rows);
    entry.columns = u32::from(case.layout.columns);
    entry.bits_allocated = u32::from(case.layout.bits_allocated);
    entry.samples_per_pixel = u32::from(case.layout.samples);
    entry.photometric_interpretation = case.layout.photometric.to_string();
    entry.default_window = case
        .window
        .map(|(center, width)| dcmview::types::WindowPreset {
            center: f64::from(center),
            width: f64::from(width),
        });
    entry
}

/// The reduction a thumbnail asks for is a function of the catalog entry and
/// the request: the largest halving that leaves the frame at least twice its
/// thumbnail along both axes, for the two codecs that can reduce and the
/// presentations that do not depend on the frame's samples.
#[test]
fn a_thumbnail_asks_for_the_documented_reduction() {
    let sized = |rows: u32, columns: u32, transfer_syntax: &str| {
        let mut entry = entry_like(&plain_j2k(), transfer_syntax);
        (entry.rows, entry.columns) = (rows, columns);
        entry
    };
    let j2k = files::JPEG_2000;
    let jpeg = files::JPEG_BASELINE;
    let shift = |entry: &FileEntry, bucket: u32| {
        thumbnail_reduction(entry, bucket, WindowMode::Default).map(|reduction| reduction.shift)
    };

    // (rows, columns, bucket, JPEG 2000, JPEG)
    let sizes = [
        // The thumbnail of a square frame is the bucket: four times it is
        // one halving, one pixel less is none.
        (512_u32, 512_u32, 128_u32, Some(1_u8), Some(1_u8)),
        (511, 512, 128, None, None),
        (512, 511, 128, None, None),
        (1023, 1023, 128, Some(1), Some(1)),
        (1024, 1024, 128, Some(2), Some(2)),
        (1025, 1025, 128, Some(2), Some(2)),
        (2047, 2048, 128, Some(2), Some(2)),
        (2048, 2048, 128, Some(3), Some(3)),
        // JPEG stops at an eighth.
        (4096, 4096, 128, Some(4), Some(3)),
        (4097, 4097, 128, Some(4), Some(3)),
        (65535, 65535, 128, Some(7), Some(3)),
        (65535, 65535, 1024, Some(4), Some(3)),
        // Larger buckets need larger frames.
        (2048, 2048, 256, Some(2), Some(2)),
        (2048, 2048, 512, Some(1), Some(1)),
        (2048, 2048, 1024, None, None),
        (4096, 3328, 256, Some(3), Some(3)),
        // The shorter axis decides too: 96 rows of thumbnail need 192.
        (768, 1024, 128, Some(2), Some(2)),
        (765, 1024, 128, Some(1), Some(1)),
        // A frame no larger than its thumbnail, a line, and no frame.
        (100, 100, 128, None, None),
        (1, 65535, 128, None, None),
        (0, 0, 128, None, None),
        (u32::MAX, u32::MAX, 128, Some(23), Some(3)),
    ];
    for (rows, columns, bucket, in_j2k, in_jpeg) in sizes {
        assert_eq!(
            shift(&sized(rows, columns, j2k), bucket),
            in_j2k,
            "JPEG 2000, {rows} x {columns} at {bucket}"
        );
        assert_eq!(
            shift(&sized(rows, columns, jpeg), bucket),
            in_jpeg,
            "JPEG, {rows} x {columns} at {bucket}"
        );
    }
    assert_eq!(JPEG_MAX_REDUCTION_SHIFT, 3);
    assert_eq!(JPEG_2000_MAX_REDUCTION_SHIFT, 31);

    // Pixels twice as high as wide: 2048 rows of 2048 are a thumbnail of
    // 64 x 128, so the rows decide.
    let mut tall = sized(2048, 2048, j2k);
    tall.series_metadata.native_pixel.normalized_pixel_aspect = Some([2.0, 1.0]);
    assert_eq!(shift(&tall, 128), Some(3));
    assert_eq!(shift(&tall, 512), Some(1));
    tall.series_metadata.native_pixel.normalized_pixel_aspect = Some([1.0, 2.0]);
    assert_eq!(shift(&tall, 128), Some(3));
    // The physical shape decides, not the stored one. 2048 rows of 4096
    // square pixels are a thumbnail of 128 x 64 and can be halved four
    // times; with pixels twice as high as wide the same frame is square on
    // screen, a thumbnail of 128 x 128, and its rows allow only three.
    let mut wide = sized(2048, 4096, j2k);
    assert_eq!(shift(&wide, 128), Some(4));
    wide.series_metadata.native_pixel.normalized_pixel_aspect = Some([2.0, 1.0]);
    assert_eq!(shift(&wide, 128), Some(3));

    // Only the codecs with a reduced decode, whatever the size.
    for transfer_syntax in [
        "1.2.840.10008.1.2.1",
        files::JPEG_LOSSLESS,
        files::JPEG_LOSSLESS_SV1,
        files::JPEG_LS,
        files::JPEG_XL,
        "1.2.840.10008.1.2.5",
        files::DEFLATED_FRAME,
        "1.2.840.10008.1.2.4.51",
        "",
    ] {
        assert_eq!(
            shift(&sized(4096, 4096, transfer_syntax), 128),
            None,
            "{transfer_syntax}"
        );
    }

    // Only presentations that do not depend on the frame's samples.
    let windowed = sized(4096, 4096, j2k);
    let mut automatic = windowed.clone();
    automatic.default_window = None;
    let mut colour = automatic.clone();
    colour.samples_per_pixel = 3;
    colour.photometric_interpretation = "YBR_RCT".to_string();
    let mut inverted = windowed.clone();
    inverted.photometric_interpretation = "MONOCHROME1".to_string();
    let mut palette = windowed.clone();
    palette.photometric_interpretation = "PALETTE COLOR".to_string();
    let asks = |entry: &FileEntry, mode| thumbnail_reduction(entry, 128, mode);
    let four = Some(Reduction { shift: 4 });
    assert_eq!(asks(&windowed, WindowMode::Default), four);
    assert_eq!(asks(&inverted, WindowMode::Default), four);
    assert_eq!(asks(&windowed, WindowMode::FullDynamic), None);
    assert_eq!(asks(&automatic, WindowMode::Default), None);
    assert_eq!(asks(&automatic, WindowMode::FullDynamic), None);
    assert_eq!(asks(&colour, WindowMode::Default), four);
    assert_eq!(asks(&colour, WindowMode::FullDynamic), four);
    assert_eq!(asks(&palette, WindowMode::Default), None);
    // A VOI LUT presents each sample by itself too, when the default
    // presentation selects it: without a stored window, in default mode.
    let mut voi_lut = automatic.clone();
    voi_lut.series_metadata.native_pixel.voi_lut = Some(dcmview::types::DicomLut {
        first_mapped_value: 0,
        bits_per_entry: 8,
        entries: (0..=255).collect(),
    });
    assert_eq!(asks(&voi_lut, WindowMode::Default), four);
    assert_eq!(asks(&voi_lut, WindowMode::FullDynamic), None);

    // What a reduced decode must return to be the entry's frame: each
    // extent divided and rounded up.
    let odd = sized(779, 1041, j2k);
    let two = Reduction { shift: 2 };
    assert!(two.accepts(&odd, 261, 195));
    for (columns, rows) in [(260, 195), (261, 194), (262, 195), (261, 196), (1041, 779)] {
        assert!(!two.accepts(&odd, columns, rows), "{columns} x {rows}");
    }
    assert!(!Reduction { shift: 0 }.accepts(&odd, 1041, 779));
    assert!(Reduction { shift: 31 }.accepts(&odd, 1, 1));
    assert!(!Reduction { shift: 31 }.accepts(&sized(0, 0, j2k), 0, 0));
}

/// The thumbnail of a reduced frame has the size of the stored frame's, not
/// the size its reduced image would have: 1024 x 707 is 128 x 88, and its
/// quarter, 256 x 177, would be 128 x 89. A frame with non-square pixels
/// keeps its physical shape, and the picture is the display frame's.
#[tokio::test]
async fn a_reduced_thumbnail_keeps_the_geometry_of_the_stored_frame() {
    let (columns, rows) = (1024_u32, 707_u32);
    let pixels: Vec<u8> = (0..rows)
        .flat_map(|row| (0..columns).map(move |column| ((row / 4 + column / 8) % 256) as u8))
        .collect();
    let mut frame = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut frame, 90)
        .encode(&pixels, columns, rows, image::ExtendedColorType::L8)
        .expect("encode JPEG");
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("odd.dcm");
    let odd = case(
        "JPEG, 1024 x 707",
        files::JPEG_BASELINE,
        gray(rows as u16, columns as u16, 8, 8),
        &[],
        WINDOW_8,
        &[128],
    );
    write_case(&path, &odd, Some(&[frame]), &[]);
    let server = files::serve(files::list(&[path]).await);
    assert_eq!(thumbnail_dimensions(rows, columns, None, 128), (128, 88));
    assert_eq!(
        thumbnail_dimensions(rows.div_ceil(4), columns.div_ceil(4), None, 128),
        (128, 89)
    );
    let (image, _, source) = thumbnail(&server, "/api/file/0/frame/0/thumbnail?size=128").await;
    assert_eq!(source, "reduced_decode");
    assert_eq!(image.dimensions(), (128, 88));
    let expected = shrunk_display(&server, 0, "default", (128, 88)).await;
    assert_close(odd.name, &image, &expected);

    let dir = tempdir().expect("temp dir");
    let mut entry = entry_of(dir.path(), &plain_jpeg()).await;
    // Pixels twice as high as wide: 1040 x 780 stored is 520 x 780 on
    // screen, a thumbnail of 85 x 128.
    entry.series_metadata.native_pixel.normalized_pixel_aspect = Some([2.0, 1.0]);
    let server = files::serve(vec![entry.clone()]);
    let size = thumbnail_dimensions(entry.rows, entry.columns, Some(2.0), 128);
    assert_eq!(size, (85, 128));
    let (image, _, source) = thumbnail(&server, "/api/file/0/frame/0/thumbnail?size=128").await;
    assert_eq!(source, "reduced_decode");
    assert_eq!(image.dimensions(), size);
    let expected = shrunk_display(&server, 0, "default", size).await;
    assert_close("non-square pixels", &image, &expected);
}

fn boxes(coordinates: Value) -> Value {
    let count = coordinates.as_array().expect("boxes").len();
    json!({ "num_roi": count, "roi_coords": coordinates, "roi_frames": [] })
}

/// A frame with redaction boxes is decoded in full, so its boxes are painted
/// on stored pixels: the thumbnail is blanked exactly where the full path
/// blanks it, and a frame whose boxes are removed is reduced again.
#[tokio::test]
async fn a_redacted_frame_is_decoded_in_full_and_blanked() {
    for case in [plain_j2k(), plain_jpeg()] {
        let dir = tempdir().expect("temp dir");
        let (server, entry) = serve_case(dir.path(), &case).await;
        let path = "/api/file/0/frame/0/thumbnail?size=128";
        let (width, height) = thumbnail_dimensions(entry.rows, entry.columns, None, 128);
        let brightest = |image: &RgbImage, rows: std::ops::Range<u32>| {
            rows.flat_map(|row| (0..width).map(move |column| image.get_pixel(column, row).0[0]))
                .max()
                .expect("rows")
        };
        // The bright bars fill the top eighth of the card.
        let bars = 0..height / 8 - 1;
        let below = height / 8 + 2..height;

        let (before, _, source) = thumbnail(&server, path).await;
        assert_eq!(source, "reduced_decode", "{}", case.name);
        assert!(brightest(&before, bars.clone()) > 180, "{}", case.name);

        let put = |body: Value| {
            let server = &server;
            async move {
                server
                    .put("/api/file/0/redactions")
                    .json(&body)
                    .await
                    .assert_status_ok();
            }
        };
        put(boxes(json!([[0, 0, entry.rows / 8, entry.columns]]))).await;
        let (redacted, cache, source) = thumbnail(&server, path).await;
        assert_eq!(
            (cache.as_str(), source.as_str()),
            ("MISS", "full_decode"),
            "{}: a frame with boxes is decoded in full",
            case.name
        );
        assert!(
            brightest(&redacted, bars.clone()) <= 8,
            "{}: the redacted bars show through: {}",
            case.name,
            brightest(&redacted, bars.clone())
        );
        // What the box does not cover is the same picture as before.
        let rest = |image: &RgbImage| {
            image::imageops::crop_imm(image, 0, below.start, width, below.end - below.start)
                .to_image()
        };
        let (mean, _) = difference(&rest(&redacted), &rest(&before));
        assert!(mean <= MEAN_TOLERANCE, "{}: {mean:.2}", case.name);

        // A box that covers nothing of this frame's pixels still makes it
        // a redacted frame.
        put(boxes(json!([[0, 0, 1, 1]]))).await;
        let (_, _, source) = thumbnail(&server, path).await;
        assert_eq!(source, "full_decode", "{}", case.name);

        put(boxes(json!([]))).await;
        let (shown, cache, source) = thumbnail(&server, path).await;
        assert_eq!(
            (cache.as_str(), source.as_str()),
            ("HIT", "thumbnail_cache"),
            "{}: without boxes the first thumbnail is current again",
            case.name
        );
        assert_eq!(shown, before, "{}", case.name);
    }
}

/// A masked session withholds a label image's thumbnail whatever would have
/// decoded it, and serves the others reduced.
#[tokio::test]
async fn a_masked_session_withholds_reduced_thumbnails_too() {
    let dir = tempdir().expect("temp dir");
    let (shown, label) = (dir.path().join("shown.dcm"), dir.path().join("label.dcm"));
    write_case(&shown, &plain_j2k(), None, &[]);
    write_case(
        &label,
        &plain_j2k(),
        None,
        &[DataElement::new(
            tags::IMAGE_TYPE,
            VR::CS,
            PrimitiveValue::from("ORIGINAL\\PRIMARY\\LABEL\\NONE"),
        )],
    );
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    for entry in files::list(&[shown, label]).await {
        registry.insert(entry);
    }
    registry.mark_scan_complete();
    let server = TestServer::new(server::router(support::app_state_with_registry(registry)));

    let (_, _, source) = thumbnail(&server, "/api/file/0/frame/0/thumbnail?size=128").await;
    assert_eq!(source, "reduced_decode");
    let display = server.get("/api/file/1/frame/0").await;
    let withheld = server.get("/api/file/1/frame/0/thumbnail?size=128").await;
    assert_eq!(withheld.status_code(), display.status_code());
    assert_eq!(withheld.status_code(), StatusCode::FORBIDDEN);
    assert_eq!(withheld.json::<Value>()["code"], "masked");
}

/// A reduced thumbnail fills the thumbnail cache and no other: the viewer's
/// raw and display frames of the same file are still cold after it.
#[tokio::test]
async fn reduced_thumbnails_leave_the_viewer_caches_alone() {
    for case in [plain_j2k(), plain_jpeg()] {
        let dir = tempdir().expect("temp dir");
        let (server, _) = serve_case(dir.path(), &case).await;
        let (_, _, source) = thumbnail(&server, "/api/file/0/frame/0/thumbnail?size=128").await;
        assert_eq!(source, "reduced_decode", "{}", case.name);
        // Raw first: a display frame fills the raw cache itself.
        for viewer_frame in ["frame/0/raw", "frame/0"] {
            let response = server.get(&format!("/api/file/0/{viewer_frame}")).await;
            response.assert_status_ok();
            assert_eq!(
                header(&response, "x-cache"),
                "MISS",
                "{}: a thumbnail filled the cache of {viewer_frame}",
                case.name
            );
        }
    }
}

/// Requests for one thumbnail share one reduced decode: one of them reports
/// it, the others the cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_concurrent_requests_share_one_reduced_decode() {
    let dir = tempdir().expect("temp dir");
    let (server, _) = serve_case(dir.path(), &plain_j2k()).await;
    let path = "/api/file/0/frame/0/thumbnail?size=128";
    let responses =
        futures::future::join_all((0..8).map(|_| async { server.get(path).await })).await;
    let mut sources = responses
        .iter()
        .map(|response| {
            response.assert_status_ok();
            assert_eq!(response.as_bytes(), responses[0].as_bytes());
            (
                header(response, "x-cache"),
                header(response, "x-thumbnail-source"),
            )
        })
        .collect::<Vec<_>>();
    sources.sort();
    let hit = ("HIT".to_string(), "thumbnail_cache".to_string());
    assert_eq!(sources[..7], vec![hit; 7][..]);
    assert_eq!(
        sources[7],
        ("MISS".to_string(), "reduced_decode".to_string())
    );
}

/// A reduced thumbnail is one piece of work under one permit, and reserves
/// what a thumbnail of the frame reserves whichever way it is decoded: the
/// estimate is the entry's, and whether a frame can be reduced is known only
/// once it has been read. A frame too large for the thumbnails' share of the
/// budget is refused with that number, reducible or not.
#[tokio::test]
async fn a_reduced_thumbnail_reserves_the_thumbnail_estimate_under_one_permit() {
    for case in [plain_j2k(), plain_jpeg()] {
        let dir = tempdir().expect("temp dir");
        let entry = entry_of(dir.path(), &case).await;
        let estimate = pixels::decode_estimate(&entry, DecodeWork::Thumbnail);
        let serve = |scheduler: &Arc<DecodeScheduler>| {
            let state =
                support::app_state(vec![entry.clone()]).with_decode_scheduler(scheduler.clone());
            TestServer::new(server::router(state))
        };

        // One permit: a second, taken while the first was held, would never
        // be granted.
        let scheduler = DecodeScheduler::with_limits(1, DecodeLimits::DEFAULT);
        let server = serve(&scheduler);
        let (_, _, source) = thumbnail(&server, "/api/file/0/frame/0/thumbnail?size=128").await;
        assert_eq!(source, "reduced_decode", "{}", case.name);
        let load = scheduler.load_when(|load| load.running == 0).await;
        assert_eq!(load.peak_reserved_bytes, estimate, "{}", case.name);

        // Thumbnails share half the budget: one byte short of the estimate
        // refuses, and the refusal is not remembered.
        let limits = |memory_bytes| DecodeLimits {
            memory_bytes,
            ..DecodeLimits::DEFAULT
        };
        let scheduler = DecodeScheduler::with_limits(2, limits(2 * estimate - 2));
        let server = serve(&scheduler);
        for _ in 0..2 {
            let refused = server.get("/api/file/0/frame/0/thumbnail?size=128").await;
            assert_eq!(refused.status_code(), 422, "{}", case.name);
            let body: Value = refused.json();
            assert_eq!(body["code"], "decode_memory_exceeded", "{}", case.name);
            assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|message| message.contains(&estimate.to_string())),
                "{}: {body}",
                case.name
            );
        }
        assert_eq!(scheduler.load().peak_reserved_bytes, 0, "{}", case.name);
    }
}

/// How often the viewer's pixel code has panicked in this process, on any
/// thread: decodes run on blocking threads, where a panic is caught and
/// answered as a decode error like any other.
fn pixel_code_panics() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PANICS: AtomicUsize = AtomicUsize::new(0);
    static WATCHED: std::sync::Once = std::sync::Once::new();
    WATCHED.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let in_pixel_code = panic
                .location()
                .is_some_and(|location| location.file().replace('\\', "/").contains("src/pixels/"));
            if in_pixel_code {
                PANICS.fetch_add(1, Ordering::SeqCst);
            }
            report(panic);
        }));
    });
    PANICS.load(Ordering::SeqCst)
}

/// `frame` with the bytes of `range` (fractions of its length) replaced by
/// `value`.
fn filled(frame: &[u8], range: std::ops::Range<f64>, value: u8) -> Vec<u8> {
    let at = |fraction: f64| (frame.len() as f64 * fraction) as usize;
    let mut damaged = frame.to_vec();
    damaged[at(range.start)..at(range.end)].fill(value);
    damaged
}

/// `frame` with the bytes of `range` replaced by zeros.
fn zeroed(frame: &[u8], range: std::ops::Range<f64>) -> Vec<u8> {
    filled(frame, range, 0)
}

/// `codestream` with the decomposition levels of its first `COD` stated as
/// `levels`.
fn j2k_levels(codestream: &[u8], levels: u8) -> Vec<u8> {
    files::j2k_u8(codestream, files::j2k_cod(codestream) + 9, levels)
}

/// Frames a reduced decode cannot handle as asked, or cannot handle at all:
/// honest codestreams with bytes replaced or cut off. Whatever they hold,
/// the thumbnail endpoint answers, never worse than the display endpoint
/// does for the same frame: a frame the viewer shows has a thumbnail, and a
/// frame that fails is a decode error that is not cached. No decoder's
/// refusal is a panic.
#[tokio::test]
async fn a_frame_the_decoder_cannot_reduce_is_served_as_the_viewer_serves_it() {
    let j2k = codestream("gray8-512x384.j2k");
    let tiled = codestream("gray12-tile-parts-1040x780.j2k");
    let jpeg = codestream("gray8-baseline-1040x780.jpg");
    let progressive = codestream("gray8-progressive-1041x779.jpg");
    let tiled_case = case(
        "tiled",
        files::JPEG_2000,
        gray(780, 1040, 16, 12),
        &["gray12-tile-parts-1040x780.j2k"],
        WINDOW_12,
        &[],
    );
    let progressive_case = case(
        "progressive",
        files::JPEG_BASELINE,
        gray(779, 1041, 8, 8),
        &["gray8-progressive-1041x779.jpg"],
        WINDOW_8,
        &[],
    );
    let cut =
        |frame: &[u8], fraction: f64| frame[..(frame.len() as f64 * fraction) as usize].to_vec();
    // (what, the case whose header it is put under, the frame)
    let frames: Vec<(&str, Case, Vec<u8>)> = vec![
        // The header states more levels than the data has, or fewer.
        ("more levels stated", plain_j2k(), j2k_levels(&j2k, 6)),
        ("many more levels stated", plain_j2k(), j2k_levels(&j2k, 32)),
        ("fewer levels stated", plain_j2k(), j2k_levels(&j2k, 1)),
        ("no levels stated", plain_j2k(), j2k_levels(&j2k, 0)),
        // A tile-part header that states its own, fewer, levels.
        (
            "a tile with fewer levels",
            tiled_case.clone(),
            files::j2k_tile_part_cod(&tiled, |codestream| j2k_levels(codestream, 1)),
        ),
        (
            "a tile with no levels",
            tiled_case.clone(),
            files::j2k_tile_part_cod(&tiled, |codestream| j2k_levels(codestream, 0)),
        ),
        // Damaged data behind an honest header.
        ("zeros in the data", plain_j2k(), zeroed(&j2k, 0.5..0.6)),
        ("zeros at the end", plain_j2k(), zeroed(&j2k, 0.9..1.0)),
        (
            "zeros in a tile",
            tiled_case.clone(),
            zeroed(&tiled, 0.3..0.5),
        ),
        // Bytes no packet may hold: the decoder itself gives up, reduced
        // or not.
        (
            "0xFF in the data",
            plain_j2k(),
            filled(&j2k, 0.1..0.2, 0xFF),
        ),
        (
            "0xFF in a tile",
            tiled_case.clone(),
            filled(&tiled, 0.4..0.5, 0xFF),
        ),
        ("cut short", plain_j2k(), cut(&j2k, 0.5)),
        ("cut after the header", plain_j2k(), cut(&j2k, 0.02)),
        (
            "JPEG, zeros in the scan",
            plain_jpeg(),
            zeroed(&jpeg, 0.5..0.6),
        ),
        ("JPEG, cut short", plain_jpeg(), cut(&jpeg, 0.5)),
        ("JPEG, cut in the tables", plain_jpeg(), cut(&jpeg, 0.01)),
        (
            "progressive JPEG, zeros in a scan",
            progressive_case.clone(),
            zeroed(&progressive, 0.5..0.6),
        ),
        (
            "progressive JPEG, cut short",
            progressive_case.clone(),
            cut(&progressive, 0.4),
        ),
    ];
    let panics = pixel_code_panics();
    let (mut shown, mut failed) = (0, 0);
    for (what, case, frame) in frames {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("damaged.dcm");
        write_case(&path, &case, Some(&[frame]), &[]);
        let entry = files::list(std::slice::from_ref(&path)).await.remove(0);
        assert!(
            thumbnail_reduction(&entry, 128, WindowMode::Default).is_some(),
            "{what}: the thumbnail asks for a reduction"
        );
        let server = files::serve(vec![entry]);
        let thumbnail_path = "/api/file/0/frame/0/thumbnail?size=128";
        let mut served = false;
        for attempt in 0..2 {
            let response = server.get(thumbnail_path).await;
            let display = server.get("/api/file/0/frame/0").await;
            let context = format!("{what}, attempt {attempt}");
            if std::env::var_os("REDUCED_THUMBNAIL_REPORT").is_some() {
                eprintln!(
                    "{context}: thumbnail {} {:?}, display {}",
                    response.status_code(),
                    response.maybe_header("x-thumbnail-source"),
                    display.status_code()
                );
            }
            if response.status_code() == StatusCode::OK {
                // A frame that has a thumbnail keeps having it.
                image::load_from_memory_with_format(response.as_bytes(), image::ImageFormat::Jpeg)
                    .unwrap_or_else(|error| panic!("{context}: {error}"));
                served = true;
            } else {
                assert!(!served, "{context}: a thumbnail that was served is gone");
                assert_eq!(
                    response.status_code(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "{context}: {}",
                    response.text()
                );
                assert_eq!(
                    response.json::<Value>()["code"],
                    "pixel_decode_failed",
                    "{context}"
                );
                assert!(
                    response.maybe_header("x-cache").is_none(),
                    "{context}: a failure has no cache state"
                );
                assert_eq!(
                    display.status_code(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "{context}: the viewer shows this frame, so it has a thumbnail: {}",
                    response.text()
                );
            }
        }

        if served {
            shown += 1;
            continue;
        }
        failed += 1;
        // The failure was not kept: the same entry over the honest frame is
        // rendered, not answered from a cache.
        write_case(&path, &case, None, &[]);
        let (_, cache, source) = thumbnail(&server, thumbnail_path).await;
        assert_eq!(
            (cache.as_str(), source.as_str()),
            ("MISS", "reduced_decode"),
            "{what}: after the file was repaired"
        );
    }
    assert!(
        shown >= 4 && failed >= 4,
        "{shown} damaged frames were shown and {failed} refused"
    );
    assert_eq!(
        pixel_code_panics(),
        panics,
        "the pixel code panicked while damaged frames were served"
    );
}
