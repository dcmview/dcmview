//! The model's TypeScript and JSON Schema, generated from the same serde
//! attributes as the wire format.
//!
//! Both are committed and checked for drift by this crate's tests:
//!
//! - [`TYPESCRIPT_PATH`], for the viewer's frontend;
//! - [`JSON_SCHEMA_PATH`], for Python and ML consumers of the native format.
//!
//! `cargo run -p dcmview-annotation --example generate_annotation_model`
//! rewrites them; with `--check` it fails when either is stale.

/// Where the generated TypeScript is committed, relative to the repository
/// root.
pub const TYPESCRIPT_PATH: &str = "frontend/src/generated/annotation-types.ts";

/// Where the generated JSON Schema is committed, relative to the repository
/// root.
pub const JSON_SCHEMA_PATH: &str =
    "crates/dcmview-annotation/schema/dcmview.annotations.schema.json";

/// The TypeScript declarations of every wire type of the model.
///
/// The roots are [`crate::Document`], [`crate::OpEnvelope`],
/// [`crate::ApplyResult`] and [`crate::FrameSet`]; every derived type they
/// reach is declared once. `FrameSet` is a root of its own because
/// [`crate::FrameScope`] states its TypeScript by hand
/// (`"all" | FrameSet`), which hides the dependency from ts-rs.
///
/// The layout is the one `examples/generate_api_types.rs` of the root
/// package uses: a two-line `//` header saying the file is generated, by
/// which command, and must not be edited; then one `export type` per Rust
/// type, each preceded by its documentation, sorted by TypeScript name and
/// separated by one blank line. Integers are `number` (every integer on the
/// wire is below 2^53). The text ends with one newline and is the same on
/// every platform and every run. It must type check on its own under the
/// frontend's `tsc --noEmit`.
pub fn typescript() -> String {
    todo!("FND3: render the model's TypeScript")
}

/// The JSON Schema (draft 2020-12) of [`crate::Document`], as schemars
/// derives it, pretty printed with serde_json and ending with one newline.
/// The same on every platform and every run.
pub fn json_schema() -> String {
    todo!("FND3: render the model's JSON Schema")
}
