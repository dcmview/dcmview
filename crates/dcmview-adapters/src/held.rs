//! How much heap the things a native import keeps hold, counted on the
//! things themselves after they are built.
//!
//! What a document costs to hold cannot be told from its text: a value of
//! two bytes can become a map node of several hundred. So the reader
//! ([`crate::native`]) does not predict it. After each file reference,
//! layer, annotation and label is built, and after the members of the
//! header are, it asks here what the thing holds and adds that to a total
//! it keeps within a budget ([`crate::limits::HELD_TEXT_MULTIPLE`]).
//!
//! # What is counted
//!
//! Every allocation a value owns, never less than the allocator gives it:
//!
//! - a string or a vector: its capacity in bytes, and [`ALLOCATION`] for
//!   the allocator's own bookkeeping. One without capacity holds nothing;
//! - a map (`BTreeMap`): its nodes. A node has room for 11 entries, so a
//!   map of up to 11 is one node, whatever it holds: a map of one member
//!   costs as much as one of eleven. A larger map has nodes that hold at
//!   least 5, and is counted as `ceil(n / 5)` leaves and a fifth as many
//!   nodes above them, which have 12 child pointers more;
//! - a JSON value kept as it was read: nothing for a number, `true`,
//!   `false` or `null` beyond the 32 bytes it has in its parent; a string
//!   as a string; an array as a vector of values; an object as a map from
//!   strings to values;
//! - and what each element or entry holds in turn.
//!
//! The figure is at least what the allocator gave out for the value, and
//! within three times that for the shapes that cost the most
//! (`tests/raster_cost/annotations.rs` compares the two for each kind).
//! The bytes a value has inside its owner (a record in a vector of
//! records) are the owner's to count.

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

fn bytes(capacity: usize) -> u64 {
    if capacity == 0 {
        0
    } else {
        capacity as u64 + ALLOCATION
    }
}

/// A string whose capacity cannot be asked for (a key, a layer id, an
/// author, a timestamp): its length, which is its capacity when it was
/// read from text.
fn text(value: &str) -> u64 {
    bytes(value.len())
}

fn optional(value: &Option<String>) -> u64 {
    value.as_ref().map_or(0, |value| bytes(value.capacity()))
}

fn vector<T>(values: &Vec<T>, each: impl Fn(&T) -> u64) -> u64 {
    bytes(values.capacity() * size_of::<T>()) + values.iter().map(each).sum::<u64>()
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
            .map(|(name, value)| bytes(name.capacity()) + value.held_bytes())
            .sum::<u64>()
}

impl Held for Value {
    fn held_bytes(&self) -> u64 {
        match self {
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
            Value::String(text) => bytes(text.capacity()),
            Value::Array(items) => vector(items, Held::held_bytes),
            Value::Object(object) => {
                nodes(object.len(), size_of::<String>() + size_of::<Value>())
                    + object
                        .iter()
                        .map(|(name, value)| bytes(name.capacity()) + value.held_bytes())
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
                .map(|(name, value)| bytes(name.capacity()) + value.held_bytes())
                .sum::<u64>()
    }
}

impl Held for LabelValue {
    fn held_bytes(&self) -> u64 {
        match self {
            LabelValue::Bool(_) | LabelValue::Number(_) => 0,
            LabelValue::Text(text) => bytes(text.capacity()),
            LabelValue::Many(values) => vector(values, |value| bytes(value.capacity())),
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
                                + tiles
                                    .values()
                                    .map(|payload| bytes(payload.0.capacity()))
                                    .sum::<u64>()
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
            + bytes(self.class.capacity())
            + self.geometry.held_bytes()
            + nodes(
                self.attributes.len(),
                size_of::<String>() + size_of::<LabelValue>(),
            )
            + self
                .attributes
                .iter()
                .map(|(name, value)| bytes(name.capacity()) + value.held_bytes())
                .sum::<u64>()
            + self.meta.held_bytes()
            + members(&self.extensions)
            + members(&self.unknown)
    }
}

impl Held for Label {
    fn held_bytes(&self) -> u64 {
        let target = match &self.target {
            LabelTarget::Patient { patient: id }
            | LabelTarget::Study { study: id }
            | LabelTarget::Series { series: id } => bytes(id.capacity()),
            LabelTarget::File { file } | LabelTarget::Frame { frame: file, .. } => {
                text(file.as_str())
            }
            LabelTarget::Folder { folder, root } => {
                bytes(folder.capacity()) + bytes(root.capacity())
            }
        };
        target
            + bytes(self.field.capacity())
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
            + bytes(self.name.capacity())
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
            + bytes(self.path.capacity())
            + optional(&self.digest)
            + members(&self.space.unknown)
            + bytes(self.format.capacity())
            + vector(&self.pixel_digest, optional)
            + self
                .frame_source
                .as_ref()
                .map_or(0, |frames| vector(frames, |_| 0))
            + optional(&self.external_id)
            + self.spacing.as_ref().map_or(0, |spacing| {
                vector(spacing, |entry| {
                    entry
                        .as_ref()
                        .map_or(0, |entry| bytes(entry.source.capacity()))
                })
            })
            + members(&self.unknown)
    }
}

/// What a whole document holds: its four lists with every element, and
/// the `extensions` and unknown members of its header. Its schema, which
/// one unit of text bounds ([`crate::limits::MAX_UNIT_VALUES`]), is not
/// counted.
impl Held for Document {
    fn held_bytes(&self) -> u64 {
        vector(&self.files, Held::held_bytes)
            + vector(&self.layers, Held::held_bytes)
            + vector(&self.annotations, Held::held_bytes)
            + vector(&self.labels, Held::held_bytes)
            + bytes(self.format.capacity())
            + bytes(self.version.capacity())
            + members(&self.extensions)
            + members(&self.unknown)
    }
}
