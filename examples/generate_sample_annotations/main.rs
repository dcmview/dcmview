//! Writes a `dcmview.annotations` document for the images under the given
//! paths, to show what the viewer draws: four classes with colours, two
//! layers, every vector shape type on each image, and file and study
//! labels. See `docs/annotations.md`, "A sample document".
//!
//! ```text
//! cargo run --example generate_sample_annotations -- [--out FILE] PATH...
//! dcmview --annotations FILE PATH...
//! ```
//!
//! Without `--out` the document goes to standard output. It names each
//! file by the key the viewer gives it, so the same paths load it again.

mod document;

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut out = None;
    let mut inputs = Vec::new();
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--out" {
            out = Some(PathBuf::from(
                arguments.next().context("--out needs a file name")?,
            ));
        } else {
            inputs.push(PathBuf::from(argument));
        }
    }
    if inputs.is_empty() {
        bail!("usage: generate_sample_annotations [--out FILE] PATH...");
    }
    let listed = document::describe(&inputs).await?;
    let text = document::document(&listed.files)?;
    match out {
        Some(path) => std::fs::write(&path, text)
            .with_context(|| format!("{} could not be written", path.display()))?,
        None => print!("{text}"),
    }
    eprintln!("annotated {} file(s)", listed.files.len());
    for (copy, first) in &listed.copies {
        eprintln!("left out {copy}: the same bytes as {first}, which it shows the records of");
    }
    Ok(())
}
