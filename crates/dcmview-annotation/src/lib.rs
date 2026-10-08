//! The neutral dcmview annotation model.
//!
//! This crate is the one definition of what an annotation is, shared by the
//! viewer, its frontend (through generated TypeScript), adapters, and other
//! repositories that pin it by dcmview release tag. It is a pure model:
//! types, their wire format, and the rules a value must meet. It has no
//! axum, tokio, DICOM or pixel-pipeline dependency and never touches the
//! filesystem, the network or the environment. Every function gives the same
//! result for the same arguments, with one exception: [`new_id`] reads the
//! system clock and the operating system's random number generator.
//!
//! The design is `docs/design/annotation-model.md`; section numbers in the
//! documentation of this crate refer to it.
//!
//! # What is here
//!
//! | Module | Holds |
//! |---|---|
//! | [`key`] | [`FileKey`] and [`KEY_RULES`]; the other validated strings ([`LayerId`], [`Author`], [`Timestamp`]) |
//! | [`mod@file`] | [`FileRef`]: the evidence that identifies a file |
//! | [`geometry`] | Coordinates, [`Geometry`] and its invariants, masks, quantization and clamping |
//! | [`frames`] | [`FrameScope`] |
//! | [`schema`] | [`LabelSchema`]: classes, fields, options |
//! | [`label`] | [`LabelTarget`], [`LabelValue`], [`Label`] |
//! | [`layer`] | [`Layer`], [`LayerPatch`] |
//! | [`record`] | [`Annotation`], [`RecordMeta`], [`new_id`] |
//! | [`document`] | [`Document`], its format and version |
//! | [`op`] | [`Op`], [`OpEnvelope`], [`Patch`], queue keys, [`ApplyResult`] |
//! | [`validate`] | [`Violation`], [`ViolationCode`], [`Invalid`], [`Context`] |
//! | [`limits`] | Every size bound, as a fixed number |
//! | [`generate`] | The generated TypeScript and JSON Schema |
//!
//! # What is not here
//!
//! - **Redaction boxes.** They are not annotations (owner decision,
//!   re-baseline amendment of 2026-10-05): they keep their own store and
//!   endpoint in the viewer and stay outside this model, its operations, the
//!   undo history and every export. Nothing in this crate represents one.
//! - Stores and applying operations to state, HTTP, file hashing and file
//!   discovery, import and export adapters, and rendering.
//!
//! # Rules
//!
//! - Reading checks shape; `validate` checks every invariant (see
//!   [`validate`]). Every write is validated. A record the lenient EMBED CSV
//!   import read stays as it was read until it is edited.
//! - Every floating-point member is written without a fraction when it is a
//!   whole number (`340`, not `340.0`), so Rust and JavaScript writers agree.
//! - Nothing here panics on input: every parser and validator returns an
//!   error. Reading is bounded by the length of the text, which is checked
//!   before it is parsed; it builds every list the text holds, however long.
//!   `validate` then compares each list and string with its bound in
//!   [`limits`] before it visits the items, and its work is linear in the
//!   size of the value plus what it indexes of the schema ([`Context`]).
//! - Within a document major version, members are only added. A member this
//!   version does not know is kept and written back on the types that have an
//!   `unknown` map: the document, the schema and its classes, fields and
//!   options, a file and its `space`, a layer and its `source`, an
//!   annotation and a label. Everywhere else (a geometry, a frame set, a
//!   code, a spacing entry, an operation, its envelope and its patches) it is
//!   ignored when read and so not written back, and a label target with one
//!   is refused.
//! - A member that is optional is left out when it is absent, and `null` is
//!   read as absent (`score`, `derived_from`, a class's `color` and `code`,
//!   a patch's members). The members that are written as `null` are the ones
//!   a type's example shows as `null`: a file's identifiers and digests, a
//!   layer's `color`, a document's `schema`, an operation's `base_rev`,
//!   `before` and `after`.
//! - A member the model names is refused when the text holds it twice. A
//!   repeated key inside a map whose keys are data (`attributes`,
//!   `extensions`, a mask's frames and tiles, the unknown members) is not:
//!   the last one is kept.
//! - The wire shapes are pinned by the fixtures under `tests/fixtures/`.
//!   Extend them when a member is added.

pub mod document;
pub mod file;
pub mod frames;
pub mod generate;
pub mod geometry;
pub mod key;
pub mod label;
pub mod layer;
pub mod limits;
mod model_checks;
mod number;
pub mod op;
pub mod record;
pub mod schema;
pub mod validate;

pub use document::{Document, DocumentError, FORMAT, VERSION, VERSION_MAJOR};
pub use file::{FileKind, FileRef, FrameSpacing, Orientation, Space};
pub use frames::{FrameScope, FrameSet};
pub use geometry::{
    quantize, Clamped, FrameIndex, Geometry, GeometryType, ImageSize, Mask, MaskEncoding, Point,
    Snap, TileCoord, TilePayload,
};
pub use key::{
    Author, AuthorKind, FileKey, InvalidValue, KeyScheme, LayerId, Timestamp, KEY_RULES,
};
pub use label::{Label, LabelTarget, LabelValue, MISSING_ID_PREFIX};
pub use layer::{Layer, LayerKind, LayerPatch, LayerSource};
pub use op::{
    ApplyResult, Current, EnvelopeError, Op, OpEnvelope, Patch, QueueKey, RevEntry, TileChange,
};
pub use record::{new_id, Annotation, RecordMeta};
pub use schema::{
    ClassDef, Code, FieldDef, FieldType, LabelSchema, OptionDef, TargetKind, IMPLICIT_CLASS_ID,
};
pub use validate::{Context, FileSizes, Invalid, Violation, ViolationCode};

// Private dispatch keeps the shared collector and every validation helper
// private while the frozen public methods remain the entry points.
trait Check<C> {
    fn check(&self, context: C) -> Result<(), Invalid>;
}
