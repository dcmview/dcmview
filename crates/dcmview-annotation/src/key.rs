//! File keys and the other validated string forms of the model.
//!
//! Each type here is a string with a fixed syntax. A value of one of these
//! types is always well formed: the only ways to build one are its `parse`
//! function and deserialization, which goes through `parse`. Code that
//! receives one never has to check it again.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Version of the file-key rules: which key a file gets and how a key is
/// written (`docs/design/annotation-model.md` 1.3, `docs/design/seams.md`
/// 12). Any change to the syntax [`FileKey::parse`] accepts, or to how a key
/// is derived from a file, raises it, because stored records are addressed by
/// key.
pub const KEY_RULES: u32 = 1;

/// A string that is not a well-formed value of the type it was parsed as.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind}: {reason}")]
pub struct InvalidValue {
    /// The kind of value that was expected, such as `"file key"`.
    pub kind: &'static str,
    /// What is wrong with it, for a person to read.
    pub reason: String,
}

/// The scheme of a [`FileKey`], which is the part before its first colon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyScheme {
    /// `sop:<SOP Instance UID>`: a DICOM instance whose UID identifies it.
    Sop,
    /// `b3:<64 lowercase hex>`: the BLAKE3 digest of the file's bytes exactly
    /// as stored. Rasters, DICOM files without a UID, and DICOM files that
    /// share a UID with different bytes.
    Blake3,
}

/// The stable identity of one file (one SOP instance or one raster).
///
/// Two forms exist under [`KEY_RULES`] 1, and no other string is a file key:
///
/// - `sop:<uid>`, where `<uid>` is 1 to 128 characters
///   ([`crate::limits::MAX_SOP_UID_BYTES`]), each an ASCII letter, digit,
///   `.`, `-` or `_`. Conforming DICOM UIDs use digits and `.` only; the
///   wider set keeps the keys of files whose UID a de-identifier replaced
///   with something else. The UID is taken as written: no trimming, no case
///   folding.
/// - `b3:<digest>`, where `<digest>` is exactly 64 characters, each `0`-`9`
///   or `a`-`f` (the full 256-bit digest, lowercase, never truncated).
///
/// Which of the two a given file gets, and when, is decided by whoever owns
/// the file list (the viewer's discovery, or a hub at campaign setup), not by
/// this type.
///
/// On the wire a key is the plain string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(type = "string")]
pub struct FileKey(String);

impl FileKey {
    /// Parses a key in one of the two forms in the type's documentation.
    ///
    /// Fails, without panicking and without allocating in proportion to the
    /// input, for any other string, including an empty one, an unknown
    /// scheme, an empty or over-long UID, a UID with any other character, and
    /// a digest of the wrong length or with an uppercase or non-hex
    /// character.
    pub fn parse(text: &str) -> Result<Self, InvalidValue> {
        let _ = text;
        todo!("FND3: parse a file key")
    }

    /// The key `sop:<uid>` for a SOP Instance UID. Fails exactly when
    /// [`FileKey::parse`] would refuse the resulting string.
    pub fn sop(uid: &str) -> Result<Self, InvalidValue> {
        let _ = uid;
        todo!("FND3: build a sop: key")
    }

    /// The key `b3:<hex>` for a BLAKE3 digest of a file's bytes, written as
    /// 64 lowercase hex characters, first byte first.
    pub fn blake3(digest: &[u8; 32]) -> Self {
        let _ = digest;
        todo!("FND3: build a b3: key")
    }

    /// The key as written on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Which form the key has.
    pub fn scheme(&self) -> KeyScheme {
        todo!("FND3: read the scheme of a file key")
    }

    /// The part after the scheme: the UID of a `sop:` key or the 64 hex
    /// characters of a `b3:` key.
    pub fn body(&self) -> &str {
        todo!("FND3: read the body of a file key")
    }
}

/// A layer's id: 1 to 64 characters ([`crate::limits::MAX_ID_BYTES`]), each
/// an ASCII letter, digit, `.`, `-` or `_`.
///
/// Layer ids are not required to be UUIDs, because a campaign configuration
/// names its initial layers. The character set has no colon, so a layer id
/// never collides with a file key or a canonical label target id when the
/// three are used as queue keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(type = "string")]
pub struct LayerId(String);

impl LayerId {
    /// Parses a layer id. Fails for an empty or over-long string and for any
    /// character outside the set in the type's documentation.
    pub fn parse(text: &str) -> Result<Self, InvalidValue> {
        let _ = text;
        todo!("FND3: parse a layer id")
    }

    /// The id as written on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What kind of actor an [`Author`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthorKind {
    /// `user:<name>`: a person.
    User,
    /// `model:<name>@<version>`: a model's output.
    Model,
    /// `import:<adapter>`: records loaded by an import adapter.
    Import,
}

/// Who created or changed a record: `user:<name>`, `model:<name>@<version>`
/// or `import:<adapter>` (`docs/design/annotation-model.md` 6.1).
///
/// The whole string is at most 256 bytes ([`crate::limits::MAX_NAME_BYTES`]).
/// The part after the prefix is not empty and holds no whitespace and no
/// control character; other characters, including non-ASCII ones, are
/// allowed. A `model:` author has exactly the form `<name>@<version>` with
/// both parts non-empty, split at the last `@`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(type = "string")]
pub struct Author(String);

impl Author {
    /// Parses an author string in one of the three forms.
    pub fn parse(text: &str) -> Result<Self, InvalidValue> {
        let _ = text;
        todo!("FND3: parse an author string")
    }

    /// The author as written on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Which of the three forms the author has.
    pub fn kind(&self) -> AuthorKind {
        todo!("FND3: read the kind of an author")
    }
}

/// A UTC instant with millisecond precision, written exactly as
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` (24 ASCII characters), for example
/// `2026-09-29T21:04:11.120Z`.
///
/// One fixed form, so that two timestamps compare as instants when they
/// compare as strings. The fields are range checked (month 01 to 12, day 01
/// to 31, hour 00 to 23, minute and second 00 to 59); a day past the end of
/// its month is not detected. No other RFC 3339 spelling is accepted: no
/// offset, no lowercase `t` or `z`, no other number of fraction digits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String", into = "String")]
#[ts(type = "string")]
pub struct Timestamp(String);

impl Timestamp {
    /// Parses a timestamp in the one accepted form.
    pub fn parse(text: &str) -> Result<Self, InvalidValue> {
        let _ = text;
        todo!("FND3: parse a timestamp")
    }

    /// The timestamp as written on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The wiring every validated string shares: deserialization goes through
/// `parse`, serialization writes the string, and the JSON Schema is a string
/// with a pattern that states the syntax for consumers in other languages.
macro_rules! validated_string {
    ($name:ident, $schema_name:literal, $pattern:literal) => {
        impl TryFrom<String> for $name {
            type Error = InvalidValue;

            fn try_from(text: String) -> Result<Self, Self::Error> {
                Self::parse(&text)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = InvalidValue;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::parse(text)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                $schema_name.into()
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({ "type": "string", "pattern": $pattern })
            }
        }
    };
}

validated_string!(
    FileKey,
    "FileKey",
    "^(sop:[A-Za-z0-9._-]{1,128}|b3:[0-9a-f]{64})$"
);
validated_string!(LayerId, "LayerId", "^[A-Za-z0-9._-]{1,64}$");
validated_string!(
    Author,
    "Author",
    "^(user:[^\\s]+|model:[^\\s]+@[^\\s@]+|import:[^\\s]+)$"
);
validated_string!(
    Timestamp,
    "Timestamp",
    "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\\.[0-9]{3}Z$"
);
