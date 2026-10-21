//! The layout a pixel digest is computed over (`dcmview_annotation::pixel`).
//!
//! The expected bytes are written out by hand from the definition, so an
//! implementation in another language can be held to the same lines.

use dcmview_annotation::{pixel_digest_text, PixelLayout, SampleFormat, PIXEL_DIGEST_HEADER_BYTES};

fn layout(
    rows: u32,
    columns: u32,
    samples_per_pixel: u32,
    bits_allocated: u32,
    sample_format: SampleFormat,
) -> PixelLayout {
    PixelLayout {
        rows,
        columns,
        samples_per_pixel,
        bits_allocated,
        sample_format,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Six little-endian `u32`s: rows, columns, samples per pixel, bits
/// allocated, the sample format and a planar configuration that is always
/// 0.
#[test]
fn the_header_is_six_little_endian_numbers() {
    assert_eq!(PIXEL_DIGEST_HEADER_BYTES, 24);
    let cases = [
        (
            layout(2, 3, 1, 8, SampleFormat::Unsigned),
            "020000000300000001000000080000000000000000000000",
        ),
        (
            layout(4, 4, 1, 16, SampleFormat::Signed),
            "040000000400000001000000100000000100000000000000",
        ),
        (
            layout(16, 16, 3, 8, SampleFormat::Unsigned),
            "100000001000000003000000080000000000000000000000",
        ),
        (
            layout(1, 2, 1, 32, SampleFormat::Float),
            "010000000200000001000000200000000200000000000000",
        ),
        // Rows and columns are not the same number, and a large one keeps
        // all four of its bytes.
        (
            layout(0x0102_0304, 0x0a0b_0c0d, 4, 64, SampleFormat::Float),
            "040302010d0c0b0a04000000400000000200000000000000",
        ),
    ];
    for (layout, expected) in cases {
        assert_eq!(hex(&layout.header()), expected, "{layout:?}");
    }
}

#[test]
fn a_frame_has_as_many_sample_bytes_as_its_layout_says_or_no_digest() {
    let unsigned = SampleFormat::Unsigned;
    let cases = [
        (layout(2, 3, 1, 8, unsigned), Some(6)),
        (layout(4, 4, 1, 16, unsigned), Some(32)),
        (layout(16, 16, 3, 8, unsigned), Some(768)),
        (layout(1, 2, 1, 32, SampleFormat::Float), Some(8)),
        (layout(3, 5, 4, 64, SampleFormat::Float), Some(480)),
        // 2^20 by 2^20 pixels of four 64-bit samples still fits.
        (layout(1_048_576, 1_048_576, 4, 64, unsigned), Some(1 << 45)),
        // No sample is 1, 12 or 24 bits wide once it is hashed.
        (layout(2, 3, 1, 1, unsigned), None),
        (layout(2, 3, 1, 12, unsigned), None),
        (layout(2, 3, 1, 24, unsigned), None),
        (layout(2, 3, 1, 0, unsigned), None),
        // A frame without pixels.
        (layout(0, 3, 1, 8, unsigned), None),
        (layout(2, 0, 1, 8, unsigned), None),
        (layout(2, 3, 0, 8, unsigned), None),
        // More bytes than a `u64` counts.
        (layout(u32::MAX, u32::MAX, u32::MAX, 64, unsigned), None),
    ];
    for (layout, expected) in cases {
        assert_eq!(layout.sample_bytes(), expected, "{layout:?}");
    }
}

#[test]
fn a_digest_is_written_as_px_and_64_lowercase_hex_digits() {
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(0x1d).wrapping_add(0xa0);
    }
    assert_eq!(
        pixel_digest_text(&digest),
        "px:a0bddaf714314e6b88a5c2dffc193653708daac7e4011e3b587592afcce90623"
    );
    assert_eq!(
        pixel_digest_text(&[0; 32]),
        format!("px:{}", "0".repeat(64))
    );
}
