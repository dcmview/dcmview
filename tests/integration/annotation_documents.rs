//! Annotation documents as a session serves them: the native export
//! (`GET /api/annotations/export.json` and `export.jsonl`), a native
//! `--annotations` import, and where the import stands
//! (`docs/design/output-adapters.md` 7.1, 8).

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::annotations::{
    AnnotationBackend, AnnotationStore, EmbedRoiAnnotations, MemoryBackend, MemoryConfig,
    NativeSource,
};
use dcmview::masking::Masker;
use dcmview::server::annotations::{import_embed_rows, import_native, AnnotationError};
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::FileEntry;
use dcmview_annotation::{ApplyResult, Author, FileKey, ImageSize, LabelSchema, OpEnvelope};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

/// Two content keys no loaded file has.
const OTHER_BYTES: &str = "b3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CHANGED_BYTES: &str = "b3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

/// A UUIDv7 that is the same in every run.
fn id(n: u32) -> String {
    format!("0199c0de-0000-7000-8000-{n:012x}")
}

/// An entry of 100 rows by 120 columns with four frames. The file need not
/// exist: a UID no other file has is a key at once.
fn entry(directory: &Path, name: &str, uid: &str) -> FileEntry {
    let mut entry = support::file_entry(directory.join(name), "1.2.840.10008.1.2.4.50", 4);
    entry.rows = 100;
    entry.columns = 120;
    entry.sop_instance_uid = uid.to_string();
    entry
}

/// A file that exists, with `bytes` in it, under `uid`.
fn written(directory: &Path, name: &str, uid: &str, bytes: &[u8]) -> FileEntry {
    std::fs::write(directory.join(name), bytes).expect("write file");
    entry(directory, name, uid)
}

fn rows(coords: &[[u32; 4]], frames: &[&[u32]]) -> EmbedRoiAnnotations {
    EmbedRoiAnnotations {
        num_roi: coords.len(),
        roi_coords: coords.to_vec(),
        roi_frames: frames.iter().map(|list| list.to_vec()).collect(),
    }
}

/// One session: its registry and store, and a server over them.
struct Session {
    registry: FileRegistry,
    store: AnnotationStore,
    server: TestServer,
}

impl Session {
    fn over(registry: FileRegistry, store: AnnotationStore) -> Self {
        let server = TestServer::new(server::router(AppState::new(
            registry.clone(),
            store.clone(),
        )));
        Self {
            registry,
            store,
            server,
        }
    }

    /// A session over `files` whose `--annotations` import has not ended.
    fn loading(files: Vec<FileEntry>) -> Self {
        Self::over(FileRegistry::from_files(files), AnnotationStore::loading())
    }

    /// A session over `files` that had no `--annotations`.
    fn plain(files: Vec<FileEntry>) -> Self {
        Self::over(FileRegistry::from_files(files), AnnotationStore::empty())
    }

    fn stage(&self, rows: HashMap<usize, EmbedRoiAnnotations>) {
        import_embed_rows(&self.registry, &self.store, rows).expect("stage rows");
    }

    /// Ends the session's import with the document in `text`.
    async fn import(&self, directory: &Path, text: &str) -> Result<Value, AnnotationError> {
        let path = directory.join(format!("document-{}.json", dcmview_annotation::new_id()));
        std::fs::write(&path, text).expect("write document");
        let source = NativeSource::from_path(&path).expect("document source");
        import_native(&self.registry, &self.store, &source, Arc::new(|| true))
            .await
            .map(|report| serde_json::to_value(report).expect("report"))
    }

    async fn op(&self, file: Option<usize>, op: Value) {
        let path = match file {
            Some(index) => format!("/api/annotations/ops?file={index}"),
            None => "/api/annotations/ops".to_string(),
        };
        let response = self
            .server
            .post(&path)
            .json(&json!({
                "op_id": dcmview_annotation::new_id(), "actor": "user:test",
                "ts": "2026-10-10T08:00:00.000Z", "op": op,
            }))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
    }

    async fn rois(&self, index: usize) -> EmbedRoiAnnotations {
        let response = self
            .server
            .get(&format!("/api/file/{index}/annotations"))
            .await;
        response.assert_status_ok();
        response.json()
    }

    async fn text(&self, path: &str) -> String {
        let response = self.server.get(path).await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{path}: {}",
            response.text()
        );
        response.text()
    }

    async fn document(&self) -> Value {
        serde_json::from_str(&self.text("/api/annotations/export.json").await).expect("document")
    }

    async fn csv(&self) -> String {
        self.text("/api/annotations/export.csv").await
    }
}

fn annotation(n: u32, file: &str, layer: &str, geometry: Value) -> Value {
    json!({
        "id": id(n), "file": file, "frames": "all", "layer": layer, "class": "roi",
        "geometry": geometry, "attributes": {}, "extensions": {},
        "rev": 9, "created_by": "user:someone", "created_at": "2001-01-01T00:00:00.000Z",
        "modified_by": "user:someone", "modified_at": "2001-01-01T00:00:00.000Z",
    })
}

fn rect(x0: u32, y0: u32, x1: u32, y1: u32) -> Value {
    json!({ "type": "rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1 })
}

fn create(n: u32, file: &str, layer: &str, geometry: Value) -> Value {
    json!({ "type": "create_annotation", "annotation": annotation(n, file, layer, geometry) })
}

/// A file reference as a document holds it, for a file of this module's
/// size.
fn reference(key: &str, uid: Option<&str>, path: &str) -> Value {
    json!({
        "key": key, "kind": "dicom", "sop_instance_uid": uid, "path": path,
        "size_bytes": 0, "rows": 100, "columns": 120, "frames": 4, "format": "dicom",
    })
}

fn document_of(files: Vec<Value>, layers: Vec<Value>, annotations: Vec<Value>) -> String {
    json!({
        "format": "dcmview.annotations", "version": "1.0",
        "files": files, "layers": layers, "annotations": annotations,
    })
    .to_string()
}

/// What of a record a round trip keeps: everything but the stamps and the
/// revision, which the store that takes the record writes itself.
fn kept(document: &Value) -> Vec<Value> {
    document["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| {
            json!([
                record["id"],
                record["file"],
                record["layer"],
                record["class"],
                record["geometry"],
                record["frames"],
                record["attributes"],
            ])
        })
        .collect()
}

/// A session with rows the CSV import staged, records operations made, a
/// second layer, and a box the import kept as read: all of it is in the
/// document, and a session that loads the document shows the same thing.
#[tokio::test]
async fn a_session_is_written_as_a_document_and_read_back_as_the_same_records() {
    let dir = tempdir().expect("temp dir");
    std::fs::create_dir(dir.path().join("sub")).expect("sub directory");
    let files = || {
        vec![
            entry(dir.path(), "a.dcm", "1.2.3.1"),
            entry(dir.path(), "sub/b.dcm", "1.2.3.2"),
            entry(dir.path(), "c.dcm", "1.2.3.3"),
        ]
    };
    let first = Session::loading(files());
    first.stage(HashMap::from([
        (0, rows(&[[10, 20, 30, 40]], &[&[2, 0, 0]])),
        // Past the image, and without area: kept as read.
        (1, rows(&[[500, 600, 700, 800], [5, 5, 5, 5]], &[])),
    ]));
    let polygon = json!({ "type": "polygon", "points": [
        { "x": 1, "y": 1 }, { "x": 9.5, "y": 1 }, { "x": 5, "y": 9.25 }] });
    first
        .op(
            Some(0),
            create(1, "sop:1.2.3.1", "default", polygon.clone()),
        )
        .await;
    first
        .op(
            None,
            json!({ "type": "create_layer",
            "layer": { "id": "second", "name": "Second reader", "kind": "user" } }),
        )
        .await;
    first
        .op(
            Some(1),
            create(2, "sop:1.2.3.2", "second", rect(1, 2, 30, 40)),
        )
        .await;
    let views = [first.rois(0).await, first.rois(1).await];
    let csv = first.csv().await;

    let response = first.server.get("/api/annotations/export.json").await;
    response.assert_status_ok();
    assert_eq!(response.header("content-type"), "application/json");
    assert_eq!(
        response.header("content-disposition"),
        "attachment; filename=\"dcmview-annotations.json\""
    );
    let text = response.text();
    let document: Value = serde_json::from_str(&text).expect("document");

    assert_eq!(document["format"], "dcmview.annotations");
    assert_eq!(document["version"], "1.0");
    assert!(document["schema"].is_null());
    // One reference for each file a record names, in the order first
    // named, with its path below the directory every loaded file is in.
    let described: Vec<Value> = document["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| {
            json!([
                file["key"],
                file["kind"],
                file["format"],
                file["path"],
                file["sop_instance_uid"],
                file["patient_id"],
                file["rows"],
                file["columns"],
                file["frames"],
                file["digest"],
            ])
        })
        .collect();
    assert_eq!(
        described,
        vec![
            json!([
                "sop:1.2.3.1",
                "dicom",
                "dicom",
                "a.dcm",
                "1.2.3.1",
                "TEST",
                100,
                120,
                4,
                null
            ]),
            json!([
                "sop:1.2.3.2",
                "dicom",
                "dicom",
                "sub/b.dcm",
                "1.2.3.2",
                "TEST",
                100,
                120,
                4,
                null
            ]),
        ]
    );
    let layers: Vec<&str> = document["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .filter_map(|layer| layer["id"].as_str())
        .collect();
    assert_eq!(layers, ["default", "second"]);
    // The staged rows are records now, each before what was drawn on its
    // file afterwards, as written: frame lists, a box past the edge, a box
    // without area.
    let records: Vec<Value> = document["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| {
            json!([
                record["file"],
                record["layer"],
                record["geometry"],
                record["frames"],
                record["created_by"],
            ])
        })
        .collect();
    let author = document["annotations"][1]["created_by"].clone();
    assert!(author
        .as_str()
        .is_some_and(|author| author.starts_with("user:")));
    assert_eq!(
        records,
        vec![
            json!(["sop:1.2.3.1", "default", rect(20, 10, 40, 30),
                   { "set": [0, 2], "as_written": [2, 0, 0] }, "import:embed"]),
            json!(["sop:1.2.3.1", "default", polygon, "all", author]),
            json!([
                "sop:1.2.3.2",
                "default",
                rect(600, 500, 800, 700),
                "all",
                "import:embed"
            ]),
            json!([
                "sop:1.2.3.2",
                "default",
                rect(5, 5, 5, 5),
                "all",
                "import:embed"
            ]),
            json!(["sop:1.2.3.2", "second", rect(1, 2, 30, 40), "all", author]),
        ]
    );
    // Making the staged rows records changed nothing the EMBED doors show.
    assert_eq!([first.rois(0).await, first.rois(1).await], views);
    assert_eq!(first.csv().await, csv);

    // The JSON Lines form is the same document, a record a line.
    let response = first.server.get("/api/annotations/export.jsonl").await;
    response.assert_status_ok();
    assert_eq!(response.header("content-type"), "application/jsonl");
    assert_eq!(
        response.header("content-disposition"),
        "attachment; filename=\"dcmview-annotations.jsonl\""
    );
    let lines_text = response.text();
    let lines: Vec<Value> = lines_text
        .lines()
        .map(|line| serde_json::from_str(line).expect("a line is JSON"))
        .collect();
    assert_eq!(lines[0]["format"], "dcmview.annotations");
    assert_eq!(lines[0]["annotations"], json!([]));
    let mut expected: Vec<Value> = Vec::new();
    for (name, list) in [
        ("file", "files"),
        ("layer", "layers"),
        ("annotation", "annotations"),
    ] {
        for item in document[list].as_array().expect("list") {
            expected.push(json!({ name: item }));
        }
    }
    expected.push(json!({ "end": { "files": 2, "layers": 2, "annotations": 5, "labels": 0 } }));
    assert_eq!(lines[1..], expected[..]);

    // A new session over the same files, started with either form.
    for (form, text) in [("JSON", &text), ("JSON Lines", &lines_text)] {
        let second = Session::loading(files());
        let report = second
            .import(dir.path(), text)
            .await
            .unwrap_or_else(|error| panic!("{form}: {error}"));
        assert_eq!(
            report,
            json!({
                "adapter": { "id": "native", "version": 1 }, "direction": "import",
                "counts": {
                    "files": 2, "annotations_in": 5, "annotations_out": 5,
                    "labels_in": 0, "labels_out": 0, "unmatched": 0, "refused": 0,
                },
                "losses": [],
                // The boxes the EMBED import kept as read are taken back
                // as read, and said to be what they are.
                "warnings": [
                    { "code": "out_of_range", "count": 1, "rows": [{ "row": 8 }] },
                    { "code": "zero_area", "count": 1, "rows": [{ "row": 9 }] },
                ],
                "matched_by": { "key": 2, "sop_instance_uid": 0, "path": 0 },
            }),
            "{form}"
        );
        let status: Value = second.server.get("/api/annotations/import").await.json();
        assert_eq!(
            status,
            json!({ "state": "ready", "report": report }),
            "{form}"
        );

        let reloaded = second.document().await;
        assert_eq!(kept(&reloaded), kept(&document), "{form}");
        assert_eq!(reloaded["files"], document["files"], "{form}");
        assert_eq!(reloaded["layers"], document["layers"], "{form}");
        for record in reloaded["annotations"].as_array().expect("annotations") {
            // The store that takes a record stamps it.
            assert_eq!(record["rev"], 1, "{form}");
            assert_eq!(record["created_by"], "import:native", "{form}");
            assert_ne!(record["created_at"], "2001-01-01T00:00:00.000Z", "{form}");
        }
        assert_eq!(
            [second.rois(0).await, second.rois(1).await],
            views,
            "{form}"
        );
        assert_eq!(second.csv().await, csv, "{form}");
    }
}

/// An import changes the session as one transaction or not at all, and a
/// session whose import failed says so wherever the import is waited for.
#[tokio::test]
async fn an_import_is_applied_whole_or_not_at_all() {
    let dir = tempdir().expect("temp dir");
    let files = || vec![entry(dir.path(), "a.dcm", "1.2.3.1")];
    let second_layer = || json!({ "id": "second", "name": "Second reader", "kind": "user" });
    let document = document_of(
        vec![reference("sop:1.2.3.1", Some("1.2.3.1"), "a.dcm")],
        vec![second_layer()],
        vec![
            annotation(1, "sop:1.2.3.1", "second", rect(1, 2, 3, 4)),
            annotation(2, "sop:1.2.3.1", "default", rect(5, 6, 7, 8)),
        ],
    );
    let key = [dcmview_annotation::FileKey::parse("sop:1.2.3.1").expect("key")];

    // The store refuses the document's last record: a client made one
    // under that id while the import was still to come. Nothing of the
    // document is in the session, not the layer, not the first record.
    let session = Session::loading(files());
    session
        .op(
            Some(0),
            create(2, "sop:1.2.3.1", "default", rect(50, 50, 60, 60)),
        )
        .await;
    let error = session
        .import(dir.path(), &document)
        .await
        .expect_err("refused");
    assert_eq!(
        error,
        AnnotationError::ImportFailed(
            "the annotation store refused the import: DuplicateId".to_string()
        )
    );
    let snapshot = session.store.backend().snapshot(&key).expect("snapshot");
    assert_eq!(
        snapshot.revision, 1,
        "only the client's operation was applied"
    );
    assert_eq!(snapshot.layers.len(), 1);
    assert_eq!(snapshot.annotations.len(), 1);
    assert_eq!(snapshot.annotations[0].id.to_string(), id(2));
    // The failure is what every door that waits for the import answers.
    let status: Value = session.server.get("/api/annotations/import").await.json();
    assert_eq!(
        status,
        json!({ "state": "failed",
                "error": "the annotation store refused the import: DuplicateId" })
    );
    for path in [
        "/api/file/0/annotations",
        "/api/annotations/export.csv",
        "/api/annotations/export.json",
        "/api/annotations/export.jsonl",
    ] {
        let response = session.server.get(path).await;
        assert_eq!(
            response.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path}"
        );
        assert_eq!(response.json::<Value>()["error"], status["error"], "{path}");
    }

    // A layer the session already has refuses the document too.
    let session = Session::loading(files());
    session
        .op(
            None,
            json!({ "type": "create_layer", "layer": second_layer() }),
        )
        .await;
    assert!(session.import(dir.path(), &document).await.is_err());
    assert!(session
        .store
        .backend()
        .snapshot(&key)
        .expect("snapshot")
        .annotations
        .is_empty());

    // A record the format refuses fails the import before anything is
    // applied, and the message names its row and nothing the file said.
    let session = Session::loading(files());
    let polygon = json!({ "type": "polygon", "points": [
        { "x": 1, "y": 1 }, { "x": 900, "y": 1 }, { "x": 5, "y": 9 }] });
    let mut odd = annotation(3, "sop:1.2.3.1", "default", polygon);
    odd["class"] = json!("DOE^JANE");
    let invalid = document_of(
        vec![reference("sop:1.2.3.1", Some("1.2.3.1"), "a.dcm")],
        vec![],
        vec![
            annotation(1, "sop:1.2.3.1", "default", rect(1, 2, 3, 4)),
            odd,
        ],
    );
    let error = session
        .import(dir.path(), &invalid)
        .await
        .expect_err("invalid");
    assert_eq!(
        error,
        AnnotationError::ImportFailed(
            "annotations record 4 breaks a rule of the annotation model: OutOfBounds".to_string()
        )
    );
    assert_eq!(session.store.backend().revision().expect("revision"), 0);

    // A file that is gone when it is read: the message says so, and does
    // not say where the file was.
    let session = Session::loading(files());
    let path = dir.path().join("gone.json");
    std::fs::write(&path, &document).expect("write document");
    let source = NativeSource::from_path(&path).expect("document source");
    std::fs::remove_file(&path).expect("remove document");
    let error = import_native(
        &session.registry,
        &session.store,
        &source,
        Arc::new(|| true),
    )
    .await
    .expect_err("no file");
    assert_eq!(
        error,
        AnnotationError::ImportFailed(
            "the annotations file could not be opened: entity not found".to_string()
        )
    );
    let status: Value = session.server.get("/api/annotations/import").await.json();
    assert_eq!(status["state"], "failed");

    // An empty document is an import that brought nothing.
    let session = Session::loading(files());
    let report = session
        .import(dir.path(), &document_of(vec![], vec![], vec![]))
        .await
        .expect("empty");
    assert_eq!(report["counts"]["annotations_out"], 0);
    assert_eq!(session.store.backend().revision().expect("revision"), 0);
    assert_eq!(session.rois(0).await.num_roi, 0);

    // A session without `--annotations` has no import to report.
    let session = Session::plain(files());
    let status: Value = session.server.get("/api/annotations/import").await.json();
    assert_eq!(status, json!({ "state": "none" }));
    let session = Session::loading(files());
    let status: Value = session.server.get("/api/annotations/import").await.json();
    assert_eq!(status, json!({ "state": "loading" }));
}

/// Which loaded file a record of a document is put on: one that was
/// checked against the key its reference names it by, found by key, UID
/// or path, under that file's settled key; or none.
#[tokio::test]
async fn a_record_lands_on_the_file_its_reference_names_or_on_no_file() {
    let dir = tempdir().expect("temp dir");
    let files = vec![
        // 0, 1: one UID, one size, different bytes. Their key is shared
        // until they are compared, and names neither.
        written(dir.path(), "first.dcm", "1.2.3.9", &[1; 64]),
        written(dir.path(), "second.dcm", "1.2.3.9", &[2; 64]),
        // 2, 3: the same bytes under two names.
        written(dir.path(), "copy-a.dcm", "1.2.3.8", &[3; 64]),
        written(dir.path(), "copy-b.dcm", "1.2.3.8", &[3; 64]),
        // 4, 5: one UID, and neither can be read to tell them apart.
        entry(dir.path(), "gone.dcm", "1.2.3.7"),
        entry(dir.path(), "gone-twin.dcm", "1.2.3.7"),
        // 6, 7, 8: files of their own.
        entry(dir.path(), "alone.dcm", "1.2.3.1"),
        entry(dir.path(), "sized.dcm", "1.2.3.6"),
        entry(dir.path(), "renumbered.dcm", "1.2.3.5"),
    ];
    let session = Session::loading(files);
    let second_bytes = format!("b3:{}", blake3::hash(&[2; 64]).to_hex());
    let mut smaller = reference("sop:1.2.3.6", Some("1.2.3.6"), "sized.dcm");
    smaller["rows"] = json!(99);
    let of_64_bytes = |mut reference: Value| {
        reference["size_bytes"] = json!(64);
        reference
    };
    let references = vec![
        // Row 2: named by its content. Two files carry its UID; the one
        // with those bytes is it.
        of_64_bytes(reference(&second_bytes, Some("1.2.3.9"), "elsewhere.dcm")),
        // Row 3: a copy, by its path.
        of_64_bytes(reference("sop:1.2.3.8", Some("1.2.3.8"), "copy-b.dcm")),
        // Row 4: files that cannot be read, so cannot be checked.
        reference("sop:1.2.3.7", Some("1.2.3.7"), "gone-twin.dcm"),
        // Row 5: found by its key, whatever its path says.
        reference("sop:1.2.3.1", Some("1.2.3.1"), "elsewhere/renamed.dcm"),
        // Row 6: found by its path, but it is another object.
        reference("sop:4.4.4", Some("4.4.4"), "alone.dcm"),
        // Row 7: no loaded file.
        reference("sop:5.5.5", Some("5.5.5"), "nowhere.dcm"),
        // Row 8: another size than the loaded file.
        smaller,
        // Row 9: named by its content, and the file its UID finds cannot
        // be read to compare: a UID alone places nothing.
        reference(OTHER_BYTES, Some("1.2.3.5"), "moved/away.dcm"),
        // Row 10: found by its path and its UID, but the bytes it was
        // written for are not the bytes of either file here.
        {
            let mut changed = of_64_bytes(reference(CHANGED_BYTES, Some("1.2.3.9"), "first.dcm"));
            changed["digest"] = json!(CHANGED_BYTES);
            changed
        },
    ];
    let records: Vec<Value> = [
        second_bytes.as_str(),
        "sop:1.2.3.8",
        "sop:1.2.3.7",
        "sop:1.2.3.1",
        "sop:4.4.4",
        "sop:5.5.5",
        "sop:1.2.3.6",
        OTHER_BYTES,
        CHANGED_BYTES,
    ]
    .iter()
    .enumerate()
    .map(|(n, file)| {
        let n = n as u32 + 1;
        annotation(n, file, "default", rect(n, n, n + 10, n + 10))
    })
    .collect();

    let report = session
        .import(dir.path(), &document_of(references, vec![], records))
        .await
        .expect("import");

    assert_eq!(
        report["counts"],
        json!({
            "files": 3, "annotations_in": 9, "annotations_out": 3,
            "labels_in": 0, "labels_out": 0, "unmatched": 1, "refused": 5,
        })
    );
    assert_eq!(
        report["warnings"],
        json!([
            { "code": "file_unverified", "count": 2, "rows": [{ "row": 4 }, { "row": 9 }] },
            { "code": "evidence_conflict", "count": 2, "rows": [{ "row": 6 }, { "row": 10 }] },
            { "code": "file_unmatched", "count": 1, "rows": [{ "row": 7 }] },
            { "code": "size_mismatch", "count": 1, "rows": [{ "row": 8 }] },
        ])
    );
    assert_eq!(
        report["matched_by"],
        json!({ "key": 2, "sop_instance_uid": 1, "path": 0 })
    );

    let shown = |n: u32| rows(&[[n, n, n + 10, n + 10]], &[]);
    // The record for the second of two files with one UID is on the
    // second, and not on the first.
    assert_eq!(session.rois(1).await, shown(1));
    assert_eq!(session.rois(0).await.num_roi, 0);
    // Copies share a key, and a record on it shows on each of them.
    assert_eq!(session.rois(2).await, shown(2));
    assert_eq!(session.rois(3).await, shown(2));
    assert_eq!(session.rois(6).await, shown(4));
    for index in [4, 5, 7, 8] {
        assert_eq!(session.rois(index).await.num_roi, 0, "file {index}");
    }

    // The document of this session names the second file by the key of
    // its own bytes, which is also its digest.
    let document = session.document().await;
    let second = document["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|file| file["path"] == "second.dcm")
        .expect("the second file");
    let key = second["key"].as_str().expect("key");
    assert!(key.starts_with("b3:"), "{key}");
    assert_eq!(key, second_bytes);
    assert_eq!(second["digest"], key);
    assert_eq!(document["annotations"][0]["file"], key);
    let paths: Vec<&str> = document["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    // A shared key is described once, by the first file that has it.
    assert_eq!(paths, ["second.dcm", "copy-a.dcm", "alone.dcm"]);
}

/// A file without a SOP Instance UID, as a raster is: keyed by its bytes.
fn unnamed(directory: &Path, name: &str, bytes: &[u8]) -> FileEntry {
    let path = directory.join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    written(directory, name, "", bytes)
}

fn content_key(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

/// The masks of a session are drawn for one process, so nothing a masked
/// session shows for a file means anything to another. Its document names
/// files by their bytes and their paths and by nothing else, and loads
/// into any session: masked by other masks, or not masked.
#[tokio::test]
async fn a_masked_sessions_document_names_files_by_content_and_loads_anywhere() {
    let dir = tempdir().expect("temp dir");
    let real = [
        "1.2.826.0.1.3680043.10.777.5",
        "1.2.826.0.1.3680043.10.777.6",
    ];
    // Written once: a file that changes after a session loaded it cannot
    // be compared by that session.
    written(dir.path(), "a.dcm", real[0], b"the bytes of a");
    written(dir.path(), "b.dcm", real[1], b"the bytes of b");
    let files = || {
        let mut files = vec![
            entry(dir.path(), "a.dcm", real[0]),
            entry(dir.path(), "b.dcm", real[1]),
        ];
        for file in &mut files {
            file.patient_id = "MRN-0042".to_string();
            file.patient_name = "DOE^JANE".to_string();
            file.study_instance_uid = "1.2.826.0.1.3680043.10.777.1".to_string();
            file.series_instance_uid = "1.2.826.0.1.3680043.10.777.2".to_string();
        }
        files
    };
    // A masked session of its own, as another process is.
    let masked = || {
        let registry = FileRegistry::masked(Arc::new(Masker::new()));
        for file in files() {
            registry.insert(file);
        }
        registry.mark_scan_complete();
        registry
    };
    let staged = || {
        HashMap::from([
            (0, rows(&[[10, 20, 30, 40]], &[])),
            (1, rows(&[[1, 2, 3, 4]], &[])),
        ])
    };

    let first = Session::over(masked(), AnnotationStore::loading());
    first.stage(staged());
    let catalog = first.text("/api/files").await;
    let shown: Value = serde_json::from_str(&catalog).expect("catalog");
    let mut secrets: Vec<String> = [
        real[0],
        real[1],
        "MRN-0042",
        "DOE^JANE",
        "1.2.826.0.1.3680043.10.777.1",
        "1.2.826.0.1.3680043.10.777.2",
    ]
    .map(str::to_string)
    .to_vec();
    // What this session shows in their place is no use anywhere else, and
    // is not written either.
    for file in shown["files"].as_array().expect("files") {
        for member in [
            "sop_instance_uid",
            "study_instance_uid",
            "series_instance_uid",
            "patient_id",
        ] {
            secrets.push(file[member].as_str().expect("shown").to_string());
        }
    }
    let clean = |what: &str, text: &str| {
        for secret in &secrets {
            assert!(
                !text.contains(secret.as_str()),
                "{what} holds {secret}: {text}"
            );
        }
    };

    let text = first.text("/api/annotations/export.json").await;
    clean("the JSON document", &text);
    clean(
        "the JSON Lines document",
        &first.text("/api/annotations/export.jsonl").await,
    );
    let document: Value = serde_json::from_str(&text).expect("document");
    assert_eq!(document["extensions"]["dcmview"], json!({ "masked": true }));
    for (index, bytes) in [&b"the bytes of a"[..], b"the bytes of b"]
        .into_iter()
        .enumerate()
    {
        let file = &document["files"][index];
        assert_eq!(file["key"], content_key(bytes));
        assert_eq!(file["digest"], file["key"]);
        assert_eq!(file["path"], ["a.dcm", "b.dcm"][index]);
        for member in [
            "sop_instance_uid",
            "study_instance_uid",
            "series_instance_uid",
            "patient_id",
        ] {
            assert!(file[member].is_null(), "{member}: {file}");
        }
        assert_eq!(document["annotations"][index]["file"], file["key"]);
    }
    let views = [first.rois(0).await, first.rois(1).await];

    // Loaded by another masked session and by one that is not masked: the
    // files are found by their paths and matched by their bytes.
    for (what, registry) in [
        ("other masks", masked()),
        ("no masks", FileRegistry::from_files(files())),
    ] {
        let second = Session::over(registry, AnnotationStore::loading());
        let report = second
            .import(dir.path(), &text)
            .await
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        assert_eq!(
            report["matched_by"],
            json!({ "key": 0, "sop_instance_uid": 0, "path": 2 }),
            "{what}: {report}"
        );
        assert_eq!(report["counts"]["refused"], 0, "{what}");
        assert_eq!(report["warnings"], json!([]), "{what}");
        assert_eq!(
            [second.rois(0).await, second.rois(1).await],
            views,
            "{what}"
        );
    }

    // A session without masking writes keys and UIDs, and no such mark.
    let open = Session::over(
        FileRegistry::from_files(files()),
        AnnotationStore::loading(),
    );
    open.stage(staged());
    let unmasked_text = open.text("/api/annotations/export.json").await;
    let unmasked: Value = serde_json::from_str(&unmasked_text).expect("document");
    assert_eq!(unmasked["files"][0]["key"], format!("sop:{}", real[0]));
    assert_eq!(unmasked["files"][0]["sop_instance_uid"], real[0]);
    assert!(unmasked["extensions"].get("dcmview").is_none());

    // Its document loads into a masked session, where a real key and a
    // real UID find nothing: by its paths, each file checked against the
    // key the document names it by. Nothing the session then answers
    // holds what the document said.
    let third = Session::over(masked(), AnnotationStore::loading());
    let report = third
        .import(dir.path(), &unmasked_text)
        .await
        .expect("import");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 0, "path": 2 })
    );
    assert_eq!([third.rois(0).await, third.rois(1).await], views);
    clean("the report", &report.to_string());
    clean(
        "the import status",
        &third.text("/api/annotations/import").await,
    );
    clean(
        "the document",
        &third.text("/api/annotations/export.json").await,
    );

    // And when an import fails, its message says nothing the file said.
    let mut named = annotation(1, &format!("sop:{}", real[0]), "default", rect(1, 2, 3, 4));
    named["class"] = json!("DOE^JANE");
    let invalid = document_of(
        vec![reference(
            &format!("sop:{}", real[0]),
            Some(real[0]),
            "a.dcm",
        )],
        vec![],
        vec![named],
    );
    let fourth = Session::over(masked(), AnnotationStore::loading());
    let error = fourth
        .import(dir.path(), &invalid)
        .await
        .expect_err("invalid");
    clean("the error", &error.to_string());
    let status = fourth.server.get("/api/annotations/import").await.text();
    assert!(status.contains("\"failed\""), "{status}");
    clean("the failed status", &status);
}

/// The reviewer's case: two files with one UID and one size and different
/// bytes, each with its own ROI, written to a document and loaded over one
/// of them alone. The other file's ROIs name bytes that are not here, and
/// land on no file.
#[tokio::test]
async fn rows_of_a_file_that_is_not_loaded_do_not_land_on_one_with_its_uid() {
    let dir = tempdir().expect("temp dir");
    let first = Session::loading(vec![
        written(dir.path(), "one.dcm", "1.2.3.9", &[1; 64]),
        written(dir.path(), "other.dcm", "1.2.3.9", &[2; 64]),
    ]);
    first.stage(HashMap::from([
        (0, rows(&[[1, 1, 2, 2]], &[])),
        (1, rows(&[[3, 3, 4, 4], [5, 5, 6, 6]], &[])),
    ]));
    let text = first.text("/api/annotations/export.json").await;
    let document: Value = serde_json::from_str(&text).expect("document");
    assert_eq!(document["files"][0]["key"], content_key(&[1; 64]));
    assert_eq!(document["files"][1]["key"], content_key(&[2; 64]));

    // Only the first file is loaded. Its UID is its own here, so nothing
    // made the viewer hash it: the import does, to compare.
    let alone = Session::loading(vec![written(dir.path(), "one.dcm", "1.2.3.9", &[1; 64])]);
    let report = alone.import(dir.path(), &text).await.expect("import");
    assert_eq!(alone.rois(0).await, rows(&[[1, 1, 2, 2]], &[]));
    assert_eq!(
        report["counts"],
        json!({
            "files": 1, "annotations_in": 3, "annotations_out": 1,
            "labels_in": 0, "labels_out": 0, "unmatched": 0, "refused": 2,
        })
    );
    assert_eq!(
        report["warnings"],
        json!([{ "code": "evidence_conflict", "count": 1, "rows": [{ "row": 3 }] }])
    );
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 1, "path": 0 })
    );
}

/// A document's paths are below the folder its session loaded. They find
/// their files when a subfolder of it, a parent of it or a copy of it
/// elsewhere is loaded, because a file is named by the end of its path and
/// matched by its bytes; and of several files a path could name, the one
/// with those bytes is the one.
#[tokio::test]
async fn a_document_finds_its_files_in_a_subfolder_a_parent_or_a_copy() {
    let dir = tempdir().expect("temp dir");
    let root = dir.path().join("data");
    let tree = |root: &Path| {
        vec![
            unnamed(root, "sub/a.png", b"bytes of a"),
            unnamed(root, "sub/deep/b.png", b"bytes of b"),
            unnamed(root, "other/c.png", b"bytes of c"),
        ]
    };
    let first = Session::loading(tree(&root));
    first.stage(HashMap::from([
        (0, rows(&[[1, 1, 2, 2]], &[])),
        (1, rows(&[[3, 3, 4, 4]], &[])),
        (2, rows(&[[5, 5, 6, 6]], &[])),
    ]));
    let text = first.text("/api/annotations/export.json").await;
    let document: Value = serde_json::from_str(&text).expect("document");
    let paths: Vec<&str> = document["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert_eq!(paths, ["sub/a.png", "sub/deep/b.png", "other/c.png"]);
    assert_eq!(document["files"][0]["key"], content_key(b"bytes of a"));
    let csv_rows = |session_csv: String| session_csv.lines().count() - 1;

    // A subfolder: two of the three files, whose common folder is deeper.
    let sub = Session::loading(vec![
        unnamed(&root, "sub/a.png", b"bytes of a"),
        unnamed(&root, "sub/deep/b.png", b"bytes of b"),
    ]);
    let report = sub.import(dir.path(), &text).await.expect("a subfolder");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 0, "path": 2 })
    );
    assert_eq!(report["counts"]["unmatched"], 1);
    assert_eq!(sub.rois(0).await, rows(&[[1, 1, 2, 2]], &[]));
    assert_eq!(sub.rois(1).await, rows(&[[3, 3, 4, 4]], &[]));

    // A parent: the same tree and a file beside it.
    let mut wider = tree(&root);
    wider.push(unnamed(dir.path(), "beside/d.png", b"bytes of d"));
    let parent = Session::loading(wider);
    let report = parent.import(dir.path(), &text).await.expect("a parent");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 0, "path": 3 })
    );
    assert_eq!(csv_rows(parent.csv().await), 3);

    // A copy elsewhere, beside a folder whose file of the same name holds
    // other bytes and is loaded first: the path names both, and the record
    // is on the one with its bytes.
    let elsewhere = dir.path().join("moved");
    let mut mixed = vec![unnamed(
        &dir.path().join("decoy"),
        "sub/a.png",
        b"other bytes!",
    )];
    mixed.extend(tree(&elsewhere));
    let moved = Session::loading(mixed);
    let report = moved.import(dir.path(), &text).await.expect("a copy");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 0, "path": 3 })
    );
    assert_eq!(report["warnings"], json!([]));
    assert_eq!(moved.rois(0).await.num_roi, 0, "the file of the same name");
    assert_eq!(moved.rois(1).await, rows(&[[1, 1, 2, 2]], &[]));

    // The file is there and its bytes are not the ones the document was
    // written for: found, compared, refused.
    let changed = Session::loading(vec![unnamed(&root, "sub/a.png", b"edited ..")]);
    let report = changed.import(dir.path(), &text).await.expect("changed");
    assert_eq!(report["counts"]["annotations_out"], 0);
    assert_eq!(report["warnings"][0]["code"], "evidence_conflict");
}

/// Byte-identical files each keep the rows written for them through a
/// document: a reload shows on each copy what the first session showed, in
/// the same order, whichever copies have been looked at, and a copy that is
/// not loaded takes its rows with it.
#[tokio::test]
async fn copies_keep_their_own_rows_through_a_document() {
    let dir = tempdir().expect("temp dir");
    let copies = |names: &[&str]| -> Vec<FileEntry> {
        names
            .iter()
            .map(|name| written(dir.path(), name, "1.2.3.8", &[3; 64]))
            .collect()
    };
    let first = Session::loading(copies(&["a.dcm", "b.dcm", "c.dcm"]));
    first.stage(HashMap::from([
        (0, rows(&[[1, 2, 3, 4]], &[])),
        (1, rows(&[[5, 6, 7, 8], [9, 9, 12, 12]], &[&[0], &[1]])),
    ]));
    // A rectangle drawn on the second copy before any row is a record: it
    // belongs to the image, shows on every copy, and stands after each
    // copy's own rows.
    first
        .op(
            Some(1),
            create(1, "sop:1.2.3.8", "default", rect(20, 20, 30, 30)),
        )
        .await;
    let views = [
        first.rois(0).await,
        first.rois(1).await,
        first.rois(2).await,
    ];
    assert_eq!(views[0].roi_coords, [[1, 2, 3, 4], [20, 20, 30, 30]]);
    assert_eq!(views[2].roi_coords, [[20, 20, 30, 30]]);
    let csv = first.csv().await;
    let text = first.text("/api/annotations/export.jsonl").await;
    // What a record is, apart from its stamps.
    let told = |document: &Value| -> Vec<Value> {
        document["annotations"]
            .as_array()
            .expect("annotations")
            .iter()
            .map(|record| json!([record["file"], record["geometry"], record["extensions"]]))
            .collect()
    };
    let written_first = told(&first.document().await);
    let rows_of: Vec<&Value> = written_first.iter().map(|record| &record[2]).collect();
    // The second copy's rows became records when it was drawn on, the
    // first copy's when the document was written.
    assert_eq!(
        rows_of,
        [
            &json!({ "embed": { "row_of": "b.dcm", "imported": true } }),
            &json!({ "embed": { "row_of": "b.dcm", "imported": true } }),
            &json!({}),
            &json!({ "embed": { "row_of": "a.dcm", "imported": true } }),
        ]
    );

    // All three copies, none of them looked at.
    let second = Session::loading(copies(&["a.dcm", "b.dcm", "c.dcm"]));
    let report = second.import(dir.path(), &text).await.expect("import");
    assert_eq!(report["counts"]["annotations_out"], 4);
    assert_eq!(
        [
            second.rois(0).await,
            second.rois(1).await,
            second.rois(2).await
        ],
        views
    );
    assert_eq!(second.csv().await, csv);
    // And written again, it is the same document but for its stamps.
    assert_eq!(told(&second.document().await), written_first);

    // Only the third and the first copy are loaded: the second copy's rows
    // are for a file that is not here.
    let fewer = Session::loading(copies(&["c.dcm", "a.dcm"]));
    let report = fewer.import(dir.path(), &text).await.expect("import");
    assert_eq!(
        (
            &report["counts"]["annotations_out"],
            &report["counts"]["unmatched"]
        ),
        (&json!(2), &json!(2))
    );
    assert_eq!(fewer.rois(0).await.roi_coords, [[20, 20, 30, 30]]);
    assert_eq!(
        fewer.rois(1).await.roi_coords,
        [[1, 2, 3, 4], [20, 20, 30, 30]]
    );
}

/// A store that takes labels: a schema with one field for files and frames.
fn labelled_store() -> AnnotationStore {
    let mut schema = LabelSchema::implicit();
    schema.fields = serde_json::from_value(json!([{
        "id": "quality", "name": "Quality", "type": "category",
        "options": [{ "id": "good", "name": "Good" }, { "id": "bad", "name": "Bad" }],
        "applies_to": ["file", "frame"],
    }]))
    .expect("field");
    AnnotationStore::with_backend(MemoryBackend::new(MemoryConfig {
        author: Author::parse("user:reader").expect("author"),
        schema: Some(schema),
    }))
}

/// Labels go through a document as annotations do: each names its file by
/// the key of that file's reference, which in a masked session's document
/// is not the key the store holds, and is put on the file that reference
/// finds. A label's `score` and `derived_from` are not kept, and the
/// report says of how many.
#[tokio::test]
async fn labels_go_through_a_document_with_the_files_they_are_on() {
    let dir = tempdir().expect("temp dir");
    let files = || {
        vec![
            written(dir.path(), "a.dcm", "1.2.3.1", b"the bytes of a"),
            written(dir.path(), "b.dcm", "1.2.3.2", b"the bytes of b"),
        ]
    };
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    for file in files() {
        registry.insert(file);
    }
    registry.mark_scan_complete();
    let first = Session::over(registry, labelled_store());
    let label = |n: u32, target: Value, value: &str| -> OpEnvelope {
        serde_json::from_value(json!({
            "op_id": id(1_000 + n), "actor": "user:test", "ts": "2026-10-10T08:00:00.000Z",
            "op": {
                "type": "set_label", "id": id(n), "base_rev": null, "target": target,
                "field": "quality", "layer": "default", "before": null, "after": value,
            },
        }))
        .expect("envelope")
    };
    let size = ImageSize {
        columns: 120,
        rows: 100,
        frames: 4,
    };
    let key = |text: &str| FileKey::parse(text).expect("file key");
    // A label is made on a file whose key is settled, as a door does it.
    for index in 0..2 {
        first.registry.ensure_key(index).await.expect("key");
    }
    let results = first
        .store
        .backend()
        .apply(
            vec![
                label(1, json!({ "file": "sop:1.2.3.1" }), "good"),
                label(2, json!({ "frame": "sop:1.2.3.2", "index": 3 }), "bad"),
            ],
            &BTreeMap::from([(key("sop:1.2.3.1"), size), (key("sop:1.2.3.2"), size)]),
        )
        .expect("store");
    assert!(
        results
            .iter()
            .all(|result| matches!(result, ApplyResult::Ok { .. })),
        "{results:?}"
    );

    let mut document = first.document().await;
    assert_eq!(document["files"].as_array().expect("files").len(), 2);
    let of = |document: &Value, n: usize| document["labels"][n]["target"].clone();
    assert_eq!(
        of(&document, 0),
        json!({ "file": content_key(b"the bytes of a") })
    );
    assert_eq!(
        of(&document, 1),
        json!({ "frame": content_key(b"the bytes of b"), "index": 3 })
    );

    // Loaded where nothing is masked, with a score on one label.
    document["labels"][1]["score"] = json!(0.5);
    let second = Session::over(FileRegistry::from_files(files()), labelled_store());
    let report = second
        .import(dir.path(), &document.to_string())
        .await
        .expect("import");
    assert_eq!(
        (
            &report["counts"]["labels_in"],
            &report["counts"]["labels_out"]
        ),
        (&json!(2), &json!(2))
    );
    assert_eq!(
        report["losses"],
        json!([{ "code": "label_members_dropped", "count": 1, "rows": [] }])
    );
    let again = second.document().await;
    assert_eq!(of(&again, 0), json!({ "file": "sop:1.2.3.1" }));
    assert_eq!(of(&again, 1), json!({ "frame": "sop:1.2.3.2", "index": 3 }));
    for n in 0..2 {
        for member in ["id", "field", "layer", "value"] {
            assert_eq!(
                again["labels"][n][member], document["labels"][n][member],
                "{member}"
            );
        }
        assert!(again["labels"][n].get("score").is_none_or(Value::is_null));
    }
}

/// A document holds records, and a record holds a settled key: the rows of
/// a file that cannot be given one cannot be written, and the export says
/// so instead of leaving them out.
#[tokio::test]
async fn a_document_export_needs_a_key_for_every_file_with_rows() {
    let dir = tempdir().expect("temp dir");
    let session = Session::loading(vec![
        entry(dir.path(), "ok.dcm", "1.2.3.1"),
        // One UID and neither file can be read: no key for either.
        entry(dir.path(), "gone.dcm", "1.2.3.7"),
        entry(dir.path(), "gone-twin.dcm", "1.2.3.7"),
    ]);
    session.stage(HashMap::from([
        (0, rows(&[[1, 2, 3, 4]], &[])),
        (2, rows(&[[5, 6, 7, 8]], &[])),
    ]));
    let csv = session.csv().await;
    assert_eq!(csv.lines().count(), 3, "the CSV writes both files' rows");

    for path in [
        "/api/annotations/export.json",
        "/api/annotations/export.jsonl",
    ] {
        let response = session.server.get(path).await;
        assert_eq!(
            response.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}: {}",
            response.text()
        );
        assert_eq!(
            response.json::<Value>()["code"],
            "file_key_unavailable",
            "{path}"
        );
    }
    // Nothing a view shows changed, and the CSV is what it was.
    assert_eq!(session.rois(0).await, rows(&[[1, 2, 3, 4]], &[]));
    assert_eq!(session.rois(2).await, rows(&[[5, 6, 7, 8]], &[]));
    assert_eq!(session.csv().await, csv);

    // Copies with the same bytes, each with a row of its own: the export
    // hashes them for their key and stores both rows under it, and each
    // copy still shows and exports the row that was written for it.
    let session = Session::loading(vec![
        written(dir.path(), "copy-a.dcm", "1.2.3.8", &[3; 64]),
        written(dir.path(), "copy-b.dcm", "1.2.3.8", &[3; 64]),
    ]);
    session.stage(HashMap::from([
        (0, rows(&[[1, 2, 3, 4]], &[])),
        (1, rows(&[[5, 6, 7, 8]], &[])),
    ]));
    let csv = session.csv().await;
    let document = session.document().await;
    assert_eq!(document["files"].as_array().map(Vec::len), Some(1));
    assert_eq!(document["files"][0]["path"], "copy-a.dcm");
    // Copies keep the key of their UID; the digest they were compared by
    // is beside it.
    assert_eq!(document["files"][0]["key"], "sop:1.2.3.8");
    assert!(document["files"][0]["digest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("b3:")));
    let boxes: Vec<&Value> = document["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| &record["geometry"])
        .collect();
    assert_eq!(boxes, [&rect(2, 1, 4, 3), &rect(6, 5, 8, 7)]);
    // Each record says which copy's row it is.
    let rows_of: Vec<&Value> = document["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| &record["extensions"]["embed"])
        .collect();
    assert_eq!(
        rows_of,
        [
            &json!({ "row_of": "copy-a.dcm", "imported": true }),
            &json!({ "row_of": "copy-b.dcm", "imported": true }),
        ]
    );
    assert_eq!(session.rois(0).await, rows(&[[1, 2, 3, 4]], &[]));
    assert_eq!(session.rois(1).await, rows(&[[5, 6, 7, 8]], &[]));
    assert_eq!(session.csv().await, csv);

    // A file without rows is not asked for a key: with the rows of the
    // unreadable file gone, the export goes through.
    let session = Session::loading(vec![
        entry(dir.path(), "ok.dcm", "1.2.3.1"),
        entry(dir.path(), "gone.dcm", "1.2.3.7"),
        entry(dir.path(), "gone-twin.dcm", "1.2.3.7"),
    ]);
    session.stage(HashMap::from([(0, rows(&[[1, 2, 3, 4]], &[]))]));
    let document = session.document().await;
    assert_eq!(document["annotations"].as_array().map(Vec::len), Some(1));
    assert_eq!(document["files"].as_array().map(Vec::len), Some(1));
}

/// Redaction boxes are not annotations: no export holds one.
#[tokio::test]
async fn redaction_boxes_are_in_no_document() {
    let dir = tempdir().expect("temp dir");
    let session = Session::plain(vec![entry(dir.path(), "a.dcm", "1.2.3.1")]);
    session
        .server
        .put("/api/file/0/redactions")
        .json(&json!({ "num_roi": 1, "roi_coords": [[1, 2, 30, 40]], "roi_frames": [] }))
        .await
        .assert_status_ok();

    let document = session.document().await;
    assert_eq!(document["annotations"], json!([]));
    assert_eq!(document["files"], json!([]));
    assert_eq!(document["labels"], json!([]));
    let lines = session.text("/api/annotations/export.jsonl").await;
    // The header, the default layer every session has, and the end.
    assert_eq!(lines.lines().count(), 3, "{lines}");
    assert!(!lines.contains("\"annotation\""));
}
