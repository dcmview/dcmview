//! Writes the model's generated TypeScript and JSON Schema to the paths in
//! `dcmview_annotation::generate`.
//!
//! `cargo run -p dcmview-annotation --example generate_annotation_model`
//! rewrites both files; with `--check` it exits non-zero when a committed
//! file differs from the model.

use dcmview_annotation::generate::{json_schema, typescript, JSON_SCHEMA_PATH, TYPESCRIPT_PATH};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut stale = false;

    for (relative, rendered) in [
        (TYPESCRIPT_PATH, typescript()),
        (JSON_SCHEMA_PATH, json_schema()),
    ] {
        let path = root.join(relative);
        if check {
            let current = std::fs::read_to_string(&path).unwrap_or_default();
            if current.replace("\r\n", "\n") != rendered {
                eprintln!(
                    "{relative} is stale; regenerate it with \
                     `cargo run -p dcmview-annotation --example generate_annotation_model`"
                );
                stale = true;
            }
        } else {
            let directory = path.parent().expect("generated file has a directory");
            std::fs::create_dir_all(directory).expect("create the generated file's directory");
            std::fs::write(&path, rendered).expect("write generated model file");
        }
    }

    if stale {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
