//! The wire format: what is written, and what is refused when read.

use super::support::{
    apply, document_value, fixture, operation_cases, read_envelope, Edit, DOCUMENT,
};
use dcmview_annotation::{
    ApplyResult, Context, Current, Document, DocumentError, Geometry, LabelValue, RevEntry,
    Violation, ViolationCode,
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
    ];
    for (value, text) in label_numbers {
        assert_eq!(
            serde_json::to_string(&LabelValue::Number(*value)).expect("value serializes"),
            *text,
            "label number {value}"
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
