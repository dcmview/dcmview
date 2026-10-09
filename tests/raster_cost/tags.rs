//! The metadata tree of a raster file (`pixels::read_raster_tags`): what it
//! shows for files written with known values, how it shows text nobody
//! vouches for, and what reading it may cost whatever the file declares.
//!
//! Every case reads from a counted source on the counting allocator, so the
//! limits are asserted in reads, bytes, nodes and heap, never in time. What
//! a masked session shows of these trees, and how the endpoints serve them,
//! is in `tests/integration/raster_tags.rs`.

use super::heap;
use super::raster_files::{
    self as files, png_chunk, png_from_chunks, riff_chunk, vp8x, webp_from_chunks, CountedFile,
};
use super::raster_tag_files::{
    self as tagged, icc_with_description, icc_with_unicode_description, jpeg_exif, jpeg_profile,
    jpeg_segment, jpeg_with_segments, jpeg_xmp, png_compressed_text, png_international_text,
    png_profile, png_text, png_with_chunks, rgb_page_samples, rgb_page_tags, rows, tiff_block,
    webp_with, Block, Dir, Layout, V,
};
use dcmview::api::contracts::TagNode;
use dcmview::pixels::{
    read_raster_tags, RasterTagLimit as Limit, RasterTagNote as Note, RasterTagPart as Part,
    RasterTagTree, RASTER_TAGS_HEAP_LIMIT_BYTES, RASTER_TAGS_MAX_DEPTH,
    RASTER_TAGS_MAX_IFD_ENTRIES, RASTER_TAGS_MAX_NODES, RASTER_TAGS_MAX_PAGES,
    RASTER_TAGS_MAX_READS, RASTER_TAGS_MAX_TEXT_BYTES, RASTER_TAGS_READ_BUDGET_BYTES,
    RASTER_TAG_NUMBERS_MAX, RASTER_TAG_TEXT_MAX_CHARS,
};
use dcmview::types::FileFormat;
use serde_json::Value;

/// One read of a file's metadata and what it cost.
struct Read {
    tree: RasterTagTree,
    /// The tree as `raster_tag_files::rows` writes it.
    rows: Vec<String>,
    reads: u64,
    bytes: u64,
    /// The most the reading thread held at once, the tree included.
    heap: u64,
}

fn read_source(format: FileFormat, mut source: CountedFile) -> Read {
    let length = source.length();
    let (tree, heap) = heap::peak_during(|| read_raster_tags(format, &mut source, length));
    let rows = rows(&serde_json::to_value(&tree.nodes).expect("tree serializes"));
    Read {
        tree,
        rows,
        reads: source.reads,
        bytes: source.bytes_read,
        heap,
    }
}

fn read(format: FileFormat, bytes: &[u8]) -> Read {
    read_source(format, CountedFile::new(bytes.to_vec()))
}

/// The limits every read keeps, whatever the file: reads, bytes, nodes,
/// depth, text and heap, and no text of the file outside a value.
fn assert_within_limits(context: &str, read: &Read) {
    assert!(
        !read.tree.notes.contains(&Note::ReaderFailed),
        "{context}: the reader failed instead of reporting damage"
    );
    assert!(
        read.reads <= RASTER_TAGS_MAX_READS,
        "{context}: {} reads",
        read.reads
    );
    assert!(
        read.bytes <= RASTER_TAGS_READ_BUDGET_BYTES,
        "{context}: {} bytes read",
        read.bytes
    );
    assert!(
        read.heap <= RASTER_TAGS_HEAP_LIMIT_BYTES,
        "{context}: {} bytes of heap held",
        read.heap
    );
    let mut count = 0;
    let mut text = 0;
    fn visit(context: &str, nodes: &[TagNode], depth: usize, count: &mut usize, text: &mut usize) {
        use dcmview::api::contracts::TagValue;
        assert!(depth <= RASTER_TAGS_MAX_DEPTH, "{context}: depth {depth}");
        for node in nodes {
            *count += 1;
            // Names are dcmview's own: a file's bytes never reach them.
            for name in [&node.tag, &node.keyword, &node.vr] {
                assert!(
                    name.len() <= 32
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == ' '),
                    "{context}: node named {name:?}"
                );
            }
            match &node.value {
                TagValue::String { value } => {
                    *text += value.len();
                    assert!(
                        value.chars().count() <= RASTER_TAG_TEXT_MAX_CHARS + 1,
                        "{context}: {} characters in one value",
                        value.chars().count()
                    );
                    assert!(
                        !value.chars().any(|c| c.is_control()
                            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                                | '\u{2060}'..='\u{2069}' | '\u{feff}')),
                        "{context}: an unescaped character in {value:?}"
                    );
                }
                TagValue::Numbers { value, .. } => {
                    assert!(value.len() <= RASTER_TAG_NUMBERS_MAX, "{context}: numbers");
                    assert!(value.iter().all(|value| value.is_finite()), "{context}");
                }
                TagValue::Number { value } => assert!(value.is_finite(), "{context}"),
                TagValue::Sequence { items, .. } => {
                    assert_eq!(items.len(), 1, "{context}: a group has one item");
                    visit(context, &items[0], depth + 1, count, text);
                }
                TagValue::Binary { .. } | TagValue::Error { .. } => {}
            }
        }
    }
    visit(context, &read.tree.nodes, 1, &mut count, &mut text);
    assert!(count <= RASTER_TAGS_MAX_NODES, "{context}: {count} nodes");
    // Each cut value ends with a three-byte mark the limit does not count.
    assert!(
        text <= RASTER_TAGS_MAX_TEXT_BYTES + 3 * count,
        "{context}: {text} bytes of text"
    );
    if std::env::var_os("RASTER_COST_REPORT").is_some() {
        println!(
            "tags {context}: {} reads, {} bytes, {} heap, {count} nodes, {:?}",
            read.reads, read.bytes, read.heap, read.tree.notes
        );
    }
}

fn assert_rows(context: &str, read: &Read, expected: &[&str]) {
    assert_within_limits(context, read);
    let actual = read.rows.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(actual, expected, "{context}");
    assert_eq!(read.tree.notes, [], "{context}: a well-formed file");
}

/// The rows of `read` that start with `prefix`.
fn rows_under<'a>(read: &'a Read, prefix: &str) -> Vec<&'a str> {
    read.rows
        .iter()
        .map(String::as_str)
        .filter(|row| row.starts_with(prefix))
        .collect()
}

fn le16(value: u16) -> [u8; 2] {
    value.to_le_bytes()
}

// ---------------------------------------------------------------------------
// What the tree shows

/// An EXIF block of known values in every directory and of every type an
/// entry can have.
fn small_exif(big_endian: bool) -> Vec<u8> {
    tiff_block(&Block {
        big_endian,
        ifd0: vec![
            (271, V::ascii("Aperture Works")),
            (274, V::Short(vec![6])),
            (282, V::Rational(vec![(72, 1)])),
            (531, V::Short(vec![1])),
        ],
        exif: Some(vec![
            (33434, V::Rational(vec![(1, 250)])),
            (34855, V::Short(vec![400])),
            (36864, V::Undefined(b"0232".to_vec())),
            (37380, V::SRational(vec![(-2, 3)])),
            (37500, V::Undefined(vec![7; 20])),
            (40962, V::Long(vec![640])),
        ]),
        gps: Some(vec![
            (0, V::Byte(vec![2, 3, 0, 0])),
            (1, V::ascii("N")),
            (2, V::Rational(vec![(48, 1), (51, 1), (2987, 100)])),
        ]),
        interop: Some(vec![(1, V::ascii("R98"))]),
        next: vec![vec![
            (259, V::Short(vec![6])),
            (513, V::Long(vec![1000])),
            (514, V::Long(vec![0])),
        ]],
        ..Block::default()
    })
    .bytes
}

/// The rows of [`small_exif`], each directory under `EXIF`. The pointers
/// are where `tiff_block` lays the directories out, one after another from
/// offset 8.
const SMALL_EXIF_ROWS: &[&str] = &[
    "EXIF |  |  | {",
    "EXIF/IFD0 |  |  | {",
    "EXIF/IFD0/0x010F | Make | ASCII | \"Aperture Works\"",
    "EXIF/IFD0/0x0112 | Orientation | SHORT | 6",
    "EXIF/IFD0/0x011A | XResolution | RATIONAL | \"72/1\"",
    "EXIF/IFD0/0x0213 | YCbCrPositioning | SHORT | 1",
    "EXIF/IFD0/0x8769 | ExifIFD | LONG | 110",
    "EXIF/IFD0/0x8825 | GPSIFD | LONG | 236",
    "EXIF/Exif |  |  | {",
    "EXIF/Exif/0x829A | ExposureTime | RATIONAL | \"1/250\"",
    "EXIF/Exif/0x8827 | ISOSpeedRatings | SHORT | 400",
    "EXIF/Exif/0x9000 | ExifVersion | UNDEFINED | \"0232\"",
    "EXIF/Exif/0x9204 | ExposureBiasValue | SRATIONAL | \"-2/3\"",
    "EXIF/Exif/0x927C | MakerNote | UNDEFINED | <20 bytes>",
    "EXIF/Exif/0xA002 | PixelXDimension | LONG | 640",
    "EXIF/Exif/0xA005 | InteropIFD | LONG | 302",
    "EXIF/GPS |  |  | {",
    "EXIF/GPS/0x0000 | GPSVersionID | BYTE | [2, 3, 0, 0]",
    "EXIF/GPS/0x0001 | GPSLatitudeRef | ASCII | \"N\"",
    "EXIF/GPS/0x0002 | GPSLatitude | RATIONAL | \"48/1, 51/1, 2987/100\"",
    "EXIF/Interop |  |  | {",
    "EXIF/Interop/0x0001 | InteroperabilityIndex | ASCII | \"R98\"",
    "EXIF/IFD1 |  |  | {",
    "EXIF/IFD1/0x0103 | Compression | SHORT | 6",
    "EXIF/IFD1/0x0201 | JPEGInterchangeFormat | LONG | 1000",
    "EXIF/IFD1/0x0202 | JPEGInterchangeFormatLength | LONG | 0",
];

#[test]
fn a_png_shows_its_header_its_chunks_and_what_they_carry() {
    let profile = icc_with_description(b"mntr", "A test display");
    let before = vec![
        png_chunk(b"pHYs", &[0, 0, 0x0b, 0x13, 0, 0, 0x17, 0x26, 1]),
        png_chunk(b"gAMA", &45_455_u32.to_be_bytes()),
        png_chunk(
            b"cHRM",
            &[
                31_270_u32, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000,
            ]
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect::<Vec<_>>(),
        ),
        png_chunk(b"sRGB", &[1]),
        png_chunk(b"sBIT", &[5, 6, 5]),
        png_chunk(b"tIME", &[0x07, 0xe5, 3, 4, 5, 6, 7]),
        png_profile(b"Display", &profile),
        png_text(b"Title", b"A small square"),
        // Latin-1: the bytes E9 and FC are é and ü.
        png_compressed_text(b"Author", b"Ren\xe9 M\xfcller"),
        png_international_text(
            b"Description",
            b"fr",
            b"Description",
            "café — naïve".as_bytes(),
            false,
        ),
        png_international_text(b"Comment", b"", b"", "compressed ✓".as_bytes(), true),
        png_chunk(b"eXIf", &small_exif(false)),
        // A second pHYs is not the file's pixel size: only the first counts.
        png_chunk(b"pHYs", &[0, 0, 0, 1, 0, 0, 0, 1, 0]),
        png_chunk(b"prVt", b"a private chunk"),
    ];
    let after = vec![png_text(b"Software", b"after the image")];
    let read = read(FileFormat::Png, &png_with_chunks(&before, &after));

    let mut expected = vec![
        "PNG:IHDR | Width |  | 2",
        "PNG:IHDR | Height |  | 2",
        "PNG:IHDR | BitDepth |  | 8",
        "PNG:IHDR | ColorType |  | 2",
        "PNG:IHDR | Compression |  | 0",
        "PNG:IHDR | Filter |  | 0",
        "PNG:IHDR | Interlace |  | 0",
        "PNG:pHYs | PixelsPerUnitX |  | 2835",
        "PNG:pHYs | PixelsPerUnitY |  | 5926",
        "PNG:pHYs | Unit |  | 1",
        "PNG:gAMA | Gamma |  | 45455",
        "PNG:cHRM | Chromaticities |  | [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000]",
        "PNG:sRGB | RenderingIntent |  | 1",
        "PNG:sBIT | SignificantBits |  | [5, 6, 5]",
        "PNG:tIME | Time |  | \"2021-03-04 05:06:07\"",
        "PNG:iCCP | ProfileName |  | \"Display\"",
        "PNG:tEXt | Text |  | \"Title: A small square\"",
        "PNG:zTXt | Text |  | \"Author: René Müller\"",
        "PNG:iTXt | Text |  | \"Description: café — naïve\"",
        "PNG:iTXt | Text |  | \"Comment: compressed ✓\"",
        "PNG:tEXt | Text |  | \"Software: after the image\"",
    ];
    expected.extend_from_slice(SMALL_EXIF_ROWS);
    expected.extend_from_slice(&[
        "ICC | Size |  | 172",
        "ICC | Version |  | \"2.1.0\"",
        "ICC | Class |  | \"mntr\"",
        "ICC | ColorSpace |  | \"RGB\"",
        "ICC | Description |  | \"A test display\"",
    ]);
    assert_rows("PNG", &read, &expected);
}

#[test]
fn a_jpeg_shows_its_segments_up_to_the_first_scan() {
    let profile = icc_with_unicode_description(b"scnr", "Scanner — wide");
    let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><dc:title>one</dc:title></x:xmpmeta>";
    let segments = vec![
        jpeg_segment(0xe0, b"JFIF\0\x01\x02\x01\x01\x2c\0\x96\0\0"),
        jpeg_exif(&small_exif(true)),
        jpeg_xmp(xmp),
        // The profile in two segments: the tree reads the first.
        jpeg_profile(1, 2, &profile),
        jpeg_profile(2, 2, &[0; 64]),
        jpeg_segment(0xee, b"Adobe\0\x64\x80\0\0\0\x01"),
        jpeg_segment(0xfe, b"first comment"),
        jpeg_segment(0xec, b"Ducky and anything else is stepped over"),
        jpeg_segment(0xfe, "second — comment".as_bytes()),
    ];
    let read = read(FileFormat::Jpeg, &jpeg_with_segments(&segments));

    let mut expected = vec![
        "JPEG:JFIF | Version |  | \"1.02\"",
        "JPEG:JFIF | Units |  | 1",
        "JPEG:JFIF | XDensity |  | 300",
        "JPEG:JFIF | YDensity |  | 150",
        "JPEG:Adobe | Version |  | 100",
        "JPEG:Adobe | Transform |  | 1",
        "JPEG:COM | Comment |  | \"first comment\"",
        "JPEG:COM | Comment |  | \"second — comment\"",
        "JPEG:SOF0 | Precision |  | 8",
        "JPEG:SOF0 | Height |  | 8",
        "JPEG:SOF0 | Width |  | 8",
        "JPEG:SOF0 | Components |  | 3",
    ];
    expected.extend_from_slice(SMALL_EXIF_ROWS);
    expected.extend_from_slice(&[
        "XMP | Packet |  | \"<x:xmpmeta xmlns:x=\\\"adobe:ns:meta/\\\"><dc:title>one</dc:title></x:xmpmeta>\"",
        "ICC | Size |  | 200",
        "ICC | Version |  | \"4.3.0\"",
        "ICC | Class |  | \"scnr\"",
        "ICC | ColorSpace |  | \"RGB\"",
        "ICC | Description |  | \"Scanner — wide\"",
    ]);
    assert_rows("JPEG", &read, &expected);

    // A progressive file names its own frame header.
    let progressive = read_source(
        FileFormat::Jpeg,
        CountedFile::new(files::progressive_jpeg()),
    );
    assert_within_limits("progressive JPEG", &progressive);
    assert_eq!(
        rows_under(&progressive, "JPEG:SOF"),
        [
            "JPEG:SOF2 | Precision |  | 8",
            "JPEG:SOF2 | Height |  | 8",
            "JPEG:SOF2 | Width |  | 32",
            "JPEG:SOF2 | Components |  | 3",
        ]
    );
}

#[test]
fn a_webp_shows_its_header_and_the_blocks_it_carries() {
    let profile = icc_with_description(b"mntr", "A test display");
    let xmp = b"<x:xmpmeta/>";
    // The EXIF chunk may or may not start with the JPEG prefix.
    let mut prefixed = b"Exif\0\0".to_vec();
    prefixed.extend_from_slice(&small_exif(false));
    for (context, exif) in [("plain", small_exif(false)), ("prefixed", prefixed)] {
        let read = read(
            FileFormat::Webp,
            &webp_with(
                Some(&profile),
                Some(&exif),
                Some(xmp),
                &riff_chunk(b"zzzz", b"unknown"),
            ),
        );
        let mut expected = vec![
            "WEBP:VP8X | Flags |  | 44",
            "WEBP:VP8X | CanvasWidth |  | 8",
            "WEBP:VP8X | CanvasHeight |  | 8",
        ];
        expected.extend_from_slice(SMALL_EXIF_ROWS);
        expected.extend_from_slice(&[
            "XMP | Packet |  | \"<x:xmpmeta/>\"",
            "ICC | Size |  | 172",
            "ICC | Version |  | \"2.1.0\"",
            "ICC | Class |  | \"mntr\"",
            "ICC | ColorSpace |  | \"RGB\"",
            "ICC | Description |  | \"A test display\"",
        ]);
        assert_rows(&format!("extended WebP, {context} EXIF"), &read, &expected);
    }

    // Simple files state their size in the bitstream.
    let lossless = files::lossless_webp(image::ExtendedColorType::Rgb8, (5, 3), &[9; 5 * 3 * 3]);
    assert_rows(
        "lossless WebP",
        &read(FileFormat::Webp, &lossless),
        &["WEBP:VP8L | Width |  | 5", "WEBP:VP8L | Height |  | 3"],
    );
    assert_rows(
        "lossy WebP",
        &read(FileFormat::Webp, &files::lossy_webp()),
        &["WEBP:VP8 | Width |  | 16", "WEBP:VP8 | Height |  | 16"],
    );
    // An animation: its loop count, and nothing of its frames.
    let mut animation = vp8x(0x02, (8, 8));
    animation.extend(riff_chunk(b"ANIM", &[0, 0, 0, 0, 7, 0]));
    animation.extend(riff_chunk(b"ANMF", &[0; 24]));
    assert_rows(
        "animated WebP",
        &read(FileFormat::Webp, &webp_from_chunks(&animation)),
        &[
            "WEBP:VP8X | Flags |  | 2",
            "WEBP:VP8X | CanvasWidth |  | 8",
            "WEBP:VP8X | CanvasHeight |  | 8",
            "WEBP:ANIM | LoopCount |  | 7",
        ],
    );
}

/// A two-page TIFF with every kind of entry on its first page.
fn described_tiff(big_endian: bool) -> Vec<u8> {
    let profile = icc_with_description(b"mntr", "A test display");
    let mut ifd0 = rgb_page_tags();
    ifd0.extend([
        (270, V::ascii("a described page")),
        (282, V::Rational(vec![(300, 1)])),
        (297, V::Short(vec![0, 2])),
        (700, V::Byte(b"<x:xmpmeta/>".to_vec())),
        (34675, V::Undefined(profile)),
        (40093, V::utf16("Zoë")),
        // A type for each remaining width and sign, in tags no table names.
        (65000, V::Raw(6, 1, [0xfe, 0, 0, 0])),
        (65001, V::Double(vec![0.5, -2.25])),
        (65002, V::Undefined(vec![1, 2, 3])),
    ]);
    let mut page1 = rgb_page_tags();
    page1.push((285, V::ascii("second")));
    tiff_block(&Block {
        big_endian,
        data: rgb_page_samples(),
        ifd0,
        exif: Some(vec![(33437, V::Rational(vec![(28, 10)]))]),
        gps: Some(vec![(5, V::Byte(vec![1]))]),
        interop: Some(vec![(1, V::ascii("R98"))]),
        next: vec![page1],
    })
    .bytes
}

#[test]
fn a_tiff_shows_the_tags_of_its_pages_and_their_directories() {
    for big_endian in [false, true] {
        let read = read(FileFormat::Tiff, &described_tiff(big_endian));
        let expected = [
            "TIFF:page 0 |  |  | {",
            "TIFF:page 0/0x0100 | ImageWidth | LONG | 2",
            "TIFF:page 0/0x0101 | ImageLength | LONG | 2",
            "TIFF:page 0/0x0102 | BitsPerSample | SHORT | [8, 8, 8]",
            "TIFF:page 0/0x0103 | Compression | SHORT | 1",
            "TIFF:page 0/0x0106 | PhotometricInterpretation | SHORT | 2",
            "TIFF:page 0/0x010E | ImageDescription | ASCII | \"a described page\"",
            "TIFF:page 0/0x0111 | StripOffsets | LONG | 8",
            "TIFF:page 0/0x0115 | SamplesPerPixel | SHORT | 3",
            "TIFF:page 0/0x0116 | RowsPerStrip | LONG | 2",
            "TIFF:page 0/0x0117 | StripByteCounts | LONG | 12",
            "TIFF:page 0/0x011A | XResolution | RATIONAL | \"300/1\"",
            "TIFF:page 0/0x0129 | PageNumber | SHORT | [0, 2]",
            "TIFF:page 0/0x02BC | XMP | BYTE | \"<x:xmpmeta/>\"",
            "TIFF:page 0/0x8769 | ExifIFD | LONG | 506",
            "TIFF:page 0/0x8773 | ICCProfile | UNDEFINED | <172 bytes>",
            "TIFF:page 0/0x8825 | GPSIFD | LONG | 544",
            "TIFF:page 0/0x9C9D | XPAuthor | BYTE | \"Zoë\"",
            "TIFF:page 0/0xFDE8 | Unknown | SBYTE | -2",
            "TIFF:page 0/0xFDE9 | Unknown | DOUBLE | [0.5, -2.25]",
            "TIFF:page 0/0xFDEA | Unknown | UNDEFINED | <3 bytes>",
            "TIFF:page 0/Exif |  |  | {",
            "TIFF:page 0/Exif/0x829D | FNumber | RATIONAL | \"28/10\"",
            "TIFF:page 0/Exif/0xA005 | InteropIFD | LONG | 562",
            "TIFF:page 0/GPS |  |  | {",
            "TIFF:page 0/GPS/0x0005 | GPSAltitudeRef | BYTE | 1",
            "TIFF:page 0/Interop |  |  | {",
            "TIFF:page 0/Interop/0x0001 | InteroperabilityIndex | ASCII | \"R98\"",
            "TIFF:page 1 |  |  | {",
            "TIFF:page 1/0x0100 | ImageWidth | LONG | 2",
            "TIFF:page 1/0x0101 | ImageLength | LONG | 2",
            "TIFF:page 1/0x0102 | BitsPerSample | SHORT | [8, 8, 8]",
            "TIFF:page 1/0x0103 | Compression | SHORT | 1",
            "TIFF:page 1/0x0106 | PhotometricInterpretation | SHORT | 2",
            "TIFF:page 1/0x0111 | StripOffsets | LONG | 8",
            "TIFF:page 1/0x0115 | SamplesPerPixel | SHORT | 3",
            "TIFF:page 1/0x0116 | RowsPerStrip | LONG | 2",
            "TIFF:page 1/0x0117 | StripByteCounts | LONG | 12",
            "TIFF:page 1/0x011D | PageName | ASCII | \"second\"",
            "ICC | Size |  | 172",
            "ICC | Version |  | \"2.1.0\"",
            "ICC | Class |  | \"mntr\"",
            "ICC | ColorSpace |  | \"RGB\"",
            "ICC | Description |  | \"A test display\"",
        ];
        assert_rows(&format!("TIFF, big endian {big_endian}"), &read, &expected);
    }
}

/// A BigTIFF page: eight-byte counts and offsets, 20-byte entries.
fn big_tiff(entries: &[(u16, u16, u64, [u8; 8])], next: u64) -> Vec<u8> {
    let mut out = b"II\x2b\0\x08\0\0\0".to_vec();
    out.extend_from_slice(&16_u64.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (tag, kind, count, field) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(field);
    }
    out.extend_from_slice(&next.to_le_bytes());
    out
}

fn field(bytes: &[u8]) -> [u8; 8] {
    let mut field = [0; 8];
    field[..bytes.len()].copy_from_slice(bytes);
    field
}

#[test]
fn a_bigtiff_page_is_read_with_its_wider_counts_and_offsets() {
    let mut file = big_tiff(
        &[
            (256, 4, 1, field(&640_u32.to_le_bytes())),
            (257, 16, 1, field(&5_000_000_000_u64.to_le_bytes())),
            (270, 2, 6, field(b"inline")),
            // Twelve bytes do not fit the field: they follow the directory.
            (305, 2, 12, field(&(16 + 8 + 5 * 20 + 8_u64).to_le_bytes())),
            (65000, 17, 1, field(&(-3_i64).to_le_bytes())),
        ],
        0,
    );
    file.extend_from_slice(b"out of line\0");
    assert_rows(
        "BigTIFF",
        &read(FileFormat::Tiff, &file),
        &[
            "TIFF:page 0 |  |  | {",
            "TIFF:page 0/0x0100 | ImageWidth | LONG | 640",
            "TIFF:page 0/0x0101 | ImageLength | LONG8 | 5000000000",
            "TIFF:page 0/0x010E | ImageDescription | ASCII | \"inline\"",
            "TIFF:page 0/0x0131 | Software | ASCII | \"out of line\"",
            "TIFF:page 0/0xFDE8 | Unknown | SLONG8 | -3",
        ],
    );
}

/// The files Pillow wrote hold what Pillow was given, in the layout real
/// encoders use.
#[test]
fn files_written_by_another_encoder_show_what_they_were_written_with() {
    let exif = |prefix: &str| -> Vec<String> {
        [
            "IFD0/0x010F | Make | ASCII | \"Aperture Works\"",
            "IFD0/0x0110 | Model | ASCII | \"Lumen 7\"",
            "IFD0/0x0112 | Orientation | SHORT | 6",
            "IFD0/0x0132 | DateTime | ASCII | \"2021:03:04 05:06:07\"",
            "IFD0/0x013B | Artist | ASCII | \"Robin Example\"",
            "Exif/0x829A | ExposureTime | RATIONAL | \"1/250\"",
            "Exif/0x8827 | ISOSpeedRatings | SHORT | 400",
            "Exif/0x9003 | DateTimeOriginal | ASCII | \"2021:03:04 05:06:07\"",
            "Exif/0xA002 | PixelXDimension | SHORT | 8",
            "Exif/0xA003 | PixelYDimension | SHORT | 8",
            "GPS/0x0001 | GPSLatitudeRef | ASCII | \"N\"",
            "GPS/0x0002 | GPSLatitude | RATIONAL | \"48/1, 51/1, 2987/100\"",
            "GPS/0x0003 | GPSLongitudeRef | ASCII | \"E\"",
            "GPS/0x0004 | GPSLongitude | RATIONAL | \"2/1, 17/1, 4003/100\"",
        ]
        .iter()
        .map(|row| format!("{prefix}{row}"))
        .collect()
    };
    let profile = [
        "ICC | Size |  | 588",
        "ICC | Version |  | \"4.4.0\"",
        "ICC | Class |  | \"mntr\"",
        "ICC | ColorSpace |  | \"RGB\"",
        "ICC | Description |  | \"sRGB built-in\"",
    ];
    let own = |rows: &[&str]| rows.iter().map(|row| row.to_string()).collect::<Vec<_>>();
    // (file, format, rows the tree must hold, in this order among others)
    let cases = [
        (
            "Pillow JPEG",
            FileFormat::Jpeg,
            tagged::pillow_jpeg(),
            [
                own(&[
                    "JPEG:JFIF | Version |  | \"1.01\"",
                    "JPEG:JFIF | Units |  | 1",
                    "JPEG:JFIF | XDensity |  | 72",
                    "JPEG:JFIF | YDensity |  | 72",
                    "JPEG:COM | Comment |  | \"made for a metadata test\"",
                    "JPEG:SOF0 | Precision |  | 8",
                    "JPEG:SOF0 | Height |  | 8",
                    "JPEG:SOF0 | Width |  | 8",
                    "JPEG:SOF0 | Components |  | 3",
                ]),
                exif("EXIF/"),
            ]
            .concat(),
        ),
        (
            "Pillow PNG",
            FileFormat::Png,
            tagged::pillow_png(),
            [
                own(&[
                    "PNG:IHDR | Width |  | 8",
                    "PNG:IHDR | Height |  | 8",
                    "PNG:IHDR | BitDepth |  | 8",
                    "PNG:IHDR | ColorType |  | 2",
                    "PNG:iCCP | ProfileName |  | \"ICC Profile\"",
                    "PNG:tEXt | Text |  | \"Title: A small square\"",
                    "PNG:zTXt | Text |  | \"Author: Robin Example\"",
                    "PNG:iTXt | Text |  | \"Description: café — naïve\"",
                    // 300 dpi is 11,811 pixels to the metre.
                    "PNG:pHYs | PixelsPerUnitX |  | 11811",
                    "PNG:pHYs | PixelsPerUnitY |  | 11811",
                    "PNG:pHYs | Unit |  | 1",
                ]),
                exif("EXIF/"),
                own(&profile),
            ]
            .concat(),
        ),
        (
            "Pillow TIFF",
            FileFormat::Tiff,
            tagged::pillow_tiff(),
            own(&[
                "TIFF:page 0/0x0100 | ImageWidth | LONG | 8",
                "TIFF:page 0/0x0101 | ImageLength | LONG | 8",
                "TIFF:page 0/0x0102 | BitsPerSample | SHORT | [8, 8, 8]",
                "TIFF:page 0/0x0103 | Compression | SHORT | 1",
                "TIFF:page 0/0x010E | ImageDescription | ASCII | \"a described page\"",
                "TIFF:page 0/0x011A | XResolution | RATIONAL | \"96/1\"",
                "TIFF:page 0/0x0131 | Software | ASCII | \"sample writer 1.0\"",
                "TIFF:page 0/0x0132 | DateTime | ASCII | \"2021:03:04 05:06:07\"",
                "TIFF:page 0/0x013B | Artist | ASCII | \"Robin Example\"",
            ]),
        ),
        (
            "Pillow WebP",
            FileFormat::Webp,
            tagged::pillow_webp(),
            [
                own(&[
                    "WEBP:VP8X | Flags |  | 44",
                    "WEBP:VP8X | CanvasWidth |  | 8",
                    "WEBP:VP8X | CanvasHeight |  | 8",
                ]),
                exif("EXIF/"),
                own(&profile),
            ]
            .concat(),
        ),
    ];
    for (context, format, bytes, expected) in cases {
        let read = read(format, &bytes);
        assert_within_limits(context, &read);
        assert_eq!(read.tree.notes, [], "{context}");
        let mut rows = read.rows.iter();
        for row in &expected {
            assert!(
                rows.any(|found| found == row),
                "{context}: no row {row:?}, or out of order, in {:#?}",
                read.rows
            );
        }
        // A small file costs about its own size: nothing is read over and
        // over.
        assert!(
            read.bytes <= 2 * bytes.len() as u64,
            "{context}: {} bytes",
            read.bytes
        );
    }
    let webp = read(FileFormat::Webp, &tagged::pillow_webp());
    assert!(
        rows_under(&webp, "XMP | Packet")
            .first()
            .is_some_and(|row| row.contains("<dc:creator>Robin Example</dc:creator>")),
        "{:#?}",
        webp.rows
    );
}

// ---------------------------------------------------------------------------
// Text nobody vouches for

/// What a PNG text chunk with the keyword `k` shows, after its `k: ` label.
fn shown_png_text(chunk: Vec<u8>) -> String {
    use dcmview::api::contracts::TagValue;
    let read = read(FileFormat::Png, &png_with_chunks(&[chunk], &[]));
    assert_within_limits("a text chunk", &read);
    assert_eq!(read.tree.notes, [], "a text chunk that is whole");
    let node = read
        .tree
        .nodes
        .iter()
        .find(|node| node.keyword == "Text")
        .unwrap_or_else(|| panic!("no text row in {:#?}", read.rows));
    let TagValue::String { value } = &node.value else {
        panic!("a text chunk shows text, not {:?}", node.value);
    };
    let text = value.strip_prefix("k:").expect("label");
    text.strip_prefix(' ').unwrap_or(text).to_string()
}

#[test]
fn text_of_a_file_is_decoded_escaped_and_cut_before_it_is_shown() {
    let utf8 = |text: &[u8]| shown_png_text(png_international_text(b"k", b"", b"", text, false));
    let latin1 = |text: &[u8]| shown_png_text(png_text(b"k", text));

    // (what the file holds, what is shown)
    let cases: [(&[u8], &str); 12] = [
        (b"plain text", "plain text"),
        // A terminal escape sequence is text, not a command.
        (
            b"\x1b[31mred\x1b[0m \x1b]0;title\x07",
            "\\u{1b}[31mred\\u{1b}[0m \\u{1b}]0;title\\u{7}",
        ),
        // Line structure is flattened to spaces.
        (b"one\ntwo\r\nthree\tfour", "one two  three four"),
        // A NUL inside a value is shown; trailing NULs and blanks are not.
        (b"a\0b\0\0  \n", "a\\u{0}b"),
        // Bytes that are not UTF-8 are replaced, never passed on.
        (
            b"bad \xff\xfe utf8 \xc3",
            "bad \u{fffd}\u{fffd} utf8 \u{fffd}",
        ),
        (
            "caf\u{e9} \u{2014} \u{1f600}".as_bytes(),
            "caf\u{e9} \u{2014} \u{1f600}",
        ),
        // Characters that reorder or hide what is shown.
        (
            "abc\u{202e}dcba\u{200b}\u{2066}x\u{feff}".as_bytes(),
            "abc\\u{202e}dcba\\u{200b}\\u{2066}x\\u{feff}",
        ),
        (
            "line\u{2028}break\u{85}next".as_bytes(),
            "line\\u{2028}break\\u{85}next",
        ),
        (b"del\x7f", "del\\u{7f}"),
        // A backslash is itself.
        (b"C:\\scans\\a", "C:\\scans\\a"),
        (b"", ""),
        (b"   ", ""),
    ];
    for (bytes, shown) in cases {
        assert_eq!(utf8(bytes), shown, "{:?}", String::from_utf8_lossy(bytes));
    }
    // Compressed text that is empty is empty text: its stream ends having
    // produced nothing, which is not damage.
    assert_eq!(shown_png_text(png_compressed_text(b"k", b"")), "");
    assert_eq!(
        shown_png_text(png_international_text(b"k", b"", b"", b"", true)),
        ""
    );
    // Latin-1 has no invalid bytes; its control range is escaped like any.
    assert_eq!(
        latin1(b"Ren\xe9 \x9b31m \x85!"),
        "Ren\u{e9} \\u{9b}31m \\u{85}!"
    );

    // A long value is cut at the character limit and says so. The limit
    // counts the whole value, here with its three-character label.
    let limit = RASTER_TAG_TEXT_MAX_CHARS - 3;
    let long = utf8(&vec![b'x'; 10 * limit]);
    assert_eq!(long.chars().count(), limit + 1);
    assert!(long.ends_with('…') && long.starts_with("xxx"), "{long:.20}");
    // Counted in characters shown, whatever they take to store or to escape.
    let wide = utf8("\u{1f600}".repeat(3 * limit).as_bytes());
    assert_eq!(wide.chars().count(), limit + 1, "four-byte characters");
    let escaped = utf8(&vec![0x1b; 3 * limit]);
    assert!(escaped.chars().count() <= limit + 1 && escaped.ends_with('…'));
    assert!(
        escaped.trim_end_matches('…').ends_with("\\u{1b}"),
        "an escape is never cut in half: {}",
        &escaped[escaped.len() - 16..]
    );
    // One at the limit is whole.
    let exact = utf8(&vec![b'x'; limit]);
    assert_eq!(exact.len(), limit);

    // The same holds wherever text comes from: a directory entry, a comment,
    // an XMP packet, a profile.
    let hostile = "a\x1b[2J\u{202e}b";
    let shown = "\"a\\\\u{1b}[2J\\\\u{202e}b\"";
    let exif = tiff_block(&Block {
        ifd0: vec![(315, V::ascii(hostile)), (40093, V::utf16(hostile))],
        ..Block::default()
    })
    .bytes;
    let profile = icc_with_unicode_description(b"mntr", hostile);
    let jpeg = read(
        FileFormat::Jpeg,
        &jpeg_with_segments(&[
            jpeg_exif(&exif),
            jpeg_xmp(hostile.as_bytes()),
            jpeg_profile(1, 1, &profile),
            jpeg_segment(0xfe, hostile.as_bytes()),
        ]),
    );
    assert_within_limits("hostile text in a JPEG", &jpeg);
    for place in [
        "EXIF/IFD0/0x013B | Artist | ASCII | ",
        "EXIF/IFD0/0x9C9D | XPAuthor | BYTE | ",
        "XMP | Packet |  | ",
        "ICC | Description |  | ",
        "JPEG:COM | Comment |  | ",
    ] {
        assert_eq!(
            rows_under(&jpeg, place),
            [format!("{place}{shown}")],
            "{place}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a read may cost

/// The chunks of a 2 x 2 PNG whose image data is `megabytes` of bytes that
/// do not compress, in chunks of `chunk` bytes.
fn png_with_image_data(megabytes: usize, chunk: usize, after: &[Vec<u8>]) -> Vec<u8> {
    let data = super::bounds::noise(megabytes * 1024 * 1024);
    let mut chunks = data
        .chunks(chunk)
        .map(|part| png_chunk(b"IDAT", part))
        .collect::<Vec<_>>();
    chunks.extend_from_slice(after);
    png_from_chunks((2, 2), 8, 2, false, &chunks)
}

/// Image data is stepped over, not read: a file's metadata costs what its
/// metadata holds, wherever it lies and however large the image is.
#[test]
fn image_data_is_stepped_over_and_never_read() {
    const KIB: u64 = 1024;
    let text = png_text(b"Comment", b"after the image");
    let exif = png_chunk(b"eXIf", &small_exif(false));

    // (file, format, the most bytes its metadata may cost, a row it shows)
    let mut cases: Vec<(&str, FileFormat, Vec<u8>, u64, &str)> = Vec::new();
    // Four megabytes of image in 8 KiB chunks, as libpng writes them: 512
    // chunk headers of eight bytes, and the chunks that are metadata.
    cases.push((
        "PNG, small IDAT chunks",
        FileFormat::Png,
        png_with_image_data(4, 8 * 1024, &[text.clone(), exif.clone()]),
        16 * KIB,
        "PNG:tEXt | Text |  | \"Comment: after the image\"",
    ));
    cases.push((
        "PNG, one IDAT chunk",
        FileFormat::Png,
        png_with_image_data(4, usize::MAX, &[text, exif]),
        8 * KIB,
        "EXIF/IFD0/0x010F | Make | ASCII | \"Aperture Works\"",
    ));
    // A TIFF page whose samples lie before its directory.
    let mut page = rgb_page_tags();
    page.push((270, V::ascii("after four megabytes")));
    cases.push((
        "TIFF, samples first",
        FileFormat::Tiff,
        tiff_block(&Block {
            data: super::bounds::noise(4 * 1024 * 1024),
            ifd0: page,
            ..Block::default()
        })
        .bytes,
        KIB,
        "TIFF:page 0/0x010E | ImageDescription | ASCII | \"after four megabytes\"",
    ));
    // A WebP carries EXIF and XMP after its image.
    let mut chunks = vp8x(0x08, (8, 8));
    chunks.extend(riff_chunk(b"VP8L", &super::bounds::noise(4 * 1024 * 1024)));
    chunks.extend(riff_chunk(b"EXIF", &small_exif(false)));
    cases.push((
        "WebP, image first",
        FileFormat::Webp,
        webp_from_chunks(&chunks),
        KIB,
        "EXIF/IFD0/0x010F | Make | ASCII | \"Aperture Works\"",
    ));
    // A JPEG is not read past its first scan.
    let mut jpeg = jpeg_with_segments(&[jpeg_segment(0xfe, b"before the scan")]);
    jpeg.splice(jpeg.len() - 2.., super::bounds::noise(4 * 1024 * 1024));
    cases.push((
        "JPEG, a long scan",
        FileFormat::Jpeg,
        jpeg,
        2 * KIB,
        "JPEG:COM | Comment |  | \"before the scan\"",
    ));

    for (context, format, bytes, most, row) in cases {
        assert!(bytes.len() > 4 * 1024 * 1024, "{context}");
        let read = read(format, &bytes);
        assert_within_limits(context, &read);
        assert!(
            read.rows.iter().any(|found| found == row),
            "{context}: {:#?}",
            read.rows
        );
        assert_eq!(read.tree.notes, [], "{context}");
        assert!(
            read.bytes <= most,
            "{context}: {} bytes read for the metadata of a {}-byte file, at most {most} expected",
            read.bytes,
            bytes.len()
        );
    }

    // A file far larger than the budget, most of it image: still its
    // metadata, read from where it lies.
    let mut large = png_with_image_data(1, usize::MAX, &[]);
    large.truncate(large.len() - 12);
    let length = large.len() as u64 + 3 * 1024 * 1024 * 1024;
    let read = read_source(
        FileFormat::Png,
        CountedFile::with_tail(large, &[0x55], length),
    );
    assert_within_limits("a PNG that claims gigabytes", &read);
    assert_eq!(
        rows_under(&read, "PNG:IHDR | Width"),
        ["PNG:IHDR | Width |  | 2"]
    );
}

/// A directory whose entries are `entries`, as the first page of a TIFF.
fn tiff_of(ifd0: Dir) -> Layout {
    tiff_block(&Block {
        ifd0,
        ..Block::default()
    })
}

fn patch(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

/// What a hostile file must still give: a row from the part that is sound,
/// and a note saying the tree is not all there is.
struct Hostile {
    context: &'static str,
    format: FileFormat,
    source: CountedFile,
    /// A row the tree must still show.
    shows: &'static str,
    /// A note the tree must carry; `None` when the file is merely odd.
    note: Option<Note>,
    /// The file holds so much to walk that reading stops at the byte or
    /// the read limit, whichever its way of reading meets first.
    past_reading: bool,
    /// Text the file holds where no value may come from.
    hides: Option<&'static str>,
}

impl Hostile {
    fn past_reading(mut self) -> Self {
        self.past_reading = true;
        self
    }

    fn hiding(mut self, text: &'static str) -> Self {
        self.hides = Some(text);
        self
    }
}

fn hostile(
    context: &'static str,
    format: FileFormat,
    bytes: Vec<u8>,
    shows: &'static str,
    note: Option<Note>,
) -> Hostile {
    Hostile {
        context,
        format,
        source: CountedFile::new(bytes),
        shows,
        note,
        past_reading: false,
        hides: None,
    }
}

const WIDTH_ROW: &str = "TIFF:page 0/0x0100 | ImageWidth | LONG | 2";

fn hostile_directories() -> Vec<Hostile> {
    let mut cases = Vec::new();
    let page = || {
        let mut tags = rgb_page_tags();
        tags.push((270, V::ascii("sound")));
        tags
    };
    let two_pages = || {
        tiff_block(&Block {
            data: rgb_page_samples(),
            ifd0: page(),
            next: vec![page()],
            ..Block::default()
        })
    };
    let damaged = Some(Note::Damaged(Part::Container));

    // The page chain loops back to the first page, or a page to itself.
    let mut loops = two_pages();
    let link = loops.link(loops.next[0]);
    let first = (loops.ifd0 as u32).to_le_bytes();
    patch(&mut loops.bytes, link, &first);
    cases.push(hostile(
        "a page chain that loops",
        FileFormat::Tiff,
        loops.bytes,
        WIDTH_ROW,
        damaged,
    ));
    let mut own = two_pages();
    let link = own.link(own.ifd0);
    let first = (own.ifd0 as u32).to_le_bytes();
    patch(&mut own.bytes, link, &first);
    cases.push(hostile(
        "a page that follows itself",
        FileFormat::Tiff,
        own.bytes,
        WIDTH_ROW,
        damaged,
    ));

    // Directories that point at themselves and at each other.
    let pointing = |exif_at: fn(&Layout) -> u32, interop_at: fn(&Layout) -> u32| {
        let mut layout = tiff_block(&Block {
            data: rgb_page_samples(),
            ifd0: page(),
            exif: Some(vec![(36864, V::Undefined(b"0232".to_vec()))]),
            gps: Some(vec![(1, V::ascii("N"))]),
            interop: Some(vec![(1, V::ascii("R98"))]),
            ..Block::default()
        });
        let exif_entry = layout.entry(layout.ifd0, 34665) + 8;
        let gps_entry = layout.entry(layout.ifd0, 34853) + 8;
        let interop_entry = layout.entry(layout.exif, 40965) + 8;
        let (exif, interop) = (exif_at(&layout), interop_at(&layout));
        patch(&mut layout.bytes, exif_entry, &exif.to_le_bytes());
        patch(&mut layout.bytes, gps_entry, &exif.to_le_bytes());
        patch(&mut layout.bytes, interop_entry, &interop.to_le_bytes());
        layout.bytes
    };
    let exif_damaged = Some(Note::Damaged(Part::Exif));
    cases.push(hostile(
        "EXIF and GPS pointers to the page itself",
        FileFormat::Tiff,
        pointing(|layout| layout.ifd0 as u32, |layout| layout.ifd0 as u32),
        WIDTH_ROW,
        exif_damaged,
    ));
    cases.push(hostile(
        "an interoperability pointer back to its EXIF directory",
        FileFormat::Tiff,
        pointing(|layout| layout.exif as u32, |layout| layout.exif as u32),
        WIDTH_ROW,
        exif_damaged,
    ));
    cases.push(hostile(
        "pointers past the end of the file",
        FileFormat::Tiff,
        pointing(|_| 0xffff_fff0, |_| 0x7fff_ffff),
        WIDTH_ROW,
        exif_damaged,
    ));

    // A hundred directories each naming the next as its EXIF directory:
    // only the pointers this reader follows lead anywhere.
    let mut nested = b"II\x2a\0\x08\0\0\0".to_vec();
    for level in 0..100_u32 {
        let next = 8 + (level + 1) * 30;
        nested.extend_from_slice(&le16(2));
        for (tag, value) in [(256_u16, 2_u32), (34665, next)] {
            nested.extend_from_slice(&le16(tag));
            nested.extend_from_slice(&le16(4));
            nested.extend_from_slice(&1_u32.to_le_bytes());
            nested.extend_from_slice(&value.to_le_bytes());
        }
        nested.extend_from_slice(&[0; 4]);
    }
    nested.extend_from_slice(&[0; 6]);
    cases.push(hostile(
        "EXIF directories nested a hundred deep",
        FileFormat::Tiff,
        nested,
        WIDTH_ROW,
        None,
    ));

    // Values that lie outside the file, wrap around, or overlap.
    let raw = |tag: u16, kind: u16, count: u32, field: u32| {
        (tag, V::Raw(kind, count, field.to_le_bytes()))
    };
    let with = |extra: Vec<(u16, V)>| {
        let mut tags = rgb_page_tags();
        tags.extend(extra);
        tiff_block(&Block {
            data: rgb_page_samples(),
            ifd0: tags,
            ..Block::default()
        })
        .bytes
    };
    cases.push(hostile(
        "values outside the file",
        FileFormat::Tiff,
        with(vec![
            raw(270, 2, 64, 0x00ff_ffff),
            raw(271, 2, 64, 0xffff_fff0),
            raw(272, 5, 4, 0xffff_ffff),
            (305, V::ascii("sound software")),
        ]),
        "TIFF:page 0/0x0131 | Software | ASCII | \"sound software\"",
        damaged,
    ));
    cases.push(hostile(
        "counts of 2^32 - 1",
        FileFormat::Tiff,
        with(vec![
            raw(270, 2, u32::MAX, 8),
            raw(271, 4, u32::MAX, 8),
            raw(272, 12, u32::MAX, 8),
            raw(273, 4, u32::MAX, 8),
            raw(700, 1, u32::MAX, 8),
            (305, V::ascii("sound software")),
        ]),
        "TIFF:page 0/0x0131 | Software | ASCII | \"sound software\"",
        damaged,
    ));
    cases.push(hostile(
        "types that are not types",
        FileFormat::Tiff,
        with(vec![
            raw(270, 0, 1, 8),
            raw(271, 14, 4, 8),
            raw(272, 99, u32::MAX, 8),
            raw(285, 0xffff, 0, 0),
            (305, V::ascii("sound software")),
        ]),
        "TIFF:page 0/0x0131 | Software | ASCII | \"sound software\"",
        None,
    ));
    // Every text value names the same bytes, and those bytes are the
    // directory itself: allowed, and no more is read for it.
    let overlapping = {
        let layout = tiff_of(rgb_page_tags());
        let at = layout.ifd0 as u32;
        let mut tags = rgb_page_tags();
        tags.extend((0..200).map(|index| raw(60_000 + index, 2, 100, at)));
        tiff_block(&Block {
            ifd0: tags,
            ..Block::default()
        })
        .bytes
    };
    cases.push(hostile(
        "two hundred values that overlap the directory",
        FileFormat::Tiff,
        overlapping,
        WIDTH_ROW,
        None,
    ));

    // A directory that declares more entries than the file holds.
    let mut lying = with(vec![]);
    let at = tiff_of(rgb_page_tags()).ifd0 + 12;
    patch(&mut lying, at, &le16(0xffff));
    cases.push(hostile(
        "a directory that declares 65,535 entries",
        FileFormat::Tiff,
        lying,
        WIDTH_ROW,
        Some(Note::Limit(Limit::Entries)),
    ));

    // One that really has them: the first are shown and the rest not read.
    let mut many = rgb_page_tags();
    many.extend((0..40_000_u16).map(|index| (300 + index, V::Short(vec![index]))));
    cases.push(hostile(
        "a directory of 40,009 entries",
        FileFormat::Tiff,
        tiff_of(many).bytes,
        WIDTH_ROW,
        Some(Note::Limit(Limit::Entries)),
    ));

    // Two thousand pages.
    let pages = tiff_block(&Block {
        data: rgb_page_samples(),
        ifd0: page(),
        next: vec![page(); 1999],
        ..Block::default()
    })
    .bytes;
    cases.push(hostile(
        "two thousand pages",
        FileFormat::Tiff,
        pages,
        WIDTH_ROW,
        Some(Note::Limit(Limit::Pages)),
    ));

    // Megabytes of text in every tag of sixteen pages.
    let wordy = |tag: u16| (tag, V::Ascii(vec![b'w'; 1024 * 1024]));
    let mut loud = page();
    loud.extend([wordy(269), wordy(271), wordy(272)]);
    cases.push(hostile(
        "megabytes of text in a page's tags",
        FileFormat::Tiff,
        tiff_of(loud).bytes,
        WIDTH_ROW,
        None,
    ));

    // As large as a tree gets: a thousand entries of as many numbers as
    // one value shows.
    let full = |page: u16| -> Dir {
        let mut tags = rgb_page_tags();
        tags.extend((0..240).map(|index| (1000 + index, V::Long(vec![u32::from(page); 64]))));
        tags
    };
    cases.push(hostile(
        "a thousand entries of sixty-four numbers",
        FileFormat::Tiff,
        tiff_block(&Block {
            data: rgb_page_samples(),
            ifd0: full(0),
            next: vec![full(1), full(2), full(3)],
            ..Block::default()
        })
        .bytes,
        WIDTH_ROW,
        None,
    ));

    // BigTIFF: a count that overflows when multiplied by its type's size,
    // an entry count of 2^64 - 1, and a page chain that loops.
    let mut big = big_tiff(
        &[
            (256, 4, 1, field(&2_u32.to_le_bytes())),
            (270, 12, u64::MAX / 2, field(&16_u64.to_le_bytes())),
            (271, 2, u64::MAX, field(&16_u64.to_le_bytes())),
        ],
        16,
    );
    cases.push(hostile(
        "BigTIFF counts that overflow",
        FileFormat::Tiff,
        big.clone(),
        WIDTH_ROW,
        damaged,
    ));
    patch(&mut big, 16, &u64::MAX.to_le_bytes());
    cases.push(hostile(
        "a BigTIFF directory of 2^64 - 1 entries",
        FileFormat::Tiff,
        big,
        WIDTH_ROW,
        Some(Note::Limit(Limit::Entries)),
    ));

    // An EXIF block whose values point outside it, at bytes the file does
    // hold: a block is read inside its own bounds only.
    let mut escaping = tiff_block(&Block {
        ifd0: vec![(271, V::ascii("sound make")), raw(315, 2, 16, 0)],
        ..Block::default()
    });
    let entry = escaping.entry(escaping.ifd0, 315) + 8;
    let beyond = escaping.bytes.len() as u32 + 40;
    patch(&mut escaping.bytes, entry, &beyond.to_le_bytes());
    cases.push(
        hostile(
            "an EXIF value that points outside its block",
            FileFormat::Jpeg,
            jpeg_with_segments(&[jpeg_exif(&escaping.bytes), jpeg_segment(0xec, &[b'O'; 200])]),
            "EXIF/IFD0/0x010F | Make | ASCII | \"sound make\"",
            Some(Note::Damaged(Part::Exif)),
        )
        .hiding("OOOO"),
    );
    cases
}

fn hostile_containers() -> Vec<Hostile> {
    let mut cases = Vec::new();
    let png = |chunks: Vec<Vec<u8>>| png_with_chunks(&chunks, &[]);
    const PNG_ROW: &str = "PNG:IHDR | Width |  | 2";
    let container = Some(Note::Damaged(Part::Container));

    // Compressed text and a compressed profile that inflate to 64 MiB,
    // deflated as tightly as the format allows (about a thousand to one),
    // so that even the few kilobytes read of a text chunk hold megabytes.
    let zeros = {
        use std::io::Write;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder
            .write_all(&vec![0; 64 * 1024 * 1024])
            .expect("deflate");
        encoder.finish().expect("deflate")
    };
    assert!(zeros.len() < 80 * 1024, "{} bytes deflated", zeros.len());
    let mut bomb = b"bomb\0\0".to_vec();
    bomb.extend_from_slice(&zeros);
    let mut itxt = b"bomb\0\x01\0\0\0".to_vec();
    itxt.extend_from_slice(&zeros);
    let mut profile = b"bomb\0\0".to_vec();
    profile.extend_from_slice(&zeros);
    cases.push(hostile(
        "compressed chunks that inflate to 64 MiB",
        FileFormat::Png,
        png(vec![
            png_chunk(b"zTXt", &bomb),
            png_chunk(b"iTXt", &itxt),
            png_chunk(b"iCCP", &profile),
        ]),
        PNG_ROW,
        Some(Note::Damaged(Part::Icc)),
    ));
    // A real profile of 3 MiB whose description lies at its end.
    let mut far = icc_with_description(b"mntr", "far away");
    let description = far.split_off(144);
    far.resize(3 * 1024 * 1024, 0);
    let at = far.len() as u32;
    far.extend_from_slice(&description);
    patch(&mut far, 136, &at.to_be_bytes());
    let length = far.len() as u32;
    patch(&mut far, 0, &length.to_be_bytes());
    cases.push(hostile(
        "a profile whose description lies megabytes in",
        FileFormat::Png,
        png(vec![png_profile(b"far", &far)]),
        "ICC | Size |  | 3145752",
        Some(Note::Limit(Limit::Inflate)),
    ));

    // Megabytes of text, and thousands of chunks.
    cases.push(hostile(
        "four megabytes in four text chunks",
        FileFormat::Png,
        png((0..4)
            .map(|_| png_text(b"k", &vec![b't'; 1024 * 1024]))
            .collect()),
        PNG_ROW,
        None,
    ));
    cases.push(hostile(
        "five thousand text chunks",
        FileFormat::Png,
        png((0..5000).map(|_| png_text(b"k", b"v")).collect()),
        PNG_ROW,
        Some(Note::Limit(Limit::Nodes)),
    ));
    cases.push(hostile(
        "six hundred text chunks of a thousand characters",
        FileFormat::Png,
        png((0..600).map(|_| png_text(b"k", &[b't'; 1000])).collect()),
        PNG_ROW,
        Some(Note::Limit(Limit::Text)),
    ));
    cases.push(
        hostile(
            "four hundred thousand chunks nobody knows",
            FileFormat::Png,
            png((0..400_000).map(|_| png_chunk(b"zzZz", b"")).collect()),
            PNG_ROW,
            None,
        )
        .past_reading(),
    );

    // Chunks that lie about their length, and text chunks without their
    // separators.
    let mut lying = png(vec![png_text(b"k", b"sound text")]);
    let at = lying
        .windows(4)
        .position(|window| window == b"IDAT")
        .expect("IDAT")
        - 4;
    patch(&mut lying, at, &u32::MAX.to_be_bytes());
    cases.push(hostile(
        "a chunk that declares 4 GiB",
        FileFormat::Png,
        lying,
        "PNG:tEXt | Text |  | \"k: sound text\"",
        container,
    ));
    let mut broken = b"k\0\x01\0".to_vec();
    broken.extend_from_slice(&[b'n'; 4000]);
    cases.push(hostile(
        "text chunks without their separators or with a broken stream",
        FileFormat::Png,
        png(vec![
            png_chunk(b"tEXt", &[b'n'; 300]),
            png_chunk(b"iTXt", &broken),
            png_chunk(b"zTXt", b"k\0\0not a zlib stream"),
            png_chunk(b"iTXt", b"k\0\x01\0\0\0not a zlib stream"),
            png_text(b"k", b"sound text"),
        ]),
        "PNG:tEXt | Text |  | \"k: sound text\"",
        Some(Note::Damaged(Part::Text)),
    ));

    // An EXIF block that describes another image than the one it is in:
    // shown as what the block says, believed by nothing.
    let other = tiff_block(&Block {
        ifd0: vec![
            (256, V::Long(vec![60_000])),
            (257, V::Long(vec![60_000])),
            (258, V::Short(vec![16, 16, 16, 16])),
            (273, V::Long(vec![u32::MAX; 1000])),
            (279, V::Long(vec![u32::MAX; 1000])),
        ],
        ..Block::default()
    })
    .bytes;
    cases.push(hostile(
        "a PNG whose EXIF describes another image",
        FileFormat::Png,
        png(vec![png_chunk(b"eXIf", &other)]),
        PNG_ROW,
        None,
    ));
    let mut prefixed = b"Exif\0\0".to_vec();
    prefixed.extend_from_slice(&other);
    cases.push(hostile(
        "a WebP whose EXIF describes another image",
        FileFormat::Webp,
        webp_with(None, Some(&prefixed), None, &[]),
        "WEBP:VP8X | CanvasWidth |  | 8",
        None,
    ));

    // JPEG: five thousand EXIF segments, megabytes of fill bytes, a segment
    // longer than the file, a profile in 255 parts.
    let exif = jpeg_exif(&small_exif(false));
    cases.push(hostile(
        "a JPEG with five thousand EXIF segments",
        FileFormat::Jpeg,
        jpeg_with_segments(&vec![exif.clone(); 5_000]),
        "EXIF/IFD0/0x010F | Make | ASCII | \"Aperture Works\"",
        None,
    ));
    let mut filled = jpeg_with_segments(&[jpeg_segment(0xfe, b"sound comment")]);
    let at = 2 + 4 + b"sound comment".len();
    filled.splice(at..at, vec![0xff; 5 * 1024 * 1024]);
    cases.push(
        hostile(
            "a JPEG with five megabytes of fill bytes",
            FileFormat::Jpeg,
            filled,
            "JPEG:COM | Comment |  | \"sound comment\"",
            None,
        )
        .past_reading(),
    );
    let mut long = jpeg_with_segments(&[
        jpeg_segment(0xfe, b"sound comment"),
        jpeg_segment(0xe1, b"Exif\0\0"),
    ]);
    long.truncate(2 + 4 + b"sound comment".len() + 4);
    patch(&mut long, 2 + 4 + b"sound comment".len() + 2, &[0xff, 0xff]);
    cases.push(hostile(
        "a JPEG segment longer than the file",
        FileFormat::Jpeg,
        long,
        "JPEG:COM | Comment |  | \"sound comment\"",
        container,
    ));
    let profile = icc_with_description(b"mntr", "in parts");
    let mut parts = vec![jpeg_profile(1, 255, &profile)];
    parts.extend((2..=255).map(|index| jpeg_profile(index, 255, &vec![index; 60_000])));
    cases.push(hostile(
        "a JPEG profile in 255 segments",
        FileFormat::Jpeg,
        jpeg_with_segments(&parts),
        "ICC | Description |  | \"in parts\"",
        None,
    ));

    // WebP: a chunk longer than the file, and more chunks than are walked.
    let mut chunks = vp8x(0, (8, 8));
    chunks.extend(riff_chunk(b"XMP ", b"sound packet"));
    chunks.extend_from_slice(b"EXIF\xff\xff\xff\x7f");
    cases.push(hostile(
        "a WebP chunk longer than the file",
        FileFormat::Webp,
        webp_from_chunks(&chunks),
        "XMP | Packet |  | \"sound packet\"",
        container,
    ));
    let mut chunks = vp8x(0, (8, 8));
    for _ in 0..600_000 {
        chunks.extend(riff_chunk(b"zzzz", b""));
    }
    cases.push(
        hostile(
            "a WebP of six hundred thousand chunks",
            FileFormat::Webp,
            webp_from_chunks(&chunks),
            "WEBP:VP8X | CanvasWidth |  | 8",
            None,
        )
        .past_reading(),
    );
    cases
}

#[test]
fn a_hostile_file_costs_what_any_file_costs_and_still_shows_what_is_sound() {
    for case in hostile_directories()
        .into_iter()
        .chain(hostile_containers())
    {
        let Hostile {
            context,
            format,
            source,
            shows,
            note,
            past_reading,
            hides,
        } = case;
        let read = read_source(format, source);
        assert_within_limits(context, &read);
        assert!(
            read.rows.iter().any(|row| row == shows),
            "{context}: no row {shows:?} in {:#?}",
            &read.rows[..read.rows.len().min(40)]
        );
        let notes = &read.tree.notes;
        if past_reading {
            // The walk ended at a limit and says so; it is not damage.
            assert!(
                !notes.is_empty()
                    && notes.iter().all(|note| matches!(
                        note,
                        Note::Limit(Limit::Reads) | Note::Limit(Limit::Bytes)
                    )),
                "{context}: notes {notes:?}"
            );
        } else if let Some(note) = note {
            assert!(notes.contains(&note), "{context}: notes {notes:?}");
        } else {
            assert_eq!(notes, &[], "{context}");
        }
        if let Some(hidden) = hides {
            assert!(
                !read.rows.iter().any(|row| row.contains(hidden)),
                "{context}: {hidden:?} is shown"
            );
        }
    }
}

/// The limits that have a number of their own, at the number and one past.
#[test]
fn limits_are_exact() {
    // Pages: sixteen are shown; a seventeenth is a note.
    let pages = |count: usize| {
        let tree = read(
            FileFormat::Tiff,
            &tiff_block(&Block {
                data: rgb_page_samples(),
                ifd0: rgb_page_tags(),
                next: vec![rgb_page_tags(); count - 1],
                ..Block::default()
            })
            .bytes,
        );
        assert_within_limits("pages", &tree);
        let shown = tree.tree.nodes.len();
        (shown, tree.tree.notes)
    };
    assert_eq!(
        pages(RASTER_TAGS_MAX_PAGES),
        (RASTER_TAGS_MAX_PAGES, vec![])
    );
    assert_eq!(
        pages(RASTER_TAGS_MAX_PAGES + 1),
        (RASTER_TAGS_MAX_PAGES, vec![Note::Limit(Limit::Pages)])
    );

    // Entries of one directory.
    let entries = |count: usize| {
        let dir = (0..count as u16)
            .map(|index| (1000 + index, V::Short(vec![index])))
            .collect();
        let tree = read(FileFormat::Tiff, &tiff_of(dir).bytes);
        assert_within_limits("entries", &tree);
        (tree.rows.len() - 1, tree.tree.notes)
    };
    assert_eq!(
        entries(RASTER_TAGS_MAX_IFD_ENTRIES),
        (RASTER_TAGS_MAX_IFD_ENTRIES, vec![])
    );
    assert_eq!(
        entries(RASTER_TAGS_MAX_IFD_ENTRIES + 1),
        (
            RASTER_TAGS_MAX_IFD_ENTRIES,
            vec![Note::Limit(Limit::Entries)]
        )
    );

    // Numbers of one value: the first are shown with the count.
    let numbers = |count: usize| {
        let tree = read(
            FileFormat::Tiff,
            &tiff_of(vec![(1000, V::Long((0..count as u32).collect()))]).bytes,
        );
        assert_within_limits("numbers", &tree);
        tree.rows[1].clone()
    };
    let list = |count: usize| {
        (0..count)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    assert_eq!(
        numbers(RASTER_TAG_NUMBERS_MAX),
        format!(
            "TIFF:page 0/0x03E8 | Unknown | LONG | [{}]",
            list(RASTER_TAG_NUMBERS_MAX)
        )
    );
    assert_eq!(
        numbers(RASTER_TAG_NUMBERS_MAX + 1),
        format!(
            "TIFF:page 0/0x03E8 | Unknown | LONG | [{}] of {}",
            list(RASTER_TAG_NUMBERS_MAX),
            RASTER_TAG_NUMBERS_MAX + 1
        )
    );

    // Nodes of one tree.
    let nodes = |count: usize| {
        let chunks = (0..count - 7)
            .map(|_| png_text(b"k", b"v"))
            .collect::<Vec<_>>();
        let tree = read(FileFormat::Png, &png_with_chunks(&chunks, &[]));
        assert_within_limits("nodes", &tree);
        (tree.rows.len(), tree.tree.notes)
    };
    assert_eq!(
        nodes(RASTER_TAGS_MAX_NODES),
        (RASTER_TAGS_MAX_NODES, vec![])
    );
    assert_eq!(
        nodes(RASTER_TAGS_MAX_NODES + 1),
        (RASTER_TAGS_MAX_NODES, vec![Note::Limit(Limit::Nodes)])
    );

    // Numbers that are not finite are not values.
    let odd = read(
        FileFormat::Tiff,
        &tiff_of(vec![
            (1000, V::Double(vec![f64::NAN])),
            (1001, V::Double(vec![1.0, f64::INFINITY])),
        ])
        .bytes,
    );
    assert_within_limits("numbers that are not finite", &odd);
    assert_eq!(
        odd.rows[1..],
        [
            "TIFF:page 0/0x03E8 | Unknown | DOUBLE | !",
            "TIFF:page 0/0x03E9 | Unknown | DOUBLE | !"
        ]
    );
}

/// Every file cut short at every length, and every byte of every file
/// replaced: the reader reports and never fails, and the limits hold.
#[test]
fn files_cut_short_or_altered_anywhere_are_read_within_the_same_limits() {
    let files = [
        (FileFormat::Png, tagged::planted_png().bytes),
        (FileFormat::Jpeg, tagged::planted_jpeg().bytes),
        (FileFormat::Tiff, tagged::planted_tiff().bytes),
        (FileFormat::Webp, tagged::planted_webp().bytes),
        (FileFormat::Tiff, described_tiff(true)),
        (FileFormat::Png, tagged::pillow_png()),
    ];
    for (format, bytes) in &files {
        let whole = read(*format, bytes);
        assert_within_limits("an unaltered file", &whole);
        assert_eq!(whole.tree.notes, [], "{format:?}");
        for length in 0..bytes.len() {
            let cut = read(*format, &bytes[..length]);
            assert_within_limits(&format!("{format:?} cut to {length} bytes"), &cut);
        }
        // One byte replaced, by each of three values.
        for at in 0..bytes.len() {
            for value in [0x00, 0xff, bytes[at] ^ 0x55] {
                let mut altered = bytes.clone();
                altered[at] = value;
                let read = read(*format, &altered);
                assert_within_limits(
                    &format!("{format:?} with byte {at} set to {value:#04x}"),
                    &read,
                );
            }
        }
    }
    // Not the format the catalog says, and nothing at all.
    for (format, bytes) in [
        (FileFormat::Jpeg, tagged::planted_png().bytes),
        (FileFormat::Png, tagged::planted_tiff().bytes),
        (FileFormat::Tiff, tagged::planted_webp().bytes),
        (FileFormat::Webp, tagged::planted_jpeg().bytes),
        (FileFormat::Png, Vec::new()),
        (FileFormat::Tiff, b"II".to_vec()),
    ] {
        let read = read(format, &bytes);
        assert_within_limits("a file of another format", &read);
        assert_eq!(read.rows, [] as [&str; 0], "{format:?}");
        assert_eq!(
            read.tree.notes,
            [Note::Damaged(Part::Container)],
            "{format:?}"
        );
    }
}

/// A tree as a JSON value, for the cases that look inside one.
#[allow(dead_code)]
fn json(read: &Read) -> Value {
    serde_json::to_value(&read.tree.nodes).expect("tree serializes")
}
