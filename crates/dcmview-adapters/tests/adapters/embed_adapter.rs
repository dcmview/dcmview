//! EMBED behind the adapter traits, and the registry.

use super::support::{key, loaded, report_json, size, Pool, Session};
use dcmview_adapters::{
    Confidence, Directions, EmbedAdapter, ExportAdapter, ExportContext, ExportError, ImportAdapter,
    Registry,
};
use dcmview_annotation::{Document, FileKey, FrameScope, Geometry};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SINGLE: &str = "sop:1.2.3.1";
const STACK: &str = "sop:1.2.3.2";
const EMPTY: &str = "sop:1.2.3.3";

fn id(n: u32) -> String {
    format!("0199c0de-0000-7000-8000-{n:012x}")
}

fn file(key: &str, path: &str, frames: u32) -> Value {
    json!({
        "key": key, "kind": "dicom", "sop_instance_uid": key.trim_start_matches("sop:"),
        "path": path, "size_bytes": 1000, "rows": 160, "columns": 240, "frames": frames,
        "format": "dicom",
    })
}

fn annotation(n: u32, file: &str, class: &str, frames: Value, geometry: Value) -> Value {
    json!({
        "id": id(n), "file": file, "frames": frames, "layer": "default", "class": class,
        "geometry": geometry, "attributes": {}, "extensions": {},
        "rev": 1, "created_by": "user:test", "created_at": "2026-10-10T08:00:00.000Z",
        "modified_by": "user:test", "modified_at": "2026-10-10T08:00:00.000Z",
    })
}

fn rect(corners: [f64; 4]) -> Value {
    let [x0, y0, x1, y1] = corners;
    json!({ "type": "rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1 })
}

/// A document with what EMBED holds and what it cannot.
fn document() -> Document {
    let mut scored = annotation(3, STACK, "mass", json!("all"), rect([1.0, 2.0, 3.0, 4.0]));
    scored["score"] = json!(0.5);
    scored["attributes"] = json!({ "margin": "spiculated" });
    serde_json::from_value(json!({
        "format": "dcmview.annotations", "version": "1.0",
        "files": [
            file(STACK, "set/stack.dcm", 3),
            file(SINGLE, "set/single.dcm", 1),
            file(EMPTY, "set/empty.dcm", 1),
        ],
        "layers": [{ "id": "default", "name": "Annotations", "kind": "user" }],
        "annotations": [
            annotation(1, SINGLE, "roi", json!("all"), rect([30.0, 20.0, 90.0, 60.0])),
            // Off the pixel grid: written rounded outward.
            annotation(2, STACK, "roi", json!({ "set": [0, 2], "as_written": [2, 0, 0] }),
                rect([20.25, 10.5, 79.5, 50.001])),
            // Every frame, in a file whose other box names some.
            scored,
            annotation(4, SINGLE, "roi", json!("all"),
                json!({ "type": "polygon", "points": [
                    { "x": 1, "y": 1 }, { "x": 9, "y": 1 }, { "x": 5, "y": 9 }] })),
            annotation(5, SINGLE, "roi", json!("all"), json!({ "type": "point", "x": 4, "y": 4 })),
            annotation(6, STACK, "roi", json!("all"), json!({ "type": "point", "x": 5, "y": 5 })),
            // The only record of its file is not a rectangle: no row.
            annotation(7, EMPTY, "roi", json!("all"), json!({ "type": "point", "x": 6, "y": 6 })),
        ],
        "labels": [{
            "id": id(20), "target": { "series": "1.2.3" }, "field": "quality", "value": "good",
            "layer": "default", "rev": 1,
            "created_by": "user:test", "created_at": "2026-10-10T08:00:00.000Z",
            "modified_by": "user:test", "modified_at": "2026-10-10T08:00:00.000Z",
        }],
    }))
    .expect("document")
}

fn paths() -> BTreeMap<FileKey, Vec<String>> {
    BTreeMap::from([
        // Two loaded paths with one key: one row each.
        (
            key(SINGLE),
            vec![
                "/data/set/single.dcm".to_string(),
                "/data/copy/single.dcm".to_string(),
            ],
        ),
    ])
}

#[test]
fn a_document_is_written_as_embed_rows_and_what_embed_cannot_hold_is_reported() {
    let mut out = Vec::new();
    let paths = paths();
    let report = EmbedAdapter
        .export(
            &document(),
            &ExportContext {
                paths: &paths,
                max_bytes: u64::MAX,
            },
            &mut out,
        )
        .expect("export");

    assert_eq!(
        String::from_utf8(out).expect("text"),
        concat!(
            "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n",
            "/data/copy/single.dcm,1,\"[[20,30,60,90]]\",[]\n",
            "/data/set/single.dcm,1,\"[[20,30,60,90]]\",[]\n",
            // No path was given for this key: the document's own.
            "set/stack.dcm,2,\"[[10,20,51,80],[2,1,4,3]]\",\"[[2,0,0],[0,1,2]]\"\n",
        )
    );
    assert_eq!(
        report_json(&report),
        json!({
            "adapter": { "id": "embed", "version": 1 },
            "direction": "export",
            "counts": {
                "files": 3, "annotations_in": 7, "annotations_out": 3,
                "labels_in": 1, "labels_out": 0, "unmatched": 0, "refused": 0,
            },
            "losses": [
                { "code": "type_skipped", "detail": "point", "count": 3, "rows": [] },
                { "code": "type_skipped", "detail": "polygon", "count": 1, "rows": [] },
                { "code": "rounded_outward", "count": 1, "rows": [] },
                { "code": "classes_merged", "count": 2, "rows": [] },
                { "code": "attributes_dropped", "count": 1, "rows": [] },
                { "code": "scores_dropped", "count": 1, "rows": [] },
                { "code": "labels_dropped", "count": 1, "rows": [] },
            ],
            "warnings": [],
        })
    );
}

#[test]
fn an_export_is_held_to_its_byte_bound_and_to_a_whole_document() {
    let paths = paths();
    let document = document();
    let mut whole = Vec::new();
    let context = |max_bytes| ExportContext {
        paths: &paths,
        max_bytes,
    };
    EmbedAdapter
        .export(&document, &context(u64::MAX), &mut whole)
        .expect("export");

    let exact = whole.len() as u64;
    let mut out = Vec::new();
    assert!(EmbedAdapter
        .export(&document, &context(exact), &mut out)
        .is_ok());
    let mut out = Vec::new();
    assert_eq!(
        EmbedAdapter
            .export(&document, &context(exact - 1), &mut out)
            .err(),
        Some(ExportError::TooLarge { limit: exact - 1 })
    );

    // An annotation on a file the document does not describe.
    let mut orphaned = document;
    orphaned.files.remove(0);
    assert_eq!(
        EmbedAdapter
            .export(&orphaned, &context(u64::MAX), &mut Vec::new())
            .err(),
        Some(ExportError::UnknownFile)
    );
}

#[test]
fn an_embed_csv_is_imported_as_rectangles_under_the_keys_the_resolver_gives() {
    let mut keyless = loaded("nokey.dcm", SINGLE, size(160, 240, 1));
    keyless.key = None;
    let pool = Pool(vec![
        loaded("single.dcm", SINGLE, size(160, 240, 1)),
        loaded("stack.dcm", STACK, size(160, 240, 3)),
        keyless,
    ]);
    let session = Session::new(None);
    let csv = concat!(
        "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n",
        "stack.dcm,2,\"[[10,20,50,80],[100,120,160,240]]\",\"[[2,0,0],[]]\"\n",
        "nokey.dcm,2,\"[[1,2,3,4],[5,6,7,8]]\",[]\n",
        "single.dcm,1,\"[[20,30,60,90]]\",[]\n",
    );

    let imported = EmbedAdapter
        .import(&mut csv.as_bytes(), &session.context(&pool))
        .expect("import");

    let records: Vec<(&str, Geometry, FrameScope, &str, &str, &str)> = imported
        .fragment
        .annotations
        .iter()
        .map(|record| {
            (
                record.file.as_str(),
                record.geometry.clone(),
                record.frames.clone(),
                record.layer.as_str(),
                record.class.as_str(),
                record.meta.created_by.as_str(),
            )
        })
        .collect();
    let rectangle = |x0: f64, y0: f64, x1: f64, y1: f64| Geometry::Rect { x0, y0, x1, y1 };
    assert_eq!(
        records,
        vec![
            (
                STACK,
                rectangle(20.0, 10.0, 80.0, 50.0),
                FrameScope::from_written(vec![2, 0, 0]),
                "default",
                "roi",
                "import:test"
            ),
            (
                STACK,
                rectangle(120.0, 100.0, 240.0, 160.0),
                FrameScope::from_written(vec![]),
                "default",
                "roi",
                "import:test"
            ),
            (
                SINGLE,
                rectangle(30.0, 20.0, 90.0, 60.0),
                FrameScope::All,
                "default",
                "roi",
                "import:test"
            ),
        ]
    );
    assert!(imported.fragment.layers.is_empty() && imported.fragment.labels.is_empty());
    let ids: std::collections::HashSet<_> = imported
        .fragment
        .annotations
        .iter()
        .map(|record| record.id)
        .collect();
    assert_eq!(ids.len(), 3, "each box has an id of its own");

    let report = report_json(&imported.report);
    assert_eq!(
        report["counts"],
        json!({
            "files": 2, "annotations_in": 5, "annotations_out": 3,
            "labels_in": 0, "labels_out": 0, "unmatched": 0, "refused": 2,
        })
    );
    assert_eq!(
        report["warnings"],
        json!([
            { "code": "frames_empty", "count": 1, "rows": [{ "row": 2, "roi": 1 }] },
            { "code": "key_unavailable", "count": 1, "rows": [{ "row": 3 }] },
        ])
    );
}

#[test]
fn the_registry_knows_its_adapters_and_which_one_a_file_is_for() {
    let registry = Registry::builtin();
    let infos: Vec<(String, Directions, String)> = registry
        .infos()
        .into_iter()
        .map(|info| (info.id, info.directions, info.extension))
        .collect();
    assert_eq!(
        infos,
        vec![
            ("embed".to_string(), Directions::Both, "csv".to_string()),
            ("native".to_string(), Directions::Both, "json".to_string()),
            (
                "native-jsonl".to_string(),
                Directions::Export,
                "jsonl".to_string()
            ),
        ]
    );
    assert!(registry.exporter("native-jsonl").is_some());
    assert!(registry.importer("native-jsonl").is_none());
    assert!(registry.exporter("coco").is_none());

    type Sniffed = Option<(&'static str, Confidence)>;
    let cases: [(&str, &[u8], Sniffed); 8] = [
        (
            "an EMBED header among other columns",
            b"empi_anon,ROI_coords,side,anon_dicom_path\n1,[],L,/a.dcm\n",
            Some(("embed", Confidence::Certain)),
        ),
        (
            "an EMBED header with a quoted name and no line feed",
            b"\"anon_dicom_path\",ROI_coords",
            Some(("embed", Confidence::Certain)),
        ),
        (
            "a CSV without the columns",
            b"path,boxes\n/a.dcm,[]\n",
            None,
        ),
        (
            "the column names in a later line only",
            b"path,boxes\nanon_dicom_path,ROI_coords\n",
            None,
        ),
        (
            "a native document",
            b"  \n{ \"version\": \"1.0\", \"format\": \"dcmview.annotations\" }",
            Some(("native", Confidence::Certain)),
        ),
        (
            "some other JSON object",
            b"{\"images\": []}",
            Some(("native", Confidence::Possible)),
        ),
        (
            "a JSON array",
            b"[{\"format\": \"dcmview.annotations\"}]",
            None,
        ),
        ("nothing", b"", None),
    ];
    for (what, head, expected) in cases {
        let found = registry
            .sniff(head)
            .map(|(adapter, confidence)| (adapter.info().id, confidence));
        assert_eq!(
            found,
            expected.map(|(id, confidence)| (id.to_string(), confidence)),
            "{what}"
        );
    }

    // Only the start of a file is looked at.
    let mut late = vec![b' '; dcmview_adapters::limits::SNIFF_BYTES];
    late.extend_from_slice(b"{\"format\": \"dcmview.annotations\"}");
    assert!(registry.sniff(&late).is_none());
}
