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
///
/// On the wire the field is one object: `id`, `name`, `applies_to` and
/// `required`, the `type` member and the members of that field type, and any
/// member this version does not know. Each member is written once. A member
/// that belongs to another field type (`options` on a `boolean` field) is
/// not a member of this field, so it is kept and written back like any
/// other unknown one.
#[derive(Debug, Clone, PartialEq, JsonSchema, TS)]
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
    /// Reading never puts `type` or a member of the field's own type here.
    ///
    /// Writing leaves out an entry whose name is `id`, `name`, `applies_to`,
    /// `required`, `type` or a member of the current field type: that member
    /// is written from the field itself. Reading never produces such an
    /// entry, but code that changes `field_type` to a type owning a member
    /// already held here does (`options` kept from a `boolean` field that
    /// becomes a `category`).
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// [`FieldDef`] as serde reads it. Two flattened members share the object's
/// remaining members: the field type takes the ones it knows but does not
/// remove them, so the map receives them as well. [`FieldDef`]'s
/// `Deserialize` drops those from the map; without that every field would
/// be written with `type` and its type's members twice.
#[derive(Deserialize)]
#[serde(rename = "FieldDef")]
struct FieldDefWire {
    id: String,
    name: String,
    #[serde(flatten)]
    field_type: FieldType,
    #[serde(default)]
    applies_to: Vec<TargetKind>,
    #[serde(default)]
    required: bool,
    #[serde(flatten)]
    rest: BTreeMap<String, Value>,
}

impl<'de> Deserialize<'de> for FieldDef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let FieldDefWire {
            id,
            name,
            field_type,
            applies_to,
            required,
            rest: mut unknown,
        } = FieldDefWire::deserialize(deserializer)?;
        for member in field_type.wire_members() {
            unknown.remove(*member);
        }
        Ok(FieldDef {
            id,
            name,
            field_type,
            applies_to,
            required,
            unknown,
        })
    }
}

/// The members [`FieldDef`] writes from its own Rust fields, the field type's
/// aside.
const FIELD_MEMBERS: [&str; 4] = ["id", "name", "applies_to", "required"];

/// [`FieldDef`] as serde writes it: the same members in the same order as
/// the derived form, with the unknown map filtered.
#[derive(Serialize)]
#[serde(rename = "FieldDef")]
struct FieldDefOut<'a> {
    id: &'a str,
    name: &'a str,
    #[serde(flatten)]
    field_type: &'a FieldType,
    applies_to: &'a [TargetKind],
    required: bool,
    #[serde(flatten)]
    rest: UnknownMembers<'a>,
}

/// The unknown members of a field without the ones the field writes itself.
/// A field type changed in Rust can own a member the map already holds;
/// writing both would name the member twice, and the text would not read.
struct UnknownMembers<'a> {
    unknown: &'a BTreeMap<String, Value>,
    owned: &'static [&'static str],
}

impl Serialize for UnknownMembers<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.unknown.iter().filter(|(member, _)| {
            let member = member.as_str();
            !FIELD_MEMBERS.contains(&member) && !self.owned.contains(&member)
        }))
    }
}

impl Serialize for FieldDef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        FieldDefOut {
            id: &self.id,
            name: &self.name,
            field_type: &self.field_type,
            applies_to: &self.applies_to,
            required: self.required,
            rest: UnknownMembers {
                unknown: &self.unknown,
                owned: self.field_type.wire_members(),
            },
        }
        .serialize(serializer)
    }
}

impl FieldType {
    /// The names of the wire members this field type owns, the tag included.
    /// Every member is named in its pattern, so a member added to a variant
    /// does not compile until it is listed here.
    fn wire_members(&self) -> &'static [&'static str] {
        match self {
            FieldType::Category {
                options: _,
                ordered: _,
            } => &["type", "options", "ordered"],
            FieldType::MultiCategory { options: _ } => &["type", "options"],
            FieldType::Boolean => &["type"],
            FieldType::Number {
                min: _,
                max: _,
                step: _,
                unit: _,
                integer: _,
            } => &["type", "min", "max", "step", "unit", "integer"],
            FieldType::Text { max_length: _ } => &["type", "max_length"],
        }
    }
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
        crate::Check::check(self, ())
    }
}
