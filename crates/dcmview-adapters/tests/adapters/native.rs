//! The native formats: what is written, and what a strict import takes.

use super::support::{fixture_document_text, key, report_json, size, Loaded, Pool, Session};
use dcmview_adapters::limits::{MAX_DOCUMENT_BYTES, MAX_LINE_BYTES};
use dcmview_adapters::{
    Bound, ExportAdapter, ExportContext, ExportError, Fragment, ImportAdapter, ImportError,
    Imported, Limits, NativeJson, NativeJsonLines, Refusal,
};
use dcmview_annotation::{Document, FileKey, Geometry, ViolationCode};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;

const LCC: &str = "sop:1.2.826.0.1.3680043.8.498.1";
const DBT: &str = "sop:1.2.826.0.1.3680043.8.498.2";
const PHOTO: &str = "b3:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn fixture() -> Document {
    Document::from_json_str(&fixture_document_text()).expect("the fixture is a document")
}

/// The fixture's three files, loaded, each under the key the fixture has.
fn pool() -> Pool {
    let file = |path, key_text: &str, rows, columns, frames, size_bytes| Loaded {
        path,
        key: Some(key(key_text)),
        size: size(rows, columns, frames),
        size_bytes,
        refusal: None,
    };
    Pool(vec![
        file(
            "cohort_a/patient_0001/mg/lcc.dcm",
            LCC,
            160,
            240,
            1,
            34_819_072,
        ),
        file(
            "cohort_a/patient_0001/mg/dbt.dcm",
            DBT,
            160,
            240,
            3,
            115_200,
        ),
        file("photos/specimen 1.jpg", PHOTO, 100, 200, 1, 20_480),
    ])
}

fn export(adapter: &dyn ExportAdapter, document: &Document) -> (String, Value) {
    let mut out = Vec::new();
    let report = adapter
        .export(
            document,
            &ExportContext {
                paths: &BTreeMap::<FileKey, Vec<String>>::new(),
                max_bytes: u64::MAX,
                limits: Limits::DEFAULT,
            },
            &mut out,
        )
        .expect("export");
    (String::from_utf8(out).expect("text"), report_json(&report))
}

fn import_in(session: &Session, pool: &Pool, text: &str) -> Result<Imported, ImportError> {
    NativeJson.import(&mut text.as_bytes(), &session.context(pool))
}

/// Imports under the fixture's own schema.
fn import(pool: &Pool, text: &str) -> Result<Imported, ImportError> {
    import_in(&Session::new(fixture().schema), pool, text)
}

#[test]
fn a_document_is_written_whole_in_both_forms() {
    let document = fixture();

    let (text, report) = export(&NativeJson, &document);
    assert_eq!(
        text,
        serde_json::to_string(&document).expect("text"),
        "compact, with nothing after it"
    );
    let counts = json!({
        "files": 3, "annotations_in": 8, "annotations_out": 8,
        "labels_in": 8, "labels_out": 8, "unmatched": 0, "refused": 0,
    });
    assert_eq!(
        report,
        json!({
            "adapter": { "id": "native", "version": 1 }, "direction": "export",
            "counts": counts, "losses": [], "warnings": [],
        })
    );

    let (lines, report) = export(&NativeJsonLines, &document);
    assert_eq!(
        report["adapter"],
        json!({ "id": "native-jsonl", "version": 1 })
    );
    assert_eq!(report["counts"], counts);
    assert!(lines.ends_with('\n') && !lines.ends_with("\n\n"));
    let lines: Vec<&str> = lines.lines().collect();
    let mut header = document.clone();
    header.files.clear();
    header.layers.clear();
    header.annotations.clear();
    header.labels.clear();
    let line = |name: &str, value: &dyn erased::Text| format!("{{\"{name}\":{}}}", value.text());
    let mut expected = vec![serde_json::to_string(&header).expect("text")];
    expected.extend(document.files.iter().map(|file| line("file", file)));
    expected.extend(document.layers.iter().map(|layer| line("layer", layer)));
    expected.extend(
        document
            .annotations
            .iter()
            .map(|annotation| line("annotation", annotation)),
    );
    expected.extend(document.labels.iter().map(|label| line("label", label)));
    // The last line counts the lines before it, so that a file cut off at
    // a line can be told from a whole one.
    expected.push(r#"{"end":{"files":3,"layers":4,"annotations":8,"labels":8}}"#.to_string());
    assert_eq!(lines, expected);
}

/// Compact JSON of anything serializable, behind one object-safe name.
mod erased {
    pub trait Text {
        fn text(&self) -> String;
    }

    impl<T: serde::Serialize> Text for T {
        fn text(&self) -> String {
            serde_json::to_string(self).expect("text")
        }
    }
}

/// What was written is read back as it was, from either form and from a
/// document written over several lines, with the same report.
#[test]
fn both_forms_and_a_pretty_document_read_back_as_the_same_records() {
    let document = fixture();
    let pool = pool();
    let (json_text, _) = export(&NativeJson, &document);
    let (lines_text, _) = export(&NativeJsonLines, &document);
    let pretty = fixture_document_text();
    assert!(
        pretty.lines().count() > 100,
        "the fixture is pretty-printed"
    );

    let from_json = import(&pool, &json_text).expect("the JSON document");
    assert_eq!(
        from_json.fragment,
        Fragment {
            layers: document.layers.clone(),
            annotations: document.annotations.clone(),
            labels: document.labels.clone(),
        }
    );
    assert_eq!(
        report_json(&from_json.report),
        json!({
            "adapter": { "id": "native", "version": 1 }, "direction": "import",
            "counts": {
                "files": 3, "annotations_in": 8, "annotations_out": 8,
                "labels_in": 8, "labels_out": 8, "unmatched": 0, "refused": 0,
            },
            "losses": [], "warnings": [],
            "matched_by": { "key": 3, "sop_instance_uid": 0, "path": 0 },
        })
    );
    for (form, text) in [("JSON Lines", &lines_text), ("pretty", &pretty)] {
        let read = import(&pool, text).unwrap_or_else(|error| panic!("{form}: {error}"));
        assert_eq!(read, from_json, "{form}");
    }
    // Blank lines between records, and records in another order, change
    // nothing but the rows a report would name.
    let mut shuffled: Vec<&str> = lines_text.lines().collect();
    let end = shuffled.len() - 1;
    shuffled[1..end].reverse();
    let shuffled = shuffled.join("\n\n");
    let read = import(&pool, &shuffled).expect("lines in another order");
    assert_eq!(read.report, from_json.report);
    assert_eq!(read.fragment.annotations.len(), 8);
}

/// Where each file of the document ends up decides what happens to its
/// records, and the report says which.
#[test]
fn records_follow_their_file_or_are_counted_and_left_out() {
    let mut pool = pool();
    // The first file is found by its UID under another key, with another
    // size in bytes; the second has another frame count; the third is
    // not loaded at all.
    pool.0[1].size = size(160, 240, 60);
    pool.0.remove(2);
    let mut document: Value = serde_json::from_str(&fixture_document_text()).expect("JSON");
    let other = "b3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    // A file reference must agree with itself, so the first file names its
    // UID and is keyed by content here.
    document["files"][0]["key"] = json!(other);
    document["files"][0]["digest"] = json!(other);
    document["files"][0]["size_bytes"] = json!(7);
    let text = document
        .to_string()
        .replace(&format!("\"{LCC}\""), &format!("\"{other}\""));

    let imported = import(&pool, &text).expect("import");

    let files: Vec<&str> = imported
        .fragment
        .annotations
        .iter()
        .map(|record| record.file.as_str())
        .collect();
    assert_eq!(
        files,
        vec![LCC, LCC, LCC],
        "placed under the loaded file's key"
    );
    let report = report_json(&imported.report);
    assert_eq!(
        report["counts"],
        json!({
            "files": 1, "annotations_in": 8, "annotations_out": 3,
            // Five labels name no file and are taken as written; one is on
            // the first file.
            "labels_in": 8, "labels_out": 6, "unmatched": 2, "refused": 5,
        })
    );
    assert_eq!(
        report["warnings"],
        json!([
            { "code": "file_size_differs", "count": 1, "rows": [{ "row": 2 }] },
            { "code": "size_mismatch", "count": 1, "rows": [{ "row": 3 }] },
            { "code": "file_unmatched", "count": 1, "rows": [{ "row": 4 }] },
        ])
    );
    assert_eq!(
        report["matched_by"],
        json!({ "key": 0, "sop_instance_uid": 1, "path": 0 })
    );

    // What the resolver refuses is reported by its code, and a file
    // without a key is refused by the importer.
    for (refusal, keyless, code) in [
        (Some(Refusal::EvidenceConflict), false, "evidence_conflict"),
        (Some(Refusal::KeyUnavailable), false, "key_unavailable"),
        (None, true, "key_unavailable"),
    ] {
        let mut pool = self::pool();
        pool.0[0].refusal = refusal;
        if keyless {
            pool.0[0].key = None;
            pool.0[0].path = "cohort_a/patient_0001/mg/lcc.dcm";
        }
        let imported = import(&pool, &fixture_document_text()).expect("import");
        let report = report_json(&imported.report);
        assert_eq!(
            report["warnings"],
            json!([{ "code": code, "count": 1, "rows": [{ "row": 2 }] }]),
            "{code}"
        );
        assert_eq!(report["counts"]["refused"], json!(4), "{code}");
        assert_eq!(report["counts"]["files"], json!(2), "{code}");
    }
}

fn id(n: u32) -> String {
    format!("0199c0de-0000-7000-8000-{n:012x}")
}

/// One file of 160 rows by 240 columns with three frames, and the given
/// annotations on it, as a document without a schema.
fn small(layers: Value, annotations: Vec<Value>) -> String {
    json!({
        "format": "dcmview.annotations", "version": "1.0",
        "files": [{
            "key": DBT, "kind": "dicom", "sop_instance_uid": "1.2.826.0.1.3680043.8.498.2",
            "path": "cohort_a/patient_0001/mg/dbt.dcm", "size_bytes": 115200,
            "rows": 160, "columns": 240, "frames": 3, "format": "dicom",
        }],
        "layers": layers,
        "annotations": annotations,
    })
    .to_string()
}

fn record(n: u32, layer: &str, frames: Value, geometry: Value) -> Value {
    json!({
        "id": id(n), "file": DBT, "frames": frames, "layer": layer, "class": "roi",
        "geometry": geometry, "attributes": {}, "extensions": {},
        "rev": 4, "created_by": "import:embed", "created_at": "2026-10-10T08:00:00.000Z",
        "modified_by": "user:test", "modified_at": "2026-10-10T09:00:00.000Z",
    })
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    json!({ "type": "rect", "x0": x0, "y0": y0, "x1": x1, "y1": y1 })
}

fn layer(id: &str) -> Value {
    json!({ "id": id, "name": "Layer", "kind": "user" })
}

/// A rectangle the lenient EMBED import kept as read is in a native
/// export as it is held. The native import takes it back as read and
/// reports it; it is strict about everything else.
#[test]
fn a_box_kept_as_read_is_taken_back_and_reported_and_nothing_else_is_lenient() {
    let session = Session::new(None);
    let pool = pool();
    let kept = vec![
        record(1, "default", json!("all"), rect(0.0, 0.0, 241.0, 161.0)),
        record(
            2,
            "default",
            json!({ "set": [] }),
            rect(600.0, 500.0, 800.0, 700.0),
        ),
        // Without area, and inside the image: column 200 of 240.
        record(3, "default", json!("all"), rect(50.0, 40.0, 200.0, 40.0)),
        record(4, "default", json!("all"), rect(90.0, 60.0, 30.0, 20.0)),
        record(5, "default", json!("all"), rect(-3.0, 5.0, 9.0, 9.0)),
        // A sound one, off the thousandth of a pixel: quantized.
        record(
            6,
            "default",
            json!({ "set": [0, 2] }),
            rect(1.00049, 2.0, 3.0, 4.0),
        ),
    ];
    let text = small(json!([layer("default")]), kept.clone());

    let imported = import_in(&session, &pool, &text).expect("import");

    let geometries: Vec<Geometry> = imported
        .fragment
        .annotations
        .iter()
        .map(|record| record.geometry.clone())
        .collect();
    let geometry =
        |value: &Value| serde_json::from_value::<Geometry>(value.clone()).expect("geometry");
    let mut expected: Vec<Geometry> = kept
        .iter()
        .map(|record| geometry(&record["geometry"]))
        .collect();
    expected[5] = geometry(&rect(1.0, 2.0, 3.0, 4.0));
    assert_eq!(geometries, expected, "as read, a sound one quantized");
    assert!(
        imported.fragment.layers.is_empty(),
        "the caller's own layer is not handed back to it"
    );
    assert_eq!(
        report_json(&imported.report)["warnings"],
        json!([
            { "code": "out_of_range", "count": 3,
              "rows": [{ "row": 4 }, { "row": 5 }, { "row": 8 }] },
            { "code": "frames_empty", "count": 1, "rows": [{ "row": 5 }] },
            { "code": "zero_area", "count": 1, "rows": [{ "row": 6 }] },
            { "code": "corners_out_of_order", "count": 1, "rows": [{ "row": 7 }] },
        ])
    );

    // Everything else is refused, and refuses the whole import, with the
    // row of the record and the rule it broke.
    let polygon = json!({ "type": "polygon", "points": [
        { "x": 1, "y": 1 }, { "x": 900, "y": 1 }, { "x": 5, "y": 9 }] });
    let good = || record(1, "default", json!("all"), rect(1.0, 2.0, 3.0, 4.0));
    let mut classed = good();
    classed["class"] = json!("mass");
    let mut scored = record(2, "default", json!("all"), rect(0.0, 0.0, 241.0, 161.0));
    scored["score"] = json!(2);
    let cases: Vec<(&str, Value, Vec<Value>, u64, ViolationCode)> = vec![
        (
            "a polygon past the edge",
            json!([]),
            vec![good(), record(2, "default", json!("all"), polygon)],
            4,
            ViolationCode::OutOfBounds,
        ),
        (
            "a frame the file does not have",
            json!([]),
            vec![record(
                1,
                "default",
                json!({ "set": [3] }),
                rect(1.0, 2.0, 3.0, 4.0),
            )],
            3,
            ViolationCode::FrameOutOfRange,
        ),
        (
            "a box past the edge that also breaks another rule",
            json!([]),
            vec![good(), scored],
            4,
            ViolationCode::OutOfBounds,
        ),
        (
            "a class the session's schema does not have",
            json!([]),
            vec![classed],
            3,
            ViolationCode::UnknownClass,
        ),
        (
            "a layer the document does not have",
            json!([layer("second")]),
            vec![record(1, "third", json!("all"), rect(1.0, 2.0, 3.0, 4.0))],
            4,
            ViolationCode::UnknownLayer,
        ),
        (
            "an id used twice",
            json!([]),
            vec![good(), good()],
            4,
            ViolationCode::DuplicateId,
        ),
        (
            "a layer id used twice",
            json!([layer("second"), layer("second")]),
            vec![],
            4,
            ViolationCode::DuplicateId,
        ),
        (
            "a layer without a name",
            json!([{ "id": "second", "name": "", "kind": "user" }]),
            vec![],
            3,
            ViolationCode::BadLayer,
        ),
    ];
    for (what, layers, annotations, row, code) in cases {
        assert_eq!(
            import_in(&session, &pool, &small(layers, annotations)).err(),
            Some(ImportError::Invalid { row, code }),
            "{what}"
        );
    }

    // A record on a file the document does not describe, a file described
    // twice, and a file reference that contradicts itself.
    let mut orphan: Value = serde_json::from_str(&small(json!([]), vec![good()])).expect("JSON");
    orphan["annotations"][0]["file"] = json!(LCC);
    let mut twice: Value = serde_json::from_str(&small(json!([]), vec![])).expect("JSON");
    let copy = twice["files"][0].clone();
    twice["files"].as_array_mut().expect("files").push(copy);
    let mut contradicting: Value = serde_json::from_str(&small(json!([]), vec![])).expect("JSON");
    contradicting["files"][0]["sop_instance_uid"] = json!("9.9.9");
    for (what, document, row, code) in [
        ("an orphan", orphan, 3, ViolationCode::UnknownFile),
        ("a file twice", twice, 3, ViolationCode::DuplicateFileKey),
        (
            "a file against itself",
            contradicting,
            2,
            ViolationCode::BadFileRef,
        ),
    ] {
        assert_eq!(
            import_in(&session, &pool, &document.to_string()).err(),
            Some(ImportError::Invalid { row, code }),
            "{what}"
        );
    }

    // The fixture's records need the fixture's schema: under the implicit
    // one its first annotation's class does not exist, and the document's
    // own schema is not taken.
    assert_eq!(
        import_in(&session, &pool, &fixture_document_text()).err(),
        Some(ImportError::Invalid {
            row: 9,
            code: ViolationCode::UnknownClass
        })
    );
    let carried = small(json!([]), vec![good()]).replacen(
        "\"version\":\"1.0\"",
        "\"version\":\"1.0\",\"schema\":{\"schema_id\":\"other\",\"schema_version\":1,\"classes\":[],\"fields\":[]}",
        1,
    );
    assert_eq!(
        report_json(&import_in(&session, &pool, &carried).expect("import").report)["warnings"],
        json!([{ "code": "schema_not_applied", "count": 1, "rows": [{ "row": 1 }] }])
    );
}

#[test]
fn what_is_not_a_native_document_is_refused_by_what_is_wrong_with_it() {
    let pool = pool();
    let (lines, _) = export(&NativeJsonLines, &fixture());
    let lines: Vec<&str> = lines.lines().collect();
    let with = |place: usize, line: &str| {
        let mut changed = lines.clone();
        changed[place] = line;
        changed.join("\n")
    };
    let document = fixture_document_text();
    let cases: Vec<(&str, String, ImportError)> = vec![
        ("nothing", String::new(), ImportError::Malformed { row: 1 }),
        ("not JSON", "anon_dicom_path,ROI_coords\n".to_string(), ImportError::Malformed { row: 1 }),
        (
            "half a document",
            document[..document.len() / 2].to_string(),
            ImportError::Malformed { row: 1 },
        ),
        (
            "another format",
            document.replacen("dcmview.annotations", "dcmview.other", 1),
            ImportError::WrongFormat,
        ),
        (
            "another major version",
            document.replacen("\"1.0\"", "\"2.0\"", 1),
            ImportError::UnsupportedVersion,
        ),
        (
            "a line that is not a record",
            with(5, "{\"annotation\":{}}"),
            ImportError::Malformed { row: 6 },
        ),
        (
            "a line with two records",
            with(2, &format!("{{\"layer\":{},\"file\":{}}}", "{\"id\":\"x\",\"name\":\"X\",\"kind\":\"user\"}", "{}")),
            ImportError::Malformed { row: 3 },
        ),
        (
            "a line of an unknown kind",
            with(3, "{\"segment\":{}}"),
            ImportError::Malformed { row: 4 },
        ),
        (
            "a header that holds records of its own, before lines",
            format!("{}\n{}", serde_json::to_string(&fixture()).expect("text"), lines[1]),
            ImportError::Malformed { row: 1 },
        ),
        (
            "JSON nested deeper than the parser goes",
            format!(
                "{{\"format\":\"dcmview.annotations\",\"version\":\"1.0\",\"extensions\":{{\"x\":{}{}}}}}",
                "[".repeat(200),
                "]".repeat(200)
            ),
            ImportError::Malformed { row: 1 },
        ),
    ];
    for (what, text, expected) in cases {
        assert_eq!(import(&pool, &text).err(), Some(expected), "{what}");
    }
    assert_eq!(
        NativeJson
            .import(
                &mut &b"{\"format\": \"\xff\"}"[..],
                &Session::new(None).context(&pool)
            )
            .err(),
        Some(ImportError::Malformed { row: 1 }),
        "text that is not UTF-8"
    );

    // Told to stop, it hands nothing on.
    let session = Session::new(fixture().schema);
    let mut context = session.context(&pool);
    context.keep_going = &|| false;
    assert_eq!(
        NativeJson.import(&mut document.as_bytes(), &context).err(),
        Some(ImportError::Cancelled)
    );
}

/// An input that repeats one byte for ever after its start, counted.
struct Endless {
    start: std::io::Cursor<Vec<u8>>,
    given: u64,
}

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut read = self.start.read(buffer)?;
        if read == 0 {
            buffer.fill(b' ');
            read = buffer.len();
        }
        self.given += read as u64;
        Ok(read)
    }
}

#[test]
fn a_native_file_is_read_within_its_bounds() {
    let pool = pool();
    let session = Session::new(fixture().schema);
    let block = 64 * 1024;
    let (lines, _) = export(&NativeJsonLines, &fixture());
    let header = lines.lines().next().expect("header");

    // A record line that never ends.
    let mut input = Endless {
        start: std::io::Cursor::new(format!("{header}\n{{\"file\":").into_bytes()),
        given: 0,
    };
    assert_eq!(
        NativeJson.import(&mut input, &session.context(&pool)).err(),
        Some(ImportError::TooLarge {
            bound: Bound::LineBytes,
            limit: MAX_LINE_BYTES,
        })
    );
    assert!(input.given <= MAX_LINE_BYTES + header.len() as u64 + 2 * block);

    // A document that never ends, on one line and over several.
    for start in ["{\"format\":", "{\n\"format\":\n"] {
        let mut input = Endless {
            start: std::io::Cursor::new(start.as_bytes().to_vec()),
            given: 0,
        };
        assert_eq!(
            NativeJson.import(&mut input, &session.context(&pool)).err(),
            Some(ImportError::TooLarge {
                bound: Bound::DocumentBytes,
                limit: MAX_DOCUMENT_BYTES,
            })
        );
        assert!(
            input.given <= MAX_DOCUMENT_BYTES + 2 * block,
            "{}",
            input.given
        );
    }

    // An export is held to the caller's bound and, as one document, to the
    // format's.
    let document = fixture();
    let (text, _) = export(&NativeJson, &document);
    let (lines, _) = export(&NativeJsonLines, &document);
    for (adapter, whole) in [
        (&NativeJson as &dyn ExportAdapter, text.len() as u64),
        (&NativeJsonLines as &dyn ExportAdapter, lines.len() as u64),
    ] {
        let write = |max_bytes| {
            adapter
                .export(
                    &document,
                    &ExportContext {
                        paths: &BTreeMap::<FileKey, Vec<String>>::new(),
                        max_bytes,
                        limits: Limits::DEFAULT,
                    },
                    &mut Vec::new(),
                )
                .map(|_| ())
        };
        assert_eq!(write(whole), Ok(()));
        assert_eq!(
            write(whole - 1),
            Err(ExportError::TooLarge { limit: whole - 1 })
        );
    }
}

fn end_at(lines: &[&str]) -> usize {
    lines.len() - 1
}

/// A JSON Lines file that was cut off at a line is not a smaller document:
/// without its last line, or with one that counts other lines than were
/// read, nothing of it is taken.
#[test]
fn a_json_lines_file_without_its_end_is_refused_whole() {
    let pool = pool();
    let (text, _) = export(&NativeJsonLines, &fixture());
    let lines: Vec<&str> = text.lines().collect();
    let end = lines.len() - 1;
    assert!(lines[end].starts_with("{\"end\":"));

    // Cut off after any line but the last, the header included.
    for kept in 1..=end {
        let cut = format!("{}\n", lines[..kept].join("\n"));
        assert_eq!(
            import(&pool, &cut).err(),
            Some(ImportError::Truncated),
            "cut off after line {kept}"
        );
    }
    // The same text with no line feed after it is a JSON document that
    // holds no record, which no session writes and anyone may.
    let alone = import(&pool, lines[0]).expect("a document without a record");
    assert_eq!(alone.fragment, Fragment::default());

    // A line was taken out, or put in twice, and the end still counts the
    // lines that were written.
    let mut shorter = lines.clone();
    shorter.remove(end - 1);
    assert_eq!(
        import(&pool, &shorter.join("\n")).err(),
        Some(ImportError::Truncated)
    );
    // Two files joined: lines after an end.
    let joined = format!("{text}{}\n", lines[1]);
    assert_eq!(
        import(&pool, &joined).err(),
        Some(ImportError::Malformed {
            row: lines.len() as u64 + 1
        })
    );
    // An end line holds its four counts and nothing else.
    for end in [
        r#"{"end":{"files":3,"layers":4,"annotations":8,"labels":8,"more":0}}"#,
        r#"{"end":{"files":3,"layers":4,"annotations":8}}"#,
        r#"{"end":{"files":3,"layers":4,"annotations":8,"labels":"8"}}"#,
        r#"{"end":{"files":3,"layers":4,"annotations":8,"labels":8},"layer":null}"#,
    ] {
        let mut other = lines.clone();
        other[end_at(&lines)] = end;
        assert_eq!(
            import(&pool, &other.join("\n")).err(),
            Some(ImportError::Malformed {
                row: lines.len() as u64
            }),
            "{end}"
        );
    }
    // An end is not a record of a document that has none of its own.
    assert_eq!(
        import(
            &pool,
            &format!(
                "{}\n{}\n",
                lines[0], r#"{"end":{"files":0,"layers":0,"annotations":0,"labels":0}}"#
            )
        )
        .expect("a header and its end")
        .fragment,
        Fragment::default()
    );
}

/// A byte order mark before a document is passed over, by the sniff and by
/// the reader, in both forms.
#[test]
fn a_byte_order_mark_before_a_document_is_passed_over() {
    let pool = pool();
    let (json_text, _) = export(&NativeJson, &fixture());
    let (lines_text, _) = export(&NativeJsonLines, &fixture());
    let plain = import(&pool, &json_text).expect("without a mark");
    for text in [&json_text, &lines_text, &fixture_document_text()] {
        let marked = format!("\u{feff}{text}");
        assert_eq!(
            NativeJson.sniff(marked.as_bytes()),
            dcmview_adapters::Confidence::Certain
        );
        assert_eq!(import(&pool, &marked).expect("with a mark"), plain);
    }
    // A mark anywhere else is not passed over.
    assert!(import(&pool, &format!(" \u{feff}{json_text}")).is_err());
}

/// The bounds a caller hands over are the bounds that apply, each exactly:
/// what is at a bound is read or written, and one more is refused with the
/// bound and its number. An export refuses what an import within the same
/// bounds would refuse for its size, and labels count as records.
#[test]
fn documents_are_read_and_written_within_the_bounds_handed_over() {
    let pool = pool();
    let document = fixture();
    let session = Session::new(document.schema.clone());
    let (json_text, _) = export(&NativeJson, &document);
    let (lines_text, _) = export(&NativeJsonLines, &document);
    let read = |text: &str, limits: Limits| {
        let mut context = session.context(&pool);
        context.limits = limits;
        NativeJson
            .import(&mut text.as_bytes(), &context)
            .map(|imported| imported.report.counts.annotations_out)
    };
    let write = |adapter: &dyn ExportAdapter, limits: Limits| {
        adapter
            .export(
                &document,
                &ExportContext {
                    paths: &BTreeMap::<FileKey, Vec<String>>::new(),
                    max_bytes: u64::MAX,
                    limits,
                },
                &mut Vec::new(),
            )
            .map(|_| ())
    };
    let within = |change: &dyn Fn(&mut Limits)| {
        let mut limits = Limits::DEFAULT;
        change(&mut limits);
        limits
    };
    let too_large = |bound, limit| ImportError::TooLarge { bound, limit };

    // The fixture has 3 files, and 8 annotations and 8 labels: 16 records.
    for text in [&json_text, &lines_text] {
        assert_eq!(read(text, within(&|limits| limits.files = 3)), Ok(8));
        assert_eq!(
            read(text, within(&|limits| limits.files = 2)),
            Err(too_large(Bound::Files, 2))
        );
        assert_eq!(read(text, within(&|limits| limits.records = 16)), Ok(8));
        assert_eq!(
            read(text, within(&|limits| limits.records = 15)),
            Err(too_large(Bound::Records, 15)),
            "labels are records too"
        );
    }
    for adapter in [&NativeJson as &dyn ExportAdapter, &NativeJsonLines] {
        assert_eq!(write(adapter, within(&|limits| limits.files = 3)), Ok(()));
        assert_eq!(
            write(adapter, within(&|limits| limits.files = 2)),
            Err(ExportError::TooMany {
                bound: Bound::Files,
                limit: 2
            })
        );
        assert_eq!(
            write(adapter, within(&|limits| limits.records = 16)),
            Ok(())
        );
        assert_eq!(
            write(adapter, within(&|limits| limits.records = 15)),
            Err(ExportError::TooMany {
                bound: Bound::Records,
                limit: 15
            })
        );
    }

    // A JSON document has a bound on its bytes, on the way out and in.
    let bytes = json_text.len() as u64;
    assert_eq!(
        write(&NativeJson, within(&|limits| limits.document_bytes = bytes)),
        Ok(())
    );
    assert_eq!(
        write(
            &NativeJson,
            within(&|limits| limits.document_bytes = bytes - 1)
        ),
        Err(ExportError::TooLarge { limit: bytes - 1 })
    );
    assert_eq!(
        write(
            &NativeJsonLines,
            within(&|limits| limits.document_bytes = 10)
        ),
        Ok(()),
        "the lines have no bound on their bytes together"
    );
    assert_eq!(
        read(&json_text, within(&|limits| limits.document_bytes = bytes)),
        Ok(8)
    );
    assert_eq!(
        read(
            &json_text,
            within(&|limits| limits.document_bytes = bytes - 1)
        ),
        Err(too_large(Bound::DocumentBytes, bytes - 1))
    );

    // A line of the JSON Lines form has one of its own, its line feed not
    // counted.
    let longest = lines_text
        .lines()
        .skip(1)
        .map(str::len)
        .max()
        .expect("lines") as u64;
    assert_eq!(
        read(&lines_text, within(&|limits| limits.line_bytes = longest)),
        Ok(8)
    );
    assert_eq!(
        read(
            &lines_text,
            within(&|limits| limits.line_bytes = longest - 1)
        ),
        Err(too_large(Bound::LineBytes, longest - 1))
    );
}

/// Two labels of a document cannot share an id, as two annotations cannot.
#[test]
fn a_label_id_used_twice_fails_the_import() {
    let pool = pool();
    let mut document: Value = serde_json::from_str(&fixture_document_text()).expect("JSON");
    let first = document["labels"][0]["id"].clone();
    document["labels"][2]["id"] = first;
    // Rows: the document, 3 files, 4 layers, 8 annotations, then labels.
    assert_eq!(
        import(&pool, &document.to_string()).err(),
        Some(ImportError::Invalid {
            row: 19,
            code: ViolationCode::DuplicateId
        })
    );
}
