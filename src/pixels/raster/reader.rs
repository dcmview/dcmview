//! A single read-ahead window and an acquisition budget, shared by every codec.
use super::{RasterSource, RASTER_READ_BUFFER_BYTES};
use std::io::{self, BufRead, Read, Seek, SeekFrom};

pub(super) struct Reader<'a> {
    source: &'a mut dyn RasterSource,
    buffer: Vec<u8>,
    start: u64,
    buffered: usize,
    position: u64,
    length: u64,
    remaining: u64,
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
        }
    }
}

impl BufRead for Reader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.position >= self.length {
            return Ok(&[]);
        }
        if self.position < self.start || self.position >= self.start + self.buffered as u64 {
            let count = (RASTER_READ_BUFFER_BYTES as u64)
                .min(self.length - self.position)
                .min(self.remaining) as usize;
            if count == 0 {
                return Err(io::Error::other("raster read budget exhausted"));
            }
            self.source.seek(SeekFrom::Start(self.position))?;
            self.buffered = self.source.read(&mut self.buffer[..count])?;
            self.remaining -= self.buffered as u64;
            self.start = self.position;
        }
        Ok(&self.buffer[(self.position - self.start) as usize..self.buffered])
    }

    fn consume(&mut self, amount: usize) {
        self.position += amount
            .min((self.start + self.buffered as u64).saturating_sub(self.position) as usize)
            as u64;
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
