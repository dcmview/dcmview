//! Raster files for the decode tests, built from the sample values a test
//! expects back.
//!
//! Lossless files are written here, by the linked encoders or byte by byte,
//! from samples the caller supplies, so the expected value of every pixel is
//! the one it was written with and never one a decoder returned. Lossy files
//! the linked encoders cannot write (progressive, subsampled and four-channel
//! JPEG, lossy and animated WebP) are embedded as bytes: each was encoded by
//! an independent tool from flat blocks of one colour, and the colours it was
//! made from are stated beside it and were checked with a second decoder
//! (libjpeg-turbo, libwebp).
//!
//! `tests/raster_cost` includes this file too, for the files it measures.

// Two test binaries include this file and each uses part of it.
#![allow(dead_code)]

use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{Read, Seek, SeekFrom, Write};

// ---------------------------------------------------------------------------
// PNG

/// A PNG of `data`, which holds the rows as PNG stores them: samples packed
/// from the high bit for depths under 8, big endian for 16.
pub fn png_of(
    color: png::ColorType,
    depth: png::BitDepth,
    (width, height): (u32, u32),
    data: &[u8],
    configure: impl FnOnce(&mut png::Encoder<'_, &mut Vec<u8>>),
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    configure(&mut encoder);
    let mut writer = encoder.write_header().expect("write PNG header");
    writer.write_image_data(data).expect("write PNG image");
    writer.finish().expect("finish PNG");
    out
}

/// One PNG chunk with its length and checksum.
pub fn png_chunk(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    let mut crc = flate2::Crc::new();
    crc.update(name);
    crc.update(payload);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data).expect("deflate");
    encoder.finish().expect("deflate")
}

/// A PNG written chunk by chunk: the signature, an `IHDR` of the given
/// fields, `chunks` as they are, and `IEND`.
pub fn png_from_chunks(
    (width, height): (u32, u32),
    depth: u8,
    color_type: u8,
    interlaced: bool,
    chunks: &[Vec<u8>],
) -> Vec<u8> {
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[depth, color_type, 0, 0, u8::from(interlaced)]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(png_chunk(b"IHDR", &header));
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out.extend(png_chunk(b"IEND", &[]));
    out
}

/// The `IDAT` chunk of 8-bit rows of `row_bytes` each, unfiltered.
pub fn png_idat(rows: &[u8], row_bytes: usize) -> Vec<u8> {
    let mut filtered = Vec::new();
    for row in rows.chunks(row_bytes) {
        filtered.push(0);
        filtered.extend_from_slice(row);
    }
    png_chunk(b"IDAT", &zlib(&filtered))
}

/// An Adam7-interlaced 8-bit gray PNG of `pixels` (row-major). The `png`
/// crate does not write interlaced files, so the seven passes are laid out
/// here.
pub fn png_adam7_gray8((width, height): (usize, usize), pixels: &[u8]) -> Vec<u8> {
    // (first column, first row, column step, row step) of each pass.
    const PASSES: [(usize, usize, usize, usize); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    let mut filtered = Vec::new();
    for (x0, y0, dx, dy) in PASSES {
        if x0 >= width {
            continue;
        }
        for y in (y0..height).step_by(dy) {
            filtered.push(0);
            filtered.extend((x0..width).step_by(dx).map(|x| pixels[y * width + x]));
        }
    }
    png_from_chunks(
        (width as u32, height as u32),
        8,
        0,
        true,
        &[png_chunk(b"IDAT", &zlib(&filtered))],
    )
}

// ---------------------------------------------------------------------------
// TIFF

/// One value of a TIFF tag written by [`tiff_file`].
#[derive(Clone)]
pub enum TiffValue {
    Short(Vec<u16>),
    Long(Vec<u32>),
    /// A type, a count and the four value bytes exactly as given: for tags
    /// whose declared count or type is the point of the test.
    Raw(u16, u32, [u8; 4]),
    /// Bytes of type UNDEFINED, stored out of line.
    Bytes(Vec<u8>),
}

/// One page of a TIFF written by [`tiff_file`]: its tags, without the strip
/// or tile offsets and byte counts, and the bytes of each strip or tile as
/// they are stored.
#[derive(Clone)]
pub struct TiffPage {
    pub tags: Vec<(u16, TiffValue)>,
    pub chunks: Vec<Vec<u8>>,
    pub tiled: bool,
}

impl TiffPage {
    /// An uncompressed page of one strip. `photometric` is tag 262.
    pub fn strip(
        (width, height): (u32, u32),
        bits: &[u16],
        photometric: u16,
        samples: Vec<u8>,
    ) -> Self {
        Self {
            tags: vec![
                (256, TiffValue::Long(vec![width])),
                (257, TiffValue::Long(vec![height])),
                (258, TiffValue::Short(bits.to_vec())),
                (259, TiffValue::Short(vec![1])),
                (262, TiffValue::Short(vec![photometric])),
                (277, TiffValue::Short(vec![bits.len() as u16])),
                (278, TiffValue::Long(vec![height])),
            ],
            chunks: vec![samples],
            tiled: false,
        }
    }

    /// The page with `tag` set to `value`, replacing any value it had.
    pub fn with(mut self, tag: u16, value: TiffValue) -> Self {
        self.tags.retain(|(existing, _)| *existing != tag);
        self.tags.push((tag, value));
        self
    }
}

/// A classic TIFF in the given byte order: for each page its strips or
/// tiles, then its out-of-line tag values, then its IFD. Returns the file
/// and the offset of each page's IFD.
pub fn tiff_file(big_endian: bool, pages: &[TiffPage]) -> (Vec<u8>, Vec<u64>) {
    let u16_bytes = |value: u16| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let u32_bytes = |value: u32| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let mut out = if big_endian {
        b"MM\0\x2a\0\0\0\0".to_vec()
    } else {
        b"II\x2a\0\0\0\0\0".to_vec()
    };
    let mut ifd_offsets = Vec::new();
    // Where the previous page's "next IFD" field is, to point it here.
    let mut link_at = 4;
    for page in pages {
        let mut offsets = Vec::new();
        let mut counts = Vec::new();
        for chunk in &page.chunks {
            offsets.push(out.len() as u32);
            counts.push(chunk.len() as u32);
            out.extend_from_slice(chunk);
        }
        let mut tags = page.tags.clone();
        let (offsets_tag, counts_tag) = if page.tiled { (324, 325) } else { (273, 279) };
        if !tags.iter().any(|(tag, _)| *tag == offsets_tag) {
            tags.push((offsets_tag, TiffValue::Long(offsets)));
        }
        if !tags.iter().any(|(tag, _)| *tag == counts_tag) {
            tags.push((counts_tag, TiffValue::Long(counts)));
        }
        tags.sort_by_key(|(tag, _)| *tag);

        let mut entries = Vec::new();
        for (tag, value) in &tags {
            let (kind, count, bytes): (u16, u32, Vec<u8>) = match value {
                TiffValue::Short(values) => (
                    3,
                    values.len() as u32,
                    values.iter().flat_map(|value| u16_bytes(*value)).collect(),
                ),
                TiffValue::Long(values) => (
                    4,
                    values.len() as u32,
                    values.iter().flat_map(|value| u32_bytes(*value)).collect(),
                ),
                TiffValue::Raw(kind, count, bytes) => (*kind, *count, bytes.to_vec()),
                TiffValue::Bytes(bytes) => (7, bytes.len() as u32, bytes.clone()),
            };
            let raw = matches!(value, TiffValue::Raw(..));
            let mut entry = u16_bytes(*tag).to_vec();
            entry.extend_from_slice(&u16_bytes(kind));
            entry.extend_from_slice(&u32_bytes(count));
            if bytes.len() <= 4 || raw {
                entry.extend_from_slice(&bytes);
                entry.resize(12, 0);
            } else {
                out.resize(out.len() + out.len() % 2, 0);
                entry.extend_from_slice(&u32_bytes(out.len() as u32));
                out.extend_from_slice(&bytes);
            }
            entries.push(entry);
        }
        out.resize(out.len() + out.len() % 2, 0);
        let ifd = out.len() as u32;
        out[link_at..link_at + 4].copy_from_slice(&u32_bytes(ifd));
        ifd_offsets.push(u64::from(ifd));
        out.extend_from_slice(&u16_bytes(entries.len() as u16));
        for entry in entries {
            out.extend_from_slice(&entry);
        }
        link_at = out.len();
        out.extend_from_slice(&[0; 4]);
    }
    (out, ifd_offsets)
}

/// A classic little-endian TIFF of one `page`, whose chunks are given in the
/// order they are decoded, written with the chunks stored in the order
/// `stored` names them (`stored[k]` is the chunk stored `k`-th).
pub fn tiff_stored_in(mut page: TiffPage, stored: &[usize]) -> Vec<u8> {
    let mut offsets = vec![0_u32; page.chunks.len()];
    // `tiff_file` writes the first page's chunks straight after the
    // eight-byte header.
    let mut at = 8;
    for &index in stored {
        offsets[index] = at;
        at += page.chunks[index].len() as u32;
    }
    let counts = page.chunks.iter().map(|chunk| chunk.len() as u32).collect();
    let (offsets_tag, counts_tag) = if page.tiled { (324, 325) } else { (273, 279) };
    page.tags.push((offsets_tag, TiffValue::Long(offsets)));
    page.tags.push((counts_tag, TiffValue::Long(counts)));
    page.chunks = stored
        .iter()
        .map(|&index| page.chunks[index].clone())
        .collect();
    tiff_file(false, &[page]).0
}

// ---------------------------------------------------------------------------
// JPEG and WebP

fn embedded(hex: &[&str]) -> Vec<u8> {
    hex.concat()
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("hex"))
        .collect()
}

/// Progressive, 4:4:4, 32 x 8: four 8 x 8 blocks of (200, 30, 30),
/// (30, 200, 30), (30, 30, 200) and (128, 128, 128). Ten scans.
pub fn progressive_jpeg() -> Vec<u8> {
    embedded(PROGRESSIVE_JPEG)
}

/// Baseline, 4:2:0, 16 x 16 of (90, 140, 200).
pub fn subsampled_jpeg() -> Vec<u8> {
    embedded(SUBSAMPLED_JPEG)
}

/// [`subsampled_jpeg`] with the sampling factors of its third component
/// changed from 1 x 1 to 4 x 2, larger than the first component's: a frame
/// header no encoder writes, on which the linked JPEG decoder panics instead
/// of returning an error.
pub fn jpeg_its_decoder_panics_on() -> Vec<u8> {
    let mut jpeg = subsampled_jpeg();
    // The frame header: length, precision, height, width, the number of
    // components, then three bytes for each, the second its factors.
    let third = jpeg_frame_marker(&jpeg) + 9 + 2 * 3 + 1;
    assert_eq!(jpeg[third], 0x11);
    jpeg[third] = 0x42;
    jpeg
}

/// Baseline, 4:1:1 (chroma at a quarter of the width), 32 x 16 of
/// (90, 140, 200). Written by libjpeg-turbo's `cjpeg -sample 4x1,1x1,1x1`.
pub fn quarter_width_chroma_jpeg() -> Vec<u8> {
    let mut hex = SAMPLED_JPEG_START.to_vec();
    hex.extend_from_slice(&["ffc00011080010002003014100021101031101"]);
    hex.extend_from_slice(SAMPLED_JPEG_TABLES);
    hex.extend_from_slice(&["ffda000c03010002110311003f00b7451457eca7e5e145145007ffd9"]);
    embedded(&hex)
}

/// Baseline, 4:4:0 (chroma at half the height), 32 x 16 of (90, 140, 200).
/// Written by libjpeg-turbo's `cjpeg -sample 1x2,1x1,1x1`.
pub fn half_height_chroma_jpeg() -> Vec<u8> {
    let mut hex = SAMPLED_JPEG_START.to_vec();
    hex.extend_from_slice(&["ffc00011080010002003011200021101031101"]);
    hex.extend_from_slice(SAMPLED_JPEG_TABLES);
    hex.extend_from_slice(&["ffda000c03010002110311003f00b7457eca7e5e1450014500145007ffd9"]);
    embedded(&hex)
}

/// Four-channel (Adobe transform 0, CMYK), 32 x 8: blocks of no ink, full
/// cyan, full black, and magenta with yellow: white, (0, 255, 255), black
/// and (255, 0, 0) in RGB.
pub fn cmyk_jpeg() -> Vec<u8> {
    embedded(CMYK_JPEG)
}

/// [`cmyk_jpeg`] with a 132-byte `APP2` profile whose colour space is `CMYK`.
pub fn cmyk_jpeg_with_profile() -> Vec<u8> {
    embedded(CMYK_PROFILE_JPEG)
}

/// The image of [`cmyk_jpeg`] stored as YCCK (Adobe transform 2).
pub fn ycck_jpeg() -> Vec<u8> {
    embedded(YCCK_JPEG)
}

/// Lossy (`VP8 `), 16 x 16 of (90, 140, 200).
pub fn lossy_webp() -> Vec<u8> {
    embedded(LOSSY_WEBP)
}

/// Lossy with an alpha plane (`VP8X`, `ALPH`, `VP8 `), 16 x 16 of
/// (90, 140, 200) at alpha 128.
pub fn lossy_alpha_webp() -> Vec<u8> {
    embedded(LOSSY_ALPHA_WEBP)
}

/// Animated and lossless, 8 x 8, two frames: (10, 20, 30), then
/// (200, 100, 50).
pub fn animated_webp() -> Vec<u8> {
    embedded(ANIMATED_WEBP)
}

/// `jpeg` with an EXIF `APP1` segment, holding only the orientation tag,
/// inserted straight after the start-of-image marker.
pub fn with_exif_orientation(mut jpeg: Vec<u8>, orientation: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0".to_vec();
    // Little-endian TIFF header, then IFD0 at offset 8 with one SHORT entry.
    exif.extend_from_slice(b"II\x2a\0\x08\0\0\0");
    exif.extend_from_slice(&1_u16.to_le_bytes());
    exif.extend_from_slice(&0x0112_u16.to_le_bytes());
    exif.extend_from_slice(&3_u16.to_le_bytes());
    exif.extend_from_slice(&1_u32.to_le_bytes());
    exif.extend_from_slice(&orientation.to_le_bytes());
    exif.extend_from_slice(&[0, 0]);
    exif.extend_from_slice(&0_u32.to_le_bytes());
    let mut segment = vec![0xff, 0xe1];
    segment.extend_from_slice(&(exif.len() as u16 + 2).to_be_bytes());
    segment.extend_from_slice(&exif);
    assert_eq!(&jpeg[..2], [0xff, 0xd8], "JPEG starts with SOI");
    jpeg.splice(2..2, segment);
    jpeg
}

/// The byte ranges of the scans of a JPEG: each from its `SOS` marker to the
/// marker that ends its entropy-coded data.
pub fn jpeg_scans(jpeg: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut scans = Vec::new();
    let mut at = 2;
    while at + 4 <= jpeg.len() {
        assert_eq!(jpeg[at], 0xff, "marker at {at}");
        let marker = jpeg[at + 1];
        if marker == 0xd9 {
            break;
        }
        let length = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
        let mut end = at + 2 + length;
        if marker == 0xda {
            // Entropy-coded data runs to the next marker that is neither a
            // stuffed zero nor a restart.
            while jpeg[end] != 0xff || matches!(jpeg[end + 1], 0 | 0xd0..=0xd7) {
                end += 1;
            }
            scans.push(at..end);
        }
        at = end;
    }
    scans
}

/// The offset of the frame header's marker byte (`0xc0` for baseline) in a
/// JPEG; the sample precision is four bytes on.
pub fn jpeg_frame_marker(jpeg: &[u8]) -> usize {
    let mut at = 2;
    loop {
        let marker = jpeg[at + 1];
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            return at + 1;
        }
        at += 2 + usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
    }
}

/// One RIFF chunk, padded to an even length.
pub fn riff_chunk(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = name.to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out.resize(out.len() + payload.len() % 2, 0);
    out
}

/// A WebP file of `chunks`.
pub fn webp_from_chunks(chunks: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(chunks.len() as u32 + 4).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(chunks);
    out
}

/// A `VP8X` chunk for a canvas of `width` x `height` with the given flags
/// (0x20 profile, 0x10 alpha, 0x08 EXIF, 0x02 animation).
pub fn vp8x(flags: u8, (width, height): (u32, u32)) -> Vec<u8> {
    let mut payload = vec![flags, 0, 0, 0];
    payload.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    payload.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    riff_chunk(b"VP8X", &payload)
}

/// A lossless WebP of 8-bit RGB or RGBA `pixels`.
pub fn lossless_webp(
    color: image::ExtendedColorType,
    (width, height): (u32, u32),
    pixels: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(pixels, width, height, color)
        .expect("encode WebP");
    out
}

/// A baseline JPEG of 8-bit gray or RGB `pixels` at quality 95.
pub fn baseline_jpeg(
    color: image::ExtendedColorType,
    (width, height): (u32, u32),
    pixels: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95)
        .encode(pixels, width, height, color)
        .expect("encode JPEG");
    out
}

/// A valid ICC profile header of `length` bytes for the data colour space
/// `space` (`b"RGB "`, `b"GRAY"`, `b"CMYK"`), filled with `fill`.
pub fn icc_profile(space: &[u8; 4], length: usize, fill: u8) -> Vec<u8> {
    let mut profile = vec![fill; length];
    profile[..128].fill(0);
    profile[..4].copy_from_slice(&(length as u32).to_be_bytes());
    profile[16..20].copy_from_slice(space);
    profile[36..40].copy_from_slice(b"acsp");
    profile
}

// ---------------------------------------------------------------------------
// A counted file

/// A file in memory that counts what is read from it: the reads issued and
/// the bytes they return. Past its real bytes it can continue with a
/// repeating pattern up to a declared length, so a test can present a file of
/// any size without holding it.
pub struct CountedFile {
    bytes: Vec<u8>,
    tail: Vec<u8>,
    length: u64,
    position: u64,
    pub reads: u64,
    pub bytes_read: u64,
}

impl CountedFile {
    pub fn new(bytes: Vec<u8>) -> Self {
        let length = bytes.len() as u64;
        Self::with_tail(bytes, &[0], length)
    }

    /// `bytes`, then `tail` repeated, `length` bytes in all.
    pub fn with_tail(bytes: Vec<u8>, tail: &[u8], length: u64) -> Self {
        Self {
            bytes,
            tail: tail.to_vec(),
            length,
            position: 0,
            reads: 0,
            bytes_read: 0,
        }
    }

    pub fn length(&self) -> u64 {
        self.length
    }
}

impl Read for CountedFile {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let count = (out.len() as u64).min(self.length.saturating_sub(self.position)) as usize;
        let real = self.bytes.len() as u64;
        for (index, byte) in out[..count].iter_mut().enumerate() {
            let at = self.position + index as u64;
            *byte = if at < real {
                self.bytes[at as usize]
            } else {
                self.tail[((at - real) % self.tail.len() as u64) as usize]
            };
        }
        self.position += count as u64;
        self.reads += 1;
        self.bytes_read += count as u64;
        Ok(count)
    }
}

impl Seek for CountedFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
        };
        self.position = target.ok_or_else(|| std::io::Error::other("invalid seek"))?;
        Ok(self.position)
    }
}

// ---------------------------------------------------------------------------
// Embedded files

const PROGRESSIVE_JPEG: &[&str] = &[
    "ffd8ffe000104a46494600010100000100010000ffdb00430002010101010102010101020202",
    "02020403020202020504040304060506060605060606070908060709070606080b08090a0a0a",
    "0a0a06080b0c0b0a0c090a0a0affdb004301020202020202050303050a0706070a0a0a0a0a0a",
    "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a",
    "0a0a0a0a0a0affc20011080008002003011100021101031101ffc40015000101000000000000",
    "00000000000000000708ffc4001801000301010000000000000000000000000608090507ffda",
    "000c0301000210031000000121056a9882663cbb46bb953caaee7fffc4001410010000000000",
    "0000000000000000000020ffda00080101000105021fffc40014110100000000000000000000",
    "000000000020ffda0008010301013f011fffc400141101000000000000000000000000000000",
    "20ffda0008010201013f011fffc40014100100000000000000000000000000000020ffda0008",
    "010100063f021fffc40014100100000000000000000000000000000020ffda0008010100013f",
    "211fffda000c03010002000300000010000fffc4001411010000000000000000000000000000",
    "0020ffda0008010301013f101fffc40014110100000000000000000000000000000020ffda00",
    "08010201013f101fffc40014100100000000000000000000000000000020ffda000801010001",
    "3f101fffd9",
];

const SUBSAMPLED_JPEG: &[&str] = &[
    "ffd8ffe000104a46494600010100000100010000ffdb00430002010101010102010101020202",
    "02020403020202020504040304060506060605060606070908060709070606080b08090a0a0a",
    "0a0a06080b0c0b0a0c090a0a0affdb004301020202020202050303050a0706070a0a0a0a0a0a",
    "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a",
    "0a0a0a0a0a0affc00011080010001003012200021101031101ffc4001f000001050101010101",
    "0100000000000000000102030405060708090a0bffc400b51000020103030204030505040400",
    "00017d01020300041105122131410613516107227114328191a1082342b1c11552d1f0243362",
    "7282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a",
    "636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6",
    "a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7",
    "e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f010003010101010101010101000000000000010203",
    "0405060708090a0bffc400b51100020102040403040705040400010277000102031104052131",
    "061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a",
    "262728292a35363738393a434445464748494a535455565758595a636465666768696a737475",
    "767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7",
    "b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9",
    "faffda000c03010002110311003f00d0a28a2bfa60fc1cffd9",
];

/// What the two `cjpeg -quality 90` files share: the start of the file with
/// its quantization tables, and the standard Huffman tables. Their frame
/// headers and scans differ.
const SAMPLED_JPEG_START: &[&str] = &[
    "ffd8ffe000104a46494600010100000100010000ffdb00430003020203020203030303040303",
    "04050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b10161011131415",
    "15150c0f171816141812141514ffdb00430103040405040509050509140d0b0d141414141414",
    "1414141414141414141414141414141414141414141414141414141414141414141414141414",
    "141414141414",
];

const SAMPLED_JPEG_TABLES: &[&str] = &[
    "ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b510",
    "00020103030204030505040400",
    "00017d01020300041105122131410613516107227114328191a1082342b1c11552d1f0243362",
    "7282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a",
    "636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6",
    "a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7",
    "e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f010003010101010101010101000000000000010203",
    "0405060708090a0bffc400b51100020102040403040705040400010277000102031104052131",
    "061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a",
    "262728292a35363738393a434445464748494a535455565758595a636465666768696a737475",
    "767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7",
    "b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9",
    "fa",
];

const CMYK_JPEG: &[&str] = &[
    "ffd8ffee000e41646f626500640000000000ffdb004300020101010101020101010202020202",
    "0403020202020504040304060506060605060606070908060709070606080b08090a0a0a0a0a",
    "06080b0c0b0a0c090a0a0affc000140800080020044311004d11005911004b1100ffc4001f00",
    "00010501010101010100000000000000000102030405060708090a0bffc400b5100002010303",
    "020403050504040000017d01020300041105122131410613516107227114328191a1082342b1",
    "c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a",
    "535455565758595a636465666768696a737475767778797a838485868788898a929394959697",
    "98999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9",
    "dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda000e0443004d0059004b00003f00fd",
    "fcafdfcafdfcafdfcafe00e8a28afeff0028a2bf803a2bf803afe00ebfbfcaffd9",
];

const CMYK_PROFILE_JPEG: &[&str] = &[
    "ffd8ffee000e41646f626500640000000000ffe200944943435f50524f46494c450001010000",
    "0084000000000000000000000000434d594b0000000000000000000000000000000061637370",
    "0000000000000000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000ffdb0043000201010101010201010102020202020403",
    "020202020504040304060506060605060606070908060709070606080b08090a0a0a0a0a0608",
    "0b0c0b0a0c090a0a0affc000140800080020044311004d11005911004b1100ffc4001f000001",
    "0501010101010100000000000000000102030405060708090a0bffc400b51000020103030204",
    "03050504040000017d01020300041105122131410613516107227114328191a1082342b1c115",
    "52d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a5354",
    "55565758595a636465666768696a737475767778797a838485868788898a9293949596979899",
    "9aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1",
    "e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda000e0443004d0059004b00003f00fdfcaf",
    "dfcafdfcafdfcafe00e8a28afeff0028a2bf803a2bf803afe00ebfbfcaffd9",
];

const YCCK_JPEG: &[&str] = &[
    "ffd8ffee000e41646f626500640000000002ffdb004300020101010101020101010202020202",
    "0403020202020504040304060506060605060606070908060709070606080b08090a0a0a0a0a",
    "06080b0c0b0a0c090a0a0affdb004301020202020202050303050a0706070a0a0a0a0a0a0a0a",
    "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a",
    "0a0a0a0affc00014080008002004011100021101031101041100ffc400160001010100000000",
    "0000000000000000000a0900ffc40014100100000000000000000000000000000000ffc40017",
    "01000301000000000000000000000000000008090affc4001411010000000000000000000000",
    "0000000000ffda000e040100021103110400003f003fe02fe2981535fc64cf3588060065986b",
    "19ff003fc7ffd9",
];

const LOSSY_WEBP: &[&str] = &[
    "524946463a00000057454250565038202e0000003002009d012a1000100000c01225a00274ba",
    "01f801f800061e00009bffdb433aa12ff6e3ffea7060e7cfc7f80000",
];

const LOSSY_ALPHA_WEBP: &[&str] = &[
    "524946465a00000057454250565038580a000000100000000f00000f0000414c504805000000",
    "012860440400565038202e0000003002009d012a1000100000c01225a00274ba01f801f80006",
    "1e00009bffdb433aa12ff6e3ffea7060e7cfc7f80000",
];

const ANIMATED_WEBP: &[&str] = &[
    "524946468800000057454250565038580a00000002000000070000070000414e494d06000000",
    "000000000000414e4d462a000000000000000000070000070000640000025650384c11000000",
    "2f07c0010007508a2ad4a3ff8188e87f0000414e4d462a000000000000000000070000070000",
    "640000005650384c110000002f07c001000750b22257a6ff8188e87f0000",
];
