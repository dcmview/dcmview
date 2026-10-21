//! The wire format: what is written, and what is refused when read.

use super::support::{
    apply, assert_no_member_twice, document, document_value, fixture, operation_cases,
    read_envelope, Edit, DOCUMENT,
};
use dcmview_annotation::{
    ApplyResult, Context, Current, Document, DocumentError, FieldDef, FieldType, Geometry,
    LabelValue, OpEnvelope, RevEntry, Violation, ViolationCode,
};
use serde_json::{json, Value};

/// One document that uses every member of the format, and members this
/// version does not know at every level that keeps them. Reading and writing
/// it must change nothing: not a member, not a default, not `340` to `340.0`.
#[test]
fn a_document_round_trips_with_every_member_and_the_unknown_ones() {
    let document = Document::from_json_str(DOCUMENT).expect("fixture document reads");
    let written = serde_json::to_value(&document).expect("document serializes");
    assert_eq!(written, document_value());
}

/// Every operation reads, writes back unchanged, is valid against the
/// fixture document's files and schema, and names the queue keys the fixture
/// lists for it, in order.
#[test]
fn every_operation_round_trips_and_names_its_queue_keys() {
    let fixture = fixture();
    let context = Context {
        files: &fixture.files,
        schema: &fixture.schema,
    };

    for case in operation_cases() {
        let envelope = read_envelope(&case.envelope);
        assert_eq!(
            serde_json::to_value(&envelope).expect("envelope serializes"),
            case.envelope,
            "{}: wire shape",
            case.name
        );
        if let Err(invalid) = envelope.validate(&context) {
            panic!(
                "{}: expected valid, got {:?}",
                case.name, invalid.violations
            );
        }
        let keys = envelope.op.queue_keys();
        let keys: Vec<&str> = keys.iter().map(|key| key.as_str()).collect();
        assert_eq!(keys, case.queue_keys, "{}: queue keys", case.name);
    }
}

/// What is written is text another reader takes: every member is written
/// once, and the text reads back to the value that was written. Comparing
/// through `serde_json::Value` cannot see a member written twice.
#[test]
fn written_text_reads_back_equal_and_names_no_member_twice() {
    let document = document();
    let text = serde_json::to_string(&document).expect("document serializes");
    assert_no_member_twice("document", &text);
    assert_eq!(
        Document::from_json_str(&text).expect("written document reads"),
        document
    );

    for case in operation_cases() {
        let envelope = read_envelope(&case.envelope);
        let text = serde_json::to_string(&envelope).expect("envelope serializes");
        assert_no_member_twice(&case.name, &text);
        assert_eq!(
            OpEnvelope::from_json_str(&text).expect("written envelope reads"),
            envelope,
            "{}",
            case.name
        );
    }

    // A field whose type was changed in Rust is written as the new type
    // alone: nothing of the type it was read with is left behind.
    let mut edited = document;
    let schema = edited.schema.as_mut().expect("fixture has a schema");
    for (index, field) in schema.fields.iter_mut().enumerate() {
        field.field_type = if index % 2 == 0 {
            FieldType::Boolean
        } else {
            FieldType::Text {
                max_length: Some(7),
            }
        };
    }
    let text = serde_json::to_string(&edited).expect("edited document serializes");
    assert_no_member_twice("edited document", &text);
    assert_eq!(
        Document::from_json_str(&text).expect("edited document reads"),
        edited
    );
}

/// Every type that keeps unknown members is one object shared by its own
/// members, sometimes a nested value written into the same object (a field's
/// type, a record's metadata), and the unknown ones. After reading the
/// fixture, which has every member, each type holds as unknown exactly the
/// members the fixture invents, and none of its own.
#[test]
fn reading_keeps_as_unknown_only_what_the_fixture_invents() {
    let document = document();
    let schema = document.schema.as_ref().expect("fixture has a schema");
    let mut kept: Vec<(String, Vec<String>)> = Vec::new();
    let mut note = |place: String, unknown: &std::collections::BTreeMap<String, Value>| {
        if !unknown.is_empty() {
            kept.push((place, unknown.keys().cloned().collect()));
        }
    };
    note("document".to_string(), &document.unknown);
    note("schema".to_string(), &schema.unknown);
    for (index, class) in schema.classes.iter().enumerate() {
        note(format!("class {index}"), &class.unknown);
    }
    for (index, field) in schema.fields.iter().enumerate() {
        note(format!("field {index}"), &field.unknown);
        if let FieldType::Category { options, .. } | FieldType::MultiCategory { options } =
            &field.field_type
        {
            for (option, def) in options.iter().enumerate() {
                note(format!("field {index} option {option}"), &def.unknown);
            }
        }
    }
    for (index, file) in document.files.iter().enumerate() {
        note(format!("file {index}"), &file.unknown);
        note(format!("file {index} space"), &file.space.unknown);
    }
    for (index, layer) in document.layers.iter().enumerate() {
        note(format!("layer {index}"), &layer.unknown);
        note(format!("layer {index} source"), &layer.source.unknown);
    }
    for (index, annotation) in document.annotations.iter().enumerate() {
        note(format!("annotation {index}"), &annotation.unknown);
    }
    for (index, label) in document.labels.iter().enumerate() {
        note(format!("label {index}"), &label.unknown);
    }
    let invented: Vec<(String, Vec<String>)> = [
        ("document", "producer"),
        ("schema", "owner_note"),
        ("class 0", "ui_group"),
        ("field 0 option 1", "retired_in"),
        ("field 4", "help"),
        ("file 1", "inventory_run"),
        ("file 2 space", "kind"),
        ("layer 1", "pinned"),
        ("layer 1 source", "file"),
        ("annotation 4", "future_member"),
        ("label 4", "future_member"),
    ]
    .into_iter()
    .map(|(place, member)| (place.to_string(), vec![member.to_string()]))
    .collect();
    assert_eq!(kept, invented);
}

/// A field is one object that holds its own members, its type's members and
/// the ones this version does not know. Reading sorts them: only the last
/// kind is kept as unknown, whatever the field type.
#[test]
fn a_field_keeps_only_the_members_it_does_not_know_as_unknown() {
    let option = json!({ "id": "a", "name": "A", "deprecated": false });
    // (wire, the members kept as unknown)
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (
            json!({ "id": "f", "name": "F", "type": "category", "options": [option.clone()],
                "ordered": true, "applies_to": ["file"], "required": true }),
            vec![],
        ),
        (
            json!({ "id": "f", "name": "F", "type": "multi_category", "options": [option.clone()],
                "applies_to": [], "required": false, "hint": "x" }),
            vec!["hint"],
        ),
        (
            json!({ "id": "f", "name": "F", "type": "boolean", "applies_to": [], "required": false }),
            vec![],
        ),
        (
            json!({ "id": "f", "name": "F", "type": "number", "min": 0, "max": 9.5, "step": 0.5,
                "unit": "mm", "integer": false, "applies_to": [], "required": false,
                "precision": 2 }),
            vec!["precision"],
        ),
        (
            json!({ "id": "f", "name": "F", "type": "text", "max_length": 80,
                "applies_to": [], "required": false }),
            vec![],
        ),
        // A member of another field type is not a member of this field.
        (
            json!({ "id": "f", "name": "F", "type": "boolean", "options": [option.clone()],
                "min": 1, "applies_to": [], "required": false }),
            vec!["min", "options"],
        ),
    ];
    for (wire, unknown) in cases {
        let field: FieldDef = serde_json::from_value(wire.clone()).expect("field reads");
        assert_eq!(
            field.unknown.keys().map(String::as_str).collect::<Vec<_>>(),
            unknown,
            "{wire}"
        );
        let text = serde_json::to_string(&field).expect("field serializes");
        assert_no_member_twice(&wire.to_string(), &text);
        assert_eq!(
            serde_json::from_str::<Value>(&text).expect("written field is JSON"),
            wire
        );
    }
}

/// A field changed in Rust can end up holding one name in two places: its
/// type's member and an entry of `unknown` (a `boolean` field read with
/// `options`, then made a `category`), or an entry named like one of the
/// field's own members. The field writes the member itself and leaves the
/// entry out, so the text names nothing twice and reads back.
#[test]
fn a_field_changed_in_rust_writes_each_member_once() {
    let option = json!({ "id": "a", "name": "A", "deprecated": false });
    let options: Vec<dcmview_annotation::OptionDef> =
        serde_json::from_value(json!([option.clone()])).expect("options read");
    // (name, wire read, unknown entries added, the new field type, wire written)
    type Case = (
        &'static str,
        Value,
        Vec<(&'static str, Value)>,
        Option<FieldType>,
        Value,
    );
    let cases: Vec<Case> = vec![
        (
            "boolean with options becomes a category",
            json!({ "id": "f", "name": "F", "type": "boolean", "options": [{ "id": "stale" }],
                "applies_to": [], "required": false, "hint": "x" }),
            vec![],
            Some(FieldType::Category {
                options: options.clone(),
                ordered: false,
            }),
            json!({ "id": "f", "name": "F", "type": "category", "options": [option.clone()],
                "ordered": false, "applies_to": [], "required": false, "hint": "x" }),
        ),
        (
            "boolean with options becomes a multi-category",
            json!({ "id": "f", "name": "F", "type": "boolean", "options": [{ "id": "stale" }],
                "ordered": true, "applies_to": [], "required": false }),
            vec![],
            Some(FieldType::MultiCategory {
                options: options.clone(),
            }),
            // `ordered` is not a member of a multi-category field: it stays.
            json!({ "id": "f", "name": "F", "type": "multi_category",
                "options": [option.clone()], "applies_to": [], "required": false,
                "ordered": true }),
        ),
        (
            "text with min and unit becomes a number",
            json!({ "id": "f", "name": "F", "type": "text", "min": 7, "unit": "cm",
                "applies_to": ["file"], "required": true }),
            vec![],
            Some(FieldType::Number {
                min: Some(1.0),
                max: None,
                step: None,
                unit: None,
                integer: true,
            }),
            json!({ "id": "f", "name": "F", "type": "number", "min": 1, "integer": true,
                "applies_to": ["file"], "required": true }),
        ),
        (
            "number with max_length becomes text",
            json!({ "id": "f", "name": "F", "type": "number", "integer": false,
                "max_length": 3, "applies_to": [], "required": false }),
            vec![],
            Some(FieldType::Text {
                max_length: Some(80),
            }),
            json!({ "id": "f", "name": "F", "type": "text", "max_length": 80,
                "applies_to": [], "required": false }),
        ),
        (
            "unknown entries named like the field's own members",
            json!({ "id": "f", "name": "F", "type": "boolean", "applies_to": ["file"],
                "required": true, "hint": "x" }),
            vec![
                ("id", json!("g")),
                ("name", json!("G")),
                ("type", json!("text")),
                ("applies_to", json!([])),
                ("required", json!(false)),
            ],
            None,
            json!({ "id": "f", "name": "F", "type": "boolean", "applies_to": ["file"],
                "required": true, "hint": "x" }),
        ),
    ];
    for (name, wire, added, field_type, written) in cases {
        let mut field: FieldDef = serde_json::from_value(wire).expect("field reads");
        for (member, value) in added {
            field.unknown.insert(member.to_string(), value);
        }
        if let Some(field_type) = field_type {
            field.field_type = field_type;
        }
        let text = serde_json::to_string(&field).expect("field serializes");
        assert_no_member_twice(name, &text);
        assert_eq!(
            serde_json::from_str::<Value>(&text).expect("written field is JSON"),
            written,
            "{name}"
        );
        let read: FieldDef = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name}: written text does not read: {error}"));
        assert_eq!(read.field_type, field.field_type, "{name}");
        assert_eq!(
            serde_json::to_string(&read).expect("field serializes"),
            text,
            "{name}"
        );
    }
}

/// A member named twice. Where the model names the member, the text is
/// refused; inside a map whose keys are data (attributes, extensions, mask
/// frames and tiles, members this version does not know) the last one is
/// kept, as JSON readers commonly do.
#[test]
fn a_member_named_twice_is_refused_where_the_model_names_it() {
    let text = document_value().to_string();
    // Writes `again` in front of the first place `written` occurs.
    let twice = |written: &str, again: &str| {
        assert!(text.contains(written), "the fixture has {written}");
        text.replacen(written, &format!("{again},{written}"), 1)
    };
    // (name, text, whether it reads)
    let cases: Vec<(&str, String, bool)> = vec![
        ("unchanged", text.clone(), true),
        (
            "a document member",
            twice(r#""version":"#, r#""version":"1.0""#),
            false,
        ),
        (
            "an annotation member",
            twice(r#""geometry":{"#, r#""geometry":null"#),
            false,
        ),
        (
            "a geometry's type",
            twice(r#""type":"rect""#, r#""type":"rect""#),
            false,
        ),
        // `rev` sits in the record metadata, which is written into the
        // record's own object.
        (
            "a record metadata member",
            twice(r#""rev":"#, r#""rev":1"#),
            false,
        ),
        (
            "a field's type",
            twice(r#""type":"category""#, r#""type":"boolean""#),
            false,
        ),
        (
            "a field type's member",
            twice(r#""max_length":"#, r#""max_length":5"#),
            false,
        ),
        (
            "a mask member",
            twice(r#""encoding":"#, r#""encoding":"tiles-v1""#),
            false,
        ),
        (
            "an attribute",
            twice(r#""clip_shape":"ribbon""#, r#""clip_shape":"coil""#),
            true,
        ),
        (
            "a member this version does not know",
            twice(r#""ui_group":"#, r#""ui_group":1"#),
            true,
        ),
    ];
    for (name, text, reads) in cases {
        let read = Document::from_json_str(&text);
        assert_eq!(read.is_ok(), reads, "{name}: {:?}", read.err());
        if let Ok(read) = read {
            assert_eq!(read, document(), "{name}: the last one is kept");
        }
    }
}

/// Numbers are written the same from Rust as from a JavaScript client:
/// coordinates quantized to 1/1000 px, whole numbers without a fraction.
#[test]
fn numbers_are_written_quantized_and_whole_numbers_without_a_fraction() {
    let coordinates: &[(f64, &str)] = &[
        (340.0, "340"),
        (0.0, "0"),
        (-0.0, "0"),
        (12.5, "12.5"),
        (12.3456, "12.346"),
        (0.1 + 0.2, "0.3"),
        (-0.0004, "0"),
        (0.0004, "0"),
        (-5.25, "-5.25"),
        (1048576.0, "1048576"),
        (1048575.999, "1048575.999"),
    ];
    for (value, text) in coordinates {
        let point = Geometry::Point { x: *value, y: 0.0 };
        assert_eq!(
            serde_json::to_string(&point).expect("point serializes"),
            format!(r#"{{"type":"point","x":{text},"y":0}}"#),
            "coordinate {value}"
        );
    }

    let label_numbers: &[(f64, &str)] = &[
        (30.0, "30"),
        (-0.0, "0"),
        (0.5, "0.5"),
        (-2.0, "-2"),
        (12.3456, "12.3456"),
        // The largest whole number written without a fraction is 2^53 - 1,
        // the largest a JavaScript reader takes exactly. From 2^53 on a
        // number is written as serde_json writes any `f64`.
        (9_007_199_254_740_991.0, "9007199254740991"),
        (-9_007_199_254_740_991.0, "-9007199254740991"),
        (9_007_199_254_740_992.0, "9007199254740992.0"),
        (-9_007_199_254_740_992.0, "-9007199254740992.0"),
    ];
    for (value, text) in label_numbers {
        assert_eq!(
            serde_json::to_string(&LabelValue::Number(*value)).expect("value serializes"),
            *text,
            "label number {value}"
        );
        assert_eq!(
            serde_json::from_str::<LabelValue>(text).expect("value reads"),
            LabelValue::Number(if *value == 0.0 { 0.0 } else { *value }),
            "label number {value} read back"
        );
    }

    for not_finite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(serde_json::to_string(&Geometry::Point {
            x: not_finite,
            y: 0.0
        })
        .is_err());
        assert!(serde_json::to_string(&LabelValue::Number(not_finite)).is_err());
    }
}

/// What reading refuses: a document of another format or major version, and
/// values whose shape or fixed syntax is wrong.
#[test]
fn reading_refuses_other_formats_versions_and_malformed_values() {
    #[derive(Debug, PartialEq)]
    enum Read {
        Ok,
        Malformed,
        WrongFormat,
        UnsupportedVersion,
    }
    let cases: Vec<(&str, Vec<Edit>, Read)> = vec![
        ("unchanged", vec![], Read::Ok),
        (
            "a later minor version reads",
            vec![Edit::Set("/version", json!("1.7"))],
            Read::Ok,
        ),
        (
            "members with defaults may be absent",
            vec![
                Edit::Remove("/extensions"),
                Edit::Remove("/labels"),
                Edit::Remove("/annotations/0/attributes"),
                Edit::Remove("/annotations/0/extensions"),
                Edit::Remove("/layers/0/color"),
                Edit::Remove("/files/0/digest"),
                Edit::Remove("/schema/fields/0/required"),
            ],
            Read::Ok,
        ),
        (
            "another format",
            vec![Edit::Set("/format", json!("coco"))],
            Read::WrongFormat,
        ),
        (
            "another major version",
            vec![Edit::Set("/version", json!("2.0"))],
            Read::UnsupportedVersion,
        ),
        (
            "a version that is not major.minor",
            vec![Edit::Set("/version", json!("1"))],
            Read::UnsupportedVersion,
        ),
        (
            "a version with a suffix",
            vec![Edit::Set("/version", json!("1.0-beta"))],
            Read::UnsupportedVersion,
        ),
        // The major version is the text `1` and nothing else.
        (
            "a major version with a leading zero",
            vec![Edit::Set("/version", json!("01.0"))],
            Read::UnsupportedVersion,
        ),
        (
            "a major version padded to nine digits",
            vec![Edit::Set("/version", json!("000000001.0"))],
            Read::UnsupportedVersion,
        ),
        (
            "a minor version of nine digits",
            vec![Edit::Set("/version", json!("1.999999999"))],
            Read::Ok,
        ),
        (
            "a minor version of ten digits",
            vec![Edit::Set("/version", json!("1.9999999999"))],
            Read::UnsupportedVersion,
        ),
        (
            "a version without its minor",
            vec![Edit::Set("/version", json!("1."))],
            Read::UnsupportedVersion,
        ),
        (
            "a version of three numbers",
            vec![Edit::Set("/version", json!("1.0.0"))],
            Read::UnsupportedVersion,
        ),
        (
            "a signed version",
            vec![Edit::Set("/version", json!("+1.0"))],
            Read::UnsupportedVersion,
        ),
        (
            "a version that is a number",
            vec![Edit::Set("/version", json!(1.0))],
            Read::Malformed,
        ),
        ("no format", vec![Edit::Remove("/format")], Read::Malformed),
        (
            "a file key with no body",
            vec![Edit::Set("/annotations/0/file", json!("sop:"))],
            Read::Malformed,
        ),
        (
            "a file key with an unknown scheme",
            vec![Edit::Set("/annotations/0/file", json!("path:/a/b.dcm"))],
            Read::Malformed,
        ),
        (
            "an unknown geometry type",
            vec![Edit::Set("/annotations/0/geometry/type", json!("circle"))],
            Read::Malformed,
        ),
        (
            "a coordinate that is not a number",
            vec![Edit::Set("/annotations/0/geometry/x0", json!("30"))],
            Read::Malformed,
        ),
        (
            "a frame scope keyword that does not exist",
            vec![Edit::Set("/annotations/0/frames", json!("none"))],
            Read::Malformed,
        ),
        (
            "a bare frame list",
            vec![Edit::Set("/annotations/0/frames", json!([0]))],
            Read::Malformed,
        ),
        (
            "a label target of two kinds",
            vec![Edit::Set(
                "/labels/0/target",
                json!({ "patient": "P1", "study": "1.2.3" }),
            )],
            Read::Malformed,
        ),
        (
            "a frame target without its index",
            vec![Edit::Set(
                "/labels/4/target",
                json!({ "frame": "sop:1.2.3" }),
            )],
            Read::Malformed,
        ),
        (
            "a layer id with a space",
            vec![Edit::Set("/layers/0/id", json!("layer one"))],
            Read::Malformed,
        ),
        (
            "an author without a kind",
            vec![Edit::Set("/annotations/0/created_by", json!("alice"))],
            Read::Malformed,
        ),
        (
            "a timestamp in another spelling",
            vec![Edit::Set(
                "/annotations/0/created_at",
                json!("2026-09-29 21:04:11"),
            )],
            Read::Malformed,
        ),
        (
            "an id that is not a UUID",
            vec![Edit::Set("/annotations/0/id", json!("annotation-1"))],
            Read::Malformed,
        ),
        (
            "a tile position that is not tx,ty",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "0;0": "AAAA" }),
            )],
            Read::Malformed,
        ),
        // Tile positions and frame indices are object keys, so each has one
        // spelling: decimal, no sign, no space, no leading zero.
        (
            "a tile position with a leading zero",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "01,2": "AAAA" }),
            )],
            Read::Malformed,
        ),
        (
            "a tile row with a leading zero",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "1,02": "AAAA" }),
            )],
            Read::Malformed,
        ),
        (
            "a tile position with a space",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "1, 2": "AAAA" }),
            )],
            Read::Malformed,
        ),
        (
            "a tile position of three numbers",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "1,2,3": "AAAA" }),
            )],
            Read::Malformed,
        ),
        (
            "a tile column past u32",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "4294967296,0": "AAAA" }),
            )],
            Read::Malformed,
        ),
        (
            "the largest tile position reads",
            vec![Edit::Set(
                "/annotations/7/geometry/frames/0",
                json!({ "4294967295,4294967295": "AAAA" }),
            )],
            Read::Ok,
        ),
        (
            "a frame index with a leading zero",
            vec![Edit::Set(
                "/annotations/7/geometry/frames",
                json!({ "00": { "0,0": "AAAA" } }),
            )],
            Read::Malformed,
        ),
        (
            "a frame index with a sign",
            vec![Edit::Set(
                "/annotations/7/geometry/frames",
                json!({ "+1": { "0,0": "AAAA" } }),
            )],
            Read::Malformed,
        ),
        (
            "a frame index past u32",
            vec![Edit::Set(
                "/annotations/7/geometry/frames",
                json!({ "4294967296": { "0,0": "AAAA" } }),
            )],
            Read::Malformed,
        ),
        (
            "the largest frame index reads",
            vec![Edit::Set(
                "/annotations/7/geometry/frames",
                json!({ "4294967295": { "0,0": "AAAA" } }),
            )],
            Read::Ok,
        ),
        (
            "an unknown layer kind",
            vec![Edit::Set("/layers/0/kind", json!("redaction"))],
            Read::Malformed,
        ),
    ];

    for (name, edits, expected) in cases {
        let text = apply(document_value(), &edits).to_string();
        let read = match Document::from_json_str(&text) {
            Ok(_) => Read::Ok,
            Err(DocumentError::Malformed(_)) => Read::Malformed,
            Err(DocumentError::WrongFormat { .. }) => Read::WrongFormat,
            Err(DocumentError::UnsupportedVersion { .. }) => Read::UnsupportedVersion,
            Err(other) => panic!("{name}: unexpected error {other:?}"),
        };
        assert_eq!(read, expected, "{name}");
    }
}

/// The result of an envelope, as a store answers and a client reads it.
#[test]
fn apply_results_keep_their_wire_shape() {
    let cases: Vec<(ApplyResult, Value)> = vec![
        (
            ApplyResult::Ok {
                revs: vec![
                    RevEntry {
                        id: "0199c0de-0000-7000-8000-000000000001".to_string(),
                        rev: 5,
                    },
                    RevEntry {
                        id: "L-01".to_string(),
                        rev: 2,
                    },
                ],
            },
            json!({ "status": "ok", "revs": [
                { "id": "0199c0de-0000-7000-8000-000000000001", "rev": 5 },
                { "id": "L-01", "rev": 2 }
            ] }),
        ),
        (
            ApplyResult::Conflict {
                current: Current::Deleted {
                    id: "0199c0de-0000-7000-8000-000000000001".to_string(),
                    rev: 6,
                },
            },
            json!({ "status": "conflict", "current": {
                "kind": "deleted", "id": "0199c0de-0000-7000-8000-000000000001", "rev": 6
            } }),
        ),
        (
            ApplyResult::Conflict {
                current: Current::Missing {
                    id: "L-09".to_string(),
                },
            },
            json!({ "status": "conflict", "current": { "kind": "missing", "id": "L-09" } }),
        ),
        (
            ApplyResult::Invalid {
                violations: vec![Violation {
                    code: ViolationCode::OutOfBounds,
                    path: "/annotation/geometry/x1".to_string(),
                    detail: "x1 is past the last column".to_string(),
                }],
            },
            json!({ "status": "invalid", "violations": [{
                "code": "out_of_bounds",
                "path": "/annotation/geometry/x1",
                "detail": "x1 is past the last column"
            }] }),
        ),
    ];

    for (result, wire) in cases {
        assert_eq!(serde_json::to_value(&result).expect("serializes"), wire);
        assert_eq!(
            serde_json::from_value::<ApplyResult>(wire).expect("reads"),
            result
        );
    }
}
