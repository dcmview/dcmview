//! What the annotation store may hold while it works, counted at the
//! allocator (`MemoryBackend`, "Cost"; `AnnotationStore`, "Staged rows").

use super::heap;
use dcmview::annotations::{
    AnnotationBackend, AnnotationStore, EmbedRoiAnnotations, MemoryBackend, MemoryConfig,
    DEFAULT_LAYER_ID,
};
use dcmview::server::{annotations::import_embed_rows, FileRegistry};
use dcmview::types::FileEntry;
use dcmview_annotation::{ApplyResult, Author, FileKey, ImageSize, OpEnvelope};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const FILE: &str = "sop:1.2.3.1";

/// A catalog entry for a DICOM file that is never opened.
fn entry(path: std::path::PathBuf, sop_instance_uid: String) -> FileEntry {
    FileEntry {
        format: Default::default(),
        raster: None,
        index: 0,
        size_bytes: 0,
        modified: None,
        path,
        label: "fixture".to_string(),
        patient_id: "TEST".to_string(),
        patient_name: String::new(),
        study_instance_uid: "1.2.3".to_string(),
        study_date: String::new(),
        study_description: String::new(),
        series_instance_uid: "1.2.3.0".to_string(),
        series_number: String::new(),
        series_description: String::new(),
        modality: "OT".to_string(),
        instance_number: String::new(),
        sop_instance_uid,
        sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
        series_metadata: Default::default(),
        has_pixels: true,
        frame_count: 1,
        rows: 16,
        columns: 16,
        bits_allocated: 8,
        pixel_representation: 0,
        samples_per_pixel: 1,
        photometric_interpretation: "MONOCHROME2".to_string(),
        rescale_slope: 1.0,
        rescale_intercept: 0.0,
        transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
        default_window: None,
    }
}

fn id(n: u32) -> String {
    format!("0199c0de-0000-7000-8000-{n:012x}")
}

fn envelope(op_id: u32, op: Value) -> OpEnvelope {
    serde_json::from_value(json!({
        "op_id": id(op_id), "actor": "user:test", "ts": "2026-10-09T08:00:00.000Z", "op": op,
    }))
    .expect("envelope")
}

/// A batch of many small operations on one large record holds the record
/// once more, not once for each operation, whether it is applied or
/// refused at its last operation.
#[test]
fn a_batch_holds_each_record_it_changes_once() {
    // 100,000 points: about 1.6 MB as a record.
    const POINTS: usize = 100_000;
    const RECORD_BYTES: u64 = POINTS as u64 * 16;
    const OPS: u64 = 1_000;
    let backend = MemoryBackend::new(MemoryConfig {
        author: Author::parse("user:test").expect("author"),
        schema: None,
    });
    let files = BTreeMap::from([(
        FileKey::parse(FILE).expect("key"),
        ImageSize {
            columns: 4_096,
            rows: 4_096,
            frames: 1,
        },
    )]);
    let points: Vec<Value> = (0..POINTS)
        .map(|n| json!({ "x": (n % 4_096) as f64, "y": (n / 4_096) as f64 + 0.5 }))
        .collect();
    let polygon = json!({
        "id": id(1), "file": FILE, "frames": "all", "layer": DEFAULT_LAYER_ID, "class": "roi",
        "geometry": { "type": "polygon", "points": points },
        "attributes": {}, "extensions": {},
        "rev": 1, "created_by": "user:test", "created_at": "2026-10-09T08:00:00.000Z",
        "modified_by": "user:test", "modified_at": "2026-10-09T08:00:00.000Z",
    });
    let applied = backend
        .apply(
            vec![envelope(
                1,
                json!({ "type": "create_annotation", "annotation": polygon }),
            )],
            &files,
        )
        .expect("store");
    assert!(
        matches!(applied[0], ApplyResult::Ok { .. }),
        "{:?}",
        applied[0]
    );

    // Each operation changes the record and is based on the one before.
    let touches =
        |from: u64, stale_last: bool| -> Value {
            let ops: Vec<Value> = (0..OPS)
            .map(|n| {
                let base_rev = if stale_last && n == OPS - 1 { 0 } else { from + n };
                json!({
                    "type": "update_annotation", "id": id(1), "file": FILE, "base_rev": base_rev,
                    "before": { "attributes": {} }, "after": { "attributes": {} },
                })
            })
            .collect();
            json!({ "type": "batch", "ops": ops })
        };
    // Room for the record a few times over (the copy a refusal restores,
    // and what validating it allocates), and far below once per operation.
    let allowed = 6 * RECORD_BYTES;
    assert!(allowed < OPS * RECORD_BYTES / 100);

    let refused = envelope(2, touches(1, true));
    let (results, peak) = heap::peak_during(|| backend.apply(vec![refused], &files));
    assert!(
        matches!(results.expect("store")[0], ApplyResult::Conflict { .. }),
        "the last operation is stale"
    );
    assert!(peak <= allowed, "a refused batch held {peak} bytes");
    assert_eq!(backend.revision().expect("revision"), 1);

    let good = envelope(3, touches(1, false));
    let (results, peak) = heap::peak_during(|| backend.apply(vec![good], &files));
    assert!(
        matches!(&results.expect("store")[0], ApplyResult::Ok { revs } if revs.len() == 1 && revs[0].rev == OPS + 1),
        "one record, changed by every operation"
    );
    assert!(peak <= allowed, "an applied batch held {peak} bytes");
}

/// The same holds for a layer: many small operations on one large layer
/// hold the layer once more, applied or refused.
#[test]
fn a_batch_holds_each_layer_it_changes_once() {
    const LAYER_BYTES: u64 = 2 * 1024 * 1024;
    const OPS: u64 = 1_600;
    let backend = MemoryBackend::new(MemoryConfig {
        author: Author::parse("user:test").expect("author"),
        schema: None,
    });
    let files: BTreeMap<FileKey, ImageSize> = BTreeMap::new();
    // A member this version does not know is kept with the layer.
    let layer = json!({ "id": "large", "name": "Large", "kind": "user",
                        "notes": "x".repeat(LAYER_BYTES as usize) });
    let applied = backend
        .apply(
            vec![envelope(
                1,
                json!({ "type": "create_layer", "layer": layer }),
            )],
            &files,
        )
        .expect("store");
    assert!(
        matches!(applied[0], ApplyResult::Ok { .. }),
        "{:?}",
        applied[0]
    );

    let renames = |stale_last: bool| -> Value {
        let ops: Vec<Value> = (0..OPS)
            .map(|n| {
                let base_rev = if stale_last && n == OPS - 1 { 0 } else { 1 + n };
                json!({
                    "type": "update_layer", "id": "large", "base_rev": base_rev,
                    "before": { "name": "Large" }, "after": { "name": format!("Large {n}") },
                })
            })
            .collect();
        json!({ "type": "batch", "ops": ops })
    };
    let allowed = 4 * LAYER_BYTES;
    assert!(allowed < OPS * LAYER_BYTES / 100);

    let refused = envelope(2, renames(true));
    let (results, peak) = heap::peak_during(|| backend.apply(vec![refused], &files));
    assert!(matches!(
        results.expect("store")[0],
        ApplyResult::Conflict { .. }
    ));
    assert!(peak <= allowed, "a refused batch held {peak} bytes");
    assert_eq!(
        backend.snapshot(&[]).expect("snapshot").layers[1].name,
        "Large"
    );

    let good = envelope(3, renames(false));
    let (results, peak) = heap::peak_during(|| backend.apply(vec![good], &files));
    assert!(
        matches!(&results.expect("store")[0], ApplyResult::Ok { revs } if revs.len() == 1 && revs[0].rev == OPS + 1)
    );
    assert!(peak <= allowed, "an applied batch held {peak} bytes");
}

/// And for a label, whose largest part is a text value.
#[test]
fn a_batch_holds_each_label_it_changes_once() {
    const VALUE_BYTES: usize = 60_000;
    const OPS: u64 = 10_000;
    let mut schema = dcmview_annotation::LabelSchema::implicit();
    schema.fields = serde_json::from_value(json!([{
        "id": "note", "name": "Note", "type": "text", "applies_to": ["series"],
    }]))
    .expect("field");
    let backend = MemoryBackend::new(MemoryConfig {
        author: Author::parse("user:test").expect("author"),
        schema: Some(schema),
    });
    let files: BTreeMap<FileKey, ImageSize> = BTreeMap::new();
    let set = |base_rev: Option<u64>, after: Value| {
        json!({ "type": "set_label", "id": id(1), "base_rev": base_rev,
                "target": { "series": "1.2.3" }, "field": "note", "layer": DEFAULT_LAYER_ID,
                "before": null, "after": after })
    };
    let applied = backend
        .apply(
            vec![envelope(1, set(None, json!("x".repeat(VALUE_BYTES))))],
            &files,
        )
        .expect("store");
    assert!(
        matches!(applied[0], ApplyResult::Ok { .. }),
        "{:?}",
        applied[0]
    );

    // Cleared ten thousand times over (a cleared label keeps its record),
    // and the last operation stale: the label is put back from one copy.
    let ops: Vec<Value> = (0..OPS)
        .map(|n| {
            let base_rev = if n == OPS - 1 { 0 } else { 1 + n };
            set(Some(base_rev), Value::Null)
        })
        .collect();
    let refused = envelope(2, json!({ "type": "batch", "ops": ops }));
    let (results, peak) = heap::peak_during(|| backend.apply(vec![refused], &files));
    assert!(matches!(
        results.expect("store")[0],
        ApplyResult::Conflict { .. }
    ));
    // A copy for each operation would be megabytes; the bound is the
    // value twice over and the batch's own bookkeeping.
    assert!(
        peak <= 2 * VALUE_BYTES as u64 + 1024 * 1024,
        "held {peak} bytes"
    );
    let document = backend.export().expect("document");
    assert_eq!(document.labels.len(), 1);
    assert_eq!(document.labels[0].meta.rev, 1);
}

/// What the store keeps of what was deleted is bounded: deleted
/// annotations whole up to `DELETED_BYTES`, and beyond that an id and a
/// revision each, within 2,048 bytes; a deleted layer and a cleared label
/// keep no content at all.
#[test]
fn what_is_kept_of_deleted_things_is_bounded() {
    const REQUESTS: u32 = 300;
    const EACH: usize = 1024 * 1024;
    const RESERVED_ID_BYTES: u64 = 2_048;
    let mut schema = dcmview_annotation::LabelSchema::implicit();
    schema.fields = serde_json::from_value(json!([{
        "id": "note", "name": "Note", "type": "text", "applies_to": ["series"],
    }]))
    .expect("field");
    let backend = MemoryBackend::new(MemoryConfig {
        author: Author::parse("user:test").expect("author"),
        schema: Some(schema),
    });
    let files = BTreeMap::from([(
        FileKey::parse(FILE).expect("key"),
        ImageSize {
            columns: 4_096,
            rows: 4_096,
            frames: 65_536,
        },
    )]);
    // One request: an annotation with a megabyte of points and a long
    // frame list, a layer with a megabyte it does not know, and a label
    // with a long value, each created and then deleted or cleared. It is
    // written as text and read once, so that building it holds little.
    let points = (0..40_000)
        .map(|p| format!(r#"{{"x":{},"y":{}.5}}"#, p % 4_096, p / 4_096))
        .collect::<Vec<_>>()
        .join(",");
    let frames = (0..20_000)
        .map(|frame| frame.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let notes = "x".repeat(EACH);
    let value = "x".repeat(60_000);
    let request = |n: u32| -> OpEnvelope {
        let stamps = r#""rev":1,"created_by":"user:test","created_at":"2026-10-09T08:00:00.000Z","modified_by":"user:test","modified_at":"2026-10-09T08:00:00.000Z""#;
        let record = format!(
            r#"{{"id":"{id}","file":"{FILE}","frames":{{"set":[{frames}]}},"layer":"{DEFAULT_LAYER_ID}","class":"roi","geometry":{{"type":"polygon","points":[{points}]}},"attributes":{{}},"extensions":{{}},{stamps}}}"#,
            id = id(n)
        );
        // The delete needs the record's id and file, not its content.
        let snapshot = format!(
            r#"{{"id":"{id}","file":"{FILE}","frames":"all","layer":"{DEFAULT_LAYER_ID}","class":"roi","geometry":{{"type":"point","x":1,"y":1}},"attributes":{{}},"extensions":{{}},{stamps}}}"#,
            id = id(n)
        );
        let label = |base_rev: &str, after: &str| {
            format!(
                r#"{{"type":"set_label","id":"{id}","base_rev":{base_rev},"target":{{"series":"1.2.{n}"}},"field":"note","layer":"{DEFAULT_LAYER_ID}","before":null,"after":{after}}}"#,
                id = id(n)
            )
        };
        let text = format!(
            r#"{{"op_id":"{op}","actor":"user:test","ts":"2026-10-09T08:00:00.000Z","op":{{"type":"batch","ops":[{{"type":"create_annotation","annotation":{record}}},{{"type":"delete_annotation","id":"{id}","base_rev":1,"snapshot":{snapshot}}},{{"type":"create_layer","layer":{{"id":"layer-{n}","name":"Layer","kind":"user","notes":"{notes}"}}}},{{"type":"delete_layer","id":"layer-{n}","base_rev":1,"snapshot":{{"id":"layer-{n}","name":"Layer","kind":"user"}}}},{create},{clear}]}}}}"#,
            op = id(1_000_000 + n),
            id = id(n),
            create = label("null", &format!("\"{value}\"")),
            clear = label("1", "null"),
        );
        OpEnvelope::from_json_str(&text).expect("envelope")
    };

    let ((), peak) = heap::peak_during(|| {
        for n in 1..=REQUESTS {
            let results = backend.apply(vec![request(n)], &files).expect("store");
            assert!(
                matches!(results[0], ApplyResult::Ok { .. }),
                "{:?}",
                results[0]
            );
        }
    });

    // The bound, a stated number of bytes for each id that stays taken
    // (three a request), and room for the one request being applied.
    let allowed = dcmview::annotations::DELETED_BYTES as u64
        + 3 * u64::from(REQUESTS) * RESERVED_ID_BYTES
        + 16 * 1024 * 1024;
    assert!(
        peak <= allowed,
        "{REQUESTS} requests left {peak} bytes held, more than {allowed}"
    );
    // Far below what the requests carried.
    assert!(allowed < u64::from(REQUESTS) * 2 * EACH as u64 / 4);
}

/// The rows of an `--annotations` CSV are kept as rows until something
/// needs them as records: a ROI costs its coordinates and its frame list,
/// within 256 bytes, not a record.
#[test]
fn imported_rows_are_held_as_rows() {
    const FILES: usize = 20_000;
    const ROIS_PER_FILE: usize = 2;
    const BYTES_PER_ROI: u64 = 256;
    let dir = tempfile::tempdir().expect("temp dir");
    let entries = (0..FILES)
        .map(|n| entry(dir.path().join(format!("{n}.dcm")), format!("1.2.3.{n}")))
        .collect();
    let registry = FileRegistry::from_files(entries);
    let store = AnnotationStore::loading();

    let (report, peak) = heap::peak_during(|| {
        let rows: HashMap<usize, EmbedRoiAnnotations> = (0..FILES)
            .map(|n| {
                (
                    n,
                    EmbedRoiAnnotations {
                        num_roi: ROIS_PER_FILE,
                        roi_coords: vec![[1, 2, 3, 4]; ROIS_PER_FILE],
                        roi_frames: vec![vec![0]; ROIS_PER_FILE],
                    },
                )
            })
            .collect();
        import_embed_rows(&registry, &store, rows).expect("import")
    });

    assert_eq!(report.files_loaded, FILES);
    let allowed = (FILES * ROIS_PER_FILE) as u64 * BYTES_PER_ROI;
    assert!(
        peak <= allowed,
        "{} ROIs held {peak} bytes, more than {BYTES_PER_ROI} each",
        FILES * ROIS_PER_FILE
    );
    assert_eq!(registry.key_stats().files_hashed, 0);
}

/// What reading and writing annotation files may hold: a CSV for a session
/// of 100,000 files with two ROIs each, and documents for a quarter of
/// that, which shows the same proportions for a quarter of the memory.
mod documents {
    use super::{entry, heap};
    use dcmview::annotations::{AnnotationSource, AnnotationStore, NativeSource};
    use dcmview::server::annotations::{
        export_embed_csv, export_native, import_embed_rows, import_native, NativeForm,
    };
    use dcmview::server::FileRegistry;
    use std::io::Write;
    use std::sync::Arc;

    const FILES: usize = 100_000;
    const ROIS: u64 = 2 * FILES as u64;

    /// `RASTER_COST_REPORT=1 ... -- --nocapture` prints what each case held.
    fn report(what: &str, peak: u64, of: u64) {
        if std::env::var_os("RASTER_COST_REPORT").is_some() {
            eprintln!("{what}: held {peak} bytes for {of}");
        }
    }

    fn registry(dir: &std::path::Path, files: usize) -> FileRegistry {
        FileRegistry::from_files(
            (0..files)
                .map(|n| entry(dir.join(format!("{n}.dcm")), format!("1.2.3.{n}")))
                .collect(),
        )
    }

    /// An EMBED CSV with a row of two ROIs for every file, and as many rows
    /// again for files that are not loaded, whose long payload is never
    /// read.
    fn write_csv(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("rois.csv");
        let mut csv = std::io::BufWriter::new(std::fs::File::create(&path).expect("create CSV"));
        writeln!(csv, "anon_dicom_path,num_ROI,ROI_coords,ROI_frames").expect("write");
        let junk = "9,".repeat(200);
        for n in 0..FILES {
            writeln!(
                csv,
                "{},2,\"[[1,2,3,4],[5,6,7,8]]\",\"[[0],[0]]\"",
                dir.join(format!("{n}.dcm")).display()
            )
            .expect("write");
            writeln!(csv, "/elsewhere/{n}.dcm,2,\"[{junk}]\",[]").expect("write");
        }
        csv.flush().expect("flush");
        path
    }

    /// A CSV is read at the cost of the rows it hands on: what the read
    /// holds is within 400 bytes a ROI, the lookup of 100,000 files by
    /// path included, and no file is hashed for it.
    #[test]
    fn a_large_csv_is_read_and_staged_at_the_cost_of_its_rows() {
        const BYTES_PER_ROI: u64 = 400;
        let dir = tempfile::tempdir().expect("temp dir");
        let registry = registry(dir.path(), FILES);
        let source = AnnotationSource::from_path(&write_csv(dir.path())).expect("source");
        let files = registry.files_snapshot();
        let store = AnnotationStore::loading();

        let (report, peak) = heap::peak_during(|| {
            let read = source.read_for_files(&files, &|| true).expect("read");
            assert_eq!(read.report.counts.unmatched, FILES as u64);
            assert_eq!(read.report.counts.annotations_out, ROIS);
            import_embed_rows(&registry, &store, read.rows).expect("stage")
        });

        assert_eq!(report.files_loaded, FILES);
        self::report("CSV import, ROIs", peak, ROIS);
        assert!(
            peak <= ROIS * BYTES_PER_ROI,
            "{ROIS} ROIs held {peak} bytes, more than {BYTES_PER_ROI} each"
        );
        assert_eq!(registry.key_stats().files_hashed, 0);
    }

    /// The exports hold the session's records once more and their text
    /// once, and a native import holds what it read once beside the
    /// records it leaves: each within a fixed multiple of the text.
    #[test]
    fn documents_of_a_large_session_are_written_and_read_at_a_multiple_of_their_text() {
        const FILES: usize = 25_000;
        const ROIS: u64 = 2 * FILES as u64;
        let dir = tempfile::tempdir().expect("temp dir");
        let registry = registry(dir.path(), FILES);
        let store = AnnotationStore::loading();
        let rows = (0..FILES)
            .map(|n| {
                (
                    n,
                    dcmview::annotations::EmbedRoiAnnotations {
                        num_roi: 2,
                        roi_coords: vec![[1, 2, 3, 4], [5, 6, 7, 8]],
                        roi_frames: vec![vec![0], vec![0]],
                    },
                )
            })
            .collect();
        import_embed_rows(&registry, &store, rows).expect("stage");
        let runtime = heap::CountedRuntime::new();

        // The first export makes the staged rows records, which the store
        // then holds for the session. The exports after it are measured.
        let first = runtime
            .peak_during(export_native(&registry, &store, NativeForm::JsonLines))
            .0
            .expect("the first export");
        assert_eq!(first.report.counts.annotations_out, ROIS);
        let text = first.bytes.len() as u64;
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, &first.bytes).expect("write document");
        drop(first);

        for form in [NativeForm::JsonLines, NativeForm::Json] {
            let (export, peak) = runtime.peak_during(export_native(&registry, &store, form));
            let export = export.expect("export");
            report(&format!("{form:?} export, bytes of text"), peak, text);
            assert!(
                peak <= 5 * text,
                "{form:?}: an export of {text} bytes held {peak}"
            );
            drop(export);
        }
        let (csv, peak) = runtime.peak_during(export_embed_csv(&registry, &store));
        let csv = csv.expect("CSV");
        assert_eq!(csv.lines().count(), FILES + 1);
        report("CSV export, bytes of text", peak, csv.len() as u64);
        assert!(
            peak <= 7 * csv.len() as u64,
            "a CSV of {} bytes held {peak}",
            csv.len()
        );
        drop(csv);

        // The document, loaded into a session of its own.
        let source = NativeSource::from_path(&path).expect("source");
        let second = AnnotationStore::loading();
        let (report, peak) = runtime.peak_during(import_native(
            &registry,
            &second,
            &source,
            Arc::new(|| true),
        ));
        let report = report.expect("import");
        assert_eq!(report.counts.annotations_out, ROIS);
        assert_eq!(report.counts.files, FILES as u64);
        self::report("native import, bytes of text", peak, text);
        assert!(
            peak <= 5 * text,
            "an import of {text} bytes held {peak}, the records it left included"
        );
        assert_eq!(registry.key_stats().files_hashed, 0);
    }

    /// Texts made to cost the most for their length, as documents and as
    /// JSON Lines. `per_record` gives one record of a kind: `(list, text)`.
    fn hostile(text: usize) -> Vec<(String, String)> {
        let head = r#""format":"dcmview.annotations","version":"1.0""#;
        let file = r#"{"key":"sop:1.2.3","kind":"dicom","sop_instance_uid":"1.2.3","path":"a.dcm","size_bytes":1,"rows":4096,"columns":4096,"frames":4,"format":"dicom"}"#;
        let meta = r#""rev":1,"created_by":"user:a","created_at":"2026-10-10T08:00:00.000Z","modified_by":"user:a","modified_at":"2026-10-10T08:00:00.000Z""#;
        let rect = r#"{"type":"rect","x0":1,"y0":1,"x1":2,"y1":2}"#;
        let annotation = |n: usize, geometry: &str, attributes: &str, more: &str| {
            format!(
                r#"{{"id":"0199c0de-0000-7000-8000-{n:012x}","file":"sop:1.2.3","frames":"all","layer":"default","class":"roi","geometry":{geometry},"attributes":{attributes},"extensions":{{}},{meta}{more}}}"#
            )
        };
        let label = |n: usize, value: &str| {
            format!(
                r#"{{"id":"0199c0de-0000-7000-9000-{n:012x}","target":{{"file":"sop:1.2.3"}},"field":"f","value":{value},"layer":"default",{meta}}}"#
            )
        };
        let filled = |open: &str, item: &str, close: &str, length: usize| {
            let count = (length.saturating_sub(open.len() + close.len())) / item.len();
            format!("{open}{}{close}", item.repeat(count))
        };
        let repeated = |item: &str, count: usize| {
            filled(
                &format!("[{item}"),
                &format!(",{item}"),
                "]",
                (item.len() + 1) * count + 1,
            )
        };
        let nested = |depth: usize| format!("{}0{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
        type Make<'a> = Box<dyn Fn(usize) -> (&'static str, String) + 'a>;
        let kinds: Vec<(&str, Make)> = vec![
            (
                "minimal files",
                Box::new(|n| {
                    (
                        "file",
                        format!(
                            r#"{{"key":"sop:1.{n}","kind":"dicom","sop_instance_uid":"1.{n}","path":"a","size_bytes":1,"rows":1,"columns":1,"frames":1,"format":"dicom"}}"#
                        ),
                    )
                }),
            ),
            (
                "files with digests of every frame",
                Box::new(|n| {
                    (
                        "file",
                        format!(
                            r#"{{"key":"sop:1.{n}","kind":"dicom","sop_instance_uid":"1.{n}","path":"a","size_bytes":1,"rows":1,"columns":1,"frames":1,"format":"dicom","pixel_digest":{}}}"#,
                            repeated("null", 2000)
                        ),
                    )
                }),
            ),
            (
                "files with spacings of every frame",
                Box::new(|n| {
                    (
                        "file",
                        format!(
                            r#"{{"key":"sop:1.{n}","kind":"dicom","sop_instance_uid":"1.{n}","path":"a","size_bytes":1,"rows":1,"columns":1,"frames":1,"format":"dicom","spacing":{}}}"#,
                            repeated(r#"{"row_mm":1,"col_mm":1,"source":""}"#, 500)
                        ),
                    )
                }),
            ),
            (
                "files with unknown one-member objects",
                Box::new(|n| {
                    (
                        "file",
                        format!(
                            r#"{{"key":"sop:1.{n}","kind":"dicom","sop_instance_uid":"1.{n}","path":"a","size_bytes":1,"rows":1,"columns":1,"frames":1,"format":"dicom","x":{}}}"#,
                            repeated(r#"{"a":0}"#, 200)
                        ),
                    )
                }),
            ),
            (
                "minimal annotations",
                Box::new(|n| ("annotation", annotation(n, rect, "{}", ""))),
            ),
            (
                "annotations whose attributes are arrays of empty strings",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(
                            n,
                            rect,
                            &format!(r#"{{"a":{}}}"#, repeated(r#""""#, 1000)),
                            "",
                        ),
                    )
                }),
            ),
            (
                "annotations with many attributes",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(
                            n,
                            rect,
                            &format!(
                                "{{{}}}",
                                (0..200)
                                    .map(|k| format!(r#""{k}":0"#))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            ),
                            "",
                        ),
                    )
                }),
            ),
            (
                "annotations that keep nested one-member objects",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(
                            n,
                            rect,
                            "{}",
                            &format!(r#","x":{}"#, repeated(&nested(40), 20)),
                        ),
                    )
                }),
            ),
            (
                "annotations that keep arrays of small numbers",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(n, rect, "{}", &format!(r#","x":{}"#, repeated("0", 1000))),
                    )
                }),
            ),
            (
                "annotations that keep arrays of empty objects and strings",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(
                            n,
                            rect,
                            "{}",
                            &format!(
                                r#","x":{},"y":{}"#,
                                repeated("{}", 500),
                                repeated(r#""""#, 500)
                            ),
                        ),
                    )
                }),
            ),
            (
                "annotations with a member again and again",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(n, rect, "{}", &r#","x":{"a":0}"#.repeat(100)),
                    )
                }),
            ),
            (
                "annotations with long escaped strings",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(n, rect, "{}", &format!(r#","x":"{}""#, r"A".repeat(2000))),
                    )
                }),
            ),
            (
                "polygons of 1,000 points",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(
                            n,
                            &format!(
                                r#"{{"type":"polygon","points":{}}}"#,
                                repeated(r#"{"x":0,"y":0}"#, 1000)
                            ),
                            "{}",
                            "",
                        ),
                    )
                }),
            ),
            (
                "geometry with members it ignores",
                Box::new(|n| {
                    (
                        "annotation",
                        annotation(
                            n,
                            &format!(
                                r#"{{"type":"rect","x0":1,"y0":1,"x1":2,"y1":2,"more":{}}}"#,
                                repeated(r#"{"a":0}"#, 300)
                            ),
                            "{}",
                            "",
                        ),
                    )
                }),
            ),
            ("minimal labels", Box::new(|n| ("label", label(n, "true")))),
            (
                "labels whose value is an array of empty strings",
                Box::new(|n| ("label", label(n, &repeated(r#""""#, 1000)))),
            ),
        ];
        let mut cases = Vec::new();
        for (what, make) in &kinds {
            let mut records = Vec::new();
            let mut length = 0;
            while length < text {
                let (kind, record) = make(records.len());
                length += record.len();
                records.push((kind, record));
            }
            let of = |kind: &str| -> Vec<&str> {
                records
                    .iter()
                    .filter(|(k, _)| *k == kind)
                    .map(|(_, record)| record.as_str())
                    .collect()
            };
            let mut files = of("file");
            if files.is_empty() {
                files.push(file);
            }
            cases.push((
                format!("{what}, JSON"),
                format!(
                    r#"{{{head},"files":[{}],"layers":[],"annotations":[{}],"labels":[{}]}}"#,
                    files.join(","),
                    of("annotation").join(","),
                    of("label").join(",")
                ),
            ));
            let mut lines = format!("{{{head}}}\n");
            for (kind, records) in [
                ("file", files),
                ("annotation", of("annotation")),
                ("label", of("label")),
            ] {
                for record in records {
                    lines.push_str(&format!("{{\"{kind}\":{record}}}\n"));
                }
            }
            lines.push_str(&format!(
                "{{\"end\":{{\"files\":{},\"layers\":0,\"annotations\":{},\"labels\":{}}}}}\n",
                of("file").len().max(1),
                of("annotation").len(),
                of("label").len()
            ));
            cases.push((format!("{what}, JSON Lines"), lines));
        }
        cases
    }

    /// One unit of as many values, and of as many objects, as a unit may
    /// hold, of the shapes that cost the most to build: in a header member
    /// and in a record, in a text of a megabyte or two.
    fn largest_units() -> Vec<(String, String)> {
        let head = r#""format":"dcmview.annotations","version":"1.0""#;
        let repeated =
            |item: &str, count: usize| format!("[{}{item}]", format!("{item},").repeat(count - 1));
        let record = |more: &str| {
            format!(
                r#"{{"id":"0199c0de-0000-7000-8000-000000000001","file":"sop:1.2.3","frames":"all","layer":"default","class":"roi","geometry":{{"type":"rect","x0":1,"y0":1,"x1":2,"y1":2}},"attributes":{{}},"extensions":{{}},"rev":1,"created_by":"user:a","created_at":"2026-10-10T08:00:00.000Z","modified_by":"user:a","modified_at":"2026-10-10T08:00:00.000Z","x":{more}}}"#
            )
        };
        let mut cases = Vec::new();
        for (what, member) in [
            ("one-member objects", repeated(r#"{"a":0}"#, 131_000)),
            (
                "nested one-member objects",
                repeated(
                    &format!("{}0{}", r#"{"a":"#.repeat(60), "}".repeat(60)),
                    2_180,
                ),
            ),
            ("small numbers", repeated("0", 1_040_000)),
            ("empty strings", repeated(r#""""#, 1_040_000)),
            (
                "empty objects and numbers",
                format!(
                    r#"[{},{}]"#,
                    repeated("{}", 131_000),
                    repeated("0", 900_000)
                ),
            ),
            (
                "one-member objects and numbers",
                format!(
                    r#"[{},{}]"#,
                    repeated(r#"{"a":0}"#, 131_000),
                    repeated("0", 650_000)
                ),
            ),
        ] {
            cases.push((
                format!("a header member of {what}"),
                format!(r#"{{{head},"x":{member}}}"#),
            ));
            cases.push((
                format!("a record that keeps {what}, JSON"),
                format!(
                    r#"{{{head},"files":[],"layers":[],"annotations":[{}]}}"#,
                    record(&member)
                ),
            ));
            cases.push((
                format!("a record that keeps {what}, JSON Lines"),
                format!("{{{head}}}\n{{\"annotation\":{}}}\n{{\"end\":{{\"files\":0,\"layers\":0,\"annotations\":1,\"labels\":0}}}}\n", record(&member)),
            ));
        }
        cases
    }

    /// For any text, reading a document holds at most `MULTIPLE` times the
    /// text, `BASE` and `TRANSIENT` (`dcmview_adapters::native`, "Cost"):
    /// what the import keeps is counted on it as it is built and held to
    /// a budget of the text read, and what building one record or the
    /// header holds is bounded by how many values it may have. Texts of
    /// records made to cost the most for their length are held to the
    /// multiple, at a size where the constants are small beside it; and
    /// the units that cost the most to build, in a short text, to the
    /// transient.
    #[test]
    fn a_hostile_document_holds_a_bounded_multiple_of_its_text() {
        use dcmview_adapters::limits::{HELD_BASE_BYTES, HELD_TEXT_MULTIPLE, UNIT_TRANSIENT_BYTES};
        use dcmview_adapters::{ImportAdapter, ImportContext, Limits, NativeJson};

        // The text itself, in a buffer that grows by doubling while it is
        // read (three times), and what is kept, by its budget.
        const MULTIPLE: u64 = 3 + HELD_TEXT_MULTIPLE;
        const TEXT: usize = 24 * 1024 * 1024;

        struct Any;
        impl dcmview_adapters::FileResolver for Any {
            fn resolve(&self, file: &dcmview_annotation::FileRef) -> dcmview_adapters::Resolution {
                dcmview_adapters::Resolution::Matched(
                    dcmview_adapters::Resolved {
                        slot: 0,
                        key: Some(file.key.clone()),
                        size: dcmview_annotation::ImageSize {
                            columns: file.columns,
                            rows: file.rows,
                            frames: file.frames,
                        },
                        size_bytes: file.size_bytes,
                    },
                    dcmview_adapters::Matcher::Key,
                )
            }
        }
        let author = dcmview_annotation::Author::parse("import:native").expect("author");
        let layer = dcmview_annotation::LayerId::parse("default").expect("layer");
        let stamp = dcmview_annotation::Timestamp::parse("2026-10-10T08:00:00.000Z").expect("time");
        let held = |text: &str| {
            let context = ImportContext {
                resolver: &Any,
                schema: Option::None,
                layer: &layer,
                author: &author,
                now: &stamp,
                keep_going: &|| true,
                limits: Limits::DEFAULT,
            };
            heap::peak_during(|| {
                NativeJson
                    .import(&mut text.as_bytes(), &context)
                    .map(|imported| imported.report.counts.annotations_out)
            })
        };
        let said = std::env::var_os("RASTER_COST_REPORT").is_some();
        for (what, text) in hostile(TEXT) {
            let (result, peak) = held(&text);
            if said {
                eprintln!(
                    "{what}: {:.2} times {} bytes, {result:?}",
                    peak as f64 / text.len() as f64,
                    text.len()
                );
            }
            assert!(
                peak <= MULTIPLE * text.len() as u64 + HELD_BASE_BYTES,
                "{what}: {} bytes of text held {peak}",
                text.len()
            );
        }
        for (what, text) in largest_units() {
            let (result, peak) = held(&text);
            if said {
                eprintln!(
                    "{what}: {} MiB for {} bytes, {result:?}",
                    peak / (1024 * 1024),
                    text.len()
                );
            }
            assert!(
                peak <= UNIT_TRANSIENT_BYTES + HELD_BASE_BYTES,
                "{what}: {} bytes of text held {peak}",
                text.len()
            );
        }
    }

    /// What a document is counted as holding is at least what the
    /// allocator gave out for it, and no more than three times that, for
    /// each kind of thing a document keeps and the shapes of it that cost
    /// the most.
    #[test]
    fn what_a_document_is_counted_as_holding_is_what_it_holds() {
        use dcmview_adapters::held::Held;
        for (what, text) in hostile(2 * 1024 * 1024) {
            if !what.ends_with(", JSON") {
                continue;
            }
            let before = heap::held();
            let document = dcmview_annotation::Document::from_json_str(&text);
            let actual = (heap::held() - before).max(0) as u64;
            let Ok(document) = document else {
                panic!("{what}: not a document");
            };
            let counted = document.held_bytes();
            assert!(
                actual <= counted && counted <= 3 * actual,
                "{what}: counted {counted} for {actual} held"
            );
        }
    }

    /// One row cannot make the reader hold more than a few rows' worth,
    /// whatever its fields claim.
    #[test]
    fn a_hostile_row_holds_a_bounded_multiple_of_one_row() {
        use dcmview_adapters::embed::read_rows;
        use dcmview_adapters::limits::MAX_CSV_ROW_BYTES;
        use dcmview_adapters::{FileResolver, Resolved};

        struct One;
        impl FileResolver for One {
            fn by_path(&self, _: &str, out: &mut Vec<Resolved>) {
                out.push(Resolved {
                    slot: 0,
                    key: None,
                    size: dcmview_annotation::ImageSize {
                        columns: 16,
                        rows: 16,
                        frames: 1,
                    },
                    size_bytes: 0,
                });
            }
        }
        let row = MAX_CSV_ROW_BYTES as usize - 64;
        let filled = |open: &str, item: &str, close: &str| {
            let count = (row - open.len() - close.len()) / item.len();
            format!("{open}{}{close}", item.repeat(count))
        };
        let cases = [
            (
                "millions of empty boxes",
                filled("\"[[]", ",[]", "]\""),
                "[]".to_string(),
            ),
            (
                "one box of millions of numbers",
                filled("\"[[1", ",1", "]]\""),
                "[]".to_string(),
            ),
            (
                "millions of boxes",
                filled("\"[[1,2,3,4]", ",[1,2,3,4]", "]\""),
                "[]".to_string(),
            ),
            (
                "millions of frames for one box",
                "\"[[1,2,3,4]]\"".to_string(),
                filled("\"[[0", ",0", "]]\""),
            ),
            (
                "millions of frame lists",
                "\"[[1,2,3,4]]\"".to_string(),
                filled("\"[[]", ",[]", "]\""),
            ),
        ];
        for (what, coords, frames) in cases {
            let csv = format!("anon_dicom_path,ROI_coords,ROI_frames\na.dcm,{coords},{frames}\n");
            let (result, peak) =
                heap::peak_during(|| read_rows(&mut csv.as_bytes(), &One, &|| true));
            assert!(result.is_err(), "{what}");
            report(what, peak, csv.len() as u64);
            assert!(
                peak <= 3 * MAX_CSV_ROW_BYTES,
                "{what}: one row of {} bytes held {peak}",
                csv.len()
            );
        }
    }
}
