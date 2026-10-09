//! DICOM Part 10 files written element by element, so a test can write a
//! value length the file does not hold, a value far larger than anything a
//! reader needs, or a data set that inflates to many times its file.
//!
//! The files of `data_sets.rs`.

use dicom_core::Tag;
use dicom_dictionary_std::{tags, uids};
use dicom_object::meta::FileMetaTableBuilder;
use std::io::Write;

pub const EXPLICIT_LE: &str = uids::EXPLICIT_VR_LITTLE_ENDIAN;
pub const DEFLATED_LE: &str = uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN;
pub const JPEG_BASELINE: &str = uids::JPEG_BASELINE8_BIT;
pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;

/// A private element no reader of the viewer asks for.
pub const PRIVATE_BLOB: Tag = Tag(0x0009, 0x1010);
/// Overlay Data of the first overlay group, which the catalog keeps.
pub const OVERLAY_DATA: Tag = Tag(0x6000, 0x3000);
/// A sequence the catalog has no use for.
pub const CONTENT_SEQUENCE: Tag = Tag(0x0040, 0xA730);
/// Data Set Trailing Padding, the last element a data set can hold.
pub const TRAILING_PADDING: Tag = Tag(0xFFFC, 0xFFFC);

fn has_long_length(vr: &str) -> bool {
    matches!(
        vr,
        "OB" | "OD" | "OF" | "OL" | "OV" | "OW" | "SQ" | "UC" | "UN" | "UR" | "UT" | "SV" | "UV"
    )
}

/// One Explicit VR Little Endian element whose header declares `declared`
/// bytes and whose value is `value`, whatever its length.
pub fn element_declaring(tag: Tag, vr: &str, declared: u32, value: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 + value.len());
    bytes.extend_from_slice(&tag.0.to_le_bytes());
    bytes.extend_from_slice(&tag.1.to_le_bytes());
    bytes.extend_from_slice(vr.as_bytes());
    if has_long_length(vr) {
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&declared.to_le_bytes());
    } else {
        let declared = u16::try_from(declared).expect("a short VR holds a 16-bit length");
        bytes.extend_from_slice(&declared.to_le_bytes());
    }
    bytes.extend_from_slice(value);
    bytes
}

/// One element whose header declares the length of `value`, padded to an
/// even length as the standard requires.
pub fn element(tag: Tag, vr: &str, value: &[u8]) -> Vec<u8> {
    let mut value = value.to_vec();
    if value.len() & 1 == 1 {
        value.push(if matches!(vr, "UI" | "OB" | "UN") {
            0
        } else {
            b' '
        });
    }
    element_declaring(tag, vr, value.len() as u32, &value)
}

fn us(tag: Tag, value: u16) -> Vec<u8> {
    element(tag, "US", &value.to_le_bytes())
}

/// The elements of an image before group 0009, in tag order. `uid` is its
/// SOP Instance UID.
pub fn identity(uid: &str) -> Vec<u8> {
    [
        element(
            tags::SOP_CLASS_UID,
            "UI",
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE.as_bytes(),
        ),
        element(tags::SOP_INSTANCE_UID, "UI", uid.as_bytes()),
        element(tags::MODALITY, "CS", b"OT"),
    ]
    .concat()
}

/// The image pixel module of a `rows` x `columns` 16-bit grayscale image.
pub fn image_module(rows: u16, columns: u16) -> Vec<u8> {
    [
        us(tags::SAMPLES_PER_PIXEL, 1),
        element(tags::PHOTOMETRIC_INTERPRETATION, "CS", b"MONOCHROME2"),
        us(tags::ROWS, rows),
        us(tags::COLUMNS, columns),
        us(tags::BITS_ALLOCATED, 16),
        us(tags::BITS_STORED, 16),
        us(tags::HIGH_BIT, 15),
        us(tags::PIXEL_REPRESENTATION, 0),
    ]
    .concat()
}

/// Native Pixel Data holding `samples`.
pub fn pixel_data(samples: &[u16]) -> Vec<u8> {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    element(tags::PIXEL_DATA, "OW", &bytes)
}

/// A sequence of undefined length holding `items`, each the elements of one
/// item of undefined length.
pub fn sequence(tag: Tag, items: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = element_declaring(tag, "SQ", u32::MAX, &[]);
    for item in items {
        bytes.extend_from_slice(&[0xfe, 0xff, 0x00, 0xe0, 0xff, 0xff, 0xff, 0xff]);
        bytes.extend_from_slice(item);
        bytes.extend_from_slice(&[0xfe, 0xff, 0x0d, 0xe0, 0, 0, 0, 0]);
    }
    bytes.extend_from_slice(&[0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0]);
    bytes
}

/// `depth` sequences, each the only element of the one item of the one
/// before, around `innermost`. Closed with delimiters when `closed`.
pub fn nested(tag: Tag, depth: usize, innermost: &[u8], closed: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..depth {
        bytes.extend_from_slice(&element_declaring(tag, "SQ", u32::MAX, &[]));
        bytes.extend_from_slice(&[0xfe, 0xff, 0x00, 0xe0, 0xff, 0xff, 0xff, 0xff]);
    }
    bytes.extend_from_slice(innermost);
    if closed {
        for _ in 0..depth {
            bytes.extend_from_slice(&[0xfe, 0xff, 0x0d, 0xe0, 0, 0, 0, 0]);
            bytes.extend_from_slice(&[0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0]);
        }
    }
    bytes
}

/// Encapsulated Pixel Data: an empty offset table and one fragment per
/// entry of `fragments`.
pub fn encapsulated_pixel_data(fragments: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = element_declaring(tags::PIXEL_DATA, "OB", u32::MAX, &[]);
    bytes.extend_from_slice(&[0xfe, 0xff, 0x00, 0xe0, 0, 0, 0, 0]);
    for fragment in fragments {
        bytes.extend_from_slice(&[0xfe, 0xff, 0x00, 0xe0]);
        bytes.extend_from_slice(&(fragment.len() as u32).to_le_bytes());
        bytes.extend_from_slice(fragment);
    }
    bytes.extend_from_slice(&[0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0]);
    bytes
}

/// The preamble, the magic code and a file meta group naming
/// `transfer_syntax`, then `meta_tail` inside the group and `data_set` after
/// it. A deflated transfer syntax deflates `data_set`.
pub fn part10_with_meta(transfer_syntax: &str, meta_tail: &[u8], data_set: &[u8]) -> Vec<u8> {
    let meta = FileMetaTableBuilder::new()
        .transfer_syntax(transfer_syntax)
        .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
        .media_storage_sop_instance_uid("2.25.4000")
        .build()
        .expect("file meta");
    let mut bytes = vec![0_u8; 128];
    bytes.extend_from_slice(b"DICM");
    let group_start = bytes.len();
    meta.write(&mut bytes).expect("write file meta");
    if !meta_tail.is_empty() {
        // The group length is the value of the first element, (0002,0000) UL.
        let length = u32::from_le_bytes(
            bytes[group_start + 8..group_start + 12]
                .try_into()
                .expect("group length"),
        ) + meta_tail.len() as u32;
        bytes[group_start + 8..group_start + 12].copy_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(meta_tail);
    }
    if transfer_syntax == DEFLATED_LE {
        let mut deflater =
            flate2::write::DeflateEncoder::new(&mut bytes, flate2::Compression::fast());
        deflater.write_all(data_set).expect("deflate the data set");
        deflater.finish().expect("finish the deflate stream");
    } else {
        bytes.extend_from_slice(data_set);
    }
    bytes
}

pub fn part10(transfer_syntax: &str, data_set: &[u8]) -> Vec<u8> {
    part10_with_meta(transfer_syntax, &[], data_set)
}

/// A 2 x 2 image with `before` between its identity and its image module
/// and `after` behind its pixel data.
pub fn image(transfer_syntax: &str, before: &[u8], after: &[u8]) -> Vec<u8> {
    part10(
        transfer_syntax,
        &[
            identity("2.25.4000"),
            before.to_vec(),
            image_module(2, 2),
            pixel_data(&[1, 2, 3, 4]),
            after.to_vec(),
        ]
        .concat(),
    )
}
