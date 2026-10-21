//! The committed TypeScript and JSON Schema.

use dcmview_annotation::generate::{json_schema, typescript, JSON_SCHEMA_PATH, TYPESCRIPT_PATH};
use std::path::Path;

/// The frontend and the Python and ML consumers read the committed files,
/// so a change to a wire type must show up in them, in the same commit.
#[test]
fn the_committed_typescript_and_json_schema_match_the_model() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (relative, rendered) in [
        (TYPESCRIPT_PATH, typescript()),
        (JSON_SCHEMA_PATH, json_schema()),
    ] {
        let committed = std::fs::read_to_string(root.join(relative))
            .unwrap_or_else(|error| panic!("{relative}: {error}"));
        assert!(
            committed.replace("\r\n", "\n") == rendered,
            "{relative} is stale; regenerate it with \
             `cargo run -p dcmview-annotation --example generate_annotation_model`"
        );
    }

    let schema: serde_json::Value = serde_json::from_str(&json_schema()).expect("schema is JSON");
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["title"], "Document");
}
