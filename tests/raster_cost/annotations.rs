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
