//! TIFF pages and embedded EXIF share one block-relative directory walker.
use super::names::{ifd_tag_keyword, ifd_type_name, ifd_type_size, IfdKind};
use super::walk::{be16, le16, Block, Walk};
use super::*;

#[derive(Clone, Copy)]
struct Layout {
    block: Block,
    little: bool,
    big: bool,
}

impl Layout {
    fn uint(self, bytes: &[u8]) -> u64 {
        if self.little {
            bytes.iter().rev().fold(0, |n, b| (n << 8) | u64::from(*b))
        } else {
            bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b))
        }
    }

    fn read(
        self,
        w: &mut Walk<'_, '_>,
        offset: u64,
        length: u64,
        part: RasterTagPart,
    ) -> Option<Vec<u8>> {
        if !self.block.contains(offset, length) {
            w.damaged(part);
            return None;
        }
        w.read(self.block.offset.checked_add(offset)?, length, part)
    }
}

#[derive(Default)]
struct Links {
    next: u64,
    exif: u64,
    gps: u64,
    interop: u64,
    profile: Option<Block>,
}

struct Directories {
    layout: Layout,
    visited: Vec<u64>,
}

impl Directories {
    fn directory(
        &mut self,
        w: &mut Walk<'_, '_>,
        offset: u64,
        kind: IfdKind,
        part: RasterTagPart,
    ) -> Option<Links> {
        if w.stopped() {
            return None;
        }
        if self.visited.contains(&offset) {
            w.damaged(part);
            return None;
        }
        if self.visited.len() == RASTER_TAGS_MAX_IFDS {
            w.sink
                .note(RasterTagNote::Limit(RasterTagLimit::Directories));
            return None;
        }
        self.visited.push(offset);
        let l = self.layout;
        let (count_size, entry_size, field_size) = if l.big { (8, 20, 8) } else { (2, 12, 4) };
        let count = l.uint(&l.read(w, offset, count_size, part)?);
        let start = offset + count_size;
        let shown = count.min(RASTER_TAGS_MAX_IFD_ENTRIES as u64);
        if shown < count {
            w.sink.note(RasterTagNote::Limit(RasterTagLimit::Entries));
        }
        let fits = ((l.block.length - start) / entry_size).min(shown);
        if fits < shown {
            w.damaged(part);
        }
        let table = l.read(w, start, fits * entry_size, part)?;
        let mut links = Links::default();
        for entry in table.chunks_exact(entry_size as usize) {
            if w.stopped() {
                return None;
            }
            let tag = l.uint(&entry[..2]) as u16;
            let code = l.uint(&entry[2..4]) as u16;
            let field_at = entry.len() - field_size as usize;
            let count = l.uint(&entry[4..field_at]);
            let field = &entry[field_at..];
            let leaf = |w: &mut Walk<'_, '_>, data| {
                w.sink.leaf(
                    TagName::Entry(tag),
                    ifd_tag_keyword(kind, tag),
                    ifd_type_name(code),
                    data,
                );
            };
            let Some(size) = ifd_type_size(code) else {
                leaf(w, TagData::Problem(TagProblem::UnknownType));
                continue;
            };
            let Some(length) = count.checked_mul(size) else {
                w.damaged(part);
                leaf(w, TagData::Problem(TagProblem::OutsideFile));
                continue;
            };
            let value_offset = l.uint(field);
            if length > field_size && !l.block.contains(value_offset, length) {
                w.damaged(part);
                leaf(w, TagData::Problem(TagProblem::OutsideFile));
                continue;
            }
            if kind == IfdKind::Image
                && tag == 34675
                && length > field_size
                && links.profile.is_none()
            {
                links.profile = Some(Block {
                    offset: l.block.offset + value_offset,
                    length,
                });
            }
            let encoding = match (kind, tag, code) {
                (_, 700, 1 | 7) => Some(TextEncoding::Utf8),
                (_, 40091..=40095, 1) => Some(TextEncoding::Utf16Le),
                (IfdKind::Exif, 36864 | 40960, _) => Some(TextEncoding::Latin1),
                (_, _, 2) => Some(TextEncoding::Utf8),
                _ => None,
            };
            let comment = kind == IfdKind::Exif && tag == 37510;
            if matches!(tag, 33723 | 34377 | 37500) || (code == 7 && encoding.is_none() && !comment)
            {
                leaf(w, TagData::Binary { length });
                continue;
            }
            let needed = if encoding.is_some() || comment {
                length.min(RASTER_TAG_VALUE_MAX_BYTES)
            } else {
                count.min(RASTER_TAG_NUMBERS_MAX as u64) * size
            };
            let owned;
            let bytes = if length <= field_size {
                &field[..length as usize]
            } else {
                owned = l.read(w, value_offset, needed, part)?;
                &owned
            };
            if count == 1 && matches!(code, 4 | 13 | 16 | 18) {
                let pointer = l.uint(bytes);
                match (kind, tag) {
                    (IfdKind::Image, 34665) => links.exif = pointer,
                    (IfdKind::Image, 34853) => links.gps = pointer,
                    (IfdKind::Exif, 40965) => links.interop = pointer,
                    _ => {}
                }
            }
            if comment {
                if bytes.starts_with(b"ASCII\0\0\0") {
                    leaf(
                        w,
                        TagData::Text {
                            bytes: &bytes[8..],
                            encoding: TextEncoding::Utf8,
                            more: length > needed,
                        },
                    );
                } else {
                    leaf(w, TagData::Binary { length });
                }
            } else if let Some(encoding) = encoding {
                leaf(
                    w,
                    TagData::Text {
                        bytes,
                        encoding,
                        more: length > needed,
                    },
                );
            } else if matches!(code, 5 | 10) {
                let mut values = [(0, 0); RASTER_TAG_NUMBERS_MAX];
                let mut n = 0;
                for pair in bytes.chunks_exact(8).take(RASTER_TAG_NUMBERS_MAX) {
                    let signed = |b| {
                        let v = l.uint(b);
                        if code == 10 {
                            i64::from(v as i32)
                        } else {
                            v as i64
                        }
                    };
                    values[n] = (signed(&pair[..4]), signed(&pair[4..]));
                    n += 1;
                }
                leaf(
                    w,
                    TagData::Rationals {
                        values: &values[..n],
                        total: count,
                    },
                );
            } else {
                let mut values = [0.0; RASTER_TAG_NUMBERS_MAX];
                let mut n = 0;
                for b in bytes
                    .chunks_exact(size as usize)
                    .take(RASTER_TAG_NUMBERS_MAX)
                {
                    let v = l.uint(b);
                    values[n] = match code {
                        6 => f64::from(v as i8),
                        8 => f64::from(v as i16),
                        9 => f64::from(v as i32),
                        17 => (v as i64) as f64,
                        11 => f64::from(f32::from_bits(v as u32)),
                        12 => f64::from_bits(v),
                        _ => v as f64,
                    };
                    n += 1;
                }
                leaf(
                    w,
                    TagData::Numbers {
                        values: &values[..n],
                        total: count,
                    },
                );
            }
        }
        if w.stopped() {
            return None;
        }
        let next_at = count
            .checked_mul(entry_size)
            .and_then(|n| start.checked_add(n));
        match next_at {
            Some(next_at) => {
                if let Some(bytes) = l.read(w, next_at, field_size, part) {
                    links.next = l.uint(&bytes);
                }
            }
            None => w.damaged(part),
        }
        Some(links)
    }

    fn group(
        &mut self,
        w: &mut Walk<'_, '_>,
        name: &'static str,
        offset: u64,
        kind: IfdKind,
    ) -> Option<Links> {
        if offset == 0 || w.stopped() || !w.sink.open(TagName::Fixed(name)) {
            return None;
        }
        let links = self.directory(w, offset, kind, RasterTagPart::Exif);
        w.sink.close();
        links
    }

    fn children(&mut self, w: &mut Walk<'_, '_>, links: &Links) {
        let interop = self
            .group(w, "Exif", links.exif, IfdKind::Exif)
            .map_or(0, |links| links.interop);
        self.group(w, "GPS", links.gps, IfdKind::Gps);
        self.group(w, "Interop", interop, IfdKind::Interop);
    }
}

fn header(
    w: &mut Walk<'_, '_>,
    block: Block,
    part: RasterTagPart,
    allow_big: bool,
) -> Option<(Layout, u64)> {
    if !block.contains(0, 8) {
        w.damaged(part);
        return None;
    }
    let b = w.read(block.offset, 8, part)?;
    let little = match &b[..2] {
        b"II" => true,
        b"MM" => false,
        _ => {
            w.damaged(part);
            return None;
        }
    };
    let magic = if little { le16(&b[2..]) } else { be16(&b[2..]) };
    let mut layout = Layout {
        block,
        little,
        big: false,
    };
    let offset = match magic {
        42 => layout.uint(&b[4..]),
        43 if allow_big && layout.uint(&b[4..6]) == 8 => {
            layout.big = true;
            layout.uint(&layout.read(w, 8, 8, part)?)
        }
        _ => {
            w.damaged(part);
            return None;
        }
    };
    Some((layout, offset))
}

pub(super) fn tiff(w: &mut Walk<'_, '_>) {
    let block = Block {
        offset: 0,
        length: w.length,
    };
    let Some((layout, mut offset)) = header(w, block, RasterTagPart::Container, true) else {
        return;
    };
    let mut directories = Directories {
        layout,
        visited: Vec::new(),
    };
    let mut page = 0;
    while offset != 0 && !w.stopped() {
        if page == RASTER_TAGS_MAX_PAGES {
            w.sink.note(RasterTagNote::Limit(RasterTagLimit::Pages));
            return;
        }
        if !w.sink.open(TagName::Page(page as u32)) {
            return;
        }
        let links = directories.directory(w, offset, IfdKind::Image, RasterTagPart::Container);
        if let Some(links) = &links {
            if page == 0 {
                w.profile = links.profile;
            }
            directories.children(w, links);
        }
        w.sink.close();
        let Some(links) = links else { return };
        offset = links.next;
        page += 1;
    }
}

pub(super) fn exif(w: &mut Walk<'_, '_>, block: Block) {
    let Some((layout, offset)) = header(w, block, RasterTagPart::Exif, false) else {
        return;
    };
    if w.stopped() || !w.sink.open(TagName::Fixed("EXIF")) {
        return;
    }
    let mut directories = Directories {
        layout,
        visited: Vec::new(),
    };
    if let Some(links) = directories.group(w, "IFD0", offset, IfdKind::Image) {
        directories.children(w, &links);
        directories.group(w, "IFD1", links.next, IfdKind::Image);
    }
    w.sink.close();
}
