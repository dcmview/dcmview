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
