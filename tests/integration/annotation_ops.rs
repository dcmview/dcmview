//! The annotation store through its doors: `POST /api/annotations/ops`, and
//! the EMBED endpoints as a view of the same records
//! (`docs/design/annotation-model.md` 7, 9; `docs/design/seams.md` 9).

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::annotations::{AnnotationStore, EmbedRoiAnnotations};
use dcmview::masking::Masker;
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::FileEntry;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

const OPS: &str = "/api/annotations/ops";

/// A UUIDv7 that is the same in every run.
fn id(n: u32) -> String {
    format!("0199c0de-0000-7000-8000-{n:012x}")
}

/// An entry of 100 rows by 120 columns with four frames and a UID of its
/// own. The file need not exist: a UID no other file has is a key at once.
fn entry(directory: &Path, name: &str, uid: &str) -> FileEntry {
    let mut entry = support::file_entry(directory.join(name), "1.2.840.10008.1.2.4.50", 4);
    entry.rows = 100;
    entry.columns = 120;
    entry.sop_instance_uid = uid.to_string();
    entry
}

fn serve(files: Vec<FileEntry>) -> TestServer {
    TestServer::new(server::router(support::app_state(files)))
}

fn rect(record: u32, file: &str, [x0, y0, x1, y1]: [f64; 4]) -> Value {
    json!({
        "id": id(record), "file": file, "frames": "all", "layer": "default", "class": "roi",
        "geometry": { "type": "rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1 },
        "attributes": {}, "extensions": {},
        // What a client says here is advisory; the store writes its own.
        "rev": 7, "created_by": "user:someone-else", "created_at": "2001-01-01T00:00:00.000Z",
        "modified_by": "user:someone-else", "modified_at": "2001-01-01T00:00:00.000Z",
    })
}

fn create(record: u32, file: &str, corners: [f64; 4]) -> Value {
    json!({ "type": "create_annotation", "annotation": rect(record, file, corners) })
}

fn move_to(record: u32, file: &str, base_rev: u64, corners: [f64; 4]) -> Value {
    let geometry = |[x0, y0, x1, y1]: [f64; 4]| json!({ "geometry": { "type": "rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1 } });
    json!({
        "type": "update_annotation", "id": id(record), "file": file, "base_rev": base_rev,
        "before": geometry([0.0, 0.0, 1.0, 1.0]), "after": geometry(corners),
    })
}

fn envelope(op_id: u32, op: Value) -> Value {
    json!({ "op_id": id(op_id), "actor": "user:test", "ts": "2026-10-09T08:00:00.000Z", "op": op })
}

/// Each call is a new envelope: operation ids count up from 1,000,000.
struct Client {
    server: TestServer,
    next_op: u32,
}

impl Client {
    fn new(server: TestServer) -> Self {
        Self {
            server,
            next_op: 1_000_000,
        }
    }

    async fn send(&mut self, op: Value) -> (StatusCode, Value) {
        self.next_op += 1;
        let response = self
            .server
            .post(OPS)
            .json(&envelope(self.next_op, op))
            .await;
        (response.status_code(), response.json())
    }

    /// Sends `op` and expects it applied; returns its `revs` as pairs.
    async fn applied(&mut self, op: Value) -> Vec<(String, u64)> {
        let (status, body) = self.send(op).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["result"]["status"], "ok", "{body}");
        assert!(body.get("code").is_none(), "{body}");
        body["result"]["revs"]
            .as_array()
            .expect("revs")
            .iter()
            .map(|entry| {
                (
                    entry["id"].as_str().expect("id").to_string(),
                    entry["rev"].as_u64().expect("rev"),
                )
            })
            .collect()
    }

    async fn rois(&self, index: usize) -> EmbedRoiAnnotations {
        let response = self
            .server
            .get(&format!("/api/file/{index}/annotations"))
            .await;
        response.assert_status_ok();
        response.json()
    }

    async fn put_rois(&self, index: usize, coords: Value, frames: Value) -> StatusCode {
        let count = coords.as_array().map_or(0, Vec::len);
        self.server
            .put(&format!("/api/file/{index}/annotations"))
            .json(&json!({ "num_roi": count, "roi_coords": coords, "roi_frames": frames }))
            .await
            .status_code()
    }

    /// The store's revision, read with an operation that is refused
    /// whatever the state and so changes nothing.
    async fn revision(&mut self) -> u64 {
        let (status, body) = self.send(json!({ "type": "batch", "ops": [] })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        body["revision"].as_u64().expect("revision")
    }
}

fn codes(body: &Value) -> Vec<&str> {
    body["result"]["violations"]
        .as_array()
        .map(|violations| {
            violations
                .iter()
                .filter_map(|violation| violation["code"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// An envelope changes everything it names or nothing, a batch included,
/// whether the operation that fails is invalid or stale, and wherever it
/// stands in the batch.
#[tokio::test]
async fn an_envelope_is_applied_whole_or_not_at_all() {
    let dir = tempdir().expect("temp dir");
    let key = "sop:1.2.3.1";
    let mut client = Client::new(serve(vec![entry(dir.path(), "a.dcm", "1.2.3.1")]));

    // The second operation leaves the image, so the first is not applied.
    let (status, body) = client
        .send(json!({ "type": "batch", "ops": [
            create(1, key, [10.0, 20.0, 30.0, 40.0]),
            create(2, key, [10.0, 20.0, 121.0, 40.0]),
        ] }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "annotation_invalid");
    assert!(body["error"].is_string());
    assert_eq!(body["result"]["status"], "invalid");
    assert_eq!(codes(&body), ["out_of_bounds"]);
    assert_eq!(body["revision"], 0);
    assert_eq!(client.rois(0).await.num_roi, 0);

    // Record 1 was never created: creating it now is not a duplicate.
    assert_eq!(
        client
            .applied(create(1, key, [10.0, 20.0, 30.0, 40.0]))
            .await,
        [(id(1), 1)]
    );
    client.applied(create(2, key, [1.0, 2.0, 3.0, 4.0])).await;
    client
        .applied(move_to(2, key, 1, [1.0, 2.0, 5.0, 6.0]))
        .await;

    // The second operation is stale (record 2 is at revision 2), so the
    // move of record 1 before it is not applied either.
    let (status, body) = client
        .send(json!({ "type": "batch", "ops": [
            move_to(1, key, 1, [50.0, 50.0, 60.0, 60.0]),
            move_to(2, key, 1, [50.0, 50.0, 60.0, 60.0]),
        ] }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "annotation_conflict");
    assert_eq!(body["result"]["status"], "conflict");
    assert_eq!(body["result"]["current"]["kind"], "annotation");
    assert_eq!(body["result"]["current"]["record"]["id"], id(2));
    assert_eq!(body["result"]["current"]["record"]["rev"], 2);
    assert_eq!(body["revision"], 3);
    assert_eq!(
        client.rois(0).await.roi_coords,
        [[20, 10, 40, 30], [2, 1, 6, 5]]
    );

    // A batch that holds together is one transaction: each operation sees
    // the ones before it, and every revision it left is reported once.
    let revs = client
        .applied(json!({ "type": "batch", "ops": [
            { "type": "create_layer", "layer": { "id": "second", "name": "Second", "kind": "user" } },
            move_to(1, key, 1, [50.0, 50.0, 60.0, 60.0]),
            { "type": "update_annotation", "id": id(1), "file": key, "base_rev": 2,
              "before": { "layer": "default" }, "after": { "layer": "second" } },
        ] }))
        .await;
    assert_eq!(revs, [("second".to_string(), 1), (id(1), 3)]);
    assert_eq!(client.revision().await, 4);
}

/// A record's revision counts its changes, delete and restore included; an
/// operation based on an older one is refused with the record as it is;
/// the store's revision counts applied envelopes only.
#[tokio::test]
async fn revisions_count_changes_and_stale_operations_are_refused() {
    let dir = tempdir().expect("temp dir");
    let key = "sop:1.2.3.1";
    let mut client = Client::new(serve(vec![entry(dir.path(), "a.dcm", "1.2.3.1")]));
    let snapshot = |record: u32| rect(record, key, [0.0, 0.0, 1.0, 1.0]);

    // A created record starts at revision 1 whatever the payload said.
    assert_eq!(
        client
            .applied(create(1, key, [10.00049, 20.0, 30.0, 40.0]))
            .await,
        [(id(1), 1)]
    );
    client.applied(create(2, key, [1.0, 2.0, 3.0, 4.0])).await;
    // A coordinate is stored on the model's grid of a thousandth of a pixel.
    let (status, body) = client
        .send(move_to(1, key, 9, [12.0, 20.0, 30.0, 40.0]))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["result"]["current"]["record"]["geometry"]["x0"], 10);
    assert_eq!(
        client
            .applied(move_to(1, key, 1, [11.0, 20.0, 30.0, 40.0]))
            .await,
        [(id(1), 2)]
    );

    let (status, body) = client
        .send(move_to(1, key, 1, [12.0, 20.0, 30.0, 40.0]))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let current = &body["result"]["current"]["record"];
    assert_eq!(current["rev"], 2);
    assert_eq!(current["geometry"]["x0"], 11);
    // The store stamps the record; the client's stamps were advisory.
    assert_ne!(current["created_by"], "user:someone-else");
    assert!(current["created_by"]
        .as_str()
        .is_some_and(|by| by.starts_with("user:")));
    assert_ne!(current["created_at"], "2001-01-01T00:00:00.000Z");
    assert!(current["modified_at"].as_str() >= current["created_at"].as_str());
    assert_eq!(body["revision"], 3);

    let delete = |base_rev: u64| json!({ "type": "delete_annotation", "id": id(1), "base_rev": base_rev, "snapshot": snapshot(1) });
    let restore = |base_rev: u64| json!({ "type": "restore_annotation", "id": id(1), "base_rev": base_rev, "snapshot": snapshot(1) });
    assert_eq!(client.applied(delete(2)).await, [(id(1), 3)]);
    assert_eq!(client.rois(0).await.roi_coords, [[2, 1, 4, 3]]);

    // A deleted record keeps its id and its revision.
    let table = [
        (
            "an update of it",
            move_to(1, key, 3, [12.0, 20.0, 30.0, 40.0]),
            StatusCode::CONFLICT,
            "deleted",
        ),
        (
            "a second delete",
            delete(3),
            StatusCode::CONFLICT,
            "deleted",
        ),
        (
            "a restore from an older revision",
            restore(2),
            StatusCode::CONFLICT,
            "deleted",
        ),
        (
            "an update of a record that never was",
            move_to(9, key, 1, [12.0, 20.0, 30.0, 40.0]),
            StatusCode::CONFLICT,
            "missing",
        ),
    ];
    for (case, op, status, kind) in table {
        let (answered, body) = client.send(op).await;
        assert_eq!(answered, status, "{case}: {body}");
        assert_eq!(body["result"]["current"]["kind"], kind, "{case}: {body}");
        if kind == "deleted" {
            assert_eq!(body["result"]["current"]["rev"], 3, "{case}: {body}");
        }
    }
    // Creating over a deleted record is never an upsert.
    let (status, body) = client.send(create(1, key, [1.0, 1.0, 2.0, 2.0])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body), ["duplicate_id"]);

    // Restored, the record is what it was and where it was.
    assert_eq!(client.applied(restore(3)).await, [(id(1), 4)]);
    assert_eq!(
        client.rois(0).await.roi_coords,
        [[20, 11, 40, 30], [2, 1, 4, 3]]
    );
    let (status, body) = client.send(restore(4)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["result"]["current"]["kind"], "annotation");

    // Five envelopes were applied; none of the refusals counted.
    assert_eq!(client.revision().await, 5);
}

/// A retry after a lost answer resends the same `op_id` and gets the first
/// answer back, with nothing applied twice.
#[tokio::test]
async fn a_repeated_op_id_returns_the_first_result_and_applies_nothing() {
    let dir = tempdir().expect("temp dir");
    let key = "sop:1.2.3.1";
    let server = serve(vec![entry(dir.path(), "a.dcm", "1.2.3.1")]);
    let first_try = envelope(5001, create(1, key, [10.0, 20.0, 30.0, 40.0]));

    let first = server.post(OPS).json(&first_try).await;
    first.assert_status_ok();
    let moved = server
        .post(OPS)
        .json(&envelope(
            5002,
            move_to(1, key, 1, [11.0, 20.0, 30.0, 40.0]),
        ))
        .await;
    moved.assert_status_ok();

    // On its own the create would now be refused as a duplicate.
    let again = server.post(OPS).json(&first_try).await;
    again.assert_status_ok();
    assert_eq!(
        again.json::<Value>()["result"],
        first.json::<Value>()["result"]
    );
    assert_eq!(again.json::<Value>()["revision"], 2);
    let moved_again = server
        .post(OPS)
        .json(&envelope(
            5002,
            move_to(1, key, 1, [11.0, 20.0, 30.0, 40.0]),
        ))
        .await;
    moved_again.assert_status_ok();
    assert_eq!(
        moved_again.json::<Value>()["result"]["revs"],
        json!([{ "id": id(1), "rev": 2 }])
    );

    let rois: EmbedRoiAnnotations = server.get("/api/file/0/annotations").await.json();
    assert_eq!(rois.roi_coords, [[20, 11, 40, 30]]);
}

/// The EMBED endpoints and the CSV export show the store's rectangles, and
/// a save through them changes only the records that differ, by the rule
/// the endpoint always had.
#[tokio::test]
async fn the_embed_endpoints_are_a_view_of_the_same_records() {
    let dir = tempdir().expect("temp dir");
    let key = "sop:1.2.3.1";
    let path = dir.path().join("a.dcm");
    let mut client = Client::new(serve(vec![entry(dir.path(), "a.dcm", "1.2.3.1")]));

    // A rectangle off the pixel grid is shown rounded outward; a point is
    // not a ROI.
    client
        .applied(create(1, key, [10.5, 20.25, 30.5, 40.0]))
        .await;
    let mut point = rect(2, key, [0.0, 0.0, 0.0, 0.0]);
    point["geometry"] = json!({ "type": "point", "x": 5.5, "y": 5.5 });
    client
        .applied(json!({ "type": "create_annotation", "annotation": point }))
        .await;
    assert_eq!(
        client.rois(0).await,
        EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[20, 10, 40, 31]],
            roi_frames: vec![],
        }
    );
    let export = client.server.get("/api/annotations/export.csv").await;
    export.assert_status_ok();
    assert_eq!(
        export.text(),
        format!(
            "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{},1,\"[[20,10,40,31]]\",[]\n",
            path.display()
        )
    );

    // Saving what is shown changes nothing: the record keeps its revision.
    let before = client.revision().await;
    assert_eq!(
        client
            .put_rois(0, json!([[20, 10, 40, 31]]), json!([]))
            .await,
        StatusCode::OK
    );
    assert_eq!(client.revision().await, before);

    // A second ROI and a frame list: the first record is changed in place
    // and keeps the geometry the save did not touch.
    assert_eq!(
        client
            .put_rois(
                0,
                json!([[20, 10, 40, 31], [1, 2, 3, 4]]),
                json!([[0], [3, 1, 1]])
            )
            .await,
        StatusCode::OK
    );
    assert_eq!(client.revision().await, before + 1);
    let (status, body) = client.send(move_to(1, key, 1, [0.0, 0.0, 9.0, 9.0])).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let current = &body["result"]["current"]["record"];
    assert_eq!(current["rev"], 2);
    assert_eq!(current["geometry"]["x0"], 10.5);
    assert_eq!(current["frames"], json!({ "set": [0] }));
    assert_eq!(
        client.rois(0).await.roi_frames,
        [vec![0], vec![3, 1, 1]],
        "a frame list is kept as written"
    );

    // The endpoint's own rule decides what a save may hold, as before: an
    // empty frame list for one ROI is accepted and kept, and a box outside
    // the image refuses the whole save.
    assert_eq!(
        client
            .put_rois(0, json!([[20, 10, 40, 31], [1, 2, 3, 4]]), json!([[0], []]))
            .await,
        StatusCode::OK
    );
    assert_eq!(
        client.rois(0).await.roi_frames,
        [vec![0], Vec::<u32>::new()]
    );
    assert_eq!(
        client
            .put_rois(
                0,
                json!([[20, 10, 40, 31], [1, 2, 3, 121]]),
                json!([[0], []])
            )
            .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        client.rois(0).await.roi_coords,
        [[20, 10, 40, 31], [1, 2, 3, 4]]
    );

    // Fewer ROIs delete the records past the end; the point is untouched.
    assert_eq!(
        client.put_rois(0, json!([]), json!([])).await,
        StatusCode::OK
    );
    assert_eq!(client.rois(0).await.num_roi, 0);
    let (status, _) = client
        .send(json!({ "type": "delete_annotation", "id": id(2), "base_rev": 1, "snapshot": rect(2, key, [0.0, 0.0, 0.0, 0.0]) }))
        .await;
    assert_eq!(status, StatusCode::OK);
}

/// Two files with the same bytes share a key and so a set of records. Each
/// keeps the ROI rows that were written for it, as before; a record made by
/// an operation belongs to the image and shows on both.
#[tokio::test]
async fn files_with_the_same_bytes_keep_the_rows_written_for_each() {
    let dir = tempdir().expect("temp dir");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden-jpeg-baseline-single-frame.dcm");
    let mut files = Vec::new();
    for name in ["first.dcm", "copy.dcm"] {
        let path = dir.path().join(name);
        std::fs::copy(&fixture, &path).expect("copy fixture");
        let mut file = support::file_entry(path, "1.2.840.10008.1.2.4.50", 1);
        file.sop_instance_uid = "1.2.3.7".to_string();
        files.push(file);
    }
    let mut client = Client::new(serve(files));

    assert_eq!(
        client.put_rois(0, json!([[1, 2, 3, 4]]), json!([])).await,
        StatusCode::OK
    );
    assert_eq!(
        client.put_rois(1, json!([[5, 6, 7, 8]]), json!([])).await,
        StatusCode::OK
    );
    client
        .applied(create(1, "sop:1.2.3.7", [9.0, 9.0, 12.0, 12.0]))
        .await;

    assert_eq!(
        client.rois(0).await.roi_coords,
        [[1, 2, 3, 4], [9, 9, 12, 12]]
    );
    assert_eq!(
        client.rois(1).await.roi_coords,
        [[5, 6, 7, 8], [9, 9, 12, 12]]
    );
    let export = client
        .server
        .get("/api/annotations/export.csv")
        .await
        .text();
    assert_eq!(export.lines().count(), 3, "{export}");
}

/// Nothing is written for a file until its key is settled, and nothing at
/// all for a file that cannot have one.
#[tokio::test]
async fn a_write_settles_the_files_key_and_fails_without_one() {
    let dir = tempdir().expect("temp dir");
    // 0: no usable UID, so its key is a digest of its bytes.
    let readable = dir.path().join("no-uid.dcm");
    std::fs::write(&readable, b"bytes that stand for a file").expect("write file");
    let mut no_uid = support::file_entry(readable, "1.2.840.10008.1.2.4.50", 1);
    no_uid.sop_instance_uid = String::new();
    // 1: the same, but the file is gone.
    let mut gone = entry(dir.path(), "gone.dcm", "");
    gone.sop_instance_uid = String::new();
    // 2 and 3: one UID and one size, and the first of them cannot be read.
    let files = vec![
        no_uid,
        gone,
        entry(dir.path(), "shared-first.dcm", "1.2.3.9"),
        entry(dir.path(), "shared-second.dcm", "1.2.3.9"),
    ];
    let registry = FileRegistry::from_files(files);
    let mut client = Client::new(TestServer::new(server::router(AppState::new(
        registry.clone(),
        AnnotationStore::empty(),
    ))));
    let roi = || json!([[1, 2, 3, 4]]);

    // A read hashes nothing and waits for nothing.
    assert_eq!(client.rois(0).await.num_roi, 0);
    assert_eq!(registry.key_stats().files_hashed, 0);

    // The save waits for the file to be hashed, and the record is under
    // the key that gave.
    assert_eq!(client.put_rois(0, roi(), json!([])).await, StatusCode::OK);
    let catalog: Value = client.server.get("/api/files").await.json();
    let key = catalog["files"][0]["file_key"]
        .as_str()
        .expect("a settled key")
        .to_string();
    assert!(key.starts_with("b3:"), "{key}");
    client.applied(create(1, &key, [5.0, 5.0, 9.0, 9.0])).await;
    assert_eq!(
        client.rois(0).await.roi_coords,
        [[1, 2, 3, 4], [5, 5, 9, 9]]
    );

    let before = client.revision().await;
    for index in [1, 2, 3] {
        let response = client
            .server
            .put(&format!("/api/file/{index}/annotations"))
            .json(&json!({ "num_roi": 1, "roi_coords": roi(), "roi_frames": [] }))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "file {index}"
        );
        let body: Value = response.json();
        assert_eq!(body["code"], "file_key_unavailable", "file {index}");
        assert!(body["error"].is_string());
        assert_eq!(client.rois(index).await.num_roi, 0, "file {index}");
    }
    // A list the endpoint refuses is refused before any file is read.
    let hashed = registry.key_stats().files_hashed;
    assert_eq!(
        client.put_rois(1, json!([[1, 2, 3, 999]]), json!([])).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(registry.key_stats().files_hashed, hashed);

    // An operation under the key the catalog shows for such a file fails
    // the same way; one under a key no file has is an invalid operation.
    let (status, body) = client
        .send(create(2, "sop:1.2.3.9", [5.0, 5.0, 9.0, 9.0]))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "file_key_unavailable");
    assert!(body.get("result").is_none(), "{body}");
    let (status, body) = client
        .send(create(2, "sop:9.9.9", [5.0, 5.0, 9.0, 9.0]))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "annotation_invalid");
    assert_eq!(codes(&body), ["unknown_file"]);
    assert_eq!(client.revision().await, before);

    let export = client.server.get("/api/annotations/export.csv").await;
    export.assert_status_ok();
    assert_eq!(export.text().lines().count(), 2, "{}", export.text());
}

/// The rows of an `--annotations` CSV become records file by file; a file
/// without a key loses its rows and the others load.
#[tokio::test]
async fn csv_rows_of_a_file_without_a_key_are_dropped_and_the_rest_load() {
    let dir = tempdir().expect("temp dir");
    let mut gone = entry(dir.path(), "gone.dcm", "");
    gone.sop_instance_uid = String::new();
    let registry = FileRegistry::from_files(vec![entry(dir.path(), "a.dcm", "1.2.3.1"), gone]);
    let store = AnnotationStore::loading();
    // Rows as a CSV wrote them: past the image edge, and a frame list out
    // of order. They are held as loaded.
    let rows = |coords: [u32; 4]| EmbedRoiAnnotations {
        num_roi: 1,
        roi_coords: vec![coords],
        roi_frames: vec![vec![2, 0, 0]],
    };

    let report = server::annotations::import_embed_rows(
        &registry,
        &store,
        HashMap::from([(0, rows([500, 600, 700, 800])), (1, rows([1, 2, 3, 4]))]),
    )
    .await
    .expect("import");
    assert_eq!(
        (
            report.files_loaded,
            report.files_without_key,
            report.files_edited
        ),
        (1, 1, 0)
    );

    let client = Client::new(TestServer::new(server::router(AppState::new(
        registry, store,
    ))));
    assert_eq!(client.rois(0).await, rows([500, 600, 700, 800]));
    assert_eq!(client.rois(1).await.num_roi, 0);
    let export = client
        .server
        .get("/api/annotations/export.csv")
        .await
        .text();
    assert_eq!(
        export,
        format!(
            "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{},1,\"[[500,600,700,800]]\",\"[[2,0,0]]\"\n",
            dir.path().join("a.dcm").display()
        )
    );
}

/// The body of an operation is bounded by the annotation model's own
/// numbers, and everything the endpoint refuses is a JSON error.
#[tokio::test]
async fn an_operation_is_bounded_by_the_models_limits() {
    let dir = tempdir().expect("temp dir");
    let key = "sop:1.2.3.1";
    let mut client = Client::new(serve(vec![entry(dir.path(), "a.dcm", "1.2.3.1")]));

    // Three mebibytes is more than a JSON body is by default, and well
    // inside what an envelope may be.
    let mut large = rect(1, key, [10.0, 20.0, 30.0, 40.0]);
    large["extensions"] = json!({ "test": "x".repeat(3 * 1024 * 1024) });
    client
        .applied(json!({ "type": "create_annotation", "annotation": large }))
        .await;

    let too_many: Vec<Value> = (0..10_001)
        .map(|n| create(100 + n, key, [1.0, 1.0, 2.0, 2.0]))
        .collect();
    let (status, body) = client
        .send(json!({ "type": "batch", "ops": too_many }))
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        body["error"]
    );
    assert_eq!(codes(&body), ["too_many_ops"]);

    let table = [
        (
            "a body past sixteen mebibytes",
            format!("{{\"pad\":\"{}\"}}", "x".repeat(16 * 1024 * 1024)),
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        ),
        (
            "text that is not JSON",
            "not json".to_string(),
            StatusCode::BAD_REQUEST,
            "invalid_json",
        ),
        (
            "JSON that is not an envelope",
            json!({ "op": { "type": "paint_it_black" } }).to_string(),
            StatusCode::BAD_REQUEST,
            "invalid_json",
        ),
    ];
    for (case, body, status, code) in table {
        let response = client.server.post(OPS).text(body).await;
        assert_eq!(response.status_code(), status, "{case}");
        let answer: Value = response.json();
        assert_eq!(answer["code"], code, "{case}");
        assert!(answer["error"].is_string(), "{case}");
    }
    assert_eq!(client.revision().await, 1);
}

/// A masked session takes and gives file keys built from masked UIDs, at
/// this door as in the catalog, and a real UID names nothing.
#[tokio::test]
async fn a_masked_session_reads_and_writes_keys_built_from_masked_uids() {
    let dir = tempdir().expect("temp dir");
    let real_uid = "1.2.826.0.1.3680043.10.777.5";
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    registry.insert(entry(dir.path(), "a.dcm", real_uid));
    registry.mark_scan_complete();
    let mut client = Client::new(TestServer::new(server::router(
        support::app_state_with_registry(registry),
    )));

    let catalog: Value = client.server.get("/api/files").await.json();
    let masked_uid = catalog["files"][0]["sop_instance_uid"]
        .as_str()
        .expect("masked UID");
    assert_ne!(masked_uid, real_uid);
    let shown = format!("sop:{masked_uid}");

    client
        .applied(create(1, &shown, [10.0, 20.0, 30.0, 40.0]))
        .await;
    assert_eq!(client.rois(0).await.roi_coords, [[20, 10, 40, 30]]);

    // The record a conflict sends back names its file as the session does.
    let (status, body) = client
        .send(move_to(1, &shown, 5, [1.0, 1.0, 2.0, 2.0]))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["result"]["current"]["record"]["file"], shown);
    assert!(!body.to_string().contains(real_uid), "{body}");

    let (status, body) = client
        .send(create(2, &format!("sop:{real_uid}"), [1.0, 1.0, 2.0, 2.0]))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body), ["unknown_file"]);
    let (status, body) = client
        .send(move_to(
            1,
            &format!("sop:{real_uid}"),
            1,
            [1.0, 1.0, 2.0, 2.0],
        ))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body), ["unknown_file"]);
    assert_eq!(client.rois(0).await.roi_coords, [[20, 10, 40, 30]]);
}
