//! RIFF chunk metadata, stepping over every image after the first header.
use super::walk::{le16, le32, Block, Walk};
use super::*;

pub(super) fn read(w: &mut Walk<'_, '_>) {
    let part = RasterTagPart::Container;
    let Some(header) = w.read(0, 12, part) else {
        return;
    };
    if &header[..4] != b"RIFF" || &header[8..] != b"WEBP" {
        w.damaged(part);
        return;
    }
    let mut offset = 12;
    let mut animation = false;
    while w.length.saturating_sub(offset) >= 8 && !w.stopped() {
        let Some(header) = w.read(offset, 8, part) else {
            return;
        };
        let length = u64::from(le32(&header[4..]));
        let start = offset + 8;
        let Some(end) = start.checked_add(length).filter(|end| *end <= w.length) else {
            w.damaged(part);
            return;
        };
        let block = Block {
            offset: start,
            length,
        };
        let kind = &header[..4];
        let needed = match kind {
            b"VP8X" | b"VP8 " if offset == 12 => Some(10),
            b"VP8L" if offset == 12 => Some(5),
            b"ANIM" if !animation => {
                animation = true;
                Some(6)
            }
            _ => None,
        };
        if let Some(needed) = needed {
            if length < needed {
                w.damaged(part);
            } else if let Some(b) = w.read(start, needed, part) {
                match kind {
                    b"VP8X" => {
                        let u24 = |b: &[u8]| {
                            u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16)
                        };
                        w.number("WEBP:VP8X", "Flags", b[0]);
                        w.number("WEBP:VP8X", "CanvasWidth", u24(&b[4..]) + 1);
                        w.number("WEBP:VP8X", "CanvasHeight", u24(&b[7..]) + 1);
                    }
                    b"VP8 " if b[3..6] == [0x9d, 1, 0x2a] => {
                        w.number("WEBP:VP8", "Width", le16(&b[6..]) & 0x3fff);
                        w.number("WEBP:VP8", "Height", le16(&b[8..]) & 0x3fff);
                    }
                    b"VP8L" if b[0] == 0x2f => {
                        let packed = le32(&b[1..]);
                        w.number("WEBP:VP8L", "Width", (packed & 0x3fff) + 1);
                        w.number("WEBP:VP8L", "Height", ((packed >> 14) & 0x3fff) + 1);
                    }
                    b"ANIM" => w.number("WEBP:ANIM", "LoopCount", le16(&b[4..])),
                    _ => w.damaged(part),
                }
            }
        }
        match kind {
            b"EXIF" if w.exif.is_none() => {
                let Some(prefix) = w.read(start, length.min(6), RasterTagPart::Exif) else {
                    return;
                };
                let skip = if prefix == b"Exif\0\0" { 6 } else { 0 };
                w.exif = Some(Block {
                    offset: start + skip,
                    length: length - skip,
                });
            }
            b"XMP " => {
                w.xmp.get_or_insert(block);
            }
            b"ICCP" => {
                w.profile.get_or_insert(block);
            }
            _ => {}
        }
        let Some(next) = end.checked_add(length % 2) else {
            return;
        };
        offset = next;
    }
}
