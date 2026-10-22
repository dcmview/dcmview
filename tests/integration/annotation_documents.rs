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
use dcmview::server::annotations::{
    export_native_within, import_embed_rows, import_native, import_native_within, AnnotationError,
    NativeForm,
};
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

    /// A session over `files` whose `--annotations` document has not been
    /// imported yet, as the viewer makes one: writes wait for the import.
    fn loading_document(files: Vec<FileEntry>) -> Self {
        Self::over(
            FileRegistry::from_files(files),
            AnnotationStore::loading_document(),
        )
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

    // A document without a record is an import that brought nothing.
    let session = Session::loading(files());
    let nothing = document_of(
        vec![reference("sop:9.9.9", Some("9.9.9"), "nowhere.dcm")],
        vec![],
        vec![],
    );
    let report = session
        .import(dir.path(), &nothing)
        .await
        .expect("no record");
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
/// session shows for a file means anything to another, and folder and file
/// names often say whose the files are. Its document names files by their
/// bytes and by nothing else, names no user, and loads into any session:
/// masked by other masks, or not masked.
#[tokio::test]
async fn a_masked_sessions_document_names_files_by_content_and_loads_anywhere() {
    let dir = tempdir().expect("temp dir");
    let real = [
        "1.2.826.0.1.3680043.10.777.5",
        "1.2.826.0.1.3680043.10.777.6",
    ];
    // Written once: a file that changes after a session loaded it cannot
    // be compared by that session.
    let names = [
        "ward-7-north/DOE_JANE_1961/scan-a.dcm",
        "ward-7-north/scan-b.dcm",
    ];
    std::fs::create_dir_all(dir.path().join("ward-7-north/DOE_JANE_1961")).expect("folders");
    written(dir.path(), names[0], real[0], b"the bytes of a");
    written(dir.path(), names[1], real[1], b"the bytes of b");
    let files = || {
        let mut files = vec![
            entry(dir.path(), names[0], real[0]),
            entry(dir.path(), names[1], real[1]),
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
    // Where the files are, and who and where the viewer runs.
    let mut places: Vec<String> = dir
        .path()
        .join(names[0])
        .components()
        .chain(Path::new(names[1]).components())
        .filter_map(|part| match part {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .filter(|part| part.len() >= 4)
        .collect();
    if let Ok(host) = std::process::Command::new("hostname").output() {
        let host = String::from_utf8_lossy(&host.stdout).trim().to_string();
        places.extend(host.split('.').next().map(str::to_string));
        places.push(host);
    }
    places.retain(|place| place.len() >= 4 && !"user:local import:embed".contains(place.as_str()));
    for variable in ["USER", "USERNAME"] {
        if let Ok(name) = std::env::var(variable) {
            if name != "local" {
                places.push(format!("user:{name}"));
            }
        }
    }
    let unplaced = |what: &str, text: &str| {
        for place in &places {
            assert!(
                !text.contains(place.as_str()),
                "{what} holds {place}: {text}"
            );
        }
    };
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
    unplaced("the JSON document", &text);
    let lines = first.text("/api/annotations/export.jsonl").await;
    clean("the JSON Lines document", &lines);
    unplaced("the JSON Lines document", &lines);
    let document: Value = serde_json::from_str(&text).expect("document");
    assert_eq!(document["extensions"]["dcmview"], json!({ "masked": true }));
    assert_eq!(document["layers"][0]["source"]["author"], "user:local");
    for (index, bytes) in [&b"the bytes of a"[..], b"the bytes of b"]
        .into_iter()
        .enumerate()
    {
        let file = &document["files"][index];
        assert_eq!(file["key"], content_key(bytes));
        assert_eq!(file["digest"], file["key"]);
        // What is left of a file: its bytes' digest and size, what kind
        // of image it is and how large, and the name the session shows.
        let mut said: Vec<(&String, &Value)> = file
            .as_object()
            .expect("file")
            .iter()
            .filter(|(_, value)| !value.is_null() && **value != json!([]))
            .collect();
        said.retain(|(member, _)| *member != "space");
        let expected = json!({
            "key": content_key(bytes), "digest": content_key(bytes), "kind": "dicom",
            "format": "dicom", "path": format!("File {}", index + 1), "size_bytes": 14,
            "rows": 100, "columns": 120, "frames": 4,
            "sop_class_uid": file["sop_class_uid"],
        });
        assert_eq!(
            said,
            expected
                .as_object()
                .expect("object")
                .iter()
                .collect::<Vec<_>>(),
            "{file}"
        );
        assert_eq!(document["annotations"][index]["file"], file["key"]);
    }
    let views = [first.rois(0).await, first.rois(1).await];

    // Loaded by another masked session and by one that is not masked: the
    // files are found among those of their size, by their bytes.
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
            json!({ "key": 2, "sop_instance_uid": 0, "path": 0 }),
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
            "scan-a.dcm",
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
    // Each row is on a key one file has, so none says which file it is of.
    for record in document["annotations"].as_array().expect("annotations") {
        assert_eq!(record["extensions"], json!({}));
    }

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

    // A reference that names the file by its UID and also says what its
    // bytes were is held to both.
    let naming = |digest: &[u8; 64]| {
        let mut named = reference("sop:1.2.3.9", Some("1.2.3.9"), "one.dcm");
        named["size_bytes"] = json!(64);
        named["digest"] = json!(content_key(digest));
        document_of(
            vec![named],
            vec![],
            vec![annotation(1, "sop:1.2.3.9", "default", rect(7, 7, 9, 9))],
        )
    };
    let other = Session::loading(vec![written(dir.path(), "one.dcm", "1.2.3.9", &[1; 64])]);
    let report = other
        .import(dir.path(), &naming(&[2; 64]))
        .await
        .expect("import");
    assert_eq!(report["counts"]["refused"], 1);
    assert_eq!(report["warnings"][0]["code"], "evidence_conflict");
    assert_eq!(other.rois(0).await.num_roi, 0);
    let same = Session::loading(vec![written(dir.path(), "one.dcm", "1.2.3.9", &[1; 64])]);
    let report = same
        .import(dir.path(), &naming(&[1; 64]))
        .await
        .expect("import");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 1, "sop_instance_uid": 0, "path": 0 })
    );
    assert_eq!(same.rois(0).await.roi_coords, [[7, 7, 9, 9]]);
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

    // What a copy shows does not wait for another copy to be looked at:
    // a rectangle drawn on the first of three shows on the third at once.
    let drawn = Session::plain(copies(&["a.dcm", "b.dcm", "c.dcm"]));
    drawn
        .op(
            Some(0),
            create(2, "sop:1.2.3.8", "default", rect(4, 4, 8, 8)),
        )
        .await;
    let only = drawn.text("/api/annotations/export.json").await;
    let third = Session::loading(copies(&["a.dcm", "b.dcm", "c.dcm"]));
    third.import(dir.path(), &only).await.expect("import");
    assert_eq!(third.rois(2).await.roi_coords, [[4, 4, 8, 8]]);

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

/// A file that is loaded is found by what a document says of its bytes
/// when nothing else finds it: the document names the copy that is not
/// loaded, by a path no loaded file has, and the file has no UID.
#[tokio::test]
async fn a_loaded_copy_is_found_by_its_bytes_when_its_path_names_another() {
    let dir = tempdir().expect("temp dir");
    let both = || {
        vec![
            unnamed(dir.path(), "first/scan.png", b"the same bytes"),
            unnamed(dir.path(), "second/scan.png", b"the same bytes"),
            // Of the same size and not the same bytes.
            unnamed(dir.path(), "second/else.png", b"not those ones"),
        ]
    };
    let first = Session::plain(both());
    let key = content_key(b"the same bytes");
    first.registry.ensure_key(0).await.expect("key");
    first
        .op(Some(0), create(1, &key, "default", rect(4, 4, 8, 8)))
        .await;
    let text = first.text("/api/annotations/export.json").await;
    let document: Value = serde_json::from_str(&text).expect("document");
    assert_eq!(document["files"][0]["path"], "first/scan.png");

    // Only the second folder is loaded, the other file of that size first.
    let second = Session::loading(vec![
        unnamed(dir.path(), "second/else.png", b"not those ones"),
        unnamed(dir.path(), "second/scan.png", b"the same bytes"),
    ]);
    let report = second.import(dir.path(), &text).await.expect("import");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 1, "sop_instance_uid": 0, "path": 0 }),
        "{report}"
    );
    assert_eq!(report["warnings"], json!([]));
    assert_eq!(second.rois(1).await.roi_coords, [[4, 4, 8, 8]]);
    assert_eq!(second.rois(0).await.num_roi, 0);

    // What the document says of the bytes is held to: a reference of
    // another size, or of another format, finds no file by content.
    for (member, value) in [("size_bytes", json!(15)), ("format", json!("jpeg"))] {
        let mut other = document.clone();
        other["files"][0][member] = value;
        let third = Session::loading(vec![unnamed(
            dir.path(),
            "second/scan.png",
            b"the same bytes",
        )]);
        let report = third
            .import(dir.path(), &other.to_string())
            .await
            .expect("import");
        assert_eq!(report["counts"]["unmatched"], 1, "{member}: {report}");
    }
}

/// A document never holds an absolute path. Files that share no folder
/// worth naming are named by their file names, and are found by them.
#[tokio::test]
async fn files_from_unrelated_folders_are_named_by_their_file_names() {
    let files = || {
        vec![
            entry(Path::new("/alpha/study"), "one.dcm", "1.2.3.1"),
            entry(Path::new("/beta/other/deep"), "two.dcm", "1.2.3.2"),
        ]
    };
    let first = Session::loading(files());
    first.stage(HashMap::from([
        (0, rows(&[[1, 2, 3, 4]], &[])),
        (1, rows(&[[5, 6, 7, 8]], &[])),
    ]));
    let text = first.text("/api/annotations/export.json").await;
    let document: Value = serde_json::from_str(&text).expect("document");
    let paths: Vec<&str> = document["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert_eq!(paths, ["one.dcm", "two.dcm"]);
    assert!(!text.contains("alpha") && !text.contains("beta"), "{text}");

    // Named by an argument below the working directory and by an absolute
    // one, the files of one folder are one tree.
    let here = std::env::current_dir().expect("working directory");
    let mixed = Session::loading(vec![
        entry(Path::new("case/left"), "one.dcm", "1.2.3.1"),
        entry(&here.join("case/right"), "two.dcm", "1.2.3.2"),
    ]);
    mixed.stage(HashMap::from([
        (0, rows(&[[1, 2, 3, 4]], &[])),
        (1, rows(&[[5, 6, 7, 8]], &[])),
    ]));
    let document = mixed.document().await;
    assert_eq!(document["files"][0]["path"], "left/one.dcm");
    assert_eq!(document["files"][1]["path"], "right/two.dcm");

    let dir = tempdir().expect("temp dir");
    let second = Session::loading(files());
    let report = second.import(dir.path(), &text).await.expect("import");
    assert_eq!(report["counts"]["annotations_out"], 2);
    assert_eq!(second.rois(1).await.roi_coords, [[5, 6, 7, 8]]);
}

/// What finds a file for a reference, case by case: an absolute path is
/// the path of one file and not the end of others'; a key in the form a
/// masked session shows names its file there; and the member that says
/// which copy a row is of binds only in its own shape, only to a file
/// that has the record's key.
#[tokio::test]
async fn references_and_rows_are_held_to_what_they_say() {
    let dir = tempdir().expect("temp dir");

    // An absolute path that is the file's own finds it. Its last
    // component alone would find it too, and that is not how it is read.
    let raster = Session::loading(vec![unnamed(dir.path(), "kept/scan.png", b"raster bytes")]);
    let key = content_key(b"raster bytes");
    let mut named = reference(
        &key,
        None,
        &dir.path().join("kept/scan.png").to_string_lossy(),
    );
    named["size_bytes"] = json!(12);
    let by_own_path = document_of(
        vec![named.clone()],
        vec![],
        vec![annotation(1, &key, "default", rect(1, 1, 2, 2))],
    );
    let report = raster
        .import(dir.path(), &by_own_path)
        .await
        .expect("import");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 0, "path": 1 }),
        "{report}"
    );
    // Another folder's absolute path with the same end is another file's:
    // the file here is found by its bytes, not by that path.
    named["path"] = json!("/elsewhere/kept/scan.png");
    let by_other_path = document_of(
        vec![named],
        vec![],
        vec![annotation(1, &key, "default", rect(1, 1, 2, 2))],
    );
    let raster = Session::loading(vec![unnamed(dir.path(), "kept/scan.png", b"raster bytes")]);
    let report = raster
        .import(dir.path(), &by_other_path)
        .await
        .expect("import");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 1, "sop_instance_uid": 0, "path": 0 }),
        "{report}"
    );

    // Nothing depends on the order files were found in. A path that names
    // two copies names the one whose own path sorts first, and a document
    // describes that copy, whichever was found first.
    for name in ["q/a.dcm", "p/a.dcm"] {
        std::fs::create_dir_all(dir.path().join(name).parent().expect("folder")).expect("folder");
        written(dir.path(), name, "1.2.3.8", &[3; 64]);
    }
    let found = Session::loading(vec![
        entry(dir.path(), "q/a.dcm", "1.2.3.8"),
        entry(dir.path(), "p/a.dcm", "1.2.3.8"),
    ]);
    let mut bound = annotation(1, "sop:1.2.3.8", "default", rect(1, 1, 2, 2));
    bound["extensions"] = json!({ "embed": { "row_of": "a.dcm" } });
    let mut named = reference("sop:1.2.3.8", Some("1.2.3.8"), "a.dcm");
    named["size_bytes"] = json!(64);
    let report = found
        .import(dir.path(), &document_of(vec![named], vec![], vec![bound]))
        .await
        .expect("import");
    assert_eq!(report["counts"]["annotations_out"], 1, "{report}");
    assert_eq!(found.rois(0).await.num_roi, 0, "the copy found first");
    assert_eq!(found.rois(1).await.num_roi, 1);
    let written_again = found.document().await;
    assert_eq!(written_again["files"][0]["path"], "p/a.dcm");
    assert_eq!(
        written_again["annotations"][0]["extensions"]["embed"]["row_of"],
        "p/a.dcm"
    );

    // A path that names more files than are compared finds none of them,
    // and the report says that this is why.
    let alike = Session::loading(
        (0..70)
            .map(|n| {
                entry(
                    &dir.path().join(format!("study-{n}")),
                    "image.dcm",
                    &format!("1.2.3.{n}"),
                )
            })
            .collect(),
    );
    let vague = document_of(
        vec![reference("sop:9.9.9", Some("9.9.9"), "image.dcm")],
        vec![],
        vec![annotation(1, "sop:9.9.9", "default", rect(1, 1, 2, 2))],
    );
    let report = alike.import(dir.path(), &vague).await.expect("import");
    assert_eq!(
        report["warnings"],
        json!([{ "code": "file_ambiguous", "count": 1, "rows": [{ "row": 2 }] }])
    );
    assert_eq!(
        (&report["counts"]["refused"], &report["counts"]["unmatched"]),
        (&json!(1), &json!(0))
    );

    // A masked session shows keys of its own, and takes them back.
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    registry.insert(entry(
        dir.path(),
        "masked.dcm",
        "1.2.826.0.1.3680043.10.777.9",
    ));
    registry.mark_scan_complete();
    let masked = Session::over(registry, AnnotationStore::loading());
    let catalog: Value = serde_json::from_str(&masked.text("/api/files").await).expect("catalog");
    let shown_uid = catalog["files"][0]["sop_instance_uid"]
        .as_str()
        .expect("shown UID");
    let shown_key = format!("sop:{shown_uid}");
    let shown = document_of(
        vec![reference(&shown_key, Some(shown_uid), "not-its-name.dcm")],
        vec![],
        vec![annotation(1, &shown_key, "default", rect(1, 1, 2, 2))],
    );
    let report = masked.import(dir.path(), &shown).await.expect("import");
    assert_eq!(
        report["matched_by"],
        json!({ "key": 1, "sop_instance_uid": 0, "path": 0 }),
        "{report}"
    );
    assert_eq!(masked.rois(0).await.num_roi, 1);

    // Rows of copies. Two copies of one image and a file of its own.
    let files = || {
        vec![
            written(dir.path(), "a.dcm", "1.2.3.8", &[3; 64]),
            written(dir.path(), "b.dcm", "1.2.3.8", &[3; 64]),
            written(dir.path(), "alone.dcm", "1.2.3.7", &[7; 64]),
        ]
    };
    let record = |n: u32, embed: Value| {
        let mut record = annotation(n, "sop:1.2.3.8", "default", rect(n, n, n + 5, n + 5));
        record["extensions"] = json!({ "embed": embed, "other": { "kept": true } });
        record
    };
    let of_64_bytes = |mut reference: Value| {
        reference["size_bytes"] = json!(64);
        reference
    };
    let text = document_of(
        vec![
            of_64_bytes(reference("sop:1.2.3.8", Some("1.2.3.8"), "a.dcm")),
            of_64_bytes(reference("sop:1.2.3.7", Some("1.2.3.7"), "alone.dcm")),
        ],
        vec![],
        vec![
            // Bound to the second copy by its path; and to one of two
            // copies, which is the one whose path sorts first.
            record(1, json!({ "row_of": "b.dcm", "imported": true })),
            record(2, json!({ "copies": 2 })),
            // Names a loaded file that does not have the record's key, a
            // file that is not loaded: left out.
            record(3, json!({ "row_of": "alone.dcm" })),
            record(4, json!({ "row_of": "c.dcm" })),
            // Not the member's shape: kept as it is, on every copy.
            record(5, json!({ "copy": 2 })),
            // Not the member's shape: kept as it is, on every copy.
            record(6, json!({ "row_of": "b.dcm", "more": 1 })),
            record(7, json!({ "row_of": 3 })),
            record(8, json!({ "row_of": "b.dcm", "copies": 2 })),
            record(9, json!({ "copies": -1 })),
            record(10, json!("b.dcm")),
        ],
    );
    let session = Session::loading(files());
    let report = session.import(dir.path(), &text).await.expect("import");
    assert_eq!(
        (
            &report["counts"]["annotations_out"],
            &report["counts"]["unmatched"]
        ),
        (&json!(8), &json!(2)),
        "{report}"
    );
    assert_eq!(
        report["losses"],
        json!([{ "code": "copy_placement_not_kept", "count": 1, "rows": [] }])
    );
    let boxes = |n: u32| [n, n, n + 5, n + 5];
    assert_eq!(
        session.rois(0).await.roi_coords,
        [
            boxes(2),
            boxes(5),
            boxes(6),
            boxes(7),
            boxes(8),
            boxes(9),
            boxes(10)
        ]
    );
    assert_eq!(
        session.rois(1).await.roi_coords,
        [
            boxes(1),
            boxes(5),
            boxes(6),
            boxes(7),
            boxes(8),
            boxes(9),
            boxes(10)
        ]
    );
    assert_eq!(session.rois(2).await.num_roi, 0);
    // The member that bound a row is taken out, and written again from
    // the binding; one that bound nothing is still there.
    let again = session.document().await;
    let extensions: Vec<&Value> = again["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| &record["extensions"])
        .collect();
    // Loaded where the image has one copy, a row is bound to it and is no
    // longer the row of one of several: the member is gone.
    let single = document_of(
        vec![of_64_bytes(reference(
            "sop:1.2.3.8",
            Some("1.2.3.8"),
            "b.dcm",
        ))],
        vec![],
        vec![record(1, json!({ "row_of": "b.dcm", "imported": true }))],
    );
    let one = Session::loading(vec![written(dir.path(), "b.dcm", "1.2.3.8", &[3; 64])]);
    one.import(dir.path(), &single).await.expect("import");
    assert_eq!(
        one.document().await["annotations"][0]["extensions"],
        json!({ "other": { "kept": true } })
    );
    let bound = |imported: bool| json!({ "embed": { "row_of": "b.dcm", "imported": imported }, "other": { "kept": true } });
    let unbound = |embed: Value| json!({ "embed": embed, "other": { "kept": true } });
    assert_eq!(
        extensions,
        [
            &bound(true),
            &json!({ "embed": { "row_of": "a.dcm", "imported": false }, "other": { "kept": true } }),
            &unbound(json!({ "copy": 2 })),
            &unbound(json!({ "row_of": "b.dcm", "more": 1 })),
            &unbound(json!({ "row_of": 3 })),
            &unbound(json!({ "row_of": "b.dcm", "copies": 2 })),
            &unbound(json!({ "copies": -1 })),
            &unbound(json!("b.dcm")),
        ]
    );
}

/// What a session may write and what downloads may hold are bounded: a
/// session with more records than a document may hold is refused as
/// such, and a document is not kept for sending while the documents
/// still being sent hold the session's budget.
#[tokio::test]
async fn exports_are_refused_past_their_bounds() {
    let dir = tempdir().expect("temp dir");
    let with_budget = |budget: Option<u64>| {
        let store = AnnotationStore::loading();
        let session = Session::over(
            FileRegistry::from_files(vec![
                entry(dir.path(), "a.dcm", "1.2.3.1"),
                entry(dir.path(), "b.dcm", "1.2.3.2"),
            ]),
            match budget {
                Some(budget) => store.with_download_budget(budget),
                None => store,
            },
        );
        session.stage(HashMap::from([
            (0, rows(&[[1, 2, 3, 4], [5, 6, 7, 8]], &[])),
            (1, rows(&[[1, 2, 3, 4]], &[])),
        ]));
        session
    };
    let limits = dcmview_adapters::Limits::DEFAULT;
    async fn export(
        session: &Session,
        limits: dcmview_adapters::Limits,
    ) -> Result<dcmview::server::annotations::NativeExport, AnnotationError> {
        export_native_within(
            &session.registry,
            &session.store,
            NativeForm::JsonLines,
            limits,
        )
        .await
    }
    let refused = |result: Result<_, AnnotationError>| match result {
        Ok(_) => panic!("the export was written"),
        Err(error) => error,
    };
    let session = with_budget(None);
    assert_eq!(
        refused(
            export(
                &session,
                dcmview_adapters::Limits {
                    records: 2,
                    ..limits
                }
            )
            .await
        ),
        AnnotationError::ExportTooMany {
            bound: dcmview_adapters::Bound::Records,
            limit: 2
        }
    );
    assert_eq!(
        refused(export(&session, dcmview_adapters::Limits { files: 1, ..limits }).await),
        AnnotationError::ExportTooMany {
            bound: dcmview_adapters::Bound::Files,
            limit: 1
        }
    );
    let size = export(&session, limits).await.expect("export").bytes.len() as u64;

    // One text is always kept, whatever its size. A second is kept beside
    // it only within the budget, and again once the first has gone.
    let tight = with_budget(Some(2 * size - 1));
    let first = export(&tight, limits).await.expect("the only download");
    assert_eq!(
        refused(export(&tight, limits).await),
        AnnotationError::ExportBusy
    );
    drop(first);
    let roomy = with_budget(Some(2 * size));
    let first = export(&roomy, limits).await.expect("the only download");
    let second = export(&roomy, limits).await.expect("within the budget");
    assert_eq!(
        refused(export(&roomy, limits).await),
        AnnotationError::ExportBusy
    );
    drop(first);
    let third = export(&roomy, limits).await.expect("room again");
    drop((second, third));

    // Through the endpoint: while a download holds the budget, another is
    // answered 503 with the time to wait, and is sent once it has ended.
    let small = with_budget(Some(1));
    let held = small.store.hold_download(1).expect("a download in flight");
    for path in [
        "/api/annotations/export.json",
        "/api/annotations/export.jsonl",
    ] {
        let response = small.server.get(path).await;
        assert_eq!(
            response.status_code(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{path}"
        );
        assert_eq!(response.header("retry-after"), "1", "{path}");
        let body: Value = response.json();
        assert_eq!(body["code"], "annotation_export_busy", "{path}");
    }
    drop(held);
    small.text("/api/annotations/export.json").await;
    // A response that was read to its end holds nothing any more.
    small.text("/api/annotations/export.jsonl").await;
}

/// A masked session's document holds no path, and byte-identical files
/// have nothing else to tell them apart, so it does not say which copy a
/// row was on: only that it was on one of so many. A session that loads
/// it puts such rows on the copy whose path sorts first, whatever order
/// the files were found in, and its report says that which copy each row
/// was on is not kept.
#[tokio::test]
async fn a_masked_document_does_not_say_which_copy_a_row_was_on() {
    let dir = tempdir().expect("temp dir");
    for name in ["a.dcm", "b.dcm", "c.dcm"] {
        written(dir.path(), name, "1.2.3.8", &[3; 64]);
    }
    let masked = |order: [&str; 3]| {
        let registry = FileRegistry::masked(Arc::new(Masker::new()));
        for name in order {
            registry.insert(entry(dir.path(), name, "1.2.3.8"));
        }
        registry.mark_scan_complete();
        Session::over(registry, AnnotationStore::loading())
    };
    let first = masked(["a.dcm", "b.dcm", "c.dcm"]);
    first.stage(HashMap::from([
        (1, rows(&[[1, 2, 3, 4]], &[])),
        (2, rows(&[[5, 6, 7, 8], [9, 9, 12, 12]], &[])),
    ]));
    let document = first.document().await;
    let copies: Vec<&Value> = document["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| &record["extensions"]["embed"])
        .collect();
    assert_eq!(copies, [&json!({ "copies": 3, "imported": true }); 3]);
    assert_eq!(document["files"].as_array().expect("files").len(), 1);

    // The same folder, its files found in every order: the rows are on
    // the same file each time, and the report says what was not kept.
    for order in [
        ["a.dcm", "b.dcm", "c.dcm"],
        ["c.dcm", "b.dcm", "a.dcm"],
        ["b.dcm", "c.dcm", "a.dcm"],
    ] {
        let second = masked(order);
        let report = second
            .import(dir.path(), &document.to_string())
            .await
            .expect("import");
        assert_eq!(report["counts"]["annotations_out"], 3, "{report}");
        assert_eq!(
            report["losses"],
            json!([{ "code": "copy_placement_not_kept", "count": 3, "rows": [] }]),
            "{order:?}"
        );
        for (index, name) in order.into_iter().enumerate() {
            let shown = second.rois(index).await.roi_coords;
            if name == "a.dcm" {
                assert_eq!(
                    shown,
                    [[1, 2, 3, 4], [5, 6, 7, 8], [9, 9, 12, 12]],
                    "{order:?}"
                );
            } else {
                assert!(shown.is_empty(), "{name} in {order:?}: {shown:?}");
            }
        }
    }
}

/// What a copy shows after a reload does not wait for the copy to be
/// hashed in the background: when the import has ended, every loaded file
/// that carries the UID of a file it matched has its key.
#[tokio::test]
async fn an_import_leaves_no_copy_of_a_matched_file_without_its_key() {
    let dir = tempdir().expect("temp dir");
    // Large enough that hashing one takes far longer than the import
    // takes to place one record on the first.
    let bytes = vec![5_u8; 24 * 1024 * 1024];
    let copies = || -> Vec<FileEntry> {
        ["a.dcm", "b.dcm", "c.dcm", "d.dcm"]
            .iter()
            .map(|name| written(dir.path(), name, "1.2.3.8", &bytes))
            .collect()
    };
    let mut named = reference("sop:1.2.3.8", Some("1.2.3.8"), "a.dcm");
    named["size_bytes"] = json!(bytes.len());
    let text = document_of(
        vec![named],
        vec![],
        vec![annotation(1, "sop:1.2.3.8", "default", rect(4, 4, 8, 8))],
    );
    let session = Session::loading(copies());
    session.import(dir.path(), &text).await.expect("import");
    for index in 0..4 {
        let status = session.registry.key_status(index).expect("status");
        assert!(status.settled, "file {index} has no key yet");
    }
    assert_eq!(session.rois(3).await.roi_coords, [[4, 4, 8, 8]]);
}

/// A client that asks for a compressed download gets the document it
/// gets without: both forms, a document of one piece and one of several.
#[tokio::test]
async fn a_compressed_download_is_the_same_document() {
    use std::io::Read;
    let dir = tempdir().expect("temp dir");
    for files in [2_usize, 4_000] {
        let session = Session::loading(
            (0..files)
                .map(|n| entry(dir.path(), &format!("{n}.dcm"), &format!("1.2.3.{n}")))
                .collect(),
        );
        session.stage(
            (0..files)
                .map(|n| (n, rows(&[[1, 2, 3, 4]], &[])))
                .collect(),
        );
        for path in [
            "/api/annotations/export.json",
            "/api/annotations/export.jsonl",
            "/api/annotations/export.csv",
        ] {
            let plain = session.server.get(path).await;
            plain.assert_status_ok();
            let plain = plain.as_bytes().to_vec();
            assert_eq!(
                plain.len() > 1024 * 1024,
                files > 2 && !path.ends_with("csv")
            );
            let packed = session
                .server
                .get(path)
                .add_header("accept-encoding", "gzip")
                .await;
            packed.assert_status_ok();
            assert_eq!(packed.header("content-encoding"), "gzip", "{path}");
            let mut unpacked = Vec::new();
            flate2::read::GzDecoder::new(packed.as_bytes().as_ref())
                .read_to_end(&mut unpacked)
                .unwrap_or_else(|error| panic!("{path}, {files} files: {error}"));
            // Ids and stamps are the store's, so two exports are one text.
            assert_eq!(unpacked, plain, "{path}, {files} files");
        }
    }
}

/// Whatever a session holds and can write, the same viewer reads back:
/// records that cost many times their text to hold (masks of one tile a
/// frame, large and deeply nested `extensions`), and the largest mask and
/// polygon the model allows. And a session is refused its export exactly
/// when a document of it would be refused its import: by the same count,
/// to the byte.
#[tokio::test]
async fn what_a_session_can_write_it_can_read() {
    let dir = tempdir().expect("temp dir");
    let file = || {
        let mut file = entry(dir.path(), "volume.dcm", "1.2.3.1");
        file.rows = 4096;
        file.columns = 4096;
        file.frame_count = 65_536;
        vec![file]
    };
    let size = ImageSize {
        columns: 4096,
        rows: 4096,
        frames: 65_536,
    };
    let key = FileKey::parse("sop:1.2.3.1").expect("key");
    let mut records = Vec::new();
    let mut create = |frames: Value, geometry: Value, extensions: Value| {
        let n = records.len() as u32 + 1;
        let mut record = annotation(n, "sop:1.2.3.1", "default", geometry);
        record["frames"] = frames;
        record["extensions"] = extensions;
        records.push(record);
    };
    // Masks of thirty frames with one tile each.
    for n in 0..400_u32 {
        let frames: Vec<u32> = (n..n + 30).collect();
        let tiles: serde_json::Map<String, Value> = frames
            .iter()
            .map(|frame| (frame.to_string(), json!({ "0,0": "AAAA" })))
            .collect();
        create(
            json!({ "set": frames }),
            json!({ "type": "mask", "encoding": "tiles-v1", "tile": 64, "depth": 1, "frames": tiles }),
            json!({}),
        );
    }
    // Extensions that are large, and that are deep.
    let deep = (0..40).fold(json!(0), |inner, _| json!({ "a": inner }));
    for _ in 0..50 {
        create(
            json!("all"),
            rect(1, 1, 9, 9),
            json!({ "x": vec![0; 2_000] }),
        );
        create(
            json!("all"),
            rect(1, 1, 9, 9),
            json!({ "x": vec![deep.clone(); 20] }),
        );
    }
    // The largest mask and the largest polygon.
    let frames: Vec<u32> = (0..65_536).collect();
    let tiles: serde_json::Map<String, Value> = frames
        .iter()
        .map(|frame| {
            (
                frame.to_string(),
                json!({ "0,0": "AAAA", "1,0": "AAAA", "0,1": "AAAA", "1,1": "AAAA" }),
            )
        })
        .collect();
    create(
        json!({ "set": frames }),
        json!({ "type": "mask", "encoding": "tiles-v1", "tile": 64, "depth": 1, "frames": tiles }),
        json!({}),
    );
    let points: Vec<Value> = (0..100_000)
        .map(|n| json!({ "x": n % 4096, "y": n / 4096 }))
        .collect();
    create(
        json!("all"),
        json!({ "type": "polygon", "points": points }),
        json!({}),
    );

    let first = Session::plain(file());
    first.registry.ensure_key(0).await.expect("key");
    let count = records.len() as u64;
    for (n, record) in records.into_iter().enumerate() {
        let envelope: OpEnvelope = serde_json::from_value(json!({
            "op_id": id(100_000 + n as u32), "actor": "user:test",
            "ts": "2026-10-10T08:00:00.000Z",
            "op": { "type": "create_annotation", "annotation": record },
        }))
        .expect("envelope");
        let results = first
            .store
            .backend()
            .apply(vec![envelope], &BTreeMap::from([(key.clone(), size)]))
            .expect("store");
        assert!(
            matches!(results[0], ApplyResult::Ok { .. }),
            "record {n}: {:?}",
            results[0]
        );
    }

    let limits = dcmview_adapters::Limits::DEFAULT;
    let mut charge = 0;
    for form in [NativeForm::Json, NativeForm::JsonLines] {
        let export = export_native_within(&first.registry, &first.store, form, limits)
            .await
            .unwrap_or_else(|error| panic!("{form:?}: {error}"));
        let text = String::from_utf8(export.bytes).expect("text");
        if form == NativeForm::Json {
            let document = dcmview_annotation::Document::from_json_str(&text).expect("document");
            charge = dcmview_adapters::held::document_charge(&document);
            // Many times the text, and far inside the budget.
            assert!(
                charge > 3 * text.len() as u64,
                "{charge} for {}",
                text.len()
            );
            assert!(charge < limits.held_bytes / 8);
        }
        let second = Session::loading(file());
        let report = second.import(dir.path(), &text).await.expect("import");
        assert_eq!(report["counts"]["annotations_out"], count, "{form:?}");
        assert_eq!(report["counts"]["refused"], 0, "{form:?}");

        // At the budget's edge the two agree: a session charged one byte
        // more than the budget is not written, and its document, written
        // under a larger one, is not read.
        let within = |held_bytes| dcmview_adapters::Limits {
            held_bytes,
            ..limits
        };
        let refused = export_native_within(&first.registry, &first.store, form, within(charge - 1))
            .await
            .err()
            .expect("past the budget");
        assert_eq!(
            refused,
            AnnotationError::ExportTooMany {
                bound: dcmview_adapters::Bound::HeldBytes,
                limit: charge - 1
            }
        );
        assert!(refused.to_string().contains("bytes of memory"), "{refused}");
        export_native_within(&first.registry, &first.store, form, within(charge))
            .await
            .expect("at the budget");
        let path = dir.path().join("edge");
        std::fs::write(&path, &text).expect("write");
        let source = NativeSource::from_path(&path).expect("source");
        for (held_bytes, read) in [(charge, true), (charge - 1, false)] {
            let third = Session::loading(file());
            let result = import_native_within(
                &third.registry,
                &third.store,
                &source,
                Arc::new(|| true),
                within(held_bytes),
            )
            .await;
            assert_eq!(
                result.is_ok(),
                read,
                "{form:?} under {held_bytes}: {result:?}"
            );
        }
    }

    // A reader holds one record to more than the charge: to a number of
    // JSON values, of objects and arrays, to a depth, and in the JSON
    // Lines form to a length of line. A record past one of those is in
    // the session, and the session is refused the export that would write
    // it, naming the bound; it is never written and then not read.
    use dcmview_adapters::Bound;
    let nested = (0..70).fold(json!(0), |inner, _| json!([inner]));
    let shapes: [(&str, Value, [Option<Bound>; 2]); 4] = [
        (
            "more values than a record may have",
            json!({ "x": vec![0; 1_100_000] }),
            [Some(Bound::UnitValues), Some(Bound::UnitValues)],
        ),
        (
            "more arrays than a record may have",
            json!({ "x": vec![json!([]); 140_000] }),
            [Some(Bound::UnitContainers), Some(Bound::UnitContainers)],
        ),
        (
            "deeper than values may nest",
            json!({ "x": nested }),
            [Some(Bound::Depth), Some(Bound::Depth)],
        ),
        (
            "longer than a line may be",
            json!({ "x": "a".repeat(17 * 1024 * 1024) }),
            [None, Some(Bound::LineBytes)],
        ),
    ];
    for (what, extensions, refused) in shapes {
        let session = Session::plain(file());
        session.registry.ensure_key(0).await.expect("key");
        let mut record = annotation(1, "sop:1.2.3.1", "default", rect(1, 1, 9, 9));
        record["extensions"] = extensions;
        let envelope: OpEnvelope = serde_json::from_value(json!({
            "op_id": id(900_000), "actor": "user:test", "ts": "2026-10-10T08:00:00.000Z",
            "op": { "type": "create_annotation", "annotation": record },
        }))
        .expect("envelope");
        let results = session
            .store
            .backend()
            .apply(vec![envelope], &BTreeMap::from([(key.clone(), size)]))
            .expect("store");
        assert!(matches!(results[0], ApplyResult::Ok { .. }), "{what}");
        for (form, refused) in [NativeForm::Json, NativeForm::JsonLines]
            .into_iter()
            .zip(refused)
        {
            let export =
                export_native_within(&session.registry, &session.store, form, limits).await;
            match (export, refused) {
                (Err(AnnotationError::ExportTooMany { bound, .. }), Some(expected)) => {
                    assert_eq!(bound, expected, "{what}, {form:?}");
                }
                (Ok(export), None) => {
                    let text = String::from_utf8(export.bytes).expect("text");
                    let second = Session::loading(file());
                    let report = second.import(dir.path(), &text).await.expect("import");
                    assert_eq!(report["counts"]["annotations_out"], 1, "{what}, {form:?}");
                }
                (other, expected) => panic!(
                    "{what}, {form:?}: {:?}, expected {expected:?}",
                    other.map(|export| export.bytes.len())
                ),
            }
        }
    }
}

/// A store that takes labels: a schema with one field for files and frames.
fn labelled_store() -> AnnotationStore {
    let mut schema = LabelSchema::implicit();
    schema.fields = serde_json::from_value(json!([{
        "id": "quality", "name": "Quality", "type": "category",
        "options": [{ "id": "good", "name": "Good" }, { "id": "bad", "name": "Bad" }],
        "applies_to": ["file", "frame", "series", "folder"],
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
                // Named by what the session showed, and by a folder.
                label(3, json!({ "series": "2.25.1234567890" }), "good"),
                label(
                    4,
                    json!({ "folder": "ward-7-north/DOE_JANE_1961", "root": "data" }),
                    "bad",
                ),
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

    let text = first.text("/api/annotations/export.json").await;
    // A masked document holds labels on files and frames only: the others
    // name a folder, or what one session showed.
    for kept_back in ["DOE_JANE_1961", "ward-7-north", "2.25.1234567890"] {
        assert!(!text.contains(kept_back), "{kept_back}: {text}");
    }
    let mut document: Value = serde_json::from_str(&text).expect("document");
    assert_eq!(document["labels"].as_array().expect("labels").len(), 2);
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

/// A label schema with two coloured classes and one field for files,
/// studies and series.
fn classed_schema() -> Value {
    json!({
        "schema_id": "reading", "schema_version": 2,
        "classes": [
            { "id": "mass", "name": "Mass", "color": "#E4572E",
              "geometry": ["rect", "polygon", "ellipse"] },
            { "id": "calc", "name": "Calcification", "color": "#3B8EA5",
              "geometry": ["point", "rect"] },
        ],
        "fields": [{
            "id": "density", "name": "Density", "type": "category",
            "options": [{ "id": "a", "name": "A" }, { "id": "b", "name": "B" }],
            "applies_to": ["file", "frame", "series", "study", "patient"],
        }],
    })
}

fn classed(n: u32, file: &str, class: &str, geometry: Value) -> Value {
    let mut record = annotation(n, file, "default", geometry);
    record["class"] = json!(class);
    record
}

fn label_record(n: u32, target: Value, value: &str) -> Value {
    json!({
        "id": id(n), "target": target, "field": "density", "layer": "default", "value": value,
        "rev": 1, "created_by": "user:someone", "created_at": "2001-01-01T00:00:00.000Z",
        "modified_by": "user:someone", "modified_at": "2001-01-01T00:00:00.000Z",
    })
}

/// A session has no label schema of its own, so the schema of its
/// `--annotations` document becomes the session's: the document's classes
/// and labels load, operations are checked against it, and the export
/// carries it. A document that fails to load leaves no schema behind.
#[tokio::test]
async fn a_documents_schema_becomes_the_sessions() {
    let dir = tempdir().expect("temp dir");
    let files = || vec![entry(dir.path(), "a.dcm", "1.2.3.1")];
    let mut document: Value = serde_json::from_str(&document_of(
        vec![reference("sop:1.2.3.1", Some("1.2.3.1"), "a.dcm")],
        vec![],
        vec![classed(1, "sop:1.2.3.1", "mass", rect(1, 2, 30, 40))],
    ))
    .expect("document");
    document["schema"] = classed_schema();
    document["labels"] = json!([
        label_record(2, json!({ "file": "sop:1.2.3.1" }), "a"),
        label_record(3, json!({ "study": "1.2.3" }), "b"),
    ]);

    let session = Session::loading(files());
    let report = session
        .import(dir.path(), &document.to_string())
        .await
        .expect("import");
    assert_eq!(report["warnings"], json!([]), "{report}");
    assert_eq!(
        (
            &report["counts"]["annotations_out"],
            &report["counts"]["labels_out"]
        ),
        (&json!(1), &json!(2))
    );
    let exported = session.document().await;
    // The document's schema, with the class the EMBED endpoints draw in.
    let mut schema: LabelSchema =
        serde_json::from_value(classed_schema()).expect("the schema as the model reads it");
    schema.classes.extend(LabelSchema::implicit().classes);
    assert_eq!(
        exported["schema"],
        serde_json::to_value(&schema).expect("schema")
    );
    assert_eq!(exported["annotations"][0]["class"], "mass");

    // An operation is checked against the session's schema now.
    let post = |class: &str| {
        session
            .server
            .post("/api/annotations/ops?file=0")
            .json(&json!({
                "op_id": dcmview_annotation::new_id(), "actor": "user:test",
                "ts": "2026-10-10T08:00:00.000Z",
                "op": { "type": "create_annotation", "annotation": classed(
                    if class == "calc" { 4 } else { 5 }, "sop:1.2.3.1", class,
                    json!({ "type": "point", "x": 3, "y": 4 })) },
            }))
    };
    assert_eq!(post("calc").await.status_code(), StatusCode::OK);
    let refused = post("nodule").await;
    assert_eq!(refused.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused.json::<Value>()["result"]["violations"][0]["code"],
        "unknown_class"
    );

    // A document whose records do not fit it puts no schema in force.
    document["annotations"][0]["class"] = json!("nodule");
    let failed = Session::loading(files());
    failed
        .import(dir.path(), &document.to_string())
        .await
        .expect_err("a class the schema does not have");
    assert!(failed.store.backend().schema().expect("schema").is_none());
}

impl Session {
    async fn native(&self, index: usize) -> Value {
        let response = self
            .server
            .get(&format!("/api/file/{index}/annotations/native"))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
        response.json()
    }
}

/// `(id, class, geometry type, embed_index)` of each record a native read
/// answers, in its order.
fn native_records(answer: &Value) -> Vec<(String, String, String, Value)> {
    answer["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .map(|record| {
            (
                record["id"].as_str().expect("id").to_string(),
                record["class"].as_str().expect("class").to_string(),
                record["geometry"]["type"]
                    .as_str()
                    .expect("geometry")
                    .to_string(),
                record["embed_index"].clone(),
            )
        })
        .collect()
}

fn label_ids(answer: &Value) -> Vec<String> {
    answer["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .map(|label| label["id"].as_str().expect("id").to_string())
        .collect()
}

/// Two files of one study, a document with classes, a coloured second
/// layer, every vector geometry and labels on files, a frame, the study,
/// the patient and another series.
fn classed_document() -> Value {
    let a = "sop:1.2.3.1";
    let b = "sop:1.2.3.2";
    let mut document: Value = serde_json::from_str(&document_of(
        vec![
            reference(a, Some("1.2.3.1"), "a.dcm"),
            reference(b, Some("1.2.3.2"), "b.dcm"),
        ],
        vec![
            json!({ "id": "second", "name": "Second reader", "kind": "user",
                     "color": "#22AA44" }),
        ],
        vec![
            classed(1, a, "mass", rect(10, 20, 30, 40)),
            classed(
                2,
                a,
                "mass",
                json!({ "type": "polygon", "points": [
                    { "x": 1, "y": 1 }, { "x": 9, "y": 1 }, { "x": 5, "y": 9 }] }),
            ),
            classed(3, a, "calc", json!({ "type": "point", "x": 3, "y": 4 })),
            classed(
                4,
                a,
                "mass",
                json!({ "type": "ellipse", "cx": 50, "cy": 40, "rx": 10, "ry": 5, "angle": 30 }),
            ),
            classed(5, a, "calc", rect(60, 60, 70, 70)),
            classed(6, b, "mass", rect(1, 1, 5, 5)),
        ],
    ))
    .expect("document");
    document["annotations"][4]["layer"] = json!("second");
    document["schema"] = classed_schema();
    document["labels"] = json!([
        label_record(11, json!({ "file": a }), "a"),
        label_record(12, json!({ "frame": a, "index": 2 }), "b"),
        label_record(13, json!({ "study": "1.2.3" }), "a"),
        label_record(14, json!({ "patient": "TEST" }), "b"),
        label_record(15, json!({ "file": b }), "b"),
        label_record(16, json!({ "series": "9.9.9" }), "a"),
    ]);
    document
}

fn study_files(directory: &Path) -> Vec<FileEntry> {
    let mut files = vec![
        entry(directory, "a.dcm", "1.2.3.1"),
        entry(directory, "b.dcm", "1.2.3.2"),
    ];
    for file in &mut files {
        file.study_instance_uid = "1.2.3".to_string();
        file.series_instance_uid = "1.2.3.4".to_string();
    }
    files
}

/// The per-file native read: the session's schema and layers, the file's
/// records of every geometry with their places in the EMBED view, and the
/// labels on the file, its frames, its study and its patient.
#[tokio::test]
async fn a_files_records_are_read_with_their_classes_layers_and_labels() {
    let dir = tempdir().expect("temp dir");
    let session = Session::loading(study_files(dir.path()));

    // The read waits for the import, as the EMBED read does.
    let waiting = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        session.server.get("/api/file/0/annotations/native"),
    )
    .await;
    assert!(waiting.is_err(), "a native read waits for the import");
    session
        .import(dir.path(), &classed_document().to_string())
        .await
        .expect("import");

    let answer = session.native(0).await;
    let keys: Vec<&String> = answer.as_object().expect("object").keys().collect();
    assert_eq!(
        keys,
        ["annotations", "labels", "layers", "revision", "schema"]
    );
    assert_eq!(answer["schema"]["schema_id"], "reading");
    assert_eq!(answer["schema"]["classes"][0]["color"], "#E4572E");
    let layers: Vec<(&Value, &Value)> = answer["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .map(|layer| (&layer["id"], &layer["color"]))
        .collect();
    assert_eq!(
        layers,
        [
            (&json!("default"), &Value::Null),
            (&json!("second"), &json!("#22AA44"))
        ]
    );
    let text = |n: u32, class: &str, kind: &str, index: Value| {
        (id(n), class.to_string(), kind.to_string(), index)
    };
    assert_eq!(
        native_records(&answer),
        [
            text(1, "mass", "rect", json!(0)),
            text(2, "mass", "polygon", Value::Null),
            text(3, "calc", "point", Value::Null),
            text(4, "mass", "ellipse", Value::Null),
            text(5, "calc", "rect", json!(1)),
        ]
    );
    assert_eq!(answer["annotations"][3]["geometry"]["angle"], 30);
    assert_eq!(answer["annotations"][4]["layer"], "second");
    assert_eq!(answer["annotations"][0]["file"], "sop:1.2.3.1");
    // The EMBED view shows the two rectangles, in those places (as EMBED
    // writes them: y0, x0, y1, x1).
    assert_eq!(
        session.rois(0).await,
        rows(&[[20, 10, 40, 30], [60, 60, 70, 70]], &[])
    );
    assert_eq!(
        label_ids(&answer),
        [id(11), id(12), id(13), id(14)],
        "the file's, its frame's, its study's and its patient's"
    );
    let revision = answer["revision"].as_u64().expect("revision");
    assert_eq!(
        revision,
        session.store.backend().revision().expect("revision")
    );

    // The other file has its own records and labels, and the shared ones.
    let other = session.native(1).await;
    assert_eq!(native_records(&other), [text(6, "mass", "rect", json!(0))]);
    assert_eq!(label_ids(&other), [id(13), id(14), id(15)]);

    // No file is a 404 in the error envelope.
    let missing = session.server.get("/api/file/9/annotations/native").await;
    assert_eq!(missing.status_code(), StatusCode::NOT_FOUND);
    assert_eq!(missing.json::<Value>()["code"], "not_found");
}

/// A save of the EMBED view changes the rectangles it shows, by their
/// places in it, and leaves every other record; the native read then gives
/// each rectangle its place in the view, and the store's new revision.
#[tokio::test]
async fn embed_indexes_follow_a_save_of_the_embed_view() {
    let dir = tempdir().expect("temp dir");
    let session = Session::loading(study_files(dir.path()));
    session
        .import(dir.path(), &classed_document().to_string())
        .await
        .expect("import");
    let before = session.native(0).await;
    let others = |answer: &Value| -> Vec<(String, String, String, Value)> {
        native_records(answer)
            .into_iter()
            .filter(|record| record.2 != "rect")
            .collect()
    };
    let unshown = others(&before);
    assert_eq!(unshown.len(), 3);

    // The first rectangle stays, the second moves, and one is added.
    let save = |rois: EmbedRoiAnnotations| {
        let server = &session.server;
        async move {
            server
                .put("/api/file/0/annotations")
                .json(&rois)
                .await
                .assert_status_ok();
        }
    };
    save(rows(&[[20, 10, 40, 30], [5, 5, 15, 15], [1, 2, 3, 4]], &[])).await;
    let after = session.native(0).await;
    let records = native_records(&after);
    assert_eq!(
        records
            .iter()
            .filter(|record| record.2 == "rect")
            .map(|record| (record.0.clone(), record.3.clone()))
            .collect::<Vec<_>>()[..2],
        [(id(1), json!(0)), (id(5), json!(1))]
    );
    assert_eq!(records.len(), 6, "{after}");
    assert_eq!(records[5].1, "roi", "a new rectangle has the EMBED class");
    assert_eq!(records[5].3, json!(2));
    assert_eq!(others(&after), unshown, "the other records are left");
    assert!(
        after["revision"].as_u64() > before["revision"].as_u64(),
        "{} then {}",
        before["revision"],
        after["revision"]
    );
    let in_view = |answer: &Value, view: &EmbedRoiAnnotations| {
        for record in answer["annotations"].as_array().expect("annotations") {
            if let Some(index) = record["embed_index"].as_u64() {
                let [y0, x0, y1, x1] = view.roi_coords[index as usize];
                assert_eq!(rect(x0, y0, x1, y1), record["geometry"], "{record}");
            }
        }
    };
    in_view(&after, &session.rois(0).await);

    // A save of one ROI keeps the first rectangle and deletes the rest.
    save(rows(&[[20, 10, 40, 30]], &[])).await;
    let last = session.native(0).await;
    assert_eq!(
        native_records(&last)
            .into_iter()
            .filter(|record| record.2 == "rect")
            .collect::<Vec<_>>(),
        [(id(1), "mass".into(), "rect".into(), json!(0))]
    );
    assert_eq!(others(&last), unshown);
    in_view(&last, &session.rois(0).await);
}

/// A read hashes nothing. A file whose key needs its bytes (a raster)
/// answers empty, its staged CSV rows staying in the EMBED view, until
/// something that writes settles its key; once a document import has
/// placed records on it, they are there.
#[tokio::test]
async fn a_file_whose_key_is_not_settled_answers_empty_and_is_not_hashed() {
    let dir = tempdir().expect("temp dir");
    let mut raster = unnamed(dir.path(), "scan.png", b"raster bytes");
    raster.format = dcmview::api::contracts::FileFormat::Png;
    let session = Session::loading(vec![raster.clone()]);
    session.stage(HashMap::from([(0, rows(&[[1, 2, 3, 4]], &[]))]));

    let answer = session.native(0).await;
    assert_eq!(answer["annotations"], json!([]));
    assert_eq!(answer["labels"], json!([]));
    assert_eq!(answer["schema"]["schema_id"], "implicit");
    assert_eq!(answer["schema"]["classes"][0]["id"], "roi");
    assert_eq!(answer["layers"][0]["id"], "default");
    let stats = session.registry.key_stats();
    assert_eq!((stats.files_hashed, stats.bytes_hashed), (0, 0));
    assert_eq!(session.registry.files_page(None, None).keys_hashing, 0);
    assert!(!session.registry.key_status(0).expect("status").settled);
    assert_eq!(session.rois(0).await, rows(&[[1, 2, 3, 4]], &[]));

    // A document import hashes the files it places records on; the read
    // then shows them.
    let key = content_key(b"raster bytes");
    let mut named = reference(&key, None, "scan.png");
    named["size_bytes"] = json!(12);
    named["kind"] = json!("raster");
    named["format"] = json!("png");
    let imported = Session::loading(vec![raster]);
    imported
        .import(
            dir.path(),
            &document_of(
                vec![named],
                vec![],
                vec![annotation(1, &key, "default", rect(1, 1, 2, 2))],
            ),
        )
        .await
        .expect("import");
    let answer = imported.native(0).await;
    assert_eq!(
        native_records(&answer),
        [(id(1), "roi".into(), "rect".into(), json!(0))]
    );
    assert_eq!(answer["annotations"][0]["file"], key);
}

/// A masked session sends keys and hierarchy ids as it shows them, and no
/// real one: a label on the study as the session shows it applies, one
/// read from a document under the real UID names nothing here.
#[tokio::test]
async fn a_masked_session_reads_records_under_the_keys_it_shows() {
    let dir = tempdir().expect("temp dir");
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    for file in study_files(dir.path()) {
        registry.insert(file);
    }
    registry.mark_scan_complete();
    let session = Session::over(registry, AnnotationStore::loading());
    let mut document = classed_document();
    document["files"] = json!([
        reference("sop:1.2.3.1", Some("1.2.3.1"), "a.dcm"),
        reference("sop:1.2.3.2", Some("1.2.3.2"), "b.dcm"),
    ]);
    // Found by their paths, and checked against their keys.
    session
        .import(dir.path(), &document.to_string())
        .await
        .expect("import");
    let catalog: Value = serde_json::from_str(&session.text("/api/files").await).expect("files");
    // Left out of the catalog: `sop:` and the UID the session shows.
    assert!(catalog["files"][0].get("file_key").is_none());
    let shown_key = format!(
        "sop:{}",
        catalog["files"][0]["sop_instance_uid"]
            .as_str()
            .expect("uid")
    );
    let shown_study = catalog["files"][0]["study_instance_uid"]
        .as_str()
        .expect("study")
        .to_string();
    assert_ne!(shown_key, "sop:1.2.3.1");

    // A label on the study as this session shows it.
    session
        .op(
            Some(0),
            json!({ "type": "set_label", "id": id(20), "base_rev": null,
                    "target": { "study": shown_study }, "field": "density",
                    "layer": "default", "before": null, "after": "b" }),
        )
        .await;
    let response = session.server.get("/api/file/0/annotations/native").await;
    response.assert_status_ok();
    let text = response.text();
    for real in ["1.2.3.1", "\"1.2.3\"", "1.2.3.2", "\"TEST\""] {
        assert!(!text.contains(real), "{real}: {text}");
    }
    let answer: Value = serde_json::from_str(&text).expect("answer");
    assert_eq!(native_records(&answer).len(), 5);
    assert!(answer["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .all(|record| record["file"] == shown_key.as_str()));
    // The file's and its frame's labels under the shown key, and the
    // study's under the shown UID; the document's study and patient
    // labels name a real UID and a real id, which this session does not
    // show.
    assert_eq!(label_ids(&answer), [id(11), id(12), id(20)]);
    assert_eq!(answer["labels"][0]["target"], json!({ "file": shown_key }));
    assert_eq!(
        answer["labels"][2]["target"],
        json!({ "study": shown_study })
    );
    // No user name: authors are written as a masked export writes them,
    // also on a record an operation made.
    session
        .op(
            Some(0),
            json!({ "type": "create_annotation", "annotation": classed(
                21, &shown_key, "calc", json!({ "type": "point", "x": 3, "y": 4 })) }),
        )
        .await;
    let response = session.server.get("/api/file/0/annotations/native").await;
    response.assert_status_ok();
    let text = response.text();
    let answer: Value = serde_json::from_str(&text).expect("answer");
    let made = answer["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .find(|record| record["id"] == id(21).as_str())
        .expect("the record the operation made");
    assert_eq!(made["created_by"], "user:local");
    assert_eq!(made["modified_by"], "user:local");
    assert_eq!(answer["labels"][2]["created_by"], "user:local");
    assert_eq!(answer["labels"][2]["modified_by"], "user:local");
    assert_eq!(answer["layers"][0]["source"]["author"], "user:local");
    let user = dcmview::annotations::session_author();
    if user.as_str() != "user:local" {
        assert!(!text.contains(user.as_str()), "{user}: {text}");
    }
}

/// A session whose document schema has no `roi` class still writes an
/// export it can load again: the EMBED endpoints make `roi` rectangles,
/// and the session's schema gains the implicit `roi` class with the
/// document's.
#[tokio::test]
async fn an_export_after_an_embed_save_loads_again_under_a_document_schema() {
    let dir = tempdir().expect("temp dir");
    let session = Session::loading(study_files(dir.path()));
    session
        .import(dir.path(), &classed_document().to_string())
        .await
        .expect("import");
    let answer = session.native(0).await;
    let classes: Vec<&Value> = answer["schema"]["classes"]
        .as_array()
        .expect("classes")
        .iter()
        .map(|class| &class["id"])
        .collect();
    assert_eq!(classes, [&json!("mass"), &json!("calc"), &json!("roi")]);
    assert_eq!(answer["schema"]["classes"][2]["name"], "ROI");
    assert!(answer["schema"]["classes"][2]["color"].is_null());

    // One more rectangle through the EMBED view: a `roi` record.
    session
        .server
        .put("/api/file/0/annotations")
        .json(&rows(
            &[[20, 10, 40, 30], [60, 60, 70, 70], [1, 2, 3, 4]],
            &[],
        ))
        .await
        .assert_status_ok();
    let text = session.text("/api/annotations/export.json").await;
    let exported: Value = serde_json::from_str(&text).expect("document");
    assert!(exported["annotations"]
        .as_array()
        .expect("annotations")
        .iter()
        .any(|record| record["class"] == "roi"));

    let again = Session::loading(study_files(dir.path()));
    let report = again.import(dir.path(), &text).await.expect("reload");
    assert_eq!(report["warnings"], json!([]), "{report}");
    assert_eq!(
        report["counts"]["annotations_out"],
        exported["annotations"]
            .as_array()
            .expect("annotations")
            .len()
    );
    assert_eq!(again.document().await["schema"], exported["schema"]);
}

/// Writes of a session with a document wait for its import, so they are
/// checked against the schema it brings; a CSV session's do not
/// (`annotations_endpoint`).
#[tokio::test]
async fn writes_wait_for_a_document_import() {
    let dir = tempdir().expect("temp dir");
    let session = Session::loading_document(study_files(dir.path()));
    let body = json!({
        "op_id": dcmview_annotation::new_id(), "actor": "user:test",
        "ts": "2026-10-10T08:00:00.000Z",
        "op": { "type": "create_annotation", "annotation": classed(
            30, "sop:1.2.3.1", "calc", json!({ "type": "point", "x": 3, "y": 4 })) },
    });
    let waiting = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        session
            .server
            .post("/api/annotations/ops?file=0")
            .json(&body),
    )
    .await;
    assert!(waiting.is_err(), "an operation waits for the document");
    let saving = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        session
            .server
            .put("/api/file/0/annotations")
            .json(&rows(&[[1, 2, 3, 4]], &[])),
    )
    .await;
    assert!(saving.is_err(), "an EMBED save waits for the document");
    session
        .import(dir.path(), &classed_document().to_string())
        .await
        .expect("import");
    // Checked against the document's schema: `calc` is one of its classes.
    let response = session
        .server
        .post("/api/annotations/ops?file=0")
        .json(&body)
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "{}",
        response.text()
    );

    // A failed import holds up no write.
    let failed = Session::loading_document(study_files(dir.path()));
    let mut broken = classed_document();
    broken["annotations"][0]["class"] = json!("nodule");
    failed
        .import(dir.path(), &broken.to_string())
        .await
        .expect_err("a class the schema does not have");
    failed
        .server
        .put("/api/file/0/annotations")
        .json(&rows(&[[1, 2, 3, 4]], &[]))
        .await
        .assert_status_ok();
}

/// A document the store refuses after the adapter took it (a layer id the
/// session already has) fails whole, and puts no schema in force.
#[tokio::test]
async fn an_import_the_store_refuses_puts_no_schema_in_force() {
    let dir = tempdir().expect("temp dir");
    // A CSV-like session, whose writes do not wait, so a layer can be
    // made before the import.
    let session = Session::loading(study_files(dir.path()));
    session
        .op(
            None,
            json!({ "type": "create_layer",
                    "layer": { "id": "second", "name": "Taken", "kind": "user" } }),
        )
        .await;
    let error = session
        .import(dir.path(), &classed_document().to_string())
        .await
        .expect_err("the layer id is taken");
    assert!(error.to_string().contains("refused"), "{error}");
    assert!(session.store.backend().schema().expect("schema").is_none());
}

/// Of byte-identical files, which share a key, a rectangle the EMBED view
/// of one made is that file's alone; a record made through an operation
/// is on both.
#[tokio::test]
async fn a_copys_embed_rows_are_not_read_for_the_other_copy() {
    let dir = tempdir().expect("temp dir");
    let files = vec![
        written(dir.path(), "a.dcm", "1.2.3.9", b"the same bytes"),
        written(dir.path(), "b.dcm", "1.2.3.9", b"the same bytes"),
    ];
    let session = Session::plain(files);
    session
        .server
        .put("/api/file/0/annotations")
        .json(&rows(&[[1, 2, 3, 4]], &[]))
        .await
        .assert_status_ok();
    let key = session
        .registry
        .key_status(0)
        .and_then(|status| status.key)
        .expect("key");
    assert_eq!(
        session.registry.key_status(1).and_then(|status| status.key),
        Some(key.clone())
    );
    session
        .op(
            Some(0),
            create(
                40,
                key.as_str(),
                "default",
                json!({ "type": "point", "x": 3, "y": 4 }),
            ),
        )
        .await;
    let own = native_records(&session.native(0).await);
    let other = native_records(&session.native(1).await);
    assert_eq!(own.len(), 2, "{own:?}");
    assert_eq!(own[0].2, "rect");
    assert_eq!(
        other,
        [(id(40), "roi".to_string(), "point".to_string(), Value::Null)]
    );
}

/// A label that lost its value is not read; an `embed_index` a document
/// carried among a record's unknown members is not sent beside the real
/// one.
#[tokio::test]
async fn a_cleared_label_and_a_carried_embed_index_are_not_sent() {
    let dir = tempdir().expect("temp dir");
    let session = Session::loading(study_files(dir.path()));
    let mut document = classed_document();
    document["annotations"][0]["embed_index"] = json!(99);
    session
        .import(dir.path(), &document.to_string())
        .await
        .expect("import");
    session
        .op(
            Some(0),
            json!({ "type": "set_label", "id": id(11), "base_rev": 1,
                    "target": { "file": "sop:1.2.3.1" }, "field": "density",
                    "layer": "default", "before": "a", "after": null }),
        )
        .await;
    let response = session.server.get("/api/file/0/annotations/native").await;
    response.assert_status_ok();
    let text = response.text();
    let answer: Value = serde_json::from_str(&text).expect("answer");
    assert_eq!(label_ids(&answer), [id(12), id(13), id(14)]);
    assert_eq!(
        text.matches("\"embed_index\"").count(),
        answer["annotations"].as_array().expect("annotations").len(),
        "{text}"
    );
    assert_eq!(answer["annotations"][0]["embed_index"], 0);
}

/// A document schema the session cannot give the class `roi` to (one with
/// 4,096 classes), and one whose own `roi` does not allow rectangles,
/// cannot hold the rectangles an EMBED save makes: a save that adds one is
/// refused with nothing written, while moving and deleting ROIs goes on,
/// and the export loads again. A deprecated `roi` that allows rectangles
/// holds them as before.
#[tokio::test]
async fn an_embed_save_is_refused_where_the_schema_cannot_hold_its_rectangles() {
    let dir = tempdir().expect("temp dir");
    let mut full = classed_schema();
    let classes = full["classes"].as_array_mut().expect("classes");
    for n in classes.len()..4_096 {
        classes.push(json!({ "id": format!("c{n}"), "name": format!("C{n}"),
                             "geometry": ["rect"] }));
    }
    let mut narrow = classed_schema();
    narrow["classes"]
        .as_array_mut()
        .expect("classes")
        .push(json!({ "id": "roi", "name": "Spot", "geometry": ["point"] }));
    let mut deprecated = classed_schema();
    deprecated["classes"]
        .as_array_mut()
        .expect("classes")
        .push(json!({ "id": "roi", "name": "Old", "geometry": ["rect"], "deprecated": true }));
    for (what, schema, adds) in [
        ("full", full, false),
        ("narrow", narrow, false),
        ("deprecated", deprecated, true),
    ] {
        let session = Session::loading_document(study_files(dir.path()));
        let mut document = classed_document();
        document["schema"] = schema;
        session
            .import(dir.path(), &document.to_string())
            .await
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        let answer = session.native(0).await;
        let in_force = answer["schema"]["classes"].as_array().expect("classes");
        assert!(in_force.len() <= 4_096, "{what}");
        assert_eq!(
            in_force.iter().filter(|class| class["id"] == "roi").count(),
            usize::from(what != "full"),
            "{what}"
        );
        let before = session.store.backend().revision().expect("revision");
        let added = session
            .server
            .put("/api/file/0/annotations")
            .json(&rows(
                &[[20, 10, 40, 30], [60, 60, 70, 70], [1, 2, 3, 4]],
                &[],
            ))
            .await;
        if adds {
            added.assert_status_ok();
        } else {
            assert_eq!(added.status_code(), StatusCode::BAD_REQUEST, "{what}");
            assert_eq!(added.json::<Value>()["code"], "bad_request", "{what}");
            assert_eq!(
                session.store.backend().revision().expect("revision"),
                before,
                "{what}: nothing written"
            );
            // Moving one and deleting the other makes no rectangle.
            session
                .server
                .put("/api/file/0/annotations")
                .json(&rows(&[[21, 11, 41, 31]], &[]))
                .await
                .assert_status_ok();
        }
        let text = session.text("/api/annotations/export.json").await;
        let again = Session::loading_document(study_files(dir.path()));
        let report = again
            .import(dir.path(), &text)
            .await
            .unwrap_or_else(|error| panic!("{what}: the export reloads: {error}"));
        assert_eq!(report["warnings"], json!([]), "{what}: {report}");
    }
}
