//! The document: the model's serialization
//! (`docs/design/annotation-model.md` 6.2, 6.4).

use crate::file::FileRef;
use crate::label::Label;
use crate::layer::Layer;
use crate::record::Annotation;
use crate::schema::LabelSchema;
use crate::validate::Invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// The `format` member of every document.
pub const FORMAT: &str = "dcmview.annotations";

/// The `version` member this crate writes: `<major>.<minor>`.
///
/// A reader accepts any document of its own major version, whatever the
/// minor. Within a major version the format changes only by adding optional
/// members, which older readers keep and ignore. Removing or renaming a
/// member, changing a meaning, or adding a geometry type, an operation or
/// any other enum value needs a new major version and a migration function
/// in this crate.
pub const VERSION: &str = "1.0";

/// The major version this crate reads.
pub const VERSION_MAJOR: u32 = 1;

/// One self-contained set of model data.
///
/// ```json
/// { "format": "dcmview.annotations", "version": "1.0",
///   "schema": { ... }, "files": [ ... ], "layers": [ ... ],
///   "annotations": [ ... ], "labels": [ ... ] }
/// ```
///
/// `schema` is `null` for a session without one, which means
/// [`LabelSchema::implicit`]. A deleted record is not in a document.
///
/// Members this version does not know are kept and written back unchanged
/// on the document, on the schema and each of its classes, fields and
/// options, and on every file (and its `space`), layer (and its `source`),
/// annotation and label. Inside a geometry, a frame set, a code or a spacing
/// entry they are ignored when read, and a label target with one is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Document {
    pub format: String,
    pub version: String,
    #[serde(default)]
    pub schema: Option<LabelSchema>,
    #[serde(default)]
    pub files: Vec<FileRef>,
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Adapter-specific data, keyed by adapter id.
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DocumentError {
    /// The text is longer than [`crate::limits::MAX_DOCUMENT_BYTES`].
    #[error("document is {bytes} bytes, more than the {limit} allowed")]
    TooLarge { bytes: usize, limit: usize },
    /// The text is not JSON, or not the JSON of a document.
    #[error("document is malformed: {0}")]
    Malformed(String),
    /// `format` is not [`FORMAT`].
    #[error("not a dcmview.annotations document: format is {found:?}")]
    WrongFormat { found: String },
    /// `version` is not `<major>.<minor>` in decimal, or its major version
    /// is not [`VERSION_MAJOR`].
    #[error("unsupported document version {found:?}")]
    UnsupportedVersion { found: String },
}

impl Document {
    /// An empty document of the current format and version with no schema.
    pub fn new() -> Document {
        Document {
            format: FORMAT.to_string(),
            version: VERSION.to_string(),
            schema: None,
            files: Vec::new(),
            layers: Vec::new(),
            annotations: Vec::new(),
            labels: Vec::new(),
            extensions: BTreeMap::new(),
            unknown: BTreeMap::new(),
        }
    }

    /// Reads a document from JSON text and checks its format and version.
    ///
    /// Bounded: text longer than 268,435,456 bytes
    /// ([`crate::limits::MAX_DOCUMENT_BYTES`]) is refused before it is
    /// parsed, and JSON nested deeper than serde_json's limit of 128 levels
    /// is refused by the parser. Never panics on any input.
    ///
    /// `format` must be [`FORMAT`]. `version` must be `<major>.<minor>`:
    /// the major is [`VERSION_MAJOR`] written the one way this crate writes
    /// it, the text `1` (`01.0` and `+1.0` are refused), and the minor is 1
    /// to 9 ASCII digits; any minor version reads. Records are read for
    /// shape only; call [`Document::validate`] for the strict check.
    pub fn from_json_str(text: &str) -> Result<Document, DocumentError> {
        let limit = crate::limits::MAX_DOCUMENT_BYTES;
        if text.len() > limit {
            return Err(DocumentError::TooLarge {
                bytes: text.len(),
                limit,
            });
        }
        let document: Document = serde_json::from_str(text)
            .map_err(|error| DocumentError::Malformed(error.to_string()))?;
        if document.format != FORMAT {
            return Err(DocumentError::WrongFormat {
                found: document.format,
            });
        }
        let valid = document.version.len() <= 19
            && document
                .version
                .split_once('.')
                .is_some_and(|(major, minor)| {
                    let digits = |part: &str| {
                        !part.is_empty()
                            && part.len() <= 9
                            && part.bytes().all(|b| b.is_ascii_digit())
                    };
                    digits(major)
                        && digits(minor)
                        && major.parse::<u32>().ok() == Some(VERSION_MAJOR)
                });
        if !valid {
            return Err(DocumentError::UnsupportedVersion {
                found: document.version,
            });
        }
        Ok(document)
    }

    /// The strict check of a whole document: what the native format's
    /// importer requires. `format` and `version` are not looked at; reading
    /// checks them.
    ///
    /// - The schema, when present, is valid ([`LabelSchema::validate`]).
    /// - Every file is valid ([`FileRef::validate`]) and no two share a key
    ///   (`duplicate_file_key`).
    /// - Every layer is valid ([`Layer::validate`]) and no two share an id
    ///   (`duplicate_id`); at most 4,096 layers
    ///   ([`crate::limits::MAX_SCHEMA_ITEMS`], `too_many_items`).
    /// - Every annotation is valid against the document's own files and
    ///   schema ([`Annotation::validate`]), is in a layer of the document
    ///   (`unknown_layer`), and no two share an id (`duplicate_id`).
    /// - Every label is valid ([`Label::validate`]), is in a layer of the
    ///   document (`unknown_layer`), no two share an id (`duplicate_id`),
    ///   and no two share target, field, layer and `created_by`
    ///   (`duplicate_label`).
    ///
    /// Work is linear in the size of the document, and stops after 32
    /// violations ([`crate::limits::MAX_VIOLATIONS`]). Linear includes the
    /// schema: a class, a field, an option, a class's attribute or geometry
    /// type and a field's target kind are each found through an index built
    /// once, never by scanning a list of the schema for each record that
    /// names one.
    pub fn validate(&self) -> Result<(), Invalid> {
        crate::Check::check(self, ())
    }
}

impl Default for Document {
    fn default() -> Document {
        Document::new()
    }
}
