//! A file is checked against its catalog entry before anything is sized
//! from it (`pixels::decode_raster_frame`, "The entry is checked against
//! the file").
//!
//! Every size a decoder would take from the file is one of these: the image
//! header a format's container repeats in its bitstream, a tag a TIFF page
//! holds twice, a tile far larger than the image it tiles, a chunk that
//! claims more bytes than the file has. Each must be compared with the
//! entry, or refused, before a decoder allocates or loops by it.

use super::bounds::{assert_hostile, assert_within_limits, decode, list, Hostile, Outcome};
use super::bounds::{BUFFER, MIB};
use super::raster_files::{self as files, CountedFile, TiffPage, TiffValue};
use dcmview::pixels;
use image::ExtendedColorType;

// ---------------------------------------------------------------------------
// WebP

/// The chunks of a WebP file after its twelve-byte header: name and payload.
fn riff_chunks(webp: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut chunks = Vec::new();
    let mut at = 12;
    while at + 8 <= webp.len() {
        let name: [u8; 4] = webp[at..at + 4].try_into().expect("chunk name");
        let length = u32::from_le_bytes(webp[at + 4..at + 8].try_into().expect("length")) as usize;
        chunks.push((name, webp[at + 8..at + 8 + length].to_vec()));
        at += 8 + length + length % 2;
    }
    chunks
}

fn chunk_of(webp: &[u8], name: &[u8; 4]) -> Vec<u8> {
    riff_chunks(webp)
        .into_iter()
        .find(|(found, _)| found == name)
        .map(|(_, payload)| payload)
        .unwrap_or_else(|| panic!("no {} chunk", String::from_utf8_lossy(name)))
}

/// A lossy bitstream (the payload of a `VP8 ` chunk) whose frame header
/// declares `width` x `height`: bytes 6 to 10, after the frame tag and the
/// start code.
fn vp8_sized(mut bitstream: Vec<u8>, (width, height): (u16, u16)) -> Vec<u8> {
    bitstream[6..8].copy_from_slice(&width.to_le_bytes());
    bitstream[8..10].copy_from_slice(&height.to_le_bytes());
    bitstream
}

/// An `ANMF` chunk: a frame of `size` at `at` on the canvas, shown for
/// 100 ms, holding `chunks`.
fn anmf((x, y): (u32, u32), (width, height): (u32, u32), chunks: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    for value in [x / 2, y / 2, width - 1, height - 1, 100] {
        payload.extend_from_slice(&value.to_le_bytes()[..3]);
    }
    payload.push(0);
    payload.extend_from_slice(chunks);
    files::riff_chunk(b"ANMF", &payload)
}

fn extended(flags: u8, canvas: (u32, u32), chunks: &[&[u8]]) -> Vec<u8> {
    let mut body = files::vp8x(flags, canvas);
    for chunk in chunks {
        body.extend_from_slice(chunk);
    }
    files::webp_from_chunks(&body)
}

const ALPHA: u8 = 0x10;
const ANIMATION: u8 = 0x02;
const PROFILE: u8 = 0x20;

/// A lossy bitstream states its own width and height, and its decoder
/// allocates by them. In an extended file the catalog entry is made from
/// the canvas, so the two are compared before any pixel is decoded: a
/// 16 x 16 entry never buys the planes of a 16,383 x 16,383 frame.
#[tokio::test]
async fn a_webp_bitstream_larger_than_its_canvas_is_refused_before_it_is_decoded() {
    use Outcome::Fails;
    let lossy = chunk_of(&files::lossy_webp(), b"VP8 ");
    let huge = files::riff_chunk(b"VP8 ", &vp8_sized(lossy.clone(), (16_383, 16_383)));
    let alpha = files::riff_chunk(b"ALPH", &chunk_of(&files::lossy_alpha_webp(), b"ALPH"));
    let anim = files::riff_chunk(b"ANIM", &[0; 6]);
    // The frame header and first partition overwritten with ones, as a
    // damaged file has them: 16,144 x 16,383.
    let mut overwritten = files::lossy_alpha_webp();
    overwritten[0x3b..0x5c].fill(0xff);
    let honest = extended(0, (16, 16), &[&files::riff_chunk(b"VP8 ", &lossy)]);
    let mut framed_with_alpha = alpha.clone();
    framed_with_alpha.extend_from_slice(&huge);

    assert_hostile(vec![
        Hostile::new(
            "webp lossy image larger than its canvas",
            extended(0, (16, 16), &[&huge]),
            Fails,
        ),
        Hostile::new(
            "webp lossy image with alpha larger than its canvas",
            extended(ALPHA, (16, 16), &[&alpha, &huge]),
            Fails,
        ),
        Hostile::new(
            "webp animation whose first frame's image is larger than the frame",
            extended(
                ANIMATION,
                (16, 16),
                &[&anim, &anmf((0, 0), (16, 16), &huge)],
            ),
            Fails,
        ),
        Hostile::new(
            "webp lossy image with an overwritten header",
            overwritten,
            Fails,
        ),
        Hostile::new(
            "webp animation frame with alpha whose image is larger than the frame",
            extended(
                ANIMATION | ALPHA,
                (16, 16),
                &[&anim, &anmf((0, 0), (16, 16), &framed_with_alpha)],
            ),
            Fails,
        ),
        // No animation flag, and no image but the one inside a frame.
        Hostile::new(
            "webp still whose only image is an animation frame's",
            extended(0, (16, 16), &[&anim, &anmf((0, 0), (16, 16), &huge)]),
            Fails,
        ),
        // The file that was listed is now one of these.
        Hostile::new("webp replaced by a larger lossy image", honest, Fails).replaced_by(extended(
            0,
            (16, 16),
            &[&huge],
        )),
    ])
    .await;
}

/// A profile chunk's declared length is not a size to allocate: one that
/// reaches past the end of the file, or is more than a frame carries, is
/// left out and the frame is decoded.
#[tokio::test]
async fn a_webp_profile_chunk_is_never_allocated_at_a_length_it_only_declares() {
    let image = files::riff_chunk(
        b"VP8L",
        &chunk_of(
            &files::lossless_webp(ExtendedColorType::Rgb8, (8, 8), &[90; 8 * 8 * 3]),
            b"VP8L",
        ),
    );
    // The heap limit of an 8 x 8 RGB frame from a file this small is a
    // little over 16 MiB. The chunk claims that much.
    let mut claim = b"ICCP".to_vec();
    claim.extend_from_slice(&(16 * MIB as u32).to_le_bytes());
    claim.extend_from_slice(&[0x33; 64]);
    let too_large = files::icc_profile(b"RGB ", pixels::RASTER_ICC_MAX_BYTES + 2, 0x33);
    let largest = files::icc_profile(b"RGB ", pixels::RASTER_ICC_MAX_BYTES, 0x33);
    let cases = [
        (
            "claim.webp",
            extended(PROFILE, (8, 8), &[&image, &claim]),
            None,
        ),
        (
            "too-large.webp",
            extended(
                PROFILE,
                (8, 8),
                &[&files::riff_chunk(b"ICCP", &too_large), &image],
            ),
            None,
        ),
        (
            "largest.webp",
            extended(
                PROFILE,
                (8, 8),
                &[&files::riff_chunk(b"ICCP", &largest), &image],
            ),
            Some(largest.len()),
        ),
    ];
    let listed: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(name, bytes, _)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for (name, bytes, profile) in &cases {
        let entry = scan.entry(name);
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert_within_limits(name, entry, bytes.len() as u64, &cost);
        let frame = cost
            .result
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(frame.icc_profile.as_ref().map(Vec::len), *profile, "{name}");
    }
}

/// What the checks above must not refuse: an extended file around a lossy
/// image of the canvas's size, and an animation whose first frame covers
/// part of the canvas.
#[tokio::test]
async fn webp_files_whose_bitstreams_fit_their_canvas_decode() {
    let lossy = files::riff_chunk(b"VP8 ", &chunk_of(&files::lossy_webp(), b"VP8 "));
    let alpha = files::riff_chunk(b"ALPH", &chunk_of(&files::lossy_alpha_webp(), b"ALPH"));
    let anim = files::riff_chunk(b"ANIM", &[0; 6]);
    let patch = files::riff_chunk(
        b"VP8L",
        &chunk_of(
            &files::lossless_webp(ExtendedColorType::Rgb8, (4, 4), &[200, 100, 50].repeat(16)),
            b"VP8L",
        ),
    );
    let mut lossy_with_alpha = alpha.clone();
    lossy_with_alpha.extend_from_slice(&lossy);
    // (name, file, a pixel and the colour it has within 4)
    type Sample = (usize, [u8; 3]);
    let cases: [(&str, Vec<u8>, Sample); 4] = [
        (
            "extended-lossy.webp",
            extended(0, (16, 16), &[&lossy]),
            (0, [90, 140, 200]),
        ),
        (
            "extended-lossy-alpha.webp",
            extended(ALPHA, (16, 16), &[&alpha, &lossy]),
            (0, [90, 140, 200]),
        ),
        // A 4 x 4 frame at (2, 2) on an 8 x 8 canvas.
        (
            "part-frame.webp",
            extended(ANIMATION, (8, 8), &[&anim, &anmf((2, 2), (4, 4), &patch)]),
            (3 * 8 + 3, [200, 100, 50]),
        ),
        (
            "lossy-alpha-frame.webp",
            extended(
                ANIMATION | ALPHA,
                (16, 16),
                &[&anim, &anmf((0, 0), (16, 16), &lossy_with_alpha)],
            ),
            (0, [90, 140, 200]),
        ),
    ];
    let listed: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(name, bytes, _)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for (name, bytes, (pixel, colour)) in &cases {
        let entry = scan.entry(name);
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert_within_limits(name, entry, bytes.len() as u64, &cost);
        let frame = cost
            .result
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let samples = entry.samples_per_pixel as usize;
        for (index, expected) in colour.iter().enumerate() {
            let found = frame.bytes[pixel * samples + index];
            assert!(
                found.abs_diff(*expected) <= 4,
                "{name}: sample {index} of pixel {pixel} is {found}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// TIFF

fn gray_tags((width, height): (u32, u32), bits: u16, format: u16) -> Vec<(u16, TiffValue)> {
    vec![
        (256, TiffValue::Long(vec![width])),
        (257, TiffValue::Long(vec![height])),
        (258, TiffValue::Short(vec![bits])),
        (259, TiffValue::Short(vec![1])),
        (262, TiffValue::Short(vec![1])),
        (277, TiffValue::Short(vec![1])),
        (339, TiffValue::Short(vec![format])),
    ]
}

/// A page of one tile. A tag in `more` replaces the page's own.
fn tile_page(
    size: (u32, u32),
    (bits, format): (u16, u16),
    tile: (u32, u32),
    more: &[(u16, TiffValue)],
    data: Vec<u8>,
) -> TiffPage {
    let mut tags = gray_tags(size, bits, format);
    tags.push((322, TiffValue::Long(vec![tile.0])));
    tags.push((323, TiffValue::Long(vec![tile.1])));
    for (tag, value) in more {
        tags.retain(|(existing, _)| existing != tag);
        tags.push((*tag, value.clone()));
    }
    TiffPage {
        tags,
        chunks: vec![data],
        tiled: true,
    }
}

/// `page` with a second entry for `tag`, after the one it has: the builder
/// keeps entries of one tag in the order given.
fn twice(mut page: TiffPage, tag: u16, value: TiffValue) -> TiffPage {
    page.tags.push((tag, value));
    page
}

fn one_page(page: TiffPage) -> Vec<u8> {
    files::tiff_file(false, &[page]).0
}

/// A page whose IFD holds a tag twice has two layouts, and readers disagree
/// on which entry counts (the linked decoder takes the last). Discovery
/// does not list such a page; a file that has become one since it was
/// listed is refused when it is decoded, whatever the tag: nothing is sized
/// by, decompressed as, or shown with the second entry.
#[tokio::test]
async fn a_tiff_page_that_repeats_a_tag_is_refused_when_it_is_decoded() {
    use Outcome::Fails;
    // 64 MiB of zeros, deflated.
    let zeros = files::zlib(&vec![0; 64 * MIB as usize]);
    let float_tile = |data: Vec<u8>| {
        tile_page(
            (16, 16),
            (32, 3),
            (16, 16),
            &[
                (259, TiffValue::Short(vec![8])),
                (317, TiffValue::Short(vec![3])),
            ],
            data,
        )
    };
    // Each file replaces one whose strip or tile is as long, so its page is
    // where the listed one was. (Zeros are their own floating-point
    // prediction, and a tile may hold more than its image needs.)
    let floats = one_page(float_tile(zeros.clone()));
    let plain = |strip: Vec<u8>| TiffPage::strip((64, 64), &[8], 1, strip);
    let listed = one_page(plain(vec![5; 64 * 64]));
    // A JPEG stream that declares 4,096 x 4,096, in a strip of the listed
    // one's length.
    let mut jpeg = files::baseline_jpeg(ExtendedColorType::L8, (64, 64), &[90; 64 * 64]);
    let marker = files::jpeg_frame_marker(&jpeg);
    jpeg[marker + 4..marker + 8].copy_from_slice(&[0x10, 0x00, 0x10, 0x00]);
    jpeg.resize(64 * 64, 0);

    assert_hostile(vec![
        // The first tile width is the image's; the second makes each row of
        // the floating-point predictor a gigabyte.
        Hostile::new("tiff with two tile widths", floats, Fails).replaced_by(one_page(twice(
            float_tile(zeros),
            322,
            TiffValue::Long(vec![1 << 28]),
        ))),
        // None, then JPEG: the compression this version does not decode.
        Hostile::new("tiff with two compressions", listed.clone(), Fails)
            .replaced_by(one_page(twice(plain(jpeg), 259, TiffValue::Short(vec![7])))),
        // BlackIsZero, then WhiteIsZero: the same samples, inverted.
        Hostile::new("tiff with two photometrics", listed.clone(), Fails).replaced_by(one_page(
            twice(plain(vec![5; 64 * 64]), 262, TiffValue::Short(vec![0])),
        )),
        Hostile::new("tiff with two strip sizes", listed.clone(), Fails).replaced_by(one_page(
            twice(plain(vec![5; 64 * 64]), 278, TiffValue::Long(vec![64])),
        )),
        // Tags no decoder reads are no exception.
        Hostile::new("tiff with two descriptions", listed.clone(), Fails).replaced_by(one_page(
            twice(
                plain(vec![5; 64 * 64]).with(270, TiffValue::Bytes(b"one\0".to_vec())),
                270,
                TiffValue::Bytes(b"two\0".to_vec()),
            ),
        )),
        Hostile::new("tiff with a private tag twice", listed, Fails).replaced_by(one_page(twice(
            plain(vec![5; 64 * 64]).with(40_000, TiffValue::Short(vec![1])),
            40_000,
            TiffValue::Short(vec![2]),
        ))),
    ])
    .await;
}

/// The linked decoder reads and discards what a tile holds beside the
/// image, row by row, so a tile far wider than its image buys work the
/// entry does not account for. A tile may be at most
/// `RASTER_TIFF_TILE_MARGIN - 1` pixels wider or longer than the image; one
/// that is larger is refused before any of it is read.
#[tokio::test]
async fn a_tiff_tile_far_larger_than_its_image_is_refused_before_it_is_read() {
    use Outcome::Fails;
    const MARGIN: u32 = pixels::RASTER_TIFF_TILE_MARGIN;
    // 64 MiB of zeros, deflated: what a tile four gigabytes wide would have
    // the decoder inflate and discard after each row of sixteen pixels.
    let zeros = files::zlib(&vec![0; 64 * MIB as usize]);
    let padded = one_page(tile_page(
        (16, 16),
        (8, 1),
        (u32::MAX, 16),
        &[(259, TiffValue::Short(vec![8]))],
        zeros,
    ));
    // The file is more than four buffers long; reading it for the tile is not
    // mistaken for reading its tags.
    assert!(padded.len() as u64 > 4 * BUFFER);
    let wide = (8 + MARGIN, 8);
    let long = (8, 8 + MARGIN);
    let floats = |tile: (u32, u32)| {
        one_page(tile_page(
            (8, 8),
            (64, 3),
            tile,
            &[
                (259, TiffValue::Short(vec![8])),
                (317, TiffValue::Short(vec![3])),
            ],
            files::zlib(&vec![0; (tile.0 * tile.1 * 8) as usize]),
        ))
    };
    assert_hostile(vec![
        // The page's tags are at the end of the file, a read or two away
        // from its header: nothing of the tile is inflated.
        Hostile::new("tiff tile four gigabytes wide", padded, Fails).reading_at_most(3 * BUFFER),
        Hostile::new(
            "tiff tile a margin wider than its image",
            one_page(tile_page(
                (8, 8),
                (8, 1),
                wide,
                &[],
                vec![5; (wide.0 * wide.1) as usize],
            )),
            Fails,
        ),
        Hostile::new(
            "tiff tile a margin longer than its image",
            one_page(tile_page(
                (8, 8),
                (8, 1),
                long,
                &[],
                vec![5; (long.0 * long.1) as usize],
            )),
            Fails,
        ),
        Hostile::new(
            "tiff tile of predicted floats a margin wider than its image",
            floats(wide),
            Fails,
        ),
    ])
    .await;
}

/// The IFD of `tiff`'s one page with its entries in the opposite order.
fn with_entries_reversed((mut tiff, pages): (Vec<u8>, Vec<u64>)) -> Vec<u8> {
    let ifd = pages[0] as usize;
    let count = usize::from(u16::from_le_bytes([tiff[ifd], tiff[ifd + 1]]));
    let entries: Vec<u8> = tiff[ifd + 2..ifd + 2 + 12 * count]
        .chunks(12)
        .rev()
        .flatten()
        .copied()
        .collect();
    tiff[ifd + 2..ifd + 2 + 12 * count].copy_from_slice(&entries);
    tiff
}

/// What the checks above must not refuse, each decoded to the samples it
/// stores: a tile up to the margin larger than its image, a strip declared
/// to hold every row there could be (the format's default), and an IFD
/// whose entries are not in ascending order.
#[tokio::test]
async fn tiff_layouts_within_the_rules_decode_to_their_stored_samples() {
    const MARGIN: u32 = pixels::RASTER_TIFF_TILE_MARGIN;
    let image: Vec<u8> = (0..64).collect();
    // The tile's rows: the image's eight pixels, then what the tile holds
    // beside them.
    let padded_tile = |(width, height): (u32, u32)| -> Vec<u8> {
        let mut tile = Vec::new();
        for row in 0..height as usize {
            let mut stored = vec![0xee; width as usize];
            if row < 8 {
                stored[..8].copy_from_slice(&image[row * 8..row * 8 + 8]);
            }
            tile.extend_from_slice(&stored);
        }
        tile
    };
    let wide = (8 + MARGIN - 1, 8);
    let long = (8, 8 + MARGIN - 1);
    let strip = TiffPage::strip((8, 8), &[8], 1, image.clone());
    let cases = [
        (
            "wide-tile.tif",
            one_page(tile_page((8, 8), (8, 1), wide, &[], padded_tile(wide))),
        ),
        (
            "long-tile.tif",
            one_page(tile_page((8, 8), (8, 1), long, &[], padded_tile(long))),
        ),
        (
            "every-row.tif",
            one_page(strip.clone().with(278, TiffValue::Long(vec![u32::MAX]))),
        ),
        (
            "descending.tif",
            with_entries_reversed(files::tiff_file(false, &[strip])),
        ),
    ];
    let listed: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(name, bytes)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for (name, bytes) in &cases {
        let entry = scan.entry(name);
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert_within_limits(name, entry, bytes.len() as u64, &cost);
        let frame = cost
            .result
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(frame.bytes, image, "{name}");
    }
}

/// Discovery reads a page's photometric interpretation from a SHORT or a
/// LONG, so a decode takes both: a WhiteIsZero page that stores it as a
/// LONG is listed `MONOCHROME1` and its frame holds the stored samples.
#[tokio::test]
async fn a_tiff_photometric_stored_as_a_long_decodes_as_it_is_listed() {
    let stored: Vec<u8> = (0..64).map(|index| index * 3).collect();
    let cases =
        [false, true].map(|big_endian| {
            files::tiff_file(
                big_endian,
                &[TiffPage::strip((8, 8), &[8], 0, stored.clone())
                    .with(262, TiffValue::Long(vec![0]))],
            )
            .0
        });
    let scan = list(&[("little.tif", &cases[0]), ("big.tif", &cases[1])]).await;
    for (name, bytes) in ["little.tif", "big.tif"].iter().zip(&cases) {
        let entry = scan.entry(name);
        assert_eq!(entry.photometric_interpretation, "MONOCHROME1", "{name}");
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert_within_limits(name, entry, bytes.len() as u64, &cost);
        let frame = cost
            .result
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(frame.bytes, stored, "{name}: the samples as stored");
    }
}

// ---------------------------------------------------------------------------
// PNG and JPEG

/// The image header is what a PNG or JPEG decoder sizes its buffers by, and
/// the entry is compared with it first (`bounds.rs` replaces a listed file
/// by a larger one). These are the other sizes those files can declare: an
/// animation's frame control ahead of the image, a second frame header, and
/// megabytes of application segments.
#[tokio::test]
async fn a_png_or_jpeg_is_sized_by_nothing_but_its_checked_image_header() {
    use Outcome::{Decodes, Fails};
    let pixels: Vec<u8> = (0..64).collect();
    // One frame, played once; then the frame's size and place.
    let control = |width: u32, height: u32| {
        let mut payload = 0_u32.to_be_bytes().to_vec();
        for value in [width, height, 0, 0] {
            payload.extend_from_slice(&value.to_be_bytes());
        }
        payload.extend_from_slice(&[0, 1, 0, 10, 0, 0]);
        files::png_chunk(b"fcTL", &payload)
    };
    let animated = |width: u32, height: u32| {
        files::png_from_chunks(
            (8, 8),
            8,
            0,
            false,
            &[
                files::png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]),
                control(width, height),
                files::png_idat(&pixels, 8),
            ],
        )
    };

    let small = files::baseline_jpeg(ExtendedColorType::L8, (16, 8), &[90; 16 * 8]);
    let marker = files::jpeg_frame_marker(&small);
    let header_length = usize::from(u16::from_be_bytes([small[marker + 1], small[marker + 2]]));
    let frame_header = marker - 1..marker + 1 + header_length;
    // A second frame header, of 60,000 x 4,000, after the first.
    let mut second = small[frame_header.clone()].to_vec();
    second[5..9].copy_from_slice(&[0x0f, 0xa0, 0xea, 0x60]);
    let mut two_headers = small.clone();
    two_headers.splice(frame_header.end..frame_header.end, second);
    // `APP2` segments by the megabyte: the 255 parts a profile may have, and
    // as many that are no profile.
    let segments = |label: &[u8], numbered: bool| {
        let mut out = small[..2].to_vec();
        for index in 0..255_u8 {
            let mut payload = label.to_vec();
            if numbered {
                payload.extend_from_slice(&[index + 1, 255]);
            }
            payload.resize(65_533, 0x33);
            out.extend_from_slice(&[0xff, 0xe2]);
            out.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
            out.extend_from_slice(&payload);
        }
        out.extend_from_slice(&small[2..]);
        out
    };

    assert_hostile(vec![
        Hostile::new(
            "png animation control of the image's size",
            animated(8, 8),
            Decodes,
        ),
        Hostile::new(
            "png animation control of two gigapixels a side",
            animated(0x7fff_ffff, 0x7fff_ffff),
            Fails,
        ),
        Hostile::new(
            "png animation control smaller than the image",
            animated(2, 2),
            Fails,
        ),
        Hostile::new(
            "jpeg with a second, larger frame header",
            two_headers,
            Fails,
        ),
        Hostile::new(
            "jpeg with sixteen megabytes of profile parts",
            segments(b"ICC_PROFILE\0", true),
            Decodes,
        ),
        Hostile::new(
            "jpeg with sixteen megabytes of other segments",
            segments(b"MPF\0", false),
            Decodes,
        ),
    ])
    .await;
}
