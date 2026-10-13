//! What pixel digests cost and hold (`pixels::PixelDigests`,
//! `pixels::frame_pixel_digest`), counted at the allocator: the table of a
//! session however many frames are recorded in it, and the heap hashing one
//! frame takes beside the frame.

use super::admission::Native;
use super::bounds::{list, Listed};
use super::heap::{self, CountedRuntime};
use bytes::Bytes;
use dcmview::api::contracts::RawFrameMetadata;
use dcmview::pixels::{
    frame_pixel_digest, record_frame_digest, PixelDigestStats, PixelDigests,
    PIXEL_DIGESTS_MAX_BYTES, PIXEL_DIGEST_BUFFER_BYTES, PIXEL_DIGEST_CAPACITY,
    PIXEL_DIGEST_FRAMES_STATED,
};
use dcmview::types::FileEntry;
use std::sync::Arc;

/// What a call may allocate that is not a buffer: nothing a frame's size
/// could hide in.
const SLACK: u64 = 4 * 1024;

fn digest_of(n: usize) -> [u8; 32] {
    let mut digest = [0_u8; 32];
    digest[..8].copy_from_slice(&(n as u64).to_le_bytes());
    digest
}

/// A table nothing was recorded in holds no heap, so a session that
/// decodes nothing pays nothing for it: not at startup, not for each file
/// it lists.
#[test]
fn a_table_nothing_was_recorded_in_holds_no_heap() {
    let (table, peak) = heap::peak_during(PixelDigests::new);
    assert_eq!(peak, 0);
    // A lock may allocate itself the first time it is taken.
    assert_eq!(table.stats(), PixelDigestStats::default());
    let before = heap::held();
    assert_eq!(table.get(0, 0), None);
    assert_eq!(
        table.of_frames(&[0, 1, 2], 100_000),
        Vec::<Option<String>>::new()
    );
    assert_eq!(heap::held(), before);
}

/// Three times its capacity is recorded, over more files than a session
/// lists and more frames than a file has: the table holds its capacity and
/// no more than its stated heap, and forgets in the order it was told.
#[test]
fn the_table_holds_its_capacity_and_its_stated_heap_whatever_is_recorded() {
    let before = heap::held();
    let table = PixelDigests::new();
    let total = PIXEL_DIGEST_CAPACITY * 3;
    // More files than a session lists, each with a frame far into it.
    let place = |n: usize| (n % 150_000 + 1_000_000, (n / 150_000) as u32 * 60_000 + 5);
    let (_, peak) = heap::peak_during(|| {
        for n in 0..total {
            let (file, frame) = place(n);
            table.record(file, frame, digest_of(n), 56);
        }
    });
    assert!(
        peak <= PIXEL_DIGESTS_MAX_BYTES as u64,
        "recording {total} frames held {peak} bytes at once"
    );
    assert!(heap::held() - before <= PIXEL_DIGESTS_MAX_BYTES as i64);
    assert_eq!(
        table.stats(),
        PixelDigestStats {
            frames_hashed: total as u64,
            bytes_hashed: total as u64 * 56,
            held: PIXEL_DIGEST_CAPACITY,
        }
    );
    // The last `PIXEL_DIGEST_CAPACITY` recorded are held, the one before
    // them is not.
    let held = |n: usize| {
        let (file, frame) = place(n);
        table.get(file, frame)
    };
    let oldest = total - PIXEL_DIGEST_CAPACITY;
    assert_eq!(held(oldest - 1), None);
    assert_eq!(held(oldest), Some(digest_of(oldest)));
    assert_eq!(held(total - 1), Some(digest_of(total - 1)));

    // A frame the table holds is replaced where it stands: nothing is
    // forgotten for it, and it is still the first to go.
    let (file, frame) = place(oldest);
    table.record(file, frame, [0xee; 32], 56);
    assert_eq!(held(oldest), Some([0xee; 32]));
    assert_eq!(held(oldest + 1), Some(digest_of(oldest + 1)));
    assert_eq!(table.stats().held, PIXEL_DIGEST_CAPACITY);
    table.record(1, 1, [0x11; 32], 56);
    assert_eq!(held(oldest), None);
    assert_eq!(held(oldest + 1), Some(digest_of(oldest + 1)));
    assert_eq!(table.get(1, 1), Some([0x11; 32]));
    assert_eq!(table.stats().held, PIXEL_DIGEST_CAPACITY);

    // What a document states of one file is as long as the file, never
    // as long as the table, and never longer than a document may hold.
    let (file, frame) = place(total - 1);
    assert_eq!(frame, 60_005);
    {
        let stated = table.of_frames(&[file], frame + 1);
        assert_eq!(stated.len() as u32, frame + 1);
        assert_eq!(stated.iter().flatten().count(), 1);
        assert!(stated[frame as usize].is_some());
    }
    assert!(table.of_frames(&[file], frame).is_empty());
    assert_eq!(
        table
            .of_frames(&[file], PIXEL_DIGEST_FRAMES_STATED as u32)
            .len(),
        PIXEL_DIGEST_FRAMES_STATED
    );
    let (_, peak) = heap::peak_during(|| {
        assert!(table
            .of_frames(&[file], PIXEL_DIGEST_FRAMES_STATED as u32 + 1)
            .is_empty());
        assert!(table.of_frames(&[file], u32::MAX).is_empty());
    });
    assert_eq!(peak, 0);

    drop(table);
    assert_eq!(heap::held(), before);
}

/// One frame decoded over and over is one frame held: neither the table
/// nor the order it forgets in grows with the number of times a digest is
/// recorded.
#[test]
fn recording_one_frame_again_and_again_holds_one_frame() {
    let table = PixelDigests::new();
    // A full table, so that a growing order would also show as frames
    // forgotten that should not be.
    for n in 0..PIXEL_DIGEST_CAPACITY {
        table.record(n / 100, (n % 100) as u32, digest_of(n), 56);
    }
    let before = heap::held();
    let (_, peak) = heap::peak_during(|| {
        for n in 0..1_000_000 {
            table.record(3, 7, digest_of(n), 56);
        }
    });
    assert_eq!(peak, 0, "recording a held frame allocated");
    assert_eq!(heap::held(), before);
    assert_eq!(table.get(3, 7), Some(digest_of(999_999)));
    assert_eq!(
        table.stats(),
        PixelDigestStats {
            frames_hashed: PIXEL_DIGEST_CAPACITY as u64 + 1_000_000,
            bytes_hashed: (PIXEL_DIGEST_CAPACITY as u64 + 1_000_000) * 56,
            held: PIXEL_DIGEST_CAPACITY,
        }
    );
    // Every frame recorded before is still held, and the next new frame
    // forgets exactly the oldest.
    assert_eq!(table.get(0, 0), Some(digest_of(0)));
    table.record(usize::MAX, 0, [1; 32], 56);
    assert_eq!(table.get(0, 0), None);
    assert_eq!(table.get(0, 1), Some(digest_of(1)));
    assert_eq!(table.get(3, 7), Some(digest_of(999_999)));
    assert_eq!(table.stats().held, PIXEL_DIGEST_CAPACITY);
}

/// A decoder that hands over cells keeps the stored field where High Bit
/// puts it; one that hands over values has moved it down. Either way the
/// digest is of the values, and a sample of 64 bits has one like any other.
#[test]
fn the_stored_field_is_hashed_from_wherever_a_decoder_leaves_it() {
    let scan = listed(16);
    let native = {
        let mut entry = scan.entry("image.dcm").clone();
        entry.bits_allocated = 16;
        entry.series_metadata.native_pixel.bits_stored = Some(12);
        entry.series_metadata.native_pixel.high_bit = Some(15);
        entry
    };
    let mut coded = native.clone();
    coded.transfer_syntax_uid = "1.2.840.10008.1.2.4.80".to_string();
    let gray16 = metadata(16, 16, 1, "MONOCHROME2");
    let values: Vec<u16> = (0..256_u32)
        .map(|i| ((i * 37 + 11) & 0x0fff) as u16)
        .collect();
    let bytes =
        |cells: &[u16]| -> Vec<u8> { cells.iter().flat_map(|cell| cell.to_le_bytes()).collect() };
    // The uncompressed decode returns the values; the JPEG-LS decoder the
    // cells, the field in bits 4 to 15 over four bits of something else.
    let cells: Vec<u16> = values
        .iter()
        .map(|value| value << 4 | (value & 0xf) ^ 0x9)
        .collect();
    let of_values = frame_pixel_digest(&native, &bytes(&values), &gray16).expect("digest");
    assert_eq!(
        frame_pixel_digest(&coded, &bytes(&cells), &gray16),
        Some(of_values)
    );
    // Signed: the sign is carried up from the field's highest bit.
    let mut signed = gray16.clone();
    signed.pixel_representation = 1;
    let extended: Vec<u16> = values
        .iter()
        .map(|value| {
            if value & 0x800 != 0 {
                value | 0xf000
            } else {
                *value
            }
        })
        .collect();
    assert_eq!(
        frame_pixel_digest(&coded, &bytes(&cells), &signed),
        frame_pixel_digest(&native, &bytes(&extended), &signed)
    );
    assert_ne!(
        frame_pixel_digest(&coded, &bytes(&cells), &signed),
        Some(of_values)
    );

    // 64-bit samples, every bit stored.
    let mut wide = scan.entry("image.dcm").clone();
    wide.bits_allocated = 64;
    wide.series_metadata.native_pixel.bits_stored = Some(64);
    wide.series_metadata.native_pixel.high_bit = Some(63);
    let body: Vec<u8> = (0..256 * 8).map(|i| (i * 7 + i / 255) as u8).collect();
    let (_, hashed) = frame_pixel_digest(&wide, &body, &metadata(16, 64, 1, "MONOCHROME2"))
        .expect("a digest of 64-bit samples");
    assert_eq!(hashed, 24 + 256 * 8);
}

/// An uncompressed 8-bit DICOM image of `side` rows and columns, as the
/// production loader lists it under `image.dcm`.
fn listed(side: u16) -> Listed {
    let bytes = Native::gray(side, 8).file(vec![0; usize::from(side) * usize::from(side)]);
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(list(&[("image.dcm", &bytes)]))
}

fn metadata(
    side: u32,
    bits_allocated: u32,
    samples_per_pixel: u32,
    photometric: &str,
) -> RawFrameMetadata {
    RawFrameMetadata {
        rows: side,
        columns: side,
        bits_allocated,
        pixel_representation: 0,
        samples_per_pixel,
        photometric_interpretation: photometric.to_string(),
        rescale_slope: 1.0,
        rescale_intercept: 0.0,
        default_wc: None,
        default_ww: None,
        padding_low: None,
        padding_high: None,
    }
}

/// Hashing a frame copies none of it: samples that are already in order
/// are hashed where they lie, and samples stored colour-by-plane pass
/// through one buffer of a fixed size, however large the frame.
#[test]
fn hashing_a_frame_takes_a_fixed_buffer_at_most() {
    const SIDE: u16 = 1024;
    let side = u32::from(SIDE);
    let pixels = usize::from(SIDE) * usize::from(SIDE);
    let scan = listed(SIDE);
    let gray = scan.entry("image.dcm").clone();
    let mut gray16 = gray.clone();
    gray16.bits_allocated = 16;
    gray16.series_metadata.native_pixel.bits_stored = Some(16);
    gray16.series_metadata.native_pixel.high_bit = Some(15);
    // Twelve bits of each sample stored: the others are taken out on the
    // way to the hash.
    let mut gray12 = gray16.clone();
    gray12.series_metadata.native_pixel.bits_stored = Some(12);
    gray12.series_metadata.native_pixel.high_bit = Some(11);
    // The same entry as a colour image stored by plane.
    let mut planar = gray.clone();
    planar.samples_per_pixel = 3;
    planar.photometric_interpretation = "RGB".to_string();
    planar.series_metadata.native_pixel.planar_configuration = Some(1);
    let mut interleaved = planar.clone();
    interleaved
        .series_metadata
        .native_pixel
        .planar_configuration = Some(0);

    let cases: [(&str, &FileEntry, RawFrameMetadata, usize, u64); 5] = [
        (
            "one byte a pixel",
            &gray,
            metadata(side, 8, 1, "MONOCHROME2"),
            pixels,
            SLACK,
        ),
        (
            "two bytes a pixel",
            &gray16,
            metadata(side, 16, 1, "MONOCHROME2"),
            pixels * 2,
            SLACK,
        ),
        (
            "twelve bits in two bytes",
            &gray12,
            metadata(side, 16, 1, "MONOCHROME2"),
            pixels * 2,
            PIXEL_DIGEST_BUFFER_BYTES as u64 + SLACK,
        ),
        (
            "colour by pixel",
            &interleaved,
            metadata(side, 8, 3, "RGB"),
            pixels * 3,
            SLACK,
        ),
        (
            "colour by plane",
            &planar,
            metadata(side, 8, 3, "RGB"),
            pixels * 3,
            PIXEL_DIGEST_BUFFER_BYTES as u64 + SLACK,
        ),
    ];
    for (name, entry, metadata, length, allowed) in cases {
        let body: Vec<u8> = (0..length).map(|i| (i * 31 + i / 977) as u8).collect();
        let (digest, peak) = heap::peak_during(|| frame_pixel_digest(entry, &body, &metadata));
        let (_, hashed) = digest.unwrap_or_else(|| panic!("{name}: no digest"));
        assert_eq!(hashed, 24 + length as u64, "{name}");
        assert!(
            peak <= allowed,
            "{name}: hashing {length} bytes held {peak} more, {allowed} allowed"
        );
    }

    // The two colour layouts of the same picture are one digest.
    let colour: Vec<u8> = (0..pixels * 3).map(|i| (i * 7 + i / 1013) as u8).collect();
    let by_plane: Vec<u8> = (0..3)
        .flat_map(|channel| colour.iter().skip(channel).step_by(3).copied())
        .collect();
    let rgb = metadata(side, 8, 3, "RGB");
    assert_eq!(
        frame_pixel_digest(&planar, &by_plane, &rgb),
        frame_pixel_digest(&interleaved, &colour, &rgb)
    );
    // A body that is not the frame has none, and neither panics.
    assert_eq!(
        frame_pixel_digest(&gray, &colour, &metadata(side, 8, 1, "MONOCHROME2")),
        None
    );
    assert_eq!(
        frame_pixel_digest(&gray, &[], &metadata(0, 8, 1, "MONOCHROME2")),
        None
    );
    assert_eq!(
        frame_pixel_digest(&gray, &colour, &metadata(side, 12, 1, "MONOCHROME2")),
        None
    );
}

/// Recording the digest of a frame a decode produced shares the frame's
/// samples with the thread that hashes them: nothing of the frame is
/// copied on the way, whoever calls and however large the frame.
#[test]
fn recording_a_digest_copies_none_of_the_frame() {
    const SIDE: u16 = 1024;
    let scan = listed(SIDE);
    let entry = Arc::new(scan.entry("image.dcm").clone());
    let frame = usize::from(SIDE) * usize::from(SIDE);
    let body = Bytes::from(
        (0..frame)
            .map(|i| (i * 13 + i / 911) as u8)
            .collect::<Vec<u8>>(),
    );
    let metadata = metadata(u32::from(SIDE), 8, 1, "MONOCHROME2");
    let table = Arc::new(PixelDigests::new());
    let runtime = CountedRuntime::new();
    let record = |frame: u32| record_frame_digest(&table, &entry, frame, &body, &metadata);
    // Once before it is measured, so the measurement does not pay for the
    // first use of a thread or of the table.
    runtime.peak_during(record(0));
    let ((), peak) = runtime.peak_during(record(0));
    assert!(
        peak <= PIXEL_DIGEST_BUFFER_BYTES as u64 + 64 * 1024,
        "recording the digest of a frame of {frame} bytes held {peak} more"
    );
    let expected = frame_pixel_digest(&entry, &body, &metadata).map(|(digest, _)| digest);
    assert!(expected.is_some());
    assert_eq!(table.get(entry.index, 0), expected);
    // Each call counts, and the frame is held once.
    assert_eq!(
        table.stats(),
        PixelDigestStats {
            frames_hashed: 2,
            bytes_hashed: 2 * (24 + frame as u64),
            held: 1,
        }
    );

    // A frame without a digest is not read and uses no thread: the same
    // entry as a baseline JPEG frame.
    let mut lossy = (*entry).clone();
    lossy.transfer_syntax_uid = "1.2.840.10008.1.2.4.50".to_string();
    let lossy = Arc::new(lossy);
    let ((), peak) = runtime.peak_during(record_frame_digest(&table, &lossy, 1, &body, &metadata));
    assert_eq!(peak, 0);
    assert_eq!(table.get(lossy.index, 1), None);
    assert_eq!(table.stats().frames_hashed, 2);
}
