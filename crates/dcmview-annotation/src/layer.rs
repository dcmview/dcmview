//! Layers: groups of annotations and labels
//! (`docs/design/annotation-model.md` 5).

use crate::key::{Author, LayerId};
use crate::validate::Invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// What a layer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    /// Someone's working layer.
    User,
    /// Records loaded from a file.
    Import,
    /// Pre-labels or inference output.
    Model,
    /// Another annotator's work shown to an admin. Always read-only.
    Review,
}

/// Where a layer's content comes from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct LayerSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author: Option<Author>,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

/// One layer. Every annotation and label belongs to exactly one, and layers
/// are session-wide, not per image.
///
/// ```json
/// { "id": "L-01", "name": "Alice", "kind": "user", "exclusive_masks": false,
///   "color": null, "readonly": false, "source": { "author": "user:alice" }, "rev": 1 }
/// ```
///
/// Whether a layer is visible, locked, or above another is view state: per
/// user and per session, never part of this record.
///
/// Invariants, checked by [`Layer::validate`]: `name` is 1 to 256 bytes
/// ([`crate::limits::MAX_NAME_BYTES`], `too_long` or `bad_layer` when
/// empty); `color`, when present, is `#` and six hex digits (`bad_color`); a
/// `review` layer is read-only (`bad_layer`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    /// Painting a segment clears those pixels from every other segment of
    /// the layer, which gives label-map behaviour.
    #[serde(default)]
    pub exclusive_masks: bool,
    /// `#RRGGBB`, or `null` to use each class's color.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub source: LayerSource,
    /// Counts the layer's changes, like a record's `rev`; a layer operation's
    /// `base_rev` is compared with it. Assigned by the store.
    #[serde(default)]
    pub rev: u64,
    /// Members this version does not know, kept so they survive a round trip.
    #[serde(flatten)]
    #[ts(skip)]
    pub unknown: BTreeMap<String, Value>,
}

impl Layer {
    /// Checks the invariants in the type's documentation.
    pub fn validate(&self) -> Result<(), Invalid> {
        crate::Check::check(self, ())
    }
}

/// The members of a [`Layer`] an `UpdateLayer` operation can change. A member
/// that is absent is not changed. `color` distinguishes absent (not changed)
/// from `null` (cleared).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct LayerPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[ts(optional, type = "string | null")]
    pub color: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exclusive_masks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub readonly: Option<bool>,
}

impl LayerPatch {
    /// Sets on `layer` every member this patch carries, and nothing else
    /// (`rev` included: the store owns it).
    pub fn apply_to(&self, layer: &mut Layer) {
        if let Some(name) = &self.name {
            layer.name = name.clone();
        }
        if let Some(color) = &self.color {
            layer.color = color.clone();
        }
        if let Some(exclusive_masks) = self.exclusive_masks {
            layer.exclusive_masks = exclusive_masks;
        }
        if let Some(readonly) = self.readonly {
            layer.readonly = readonly;
        }
    }
}

/// Reads a member that is present, `null` included, as `Some`: with
/// `#[serde(default)]` an absent member stays `None`.
fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}
