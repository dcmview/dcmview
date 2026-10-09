//! What a compressed DICOM frame may cost before it is known to be the
//! image its file's header declares (`pixels::codestream`).
//!
//! A frame's own header is input nobody vouches for, so the limits are
//! stated in things that can be counted, and none of them is a time: the
//! heap the process holds while the header is read and while a frame is
//! refused, counted at the allocator, with files a few hundred bytes long
//! that declare images of millions of pixels.
//!
//! These tests are a test binary of their own because they replace the
//! global allocator. For the same reason no test here decodes an honest
//! JPEG 2000 frame: the decoder frees with layouts that the allocator shim
//! of a debug build rejects. Honest frames of every kind are decoded in
//! `tests/integration/codestream_agreement.rs`.
//!
//! `CODESTREAM_COST_REPORT=1 cargo test --test codestream_cost -- --nocapture`
//! prints what each case cost.

#[path = "../integration/codestream_files.rs"]
mod codestream_files;
mod heap;

use axum::http::StatusCode;
use codestream_files::{self as files, hex, siz, Layout};
use dcmview::pixels::codestream::{
    agrees, declared, jxl_decode_limit, CodestreamKind, Declared, Structure,
    CODESTREAM_MAX_HEADER_SEGMENTS, CODESTREAM_MISMATCH, J2K_MAX_TILES, J2K_MIN_PACKET_BUDGET,
    J2K_PIXELS_PER_PACKET, JP2_MAX_BOXES, JPEG_MAX_SCANS,
};
use dcmview::types::FileEntry;
use tempfile::tempdir;

/// Every test of this binary runs on the counting allocator.
#[global_allocator]
static ALLOCATOR: heap::CountingAllocator = heap::CountingAllocator;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

/// The most heap reading one frame's header may hold: nothing that grows
/// with what the frame declares. The JPEG XL header is read by the decoder
/// itself, which builds its header structures.
const HEADER_HEAP: u64 = 64 * KIB;
const JXL_HEADER_HEAP: u64 = MIB;

/// The most heap the whole server may hold while it refuses one frame,
/// whatever the frame declares, over twice the file's own length: the
/// request, the file's data set and the frame's encoded bytes. The frames
/// below declare images that take 16 MiB and more to decode.
const REFUSAL_HEAP: u64 = 256 * KIB;

fn report(line: impl FnOnce() -> String) {
    if std::env::var_os("CODESTREAM_COST_REPORT").is_some() {
        println!("{}", line());
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn header_heap(kind: CodestreamKind) -> u64 {
    if kind == CodestreamKind::JpegXl {
        JXL_HEADER_HEAP
    } else {
        HEADER_HEAP
    }
}

/// A deterministic stream of bytes.
fn noise(count: usize, seed: u64) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64 ^ seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}

fn honest_streams() -> Vec<(&'static str, CodestreamKind, Vec<u8>)> {
    use CodestreamKind::{Jpeg, Jpeg2000, JpegLs, JpegXl};
    vec![
        ("JPEG_GRAY8_RESTART", Jpeg, hex(files::JPEG_GRAY8_RESTART)),
        (
            "JPEG_RGB8_PROGRESSIVE",
            Jpeg,
            hex(files::JPEG_RGB8_PROGRESSIVE),
        ),
        (
            "JPEG_LOSSLESS_GRAY12",
            Jpeg,
            hex(files::JPEG_LOSSLESS_GRAY12),
        ),
        ("JPEGLS_GRAY8", JpegLs, hex(files::JPEGLS_GRAY8)),
        ("JPEGLS_GRAY12", JpegLs, hex(files::JPEGLS_GRAY12)),
        ("J2K_GRAY8", Jpeg2000, hex(files::J2K_GRAY8)),
        ("J2K_GRAY8_JP2", Jpeg2000, hex(files::J2K_GRAY8_JP2)),
        ("J2K_RGB8_RCT", Jpeg2000, hex(files::J2K_RGB8_RCT)),
        (
            "J2K_GRAY8_TILE_PARTS",
            Jpeg2000,
            hex(files::J2K_GRAY8_TILE_PARTS),
        ),
        ("JXL_GRAY8", JpegXl, hex(files::JXL_GRAY8)),
        ("JXL_RGBA8", JpegXl, hex(files::JXL_RGBA8)),
        (
            "JXL_GRAY8_CONTAINER",
            JpegXl,
            hex(files::JXL_GRAY8_CONTAINER),
        ),
    ]
}

/// Whatever a frame holds, reading what it declares returns, and holds no
/// more than a header's worth of heap: every prefix of every honest stream,
/// every stream with one of its first bytes replaced, and noise.
#[test]
fn reading_what_a_frame_declares_never_panics_and_holds_a_header_s_worth_of_heap() {
    let _alone = heap::alone();
    for (name, kind, stream) in honest_streams() {
        let limit = header_heap(kind);
        let mut inputs = (0..=stream.len())
            .map(|length| stream[..length].to_vec())
            .collect::<Vec<_>>();
        // The JPEG XL header is read by the decoder, whose own cap on an
        // embedded colour profile is what bounds a header edited to declare
        // one (see `codestream::declared`); its edits are left out here.
        let edited = if kind == CodestreamKind::JpegXl {
            0
        } else {
            stream.len().min(160)
        };
        for at in 0..edited {
            for value in [0x00, 0x01, 0x7F, 0x80, 0xFF] {
                let mut edited = stream.clone();
                edited[at] = value;
                inputs.push(edited);
            }
        }
        for seed in 0..8 {
            let mut noisy = stream[..stream.len().min(4)].to_vec();
            noisy.extend(noise(4096, seed));
            inputs.push(noisy);
            inputs.push(noise(4096, seed));
        }
        let mut most = 0;
        for input in &inputs {
            let (_, held) = heap::peak_during(|| declared(kind, input));
            most = most.max(held);
            assert!(
                held <= limit,
                "{name}: {held} bytes of heap held reading a {}-byte frame, the limit is {limit}",
                input.len()
            );
        }
        report(|| {
            format!(
                "{name}: {} inputs, at most {most} bytes of heap",
                inputs.len()
            )
        });
    }
}

fn jpeg(process: u8, scans: u32) -> Structure {
    Structure::Jpeg { process, scans }
}

fn j2k(
    codestream: std::ops::Range<usize>,
    tiles: u64,
    tile_pixels: u64,
    packets: u64,
) -> Structure {
    Structure::Jpeg2000 {
        codestream,
        uniform: true,
        tiles,
        tile_pixels,
        packets,
    }
}

const JXL_STILL: Structure = Structure::JpegXl {
    integer_samples: true,
    animated: false,
};

fn says(
    columns: u32,
    rows: u32,
    components: u32,
    precision: u32,
    structure: Structure,
) -> Declared {
    Declared {
        columns,
        rows,
        components,
        precision,
        structure,
    }
}

/// What honest encoders' output declares, read from the bytes alone.
#[test]
fn a_frame_s_own_header_is_read_as_its_format_defines_it() {
    let _alone = heap::alone();
    use CodestreamKind::{Jpeg, Jpeg2000, JpegLs, JpegXl};
    let j2k_len = |text: &str| 0..hex(text).len();
    let cases = [
        (
            Jpeg,
            files::JPEG_GRAY8_RESTART,
            says(16, 16, 1, 8, jpeg(0xC0, 1)),
        ),
        (
            Jpeg,
            files::JPEG_GRAY8_WITH_PROFILE,
            says(16, 16, 1, 8, jpeg(0xC0, 1)),
        ),
        (
            Jpeg,
            files::JPEG_RGB8_420_ODD,
            says(15, 9, 3, 8, jpeg(0xC0, 1)),
        ),
        (
            Jpeg,
            files::JPEG_GRAY8_PROGRESSIVE,
            says(16, 16, 1, 8, jpeg(0xC2, 6)),
        ),
        (
            Jpeg,
            files::JPEG_RGB8_PROGRESSIVE,
            says(16, 16, 3, 8, jpeg(0xC2, 10)),
        ),
        (
            Jpeg,
            files::JPEG_LOSSLESS_GRAY8,
            says(16, 16, 1, 8, jpeg(0xC3, 1)),
        ),
        (
            Jpeg,
            files::JPEG_LOSSLESS_GRAY12,
            says(16, 16, 1, 16, jpeg(0xC3, 1)),
        ),
        (
            JpegLs,
            files::JPEGLS_GRAY8,
            says(16, 16, 1, 8, jpeg(0xF7, 0)),
        ),
        (
            JpegLs,
            files::JPEGLS_GRAY12,
            says(16, 16, 1, 16, jpeg(0xF7, 0)),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8,
            says(16, 16, 1, 8, j2k(j2k_len(files::J2K_GRAY8), 1, 256, 5)),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_ONE_RESOLUTION,
            says(
                16,
                16,
                1,
                8,
                j2k(j2k_len(files::J2K_GRAY8_ONE_RESOLUTION), 1, 256, 1),
            ),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_TILED,
            says(16, 16, 1, 8, j2k(j2k_len(files::J2K_GRAY8_TILED), 4, 64, 2)),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_LAYERS,
            says(
                16,
                16,
                1,
                8,
                j2k(j2k_len(files::J2K_GRAY8_LAYERS), 1, 256, 15),
            ),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_PRECINCTS,
            says(
                64,
                64,
                1,
                8,
                j2k(j2k_len(files::J2K_GRAY8_PRECINCTS), 1, 4096, 12),
            ),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_TILE_PARTS,
            says(
                64,
                64,
                1,
                8,
                j2k(j2k_len(files::J2K_GRAY8_TILE_PARTS), 4, 1024, 3),
            ),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_JP2,
            says(
                16,
                16,
                1,
                8,
                j2k(85..hex(files::J2K_GRAY8_JP2).len(), 1, 256, 5),
            ),
        ),
        (
            Jpeg2000,
            files::J2K_RGB8_RCT,
            says(16, 16, 3, 8, j2k(j2k_len(files::J2K_RGB8_RCT), 1, 256, 5)),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY12,
            says(16, 16, 1, 12, j2k(j2k_len(files::J2K_GRAY12), 1, 256, 3)),
        ),
        (
            Jpeg2000,
            files::J2K_GRAY8_ODD,
            says(15, 9, 1, 8, j2k(j2k_len(files::J2K_GRAY8_ODD), 1, 135, 4)),
        ),
        (JpegXl, files::JXL_GRAY8, says(16, 16, 1, 8, JXL_STILL)),
        (JpegXl, files::JXL_RGB8, says(16, 16, 3, 8, JXL_STILL)),
        (JpegXl, files::JXL_GRAY16, says(16, 16, 1, 16, JXL_STILL)),
        (
            JpegXl,
            files::JXL_GRAY8_CONTAINER,
            says(16, 16, 1, 8, JXL_STILL),
        ),
        (JpegXl, files::JXL_RGBA8, says(16, 16, 4, 8, JXL_STILL)),
        (
            JpegXl,
            files::JXL_GRAY8_2048X2048,
            says(2048, 2048, 1, 8, JXL_STILL),
        ),
        (
            JpegXl,
            files::JXL_ANIMATED,
            says(
                16,
                16,
                4,
                8,
                Structure::JpegXl {
                    integer_samples: true,
                    animated: true,
                },
            ),
        ),
    ];
    for (kind, text, expected) in cases {
        let read = declared(kind, &hex(text)).unwrap_or_else(|error| {
            panic!("{kind:?} {expected:?}: an honest header is read: {error:#}")
        });
        assert_eq!(read, expected, "{kind:?}");
    }
}

/// Headers that are not there to read, or lie beyond what is read for one.
#[test]
fn a_header_that_cannot_be_read_within_the_bounds_is_an_error() {
    let _alone = heap::alone();
    use CodestreamKind::{Jpeg, Jpeg2000, JpegLs, JpegXl};
    let jpeg = hex(files::JPEG_GRAY8_RESTART);
    let jpeg_ls = hex(files::JPEGLS_GRAY8);
    let j2k = hex(files::J2K_GRAY8);
    let jp2 = hex(files::J2K_GRAY8_JP2);
    let jxl = hex(files::JXL_GRAY8);
    // A JP2 file whose codestream box comes after more boxes than are read.
    let mut boxes = jp2[..12].to_vec();
    for _ in 0..JP2_MAX_BOXES {
        boxes.extend_from_slice(&[0, 0, 0, 8, b'f', b'r', b'e', b'e']);
    }
    boxes.extend_from_slice(&jp2[12..]);
    // Frame headers cut inside themselves.
    let jpeg_cut = jpeg[..files::jpeg_frame_header(&jpeg) + 4].to_vec();
    let within = CODESTREAM_MAX_HEADER_SEGMENTS - 8;
    let beyond = CODESTREAM_MAX_HEADER_SEGMENTS + 1;
    let cases: Vec<(&str, CodestreamKind, Vec<u8>, bool)> = vec![
        ("jpeg", Jpeg, jpeg.clone(), true),
        ("jpeg, empty", Jpeg, Vec::new(), false),
        ("jpeg, no start of image", Jpeg, jpeg[2..].to_vec(), false),
        ("jpeg, cut in the frame header", Jpeg, jpeg_cut, false),
        ("jpeg, a jpeg-ls frame", Jpeg, jpeg_ls.clone(), false),
        ("jpeg, noise", Jpeg, noise(512, 1), false),
        (
            "jpeg, many segments before the frame header",
            Jpeg,
            files::jpeg_with_segments(&jpeg, within),
            true,
        ),
        (
            "jpeg, more segments than are read",
            Jpeg,
            files::jpeg_with_segments(&jpeg, beyond),
            false,
        ),
        ("jpeg-ls", JpegLs, jpeg_ls.clone(), true),
        ("jpeg-ls, a jpeg frame", JpegLs, jpeg.clone(), false),
        ("jpeg-ls, cut", JpegLs, jpeg_ls[..8].to_vec(), false),
        (
            "jpeg-ls, more segments than are read",
            JpegLs,
            files::jpeg_with_segments(&jpeg_ls, beyond),
            false,
        ),
        ("j2k", Jpeg2000, j2k.clone(), true),
        ("j2k, empty", Jpeg2000, Vec::new(), false),
        ("j2k, cut in SIZ", Jpeg2000, j2k[..40].to_vec(), false),
        (
            "j2k, cut before the coding style",
            Jpeg2000,
            j2k[..47].to_vec(),
            false,
        ),
        ("j2k, a jpeg frame", Jpeg2000, jpeg.clone(), false),
        ("j2k, noise", Jpeg2000, noise(512, 2), false),
        (
            "j2k, a jp2 signature alone",
            Jpeg2000,
            jp2[..12].to_vec(),
            false,
        ),
        (
            "j2k, a jp2 file cut in its codestream box",
            Jpeg2000,
            jp2[..100].to_vec(),
            false,
        ),
        (
            "j2k, a jp2 file with more boxes than are read",
            Jpeg2000,
            boxes,
            false,
        ),
        (
            "j2k, many segments in the main header",
            Jpeg2000,
            files::j2k_with_segments(&j2k, within),
            true,
        ),
        (
            "j2k, more main header segments than are read",
            Jpeg2000,
            files::j2k_with_segments(&j2k, beyond),
            false,
        ),
        (
            "j2k, 16,384 components",
            Jpeg2000,
            files::j2k_components(&j2k, 16_384),
            false,
        ),
        (
            "j2k, no tile width",
            Jpeg2000,
            files::j2k_u32(&j2k, siz::XTSIZ, 0),
            false,
        ),
        (
            "j2k, an image origin past its size",
            Jpeg2000,
            files::j2k_u32(&j2k, siz::XOSIZ, 16),
            false,
        ),
        ("jxl", JpegXl, jxl.clone(), true),
        ("jxl, empty", JpegXl, Vec::new(), false),
        ("jxl, cut", JpegXl, jxl[..3].to_vec(), false),
        ("jxl, a jpeg frame", JpegXl, jpeg.clone(), false),
        ("jxl, noise", JpegXl, noise(512, 3), false),
    ];
    for (name, kind, frame, readable) in cases {
        let (read, held) = heap::peak_during(|| declared(kind, &frame));
        assert_eq!(read.is_ok(), readable, "{name}: {:?}", read.err());
        let limit = header_heap(kind);
        assert!(
            held <= limit,
            "{name}: {held} bytes of heap held, the limit is {limit}"
        );
    }
}

/// The catalog entry the production loader makes for a header.
fn entry(runtime: &tokio::runtime::Runtime, transfer_syntax: &str, layout: Layout) -> FileEntry {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("header.dcm");
    files::write(&path, transfer_syntax, layout, &[vec![0, 0]]);
    runtime
        .block_on(files::list(&[path]))
        .pop()
        .expect("one entry")
}

/// The accept table of `codestream::agrees`, row by row.
#[test]
fn a_frame_agrees_with_its_entry_exactly_by_the_accept_table() {
    let _alone = heap::alone();
    let runtime = runtime();
    let gray8 = entry(&runtime, files::JPEG_2000, Layout::gray8(16, 16));
    let gray16 = entry(&runtime, files::JPEG_2000, Layout::gray(16, 16, 16, 12));
    let color8 = entry(&runtime, files::JPEG_2000, Layout::color8(16, 16, "RGB"));
    let wide = entry(&runtime, files::JPEG_2000, Layout::gray8(1024, 1024));
    let still = || JXL_STILL;
    let floating = Structure::JpegXl {
        integer_samples: false,
        animated: false,
    };
    let moving = Structure::JpegXl {
        integer_samples: true,
        animated: true,
    };
    let subsampled = Structure::Jpeg2000 {
        codestream: 0..0,
        uniform: false,
        tiles: 1,
        tile_pixels: 256,
        packets: 6,
    };
    let one_tile = |packets| j2k(0..0, 1, 256, packets);
    let floor = J2K_MIN_PACKET_BUDGET;
    let wide_budget = 1024 * 1024 / J2K_PIXELS_PER_PACKET;
    assert!(
        wide_budget < floor,
        "the floor decides for a megapixel tile"
    );
    let cases: Vec<(&str, &FileEntry, Declared, bool)> = vec![
        // Size and components, the same for every kind.
        (
            "the entry's image",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC0, 1)),
            true,
        ),
        ("wider", &gray8, says(17, 16, 1, 8, jpeg(0xC0, 1)), false),
        ("narrower", &gray8, says(15, 16, 1, 8, jpeg(0xC0, 1)), false),
        ("taller", &gray8, says(16, 17, 1, 8, jpeg(0xC0, 1)), false),
        ("shorter", &gray8, says(16, 15, 1, 8, jpeg(0xC0, 1)), false),
        (
            "transposed",
            &wide,
            says(1024, 1023, 1, 8, jpeg(0xC0, 1)),
            false,
        ),
        (
            "more components",
            &gray8,
            says(16, 16, 3, 8, jpeg(0xC0, 1)),
            false,
        ),
        (
            "fewer components",
            &color8,
            says(16, 16, 1, 8, jpeg(0xC0, 1)),
            false,
        ),
        ("colour", &color8, says(16, 16, 3, 8, jpeg(0xC0, 3)), true),
        // JPEG by process.
        (
            "extended sequential, 8 bits",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC1, 1)),
            true,
        ),
        (
            "progressive, 8 bits",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC2, 10)),
            true,
        ),
        (
            "baseline under 16 bits",
            &gray16,
            says(16, 16, 1, 8, jpeg(0xC0, 1)),
            false,
        ),
        (
            "extended, 12 bits",
            &gray16,
            says(16, 16, 1, 12, jpeg(0xC1, 1)),
            false,
        ),
        (
            "lossless, 8 in 8",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC3, 1)),
            true,
        ),
        (
            "lossless, 7 in 8",
            &gray8,
            says(16, 16, 1, 7, jpeg(0xC3, 1)),
            false,
        ),
        (
            "lossless, 12 in 8",
            &gray8,
            says(16, 16, 1, 12, jpeg(0xC3, 1)),
            false,
        ),
        (
            "lossless, 8 in 16",
            &gray16,
            says(16, 16, 1, 8, jpeg(0xC3, 1)),
            false,
        ),
        (
            "lossless, 7 in 16",
            &gray16,
            says(16, 16, 1, 7, jpeg(0xC3, 1)),
            true,
        ),
        (
            "lossless, 9 in 16",
            &gray16,
            says(16, 16, 1, 9, jpeg(0xC3, 1)),
            true,
        ),
        (
            "lossless, 12 in 16",
            &gray16,
            says(16, 16, 1, 12, jpeg(0xC3, 1)),
            true,
        ),
        (
            "lossless, 16 in 16",
            &gray16,
            says(16, 16, 1, 16, jpeg(0xC3, 1)),
            true,
        ),
        (
            "lossless, 1 bit",
            &gray16,
            says(16, 16, 1, 1, jpeg(0xC3, 1)),
            false,
        ),
        (
            "differential",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC5, 1)),
            false,
        ),
        (
            "arithmetic",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC9, 1)),
            false,
        ),
        (
            "as many scans as are allowed",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC2, JPEG_MAX_SCANS)),
            true,
        ),
        (
            "a scan more",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xC2, JPEG_MAX_SCANS + 1)),
            false,
        ),
        // JPEG-LS.
        (
            "jpeg-ls, 8 in 8",
            &gray8,
            says(16, 16, 1, 8, jpeg(0xF7, 0)),
            true,
        ),
        (
            "jpeg-ls, 2 in 8",
            &gray8,
            says(16, 16, 1, 2, jpeg(0xF7, 0)),
            true,
        ),
        (
            "jpeg-ls, 9 in 8",
            &gray8,
            says(16, 16, 1, 9, jpeg(0xF7, 0)),
            false,
        ),
        (
            "jpeg-ls, 8 in 16",
            &gray16,
            says(16, 16, 1, 8, jpeg(0xF7, 0)),
            false,
        ),
        (
            "jpeg-ls, 12 in 16",
            &gray16,
            says(16, 16, 1, 12, jpeg(0xF7, 0)),
            true,
        ),
        (
            "jpeg-ls, 16 in 16",
            &gray16,
            says(16, 16, 1, 16, jpeg(0xF7, 0)),
            true,
        ),
        // JPEG 2000.
        ("j2k, 8 in 8", &gray8, says(16, 16, 1, 8, one_tile(6)), true),
        ("j2k, 1 in 8", &gray8, says(16, 16, 1, 1, one_tile(6)), true),
        (
            "j2k, 9 in 8",
            &gray8,
            says(16, 16, 1, 9, one_tile(6)),
            false,
        ),
        (
            "j2k, 8 in 16",
            &gray16,
            says(16, 16, 1, 8, one_tile(6)),
            true,
        ),
        (
            "j2k, 16 in 16",
            &gray16,
            says(16, 16, 1, 16, one_tile(6)),
            true,
        ),
        (
            "j2k, 17 in 16",
            &gray16,
            says(16, 16, 1, 17, one_tile(6)),
            false,
        ),
        (
            "j2k, subsampled",
            &gray8,
            says(16, 16, 1, 8, subsampled),
            false,
        ),
        (
            "j2k, as many tiles as are allowed",
            &gray8,
            says(16, 16, 1, 8, j2k(0..0, J2K_MAX_TILES, 1, 1)),
            true,
        ),
        (
            "j2k, a tile more",
            &gray8,
            says(16, 16, 1, 8, j2k(0..0, J2K_MAX_TILES + 1, 1, 1)),
            false,
        ),
        (
            "j2k, the least packet budget",
            &gray8,
            says(16, 16, 1, 8, one_tile(floor)),
            true,
        ),
        (
            "j2k, a packet more",
            &gray8,
            says(16, 16, 1, 8, one_tile(floor + 1)),
            false,
        ),
        (
            "j2k, a megapixel tile within the least budget",
            &wide,
            says(1024, 1024, 1, 8, j2k(0..0, 1, 1024 * 1024, floor)),
            true,
        ),
        (
            "j2k, a tile large enough for more packets",
            &wide,
            says(
                1024,
                1024,
                1,
                8,
                j2k(0..0, 1, 1 << 30, (1 << 30) / J2K_PIXELS_PER_PACKET),
            ),
            true,
        ),
        (
            "j2k, a packet more than a large tile's budget",
            &wide,
            says(
                1024,
                1024,
                1,
                8,
                j2k(0..0, 1, 1 << 30, (1 << 30) / J2K_PIXELS_PER_PACKET + 1),
            ),
            false,
        ),
        // JPEG XL.
        ("jxl, 8 in 8", &gray8, says(16, 16, 1, 8, still()), true),
        ("jxl, 16 in 8", &gray8, says(16, 16, 1, 16, still()), false),
        ("jxl, 12 in 16", &gray16, says(16, 16, 1, 12, still()), true),
        ("jxl, 8 in 16", &gray16, says(16, 16, 1, 8, still()), true),
        (
            "jxl, with alpha",
            &color8,
            says(16, 16, 4, 8, still()),
            false,
        ),
        (
            "jxl, floating point",
            &gray16,
            says(16, 16, 1, 16, floating),
            false,
        ),
        (
            "jxl, an animation",
            &gray8,
            says(16, 16, 1, 8, moving),
            false,
        ),
    ];
    for (name, file, declared, accepted) in cases {
        let verdict = agrees(file, &declared);
        assert_eq!(verdict.is_ok(), accepted, "{name}: {:?}", verdict.err());
    }
}

struct Hostile {
    name: &'static str,
    transfer_syntax: &'static str,
    layout: Layout,
    frame: Vec<u8>,
}

/// Frames a few hundred bytes long whose own headers ask for far more than
/// their file's header: larger and smaller images, other component counts
/// and depths, absurd tile, packet and scan structures.
fn hostile() -> Vec<Hostile> {
    let gray8 = Layout::gray8(16, 16);
    let color8 = Layout::color8(16, 16, "RGB");
    let jpeg = hex(files::JPEG_GRAY8_RESTART);
    let progressive = hex(files::JPEG_GRAY8_PROGRESSIVE);
    let jpeg_ls = hex(files::JPEGLS_GRAY8);
    let j2k = hex(files::J2K_GRAY8);
    let precincts = hex(files::J2K_GRAY8_PRECINCTS);
    let tile_parts = hex(files::J2K_GRAY8_TILE_PARTS);
    let many_packets =
        |codestream: &[u8]| files::j2k_layers(&files::j2k_precincts(codestream, 1), 65_535);
    let case = |name, transfer_syntax, layout, frame| Hostile {
        name,
        transfer_syntax,
        layout,
        frame,
    };
    vec![
        case(
            "jpeg, 65,535 x 65,535",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_sized(&jpeg, 65_535, 65_535),
        ),
        case(
            "jpeg, 4,096 x 4,096",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_sized(&jpeg, 4096, 4096),
        ),
        case(
            "jpeg, 8 x 8",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_sized(&jpeg, 8, 8),
        ),
        case(
            "jpeg, three components",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_components(&files::jpeg_sized(&jpeg, 4096, 4096), 3),
        ),
        case(
            "jpeg, four components",
            files::JPEG_BASELINE,
            color8,
            files::jpeg_components(&jpeg, 4),
        ),
        case(
            "jpeg, one component under three",
            files::JPEG_BASELINE,
            color8,
            jpeg.clone(),
        ),
        case(
            "jpeg, 16-bit lossless under 8 bits",
            files::JPEG_LOSSLESS_SV1,
            gray8,
            hex(files::JPEG_LOSSLESS_GRAY12),
        ),
        case(
            "jpeg, 7-bit lossless under 8 bits",
            files::JPEG_LOSSLESS_SV1,
            gray8,
            files::jpeg_precision(&hex(files::JPEG_LOSSLESS_GRAY8), 7),
        ),
        case(
            "jpeg, 8 bits under 16",
            files::JPEG_BASELINE,
            Layout::gray(16, 16, 16, 12),
            jpeg.clone(),
        ),
        case(
            "jpeg, 5,000 scans",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_with_scans(&progressive, 5000),
        ),
        case(
            "jpeg, more segments than are read",
            files::JPEG_BASELINE,
            gray8,
            files::jpeg_with_segments(&jpeg, CODESTREAM_MAX_HEADER_SEGMENTS + 1),
        ),
        case(
            "jpeg, cut in the frame header",
            files::JPEG_BASELINE,
            gray8,
            jpeg[..files::jpeg_frame_header(&jpeg) + 4].to_vec(),
        ),
        case("jpeg, noise", files::JPEG_BASELINE, gray8, noise(600, 4)),
        case(
            "jpeg-ls, 65,535 x 65,535",
            files::JPEG_LS,
            gray8,
            files::jpeg_sized(&jpeg_ls, 65_535, 65_535),
        ),
        case(
            "jpeg-ls, 4,096 x 4,096",
            files::JPEG_LS,
            gray8,
            files::jpeg_sized(&jpeg_ls, 4096, 4096),
        ),
        case(
            "jpeg-ls, 8 x 8",
            files::JPEG_LS,
            gray8,
            files::jpeg_sized(&jpeg_ls, 8, 8),
        ),
        case(
            "jpeg-ls, 255 components",
            files::JPEG_LS,
            gray8,
            files::jpeg_components(&files::jpeg_sized(&jpeg_ls, 4096, 4096), 255),
        ),
        case(
            "jpeg-ls, 16 bits under 8",
            files::JPEG_LS,
            gray8,
            hex(files::JPEGLS_GRAY12),
        ),
        case("jpeg-ls, cut", files::JPEG_LS, gray8, jpeg_ls[..8].to_vec()),
        case("jpeg-ls, noise", files::JPEG_LS, gray8, noise(600, 5)),
        case(
            "j2k, 4,294,967,295 x 4,294,967,295",
            files::JPEG_2000,
            gray8,
            files::j2k_sized(&j2k, u32::MAX, u32::MAX),
        ),
        case(
            "j2k, 16,000 x 16,000",
            files::JPEG_2000,
            gray8,
            files::j2k_sized(&j2k, 16_000, 16_000),
        ),
        case(
            "j2k, 8 x 8",
            files::JPEG_2000,
            gray8,
            files::j2k_sized(&j2k, 8, 8),
        ),
        case(
            "j2k, an image offset that leaves 8 x 8",
            files::JPEG_2000,
            gray8,
            files::j2k_u32(&files::j2k_u32(&j2k, siz::XOSIZ, 8), siz::YOSIZ, 8),
        ),
        case(
            "j2k, three components",
            files::JPEG_2000,
            gray8,
            files::j2k_components(&files::j2k_sized(&j2k, 2048, 2048), 3),
        ),
        case(
            "j2k, one component under three",
            files::JPEG_2000,
            color8,
            j2k.clone(),
        ),
        case(
            "j2k, 16 bits under 8",
            files::JPEG_2000,
            gray8,
            hex(files::J2K_GRAY16),
        ),
        case(
            "j2k, 38 bits",
            files::JPEG_2000,
            gray8,
            files::j2k_u8(&j2k, siz::SSIZ, 37),
        ),
        case(
            "j2k, subsampled",
            files::JPEG_2000,
            gray8,
            files::j2k_u8(&j2k, siz::XRSIZ, 2),
        ),
        case(
            "j2k, 16,384 tiles of one pixel",
            files::JPEG_2000,
            Layout::gray8(128, 128),
            files::j2k_u32(
                &files::j2k_u32(&files::j2k_sized(&j2k, 128, 128), siz::XTSIZ, 1),
                siz::YTSIZ,
                1,
            ),
        ),
        case(
            "j2k, 65,535 layers of tiny precincts",
            files::JPEG_2000,
            Layout::gray8(64, 64),
            many_packets(&precincts),
        ),
        case(
            "j2k, the same in a tile-part header",
            files::JPEG_2000,
            Layout::gray8(64, 64),
            files::j2k_tile_part_cod(&tile_parts, many_packets),
        ),
        case(
            "j2k, more main header segments than are read",
            files::JPEG_2000,
            gray8,
            files::j2k_with_segments(&j2k, CODESTREAM_MAX_HEADER_SEGMENTS + 1),
        ),
        case(
            "j2k, cut in SIZ",
            files::JPEG_2000,
            gray8,
            j2k[..40].to_vec(),
        ),
        case("j2k, noise", files::JPEG_2000, gray8, noise(600, 6)),
        case(
            "jxl, 2,048 x 2,048",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_GRAY8_2048X2048),
        ),
        case(
            "jxl, 8 x 8",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_GRAY8_8X8),
        ),
        case(
            "jxl, 32 x 32",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_GRAY8_32X32),
        ),
        case(
            "jxl, three components",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_RGB8),
        ),
        case(
            "jxl, one component under three",
            files::JPEG_XL,
            color8,
            hex(files::JXL_GRAY8),
        ),
        case(
            "jxl, an alpha channel",
            files::JPEG_XL,
            color8,
            hex(files::JXL_RGBA8),
        ),
        case(
            "jxl, 16 bits under 8",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_GRAY16),
        ),
        case(
            "jxl, an animation",
            files::JPEG_XL,
            Layout::color8(16, 16, "RGB"),
            hex(files::JXL_ANIMATED),
        ),
        case(
            "jxl, cut",
            files::JPEG_XL,
            gray8,
            hex(files::JXL_GRAY8)[..3].to_vec(),
        ),
        case("jxl, noise", files::JPEG_XL, gray8, noise(600, 7)),
        case(
            "deflate, 256 MiB under 32 bytes",
            files::DEFLATED_FRAME,
            Layout::gray(16, 16, 1, 1),
            files::deflated_zeros(1 << 28),
        ),
        case(
            "deflate, a byte under 32",
            files::DEFLATED_FRAME,
            Layout::gray(16, 16, 1, 1),
            files::deflated_zeros(31),
        ),
    ]
}

/// A frame that asks for more than its entry is refused at the cost of
/// reading it: the display and the raw endpoint both answer with a decode
/// error that says so, and the process never holds more than
/// `REFUSAL_HEAP` for it.
#[test]
fn a_frame_that_disagrees_with_its_header_is_refused_before_anything_is_allocated_for_it() {
    let _alone = heap::alone();
    let runtime = runtime();
    let dir = tempdir().expect("temp dir");
    for (number, case) in hostile().into_iter().enumerate() {
        let path = dir.path().join(format!("hostile-{number}.dcm"));
        files::write(&path, case.transfer_syntax, case.layout, &[case.frame]);
        let length = std::fs::metadata(&path).expect("written").len();
        assert!(length < 2 * MIB, "{}: a small file", case.name);
        let server = files::serve(runtime.block_on(files::list(&[path])));
        for endpoint in ["/api/file/0/frame/0", "/api/file/0/frame/0/raw"] {
            // Whatever the first request leaves in caches is not the frame.
            for attempt in 0..2 {
                let (response, held) =
                    heap::peak_during(|| runtime.block_on(async { server.get(endpoint).await }));
                let context = format!("{} at {endpoint}, attempt {attempt}", case.name);
                assert_eq!(
                    response.status_code(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "{context}: {}",
                    response.text()
                );
                assert!(
                    response.text().contains(CODESTREAM_MISMATCH),
                    "{context}: {}",
                    response.text()
                );
                let limit = REFUSAL_HEAP + 2 * length;
                assert!(
                    held <= limit,
                    "{context}: {held} bytes of heap held for a {length}-byte file, the limit is {limit}"
                );
                report(|| format!("{context}: {held} bytes of heap, file {length}"));
            }
        }
    }
}

/// An honest JPEG XL frame decodes inside the allowance its entry gives the
/// decoder, which is what bounds a frame whose image header agrees but
/// whose contents ask for more.
#[test]
fn a_jpeg_xl_frame_decodes_within_its_entry_s_allowance() {
    let _alone = heap::alone();
    let runtime = runtime();
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("large.dcm");
    files::write(
        &path,
        files::JPEG_XL,
        Layout::gray8(2048, 2048),
        &[hex(files::JXL_GRAY8_2048X2048)],
    );
    let listed = runtime.block_on(files::list(&[path]));
    let allowance = jxl_decode_limit(&listed[0]);
    assert_eq!(allowance, 4 * MIB + 64 * 2048 * 2048);
    let server = files::serve(listed);
    let (response, held) = heap::peak_during(|| {
        runtime.block_on(async { server.get("/api/file/0/frame/0/raw").await })
    });
    response.assert_status_ok();
    assert_eq!(response.as_bytes().len(), 2048 * 2048);
    report(|| format!("jxl 2048 x 2048: {held} bytes of heap, allowance {allowance}"));
    assert!(
        held <= allowance,
        "{held} bytes of heap held, the allowance is {allowance}"
    );
}
