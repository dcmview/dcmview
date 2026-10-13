//! Pixel digests: the `px:` digest of each frame the viewer has decoded
//! (`docs/design/annotation-model.md` 1.6), and the session's table of
//! them.
//!
//! What a digest is a digest of is defined once, in
//! [`dcmview_annotation::pixel`]. This module maps a raw frame onto that
//! definition, hashes it, and keeps what was hashed.
//!
//! # When a digest is computed
//!
//! Only as part of a raw decode ([`super::service::load_raw_frame`] and the
//! raw decodes a display frame, an overlay or a legend make), from the
//! samples that decode produced, before the decode ends: under the permit
//! the decode runs under, and so within the estimate that admitted it. A
//! frame the raw cache answers is not hashed again. Nothing else computes
//! one: not discovery, not the catalog, not a thumbnail, not a display
//! frame that never reaches the raw tier, not an export and not an import.
//! A frame nobody decoded has no digest.
//!
//! # What it costs
//!
//! One pass over the frame's samples on a blocking thread, with no copy of
//! the frame: at most [`PIXEL_DIGEST_BUFFER_BYTES`] of heap beside the
//! samples, whatever their layout. A frame that has no digest
//! ([`exact_samples`]) is not read at all.
//!
//! # What is kept
//!
//! 32 bytes a frame, in a table of at most [`PIXEL_DIGEST_CAPACITY`] frames
//! that holds at most [`PIXEL_DIGESTS_MAX_BYTES`] of heap however many
//! files a session lists and however many frames one of them has. A table
//! nothing was recorded in holds no heap.

use crate::api::contracts::RawFrameMetadata;
use crate::types::{FileEntry, FileFormat, NativePixelDataKind};
use bytes::Bytes;
use dcmview_annotation::{PixelLayout, SampleFormat, PIXEL_DIGEST_HEADER_BYTES};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use super::syntax::{codec_for_file, codec_for_syntax, Codec};

/// The most frames a session keeps a pixel digest of: 65,536. Recording one
/// more forgets the one recorded longest ago.
pub const PIXEL_DIGEST_CAPACITY: usize = 65_536;

/// The most frames a file may have and state pixel digests in a document:
/// 65,536, the most frames a frame set names
/// (`dcmview_annotation::limits::MAX_FRAMES_IN_SET`). It bounds the list a
/// document holds for one file.
pub const PIXEL_DIGEST_FRAMES_STATED: usize = dcmview_annotation::limits::MAX_FRAMES_IN_SET;

/// The most heap a [`PixelDigests`] holds, at capacity: 16 MiB.
pub const PIXEL_DIGESTS_MAX_BYTES: usize = 16 * 1024 * 1024;

/// The most heap hashing one frame takes beside the frame itself: 64 KiB,
/// for the buffer a frame stored colour-by-plane is put in order through.
pub const PIXEL_DIGEST_BUFFER_BYTES: usize = 64 * 1024;

/// What a session's pixel digests have cost and hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelDigestStats {
    /// Digests computed since the session began. A decode whose frame has
    /// no digest counts nothing.
    pub frames_hashed: u64,
    /// The bytes those digests were computed over: for each, the 24 bytes
    /// of its header and its samples.
    pub bytes_hashed: u64,
    /// Frames the table holds a digest of now, at most
    /// [`PIXEL_DIGEST_CAPACITY`].
    pub held: usize,
}

/// The pixel digests of one session, by catalog index and frame.
///
/// A session has one, held by its `FileRegistry`
/// (`FileRegistry::pixel_digests`) and named by its raw frame cache
/// (`BudgetedLru::with_pixel_digests`), which is how a raw decode finds it.
///
/// - **Bounded.** At most [`PIXEL_DIGEST_CAPACITY`] frames. Recording a
///   frame the table does not hold when it is full forgets the frame whose
///   digest was first recorded longest ago; recording a frame it holds
///   replaces the digest and forgets nothing. The heap it holds never
///   passes [`PIXEL_DIGESTS_MAX_BYTES`], and is none before the first
///   record.
/// - **The last decode wins.** A file may change while a session runs; the
///   digest of a frame is the one its latest raw decode produced.
/// - **Never a refusal.** The table holds digests only. A frame whose digest
///   could not be computed is absent, and the next decode of it tries again.
/// - **Never from outside.** Only a raw decode of this session records
///   ([`record_frame_digest`]). A digest a document states is compared with
///   the table and never put into it.
///
/// Every method takes the table's lock for a constant amount of work
/// ([`PixelDigests::of_frames`]: for one lookup a frame and file it is
/// asked about), and none blocks on anything else. A poisoned lock reads
/// as an empty table and records nothing; no method panics.
pub struct PixelDigests {
    state: Mutex<DigestTable>,
}

/// What the table's lock guards.
#[derive(Default)]
struct DigestTable {
    digests: HashMap<(usize, u32), [u8; 32]>,
    order: VecDeque<(usize, u32)>,
    frames_hashed: u64,
    bytes_hashed: u64,
}

impl Default for PixelDigests {
    fn default() -> Self {
        Self::new()
    }
}

impl PixelDigests {
    /// An empty table. Allocates nothing.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(DigestTable::default()),
        }
    }

    /// The digest of frame `frame` of the file at catalog index `file`, when
    /// the table holds one.
    pub fn get(&self, file: usize, frame: u32) -> Option<[u8; 32]> {
        self.state.lock().ok()?.digests.get(&(file, frame)).copied()
    }

    /// Records `digest` as the digest of frame `frame` of the file at
    /// catalog index `file`, computed over `hashed_bytes` bytes, and counts
    /// it in [`PixelDigestStats::frames_hashed`] and
    /// [`PixelDigestStats::bytes_hashed`].
    pub fn record(&self, file: usize, frame: u32, digest: [u8; 32], hashed_bytes: u64) {
        let Ok(mut table) = self.state.lock() else {
            return;
        };
        table.frames_hashed = table.frames_hashed.saturating_add(1);
        table.bytes_hashed = table.bytes_hashed.saturating_add(hashed_bytes);
        let key = (file, frame);
        if let Some(held) = table.digests.get_mut(&key) {
            *held = digest;
            return;
        }
        if table.digests.len() == PIXEL_DIGEST_CAPACITY {
            if let Some(oldest) = table.order.pop_front() {
                table.digests.remove(&oldest);
            }
        }
        table.order.push_back(key);
        table.digests.insert(key, digest);
    }

    /// What the table has counted and holds.
    pub fn stats(&self) -> PixelDigestStats {
        let Ok(table) = self.state.lock() else {
            return PixelDigestStats::default();
        };
        PixelDigestStats {
            frames_hashed: table.frames_hashed,
            bytes_hashed: table.bytes_hashed,
            held: table.digests.len(),
        }
    }

    /// The pixel digests a document states for a file of `frames` frames
    /// whose loaded copies are at the catalog indexes `files`: for each
    /// frame below `frames`, in order, the digest of the first of `files`
    /// the table holds one for, as [`dcmview_annotation::pixel_digest_text`]
    /// writes it, and `None` when it holds none for any of them.
    ///
    /// Empty, and no allocation, when the table holds no digest of any
    /// frame below `frames` of any of `files`, and when `frames` is more
    /// than [`PIXEL_DIGEST_FRAMES_STATED`]: a file of more frames than that
    /// states none, so a list is never longer than that, whatever a file's
    /// header says. Otherwise exactly `frames` entries. The work is one
    /// lookup for each frame and file.
    pub fn of_frames(&self, files: &[usize], frames: u32) -> Vec<Option<String>> {
        let Ok(table) = self.state.lock() else {
            return Vec::new();
        };
        if table.digests.is_empty() || frames as usize > PIXEL_DIGEST_FRAMES_STATED {
            return Vec::new();
        }
        let digest_of = |frame| {
            files
                .iter()
                .find_map(|file| table.digests.get(&(*file, frame)))
        };
        if !(0..frames).any(|frame| digest_of(frame).is_some()) {
            return Vec::new();
        }
        (0..frames)
            .map(|frame| digest_of(frame).map(dcmview_annotation::pixel_digest_text))
            .collect()
    }
}

/// Whether decoding a frame of `file` gives exactly the samples the file
/// stores, so that its frames have pixel digests.
///
/// | File | Exact |
/// |---|---|
/// | DICOM, uncompressed in any byte order, deflated, Deflated Image Frame, RLE, JPEG Lossless, JPEG-LS Lossless, JPEG 2000 Lossless Only, JPEG XL Lossless | yes |
/// | DICOM, JPEG Baseline | no |
/// | DICOM in a transfer syntax the viewer does not decode | no |
/// | PNG | yes |
/// | TIFF | yes, unless its alpha is associated (`RasterMetadata::alpha_associated`) |
/// | JPEG, WebP | no |
///
/// It reads the catalog entry only (`pixels::codec_for_file`,
/// `FileEntry::format`, `FileEntry::raster`), never the file.
pub fn exact_samples(file: &FileEntry) -> bool {
    if file.format.is_raster() {
        return match file.format {
            FileFormat::Png => true,
            FileFormat::Tiff => file
                .raster
                .as_ref()
                .is_none_or(|raster| !raster.alpha_associated),
            _ => false,
        };
    }
    codec_for_file(file).is_some_and(|codec| codec != Codec::JpegBaseline)
}

/// The pixel digest of `samples`, which are a frame's samples already in
/// the order and width [`dcmview_annotation::pixel`] defines: BLAKE3 of
/// [`PixelLayout::header`] and then `samples`.
///
/// `None` when `samples` is not [`PixelLayout::sample_bytes`] bytes long.
pub fn pixel_digest(layout: &PixelLayout, samples: &[u8]) -> Option<[u8; 32]> {
    if layout.sample_bytes() != Some(samples.len() as u64) {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(&layout.header());
    hasher.update(samples);
    Some(*hasher.finalize().as_bytes())
}

/// The pixel digest of one raw frame of `file`, as
/// [`super::service::load_raw_frame`] returns it before any redaction, and
/// the number of bytes it was computed over (24 and the samples).
///
/// The layout is read from the frame:
///
/// - rows, columns and samples per pixel are `metadata`'s;
/// - a sample is as wide as the body holds it: one byte when
///   `metadata.bits_allocated` is 1 or 8, else `bits_allocated / 8` bytes
///   for 16, 32 and 64, and the header's bits allocated is eight times
///   that;
/// - the sample format is floating point when the catalog says the file's
///   pixel data is (`NativePixelMetadata::pixel_data_kind` `Float32` or
///   `Float64`), else signed when `metadata.pixel_representation` is 1,
///   else unsigned;
/// - the samples are hashed colour-by-pixel. A body that already is (one
///   sample a pixel, or interleaved colour) is hashed where it lies. A body
///   stored colour-by-plane (an uncompressed DICOM frame with Planar
///   Configuration 1) is put in order through a buffer of at most
///   [`PIXEL_DIGEST_BUFFER_BYTES`]; the frame is never copied whole.
///
/// `None`, having read no sample, when the frame has no digest:
///
/// - [`exact_samples`] is `false` for `file`;
/// - the body holds chrominance at half resolution (uncompressed
///   `YBR_FULL_422`, two samples a pixel);
/// - `metadata.bits_allocated` is none of 1, 8, 16, 32 and 64, a dimension
///   or the sample count is 0, or the body is not exactly as long as the
///   layout says.
///
/// It never panics and reads nothing but its arguments.
pub fn frame_pixel_digest(
    file: &FileEntry,
    body: &[u8],
    metadata: &RawFrameMetadata,
) -> Option<([u8; 32], u64)> {
    if !exact_samples(file) {
        return None;
    }
    let size = match metadata.bits_allocated {
        1 | 8 => 1,
        bits @ (16 | 32 | 64) => bits / 8,
        _ => return None,
    };
    let sample_format = match file.series_metadata.native_pixel.pixel_data_kind {
        Some(NativePixelDataKind::Float32 | NativePixelDataKind::Float64) => SampleFormat::Float,
        _ if metadata.pixel_representation == 1 => SampleFormat::Signed,
        _ => SampleFormat::Unsigned,
    };
    let layout = PixelLayout {
        rows: metadata.rows,
        columns: metadata.columns,
        samples_per_pixel: metadata.samples_per_pixel,
        bits_allocated: size * 8,
        sample_format,
    };
    if layout.sample_bytes() != Some(body.len() as u64) {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(&layout.header());
    let planar = metadata.samples_per_pixel > 1
        && file.series_metadata.native_pixel.planar_configuration == Some(1)
        && codec_for_syntax(&file.transfer_syntax_uid) == Some(Codec::Native);
    if planar {
        // The checked body length also bounds every sample offset below.
        let size = size as usize;
        let pixels = metadata.rows as usize * metadata.columns as usize;
        let mut buffer = Vec::with_capacity(PIXEL_DIGEST_BUFFER_BYTES.min(body.len()));
        for pixel in 0..pixels {
            for sample in 0..metadata.samples_per_pixel as usize {
                let offset = (sample * pixels + pixel) * size;
                if buffer.len() + size > buffer.capacity() {
                    hasher.update(&buffer);
                    buffer.clear();
                }
                buffer.extend_from_slice(body.get(offset..offset + size)?);
            }
        }
        hasher.update(&buffer);
    } else {
        hasher.update(body);
    }
    Some((
        *hasher.finalize().as_bytes(),
        PIXEL_DIGEST_HEADER_BYTES as u64 + body.len() as u64,
    ))
}

/// Computes the pixel digest of frame `frame` of `file` from the samples a
/// raw decode just produced and records it in `digests`.
///
/// Called by the raw decode itself, before it returns its samples, so the
/// digest is in the table before any request sees the frame, and the work
/// runs under the decode's permit.
///
/// - When [`exact_samples`] is `false` for `file`, it returns at once: no
///   thread is used and nothing is counted.
/// - Otherwise [`frame_pixel_digest`] runs on a blocking thread
///   (`tokio::task::spawn_blocking`) over a clone of `body`, which shares
///   the samples and copies none, and a digest it returns is recorded with
///   [`PixelDigests::record`] under `file.index` and `frame`.
/// - It never fails the decode: a frame without a digest, and a hashing
///   thread that failed, record nothing, and nothing is remembered about
///   the failure.
pub async fn record_frame_digest(
    digests: &Arc<PixelDigests>,
    file: &Arc<FileEntry>,
    frame: u32,
    body: &Bytes,
    metadata: &RawFrameMetadata,
) {
    if !exact_samples(file) {
        return;
    }
    let hashed_file = file.clone();
    let body = body.clone();
    let metadata = metadata.clone();
    let result =
        tokio::task::spawn_blocking(move || frame_pixel_digest(&hashed_file, &body, &metadata))
            .await;
    if let Ok(Some((digest, bytes))) = result {
        digests.record(file.index, frame, digest, bytes);
    }
}
