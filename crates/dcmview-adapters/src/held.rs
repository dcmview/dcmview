//! How much memory a document costs to import, counted on the things it
//! is made of.
//!
//! What a document costs to hold cannot be told from its text: a value of
//! two bytes can become a map node of several hundred. So nothing here
//! predicts it from text. [`Held`] says what a file reference, a layer, an
//! annotation or a label holds, by what is in it; [`Charged`] adds the
//! room an import needs for one such item beside what the item holds; and
//! [`document_charge`] is the sum for a whole document.
//!
//! The reader ([`crate::native`]) adds the charge of each item as it is
//! built to a total it keeps within [`crate::Limits::held_bytes`], and the
//! writers refuse a document whose charge is more than that. **Both use
//! the functions of this module on the same records, so what an export
//! writes an import with the same bounds reads**: the charge is a function
//! of what a record says (the lengths of its strings and lists, the
//! number of its members), not of how it happens to be laid out in
//! memory, and a record says the same thing before it is written and
//! after it is read.
//!
//! # What an item holds
//!
//! Every allocation it owns, counted so that the figure is never less
//! than what the allocator gives a record that was read from text:
//!
//! - a string: its length, and [`ALLOCATION`] for the allocator's own
//!   bookkeeping. An empty one holds nothing;
//! - a list (a vector): room for twice its elements and for no fewer
//!   than four, which is the most a vector that grew by doubling has, and
//!   [`ALLOCATION`];
//! - a map (`BTreeMap`): its nodes. A node has room for 11 entries, so a
//!   map of up to 11 is one node, whatever it holds: a map of one member
//!   costs as much as one of eleven. A larger map has nodes that hold at
//!   least 5, and is counted as `ceil(n / 5)` leaves and a fifth as many
//!   nodes above them, which have 12 child pointers more;
//! - a JSON value kept as it was read: nothing for a number, `true`,
//!   `false` or `null` beyond the 32 bytes it has in its parent; a string
//!   as a string; an array as a list of values; an object as a map from
//!   strings to values;
//! - and what each element or entry holds in turn.
//!
//! `tests/raster_cost/annotations.rs` compares the figure with what the
//! allocator gave out, for each kind of item and the shapes of it that
//! cost the most.
//!
//! # What an item is charged
//!
//! What it holds, and its room ([`Charged::ROOM`]): the bytes of the item
//! itself in every list it passes through on its way into a session, at
//! the moment the most of them exist, and its share of the structures
//! that have one entry a record (row numbers, sets of ids, the store's
//! indexes). A file reference and a layer are in the reader's list, which
//! grows by a quarter (nine quarters of the item while it grows). An
//! annotation and a label go on into the store, where the item is at once
//! in the list being emptied and in the store's (three times the item
//! covers the two lists and the operation that carries it).

use dcmview_annotation::{
    Annotation, Document, FileRef, FrameScope, Geometry, Label, LabelTarget, LabelValue, Layer,
    RecordMeta,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::mem::size_of;

/// What one allocation is counted as costing beside its bytes.
pub const ALLOCATION: u64 = 16;

/// The heap a value owns, in bytes, by the module documentation.
pub trait Held {
    fn held_bytes(&self) -> u64;
}

fn bytes(length: usize) -> u64 {
    if length == 0 {
        0
    } else {
        length as u64 + ALLOCATION
    }
}

fn text(value: &str) -> u64 {
    bytes(value.len())
}

fn optional(value: &Option<String>) -> u64 {
    value.as_ref().map_or(0, |value| text(value))
}

fn vector<T>(values: &[T], each: impl Fn(&T) -> u64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    bytes((2 * values.len()).max(4) * size_of::<T>()) + values.iter().map(each).sum::<u64>()
}

/// The nodes of a map of `entries` entries of `entry` bytes each.
fn nodes(entries: usize, entry: usize) -> u64 {
    if entries == 0 {
        return 0;
    }
    let leaf = 11 * entry as u64 + 16 + ALLOCATION;
    if entries <= 11 {
        return leaf;
    }
    let leaves = entries.div_ceil(5) as u64;
    leaves * leaf + leaves.div_ceil(5) * (leaf + 12 * 8)
}

fn members(map: &BTreeMap<String, Value>) -> u64 {
    nodes(map.len(), size_of::<String>() + size_of::<Value>())
        + map
            .iter()
            .map(|(name, value)| text(name) + value.held_bytes())
            .sum::<u64>()
}

impl Held for Value {
    fn held_bytes(&self) -> u64 {
        match self {
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
            Value::String(text) => self::text(text),
            Value::Array(items) => vector(items, Held::held_bytes),
            Value::Object(object) => {
                nodes(object.len(), size_of::<String>() + size_of::<Value>())
                    + object
                        .iter()
                        .map(|(name, value)| text(name) + value.held_bytes())
                        .sum::<u64>()
            }
        }
    }
}

impl Held for serde_json::Map<String, Value> {
    fn held_bytes(&self) -> u64 {
        nodes(self.len(), size_of::<String>() + size_of::<Value>())
            + self
                .iter()
                .map(|(name, value)| text(name) + value.held_bytes())
                .sum::<u64>()
    }
}

impl Held for LabelValue {
    fn held_bytes(&self) -> u64 {
        match self {
            LabelValue::Bool(_) | LabelValue::Number(_) => 0,
            LabelValue::Text(text) => self::text(text),
            LabelValue::Many(values) => vector(values, |value| text(value)),
        }
    }
}

impl Held for RecordMeta {
    fn held_bytes(&self) -> u64 {
        text(self.created_by.as_str())
            + text(self.created_at.as_str())
            + text(self.modified_by.as_str())
            + text(self.modified_at.as_str())
    }
}

impl Held for FrameScope {
    fn held_bytes(&self) -> u64 {
        match self {
            FrameScope::All => 0,
            FrameScope::Set(frames) => {
                vector(&frames.set, |_| 0)
                    + frames
                        .as_written
                        .as_ref()
                        .map_or(0, |written| vector(written, |_| 0))
            }
        }
    }
}

impl Held for Geometry {
    fn held_bytes(&self) -> u64 {
        match self {
            Geometry::Polyline { points } | Geometry::Polygon { points } => vector(points, |_| 0),
            Geometry::Mask(mask) => {
                nodes(mask.frames.len(), 4 + size_of::<BTreeMap<u64, String>>())
                    + mask
                        .frames
                        .values()
                        .map(|tiles| {
                            nodes(tiles.len(), 8 + size_of::<String>())
                                + tiles.values().map(|payload| text(&payload.0)).sum::<u64>()
                        })
                        .sum::<u64>()
            }
            _ => 0,
        }
    }
}

impl Held for Annotation {
    fn held_bytes(&self) -> u64 {
        text(self.file.as_str())
            + self.frames.held_bytes()
            + text(self.layer.as_str())
            + text(&self.class)
            + self.geometry.held_bytes()
            + nodes(
                self.attributes.len(),
                size_of::<String>() + size_of::<LabelValue>(),
            )
            + self
                .attributes
                .iter()
                .map(|(name, value)| text(name) + value.held_bytes())
                .sum::<u64>()
            + self.meta.held_bytes()
            + members(&self.extensions)
            + members(&self.unknown)
    }
}

/// What a label's target holds.
fn target(label: &Label) -> u64 {
    match &label.target {
        LabelTarget::Patient { patient: id }
        | LabelTarget::Study { study: id }
        | LabelTarget::Series { series: id } => text(id),
        LabelTarget::File { file } | LabelTarget::Frame { frame: file, .. } => text(file.as_str()),
        LabelTarget::Folder { folder, root } => text(folder) + text(root),
    }
}

impl Held for Label {
    fn held_bytes(&self) -> u64 {
        target(self)
            + text(&self.field)
            + self.value.held_bytes()
            + text(self.layer.as_str())
            + self.meta.held_bytes()
            + members(&self.extensions)
            + members(&self.unknown)
    }
}

impl Held for Layer {
    fn held_bytes(&self) -> u64 {
        text(self.id.as_str())
            + text(&self.name)
            + optional(&self.color)
            + self
                .source
                .author
                .as_ref()
                .map_or(0, |author| text(author.as_str()))
            + members(&self.source.unknown)
            + members(&self.unknown)
    }
}

impl Held for FileRef {
    fn held_bytes(&self) -> u64 {
        text(self.key.as_str())
            + optional(&self.sop_instance_uid)
            + optional(&self.sop_class_uid)
            + optional(&self.study_instance_uid)
            + optional(&self.series_instance_uid)
            + optional(&self.patient_id)
            + text(&self.path)
            + optional(&self.digest)
            + members(&self.space.unknown)
            + text(&self.format)
            + vector(&self.pixel_digest, optional)
            + self
                .frame_source
                .as_ref()
                .map_or(0, |frames| vector(frames, |_| 0))
            + optional(&self.external_id)
            + self.spacing.as_ref().map_or(0, |spacing| {
                vector(spacing, |entry| {
                    entry.as_ref().map_or(0, |entry| text(&entry.source))
                })
            })
            + members(&self.unknown)
    }
}

/// An item of a document, with the room an import needs for it beside
/// what it holds (the module documentation, "What an item is charged").
pub trait Charged: Held {
    /// The room for one item, in bytes.
    const ROOM: u64;

    /// What importing this item is counted as costing.
    fn charge(&self) -> u64 {
        self.held_bytes() + Self::ROOM
    }
}

impl Charged for FileRef {
    const ROOM: u64 = (size_of::<FileRef>() as u64 * 9).div_ceil(4) + 192;
}

impl Charged for Layer {
    const ROOM: u64 = (size_of::<Layer>() as u64 * 9).div_ceil(4) + 192;
}

impl Charged for Annotation {
    const ROOM: u64 = size_of::<Annotation>() as u64 * 3 + 448;
}

impl Charged for Label {
    const ROOM: u64 = size_of::<Label>() as u64 * 3 + 448;

    /// A store finds a label by what it is on: its index holds the
    /// label's target, field and layer a second time, with the author,
    /// so those are charged twice and 128 bytes more.
    fn charge(&self) -> u64 {
        self.held_bytes()
            + Self::ROOM
            + target(self)
            + text(&self.field)
            + text(self.layer.as_str())
            + 128
    }
}

/// What the members of a document that are not in its four lists are
/// counted as holding: its `extensions` and the members this version does
/// not know. Its schema is not counted; one unit of text bounds it
/// ([`crate::limits::MAX_UNIT_VALUES`]).
pub fn header_charge(document: &Document) -> u64 {
    members(&document.extensions) + members(&document.unknown)
}

/// What importing `document` is counted as costing: its header and every
/// item of its four lists. An import refuses a document whose charge is
/// more than [`crate::Limits::held_bytes`], adding it up as it reads, and
/// an export refuses to write one.
pub fn document_charge(document: &Document) -> u64 {
    header_charge(document)
        + document.files.iter().map(Charged::charge).sum::<u64>()
        + document.layers.iter().map(Charged::charge).sum::<u64>()
        + document
            .annotations
            .iter()
            .map(Charged::charge)
            .sum::<u64>()
        + document.labels.iter().map(Charged::charge).sum::<u64>()
}

/// What a whole document holds once it is read: its four lists with
/// every element, and the `extensions` and unknown members of its header.
impl Held for Document {
    fn held_bytes(&self) -> u64 {
        vector(&self.files, Held::held_bytes)
            + vector(&self.layers, Held::held_bytes)
            + vector(&self.annotations, Held::held_bytes)
            + vector(&self.labels, Held::held_bytes)
            + text(&self.format)
            + text(&self.version)
            + header_charge(self)
    }
}
