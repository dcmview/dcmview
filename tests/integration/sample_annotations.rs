//! The sample annotation document (`examples/generate_sample_annotations`):
//! what it writes for the committed fixtures is
//! `tests/fixtures/annotation-documents/sample.json`, and the viewer loads
//! that file through `--annotations` whole.

#[path = "../../examples/generate_sample_annotations/document.rs"]
mod document;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The images the committed document is written for: two patients, one
/// with two images of one study, a multi-frame image and a TIFF.
fn inputs() -> Vec<PathBuf> {
    [
        "golden-masking-patient-a-us-1.dcm",
        "golden-masking-patient-a-us-2.dcm",
        "golden-masking-patient-b-us.dcm",
        "golden-uncompressed-u16-multiframe.dcm",
        "tiff-jpeg/gray-150x133.tif",
    ]
    .iter()
    .map(|name| fixtures().join(name))
    .collect()
}

fn committed() -> PathBuf {
    fixtures().join("annotation-documents/sample.json")
}

/// Regenerate with `cargo run --example generate_sample_annotations --
/// --out tests/fixtures/annotation-documents/sample.json` and the paths of
/// [`inputs`] (`docs/annotations.md`, "A sample document").
#[tokio::test]
async fn the_committed_sample_document_is_what_the_generator_writes() {
    let files = document::describe(&inputs()).await.expect("describe");
    let text = document::document(&files).expect("document");
    let on_disk = std::fs::read_to_string(committed()).expect("committed document");
    assert!(
        text == on_disk,
        "tests/fixtures/annotation-documents/sample.json is stale: regenerate it"
    );
    let parsed: Value = serde_json::from_str(&text).expect("JSON");
    let classes: Vec<&Value> = parsed["schema"]["classes"]
        .as_array()
        .expect("classes")
        .iter()
        .map(|class| &class["color"])
        .collect();
    assert_eq!(classes.len(), 4);
    let mut kinds: Vec<&str> = parsed["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .filter_map(|record| record["geometry"]["type"].as_str())
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(
        kinds,
        ["ellipse", "line", "point", "polygon", "polyline", "rect"]
    );
}

/// The real binary over the same images with `--annotations` and the
/// committed document: every file is found, every record and label is
/// loaded, and the session takes the document's schema.
#[tokio::test]
async fn the_sample_document_loads_through_annotations() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcmview"))
        .arg("--startup-json")
        .arg("--no-browser")
        .arg("--annotations")
        .arg(committed())
        .args(inputs())
        .env_remove("DCMVIEW_TOKEN")
        .env("DCMVIEW_VSCODE_BYPASS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn dcmview");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let started: Value = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let line = stdout
                .next_line()
                .await
                .expect("read stdout")
                .expect("dcmview exited before it started");
            if line.starts_with("{\"type\":\"server_started\"") {
                return serde_json::from_str(&line).expect("startup JSON");
            }
        }
    })
    .await
    .expect("startup event");
    let base = started["base_url"].as_str().expect("base_url").to_string();
    let token = started["token"].as_str().expect("token").to_string();
    let client = reqwest::Client::new();
    let get = |path: &str| {
        client
            .get(format!("{base}{path}"))
            .bearer_auth(&token)
            .timeout(Duration::from_secs(30))
            .send()
    };

    let status: Value = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status: Value = get("/api/annotations/import")
                .await
                .expect("import status")
                .json()
                .await
                .expect("JSON");
            if status["state"] != "loading" {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("import end");
    assert_eq!(status["state"], "ready", "{status}");
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(committed()).expect("document"))
            .expect("JSON");
    let count = |member: &str| document[member].as_array().expect(member).len();
    let report = &status["report"];
    assert_eq!(report["counts"]["files"], count("files"), "{report}");
    assert_eq!(report["counts"]["annotations_out"], count("annotations"));
    assert_eq!(report["counts"]["labels_out"], count("labels"));
    assert_eq!(report["counts"]["unmatched"], 0);
    assert_eq!(report["counts"]["refused"], 0);
    assert_eq!(report["warnings"], serde_json::json!([]), "{report}");
    assert_eq!(report["losses"], serde_json::json!([]), "{report}");

    let exported: Value = get("/api/annotations/export.json")
        .await
        .expect("export")
        .json()
        .await
        .expect("JSON");
    assert_eq!(exported["schema"]["schema_id"], "dcmview-sample");
    assert_eq!(
        exported["annotations"]
            .as_array()
            .expect("annotations")
            .len(),
        count("annotations")
    );
    let _ = child.kill().await;
}
