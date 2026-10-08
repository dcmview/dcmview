//! Honest, hostile and damaged files, each decoded with the catalog entry
//! the production loader made for it, or for the file it replaced.

use super::heap;
use super::raster_cases::valid_files;
use super::raster_files::{self as files, CountedFile, TiffPage, TiffValue};
use dcmview::loader::{self, DiscoverOptions, FormatSelection};
use dcmview::pixels::{
    self, PixelError, PixelResult, RasterFrame, RASTER_JPEG_MAX_SCANS, RASTER_READ_BUFFER_BYTES,
    RASTER_TIFF_MAX_CHUNKS,
};
use dcmview::types::FileEntry;
use image::ExtendedColorType;
use std::fs;
use tempfile::tempdir;
use tokio::sync::mpsc;

pub(super) const BUFFER: u64 = RASTER_READ_BUFFER_BYTES as u64;
pub(super) const MIB: u64 = 1024 * 1024;

/// What one decode cost.
pub(super) struct Cost {
    pub result: PixelResult<RasterFrame>,
    pub reads: u64,
    pub bytes: u64,
    /// The most the decoding thread held at once, over what it held before.
    pub heap: u64,
}

pub(super) fn decode(entry: &FileEntry, frame: u32, mut source: CountedFile) -> Cost {
    let length = source.length();
    let (result, heap) =
        heap::peak_during(|| pixels::decode_raster_frame(entry, frame, &mut source, length));
    Cost {
        result,
        reads: source.reads,
        bytes: source.bytes_read,
        heap,
    }
}

/// The limits every decode keeps, whatever its outcome: the read budget and
/// the heap limit of the entry, and a read buffer that is actually used.
pub(super) fn assert_within_limits(context: &str, entry: &FileEntry, length: u64, cost: &Cost) {
    let budget = pixels::raster_read_budget(entry).expect("entry within the pixel limit");
    let heap_limit =
        pixels::raster_decode_heap_limit(entry, length).expect("entry within the pixel limit");
    assert!(
        cost.bytes <= budget,
        "{context}: {} bytes read, the budget is {budget}",
        cost.bytes
    );
    assert!(
        cost.heap <= heap_limit,
        "{context}: {} bytes of heap held, the limit is {heap_limit}",
        cost.heap
    );
    if let Ok(frame) = &cost.result {
        assert_eq!(
            Some(frame.bytes.len() as u64),
            pixels::raster_frame_bytes(entry),
            "{context}: a decoded frame has the entry's size"
        );
    }
}

/// The catalog entries the production loader makes for a set of files.
pub(super) struct Listed {
    entries: Vec<FileEntry>,
    // The entries name files in this directory.
    _dir: tempfile::TempDir,
}

impl Listed {
    pub(super) fn entry(&self, name: &str) -> &FileEntry {
        self.entries
            .iter()
            .find(|entry| entry.path.file_name().and_then(|name| name.to_str()) == Some(name))
            .unwrap_or_else(|| panic!("{name} was not listed"))
    }
}

/// Writes `files` into a directory and runs the production loader over it.
pub(super) async fn list(files: &[(&str, &[u8])]) -> Listed {
    let dir = tempdir().expect("temp dir");
    for (name, bytes) in files {
        fs::write(dir.path().join(name), bytes).expect("write raster");
    }
    let paths = [dir.path().to_path_buf()];
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let discover = loader::discover_progressive(
        &paths,
        DiscoverOptions {
            recursive: true,
            filters: Vec::new(),
            formats: FormatSelection::all(),
        },
        events_tx,
        loader::DiscoveryCancellation::new(),
    );
    let collect = async {
        let mut entries = Vec::new();
        while let Some(event) = events_rx.recv().await {
            if let loader::DiscoveryEvent::Selected { file, .. } = event {
                entries.push(*file);
            }
        }
        entries
    };
    let (report, entries) = tokio::join!(discover, collect);
    report.expect("discovery completes");
    assert_eq!(entries.len(), files.len(), "every file is listed");
    Listed { entries, _dir: dir }
}

// ---------------------------------------------------------------------------
// Honest files

/// A deterministic stream of bytes that does not compress.
pub(super) fn noise(count: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}

/// A file read front to back costs one read per buffer, and a TIFF frame is
/// read from its own page: the last of two thousand costs what the first
/// does.
#[tokio::test]
async fn a_decode_reads_its_file_once_through_a_buffer_and_a_tiff_frame_only_its_page() {
    const PAGES: usize = 2_000;
    let pixels = noise(512 * 512 * 3);
    let page = |index: usize| -> Vec<u8> {
        (0..64 * 64)
            .map(|pixel| ((index + pixel) % 251) as u8)
            .collect()
    };
    let stack = files::tiff_file(
        false,
        &(0..PAGES)
            .map(|index| TiffPage::strip((64, 64), &[8], 1, page(index)))
            .collect::<Vec<_>>(),
    )
    .0;
    // 512 x 512 in 64 tiles of 64 x 64, each deflated on its own.
    let tiles: Vec<Vec<u8>> = (0..64).map(|_| files::zlib(&noise(64 * 64))).collect();
    let tiled = files::tiff_file(
        false,
        &[TiffPage {
            tags: vec![
                (256, TiffValue::Long(vec![512])),
                (257, TiffValue::Long(vec![512])),
                (258, TiffValue::Short(vec![8])),
                (259, TiffValue::Short(vec![8])),
                (262, TiffValue::Short(vec![1])),
                (277, TiffValue::Short(vec![1])),
                (322, TiffValue::Long(vec![64])),
                (323, TiffValue::Long(vec![64])),
            ],
            chunks: tiles,
            tiled: true,
        }],
    )
    .0;
    let sequential: [(&str, Vec<u8>); 3] = [
        (
            "noise.png",
            files::png_of(
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                (512, 512),
                &pixels,
                |_| {},
            ),
        ),
        (
            "noise.jpg",
            files::baseline_jpeg(ExtendedColorType::Rgb8, (512, 512), &pixels),
        ),
        (
            "noise.webp",
            files::lossless_webp(ExtendedColorType::Rgb8, (512, 512), &pixels),
        ),
    ];
    let mut listed: Vec<(&str, &[u8])> = sequential
        .iter()
        .map(|(name, bytes)| (*name, bytes.as_slice()))
        .collect();
    listed.push(("stack.tif", &stack));
    listed.push(("tiled.tif", &tiled));
    let scan = list(&listed).await;

    for (name, bytes) in &sequential {
        let entry = scan.entry(name);
        let length = bytes.len() as u64;
        assert!(length > 4 * BUFFER, "{name} spans several buffers");
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert!(cost.result.is_ok(), "{name}: {:?}", cost.result.err());
        assert_within_limits(name, entry, length, &cost);
        // The WebP decoder goes back to its image chunk after reading the
        // chunk headers, which reads its first buffers once more.
        let reread = if name.ends_with(".webp") { 2 } else { 0 };
        assert!(
            cost.bytes <= length + reread * BUFFER,
            "{name}: {} bytes read of {length}",
            cost.bytes
        );
        assert!(
            cost.reads <= length / BUFFER + 2 + reread,
            "{name}: {} reads for {length} bytes",
            cost.reads
        );
    }

    let entry = scan.entry("stack.tif");
    assert_eq!(entry.frame_count as usize, PAGES);
    for frame in [0, PAGES / 2, PAGES - 1] {
        let cost = decode(entry, frame as u32, CountedFile::new(stack.clone()));
        let decoded = cost.result.as_ref().expect("the frame decodes");
        assert_eq!(decoded.bytes, page(frame), "frame {frame}");
        assert_within_limits("stack.tif", entry, stack.len() as u64, &cost);
        // Four reads to find the page, and one for its strip.
        assert!(
            cost.reads <= 5 && cost.bytes <= 5 * BUFFER,
            "frame {frame} of {PAGES}: {} reads, {} bytes",
            cost.reads,
            cost.bytes
        );
    }

    let entry = scan.entry("tiled.tif");
    let length = tiled.len() as u64;
    let cost = decode(entry, 0, CountedFile::new(tiled.clone()));
    assert!(cost.result.is_ok(), "tiled.tif: {:?}", cost.result.err());
    assert_within_limits("tiled.tif", entry, length, &cost);
    // The tiles lie one after another, so going from one to the next costs
    // no read: far fewer reads than the page has tiles.
    assert!(
        cost.bytes <= length + 4 * BUFFER && cost.reads <= length / BUFFER + 8,
        "tiled.tif: {} reads, {} bytes of {length}",
        cost.reads,
        cost.bytes
    );
}

// ---------------------------------------------------------------------------
// Hostile files

pub(super) enum Outcome {
    /// Refused before anything is read, with this `raster.*` reason.
    Refused(&'static str),
    /// A decode error.
    Fails,
    /// A frame of the entry's size.
    Decodes,
}

pub(super) struct Hostile {
    name: &'static str,
    /// What discovery saw: the file the catalog entry describes.
    listed: Vec<u8>,
    /// What the decoder is given, when the file is no longer the one that
    /// was listed.
    replaced: Option<CountedFile>,
    frame: u32,
    outcome: Outcome,
    /// The most bytes the decode may read, when the case pins less than the
    /// read budget.
    most_bytes: Option<u64>,
    /// The most heap the decode may hold, when the case pins less than the
    /// heap limit.
    most_heap: Option<u64>,
}

impl Hostile {
    pub(super) fn new(name: &'static str, listed: Vec<u8>, outcome: Outcome) -> Self {
        Self {
            name,
            listed,
            replaced: None,
            frame: 0,
            outcome,
            most_bytes: None,
            most_heap: None,
        }
    }

    pub(super) fn replaced_by(mut self, bytes: Vec<u8>) -> Self {
        self.replaced = Some(CountedFile::new(bytes));
        self
    }

    pub(super) fn reading_at_most(mut self, bytes: u64) -> Self {
        self.most_bytes = Some(bytes);
        self
    }

    pub(super) fn holding_at_most(mut self, bytes: u64) -> Self {
        self.most_heap = Some(bytes);
        self
    }
}

fn gray_png(width: u32, height: u32) -> Vec<u8> {
    files::png_of(
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        (width, height),
        &vec![7; (width * height) as usize],
        |_| {},
    )
}

/// `jpeg` with the scan that refines its DC coefficients repeated until the
/// file has `scans` scans. Refining a bit twice sets it twice, so the image
/// is the same and only the number of passes grows.
fn jpeg_with_scans(jpeg: &[u8], scans: usize) -> Vec<u8> {
    let ranges = files::jpeg_scans(jpeg);
    let refinement = ranges
        .iter()
        .find(|range| {
            // Spectral selection 0..0 with a successive-approximation high
            // bit: the last bytes of the scan header.
            let header_end = range.start
                + 2
                + usize::from(u16::from_be_bytes([
                    jpeg[range.start + 2],
                    jpeg[range.start + 3],
                ]));
            jpeg[header_end - 3..header_end - 1] == [0, 0] && jpeg[header_end - 1] >> 4 != 0
        })
        .expect("a DC refinement scan")
        .clone();
    let mut out = jpeg[..refinement.end].to_vec();
    for _ in ranges.len()..scans {
        out.extend_from_slice(&jpeg[refinement.clone()]);
    }
    out.extend_from_slice(&jpeg[refinement.end..]);
    assert_eq!(files::jpeg_scans(&out).len(), scans);
    out
}

fn hostile_files() -> Vec<Hostile> {
    use Outcome::{Decodes, Fails, Refused};
    let small_png = gray_png(4, 4);
    let rgb: Vec<u8> = (0..8 * 8 * 3).map(|index| index as u8).collect();
    let small_jpeg = files::baseline_jpeg(ExtendedColorType::L8, (16, 8), &[90; 16 * 8]);
    let small_webp = files::lossless_webp(ExtendedColorType::Rgb8, (8, 8), &rgb);
    let small_tiff = |compression: u16, strip: Vec<u8>| {
        TiffPage::strip((8, 8), &[8], 1, strip).with(259, TiffValue::Short(vec![compression]))
    };
    let plain_tiff = files::tiff_file(false, &[small_tiff(1, vec![5; 64])]).0;
    // 64 MiB of zeros, deflated: a few hundred kilobytes.
    let bomb = files::zlib(&vec![0; 64 * MIB as usize]);

    // A PNG whose ancillary chunks inflate to far more than the image.
    let mut profile = b"profile\0\0".to_vec();
    profile.extend_from_slice(&bomb);
    let mut text = b"Comment\0\0".to_vec();
    text.extend_from_slice(&bomb);
    let png_bombs = files::png_from_chunks(
        (8, 8),
        8,
        2,
        false,
        &[
            files::png_chunk(b"iCCP", &profile),
            files::png_chunk(b"zTXt", &text),
            files::png_idat(&rgb, 24),
        ],
    );
    // Image data that inflates to far more than the image.
    let png_long_idat =
        files::png_from_chunks((8, 8), 8, 0, false, &[files::png_chunk(b"IDAT", &bomb)]);
    // Two palette entries, and a pixel that names the fourth.
    let png_bad_index = files::png_from_chunks(
        (4, 1),
        2,
        3,
        false,
        &[
            files::png_chunk(b"PLTE", &[0, 0, 0, 255, 255, 255]),
            files::png_idat(&[0b00_01_11_00], 1),
        ],
    );
    let mut png_cut = gray_png(64, 64);
    png_cut.truncate(png_cut.len() * 3 / 5);

    // A JPEG whose frame header now declares 60,000 x 4,000.
    let mut jpeg_resized = small_jpeg.clone();
    let marker = files::jpeg_frame_marker(&jpeg_resized);
    jpeg_resized[marker + 4..marker + 8].copy_from_slice(&[0x0f, 0xa0, 0xea, 0x60]);
    // Six megabytes of profile in APP2 segments: more than a frame carries.
    let oversized = files::icc_profile(b"RGB ", 6 * MIB as usize, 0x33);
    let segments: Vec<&[u8]> = oversized.chunks(65_000).collect();
    let mut jpeg_profile = small_jpeg[..2].to_vec();
    for (index, part) in segments.iter().enumerate() {
        jpeg_profile.extend_from_slice(&[0xff, 0xe2]);
        jpeg_profile.extend_from_slice(&(part.len() as u16 + 16).to_be_bytes());
        jpeg_profile.extend_from_slice(b"ICC_PROFILE\0");
        jpeg_profile.extend_from_slice(&[index as u8 + 1, segments.len() as u8]);
        jpeg_profile.extend_from_slice(part);
    }
    jpeg_profile.extend_from_slice(&small_jpeg[2..]);
    let progressive = files::progressive_jpeg();

    // An extended WebP whose canvas says 2 x 2 over an 8 x 8 image.
    let mut webp_canvas = files::vp8x(0, (2, 2));
    webp_canvas.extend_from_slice(&small_webp[12..]);
    // A profile chunk that claims four gigabytes, at the end of the file.
    let mut webp_claim = files::vp8x(0x20, (8, 8));
    webp_claim.extend_from_slice(b"ICCP\xf0\xff\xff\xff");
    webp_claim.extend_from_slice(&small_webp[12..]);
    // An animation whose first frame is larger than its canvas.
    let mut webp_frame = files::vp8x(0x02, (8, 8));
    webp_frame.extend(files::riff_chunk(b"ANIM", &[0; 6]));
    let mut frame = vec![0; 6];
    frame.extend_from_slice(&63_u32.to_le_bytes()[..3]);
    frame.extend_from_slice(&63_u32.to_le_bytes()[..3]);
    frame.extend_from_slice(&[100, 0, 0, 0]);
    frame.extend_from_slice(&small_webp[12..]);
    webp_frame.extend(files::riff_chunk(b"ANMF", &frame));

    // A second page with the first one's layout, compressed as a JPEG that
    // declares 65,535 x 65,535.
    let mut huge_jpeg = small_jpeg.clone();
    huge_jpeg[marker + 4..marker + 8].copy_from_slice(&[0xff; 4]);
    let (jpeg_page, _) = files::tiff_file(
        false,
        &[small_tiff(1, vec![5; 64]), small_tiff(7, huge_jpeg)],
    );
    // A strip that claims to be four gigabytes of deflated data.
    let tiff_claim = files::tiff_file(
        false,
        &[small_tiff(8, files::zlib(&[5; 64]))
            .with(279, TiffValue::Raw(4, 1, [0xf0, 0xff, 0xff, 0xff]))],
    )
    .0;
    // A strip that starts past the end of the file.
    let tiff_far = files::tiff_file(
        false,
        &[small_tiff(1, vec![5; 64]).with(273, TiffValue::Long(vec![0x7fff_0000]))],
    )
    .0;
    // A deflated strip that is only the start of its stream.
    let whole = files::zlib(&noise(64));
    let tiff_cut = files::tiff_file(false, &[small_tiff(8, whole[..whole.len() / 2].to_vec())]).0;
    // Tags that declare a billion values at offset 8.
    let counted = |tag: u16, kind: u16| {
        files::tiff_file(
            false,
            &[small_tiff(1, vec![5; 64])
                .with(tag, TiffValue::Raw(kind, 0x4000_0000, [8, 0, 0, 0]))],
        )
        .0
    };
    // One pixel wide, one row to a strip.
    let strips = |rows: usize| {
        files::tiff_file(
            false,
            &[TiffPage {
                tags: vec![
                    (256, TiffValue::Long(vec![1])),
                    (257, TiffValue::Long(vec![rows as u32])),
                    (258, TiffValue::Short(vec![8])),
                    (259, TiffValue::Short(vec![1])),
                    (262, TiffValue::Short(vec![1])),
                    (277, TiffValue::Short(vec![1])),
                    (278, TiffValue::Long(vec![1])),
                ],
                chunks: vec![vec![5]; rows],
                tiled: false,
            }],
        )
        .0
    };
    // Tiles two gigabytes wide, and strips of no rows.
    let tiff_tiles = files::tiff_file(
        false,
        &[TiffPage {
            tags: vec![
                (256, TiffValue::Long(vec![8])),
                (257, TiffValue::Long(vec![8])),
                (258, TiffValue::Short(vec![8])),
                (259, TiffValue::Short(vec![1])),
                (262, TiffValue::Short(vec![1])),
                (277, TiffValue::Short(vec![1])),
                (322, TiffValue::Long(vec![0x8000_0000])),
                (323, TiffValue::Long(vec![0x8000_0000])),
            ],
            chunks: vec![vec![5; 64]],
            tiled: true,
        }],
    )
    .0;
    let tiff_no_rows = files::tiff_file(
        false,
        &[small_tiff(1, vec![5; 64]).with(278, TiffValue::Long(vec![0]))],
    )
    .0;
    // The page that was listed is now 16,000 x 16,000, in one strip.
    let tiff_resized = files::tiff_file(
        false,
        &[small_tiff(1, vec![5; 64])
            .with(256, TiffValue::Long(vec![16_000]))
            .with(257, TiffValue::Long(vec![16_000]))
            .with(278, TiffValue::Long(vec![16_000]))],
    )
    .0;

    let budget_of_small = 64 * MIB + 4 * 64 * 3;
    let longer_than_budget =
        |bytes: &[u8]| CountedFile::with_tail(bytes.to_vec(), &[0], budget_of_small + MIB);

    vec![
        // More pixels than a frame may have: nothing is read.
        Hostile::new(
            "png of 400 megapixels",
            files::png_from_chunks(
                (20_000, 20_000),
                8,
                0,
                false,
                &[files::png_chunk(b"IDAT", &[])],
            ),
            Refused("raster.too_large"),
        )
        .reading_at_most(0),
        // Ancillary data that inflates far past the image is not inflated
        // into memory: the frame decodes, without the profile.
        Hostile::new("png with inflating profile and text", png_bombs, Decodes),
        Hostile::new("png with inflating image data", png_long_idat, Decodes),
        Hostile::new("png cut short", png_cut, Fails),
        Hostile::new("png index beyond its palette", png_bad_index, Fails),
        // A file that grew after it was listed is not read at all, and one
        // that now declares another size is not believed.
        Hostile {
            replaced: Some(longer_than_budget(&small_png)),
            ..Hostile::new("png longer than its budget", small_png.clone(), Fails)
        }
        .reading_at_most(0),
        Hostile::new("png replaced by a larger image", small_png.clone(), Fails).replaced_by(
            files::png_from_chunks(
                (16_000, 16_000),
                8,
                0,
                false,
                &[files::png_idat(&[1, 2, 3, 4], 4)],
            ),
        ),
        Hostile {
            replaced: Some(longer_than_budget(&small_jpeg)),
            ..Hostile::new("jpeg longer than its budget", small_jpeg.clone(), Fails)
        }
        .reading_at_most(0),
        Hostile::new("jpeg replaced by a larger image", small_jpeg.clone(), Fails)
            .replaced_by(jpeg_resized),
        Hostile::new("jpeg replaced by a colour image", small_jpeg.clone(), Fails).replaced_by(
            files::baseline_jpeg(ExtendedColorType::Rgb8, (16, 8), &[90; 16 * 8 * 3]),
        ),
        Hostile::new("jpeg with six megabytes of profile", jpeg_profile, Decodes),
        // The linked decoder panics on this frame header; a decode does not.
        Hostile::new(
            "jpeg its decoder panics on",
            files::jpeg_its_decoder_panics_on(),
            Fails,
        ),
        // Scans up to the limit decode; one more does not.
        Hostile::new(
            "jpeg at the scan limit",
            jpeg_with_scans(&progressive, RASTER_JPEG_MAX_SCANS),
            Decodes,
        ),
        Hostile::new(
            "jpeg over the scan limit",
            jpeg_with_scans(&progressive, RASTER_JPEG_MAX_SCANS + 1),
            Fails,
        ),
        Hostile {
            replaced: Some(longer_than_budget(&small_webp)),
            ..Hostile::new("webp longer than its budget", small_webp.clone(), Fails)
        }
        .reading_at_most(0),
        // Discovery takes the canvas and its flags on trust; a decode does
        // not.
        Hostile::new(
            "webp canvas smaller than its image",
            files::webp_from_chunks(&webp_canvas),
            Fails,
        ),
        Hostile::new("webp chunk of four gigabytes", small_webp.clone(), Fails)
            .replaced_by(files::webp_from_chunks(&webp_claim)),
        Hostile::new(
            "webp frame larger than its canvas",
            small_webp.clone(),
            Fails,
        )
        .replaced_by(files::webp_from_chunks(&webp_frame)),
        // A later page need not share page 0's compression, and its own is
        // checked before its strips are touched.
        Hostile {
            frame: 1,
            ..Hostile::new(
                "tiff frame compressed as jpeg",
                jpeg_page,
                Refused("raster.unsupported_compression"),
            )
        }
        .reading_at_most(2 * BUFFER),
        Hostile::new("tiff strip of four gigabytes", tiff_claim, Fails),
        Hostile::new("tiff strip past the end", tiff_far, Fails),
        Hostile::new("tiff strip cut short", tiff_cut, Fails),
        Hostile::new("tiff bit depths by the billion", plain_tiff.clone(), Fails)
            .replaced_by(counted(258, 3)),
        Hostile::new(
            "tiff strip offsets by the billion",
            plain_tiff.clone(),
            Fails,
        )
        .replaced_by(counted(273, 4)),
        Hostile::new(
            "tiff strip offsets as a billion bytes",
            plain_tiff.clone(),
            Fails,
        )
        .replaced_by(counted(273, 1)),
        // As many strips as a page may have decode; one more does not.
        Hostile::new(
            "tiff at the strip limit",
            strips(RASTER_TIFF_MAX_CHUNKS),
            Decodes,
        ),
        Hostile::new(
            "tiff over the strip limit",
            strips(RASTER_TIFF_MAX_CHUNKS + 1),
            Fails,
        ),
        Hostile::new("tiff tiles two gigabytes wide", tiff_tiles, Fails),
        Hostile::new("tiff strips of no rows", tiff_no_rows, Fails),
        Hostile::new(
            "tiff page replaced by a larger one",
            plain_tiff.clone(),
            Fails,
        )
        .replaced_by(tiff_resized),
        Hostile::new("tiff cut before its page", plain_tiff.clone(), Fails)
            .replaced_by(plain_tiff[..plain_tiff.len() - 40].to_vec()),
    ]
}

/// Whatever a file declares, decoding it reads no more than the entry's
/// budget and holds no more heap than the entry's limit, and a declared
/// size, length or count is never what an allocation is sized by.
#[tokio::test]
async fn a_hostile_file_costs_no_more_than_its_entry_allows() {
    assert_hostile(hostile_files()).await;
}

/// Lists what discovery saw of each case, decodes what the case presents,
/// and checks the outcome and the limits. A case is named by its format
/// first ("tiff strip past the end").
pub(super) async fn assert_hostile(cases: Vec<Hostile>) {
    let names: Vec<String> = (0..cases.len())
        .map(|index| {
            let format = cases[index].name.split(' ').next().expect("format");
            format!("{index}.{format}")
        })
        .collect();
    let listed: Vec<(&str, &[u8])> = names
        .iter()
        .zip(&cases)
        .map(|(file, case)| (file.as_str(), case.listed.as_slice()))
        .collect();
    let scan = list(&listed).await;

    for (file, case) in names.iter().zip(cases) {
        let name = case.name;
        let entry = scan.entry(file);
        let source = case
            .replaced
            .unwrap_or_else(|| CountedFile::new(case.listed.clone()));
        let length = source.length();
        let cost = decode(entry, case.frame, source);

        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!(
                "{name}: {} reads, {} bytes, heap {}, {}",
                cost.reads,
                cost.bytes,
                cost.heap,
                match &cost.result {
                    Ok(_) => "decoded".to_string(),
                    Err(error) => format!("{error:#}"),
                }
            );
        }
        match (&cost.result, &case.outcome) {
            (Err(PixelError::UnsupportedLayout(reason)), Outcome::Refused(expected)) => {
                assert_eq!(reason, expected, "{name}")
            }
            (Err(PixelError::Decode { .. }), Outcome::Fails) => {}
            (Ok(_), Outcome::Decodes) => {}
            (result, _) => panic!(
                "{name}: unexpected {}",
                match result {
                    Ok(_) => "frame".to_string(),
                    Err(error) => format!("error: {error}"),
                }
            ),
        }
        if matches!(case.outcome, Outcome::Refused(_)) {
            // Past the pixel limit there is no budget to measure against.
            assert!(cost.heap <= MIB, "{name}: {} bytes of heap", cost.heap);
        } else {
            assert_within_limits(name, entry, length, &cost);
        }
        if let Some(most) = case.most_bytes {
            assert!(
                cost.bytes <= most,
                "{name}: {} bytes read, at most {most} expected",
                cost.bytes
            );
        }
        if let Some(most) = case.most_heap {
            assert!(
                cost.heap <= most,
                "{name}: {} bytes of heap held, at most {most} expected",
                cost.heap
            );
        }
    }
}

/// A profile that is too large or is not for RGB is left out; the frame is
/// still decoded.
#[tokio::test]
async fn an_unusable_profile_is_dropped_and_the_frame_kept() {
    let small_jpeg = files::baseline_jpeg(ExtendedColorType::Rgb8, (8, 8), &[90; 8 * 8 * 3]);
    let with_profile = |profile: &[u8]| {
        let parts: Vec<&[u8]> = profile.chunks(65_000).collect();
        let mut out = small_jpeg[..2].to_vec();
        for (index, part) in parts.iter().enumerate() {
            out.extend_from_slice(&[0xff, 0xe2]);
            out.extend_from_slice(&(part.len() as u16 + 16).to_be_bytes());
            out.extend_from_slice(b"ICC_PROFILE\0");
            out.extend_from_slice(&[index as u8 + 1, parts.len() as u8]);
            out.extend_from_slice(part);
        }
        out.extend_from_slice(&small_jpeg[2..]);
        out
    };
    let largest = files::icc_profile(b"RGB ", pixels::RASTER_ICC_MAX_BYTES, 0x33);
    let too_large = files::icc_profile(b"RGB ", pixels::RASTER_ICC_MAX_BYTES + 1, 0x33);
    let cases = [
        ("largest.jpg", with_profile(&largest), Some(largest)),
        ("too-large.jpg", with_profile(&too_large), None),
    ];
    let listed: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(name, bytes, _)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;

    for (name, bytes, profile) in &cases {
        let entry = scan.entry(name);
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert_within_limits(name, entry, bytes.len() as u64, &cost);
        let frame = cost.result.expect("the frame decodes");
        assert_eq!(
            frame.icc_profile.as_ref().map(Vec::len),
            profile.as_ref().map(Vec::len),
            "{name}"
        );
    }
}

/// A TIFF strip whose deflate stream never ends is read to the end of the
/// bytes its byte count gives it, or to the budget when it claims more, and
/// no further, however long the file is.
#[tokio::test]
async fn an_endless_compressed_strip_is_read_to_its_end_or_the_budget_and_no_further() {
    // The budget of an 8 x 8 page of one byte a sample.
    let budget = 64 * MIB + 4 * 64;
    // (the strip's byte count, the most bytes the decode may read)
    for (claimed, most) in [(1024, 4 * BUFFER), (budget, budget)] {
        // A zlib header, then stored blocks of no bytes for as long as the
        // file goes on: input that never yields a sample.
        let page = TiffPage::strip((8, 8), &[8], 1, vec![0x78, 0x01])
            .with(259, TiffValue::Short(vec![8]))
            .with(273, TiffValue::Long(vec![0x1000]))
            .with(279, TiffValue::Long(vec![claimed as u32]));
        let mut bytes = files::tiff_file(false, &[page]).0;
        bytes.resize(0x1000, 0);
        bytes.extend_from_slice(&[0x78, 0x01]);
        let scan = list(&[("endless.tif", &bytes)]).await;
        let entry = scan.entry("endless.tif");
        assert_eq!(pixels::raster_read_budget(entry), Some(budget));

        let length = 4 * budget;
        let source = CountedFile::with_tail(bytes, &[0x00, 0x00, 0x00, 0xff, 0xff], length);
        let cost = decode(entry, 0, source);

        assert!(matches!(cost.result, Err(PixelError::Decode { .. })));
        assert_within_limits("endless.tif", entry, length, &cost);
        assert!(
            cost.bytes <= most && cost.reads <= most / BUFFER + 4,
            "a strip of {claimed} bytes: {} reads, {} bytes",
            cost.reads,
            cost.bytes
        );
    }
}

/// Every valid file of the decode tests, cut short and with single bytes
/// changed: a decode returns a frame of the right size or an error, within
/// its limits, and never panics.
#[tokio::test]
async fn damaged_files_fail_or_decode_within_the_same_limits() {
    let valid = valid_files();
    let listed: Vec<(&str, &[u8])> = valid
        .iter()
        .map(|(name, bytes)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;

    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |below: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize % below
    };
    for (name, bytes) in &valid {
        let entry = scan.entry(name);
        let mut damaged: Vec<Vec<u8>> = (1..8)
            .map(|eighths| bytes[..bytes.len() * eighths / 8].to_vec())
            .collect();
        for _ in 0..24 {
            let mut changed = bytes.clone();
            let at = next(changed.len());
            changed[at] ^= 1 << next(8);
            damaged.push(changed);
        }
        for (index, file) in damaged.into_iter().enumerate() {
            let context = format!("{name}, damage {index}");
            let length = file.len() as u64;
            let cost = decode(entry, 0, CountedFile::new(file));
            assert_within_limits(&context, entry, length, &cost);
            // Half of a lossless file of one image has no frame to give.
            // (More of it can: the end of a PNG is an empty chunk.)
            let halved = index < 4;
            let lossless = name.ends_with(".png") || name.ends_with(".tif");
            let one_image = entry.frame_count == 1
                && entry.raster.as_ref().is_some_and(|raster| !raster.animated);
            if halved && lossless && one_image {
                assert!(cost.result.is_err(), "{context}: a frame from a cut file");
            }
        }
    }
}
