//! Where a TIFF page's strips and tiles lie in the file, and what reading
//! them costs (`pixels::decode_raster_frame`, "Bytes read" and "Reads").
//!
//! A writer may store the strips of a page in any order, and may point
//! several tiles at the same bytes. Reading each once costs its own bytes;
//! bytes that are used again are charged again.

use super::bounds::BUFFER;
use super::bounds::{assert_hostile, assert_within_limits, decode, list, noise, Hostile, Outcome};
use super::raster_files::{self as files, CountedFile, TiffPage, TiffValue};

/// What a decode of a page of `chunks` strips or tiles may read beside
/// them: the file's header and the page's IFD with the buffer read ahead of
/// each, and the offsets and byte counts of the chunks, once for the checks
/// made before the decoder and once by the decoder.
fn page_overhead(chunks: u64) -> u64 {
    6 * BUFFER + 2 * 2 * 4 * chunks
}

/// PackBits of `data`, as literal runs.
fn packbits(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for run in data.chunks(128) {
        out.push(run.len() as u8 - 1);
        out.extend_from_slice(run);
    }
    out
}

/// The tiles of `image` (`width` pixels a row, one byte a pixel), row by
/// row. The image's sides are multiples of the tile's.
fn tiles_of(image: &[u8], width: usize, (tile_w, tile_h): (usize, usize)) -> Vec<Vec<u8>> {
    let height = image.len() / width;
    let mut tiles = Vec::new();
    for top in (0..height).step_by(tile_h) {
        for left in (0..width).step_by(tile_w) {
            let mut tile = Vec::with_capacity(tile_w * tile_h);
            for row in top..top + tile_h {
                tile.extend_from_slice(&image[row * width + left..row * width + left + tile_w]);
            }
            tiles.push(tile);
        }
    }
    tiles
}

fn gray_page(
    (width, height): (u32, u32),
    compression: u16,
    layout: &[(u16, u32)],
    chunks: Vec<Vec<u8>>,
) -> TiffPage {
    let mut tags = vec![
        (256, TiffValue::Long(vec![width])),
        (257, TiffValue::Long(vec![height])),
        (258, TiffValue::Short(vec![8])),
        (259, TiffValue::Short(vec![compression])),
        (262, TiffValue::Short(vec![1])),
        (277, TiffValue::Short(vec![1])),
    ];
    tags.extend(
        layout
            .iter()
            .map(|&(tag, value)| (tag, TiffValue::Long(vec![value]))),
    );
    TiffPage {
        tags,
        chunks,
        tiled: layout.iter().any(|&(tag, _)| tag == 322),
    }
}

/// Reading every strip or tile of a page once costs the bytes of the file
/// and a fixed overhead, whatever order they are stored in: last row first,
/// every other one, or shuffled.
#[tokio::test]
async fn strips_and_tiles_cost_their_own_bytes_in_whatever_order_they_are_stored() {
    // 2,048 rows of 2,048 pixels, one row to a strip: four megabytes.
    let rows: Vec<u8> = (0..2048 * 2048)
        .map(|index| ((index / 2048) * 7 + index % 2048) as u8)
        .collect();
    let square = noise(512 * 512);
    let small: Vec<u8> = (0..256 * 256).map(|index| (index % 253) as u8).collect();
    // (name, page with its chunks in decode order, the samples it holds)
    let pages: Vec<(&str, TiffPage, &[u8])> = vec![
        (
            "one-row strips",
            gray_page(
                (2048, 2048),
                1,
                &[(278, 1)],
                rows.chunks(2048).map(<[u8]>::to_vec).collect(),
            ),
            &rows,
        ),
        (
            "deflated strips",
            gray_page(
                (512, 512),
                8,
                &[(278, 8)],
                square.chunks(512 * 8).map(files::zlib).collect(),
            ),
            &square,
        ),
        (
            "packed strips",
            gray_page(
                (256, 256),
                32773,
                &[(278, 4)],
                small.chunks(256 * 4).map(packbits).collect(),
            ),
            &small,
        ),
        (
            "deflated tiles",
            gray_page(
                (512, 512),
                8,
                &[(322, 64), (323, 64)],
                tiles_of(&square, 512, (64, 64))
                    .iter()
                    .map(|tile| files::zlib(tile))
                    .collect(),
            ),
            &square,
        ),
        (
            "plain tiles",
            gray_page(
                (256, 256),
                1,
                &[(322, 16), (323, 16)],
                tiles_of(&small, 256, (16, 16)),
            ),
            &small,
        ),
    ];
    type Order = fn(usize) -> Vec<usize>;
    let orders: [(&str, Order); 3] = [
        ("last first", |count| (0..count).rev().collect()),
        ("every other one", |count| {
            (0..count).step_by(2).chain((1..count).step_by(2)).collect()
        }),
        ("shuffled", |count| {
            let mut order: Vec<usize> = (0..count).collect();
            let mut state = 0x2545_f491_4f6c_dd1d_u64;
            for index in (1..count).rev() {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                order.swap(index, (state >> 33) as usize % (index + 1));
            }
            order
        }),
    ];

    let mut cases = Vec::new();
    for (name, page, samples) in &pages {
        for (order, stored) in &orders {
            let file = files::tiff_stored_in(page.clone(), &stored(page.chunks.len()));
            cases.push((
                format!("{name}, {order}"),
                file,
                page.chunks.len() as u64,
                *samples,
            ));
        }
    }
    let names: Vec<String> = (0..cases.len())
        .map(|index| format!("{index}.tif"))
        .collect();
    let listed: Vec<(&str, &[u8])> = names
        .iter()
        .zip(&cases)
        .map(|(file, case)| (file.as_str(), case.1.as_slice()))
        .collect();
    let scan = list(&listed).await;

    for (file, (name, bytes, chunks, samples)) in names.iter().zip(&cases) {
        let entry = scan.entry(file);
        let length = bytes.len() as u64;
        let cost = decode(entry, 0, CountedFile::new(bytes.clone()));
        assert_within_limits(name, entry, length, &cost);
        let frame = cost
            .result
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(frame.bytes == *samples, "{name}: the stored samples");
        assert!(
            cost.bytes <= length + page_overhead(*chunks),
            "{name}: {} bytes read of a file of {length}",
            cost.bytes
        );
        // One read for each chunk that does not follow the one before it,
        // and one for each further buffer of a chunk.
        assert!(
            cost.reads <= chunks + length / BUFFER + 8,
            "{name}: {} reads for {chunks} chunks in {length} bytes",
            cost.reads
        );
    }
}

/// A deflate stream of `padding` bytes that yields `row`: blocks of no
/// bytes, then one that stores the row.
fn padded_stream(row: &[u8], padding: usize) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    while out.len() + 5 <= padding {
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0xff, 0xff]);
    }
    out.push(0x01);
    out.extend_from_slice(&(row.len() as u16).to_le_bytes());
    out.extend_from_slice(&(!(row.len() as u16)).to_le_bytes());
    out.extend_from_slice(row);
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in row {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

/// Tiles that share their bytes decode, and cost the file once. Strips that
/// point at the same bytes tens of thousands of times do not buy that many
/// passes over them: every byte a decoder is handed is charged to the read
/// budget, whether the file was read for it again or not.
#[tokio::test]
async fn bytes_used_again_are_charged_again() {
    // 256 tiles of 16 x 16 that are all the one stored tile.
    let tile: Vec<u8> = (0..=255).collect();
    let mut shared = gray_page((256, 256), 1, &[(322, 16), (323, 16)], vec![tile.clone()]);
    shared.tags.push((324, TiffValue::Long(vec![8; 256])));
    shared.tags.push((325, TiffValue::Long(vec![256; 256])));
    let shared = files::tiff_file(false, &[shared]).0;
    let scan = list(&[("shared.tif", &shared)]).await;
    let entry = scan.entry("shared.tif");
    let length = shared.len() as u64;
    let cost = decode(entry, 0, CountedFile::new(shared));
    assert_within_limits("shared tiles", entry, length, &cost);
    let frame = cost.result.expect("shared tiles decode");
    for (index, &sample) in frame.bytes.iter().enumerate() {
        let (row, column) = (index / 256, index % 256);
        assert_eq!(sample, tile[row % 16 * 16 + column % 16], "pixel {index}");
    }
    assert!(cost.bytes <= length + page_overhead(256));

    // 65,536 rows of eight pixels, one row to a strip, each strip a deflate
    // stream that is mostly padding. Stored once each they would be far
    // more than the page's budget; so they are when they share bytes.
    let strips = |streams: &[Vec<u8>]| {
        let mut page = gray_page((8, 65_536), 8, &[(278, 1)], streams.to_vec());
        let mut offsets = Vec::new();
        let mut at = 8;
        for stream in streams {
            offsets.push(at);
            at += stream.len() as u32;
        }
        let pick = |values: &[u32]| (0..65_536).map(|row| values[row % values.len()]).collect();
        let counts: Vec<u32> = streams.iter().map(|stream| stream.len() as u32).collect();
        page.tags.push((273, TiffValue::Long(pick(&offsets))));
        page.tags.push((279, TiffValue::Long(pick(&counts))));
        files::tiff_file(false, &[page]).0
    };
    let row = [1, 2, 3, 4, 5, 6, 7, 8];
    // Every strip is the same 60 KiB, which stay in the read buffer: no
    // strip but the first needs a read. (65,536 times 60 KiB is 3.7 GiB; the
    // page's budget is 66 MiB.)
    let one = strips(&[padded_stream(&row, 60 * 1024)]);
    // Two streams of 4 KiB by turns, which each strip reads again: 256 MiB.
    let two = strips(&[padded_stream(&row, 4096), padded_stream(&row, 4096)]);
    let most = one.len() as u64 + page_overhead(65_536);
    assert_hostile(vec![
        Hostile::new("tiff strips that all share one stream", one, Outcome::Fails)
            .reading_at_most(most),
        Hostile::new("tiff strips that share two streams", two, Outcome::Fails),
    ])
    .await;
}
