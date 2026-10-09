//! Exact-span reads and the small amount of state retained between parts.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Block {
    pub offset: u64,
    pub length: u64,
}

impl Block {
    pub fn contains(self, offset: u64, length: u64) -> bool {
        offset
            .checked_add(length)
            .is_some_and(|end| end <= self.length)
    }
}

pub(super) struct Walk<'a, 'b> {
    pub reader: &'a mut Reader<'b>,
    pub length: u64,
    pub sink: &'a mut TagSink,
    failed: bool,
    pub exif: Option<Block>,
    pub xmp: Option<Block>,
    pub profile: Option<Block>,
    pub compressed_profile: bool,
}

impl<'a, 'b> Walk<'a, 'b> {
    pub fn new(reader: &'a mut Reader<'b>, length: u64, sink: &'a mut TagSink) -> Self {
        Self {
            reader,
            length,
            sink,
            failed: false,
            exif: None,
            xmp: None,
            profile: None,
            compressed_profile: false,
        }
    }

    pub fn stopped(&self) -> bool {
        self.failed || self.sink.refused()
    }

    pub fn damaged(&mut self, part: RasterTagPart) {
        if !self.stopped() {
            self.sink.note(RasterTagNote::Damaged(part));
        }
    }

    /// All file access passes here. Check the range before allocating, and
    /// distinguish absent bytes from an in-range read refused by the source.
    pub fn read(&mut self, offset: u64, length: u64, part: RasterTagPart) -> Option<Vec<u8>> {
        let end = offset.checked_add(length).filter(|end| *end <= self.length);
        if end.is_none() || length > (RASTER_TAGS_INFLATE_MAX_BYTES + 1024) as u64 {
            self.damaged(part);
            return None;
        }
        if self.stopped() {
            return None;
        }
        let mut bytes = vec![0; length as usize];
        self.reader.read_span(offset..end?);
        if self
            .reader
            .seek(SeekFrom::Start(offset))
            .and_then(|_| self.reader.read_exact(&mut bytes))
            .is_err()
        {
            self.failed = true;
            return None;
        }
        Some(bytes)
    }

    pub fn number(&mut self, tag: &'static str, keyword: &'static str, value: impl Into<f64>) {
        self.sink.leaf(
            TagName::Fixed(tag),
            keyword,
            "",
            TagData::Number(value.into()),
        );
    }
}

pub(super) fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}
pub(super) fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
pub(super) fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
pub(super) fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// A fixed output buffer also bounds inflation when the stream is incomplete.
pub(super) fn inflate(bytes: &[u8], capacity: usize) -> (Vec<u8>, bool) {
    let mut decoder = flate2::Decompress::new(true);
    let mut out = vec![0; capacity];
    let ended = matches!(
        decoder.decompress(bytes, &mut out, flate2::FlushDecompress::Finish),
        Ok(flate2::Status::StreamEnd)
    );
    out.truncate(decoder.total_out() as usize);
    (out, ended)
}
