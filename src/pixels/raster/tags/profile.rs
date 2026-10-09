//! The bounded head of an ICC profile and its first description record.
use super::walk::{be32, inflate, Block, Walk};
use super::*;

pub(super) fn read(w: &mut Walk<'_, '_>, block: Block) {
    let compressed = w.compressed_profile;
    let limit = if compressed {
        RASTER_TAGS_INFLATE_MAX_BYTES + 1024
    } else {
        RASTER_TAGS_ICC_HEAD_BYTES
    };
    let Some(bytes) = w.read(
        block.offset,
        block.length.min(limit as u64),
        RasterTagPart::Icc,
    ) else {
        return;
    };
    let (head, cut) = if compressed {
        // No byte past the inflation contract is produced, even to probe
        // for the end. The decoder's status tells us whether it ended.
        let (out, ended) = inflate(&bytes, RASTER_TAGS_INFLATE_MAX_BYTES);
        (out, !ended || block.length > limit as u64)
    } else {
        (bytes, block.length > limit as u64)
    };
    if head.len() < 128 || &head[36..40] != b"acsp" {
        w.damaged(RasterTagPart::Icc);
        return;
    }
    w.number("ICC", "Size", be32(&head));
    w.sink.leaf(
        TagName::Fixed("ICC"),
        "Version",
        "",
        TagData::Composed(format!("{}.{}.{}", head[8], head[9] >> 4, head[9] & 15)),
    );
    for (key, bytes) in [("Class", &head[12..16]), ("ColorSpace", &head[16..20])] {
        w.sink.leaf(
            TagName::Fixed("ICC"),
            key,
            "",
            TagData::Text {
                bytes,
                encoding: TextEncoding::Latin1,
                more: false,
            },
        );
    }
    if w.stopped() {
        return;
    }
    let Some(count) = head.get(128..132).map(be32) else {
        missing(w, cut);
        return;
    };
    let table = &head[132..];
    // At most the table entries actually in the bounded head are visited.
    for entry in table.chunks_exact(12).take(count as usize) {
        if &entry[..4] != b"desc" {
            continue;
        }
        let offset = u64::from(be32(&entry[4..]));
        let length = u64::from(be32(&entry[8..]));
        let Some(end) = offset
            .checked_add(length)
            .filter(|end| *end <= head.len() as u64)
        else {
            missing(w, cut);
            return;
        };
        description(w, &head[offset as usize..end as usize], cut);
        return;
    }
    if u64::from(count) > (table.len() / 12) as u64 {
        missing(w, cut);
    }
}

fn missing(w: &mut Walk<'_, '_>, cut: bool) {
    if w.stopped() {
        return;
    }
    if cut {
        w.sink.note(RasterTagNote::Limit(RasterTagLimit::Inflate));
    } else {
        w.damaged(RasterTagPart::Icc);
    }
}

fn description(w: &mut Walk<'_, '_>, data: &[u8], cut: bool) {
    let (text, encoding) = if data.starts_with(b"desc") && data.len() >= 12 {
        let length = (be32(&data[8..]) as usize).min(data.len() - 12);
        (&data[12..12 + length], TextEncoding::Utf8)
    } else if data.starts_with(b"mluc") && data.len() >= 28 && be32(&data[8..]) > 0 {
        let length = u64::from(be32(&data[20..]));
        let offset = u64::from(be32(&data[24..]));
        let Some(end) = offset
            .checked_add(length)
            .filter(|end| *end <= data.len() as u64)
        else {
            missing(w, cut);
            return;
        };
        (&data[offset as usize..end as usize], TextEncoding::Utf16Be)
    } else {
        w.damaged(RasterTagPart::Icc);
        return;
    };
    let shown = text.len().min(RASTER_TAG_VALUE_MAX_BYTES as usize);
    w.sink.leaf(
        TagName::Fixed("ICC"),
        "Description",
        "",
        TagData::Text {
            bytes: &text[..shown],
            encoding,
            more: shown < text.len(),
        },
    );
}
