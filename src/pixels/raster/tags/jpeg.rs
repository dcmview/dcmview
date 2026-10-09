//! JPEG markers before the first scan; only the first metadata blocks.
use super::walk::{be16, Block, Walk};
use super::*;

pub(super) fn read(w: &mut Walk<'_, '_>) {
    let part = RasterTagPart::Container;
    let Some(signature) = w.read(0, 2, part) else {
        return;
    };
    if signature != [0xff, 0xd8] {
        w.damaged(part);
        return;
    }
    let mut offset = 2;
    let (mut jfif, mut adobe, mut frame) = (false, false, false);
    while offset < w.length && !w.stopped() {
        // One small lookahead per marker, including the identifiers needed
        // to decide whether an application segment is metadata we show.
        let Some(head) = w.read(offset, (w.length - offset).min(36), part) else {
            return;
        };
        if head.len() < 2 || head[0] != 0xff {
            w.damaged(part);
            return;
        }
        let marker = head[1];
        match marker {
            0xff => {
                offset += 1;
                continue;
            }
            0x01 | 0xd0..=0xd8 => {
                offset += 2;
                continue;
            }
            0xd9 | 0xda => return,
            0 => {
                w.damaged(part);
                return;
            }
            _ => {}
        }
        if head.len() < 4 {
            w.damaged(part);
            return;
        }
        let size = u64::from(be16(&head[2..]));
        if size < 2 {
            w.damaged(part);
            return;
        }
        let Some(end) = offset.checked_add(2 + size).filter(|end| *end <= w.length) else {
            w.damaged(part);
            return;
        };
        let block = Block {
            offset: offset + 4,
            length: size - 2,
        };
        let b = &head[4..head.len().min(4 + block.length as usize)];
        match marker {
            0xe0 if !jfif && b.starts_with(b"JFIF\0") => {
                jfif = true;
                if b.len() < 14 {
                    w.damaged(part);
                } else {
                    w.sink.leaf(
                        TagName::Fixed("JPEG:JFIF"),
                        "Version",
                        "",
                        TagData::Composed(format!("{}.{:02}", b[5], b[6])),
                    );
                    w.number("JPEG:JFIF", "Units", b[7]);
                    w.number("JPEG:JFIF", "XDensity", be16(&b[8..]));
                    w.number("JPEG:JFIF", "YDensity", be16(&b[10..]));
                }
            }
            0xe1 if b.starts_with(b"Exif\0\0") => {
                w.exif.get_or_insert(Block {
                    offset: block.offset + 6,
                    length: block.length - 6,
                });
            }
            0xe1 if b.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => {
                let skip = b"http://ns.adobe.com/xap/1.0/\0".len() as u64;
                w.xmp.get_or_insert(Block {
                    offset: block.offset + skip,
                    length: block.length - skip,
                });
            }
            0xe2 if b.len() >= 14 && b.starts_with(b"ICC_PROFILE\0") && b[12] == 1 => {
                w.profile.get_or_insert(Block {
                    offset: block.offset + 14,
                    length: block.length - 14,
                });
            }
            0xee if !adobe && b.starts_with(b"Adobe") => {
                adobe = true;
                if b.len() < 12 {
                    w.damaged(part);
                } else {
                    w.number("JPEG:Adobe", "Version", be16(&b[5..]));
                    w.number("JPEG:Adobe", "Transform", b[11]);
                }
            }
            0xfe => {
                if let Some(bytes) = w.read(
                    block.offset,
                    block.length.min(RASTER_TAG_VALUE_MAX_BYTES),
                    part,
                ) {
                    w.sink.leaf(
                        TagName::Fixed("JPEG:COM"),
                        "Comment",
                        "",
                        TagData::Text {
                            bytes: &bytes,
                            encoding: TextEncoding::Utf8,
                            more: block.length > RASTER_TAG_VALUE_MAX_BYTES,
                        },
                    );
                }
            }
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) && !frame => {
                frame = true;
                if b.len() < 6 {
                    w.damaged(part);
                } else {
                    for (key, value) in [
                        ("Precision", u16::from(b[0])),
                        ("Height", be16(&b[1..])),
                        ("Width", be16(&b[3..])),
                        ("Components", u16::from(b[5])),
                    ] {
                        w.sink.leaf(
                            TagName::FrameHeader(marker - 0xc0),
                            key,
                            "",
                            TagData::Number(f64::from(value)),
                        );
                    }
                }
            }
            _ => {}
        }
        offset = end;
    }
}
