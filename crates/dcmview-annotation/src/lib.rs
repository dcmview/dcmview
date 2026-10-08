//! The neutral dcmview annotation model.
//!
//! This crate is the one definition of what an annotation is, shared by the
//! viewer, its frontend (through generated TypeScript), adapters, and other
//! repositories that pin it by dcmview release tag. It is a pure model:
//! types, their wire format, and the rules a value must meet. It has no
//! axum, tokio, DICOM or pixel-pipeline dependency and never touches the
//! filesystem or the network.
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
//!   error, and refuses input past the bounds in [`limits`] before doing
//!   work proportional to it.
//! - Within a document major version, members are only added; unknown
//!   members are kept and written back.
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
