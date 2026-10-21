//! The bilevel TIFF files under `tests/fixtures/tiff-bilevel`, the image
//! they hold, and the edits the tests make to their tags.
//!
//! The files are honest libtiff output of [`card`]
//! (`scripts/generate_tiff_bilevel_fixtures.py`). A file that is not honest
//! is one of them with a count or a tag value changed here; no compressed
//! data is written by hand.
//!
//! `tests/raster_cost` includes this file too.

use std::path::PathBuf;

/// The fixture `name`, as bytes.
pub fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tiff-bilevel")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The small fixtures: each the 37 x 21 [`card`], and whether it stores
/// white as zero.
pub fn small_fixtures() -> Vec<(String, bool)> {
    let mut names = Vec::new();
    for compression in ["none", "packbits", "lzw", "deflate", "g4"] {
        for layout in ["", "-strips", "-fill-order-2", "-tiles"] {
            names.push((format!("{compression}{layout}-37x21.tif"), false));
        }
    }
    names.push(("g4-white-is-zero-37x21.tif".to_string(), true));
    names
}

/// The image of every fixture `width` x `height`, one value (0 or 1) per
/// pixel, row by row, where 1 is white: diagonal stripes over the left half
/// (one pixel wide in a small card, 64 in a card 4,096 wide), a disc on the
/// right half, and the last column white.
pub fn card(width: usize, height: usize) -> Vec<u8> {
    let unit = (width / 64).max(1);
    let (centre_x, centre_y) = ((width * 3 / 4) as i64, (height / 2) as i64);
    let radius = (width.min(height) / 4) as i64;
    let mut values = Vec::with_capacity(width * height);
    for row in 0..height {
        for column in 0..width {
            let value = if column == width - 1 {
                true
            } else if column < width / 2 {
                (column * 3 + row * 5) / unit % 7 < 3
            } else {
                let (x, y) = (column as i64 - centre_x, row as i64 - centre_y);
                x * x + y * y <= radius * radius
            };
            values.push(u8::from(value));
        }
    }
    values
}

/// Where the IFD entry of `tag` in the first page of a classic TIFF is.
fn entry_at(bytes: &[u8], tag: u16) -> usize {
    let little = &bytes[..2] == b"II";
    let u16_at = |at: usize| {
        let pair = [bytes[at], bytes[at + 1]];
        if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    };
    let ifd = read_u32(bytes, 4) as usize;
    let count = usize::from(u16_at(ifd));
    (0..count)
        .map(|index| ifd + 2 + 12 * index)
        .find(|&at| u16_at(at) == tag)
        .unwrap_or_else(|| panic!("no tag {tag} in the first page"))
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    let quad = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
    if &bytes[..2] == b"II" {
        u32::from_le_bytes(quad)
    } else {
        u32::from_be_bytes(quad)
    }
}

fn number(bytes: &[u8], value: u32, size: usize) -> Vec<u8> {
    if &bytes[..2] == b"II" {
        value.to_le_bytes()[..size].to_vec()
    } else {
        value.to_be_bytes()[4 - size..].to_vec()
    }
}

/// `bytes` with the entry of `tag` replaced by one LONG `value`, under the
/// tag number `renamed` (`tag` itself to keep it).
pub fn with_tag(mut bytes: Vec<u8>, tag: u16, renamed: u16, value: u32) -> Vec<u8> {
    let at = entry_at(&bytes, tag);
    let entry = [
        number(&bytes, u32::from(renamed), 2),
        number(&bytes, 4, 2),
        number(&bytes, 1, 4),
        number(&bytes, value, 4),
    ]
    .concat();
    bytes[at..at + 12].copy_from_slice(&entry);
    bytes
}

/// The values of `tag`, SHORT or LONG, in the first page.
pub fn tag_values(bytes: &[u8], tag: u16) -> Vec<u32> {
    let at = entry_at(bytes, tag);
    let little = &bytes[..2] == b"II";
    let kind = if little {
        u16::from_le_bytes([bytes[at + 2], bytes[at + 3]])
    } else {
        u16::from_be_bytes([bytes[at + 2], bytes[at + 3]])
    };
    let size = if kind == 3 { 2 } else { 4 };
    let count = read_u32(bytes, at + 4) as usize;
    let start = if size * count <= 4 {
        at + 8
    } else {
        read_u32(bytes, at + 8) as usize
    };
    (0..count)
        .map(|index| {
            let at = start + size * index;
            if size == 2 {
                let pair = [bytes[at], bytes[at + 1]];
                u32::from(if little {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                })
            } else {
                read_u32(bytes, at)
            }
        })
        .collect()
}

/// `bytes` with the one value of a strip or tile byte count, offset or
/// other single-valued LONG or SHORT tag `tag` set to `value`. A tag of
/// several values has its value `index` set, in its own type.
pub fn with_value(mut bytes: Vec<u8>, tag: u16, index: usize, value: u32) -> Vec<u8> {
    let at = entry_at(&bytes, tag);
    let little = &bytes[..2] == b"II";
    let kind = if little {
        u16::from_le_bytes([bytes[at + 2], bytes[at + 3]])
    } else {
        u16::from_be_bytes([bytes[at + 2], bytes[at + 3]])
    };
    let size = if kind == 3 { 2 } else { 4 };
    let count = read_u32(&bytes, at + 4) as usize;
    assert!(index < count, "tag {tag} has {count} values");
    let start = if size * count <= 4 {
        at + 8
    } else {
        read_u32(&bytes, at + 8) as usize
    };
    let place = start + size * index;
    let value = number(&bytes, value, size);
    bytes[place..place + size].copy_from_slice(&value);
    bytes
}
