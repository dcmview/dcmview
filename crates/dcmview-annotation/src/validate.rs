//! What a failed validation reports, and what a validation is given.
//!
//! Two levels of checking exist in this crate, and they are different on
//! purpose:
//!
//! - **Reading** (serde) checks shape: the right members with the right JSON
//!   types, known enum values, and the fixed syntax of file keys, layer ids,
//!   authors and timestamps. A record that reads may still break an
//!   invariant.
//! - **Validating** (the `validate` functions) is the strict check of every
//!   invariant. Every write goes through it: operations, and documents in the
//!   native format. The lenient EMBED CSV import does not: it keeps what it
//!   read and reports it.

use crate::geometry::ImageSize;
use crate::key::FileKey;
use crate::schema::LabelSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// Which rule a value broke. The wire strings are the snake-case variant
/// names; they are stable, so a client can phrase them and a hub can count
/// them. Variants are only added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ViolationCode {
    /// A number that is not finite.
    NonFinite,
    /// A position outside `[0, columns]` by `[0, rows]`.
    OutOfBounds,
    /// A rectangle with no area or with its corners out of order, or an
    /// ellipse radius that is not positive.
    Degenerate,
    /// An ellipse angle outside `[0, 180)`.
    BadAngle,
    TooFewPoints,
    TooManyPoints,
    /// A mask whose tile size or depth is not one this version writes.
    MaskLayout,
    /// A mask with no frame, or a frame with no tile.
    MaskEmpty,
    MaskTileOutOfBounds,
    /// A tile payload that is empty, too long, or not base64 text.
    MaskPayload,
    /// A mask annotation whose `frames` is not exactly its tiles' frames.
    MaskFramesMismatch,
    TooManyTiles,
    /// An explicit frame set with no frame.
    FramesEmpty,
    /// A frame set that is not strictly ascending.
    FramesNotNormalized,
    FrameOutOfRange,
    /// A list kept as written that names other frames than the set.
    FramesAsWrittenMismatch,
    TooManyFrames,
    /// A record or operation id that is not a version 7 UUID.
    IdNotUuidV7,
    /// A schema, class, field or option id with a character outside the
    /// allowed set, or of the wrong length.
    BadId,
    /// Two things that must differ share an id: records, layers, classes,
    /// fields, options of one field, option ids in one value, or tiles of
    /// one operation.
    DuplicateId,
    /// Two files with one key.
    DuplicateFileKey,
    /// Two labels for one target, field, layer and author.
    DuplicateLabel,
    UnknownFile,
    UnknownLayer,
    UnknownClass,
    UnknownField,
    UnknownOption,
    /// The class does not allow the geometry's type.
    GeometryNotAllowed,
    /// The class does not carry this attribute.
    AttributeNotAllowed,
    /// The field does not apply to this kind of target.
    TargetNotAllowed,
    /// A value of the wrong JSON type for its field.
    ValueType,
    /// A number below its field's minimum, above its maximum, not finite,
    /// or not whole where the field wants an integer.
    ValueOutOfRange,
    /// A string longer than its bound.
    TooLong,
    /// A list longer than its bound, where no more specific code exists.
    TooManyItems,
    /// A color that is not `#` and six hex digits.
    BadColor,
    /// A score outside `[0, 1]`.
    BadScore,
    /// A digest that does not have its scheme's form.
    BadDigest,
    BadFileRef,
    BadTarget,
    BadLayer,
    BadSchema,
    /// A patch, or a tile list, that changes nothing.
    EmptyPatch,
    /// `before` and `after` do not set the same members.
    PatchFieldsMismatch,
    /// A snapshot whose id is not the operation's.
    SnapshotIdMismatch,
    EmptyBatch,
    NestedBatch,
    TooManyOps,
}

/// One broken rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Violation {
    pub code: ViolationCode,
    /// Where in the validated value, as a JSON Pointer (RFC 6901) relative
    /// to it: `/geometry/x1`, `/annotations/3/frames`, or the empty string
    /// for the value itself.
    pub path: String,
    /// The detail, for a person to read. Not stable.
    pub detail: String,
}

/// A value that failed validation: 1 to 32 violations
/// ([`crate::limits::MAX_VIOLATIONS`]), in the order they were found.
/// Validation stops looking once it has that many.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, thiserror::Error)]
#[error("{} violation(s), first: {:?} at {:?}", .violations.len(), .violations.first().map(|violation| violation.code), .violations.first().map(|violation| violation.path.as_str()))]
pub struct Invalid {
    pub violations: Vec<Violation>,
}

impl Invalid {
    /// Whether any violation has this code.
    pub fn has(&self, code: ViolationCode) -> bool {
        self.violations
            .iter()
            .any(|violation| violation.code == code)
    }
}

/// How a validation looks up the dimensions of a file by its key.
pub trait FileSizes {
    /// The file's dimensions, or `None` when no file has this key.
    fn size_of(&self, key: &FileKey) -> Option<ImageSize>;
}

impl FileSizes for BTreeMap<FileKey, ImageSize> {
    fn size_of(&self, key: &FileKey) -> Option<ImageSize> {
        self.get(key).copied()
    }
}

impl<F: Fn(&FileKey) -> Option<ImageSize>> FileSizes for F {
    fn size_of(&self, key: &FileKey) -> Option<ImageSize> {
        self(key)
    }
}

/// What a record or an operation is validated against: the files that exist
/// and the label schema in force. A session without a schema passes
/// [`LabelSchema::implicit`].
///
/// # Values against fields
///
/// Wherever a label value or an attribute value is checked against its
/// field:
///
/// | Field type | Valid value |
/// |---|---|
/// | `category` | a string that is the id of one of the field's options, deprecated ones included (`value_type`, `unknown_option`) |
/// | `multi_category` | an array of at most 1,024 option ids ([`crate::limits::MAX_VALUES`]), each an option of the field (`unknown_option`) and none twice (`duplicate_id`); may be empty (`value_type`, `too_many_items`) |
/// | `boolean` | a boolean (`value_type`) |
/// | `number` | a finite number, not below `min`, not above `max`, and whole when `integer` is set (`value_type`, `value_out_of_range`) |
/// | `text` | a string of at most `max_length` bytes, and never more than 65,536 ([`crate::limits::MAX_TEXT_BYTES`]) (`value_type`, `too_long`) |
///
/// `required` is advisory and never makes a value or a record invalid.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub files: &'a dyn FileSizes,
    pub schema: &'a LabelSchema,
}
