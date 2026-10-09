//! Raster files that carry metadata, for the metadata tree tests, built
//! from the values a test expects to see.
//!
//! Directories, text chunks, segments and profiles are written here byte by
//! byte from values the caller supplies, so every expected value is one a
//! file was written with and never one the reader under test returned. The
//! four `pillow_*` files are the exception that keeps the hand-written
//! layouts honest: they were written by Pillow 12.1.1 (its own PNG and TIFF
//! writers, libjpeg-turbo, libwebp 1.6.0) from an 8 x 8 image and the
//! metadata stated beside each, and are embedded as bytes.
//!
//! `tests/raster_cost` includes this file too.

// Two test binaries include this file and each uses part of it.
#![allow(dead_code)]

use super::raster_files::{
    baseline_jpeg, lossless_webp, png_chunk, png_from_chunks, png_idat, riff_chunk, vp8x,
    webp_from_chunks, zlib,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// TIFF directories: a TIFF file's pages, or an EXIF block

/// One value of a directory entry.
#[derive(Clone, Debug)]
pub enum V {
    /// The bytes and a terminating NUL.
    Ascii(Vec<u8>),
    Byte(Vec<u8>),
    Undefined(Vec<u8>),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
    Double(Vec<f64>),
    /// A type code, a count and the four bytes of the value field exactly
    /// as given: for entries whose declared type or count is the point.
    Raw(u16, u32, [u8; 4]),
}

impl V {
    pub fn ascii(text: &str) -> Self {
        Self::Ascii(text.as_bytes().to_vec())
    }

    /// `text` as the UTF-16 little-endian bytes of an `XP` tag.
    pub fn utf16(text: &str) -> Self {
        let mut bytes = text
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        bytes.extend_from_slice(&[0, 0]);
        Self::Byte(bytes)
    }
}

pub type Dir = Vec<(u16, V)>;

/// A classic TIFF structure: the header, `data`, then the directories.
#[derive(Clone, Default)]
pub struct Block {
    pub big_endian: bool,
    /// Bytes placed straight after the eight-byte header, at offset 8: the
    /// samples of a TIFF page.
    pub data: Vec<u8>,
    pub ifd0: Dir,
    /// The EXIF directory; `ifd0` gets tag 34665 pointing to it.
    pub exif: Option<Dir>,
    /// The GPS directory; `ifd0` gets tag 34853 pointing to it.
    pub gps: Option<Dir>,
    /// The interoperability directory; `exif` gets tag 40965 pointing to it.
    pub interop: Option<Dir>,
    /// The directories chained after `ifd0`: `IFD1` of an EXIF block, the
    /// further pages of a TIFF file.
    pub next: Vec<Dir>,
}

/// A written [`Block`] and where its directories lie, for tests that then
/// damage one.
pub struct Layout {
    pub bytes: Vec<u8>,
    pub ifd0: usize,
    pub exif: usize,
    pub gps: usize,
    pub interop: usize,
    pub next: Vec<usize>,
}

impl Layout {
    /// The offset of the entry for `tag` in the directory at `directory`
    /// (little endian only).
    pub fn entry(&self, directory: usize, tag: u16) -> usize {
        let count = u16::from_le_bytes([self.bytes[directory], self.bytes[directory + 1]]);
        (0..usize::from(count))
            .map(|index| directory + 2 + index * 12)
            .find(|at| self.bytes[*at..*at + 2] == tag.to_le_bytes())
            .unwrap_or_else(|| panic!("no entry for tag {tag}"))
    }

    /// The offset of the link to the next directory after the one at
    /// `directory` (little endian only).
    pub fn link(&self, directory: usize) -> usize {
        let count = u16::from_le_bytes([self.bytes[directory], self.bytes[directory + 1]]);
        directory + 2 + usize::from(count) * 12
    }
}

fn entry_parts(value: &V, big: bool) -> (u16, u32, Vec<u8>) {
    let u16s = |values: &[u16]| -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| {
                if big {
                    value.to_be_bytes()
                } else {
                    value.to_le_bytes()
                }
            })
            .collect()
    };
    let u32s = |values: &[u32]| -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| {
                if big {
                    value.to_be_bytes()
                } else {
                    value.to_le_bytes()
                }
            })
            .collect()
    };
    match value {
        V::Ascii(bytes) => {
            let mut bytes = bytes.clone();
            bytes.push(0);
            (2, bytes.len() as u32, bytes)
        }
        V::Byte(bytes) => (1, bytes.len() as u32, bytes.clone()),
        V::Undefined(bytes) => (7, bytes.len() as u32, bytes.clone()),
        V::Short(values) => (3, values.len() as u32, u16s(values)),
        V::Long(values) => (4, values.len() as u32, u32s(values)),
        V::Rational(values) => (
            5,
            values.len() as u32,
            u32s(
                &values
                    .iter()
                    .flat_map(|(numerator, denominator)| [*numerator, *denominator])
                    .collect::<Vec<_>>(),
            ),
        ),
        V::SRational(values) => (
            10,
            values.len() as u32,
            u32s(
                &values
                    .iter()
                    .flat_map(|(numerator, denominator)| [*numerator as u32, *denominator as u32])
                    .collect::<Vec<_>>(),
            ),
        ),
        V::Double(values) => (
            12,
            values.len() as u32,
            values
                .iter()
                .flat_map(|value| {
                    if big {
                        value.to_be_bytes()
                    } else {
                        value.to_le_bytes()
                    }
                })
                .collect(),
        ),
        V::Raw(kind, count, field) => (*kind, *count, field.to_vec()),
    }
}

fn directory_size(dir: &Dir, big: bool) -> usize {
    let outside: usize = dir
        .iter()
        .map(|(_, value)| {
            let (_, _, bytes) = entry_parts(value, big);
            if bytes.len() > 4 {
                bytes.len() + bytes.len() % 2
            } else {
                0
            }
        })
        .sum();
    2 + dir.len() * 12 + 4 + outside
}

fn write_directory(out: &mut Vec<u8>, dir: &Dir, next: usize, big: bool) {
    let at = out.len();
    let u16_bytes = |value: u16| {
        if big {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let u32_bytes = |value: u32| {
        if big {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let mut table = u16_bytes(dir.len() as u16).to_vec();
    let mut outside = Vec::new();
    let outside_at = at + 2 + dir.len() * 12 + 4;
    for (tag, value) in dir {
        let (kind, count, bytes) = entry_parts(value, big);
        table.extend_from_slice(&u16_bytes(*tag));
        table.extend_from_slice(&u16_bytes(kind));
        table.extend_from_slice(&u32_bytes(count));
        if bytes.len() <= 4 {
            let mut field = bytes;
            field.resize(4, 0);
            table.extend_from_slice(&field);
        } else {
            table.extend_from_slice(&u32_bytes((outside_at + outside.len()) as u32));
            outside.extend_from_slice(&bytes);
            outside.resize(outside.len() + outside.len() % 2, 0);
        }
    }
    table.extend_from_slice(&u32_bytes(next as u32));
    out.extend_from_slice(&table);
    out.extend_from_slice(&outside);
}

/// Writes `block`: entries in ascending tag order, each directory followed
/// by its out-of-line values.
pub fn tiff_block(block: &Block) -> Layout {
    let big = block.big_endian;
    let sorted = |dir: &Dir| {
        let mut dir = dir.clone();
        dir.sort_by_key(|(tag, _)| *tag);
        dir
    };
    let mut ifd0 = block.ifd0.clone();
    let mut exif = block.exif.clone();
    // Pointers are placeholders until the layout is known; their size is.
    if exif.is_some() {
        ifd0.push((34665, V::Long(vec![0])));
    }
    if block.gps.is_some() {
        ifd0.push((34853, V::Long(vec![0])));
    }
    if let (Some(exif), Some(_)) = (&mut exif, &block.interop) {
        exif.push((40965, V::Long(vec![0])));
    }
    let mut at = 8 + block.data.len() + block.data.len() % 2;
    let mut place = |dir: Option<&Dir>| -> usize {
        let Some(dir) = dir else { return 0 };
        let here = at;
        at += directory_size(dir, big);
        here
    };
    let ifd0_at = place(Some(&ifd0));
    let exif_at = place(exif.as_ref());
    let gps_at = place(block.gps.as_ref());
    let interop_at = place(block.interop.as_ref());
    let next_at = block
        .next
        .iter()
        .map(|dir| place(Some(dir)))
        .collect::<Vec<_>>();

    let set = |dir: &mut Dir, tag: u16, offset: usize| {
        for (existing, value) in dir.iter_mut() {
            if *existing == tag {
                *value = V::Long(vec![offset as u32]);
            }
        }
    };
    set(&mut ifd0, 34665, exif_at);
    set(&mut ifd0, 34853, gps_at);
    if let Some(exif) = &mut exif {
        set(exif, 40965, interop_at);
    }

    let mut out = if big {
        b"MM\0\x2a".to_vec()
    } else {
        b"II\x2a\0".to_vec()
    };
    out.extend_from_slice(&if big {
        (ifd0_at as u32).to_be_bytes()
    } else {
        (ifd0_at as u32).to_le_bytes()
    });
    out.extend_from_slice(&block.data);
    out.resize(out.len() + out.len() % 2, 0);
    write_directory(
        &mut out,
        &sorted(&ifd0),
        next_at.first().copied().unwrap_or(0),
        big,
    );
    for dir in [&exif, &block.gps, &block.interop].into_iter().flatten() {
        write_directory(&mut out, &sorted(dir), 0, big);
    }
    for (index, dir) in block.next.iter().enumerate() {
        let next = next_at.get(index + 1).copied().unwrap_or(0);
        write_directory(&mut out, &sorted(dir), next, big);
    }
    Layout {
        bytes: out,
        ifd0: ifd0_at,
        exif: exif_at,
        gps: gps_at,
        interop: interop_at,
        next: next_at,
    }
}

/// The layout tags of an uncompressed 2 x 2 8-bit RGB page whose 12 sample
/// bytes are the block's `data`.
pub fn rgb_page_tags() -> Dir {
    vec![
        (256, V::Long(vec![2])),
        (257, V::Long(vec![2])),
        (258, V::Short(vec![8, 8, 8])),
        (259, V::Short(vec![1])),
        (262, V::Short(vec![2])),
        (273, V::Long(vec![8])),
        (277, V::Short(vec![3])),
        (278, V::Long(vec![2])),
        (279, V::Long(vec![12])),
    ]
}

/// The samples of [`rgb_page_tags`].
pub fn rgb_page_samples() -> Vec<u8> {
    vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
}

// ---------------------------------------------------------------------------
// Profiles

/// An ICC profile of the display class for RGB data, version 2.1, with one
/// tag: a `desc` of `description`. `class` is its four class bytes.
pub fn icc_with_description(class: &[u8; 4], description: &str) -> Vec<u8> {
    let mut desc = b"desc\0\0\0\0".to_vec();
    desc.extend_from_slice(&(description.len() as u32 + 1).to_be_bytes());
    desc.extend_from_slice(description.as_bytes());
    desc.push(0);
    icc_with_tag(class, 0x0210_0000, &desc)
}

/// The same for version 4.3, whose description is an `mluc` record in
/// UTF-16 big endian.
pub fn icc_with_unicode_description(class: &[u8; 4], description: &str) -> Vec<u8> {
    let text = description
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>();
    let mut desc = b"mluc\0\0\0\0".to_vec();
    desc.extend_from_slice(&1_u32.to_be_bytes());
    desc.extend_from_slice(&12_u32.to_be_bytes());
    desc.extend_from_slice(b"enUS");
    desc.extend_from_slice(&(text.len() as u32).to_be_bytes());
    desc.extend_from_slice(&28_u32.to_be_bytes());
    desc.extend_from_slice(&text);
    icc_with_tag(class, 0x0430_0000, &desc)
}

fn icc_with_tag(class: &[u8; 4], version: u32, desc: &[u8]) -> Vec<u8> {
    let mut profile = vec![0_u8; 128];
    profile[8..12].copy_from_slice(&version.to_be_bytes());
    profile[12..16].copy_from_slice(class);
    profile[16..20].copy_from_slice(b"RGB ");
    profile[20..24].copy_from_slice(b"XYZ ");
    profile[36..40].copy_from_slice(b"acsp");
    profile.extend_from_slice(&1_u32.to_be_bytes());
    profile.extend_from_slice(b"desc");
    profile.extend_from_slice(&144_u32.to_be_bytes());
    profile.extend_from_slice(&(desc.len() as u32).to_be_bytes());
    profile.extend_from_slice(desc);
    profile.resize(profile.len().next_multiple_of(4), 0);
    let length = profile.len() as u32;
    profile[..4].copy_from_slice(&length.to_be_bytes());
    profile
}

// ---------------------------------------------------------------------------
// PNG chunks

pub fn png_text(keyword: &[u8], text: &[u8]) -> Vec<u8> {
    let mut payload = keyword.to_vec();
    payload.push(0);
    payload.extend_from_slice(text);
    png_chunk(b"tEXt", &payload)
}

pub fn png_compressed_text(keyword: &[u8], text: &[u8]) -> Vec<u8> {
    let mut payload = keyword.to_vec();
    payload.extend_from_slice(&[0, 0]);
    payload.extend_from_slice(&zlib(text));
    png_chunk(b"zTXt", &payload)
}

pub fn png_international_text(
    keyword: &[u8],
    language: &[u8],
    translated: &[u8],
    text: &[u8],
    compressed: bool,
) -> Vec<u8> {
    let mut payload = keyword.to_vec();
    payload.extend_from_slice(&[0, u8::from(compressed), 0]);
    payload.extend_from_slice(language);
    payload.push(0);
    payload.extend_from_slice(translated);
    payload.push(0);
    if compressed {
        payload.extend_from_slice(&zlib(text));
    } else {
        payload.extend_from_slice(text);
    }
    png_chunk(b"iTXt", &payload)
}

pub fn png_profile(name: &[u8], profile: &[u8]) -> Vec<u8> {
    let mut payload = name.to_vec();
    payload.extend_from_slice(&[0, 0]);
    payload.extend_from_slice(&zlib(profile));
    png_chunk(b"iCCP", &payload)
}

/// A 2 x 2 8-bit RGB PNG of `before`, its image data, and `after`.
pub fn png_with_chunks(before: &[Vec<u8>], after: &[Vec<u8>]) -> Vec<u8> {
    let mut chunks = before.to_vec();
    chunks.push(png_idat(&rgb_page_samples(), 6));
    chunks.extend_from_slice(after);
    png_from_chunks((2, 2), 8, 2, false, &chunks)
}

// ---------------------------------------------------------------------------
// JPEG segments

pub fn jpeg_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0xff, marker];
    out.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn jpeg_exif(block: &[u8]) -> Vec<u8> {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(block);
    jpeg_segment(0xe1, &payload)
}

pub fn jpeg_xmp(packet: &[u8]) -> Vec<u8> {
    let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    payload.extend_from_slice(packet);
    jpeg_segment(0xe1, &payload)
}

/// Profile segment `sequence` of `count`.
pub fn jpeg_profile(sequence: u8, count: u8, part: &[u8]) -> Vec<u8> {
    let mut payload = b"ICC_PROFILE\0".to_vec();
    payload.extend_from_slice(&[sequence, count]);
    payload.extend_from_slice(part);
    jpeg_segment(0xe2, &payload)
}

/// An 8 x 8 baseline RGB JPEG of one colour whose segments before the
/// tables are `segments`, in place of the encoder's own `APP0`.
pub fn jpeg_with_segments(segments: &[Vec<u8>]) -> Vec<u8> {
    let encoded = baseline_jpeg(image::ExtendedColorType::Rgb8, (8, 8), &[120; 8 * 8 * 3]);
    assert_eq!(encoded[..4], [0xff, 0xd8, 0xff, 0xe0], "SOI, then APP0");
    let app0 = 2 + 2 + usize::from(u16::from_be_bytes([encoded[4], encoded[5]]));
    let mut out = vec![0xff, 0xd8];
    for segment in segments {
        out.extend_from_slice(segment);
    }
    out.extend_from_slice(&encoded[app0..]);
    out
}

// ---------------------------------------------------------------------------
// WebP chunks

/// The image chunk (`VP8L`) of a lossless 8 x 8 RGB WebP of one colour.
pub fn webp_image_chunk() -> Vec<u8> {
    let file = lossless_webp(image::ExtendedColorType::Rgb8, (8, 8), &[120; 8 * 8 * 3]);
    assert_eq!(&file[12..16], b"VP8L", "a simple lossless file");
    file[12..].to_vec()
}

/// An extended 8 x 8 WebP: `VP8X` with the flags for what is given, the
/// profile, the image, the EXIF block, the XMP packet, then `extra`.
pub fn webp_with(
    profile: Option<&[u8]>,
    exif: Option<&[u8]>,
    xmp: Option<&[u8]>,
    extra: &[u8],
) -> Vec<u8> {
    let flags = (u8::from(profile.is_some()) * 0x20)
        | (u8::from(exif.is_some()) * 0x08)
        | (u8::from(xmp.is_some()) * 0x04);
    let mut chunks = vp8x(flags, (8, 8));
    if let Some(profile) = profile {
        chunks.extend(riff_chunk(b"ICCP", profile));
    }
    chunks.extend(webp_image_chunk());
    if let Some(exif) = exif {
        chunks.extend(riff_chunk(b"EXIF", exif));
    }
    if let Some(xmp) = xmp {
        chunks.extend(riff_chunk(b"XMP ", xmp));
    }
    chunks.extend_from_slice(extra);
    webp_from_chunks(&chunks)
}

// ---------------------------------------------------------------------------
// Files with something planted in every place that can hold text

/// One thing written into a file that says something about a person, a
/// device, a place or a time.
#[derive(Clone, Debug)]
pub struct Planted {
    /// Where in the file it is.
    pub place: &'static str,
    /// What to look for in a response.
    pub text: String,
    /// Whether the metadata tree of an unmasked session shows it. What the
    /// tree never shows (a maker note's bytes, a block that is not parsed)
    /// must still not turn up in a masked one.
    pub shown: bool,
}

/// A file and everything planted in it.
pub struct PlantedFile {
    pub name: &'static str,
    pub bytes: Vec<u8>,
    pub planted: Vec<Planted>,
}

struct Planter {
    prefix: &'static str,
    planted: Vec<Planted>,
}

impl Planter {
    fn new(prefix: &'static str) -> Self {
        Self {
            prefix,
            planted: Vec::new(),
        }
    }

    /// A string found nowhere else, recorded as planted at `place`.
    fn text(&mut self, place: &'static str, shown: bool) -> String {
        let text = format!("{}{:02}qz", self.prefix, self.planted.len());
        self.planted.push(Planted {
            place,
            text: text.clone(),
            shown,
        });
        text
    }

    /// Records `text`, which the caller writes itself, as planted.
    fn given(&mut self, place: &'static str, text: &str) {
        self.planted.push(Planted {
            place,
            text: text.to_string(),
            shown: true,
        });
    }

    /// The entries of an image directory that say who, with what and when.
    fn image_entries(&mut self) -> Dir {
        self.given("DateTime", "1987:06:05 04:03:02");
        vec![
            (269, V::ascii(&self.text("DocumentName", true))),
            (270, V::ascii(&self.text("ImageDescription", true))),
            (271, V::ascii(&self.text("Make", true))),
            (272, V::ascii(&self.text("Model", true))),
            (285, V::ascii(&self.text("PageName", true))),
            (305, V::ascii(&self.text("Software", true))),
            (306, V::ascii("1987:06:05 04:03:02")),
            (315, V::ascii(&self.text("Artist", true))),
            (316, V::ascii(&self.text("HostComputer", true))),
            (33432, V::ascii(&self.text("Copyright", true))),
            (
                33723,
                V::Undefined(self.text("IPTC block", false).into_bytes()),
            ),
            (40092, V::utf16(&self.text("XPComment", true))),
            (40093, V::utf16(&self.text("XPAuthor", true))),
            // Tags no table names: text, and a number that could be a serial.
            (65000, V::ascii(&self.text("private text tag", true))),
            (65001, V::Long(vec![self.number("private number tag")])),
        ]
    }

    /// A number found nowhere else, recorded as planted and shown.
    fn number(&mut self, place: &'static str) -> u32 {
        let number = 918_273_600 + self.planted.len() as u32;
        self.given(place, &number.to_string());
        number
    }

    fn exif_entries(&mut self) -> Dir {
        self.given("DateTimeOriginal", "1988:07:06 05:04:03");
        let mut comment = b"ASCII\0\0\0".to_vec();
        comment.extend_from_slice(self.text("UserComment", true).as_bytes());
        vec![
            (36864, V::Undefined(b"0232".to_vec())),
            (36867, V::ascii("1988:07:06 05:04:03")),
            (
                37500,
                V::Undefined(self.text("MakerNote", false).into_bytes()),
            ),
            (37510, V::Undefined(comment)),
            (42016, V::ascii(&self.text("ImageUniqueID", true))),
            (42032, V::ascii(&self.text("CameraOwnerName", true))),
            (42033, V::ascii(&self.text("BodySerialNumber", true))),
            (42036, V::ascii(&self.text("LensModel", true))),
            (42037, V::ascii(&self.text("LensSerialNumber", true))),
            (64000, V::ascii(&self.text("private EXIF tag", true))),
        ]
    }

    fn gps_entries(&mut self) -> Dir {
        self.given("GPSLatitude", "48123/1000");
        self.given("GPSLongitude", "2987/1000");
        self.given("GPSDateStamp", "1989:08:07");
        vec![
            (1, V::ascii("N")),
            (2, V::Rational(vec![(48123, 1000), (0, 1), (0, 1)])),
            (3, V::ascii("E")),
            (4, V::Rational(vec![(2987, 1000), (0, 1), (0, 1)])),
            (
                28,
                V::Undefined(self.text("GPSAreaInformation", false).into_bytes()),
            ),
            (29, V::ascii("1989:08:07")),
        ]
    }

    /// An EXIF block with every directory.
    fn exif_block(&mut self) -> Vec<u8> {
        let ifd0 = self.image_entries();
        let exif = self.exif_entries();
        let gps = self.gps_entries();
        let interop = vec![(1, V::ascii(&self.text("InteroperabilityIndex", true)))];
        let thumbnail = vec![
            (271, V::ascii(&self.text("Make of the thumbnail", true))),
            (513, V::Long(vec![self.number("thumbnail offset")])),
            (514, V::Long(vec![0])),
        ];
        tiff_block(&Block {
            ifd0,
            exif: Some(exif),
            gps: Some(gps),
            interop: Some(interop),
            next: vec![thumbnail],
            ..Block::default()
        })
        .bytes
    }

    fn xmp(&mut self) -> Vec<u8> {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><dc:creator>{}</dc:creator></x:xmpmeta>",
            self.text("XMP packet", true)
        )
        .into_bytes()
    }

    fn finish(self, name: &'static str, bytes: Vec<u8>) -> PlantedFile {
        PlantedFile {
            name,
            bytes,
            planted: self.planted,
        }
    }
}

/// A PNG with something planted in every chunk that can hold text.
pub fn planted_png() -> PlantedFile {
    let mut plant = Planter::new("pngsecret");
    let class = *b"zqPc";
    plant.given("ICC class", "zqPc");
    let profile = icc_with_description(&class, &plant.text("ICC description", true));
    plant.given("tIME", "1986-05-04 03:02:01");
    let before = vec![
        png_chunk(b"sBIT", &[8, 8, 8]),
        png_chunk(b"pHYs", &[0, 0, 0x0b, 0x13, 0, 0, 0x0b, 0x13, 1]),
        png_chunk(b"tIME", &[0x07, 0xc2, 5, 4, 3, 2, 1]),
        png_profile(plant.text("iCCP profile name", true).as_bytes(), &profile),
        png_text(
            plant.text("tEXt keyword", true).as_bytes(),
            plant.text("tEXt text", true).as_bytes(),
        ),
        png_compressed_text(
            plant.text("zTXt keyword", true).as_bytes(),
            plant.text("zTXt text", true).as_bytes(),
        ),
        png_international_text(
            plant.text("iTXt keyword", true).as_bytes(),
            plant.text("iTXt language tag", false).as_bytes(),
            plant.text("iTXt translated keyword", false).as_bytes(),
            plant.text("iTXt text", true).as_bytes(),
            false,
        ),
        png_international_text(
            plant.text("compressed iTXt keyword", true).as_bytes(),
            b"",
            b"",
            plant.text("compressed iTXt text", true).as_bytes(),
            true,
        ),
        png_chunk(b"eXIf", &plant.exif_block()),
        png_chunk(b"prVt", plant.text("private chunk", false).as_bytes()),
    ];
    let after = vec![png_text(
        b"Comment",
        plant.text("tEXt after the image", true).as_bytes(),
    )];
    let bytes = png_with_chunks(&before, &after);
    plant.finish("planted.png", bytes)
}

/// A JPEG with something planted in every segment that can hold text.
pub fn planted_jpeg() -> PlantedFile {
    let mut plant = Planter::new("jpgsecret");
    plant.given("ICC class", "zqJc");
    let profile = icc_with_description(b"zqJc", &plant.text("ICC description", true));
    let mut photoshop = b"Photoshop 3.0\0".to_vec();
    photoshop.extend_from_slice(plant.text("Photoshop block", false).as_bytes());
    let mut second = Planter::new("jpgsecond");
    let second_exif = tiff_block(&Block {
        ifd0: vec![(315, V::ascii(&second.text("a second EXIF segment", false)))],
        ..Block::default()
    })
    .bytes;
    let segments = vec![
        jpeg_segment(0xe0, b"JFIF\0\x01\x01\x01\0\x48\0\x48\0\0"),
        jpeg_exif(&plant.exif_block()),
        jpeg_xmp(&plant.xmp()),
        jpeg_profile(1, 1, &profile),
        jpeg_segment(0xed, &photoshop),
        jpeg_segment(0xfe, plant.text("COM", true).as_bytes()),
        jpeg_exif(&second_exif),
        jpeg_segment(0xfe, plant.text("a second COM", true).as_bytes()),
    ];
    let mut planted = plant.finish("planted.jpg", jpeg_with_segments(&segments));
    planted.planted.extend(second.planted);
    planted
}

/// A two-page TIFF with something planted in every tag that can hold text.
pub fn planted_tiff() -> PlantedFile {
    let mut plant = Planter::new("tifsecret");
    plant.given("ICC class", "zqTc");
    let profile = icc_with_description(b"zqTc", &plant.text("ICC description", true));
    let mut ifd0 = rgb_page_tags();
    ifd0.extend(plant.image_entries());
    ifd0.push((700, V::Byte(plant.xmp())));
    ifd0.push((34675, V::Undefined(profile.clone())));
    let exif = plant.exif_entries();
    let gps = plant.gps_entries();
    let interop = vec![(1, V::ascii(&plant.text("InteroperabilityIndex", true)))];
    // The second page carries the same profile, as pages of one image do.
    let mut second = rgb_page_tags();
    second.push((
        270,
        V::ascii(&plant.text("ImageDescription of page 1", true)),
    ));
    second.push((34675, V::Undefined(profile)));
    let bytes = tiff_block(&Block {
        data: rgb_page_samples(),
        ifd0,
        exif: Some(exif),
        gps: Some(gps),
        interop: Some(interop),
        next: vec![second],
        ..Block::default()
    })
    .bytes;
    plant.finish("planted.tif", bytes)
}

/// A WebP with something planted in every chunk that can hold text.
pub fn planted_webp() -> PlantedFile {
    let mut plant = Planter::new("webpsecret");
    plant.given("ICC class", "zqWc");
    let profile = icc_with_unicode_description(b"zqWc", &plant.text("ICC description", true));
    let exif = plant.exif_block();
    let xmp = plant.xmp();
    let extra = riff_chunk(b"zqzq", plant.text("private chunk", false).as_bytes());
    let bytes = webp_with(Some(&profile), Some(&exif), Some(&xmp), &extra);
    plant.finish("planted.webp", bytes)
}

pub fn planted_files() -> Vec<PlantedFile> {
    vec![
        planted_png(),
        planted_jpeg(),
        planted_tiff(),
        planted_webp(),
    ]
}

// ---------------------------------------------------------------------------
// Reading a tree in a test

/// The tree as one line per node, depth first, for comparing and for
/// reading a failure: `path | keyword | vr | value`. `path` is the `tag`s
/// from the top joined by `/`. A value is `"text"`, a number, `[numbers]`
/// (then ` of N` when it states a longer count), `<N bytes>`, `!` for a
/// problem (its words are not part of any contract), and `{` for a group,
/// whose children follow.
pub fn rows(nodes: &Value) -> Vec<String> {
    fn number(value: &Value) -> String {
        let value = value.as_f64().expect("number");
        if value.fract() == 0.0 && value.abs() < 1e15 {
            format!("{}", value as i64)
        } else {
            format!("{value}")
        }
    }
    fn walk(nodes: &Value, prefix: &str, out: &mut Vec<String>) {
        for node in nodes.as_array().expect("nodes") {
            let tag = node["tag"].as_str().expect("tag");
            let path = if prefix.is_empty() {
                tag.to_string()
            } else {
                format!("{prefix}/{tag}")
            };
            let value = &node["value"];
            let shown = match value["type"].as_str().expect("value type") {
                "string" => format!("{:?}", value["value"].as_str().expect("text")),
                "number" => number(&value["value"]),
                "numbers" => {
                    let list = value["value"]
                        .as_array()
                        .expect("numbers")
                        .iter()
                        .map(number)
                        .collect::<Vec<_>>()
                        .join(", ");
                    match value["total"].as_u64() {
                        Some(total) => format!("[{list}] of {total}"),
                        None => format!("[{list}]"),
                    }
                }
                "binary" => format!("<{} bytes>", value["length"]),
                "error" => "!".to_string(),
                "sequence" => "{".to_string(),
                other => panic!("unexpected value type {other}"),
            };
            out.push(format!(
                "{path} | {} | {} | {shown}",
                node["keyword"].as_str().expect("keyword"),
                node["vr"].as_str().expect("vr"),
            ));
            if let Some(items) = value["items"].as_array() {
                for item in items {
                    walk(item, &path, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(nodes, "", &mut out);
    out
}

// ---------------------------------------------------------------------------
// Files written by Pillow

fn embedded(hex: &[&str]) -> Vec<u8> {
    hex.concat()
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("hex digit")
        })
        .collect()
}

/// What every Pillow file's EXIF holds: `Make` "Aperture Works", `Model`
/// "Lumen 7", `Orientation` 6, `DateTime` "2021:03:04 05:06:07", `Artist`
/// "Robin Example"; in the EXIF directory `ExposureTime` 1/250, ISO 400,
/// `DateTimeOriginal` as `DateTime`, pixel dimensions 8 x 8; in the GPS
/// directory latitude N 48, 51, 29.87 and longitude E 2, 17, 40.03.
///
/// JPEG at quality 50 and 72 dpi, with the comment "made for a metadata
/// test".
pub fn pillow_jpeg() -> Vec<u8> {
    embedded(PILLOW_JPEG)
}

/// PNG at 300 dpi with `tEXt` Title "A small square", `zTXt` Author "Robin
/// Example", `iTXt` Description "café — naïve" (language `fr`), the EXIF
/// above, and Pillow's built-in sRGB profile (588 bytes, version 4.4,
/// description "sRGB built-in") under the name "ICC Profile".
pub fn pillow_png() -> Vec<u8> {
    embedded(PILLOW_PNG)
}

/// Uncompressed TIFF at 96 dpi with `ImageDescription` "a described page",
/// `Software` "sample writer 1.0", `DateTime` "2021:03:04 05:06:07" and
/// `Artist` "Robin Example".
pub fn pillow_tiff() -> Vec<u8> {
    embedded(PILLOW_TIFF)
}

/// Lossless WebP with the EXIF above, the same profile, and an XMP packet
/// naming the creator "Robin Example".
pub fn pillow_webp() -> Vec<u8> {
    embedded(PILLOW_WEBP)
}

const PILLOW_JPEG: &[&str] = &[
    "ffd8ffe000104a46494600010101004800480000ffe101684578696600004d4d002a00000008",
    "0007010f00020000000f00000062011000020000000800000072011200030000000100060000",
    "01320002000000140000007a013b00020000000e0000008e87690004000000010000009c8825",
    "000400000001000000fa00000000417065727475726520576f726b7300004c756d656e203700",
    "323032313a30333a30342030353a30363a303700526f62696e204578616d706c65000005829a",
    "000500000001000000de8827000300000001019000009003000200000014000000e6a0020003",
    "0000000100080000a003000300000001000800000000000000000001000000fa323032313a30",
    "333a30342030353a30363a303700000400010002000000024e00000000020005000000030000",
    "0130000300020000000245000000000400050000000300000148000000000000003000000001",
    "000000330000000100000bab000000640000000200000001000000110000000100000fa30000",
    "0064fffe001a6d61646520666f722061206d657461646174612074657374ffdb004300100b0c",
    "0e0c0a100e0d0e1211101318281a181616183123251d283a333d3c3933383740485c4e404457",
    "453738506d51575f626768673e4d71797064785c656763ffdb0043011112121815182f1a1a2f",
    "6342384263636363636363636363636363636363636363636363636363636363636363636363",
    "63636363636363636363636363636363ffc00011080008000803012200021101031101ffc400",
    "1f0000010501010101010100000000000000000102030405060708090a0bffc400b510000201",
    "0303020403050504040000017d01020300041105122131410613516107227114328191a10823",
    "42b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748",
    "494a535455565758595a636465666768696a737475767778797a838485868788898a92939495",
    "969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7",
    "d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101",
    "010000000000000102030405060708090a0bffc400b511000201020404030407050404000102",
    "77000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a",
    "162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a63",
    "6465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6",
    "a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8",
    "e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f007514515e91c27fffd9",
];

const PILLOW_PNG: &[&str] = &[
    "89504e470d0a1a0a0000000d49484452000000080000000808020000004b6d29dc0000017569",
    "4343504943432050726f66696c650000789c7591bd4b425118c67f5a5199e150435483834590",
    "42144463d9e022116690d5a2d7af40ed72af12d21ab434080d514b5f43ff41ad416b41101441",
    "4473635f4bc8ed3d2a28a1e772eefbe339e77979ef73c11ecc6859b37d16b2b9bc110af8dd2b",
    "915577e73b0eba817186a29aa9cf2d2e0669b97e1eb1a9fae053bd5adf6bba7ae20953035b97",
    "f0b4a61b79619986e0565e57bc27dcafa5a371e11361af21030adf2a3d56e537c5a92a7f2936",
    "c2a179b0ab9eee5403c71a584b1b59e131614f3653d06af3a82f712672cb4b5207650f631222",
    "801f37310a6c90218f4f6a4e326bee9ba8f816d8148f266f9d22863852a4c5eb15b5205d1352",
    "93a227e4c95054b9ffcfd34c4e4d56bb3bfdd0f16a599f23d0b90fe59265fd9e5a56f90cda5e",
    "e03a57f76f4a4e33dfa297ea9ae7185c3b707953d7620770b50b03cf7ad48856a436d9f66412",
    "3e2ea037027df7e058ab66553be7fc09c2dbf28beee0f00846e5be6bfd0ffae86807052a996f",
    "00000014744558745469746c65004120736d616c6c207371756172654d420adc0000001d7a54",
    "5874417574686f720000789c0bca4fcacc5370ad48cc2dc8490500217204e774555e87000000",
    "2d695458744465736372697074696f6e0000006672004465736372697074696f6e00636166c3",
    "a920e28094206e61c3af7665ca517fd4000000097048597300002e2300002e230178a53f7600",
    "000160655849664d4d002a000000080007010f00020000000f00000062011000020000000800",
    "00007201120003000000010006000001320002000000140000007a013b00020000000e000000",
    "8e87690004000000010000009c8825000400000001000000fa00000000417065727475726520",
    "576f726b7300004c756d656e203700323032313a30333a30342030353a30363a303700526f62",
    "696e204578616d706c65000005829a000500000001000000de88270003000000010190000090",
    "03000200000014000000e6a00200030000000100080000a00300030000000100080000000000",
    "0000000001000000fa323032313a30333a30342030353a30363a303700000400010002000000",
    "024e000000000200050000000300000130000300020000000245000000000400050000000300",
    "000148000000000000003000000001000000330000000100000bab0000006400000002000000",
    "01000000110000000100000fa300000064f2c7c11e0000001449444154789c638cea39c1800d",
    "3061151db41200533d01bebdf202bf0000000049454e44ae426082",
];

const PILLOW_TIFF: &[&str] = &[
    "49492a0008000000110000010400010000000800000001010400010000000800000002010300",
    "03000000da0000000301030001000000010000000601030001000000020000000e0102001100",
    "0000e00000001101040001000000360100001501030001000000030000001601040001000000",
    "080000001701040001000000c00000001a01050001000000f20000001b01050001000000fa00",
    "00001c0103000100000001000000280103000100000002000000310102001200000002010000",
    "3201020014000000140100003b0102000e000000280100000000000008000800080061206465",
    "73637269626564207061676500006000000001000000600000000100000073616d706c652077",
    "726974657220312e3000323032313a30333a30342030353a30363a303700526f62696e204578",
    "616d706c65005a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8c",
    "c85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a",
    "8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc8",
    "5a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8c",
    "c85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a8cc85a",
    "8cc85a8cc85a8cc8",
];

const PILLOW_WEBP: &[&str] = &[
    "524946462a05000057454250565038580a0000002c000000070000070000494343504c020000",
    "0000024c6c636d73044000006d6e74725247422058595a2007ea000a00090000002b001b6163",
    "73704150504c0000000000000000000000000000000000000000000000000000f6d600010000",
    "0000d32d6c636d73000000000000000000000000000000000000000000000000000000000000",
    "00000000000000000000000000000000000b6465736300000108000000366370727400000140",
    "0000004c777470740000018c0000001463686164000001a00000002c7258595a000001cc0000",
    "00146258595a000001e0000000146758595a000001f400000014725452430000020800000020",
    "6754524300000208000000206254524300000208000000206368726d00000228000000246d6c",
    "756300000000000000010000000c656e55530000001a0000001c007300520047004200200062",
    "00750069006c0074002d0069006e00006d6c756300000000000000010000000c656e55530000",
    "00300000001c004e006f00200063006f0070007900720069006700680074002c002000750073",
    "006500200066007200650065006c007958595a20000000000000f6d6000100000000d32d7366",
    "33320000000000010c42000005defffff325000007930000fd90fffffba1fffffda2000003dc",
    "0000c06e58595a200000000000006fa0000038f50000039058595a20000000000000249f0000",
    "0f840000b6c358595a2000000000000062970000b787000018d9706172610000000000030000",
    "000266660000f2a700000d59000013d000000a5b6368726d00000000000300000000a3d70000",
    "547b00004ccd0000999a0000266600000f5c5650384c110000002f07c001000750c66a15b9ff",
    "8188e87f000045584946600100004d4d002a000000080007010f00020000000f000000620110",
    "0002000000080000007201120003000000010006000001320002000000140000007a013b0002",
    "0000000e0000008e87690004000000010000009c8825000400000001000000fa000000004170",
    "65727475726520576f726b7300004c756d656e203700323032313a30333a30342030353a3036",
    "3a303700526f62696e204578616d706c65000005829a000500000001000000de882700030000",
    "0001019000009003000200000014000000e6a00200030000000100080000a003000300000001",
    "000800000000000000000001000000fa323032313a30333a30342030353a30363a3037000004",
    "00010002000000024e0000000002000500000003000001300003000200000002450000000004",
    "00050000000300000148000000000000003000000001000000330000000100000bab00000064",
    "0000000200000001000000110000000100000fa300000064584d5020350100003c3f78706163",
    "6b657420626567696e3d22222069643d2257354d304d7043656869487a7265537a4e54637a6b",
    "633964223f3e3c783a786d706d65746120786d6c6e733a783d2261646f62653a6e733a6d6574",
    "612f223e3c7264663a52444620786d6c6e733a7264663d22687474703a2f2f7777772e77332e",
    "6f72672f313939392f30322f32322d7264662d73796e7461782d6e7323223e3c7264663a4465",
    "736372697074696f6e20786d6c6e733a64633d22687474703a2f2f7075726c2e6f72672f6463",
    "2f656c656d656e74732f312e312f223e3c64633a63726561746f723e526f62696e204578616d",
    "706c653c2f64633a63726561746f723e3c2f7264663a4465736372697074696f6e3e3c2f7264",
    "663a5244463e3c2f783a786d706d6574613e3c3f787061636b657420656e643d2277223f3e00",
];
