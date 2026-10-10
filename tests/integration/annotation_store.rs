//! The in-memory annotation store through `AnnotationBackend`: the rules of
//! the operations no endpoint of a session without a label schema reaches
//! (labels), and the ones a view of rectangles does not show (layers,
//! masks, snapshots, the document).

use dcmview::annotations::{
    AnnotationBackend, MemoryBackend, MemoryConfig, DEFAULT_LAYER_ID, DEFAULT_LAYER_NAME,
    REMEMBERED_OPS, REMEMBERED_REVS,
};
use dcmview_annotation::{
    ApplyResult, Author, Current, FileKey, Geometry, ImageSize, LabelSchema, OpEnvelope,
    ViolationCode, FORMAT, VERSION,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const FIRST: &str = "sop:1.2.3.1";
const SECOND: &str = "sop:1.2.3.2";

/// A UUIDv7 that is the same in every run.
fn id(n: u32) -> String {
    format!("0199c0de-0000-7000-8000-{n:012x}")
}

fn key(text: &str) -> FileKey {
    FileKey::parse(text).expect("file key")
}

/// A store whose schema has one label field, and the two files of 100 rows
/// by 120 columns and four frames its operations are judged against.
struct Store {
    backend: MemoryBackend,
    files: BTreeMap<FileKey, ImageSize>,
    next_op: u32,
}

impl Store {
    fn new() -> Self {
        let mut schema = LabelSchema::implicit();
        schema.fields = serde_json::from_value(json!([{
            "id": "quality", "name": "Quality", "type": "category",
            "options": [{ "id": "good", "name": "Good" }, { "id": "bad", "name": "Bad" }],
            "applies_to": ["file", "series"],
        }]))
        .expect("field");
        let size = ImageSize {
            columns: 120,
            rows: 100,
            frames: 4,
        };
        Self {
            backend: MemoryBackend::new(MemoryConfig {
                author: Author::parse("user:reader").expect("author"),
                schema: Some(schema),
            }),
            files: BTreeMap::from([(key(FIRST), size), (key(SECOND), size)]),
            next_op: 1_000_000,
        }
    }

    fn envelope(&mut self, op: Value) -> OpEnvelope {
        self.next_op += 1;
        serde_json::from_value(json!({
            "op_id": id(self.next_op), "actor": "user:test",
            "ts": "2026-10-09T08:00:00.000Z", "op": op,
        }))
        .expect("envelope")
    }

    fn apply(&mut self, op: Value) -> ApplyResult {
        let envelope = self.envelope(op);
        self.backend
            .apply(vec![envelope], &self.files)
            .expect("store")
            .pop()
            .expect("one result")
    }

    /// Applies `op` and returns the revisions it left.
    fn applied(&mut self, op: Value) -> Vec<(String, u64)> {
        match self.apply(op.clone()) {
            ApplyResult::Ok { revs } => revs.into_iter().map(|rev| (rev.id, rev.rev)).collect(),
            other => panic!("{op} was refused: {other:?}"),
        }
    }

    /// Applies `op` and returns the first rule it broke.
    fn refused(&mut self, op: Value) -> ViolationCode {
        match self.apply(op.clone()) {
            ApplyResult::Invalid { violations } if !violations.is_empty() => violations[0].code,
            other => panic!("{op} was not refused as invalid: {other:?}"),
        }
    }

    /// Applies `op` and returns what it should have been based on.
    fn conflict(&mut self, op: Value) -> Current {
        match self.apply(op.clone()) {
            ApplyResult::Conflict { current } => current,
            other => panic!("{op} was not a conflict: {other:?}"),
        }
    }

    fn revision(&self) -> u64 {
        self.backend.revision().expect("revision")
    }
}

fn annotation(record: u32, file: &str, layer: &str, geometry: Value, frames: Value) -> Value {
    json!({
        "id": id(record), "file": file, "frames": frames, "layer": layer, "class": "roi",
        "geometry": geometry, "attributes": {}, "extensions": {},
        "rev": 1, "created_by": "user:test", "created_at": "2026-10-09T08:00:00.000Z",
        "modified_by": "user:test", "modified_at": "2026-10-09T08:00:00.000Z",
    })
}

fn rect(record: u32, file: &str, layer: &str) -> Value {
    annotation(
        record,
        file,
        layer,
        json!({ "type": "rect", "x0": 10, "y0": 20, "x1": 30, "y1": 40 }),
        json!("all"),
    )
}

fn create(record: Value) -> Value {
    json!({ "type": "create_annotation", "annotation": record })
}

fn delete(record: u32, base_rev: u64) -> Value {
    json!({ "type": "delete_annotation", "id": id(record), "base_rev": base_rev,
            "snapshot": rect(record, FIRST, DEFAULT_LAYER_ID) })
}

fn label(record: u32, base_rev: Option<u64>, target: Value, layer: &str, after: Value) -> Value {
    json!({
        "type": "set_label", "id": id(record), "base_rev": base_rev, "target": target,
        "field": "quality", "layer": layer, "before": null, "after": after,
    })
}

/// The envelopes of one call are applied in the order given, each as its
/// own transaction, and a refused one stops nothing after it.
#[test]
fn envelopes_of_one_call_apply_in_order_and_a_refusal_stops_nothing() {
    let mut store = Store::new();
    let moved = |base_rev: u64| {
        json!({
            "type": "update_annotation", "id": id(1), "file": FIRST, "base_rev": base_rev,
            "before": { "class": "roi" }, "after": { "class": "roi" },
        })
    };
    let envelopes = vec![
        store.envelope(create(rect(1, FIRST, DEFAULT_LAYER_ID))),
        store.envelope(moved(1)),
        store.envelope(moved(1)),
        store.envelope(delete(1, 2)),
    ];

    let results = store.backend.apply(envelopes, &store.files).expect("store");

    let revs = |result: &ApplyResult| match result {
        ApplyResult::Ok { revs } => revs.iter().map(|rev| rev.rev).collect::<Vec<_>>(),
        other => panic!("refused: {other:?}"),
    };
    assert_eq!(results.len(), 4);
    assert_eq!(revs(&results[0]), [1]);
    assert_eq!(revs(&results[1]), [2]);
    assert!(
        matches!(&results[2], ApplyResult::Conflict { current: Current::Annotation { record } } if record.meta.rev == 2),
        "{:?}",
        results[2]
    );
    assert_eq!(revs(&results[3]), [3]);
    assert_eq!(store.revision(), 3);
}

/// A snapshot and the exported document hold the records that are not
/// deleted, in the order they were created, and name the revision they
/// were read at.
#[test]
fn a_snapshot_and_the_document_hold_live_records_in_creation_order() {
    let mut store = Store::new();
    let empty = store.backend.snapshot(&[]).expect("snapshot");
    assert_eq!(empty.revision, 0);
    assert_eq!(empty.layers.len(), 1);
    assert_eq!(empty.layers[0].id.as_str(), DEFAULT_LAYER_ID);
    assert_eq!(empty.layers[0].name, DEFAULT_LAYER_NAME);
    assert_eq!(empty.layers[0].rev, 1);

    store.applied(create(rect(1, FIRST, DEFAULT_LAYER_ID)));
    store.applied(create(rect(2, SECOND, DEFAULT_LAYER_ID)));
    store.applied(create(rect(3, FIRST, DEFAULT_LAYER_ID)));
    store.applied(create(rect(4, FIRST, DEFAULT_LAYER_ID)));
    store.applied(delete(4, 1));
    // Deleted and restored, record 1 is where it was.
    store.applied(delete(1, 1));
    store.applied(
        json!({ "type": "restore_annotation", "id": id(1), "base_rev": 2,
        "snapshot": rect(1, FIRST, DEFAULT_LAYER_ID) }),
    );
    store.applied(label(
        10,
        None,
        json!({ "file": FIRST }),
        DEFAULT_LAYER_ID,
        json!("good"),
    ));
    store.applied(label(
        11,
        None,
        json!({ "series": "1.2.3" }),
        DEFAULT_LAYER_ID,
        json!("bad"),
    ));

    let ids = |records: &[dcmview_annotation::Annotation]| {
        records
            .iter()
            .map(|record| record.id.to_string())
            .collect::<Vec<_>>()
    };
    let first = store.backend.snapshot(&[key(FIRST)]).expect("snapshot");
    assert_eq!(ids(&first.annotations), [id(1), id(3)]);
    assert_eq!(first.annotations[0].meta.rev, 3);
    assert_eq!(first.annotations[0].meta.created_by.as_str(), "user:reader");
    assert_eq!(
        first.labels.len(),
        1,
        "the label on the file, not the series"
    );
    assert_eq!(first.revision, store.revision());
    assert_eq!(first.revision, 9);
    let both = store
        .backend
        .snapshot(&[key(SECOND), key(FIRST), key(SECOND)])
        .expect("snapshot");
    assert_eq!(ids(&both.annotations), [id(1), id(2), id(3)]);

    let document = store.backend.export().expect("document");
    assert_eq!(
        (document.format.as_str(), document.version.as_str()),
        (FORMAT, VERSION)
    );
    assert_eq!(ids(&document.annotations), [id(1), id(2), id(3)]);
    assert_eq!(document.labels.len(), 2);
    assert_eq!(document.layers.len(), 1);
    assert!(document.files.is_empty());
    assert!(document
        .schema
        .is_some_and(|schema| schema.fields.len() == 1));

    // An operation that names a record under another file's key does not
    // know the record it is changing.
    let current = store.conflict(json!({
        "type": "update_annotation", "id": id(1), "file": SECOND, "base_rev": 3,
        "before": { "class": "roi" }, "after": { "class": "roi" },
    }));
    assert!(matches!(current, Current::Annotation { record } if record.file == key(FIRST)));
}

/// There is one label for a target, field, layer and author; it keeps its
/// id and revision when its value is cleared, so a stale or repeated
/// operation is refused instead of applied twice.
#[test]
fn a_label_is_one_record_per_target_and_keeps_its_revision_when_cleared() {
    let mut store = Store::new();
    let target = || json!({ "file": FIRST });
    let set = |base_rev: Option<u64>, after: Value| {
        label(10, base_rev, target(), DEFAULT_LAYER_ID, after)
    };
    let labels = |store: &Store| {
        store
            .backend
            .snapshot(&[key(FIRST)])
            .expect("snapshot")
            .labels
            .into_iter()
            .map(|label| (label.id.to_string(), label.meta.rev))
            .collect::<Vec<_>>()
    };

    assert_eq!(store.applied(set(None, json!("good"))), [(id(10), 1)]);
    assert_eq!(
        store.refused(label(11, None, target(), DEFAULT_LAYER_ID, json!("bad"))),
        ViolationCode::DuplicateLabel
    );
    assert_eq!(
        store.refused(set(None, json!("bad"))),
        ViolationCode::DuplicateId
    );
    // The same field on another frame of the file is another label.
    store.applied(label(
        12,
        None,
        json!({ "series": "1.2.3" }),
        DEFAULT_LAYER_ID,
        json!("good"),
    ));

    assert_eq!(store.applied(set(Some(1), json!("bad"))), [(id(10), 2)]);
    assert!(matches!(
        store.conflict(set(Some(1), json!("good"))),
        Current::Label { record } if record.meta.rev == 2
    ));

    // Cleared, the label leaves snapshots and keeps its revision.
    assert_eq!(store.applied(set(Some(2), Value::Null)), [(id(10), 3)]);
    assert_eq!(labels(&store), []);
    assert!(matches!(
        store.conflict(set(Some(2), json!("good"))),
        Current::Deleted { rev: 3, .. }
    ));
    assert_eq!(
        store.refused(label(13, None, target(), DEFAULT_LAYER_ID, json!("good"))),
        ViolationCode::DuplicateLabel,
        "a cleared label still holds its place"
    );
    assert_eq!(store.applied(set(Some(3), json!("good"))), [(id(10), 4)]);
    assert_eq!(labels(&store), [(id(10), 4)]);

    let table = [
        (
            "a new label without a value",
            label(
                14,
                None,
                json!({ "file": SECOND }),
                DEFAULT_LAYER_ID,
                Value::Null,
            ),
            ViolationCode::EmptyPatch,
        ),
        (
            "a label in a layer that does not exist",
            label(
                14,
                None,
                json!({ "file": SECOND }),
                "nowhere",
                json!("good"),
            ),
            ViolationCode::UnknownLayer,
        ),
        (
            "a value that is not an option of the field",
            label(
                14,
                None,
                json!({ "file": SECOND }),
                DEFAULT_LAYER_ID,
                json!("fine"),
            ),
            ViolationCode::UnknownOption,
        ),
    ];
    for (case, op, code) in table {
        assert_eq!(store.refused(op), code, "{case}");
    }
    assert!(matches!(
        store.conflict(label(
            15,
            Some(1),
            target(),
            DEFAULT_LAYER_ID,
            json!("good")
        )),
        Current::Missing { .. }
    ));
}

/// Layers have revisions like records, an id is never used twice, and a
/// layer is deleted only once it is empty, which a batch can arrange.
#[test]
fn a_layer_is_deleted_only_when_empty_and_its_id_is_not_reused() {
    let mut store = Store::new();
    let layer = || json!({ "id": "second", "name": "Second", "kind": "user", "rev": 9 });
    let rename = |base_rev: u64, name: &str| {
        json!({
            "type": "update_layer", "id": "second", "base_rev": base_rev,
            "before": { "name": "Second" }, "after": { "name": name },
        })
    };
    let delete_layer = |layer_id: &str, base_rev: u64| {
        let mut snapshot = layer();
        snapshot["id"] = json!(layer_id);
        json!({ "type": "delete_layer", "id": layer_id, "base_rev": base_rev, "snapshot": snapshot })
    };

    assert_eq!(
        store.applied(json!({ "type": "create_layer", "layer": layer() })),
        [("second".to_string(), 1)],
        "a layer starts at revision 1 whatever the payload said"
    );
    assert_eq!(
        store.refused(json!({ "type": "create_layer", "layer": layer() })),
        ViolationCode::DuplicateId
    );
    assert_eq!(
        store.applied(rename(1, "Reader two")),
        [("second".to_string(), 2)]
    );
    assert!(matches!(
        store.conflict(rename(1, "Reader three")),
        Current::Layer { record } if record.rev == 2 && record.name == "Reader two"
    ));
    assert_eq!(store.refused(rename(2, "")), ViolationCode::BadLayer);

    store.applied(create(rect(1, FIRST, "second")));
    assert_eq!(
        store.refused(create(rect(2, FIRST, "nowhere"))),
        ViolationCode::UnknownLayer
    );
    assert_eq!(
        store.refused(delete_layer("second", 2)),
        ViolationCode::BadLayer,
        "the layer holds a record"
    );
    assert_eq!(
        store.refused(delete_layer(DEFAULT_LAYER_ID, 1)),
        ViolationCode::BadLayer,
        "the default layer stays"
    );

    // Its record deleted in the same batch, the layer goes with it.
    let revs = store.applied(json!({ "type": "batch", "ops": [
        delete(1, 1),
        delete_layer("second", 2),
    ] }));
    assert_eq!(revs, [(id(1), 2), ("second".to_string(), 3)]);
    let layers = store.backend.snapshot(&[]).expect("snapshot").layers;
    assert_eq!(layers.len(), 1);

    assert!(matches!(
        store.conflict(rename(3, "Back")),
        Current::Deleted { rev: 3, .. }
    ));
    assert_eq!(
        store.refused(json!({ "type": "create_layer", "layer": layer() })),
        ViolationCode::DuplicateId
    );
    assert_eq!(
        store.refused(create(rect(3, FIRST, "second"))),
        ViolationCode::UnknownLayer
    );
    // Its layer gone, the deleted record cannot come back.
    assert_eq!(
        store.refused(
            json!({ "type": "restore_annotation", "id": id(1), "base_rev": 2,
            "snapshot": rect(1, FIRST, "second") })
        ),
        ViolationCode::UnknownLayer
    );
}

/// A tile operation changes the tiles of one frame, and the frames the
/// mask's annotation names are always the frames that hold tiles.
#[test]
fn mask_tiles_change_one_frame_and_the_annotations_frames_follow() {
    let mut store = Store::new();
    let mask = annotation(
        1,
        FIRST,
        DEFAULT_LAYER_ID,
        json!({ "type": "mask", "encoding": "tiles-v1", "tile": 64, "depth": 1,
                "frames": { "0": { "0,0": "AAAA" } } }),
        json!({ "set": [0] }),
    );
    let tiles = |record: u32, file: &str, base_rev: u64, frame: u32, tiles: Value| {
        json!({ "type": "mask_tiles", "id": id(record), "file": file, "base_rev": base_rev,
                "frame": frame, "tiles": tiles })
    };
    let paint = || json!([{ "tx": 1, "ty": 0, "before": null, "after": "BBBB" }]);
    let erase =
        |tx: u32, payload: &str| json!([{ "tx": tx, "ty": 0, "before": payload, "after": null }]);
    let frames = |store: &Store| {
        let snapshot = store.backend.snapshot(&[key(FIRST)]).expect("snapshot");
        serde_json::to_value(&snapshot.annotations[0]).expect("record")["frames"].clone()
    };
    store.applied(create(mask));
    store.applied(create(rect(2, FIRST, DEFAULT_LAYER_ID)));

    assert_eq!(store.applied(tiles(1, FIRST, 1, 2, paint())), [(id(1), 2)]);
    assert_eq!(frames(&store), json!({ "set": [0, 2] }));
    // The last tile of frame 0 erased, the frame is no longer the mask's.
    assert_eq!(
        store.applied(tiles(1, FIRST, 2, 0, erase(0, "AAAA"))),
        [(id(1), 3)]
    );
    assert_eq!(frames(&store), json!({ "set": [2] }));
    let record = store.backend.snapshot(&[key(FIRST)]).expect("snapshot");
    assert_eq!(
        serde_json::to_value(&record.annotations[0]).expect("record")["geometry"]["frames"],
        json!({ "2": { "1,0": "BBBB" } })
    );

    // A mask may not be left without a tile; the refusal changes nothing.
    assert_eq!(
        store.refused(tiles(1, FIRST, 3, 2, erase(1, "BBBB"))),
        ViolationCode::MaskEmpty
    );
    assert_eq!(frames(&store), json!({ "set": [2] }));
    assert_eq!(
        store.refused(tiles(2, FIRST, 1, 0, paint())),
        ViolationCode::GeometryNotAllowed,
        "record 2 is a rectangle"
    );
    assert!(matches!(
        store.conflict(tiles(1, SECOND, 3, 2, paint())),
        Current::Annotation { .. }
    ));
    assert!(matches!(
        store.conflict(tiles(1, FIRST, 2, 2, paint())),
        Current::Annotation { record } if record.meta.rev == 3
    ));
}

/// A cleared label does not hold its layer, so the layer can be deleted;
/// the label cannot then get a value back in a layer that no longer exists.
#[test]
fn a_cleared_label_cannot_regain_its_value_in_a_deleted_layer() {
    let mut store = Store::new();
    store.applied(json!({ "type": "create_layer",
        "layer": { "id": "second", "name": "Second", "kind": "user" } }));
    let in_second = |base_rev: Option<u64>, after: Value| {
        label(20, base_rev, json!({ "file": SECOND }), "second", after)
    };
    store.applied(in_second(None, json!("good")));
    store.applied(in_second(Some(1), Value::Null));
    store.applied(
        json!({ "type": "delete_layer", "id": "second", "base_rev": 1,
        "snapshot": { "id": "second", "name": "Second", "kind": "user" } }),
    );

    assert_eq!(
        store.refused(in_second(Some(2), json!("bad"))),
        ViolationCode::UnknownLayer
    );
    // Clearing it again is not a write into the layer.
    assert_eq!(
        store.applied(in_second(Some(2), Value::Null)),
        [(id(20), 3)]
    );
    let document = store.backend.export().expect("document");
    assert!(document.labels.is_empty() && document.layers.len() == 1);
}

/// What a strict write stores is on the model's grid, and a refused
/// envelope leaves nothing behind: not a record, not a place in a file's
/// list, not a revision.
#[test]
fn a_stored_geometry_is_quantized_and_a_refusal_leaves_no_trace() {
    let mut store = Store::new();
    let off_grid = annotation(
        1,
        FIRST,
        DEFAULT_LAYER_ID,
        json!({ "type": "rect", "x0": 10.00049, "y0": 20.0004, "x1": 30.0006, "y1": 40 }),
        json!("all"),
    );
    store.applied(create(off_grid));
    let stored = store.backend.snapshot(&[key(FIRST)]).expect("snapshot");
    assert_eq!(
        stored.annotations[0].geometry,
        Geometry::Rect {
            x0: 10.0,
            y0: 20.0,
            x1: 30.001,
            y1: 40.0
        }
    );
    let exported = store.backend.export().expect("document");
    assert_eq!(
        exported.annotations[0].geometry,
        stored.annotations[0].geometry
    );

    // The batch creates a record on a file that has none, then fails.
    let refused = store.refused(json!({ "type": "batch", "ops": [
        create(rect(2, SECOND, DEFAULT_LAYER_ID)),
        create(rect(1, SECOND, DEFAULT_LAYER_ID)),
    ] }));
    assert_eq!(refused, ViolationCode::DuplicateId);
    assert_eq!(store.revision(), 1);
    let second = |store: &Store| {
        store
            .backend
            .snapshot(&[key(SECOND)])
            .expect("snapshot")
            .annotations
            .into_iter()
            .map(|record| record.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(second(&store), Vec::<String>::new());
    store.applied(create(rect(3, SECOND, DEFAULT_LAYER_ID)));
    assert_eq!(second(&store), [id(3)]);
    store.applied(create(rect(2, SECOND, DEFAULT_LAYER_ID)));
    assert_eq!(second(&store), [id(3), id(2)]);
}

/// A result lists every record and layer once, also when a layer's id is
/// the text of a record's; and a session holds at most 4,096 layers.
#[test]
fn a_result_keeps_records_and_layers_apart_and_layers_are_bounded() {
    let mut store = Store::new();
    let layer = |layer_id: &str| json!({ "type": "create_layer", "layer": { "id": layer_id, "name": "Layer", "kind": "user" } });
    let revs = store.applied(json!({ "type": "batch", "ops": [
        layer(&id(1)),
        create(rect(1, FIRST, &id(1))),
    ] }));
    assert_eq!(revs, [(id(1), 1), (id(1), 1)]);

    // The default layer and the one above make two; 4,094 more fill the
    // session, in one envelope.
    let fill: Vec<Value> = (0..4_094).map(|n| layer(&format!("layer-{n}"))).collect();
    assert_eq!(
        store.applied(json!({ "type": "batch", "ops": fill })).len(),
        4_094
    );
    assert_eq!(
        store.refused(layer("one-too-many")),
        ViolationCode::TooManyItems
    );
    // A deleted layer makes room for another.
    store.applied(
        json!({ "type": "delete_layer", "id": "layer-0", "base_rev": 1,
        "snapshot": { "id": "layer-0", "name": "Layer", "kind": "user" } }),
    );
    store.applied(layer("one-more"));
    assert_eq!(
        store.backend.snapshot(&[]).expect("snapshot").layers.len(),
        4_096
    );
}

/// The store remembers the results of recent envelopes, bounded by their
/// number and by what they hold, and an envelope it has forgotten is
/// judged again: refused, never applied a second time.
#[test]
fn remembered_results_are_bounded_and_a_forgotten_envelope_is_judged_again() {
    let apply = |store: &Store, envelope: &OpEnvelope| {
        store
            .backend
            .apply(vec![envelope.clone()], &store.files)
            .expect("store")
            .pop()
            .expect("one result")
    };
    let forgotten = |result: ApplyResult| matches!(result, ApplyResult::Invalid { ref violations } if violations[0].code == ViolationCode::DuplicateId);

    // By number: one more envelope than the store remembers.
    let mut store = Store::new();
    let touch = |base_rev: u64| {
        json!({
            "type": "update_annotation", "id": id(1), "file": FIRST, "base_rev": base_rev,
            "before": { "class": "roi" }, "after": { "class": "roi" },
        })
    };
    let first = store.envelope(create(rect(1, FIRST, DEFAULT_LAYER_ID)));
    let second = store.envelope(touch(1));
    let rest: Vec<OpEnvelope> = (2..REMEMBERED_OPS as u64)
        .map(|base_rev| store.envelope(touch(base_rev)))
        .collect();
    let created = apply(&store, &first);
    let touched = apply(&store, &second);
    for result in store.backend.apply(rest, &store.files).expect("store") {
        assert!(matches!(result, ApplyResult::Ok { .. }), "{result:?}");
    }
    // Exactly as many as are remembered: the oldest is still answered.
    assert_eq!(store.revision(), REMEMBERED_OPS as u64);
    assert_eq!(apply(&store, &first), created);
    let last = store.envelope(touch(REMEMBERED_OPS as u64));
    assert!(matches!(apply(&store, &last), ApplyResult::Ok { .. }));
    // One more, and the oldest is an envelope the store has not seen: the
    // create is refused as a duplicate, not applied again.
    assert!(forgotten(apply(&store, &first)));
    assert_eq!(apply(&store, &second), touched, "the next oldest stays");
    assert_eq!(store.revision(), REMEMBERED_OPS as u64 + 1);

    // By what they hold: batches whose results list 10,000 revisions each.
    let mut store = Store::new();
    let batches = REMEMBERED_REVS / 10_000 + 1;
    let envelopes: Vec<OpEnvelope> = (0..batches)
        .map(|number| {
            let ops: Vec<Value> = (0..10_000)
                .map(|n| {
                    label(
                        (number * 10_000 + n) as u32,
                        None,
                        json!({ "series": format!("{number}.{n}") }),
                        DEFAULT_LAYER_ID,
                        json!("good"),
                    )
                })
                .collect();
            store.envelope(json!({ "type": "batch", "ops": ops }))
        })
        .collect();
    let results: Vec<ApplyResult> = envelopes
        .iter()
        .map(|envelope| apply(&store, envelope))
        .collect();
    assert!(results
        .iter()
        .all(|result| matches!(result, ApplyResult::Ok { revs } if revs.len() == 10_000)));
    assert!(
        forgotten(apply(&store, &envelopes[0])),
        "the oldest batch no longer fits and is judged again"
    );
    assert_eq!(apply(&store, &envelopes[1]), results[1]);
    assert_eq!(store.revision(), batches as u64);
}
