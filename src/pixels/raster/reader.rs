//! A single read-ahead window and a read budget, shared by every codec.
//!
//! The budget is charged for every byte a decoder is handed: each byte a
//! read obtains from the file, read-ahead included, and each buffered byte
//! handed out a second time after a seek back. So neither reading the file
//! nor running a decoder over bytes it already saw is free, and both end at
//! the same limit.
use super::{RasterSource, RASTER_READ_BUFFER_BYTES};
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::ops::Range;

pub(super) struct Reader<'a> {
    source: &'a mut dyn RasterSource,
    buffer: Vec<u8>,
    start: u64,
    buffered: usize,
    position: u64,
    length: u64,
    remaining: u64,
    /// The buffered bytes before this position have been handed out once;
    /// handing them out again is charged again.
    served: u64,
    /// Ranges of the file that are read whole and once, sorted by start
    /// ([`Reader::read_spans`]).
    spans: Vec<Range<u64>>,
}

impl<'a> Reader<'a> {
    pub(super) fn new(source: &'a mut dyn RasterSource, length: u64, budget: u64) -> Self {
        Self {
            source,
            buffer: vec![0; RASTER_READ_BUFFER_BYTES],
            start: 0,
            buffered: 0,
            position: 0,
            length,
            remaining: budget,
            served: 0,
            spans: Vec::new(),
        }
    }

    /// Names the ranges of the file a decoder is about to read from start to
    /// end, one after another in any order: the runs of a TIFF page's strips
    /// or tiles. A read that begins inside one never reads ahead past its
    /// end, so reading every range once costs the bytes of the ranges,
    /// wherever they lie in the file. Reads outside every range ask for a
    /// whole buffer, as before.
    pub(super) fn read_spans(&mut self, mut spans: Vec<Range<u64>>) {
        spans.retain(|span| span.start < span.end);
        spans.sort_unstable_by_key(|span| span.start);
        self.spans = spans;
    }

    /// The most a read at `position` may ask for: to the end of the span
    /// that holds it, or a whole buffer.
    fn read_ahead(&self, position: u64) -> u64 {
        let after = self.spans.partition_point(|span| span.start <= position);
        match after.checked_sub(1).map(|index| &self.spans[index]) {
            Some(span) if position < span.end => span.end - position,
            _ => RASTER_READ_BUFFER_BYTES as u64,
        }
    }

    fn exhausted() -> io::Error {
        io::Error::other("raster read budget exhausted")
    }
}

impl BufRead for Reader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.position >= self.length {
            return Ok(&[]);
        }
        let end = self.start + self.buffered as u64;
        if self.position < self.start || self.position >= end {
            let count = (RASTER_READ_BUFFER_BYTES as u64)
                .min(self.length - self.position)
                .min(self.read_ahead(self.position))
                .min(self.remaining) as usize;
            if count == 0 {
                return Err(Self::exhausted());
            }
            self.source.seek(SeekFrom::Start(self.position))?;
            self.buffered = self.source.read(&mut self.buffer[..count])?;
            self.remaining -= self.buffered as u64;
            self.start = self.position;
            self.served = self.position;
            return Ok(&self.buffer[..self.buffered]);
        }
        let from = (self.position - self.start) as usize;
        if self.position < self.served {
            // Bytes handed out before: only as many as the budget still
            // covers, and `consume` charges them.
            if self.remaining == 0 {
                return Err(Self::exhausted());
            }
            let again = (self.served - self.position).min(self.remaining) as usize;
            return Ok(&self.buffer[from..from + again]);
        }
        Ok(&self.buffer[from..self.buffered])
    }

    fn consume(&mut self, amount: usize) {
        let end = self.start + self.buffered as u64;
        let amount = (amount as u64).min(end.saturating_sub(self.position));
        let again = self.served.saturating_sub(self.position).min(amount);
        self.remaining = self.remaining.saturating_sub(again);
        if amount > 0 {
            self.position += amount;
            self.served = self.served.max(self.position);
        }
    }
}

impl Read for Reader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let count = out.len().min(available.len());
        out[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }

    fn read_to_end(&mut self, out: &mut Vec<u8>) -> io::Result<usize> {
        let count = usize::try_from(self.length.saturating_sub(self.position))
            .map_err(|_| io::Error::other("raster length exceeds address space"))?;
        out.try_reserve_exact(count).map_err(io::Error::other)?;
        let original = out.len();
        loop {
            let available = self.fill_buf()?;
            if available.is_empty() {
                return Ok(out.len() - original);
            }
            let consumed = available.len();
            out.extend_from_slice(available);
            self.consume(consumed);
        }
    }
}

impl Seek for Reader<'_> {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        let position = match seek {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
        };
        self.position = position.ok_or_else(|| io::Error::other("invalid raster seek"))?;
        Ok(self.position)
    }
}
