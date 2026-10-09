//! PNG chunk headers, bounded text and deferred EXIF/profile locations.
use super::walk::{be16, be32, inflate, Block, Walk};
use super::*;

pub(super) fn read(w: &mut Walk<'_, '_>) {
    let Some(signature) = w.read(0, 8, RasterTagPart::Container) else {
        return;
    };
    if signature != b"\x89PNG\r\n\x1a\n" {
        w.damaged(RasterTagPart::Container);
        return;
    }
    let mut offset = 8;
    let mut first = true;
    let mut seen = [false; 10];
    while offset < w.length && !w.stopped() {
        let Some(header) = w.read(offset, 8, RasterTagPart::Container) else {
            return;
        };
        let length = u64::from(be32(&header));
        let start = offset + 8;
        let Some(end) = start
            .checked_add(length)
            .and_then(|n| n.checked_add(4))
            .filter(|n| *n <= w.length)
        else {
            w.damaged(RasterTagPart::Container);
            return;
        };
        let kind = &header[4..];
        if first && kind != b"IHDR" {
            w.damaged(RasterTagPart::Container);
        }
        if kind == b"IEND" {
            return;
        }
        match kind {
            b"tEXt" | b"zTXt" | b"iTXt" => text(
                w,
                kind,
                Block {
                    offset: start,
                    length,
                },
            ),
            b"eXIf" => {
                w.exif.get_or_insert(Block {
                    offset: start,
                    length,
                });
            }
            _ => {
                let field = match kind {
                    b"IHDR" if first => Some((0, 13)),
                    b"pHYs" => Some((1, 9)),
                    b"gAMA" => Some((2, 4)),
                    b"cHRM" => Some((3, 32)),
                    b"sRGB" => Some((4, 1)),
                    b"sBIT" => Some((5, length)),
                    b"tIME" => Some((6, 7)),
                    b"acTL" => Some((7, 8)),
                    b"iCCP" => Some((8, length.min(82))),
                    _ => None,
                };
                if let Some((index, need)) = field {
                    if !seen[index] {
                        seen[index] = true;
                        if length < need || (kind == b"sBIT" && !(1..=4).contains(&length)) {
                            w.damaged(RasterTagPart::Container);
                        } else if let Some(bytes) = w.read(start, need, RasterTagPart::Container) {
                            fields(
                                w,
                                kind,
                                &bytes,
                                Block {
                                    offset: start,
                                    length,
                                },
                            );
                        }
                    }
                }
            }
        }
        first = false;
        offset = end;
    }
}

fn fields(w: &mut Walk<'_, '_>, kind: &[u8], b: &[u8], block: Block) {
    match kind {
        b"IHDR" => {
            w.number("PNG:IHDR", "Width", be32(b));
            w.number("PNG:IHDR", "Height", be32(&b[4..]));
            for (key, byte) in [
                "BitDepth",
                "ColorType",
                "Compression",
                "Filter",
                "Interlace",
            ]
            .into_iter()
            .zip(&b[8..13])
            {
                w.number("PNG:IHDR", key, *byte);
            }
        }
        b"pHYs" => {
            w.number("PNG:pHYs", "PixelsPerUnitX", be32(b));
            w.number("PNG:pHYs", "PixelsPerUnitY", be32(&b[4..]));
            w.number("PNG:pHYs", "Unit", b[8]);
        }
        b"gAMA" => w.number("PNG:gAMA", "Gamma", be32(b)),
        b"cHRM" => {
            let values: Vec<f64> = b.chunks_exact(4).map(|b| f64::from(be32(b))).collect();
            w.sink.leaf(
                TagName::Fixed("PNG:cHRM"),
                "Chromaticities",
                "",
                TagData::Numbers {
                    values: &values,
                    total: 8,
                },
            );
        }
        b"sRGB" => w.number("PNG:sRGB", "RenderingIntent", b[0]),
        b"sBIT" => {
            let values: Vec<f64> = b.iter().map(|b| f64::from(*b)).collect();
            w.sink.leaf(
                TagName::Fixed("PNG:sBIT"),
                "SignificantBits",
                "",
                TagData::Numbers {
                    values: &values,
                    total: b.len() as u64,
                },
            );
        }
        b"tIME" => {
            w.sink.leaf(
                TagName::Fixed("PNG:tIME"),
                "Time",
                "",
                TagData::Composed(format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    be16(b),
                    b[2],
                    b[3],
                    b[4],
                    b[5],
                    b[6]
                )),
            );
        }
        b"acTL" => {
            w.number("PNG:acTL", "Frames", be32(b));
            w.number("PNG:acTL", "Plays", be32(&b[4..]));
        }
        b"iCCP" => {
            let Some(nul) = b.iter().take(80).position(|b| *b == 0) else {
                w.damaged(RasterTagPart::Icc);
                return;
            };
            w.sink.leaf(
                TagName::Fixed("PNG:iCCP"),
                "ProfileName",
                "",
                TagData::Text {
                    bytes: &b[..nul],
                    encoding: TextEncoding::Latin1,
                    more: false,
                },
            );
            let skip = nul as u64 + 2;
            if skip >= block.length {
                w.damaged(RasterTagPart::Icc);
                return;
            }
            w.profile = Some(Block {
                offset: block.offset + skip,
                length: block.length - skip,
            });
            w.compressed_profile = true;
        }
        _ => {}
    }
}

fn text(w: &mut Walk<'_, '_>, kind: &[u8], block: Block) {
    let Some(head) = w.read(block.offset, block.length.min(2048), RasterTagPart::Text) else {
        return;
    };
    let Some(nul) = head.iter().take(80).position(|b| *b == 0) else {
        w.damaged(RasterTagPart::Text);
        return;
    };
    let mut skip = nul + 1;
    let (name, encoding, compressed) = match kind {
        b"tEXt" => ("PNG:tEXt", TextEncoding::Latin1, false),
        b"zTXt" => {
            skip += 1;
            ("PNG:zTXt", TextEncoding::Latin1, true)
        }
        _ => {
            if head.len() < skip + 2 {
                w.damaged(RasterTagPart::Text);
                return;
            }
            let compressed = head[skip] == 1;
            skip += 2;
            for _ in 0..2 {
                let Some(end) = head[skip..].iter().position(|b| *b == 0) else {
                    w.damaged(RasterTagPart::Text);
                    return;
                };
                skip += end + 1;
            }
            ("PNG:iTXt", TextEncoding::Utf8, compressed)
        }
    };
    if skip as u64 > block.length {
        w.damaged(RasterTagPart::Text);
        return;
    }
    let length = block.length - skip as u64;
    let limit = RASTER_TAG_VALUE_MAX_BYTES + if compressed { 64 } else { 0 };
    let Some(bytes) = w.read(
        block.offset + skip as u64,
        length.min(limit),
        RasterTagPart::Text,
    ) else {
        return;
    };
    let (mut value, more) = if compressed {
        let (out, ended) = inflate(&bytes, RASTER_TAG_VALUE_MAX_BYTES as usize + 1);
        if !bytes.is_empty() && out.is_empty() {
            w.damaged(RasterTagPart::Text);
            w.sink.leaf(
                TagName::Fixed(name),
                "Text",
                "",
                TagData::Problem(TagProblem::Unreadable),
            );
            return;
        }
        if !ended && length <= limit && out.len() <= RASTER_TAG_VALUE_MAX_BYTES as usize {
            w.damaged(RasterTagPart::Text);
        }
        let more = !ended || out.len() > RASTER_TAG_VALUE_MAX_BYTES as usize;
        (out, more)
    } else {
        (bytes, length > limit)
    };
    value.truncate(RASTER_TAG_VALUE_MAX_BYTES as usize);
    w.sink.leaf(
        TagName::Fixed(name),
        "Text",
        "",
        TagData::LabelledText {
            label: &head[..nul],
            bytes: &value,
            encoding,
            more,
        },
    );
}
