//! The label schema: classes, fields and their options
//! (`docs/design/annotation-model.md` 4.2).
//!
//! A schema is campaign configuration. It says which classes a shape may
//! have, which attributes a class carries, and which fields can be set on a
//! patient, study, series, file, frame or folder. Ids are stable and names
//! are display text: renaming never touches data.

use crate::geometry::GeometryType;
use crate::number::serialize_optional_number;
use crate::validate::Invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// The id of the one class a session without a schema has.
pub const IMPLICIT_CLASS_ID: &str = "roi";

/// The kind of thing a label is set on, as a field's `applies_to` lists it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Patient,
    Study,
    Series,
    File,
    Frame,
    Folder,
}

/// A coded concept (coding scheme, value, meaning), which DICOM SEG and SR
/// exports read. Unused otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Code {
    pub scheme: String,
    pub value: String,
    pub meaning: String,
}

/// One choice of a `category` or `multi_category` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct OptionDef {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<Code>,
    /// A removed option stays in the schema, marked deprecated, so values
    /// that use it still render and still validate.
    #[serde(default)]
    pub deprecated: bool,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// What values a field takes. Serialized into the field's own object: the
/// `type` member names the variant and the variant's members sit beside the
/// field's `id` and `name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldType {
    /// One option id.
    Category {
        options: Vec<OptionDef>,
        /// The option order is a scale (`docs/design/seams.md` 7).
        #[serde(default)]
        ordered: bool,
    },
    /// A list of distinct option ids.
    MultiCategory {
        options: Vec<OptionDef>,
    },
    Boolean,
    Number {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_optional_number"
        )]
        #[ts(optional)]
        min: Option<f64>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_optional_number"
        )]
        #[ts(optional)]
        max: Option<f64>,
        /// A hint for the input control. Values are not checked against it.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_optional_number"
        )]
        #[ts(optional)]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        unit: Option<String>,
        /// Only whole numbers are valid values.
        #[serde(default)]
        integer: bool,
    },
    Text {
        /// Longest value in bytes. Absent means
        /// [`crate::limits::MAX_TEXT_BYTES`], which also caps any larger
        /// number given here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        max_length: Option<u32>,
    },
}

/// One field: an attribute of a class, a label on the targets in
/// `applies_to`, or both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FieldDef {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub field_type: FieldType,
    /// The target kinds this field can label. Empty for a field that is only
    /// an attribute of classes.
    #[serde(default)]
    pub applies_to: Vec<TargetKind>,
    /// Advisory: drives "incomplete" markers and a campaign's "done" check.
    /// A missing required value never makes a record invalid.
    #[serde(default)]
    pub required: bool,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// What a shape can be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ClassDef {
    pub id: String,
    pub name: String,
    /// `#RRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color: Option<String>,
    /// The geometry types an annotation of this class may have.
    pub geometry: Vec<GeometryType>,
    /// Ids of the fields an annotation of this class may carry as
    /// attributes.
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<Code>,
    /// A removed class stays in the schema, marked deprecated, so records
    /// that use it still render and still validate.
    #[serde(default)]
    pub deprecated: bool,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// A label schema. It has its own id and version, independent of the model
/// version.
///
/// Invariants, checked by [`LabelSchema::validate`]:
///
/// - `schema_id` and every class, field and option id is 1 to 64 characters
///   ([`crate::limits::MAX_ID_BYTES`]), each an ASCII letter, digit, `.`,
///   `-` or `_` (`bad_id`).
/// - Class ids are distinct, field ids are distinct, and option ids are
///   distinct within a field (`duplicate_id`).
/// - Every attribute a class lists is a field of the schema
///   (`unknown_field`).
/// - A class allows at least one geometry type, and a category or
///   multi-category field has at least one option (`bad_schema`).
/// - A number field's `min` is not above its `max`, and both are finite
///   (`bad_schema`).
/// - A color is `#` and six hex digits (`bad_color`).
/// - Names, units and code members are at most 256 bytes
///   ([`crate::limits::MAX_NAME_BYTES`], `too_long`); at most 4,096 classes,
///   fields, and options per field ([`crate::limits::MAX_SCHEMA_ITEMS`],
///   `too_many_items`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LabelSchema {
    pub schema_id: String,
    pub schema_version: u32,
    #[serde(default)]
    pub classes: Vec<ClassDef>,
    #[serde(default)]
    pub fields: Vec<FieldDef>,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

impl LabelSchema {
    /// The schema of a session that has none: one class, `roi`, allowing
    /// every geometry type, with no attributes, and no fields. This is the
    /// viewer's behaviour without `--annotation-config`.
    pub fn implicit() -> LabelSchema {
        LabelSchema {
            schema_id: "implicit".to_string(),
            schema_version: 1,
            classes: vec![ClassDef {
                id: IMPLICIT_CLASS_ID.to_string(),
                name: "ROI".to_string(),
                color: None,
                geometry: GeometryType::ALL.to_vec(),
                attributes: Vec::new(),
                code: None,
                deprecated: false,
                unknown: BTreeMap::new(),
            }],
            fields: Vec::new(),
            unknown: BTreeMap::new(),
        }
    }

    /// Checks the invariants in the type's documentation.
    ///
    /// Bounded work: a list longer than
    /// [`crate::limits::MAX_SCHEMA_ITEMS`] is refused on its length alone.
    pub fn validate(&self) -> Result<(), Invalid> {
        todo!("FND3: validate a label schema")
    }
}
