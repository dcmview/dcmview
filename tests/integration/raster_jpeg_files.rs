//! The JPEG-compressed TIFF files under `tests/fixtures/tiff-jpeg`, what
//! libtiff decodes them to, and the edits the tests make to them.
//!
//! The files are honest libtiff output (`scripts/generate_tiff_jpeg_fixtures.py`).
//! A file that is not honest is one of them with a tag, a count or a byte
//! of a frame header changed here; no compressed data is written by hand.
//! Tags are edited with the helpers of `raster_bilevel_files.rs`.
//!
//! `tests/raster_cost` includes this file too.

use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tiff-jpeg")
        .join(name)
}

/// The fixture `name`, as bytes.
pub fn fixture(name: &str) -> Vec<u8> {
    let path = path(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The small fixtures, each a 150 x 133 card, with the samples a pixel it
/// decodes to (1 or 3) and the name of libtiff's decode of it
/// (`libtiff-decodes.txt`).
pub fn small_fixtures() -> Vec<(String, u32, String)> {
    let listing = String::from_utf8(fixture("libtiff-decodes.txt")).expect("listing is text");
    listing
        .lines()
        .map(|line| {
            let (name, decode) = line.split_once(' ').expect("two names a line");
            let samples = if name.starts_with("gray") { 1 } else { 3 };
            (name.to_string(), samples, decode.to_string())
        })
        .collect()
}

/// What libtiff decoded the fixture whose decode is `decode` to: 8-bit
/// gray or RGB samples, row by row.
pub fn libtiff_decode(decode: &str) -> Vec<u8> {
    let file = std::fs::File::open(path(decode)).expect("open libtiff decode");
    let mut decoder = tiff::decoder::Decoder::new(file).expect("libtiff decode is a TIFF");
    match decoder.read_image().expect("decode libtiff decode") {
        tiff::decoder::DecodingResult::U8(samples) => samples,
        _ => panic!("{decode}: not 8-bit samples"),
    }
}

/// Where the frame header (SOF0 or SOF2 marker) of the first strip or tile
/// of `bytes` is, found by stepping over the marker segments before it.
pub fn frame_header_at(bytes: &[u8], offset_tag: u16) -> usize {
    use super::raster_bilevel_files::tag_values;
    let start = tag_values(bytes, offset_tag)[0] as usize;
    assert_eq!(
        bytes[start..start + 2],
        [0xFF, 0xD8],
        "a strip begins with SOI"
    );
    let mut at = start + 2;
    loop {
        assert_eq!(bytes[at], 0xFF, "a marker");
        let marker = bytes[at + 1];
        if matches!(marker, 0xC0 | 0xC2) {
            return at;
        }
        let length = usize::from(bytes[at + 2]) << 8 | usize::from(bytes[at + 3]);
        at += 2 + length;
    }
}

/// `bytes` with the frame header of its first strip or tile declaring
/// `width` x `height`.
pub fn with_frame_size(mut bytes: Vec<u8>, offset_tag: u16, width: u16, height: u16) -> Vec<u8> {
    let at = frame_header_at(&bytes, offset_tag);
    bytes[at + 5..at + 7].copy_from_slice(&height.to_be_bytes());
    bytes[at + 7..at + 9].copy_from_slice(&width.to_be_bytes());
    bytes
}

/// `bytes` with the frame header of its first strip or tile declaring
/// `precision` bits a sample.
pub fn with_frame_precision(mut bytes: Vec<u8>, offset_tag: u16, precision: u8) -> Vec<u8> {
    let at = frame_header_at(&bytes, offset_tag);
    bytes[at + 4] = precision;
    bytes
}

/// `bytes` with the count of the entry of `tag` set to `count`, its type
/// and value left as they are.
pub fn with_count(mut bytes: Vec<u8>, tag: u16, count: u32) -> Vec<u8> {
    let little = &bytes[..2] == b"II";
    let read = |bytes: &[u8], at: usize, size: usize| -> u32 {
        let mut value = 0_u32;
        for index in 0..size {
            let byte = u32::from(bytes[at + if little { size - 1 - index } else { index }]);
            value = (value << 8) | byte;
        }
        value
    };
    let ifd = read(&bytes, 4, 4) as usize;
    let entries = read(&bytes, ifd, 2) as usize;
    let at = (0..entries)
        .map(|index| ifd + 2 + 12 * index)
        .find(|&at| read(&bytes, at, 2) == u32::from(tag))
        .unwrap_or_else(|| panic!("no tag {tag} in the first page"));
    let count = if little {
        count.to_le_bytes()
    } else {
        count.to_be_bytes()
    };
    bytes[at + 4..at + 8].copy_from_slice(&count);
    bytes
}

/// The largest difference between two sets of samples of one size.
pub fn largest_difference(left: &[u8], right: &[u8]) -> u8 {
    assert_eq!(left.len(), right.len(), "samples of one size");
    left.iter()
        .zip(right)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}
