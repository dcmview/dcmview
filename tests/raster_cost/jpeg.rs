//! What a JPEG-compressed TIFF page costs: its frame, the bytes of one strip
//! or tile, one decoded strip or tile and what the JPEG decoder holds while
//! it decodes one, and a hostile strip or tile refused before it costs more.
//!
//! The files are libtiff's own output (`raster_jpeg_files.rs`); a hostile
//! one is an honest one with a tag, a count or a frame header byte changed.

use super::bounds::{assert_within_limits, list, Hostile, Outcome, BUFFER, MIB};
use super::raster_bilevel_files::{tag_values, with_tag};
use super::raster_files::CountedFile;
use super::raster_jpeg_files::{
    fixture, frame_header_at, largest_difference, libtiff_decode, small_fixtures, with_count,
    with_frame_precision, with_frame_size,
};

/// What a JPEG page of these fixtures may hold beside its frame and the
/// bytes of its longest strip or tile, when its largest strip or tile
/// decodes to `chunk` bytes and is `width` pixels across: that strip or
/// tile decoded, the decoder's coefficients of it (two bytes a sample, held
/// whole for a progressive stream), the decoder's rows of it and 512 KiB
/// of tables and state. The rows were measured at at most 305 bytes a
/// pixel across, at 65,500 pixels with the luma sampled four times down.
fn beside_the_frame(chunk: u64, width: u64) -> u64 {
    3 * chunk + 384 * (width + 31) + 512 * 1024
}

/// Every honest fixture decodes, small and large (the small ones to what
/// libtiff decodes them to, give or take what the two decoders round
/// differently), reads no more than its own bytes and a buffer or two of
/// tags, and holds its frame, the bytes of its longest strip or tile and no
/// more than [`beside_the_frame`]: from 150 x 133 pages in strips and tiles
/// to progressive strips of 2,048 x 2,048 and two strips 65,500 pixels
/// wide.
#[tokio::test]
async fn an_honest_jpeg_page_holds_its_frame_and_one_strip_or_tile() {
    let mut cases: Vec<(String, Option<String>)> = small_fixtures()
        .into_iter()
        .map(|(name, _, decode)| (name, Some(decode)))
        .collect();
    for name in [
        "ycbcr-progressive-2048x2048.tif",
        "rgb-progressive-2048x2048.tif",
        "ycbcr-strips-65500x32.tif",
    ] {
        cases.push((name.to_string(), None));
    }
    let files: Vec<(&str, Vec<u8>)> = cases
        .iter()
        .map(|(name, _)| (name.as_str(), fixture(name)))
        .collect();
    let listed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (*name, bytes.as_slice()))
        .collect();
    let scan = list(&listed).await;
    for ((name, bytes), (_, decode)) in files.iter().zip(&cases) {
        let entry = scan.entry(name);
        let length = bytes.len() as u64;
        let cost = super::bounds::decode(entry, 0, CountedFile::new(bytes.clone()));
        let frame = cost
            .result
            .as_ref()
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_within_limits(name, entry, length, &cost);
        if let Some(decode) = decode {
            let difference = largest_difference(&frame.bytes, &libtiff_decode(decode));
            assert!(difference <= 4, "{name}: {difference} from libtiff's");
        }
        // The largest strip or tile, decoded, and how wide it is.
        let tiled = name.contains("tiles");
        let (width, height) = if tiled {
            (tag_values(bytes, 322)[0], tag_values(bytes, 323)[0])
        } else {
            (entry.columns, tag_values(bytes, 278)[0].min(entry.rows))
        };
        let chunk = u64::from(width) * u64::from(height) * u64::from(entry.samples_per_pixel);
        let longest = tag_values(bytes, if tiled { 325 } else { 279 })
            .into_iter()
            .max()
            .map_or(0, u64::from);
        let beside = cost.heap - frame.bytes.len() as u64 - longest;
        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!(
                "{name}: {} reads, {} bytes of {length}, heap {}: frame {}, \
                 longest strip or tile {longest}, {beside} beside \
                 ({:.2} decoded strips or tiles of {chunk})",
                cost.reads,
                cost.bytes,
                cost.heap,
                frame.bytes.len(),
                beside as f64 / chunk as f64
            );
        }
        let most = beside_the_frame(chunk, u64::from(width));
        assert!(
            beside <= most,
            "{name}: {beside} bytes beside the frame and the strip, at most {most}"
        );
        assert!(
            cost.bytes <= length + 2 * BUFFER,
            "{name}: {} bytes read of {length}",
            cost.bytes
        );
    }
}

/// JPEG pages whose tags or frame headers were changed after libtiff wrote
/// them. Each is a decode error, holding no more than its case allows;
/// `bounds::hostile_files` includes them, so the read budget and the
/// reservation are checked for them too.
pub(super) fn hostile_jpeg_files() -> Vec<Hostile> {
    let tiles = fixture("ycbcr-tiles-150x133.tif");
    let strips = fixture("gray-strips-150x133.tif");
    let small = |case: Hostile| case.holding_at_most(MIB);
    let mut cases = Vec::new();
    for (name, size) in [
        (
            "tiff jpeg frame header of 16,000 x 16,000 in a tile",
            16_000,
        ),
        (
            "tiff jpeg frame header of 65,535 x 65,535 in a tile",
            65_535,
        ),
    ] {
        cases.push(small(Hostile::new(
            name,
            with_frame_size(tiles.clone(), 324, size, size),
            Outcome::Fails,
        )));
    }
    cases.push(small(Hostile::new(
        "tiff jpeg frame header smaller than its tile",
        with_frame_size(tiles.clone(), 324, 64, 64),
        Outcome::Fails,
    )));
    cases.push(small(Hostile::new(
        "tiff jpeg frame header taller than its strip",
        with_frame_size(strips.clone(), 273, 150, 17),
        Outcome::Fails,
    )));
    // An RGB page relabelled gray: its strips hold three components.
    let rgb = fixture("rgb-strips-150x133.tif");
    let relabelled = with_tag(
        with_tag(with_tag(rgb, 258, 258, 8), 277, 277, 1),
        262,
        262,
        1,
    );
    cases.push(small(Hostile::new(
        "tiff jpeg strip of three components on a gray page",
        relabelled,
        Outcome::Fails,
    )));
    cases.push(small(Hostile::new(
        "tiff jpeg strip of 12-bit samples",
        with_frame_precision(strips.clone(), 273, 12),
        Outcome::Fails,
    )));
    // The chroma of a YCbCr tile sampled as often as its luma.
    let mut chroma = tiles.clone();
    let at = frame_header_at(&chroma, 324);
    chroma[at + 14] = 0x22;
    cases.push(small(Hostile::new(
        "tiff jpeg tile whose chroma is sampled twice",
        chroma,
        Outcome::Fails,
    )));
    // The widest strip libjpeg writes, its luma read as sampled once across
    // and four times down: of the factors a YCbCr page may have, the one
    // whose rows hold the most while the strip is decoded. What a damaged
    // stream decodes to is the decoder's decision; it is held to the
    // honest bound all the same.
    let mut sampled = fixture("ycbcr-strips-65500x32.tif");
    let at = frame_header_at(&sampled, 273);
    sampled[at + 11] = 0x14;
    let (chunk, width) = (65_500 * 16 * 3, 65_500);
    cases.push(
        Hostile::new(
            "tiff jpeg strip whose luma is sampled four times down",
            sampled,
            Outcome::Decodes,
        )
        .holding_at_most(65_500 * 32 * 3 + 70_703 + beside_the_frame(chunk, width)),
    );
    for (name, count) in [
        ("tiff jpeg tables one byte over the limit", 64 * 1024 + 1),
        ("tiff jpeg tables of four gigabytes", u32::MAX),
    ] {
        cases.push(
            small(Hostile::new(
                name,
                with_count(strips.clone(), 347, count),
                Outcome::Fails,
            ))
            .reading_at_most(2 * BUFFER),
        );
    }
    // One tile of 4,096 x 4,096 for the whole image: as RGB it decodes to
    // more than 16 MiB and is refused before it is read; as gray it decodes
    // to exactly 16 MiB, which is allowed, and its frame header, of the
    // 128 x 128 tile libtiff wrote, is then not its size.
    let one_tile = |bytes: Vec<u8>| {
        let offset = tag_values(&bytes, 324)[0];
        let count = tag_values(&bytes, 325)[0];
        let bytes = with_tag(with_tag(bytes, 322, 322, 4096), 323, 323, 4096);
        with_tag(with_tag(bytes, 324, 324, offset), 325, 325, count)
    };
    cases.push(small(Hostile::new(
        "tiff jpeg tile that decodes to more than 16 MiB",
        one_tile(tiles.clone()),
        Outcome::Fails,
    )));
    cases.push(
        Hostile::new(
            "tiff jpeg tile that decodes to 16 MiB",
            one_tile(fixture("gray-tiles-150x133.tif")),
            Outcome::Fails,
        )
        .holding_at_most(17 * MIB),
    );
    // A JPEG page where discovery listed an uncompressed one.
    let gray = fixture("gray-150x133.tif");
    cases.push(small(
        Hostile::new(
            "tiff jpeg page where none was listed",
            with_tag(gray.clone(), 259, 259, 1),
            Outcome::Fails,
        )
        .replaced_by(gray),
    ));
    cases
}

/// Each hostile JPEG page is refused by the check meant for it, before the
/// decoder is given its strip or tile, and holds less than 1 MiB while it
/// is.
#[tokio::test]
async fn a_hostile_jpeg_page_is_refused_by_its_own_check() {
    let reasons = [
        (
            "tiff jpeg frame header of 16,000 x 16,000 in a tile",
            "declares 16000 x 16000, not the size of its tile",
        ),
        (
            "tiff jpeg frame header of 65,535 x 65,535 in a tile",
            "declares 65535 x 65535, not the size of its tile",
        ),
        (
            "tiff jpeg frame header smaller than its tile",
            "declares 64 x 64, not the size of its tile",
        ),
        (
            "tiff jpeg frame header taller than its strip",
            "declares 150 x 17, not the size of its strip",
        ),
        (
            "tiff jpeg strip of three components on a gray page",
            "declares 3 components for 1 samples",
        ),
        (
            "tiff jpeg strip of 12-bit samples",
            "declares 12-bit samples",
        ),
        (
            "tiff jpeg tile whose chroma is sampled twice",
            "sampled other than its page allows",
        ),
        (
            "tiff jpeg tables one byte over the limit",
            "TIFF JPEG tables exceed 65536 bytes",
        ),
        (
            "tiff jpeg tables of four gigabytes",
            "TIFF JPEG tables exceed 65536 bytes",
        ),
        (
            "tiff jpeg tile that decodes to more than 16 MiB",
            "decodes to more than 16777216 bytes",
        ),
        (
            "tiff jpeg page where none was listed",
            "TIFF page differs from catalog",
        ),
    ];
    let cases: Vec<Hostile> = hostile_jpeg_files();
    let cases: Vec<_> = cases
        .iter()
        .filter_map(Hostile::on_disk)
        .filter(|case| reasons.iter().any(|(name, _)| *name == case.name))
        .collect();
    assert_eq!(cases.len(), reasons.len());
    let files: Vec<(String, &[u8])> = cases
        .iter()
        .enumerate()
        .map(|(index, case)| (format!("{index}.tif"), case.listed))
        .collect();
    let listed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), *bytes))
        .collect();
    let scan = list(&listed).await;
    for ((file, _), case) in files.iter().zip(&cases) {
        let reason = reasons
            .iter()
            .find_map(|(name, reason)| (*name == case.name).then_some(*reason))
            .expect("a reason");
        let presented = CountedFile::new(case.replaced.unwrap_or(case.listed).to_vec());
        let cost = super::bounds::decode(scan.entry(file), 0, presented);
        let error = cost.result.expect_err(case.name);
        let chain = error.detail().unwrap_or_default();
        assert!(chain.contains(reason), "{}: {chain}", case.name);
        assert!(cost.heap <= MIB, "{}: heap {}", case.name, cost.heap);
    }
}

/// A page whose one strip is declared taller than the image, within the
/// tile margin, holds no more than the rows the image has when its strip
/// declares those: here a 2,048 x 2,048 RGB strip under a `RowsPerStrip`
/// of 2,848, which would decode to more than 16 MiB at that height.
#[tokio::test]
async fn a_strip_declared_taller_than_the_image_decodes_its_rows() {
    let honest = fixture("rgb-progressive-2048x2048.tif");
    let taller = with_tag(honest.clone(), 278, 278, 2_848);
    let scan = list(&[("honest.tif", &honest), ("taller.tif", &taller)]).await;
    let decoded = |name: &str, bytes: &[u8]| {
        let cost = super::bounds::decode(scan.entry(name), 0, CountedFile::new(bytes.to_vec()));
        assert_within_limits(name, scan.entry(name), bytes.len() as u64, &cost);
        cost.result.unwrap_or_else(|error| panic!("{name}: {error:?}"))
    };
    assert_eq!(
        decoded("taller.tif", &taller).bytes,
        decoded("honest.tif", &honest).bytes
    );
}
