//! Every size bound the crate enforces, as fixed numbers.
//!
//! Model data arrives from other processes and from files a user picked.
//! The two text bounds are checked before any parsing; within them, reading
//! builds whatever the text holds. Every other bound is checked by the
//! `validate` functions, each before the items of the list or the bytes of
//! the string it bounds are visited. A parser of one of the validated
//! strings ([`crate::FileKey`] and the like) refuses an over-long string on
//! its length. The bounds are part of the contract: a consumer may rely on a
//! validated value staying inside them. Raising one is a compatible change;
//! lowering one is not.

/// Largest JSON document [`crate::Document::from_json_str`] reads:
/// 268,435,456 bytes (256 MiB). Larger exports use JSON Lines.
pub const MAX_DOCUMENT_BYTES: usize = 268_435_456;

/// Largest JSON operation envelope [`crate::OpEnvelope::from_json_str`]
/// reads: 16,777,216 bytes (16 MiB). A `Batch` is one envelope. An operation
/// that carries a whole annotation (create, delete and restore) carries its
/// mask, so a mask of more tiles than fit here cannot travel as one
/// operation: about 2,000 tiles at the longest payload
/// ([`MAX_TILE_PAYLOAD_CHARS`]), far fewer than [`MAX_MASK_TILES`].
pub const MAX_ENVELOPE_BYTES: usize = 16_777_216;

/// Most operations in one `Batch`: 10,000.
pub const MAX_BATCH_OPS: usize = 10_000;

/// Most vertices in one polyline or polygon: 100,000.
pub const MAX_POINTS: usize = 100_000;

/// Most frame indices in one frame set, and in the list kept as written
/// beside it: 65,536.
pub const MAX_FRAMES_IN_SET: usize = 65_536;

/// Largest `rows` or `columns` a [`crate::FileRef`] may declare: 1,048,576.
pub const MAX_IMAGE_DIMENSION: u32 = 1_048_576;

/// Most tiles in one mask, summed over its frames, and most tile changes in
/// one `MaskTiles` operation: 262,144.
pub const MAX_MASK_TILES: usize = 262_144;

/// Longest tile payload, in base64 characters: 8,192. A 64 by 64 tile at
/// eight bits per pixel is 4,096 bytes before compression.
pub const MAX_TILE_PAYLOAD_CHARS: usize = 8_192;

/// Longest identifier, in bytes: 64. Applies to layer ids, to schema, class,
/// field and option ids, and to folder root ids.
pub const MAX_ID_BYTES: usize = 64;

/// Longest unique identifier body of a `sop:` file key, in bytes: 128. DICOM
/// allows 64; real files exceed it.
pub const MAX_SOP_UID_BYTES: usize = 128;

/// Longest display name (layer, class, field, option, code meaning), unit,
/// author string, patient, study or series identifier, format name and
/// external id, in bytes: 256.
pub const MAX_NAME_BYTES: usize = 256;

/// Longest path (`FileRef.path`, a folder target's path), in bytes: 4,096.
pub const MAX_PATH_BYTES: usize = 4_096;

/// Longest text value of a label or attribute, in bytes, whatever larger
/// `max_length` a schema declares: 65,536.
pub const MAX_TEXT_BYTES: usize = 65_536;

/// Most attributes on one annotation, and most option ids in one
/// multi-category value: 1,024.
pub const MAX_VALUES: usize = 1_024;

/// Most classes in a label schema, most fields in one, most options in one
/// field, and most layers in a document: 4,096 each.
pub const MAX_SCHEMA_ITEMS: usize = 4_096;

/// Most violations one validation call reports: 32. Validation stops looking
/// once it has collected this many.
pub const MAX_VIOLATIONS: usize = 32;
