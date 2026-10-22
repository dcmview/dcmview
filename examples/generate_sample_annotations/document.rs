//! What the sample document holds, for the images the viewer lists under
//! some paths. Shared by the `generate_sample_annotations` example and the
//! test that keeps its committed output
//! (`tests/fixtures/annotation-documents/sample.json`) current.

use anyhow::{bail, Context, Result};
use dcmview::loader::{self, DiscoverOptions, DiscoveryEvent};
use dcmview::server::FileRegistry;
use dcmview::types::FileEntry;
use dcmview_annotation::{Document, FileKey};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// Who the sample records say made them, and when.
const AUTHOR: &str = "user:sample";
const STAMP: &str = "2026-10-01T09:00:00.000Z";

/// The second layer, which has a colour of its own.
const SECOND_LAYER: &str = "reader-2";

/// One image file as the document names it.
pub struct Described {
    pub file: FileEntry,
    /// The key the viewer gives the file (`FileRegistry::ensure_key`).
    pub key: FileKey,
    /// The file's path below the folder the inputs share, with `/`.
    pub path: String,
}

/// The image files the viewer lists under `inputs`, as it would load them
/// (a recursive walk, every format), sorted by path, each with the key the
/// viewer settles for it. A file without pixels, or with no rows, columns
/// or frames, is left out.
pub async fn describe(inputs: &[PathBuf]) -> Result<Vec<Described>> {
    let (events, mut received) = mpsc::channel(64);
    let scan = loader::discover_progressive(
        inputs,
        DiscoverOptions {
            recursive: true,
            filters: Vec::new(),
            formats: Default::default(),
        },
        events,
        loader::DiscoveryCancellation::new(),
    );
    let collect = async {
        let mut files = Vec::new();
        while let Some(event) = received.recv().await {
            if let DiscoveryEvent::Selected { file, .. } = event {
                files.push(*file);
            }
        }
        files
    };
    let (scanned, mut files) = tokio::join!(scan, collect);
    scanned.context("the inputs could not be scanned")?;
    files.retain(|file| {
        file.has_pixels && file.rows > 0 && file.columns > 0 && file.frame_count > 0
    });
    if files.is_empty() {
        bail!("no image file was found under the inputs");
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for (index, file) in files.iter_mut().enumerate() {
        file.index = index;
    }
    let base = common_folder(&files);
    let registry = FileRegistry::from_files(files.clone());
    let mut described = Vec::with_capacity(files.len());
    for file in files {
        let key = registry
            .ensure_key(file.index)
            .await
            .map_err(|error| anyhow::anyhow!("{}: no key: {error:?}", file.path.display()))?;
        let path = file
            .path
            .strip_prefix(&base)
            .unwrap_or(&file.path)
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        described.push(Described { file, key, path });
    }
    Ok(described)
}

/// The deepest folder that holds every file.
fn common_folder(files: &[FileEntry]) -> PathBuf {
    let mut base: Option<PathBuf> = None;
    for file in files {
        let parent = file.path.parent().unwrap_or(Path::new(""));
        base = Some(match base {
            None => parent.to_path_buf(),
            Some(base) => base
                .components()
                .zip(parent.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    base.unwrap_or_default()
}

/// The sample document for `files`: four classes with colours, two layers
/// (the second with its own colour), on every file one shape of each
/// vector geometry type (point, line, polyline, polygon, rectangle and
/// an ellipse with an angle) spread over the classes and layers, a
/// quality label on every file, a note on the first, and a density label
/// on every study a DICOM file names. Shapes are placed as fractions of
/// each image, so they fit any size. Ids and stamps are fixed, so the
/// same files give the same text.
///
/// The document is checked with `Document::validate` (the model's rules,
/// which its JSON Schema is generated from) before it is returned.
pub fn document(files: &[Described]) -> Result<String> {
    let mut next = 0_u64;
    let mut id = || {
        next += 1;
        format!("0199d3e0-0000-7000-8000-{next:012x}")
    };
    let mut references = Vec::new();
    let mut annotations = Vec::new();
    let mut labels = Vec::new();
    let mut studies = Vec::new();
    for (n, described) in files.iter().enumerate() {
        let file = &described.file;
        let key = described.key.as_str();
        references.push(reference(described));
        let (w, h) = (f64::from(file.columns), f64::from(file.rows));
        let r = w.min(h);
        let at = |x: f64, y: f64| json!({ "x": round(x * w), "y": round(y * h) });
        let first_frame = if file.frame_count > 1 {
            json!({ "set": [0] })
        } else {
            json!("all")
        };
        let shapes = [
            (
                "mass",
                "default",
                json!("all"),
                json!({
                "type": "rect", "x0": round(0.10 * w), "y0": round(0.10 * h),
                "x1": round(0.40 * w), "y1": round(0.35 * h) }),
            ),
            (
                "mass",
                "default",
                json!("all"),
                json!({
                "type": "polygon",
                "points": [at(0.55, 0.10), at(0.90, 0.20), at(0.80, 0.45), at(0.60, 0.40)] }),
            ),
            (
                "mass",
                SECOND_LAYER,
                json!("all"),
                json!({
                "type": "ellipse", "cx": round(0.30 * w), "cy": round(0.65 * h),
                "rx": round(0.15 * r), "ry": round(0.08 * r), "angle": 30 }),
            ),
            (
                "calcification",
                "default",
                json!("all"),
                json!({
                "type": "point", "x": round(0.70 * w), "y": round(0.68 * h) }),
            ),
            (
                "calcification",
                "default",
                json!("all"),
                json!({
                "type": "point", "x": round(0.76 * w), "y": round(0.62 * h) }),
            ),
            (
                "calcification",
                SECOND_LAYER,
                json!("all"),
                json!({
                "type": "rect", "x0": round(0.64 * w), "y0": round(0.55 * h),
                "x1": round(0.84 * w), "y1": round(0.75 * h) }),
            ),
            (
                "distortion",
                SECOND_LAYER,
                first_frame,
                json!({
                "type": "line", "points": [at(0.10, 0.90), at(0.45, 0.80)] }),
            ),
            (
                "distortion",
                "default",
                json!("all"),
                json!({
                "type": "polyline",
                "points": [at(0.55, 0.92), at(0.65, 0.82), at(0.80, 0.90), at(0.92, 0.80)] }),
            ),
            (
                "roi",
                "default",
                json!("all"),
                json!({
                "type": "rect", "x0": round(0.45 * w), "y0": round(0.45 * h),
                "x1": round(0.55 * w), "y1": round(0.55 * h) }),
            ),
        ];
        for (class, layer, frames, geometry) in shapes {
            annotations.push(json!({
                "id": id(), "file": key, "frames": frames, "layer": layer, "class": class,
                "geometry": geometry, "attributes": {}, "extensions": {},
                "rev": 1, "created_by": AUTHOR, "created_at": STAMP,
                "modified_by": AUTHOR, "modified_at": STAMP,
            }));
        }
        let quality = if n % 2 == 0 { "good" } else { "limited" };
        labels.push(label(
            id(),
            json!({ "file": key }),
            "quality",
            json!(quality),
        ));
        if n == 0 {
            labels.push(label(
                id(),
                json!({ "file": key }),
                "note",
                json!("Sample annotations: every shape type in four classes."),
            ));
        }
        if !file.format.is_raster()
            && !file.study_instance_uid.is_empty()
            && !studies.contains(&file.study_instance_uid)
        {
            studies.push(file.study_instance_uid.clone());
        }
    }
    for (n, study) in studies.iter().enumerate() {
        let density = ["b", "c", "a", "d"][n % 4];
        labels.push(label(
            id(),
            json!({ "study": study }),
            "density",
            json!(density),
        ));
    }
    let document = json!({
        "format": "dcmview.annotations", "version": "1.0",
        "schema": schema(),
        "files": references,
        "layers": [
            { "id": "default", "name": "Annotations", "kind": "user",
              "source": { "author": AUTHOR }, "rev": 1 },
            { "id": SECOND_LAYER, "name": "Second reader", "kind": "user",
              "color": "#17BEBB", "source": { "author": AUTHOR }, "rev": 1 },
        ],
        "annotations": annotations,
        "labels": labels,
    });
    let document = Document::from_json_str(&document.to_string())
        .map_err(|error| anyhow::anyhow!("the sample document does not read: {error}"))?;
    document
        .validate()
        .map_err(|invalid| anyhow::anyhow!("the sample document is not valid: {invalid:?}"))?;
    let mut text = serde_json::to_string_pretty(&document)?;
    text.push('\n');
    Ok(text)
}

/// Four classes, each with its own colour, and the label fields the
/// sample's labels use.
fn schema() -> Value {
    json!({
        "schema_id": "dcmview-sample", "schema_version": 1,
        "classes": [
            { "id": "mass", "name": "Mass", "color": "#E4572E",
              "geometry": ["rect", "polygon", "ellipse"] },
            { "id": "calcification", "name": "Calcification", "color": "#2E86AB",
              "geometry": ["point", "rect"] },
            { "id": "distortion", "name": "Architectural distortion", "color": "#F2A541",
              "geometry": ["line", "polyline"] },
            { "id": "roi", "name": "ROI", "color": "#8E44AD",
              "geometry": ["point", "line", "polyline", "polygon", "rect", "ellipse", "mask"] },
        ],
        "fields": [
            { "id": "quality", "name": "Image quality", "type": "category", "ordered": true,
              "options": [{ "id": "limited", "name": "Limited" }, { "id": "good", "name": "Good" }],
              "applies_to": ["file"] },
            { "id": "density", "name": "Breast density", "type": "category", "ordered": true,
              "options": [
                  { "id": "a", "name": "A" }, { "id": "b", "name": "B" },
                  { "id": "c", "name": "C" }, { "id": "d", "name": "D" }],
              "applies_to": ["study"] },
            { "id": "note", "name": "Note", "type": "text", "max_length": 200,
              "applies_to": ["file", "study"] },
        ],
    })
}

fn reference(described: &Described) -> Value {
    let file = &described.file;
    let present = |value: &str| (!value.is_empty()).then(|| value.to_string());
    let key = described.key.as_str();
    json!({
        "key": key,
        "kind": if file.format.is_raster() { "raster" } else { "dicom" },
        "format": file.format.as_str(),
        "sop_instance_uid": present(&file.sop_instance_uid),
        "sop_class_uid": present(&file.sop_class_uid),
        "study_instance_uid": present(&file.study_instance_uid),
        "series_instance_uid": present(&file.series_instance_uid),
        "patient_id": present(&file.patient_id),
        "path": described.path,
        "size_bytes": file.size_bytes,
        "rows": file.rows,
        "columns": file.columns,
        "frames": file.frame_count,
        "digest": key.starts_with("b3:").then_some(key),
    })
}

fn label(id: String, target: Value, field: &str, value: Value) -> Value {
    json!({
        "id": id, "target": target, "field": field, "layer": "default", "value": value,
        "extensions": {}, "rev": 1, "created_by": AUTHOR, "created_at": STAMP,
        "modified_by": AUTHOR, "modified_at": STAMP,
    })
}

/// Two decimals: short text, and well inside the model's precision.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
