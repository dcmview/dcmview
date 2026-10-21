//! What a bilevel TIFF page costs: the frame, and little beside it, however
//! it is compressed, and a hostile strip refused before it costs more.
//!
//! The files are libtiff's own output (`raster_bilevel_files.rs`); a hostile
//! one is an honest one with a count or a tag value changed.

use super::bounds::{assert_within_limits, decode, list, noise, Hostile, Outcome, BUFFER, MIB};
use super::raster_bilevel_files::{
    card, fixture, small_fixtures, tag_values, with_tag, with_value,
};
use super::raster_files::{self as files, CountedFile, TiffPage};

/// What a bilevel decode of these fixtures may hold beside its frame: one
/// packed row, an input buffer, the Deflate or LZW decoder's state, two
/// Group 4 lines and tables of a few strips or tiles, all far below this.
/// A page of tens of thousands of strips holds a few MiB more for its
/// offset and count tables, as every TIFF page does.
const BESIDE_THE_FRAME: u64 = MIB;

/// The size of `g4-stripes-4096x4.tif`.
const STRIPES: (usize, usize) = (4096, 4);

/// The image of `g4-stripes-4096x4.tif`: a white first row, then stripes
/// two pixels wide that move one pixel left from each row to the next.
fn stripes((width, height): (usize, usize)) -> Vec<u8> {
    (0..height)
        .flat_map(|row| (0..width).map(move |column| u8::from(row > 0 && (column + row) % 4 < 2)))
        .collect()
}

/// `g4-stripes-4096x4.tif` read as narrower than it was written: `width`
/// pixels across and `rows` down, in one strip. The vertical codes of a
/// row's stripes beyond the new width all land on its last pixel. Read
/// 1,000 pixels wide, a row ends with more transitions than that width
/// allows; read 2 pixels wide (where each code of the first striped row is
/// a row of its own, so the rows after it are reached only after a
/// thousand), its codes run past the bytes any row of that width may take.
fn narrowed((width, rows): (u32, u32)) -> Vec<u8> {
    let bytes = with_tag(fixture("g4-stripes-4096x4.tif"), 256, 256, width);
    with_tag(with_tag(bytes, 257, 257, rows), 278, 278, rows)
}

const TOO_MANY_TRANSITIONS: (u32, u32) = (1_000, 16);
const TOO_LONG: (u32, u32) = (2, 2_048);

/// An uncompressed bilevel page of `width` x `height` bits that do not
/// repeat, and the values it stores.
pub(super) fn uncompressed(width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
    let row_bytes = width.div_ceil(8);
    let packed = noise(row_bytes * height);
    let values = packed
        .chunks(row_bytes)
        .flat_map(|row| (0..width).map(move |column| (row[column / 8] >> (7 - column % 8)) & 1))
        .collect();
    let page = TiffPage::strip((width as u32, height as u32), &[1], 1, packed);
    (files::tiff_file(false, &[page]).0, values)
}

/// Every honest bilevel fixture decodes to its stored bits, reads no more
/// than its own bytes and a buffer or two of tags, and holds its frame and
/// less than [`BESIDE_THE_FRAME`] beside it, from the smallest to a 4,096 x 3,072
/// Group 4 page and an 8,192 x 8,192 uncompressed one.
#[tokio::test]
async fn an_honest_bilevel_page_holds_its_frame_and_little_beside_it() {
    let mut cases: Vec<(String, Vec<u8>, Vec<u8>)> = small_fixtures()
        .into_iter()
        .map(|(name, white_is_zero)| {
            let image = card(37, 21);
            let stored = if white_is_zero {
                image.iter().map(|value| 1 - value).collect()
            } else {
                image
            };
            let bytes = fixture(&name);
            (name, bytes, stored)
        })
        .collect();
    cases.push((
        "g4-4096x3072.tif".into(),
        fixture("g4-4096x3072.tif"),
        card(4096, 3072),
    ));
    cases.push((
        "g4-stripes-4096x4.tif".into(),
        fixture("g4-stripes-4096x4.tif"),
        stripes(STRIPES),
    ));
    let (large, values) = uncompressed(8192, 8192);
    cases.push(("none-8192x8192.tif".into(), large, values));

    let listed: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(name, bytes, _)| (name.as_str(), bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for (name, bytes, stored) in &cases {
        let entry = scan.entry(name);
        let length = bytes.len() as u64;
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!(
                "{name}: {} reads, {} bytes, heap {} for a frame of {}",
                cost.reads,
                cost.bytes,
                cost.heap,
                stored.len()
            );
        }
        let frame = cost
            .result
            .as_ref()
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(&frame.bytes, stored, "{name}");
        assert_within_limits(name, entry, length, &cost);
        assert!(
            cost.heap <= stored.len() as u64 + BESIDE_THE_FRAME,
            "{name}: {} bytes of heap for a frame of {}",
            cost.heap,
            stored.len()
        );
        assert!(
            cost.bytes <= length + 2 * BUFFER,
            "{name}: {} bytes read of {length}",
            cost.bytes
        );
    }
}

/// Bilevel pages whose strips or tags were changed after libtiff wrote
/// them. Each is a decode error, or refused, holding no more than
/// [`BESIDE_THE_FRAME`]; `bounds::hostile_files` includes them, so the
/// read budget and the reservation are checked for them too.
pub(super) fn hostile_bilevel_files() -> Vec<Hostile> {
    let half = |name: &str, tag| {
        let bytes = fixture(name);
        let count = tag_values(&bytes, tag)[0];
        with_value(bytes, tag, 0, count / 2)
    };
    let g4 = fixture("g4-37x21.tif");
    let small = |case: Hostile| case.holding_at_most(BESIDE_THE_FRAME);
    let mut cases = Vec::new();
    for name in [
        "none-37x21.tif",
        "packbits-37x21.tif",
        "lzw-37x21.tif",
        "deflate-37x21.tif",
        "g4-37x21.tif",
        "g4-strips-37x21.tif",
    ] {
        cases.push(small(Hostile::new(
            "tiff bilevel strip cut short",
            half(name, 279),
            Outcome::Fails,
        )));
    }
    // libtiff only warns when a Group 4 strip reaches its end-of-block
    // code before its last row; a decode never pads the frame out.
    cases.push(small(Hostile::new(
        "tiff g4 strip that ends a row early",
        with_tag(with_tag(g4.clone(), 257, 257, 22), 278, 278, 22),
        Outcome::Fails,
    )));
    cases.push(small(Hostile::new(
        "tiff bilevel tile cut short",
        half("g4-tiles-37x21.tif", 325),
        Outcome::Fails,
    )));
    // A byte count past the end of the file costs nothing when the rows
    // are complete before it; a strip that begins too near the end is cut
    // short by it.
    cases.push(
        small(Hostile::new(
            "tiff bilevel byte count past the end of the file",
            with_value(g4.clone(), 279, 0, 16 * MIB as u32),
            Outcome::Decodes,
        ))
        .reading_at_most(g4.len() as u64),
    );
    let none = fixture("none-37x21.tif");
    cases.push(
        small(Hostile::new(
            "tiff bilevel strip that runs past the end of the file",
            with_value(none.clone(), 273, 0, none.len() as u32 - 50),
            Outcome::Fails,
        ))
        .reading_at_most(none.len() as u64),
    );
    cases.push(
        small(Hostile::new(
            "tiff bilevel strip of four gigabytes",
            with_value(g4.clone(), 279, 0, u32::MAX),
            Outcome::Fails,
        ))
        .reading_at_most(g4.len() as u64),
    );
    cases.push(small(Hostile::new(
        "tiff g4 row with more transitions than its width",
        narrowed(TOO_MANY_TRANSITIONS),
        Outcome::Fails,
    )));
    cases.push(small(Hostile::new(
        "tiff g4 row longer than its width allows",
        narrowed(TOO_LONG),
        Outcome::Fails,
    )));
    // `PlanarConfiguration` 1 is the default, so its entry can carry
    // another tag.
    cases.push(small(Hostile::new(
        "tiff bilevel page with a predictor",
        with_tag(fixture("lzw-37x21.tif"), 284, 317, 2),
        Outcome::Fails,
    )));
    cases.push(small(Hostile::new(
        "tiff bilevel page with fill order 3",
        with_tag(g4.clone(), 284, 266, 3),
        Outcome::Fails,
    )));
    cases.push(
        small(Hostile::new(
            "tiff bilevel page recompressed as group 3",
            g4.clone(),
            Outcome::Refused("raster.unsupported_compression"),
        ))
        .replaced_by(with_tag(g4, 259, 259, 3)),
    );
    cases
}

/// The two bounds on a Group 4 row, each reached by its own file: a row may
/// end with at most one transition more than its width, and may take at
/// most two bytes a pixel of its strip while it is decoded.
#[tokio::test]
async fn a_group4_row_is_bounded_by_its_width() {
    let cases = [
        (
            "transitions.tif",
            narrowed(TOO_MANY_TRANSITIONS),
            "a Group 4 line has more transitions than its width allows",
        ),
        (
            "long.tif",
            narrowed(TOO_LONG),
            "a Group 4 line is longer than its width allows",
        ),
    ];
    let listed: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(name, bytes, _)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for (name, bytes, reason) in &cases {
        let cost = decode(scan.entry(name), 0, CountedFile::new(bytes.clone()));
        let error = cost.result.expect_err(name);
        let chain = error.detail().unwrap_or_default();
        assert!(chain.contains(reason), "{name}: {chain}");
        assert!(cost.heap <= BESIDE_THE_FRAME, "{name}: heap {}", cost.heap);
    }
}
