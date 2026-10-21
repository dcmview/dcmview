//! The strict check of a document: what the native format requires.

use super::support::{
    apply, assert_outcome, document, document_value, Edit, Expect, FILE_A, FILE_B, UUID_V4,
};
use dcmview_annotation::{
    limits::MAX_VIOLATIONS, Context, Document, FileKey, Geometry, ImageSize, LabelSchema,
    ViolationCode,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn validate(value: Value) -> Result<(), dcmview_annotation::Invalid> {
    Document::from_json_str(&value.to_string())
        .expect("edited document still reads")
        .validate()
}

#[test]
fn the_fixture_document_is_valid() {
    assert_outcome("fixture", document().validate(), Expect::Valid);
}

/// The fixture document with one thing changed. Each row names the rule it
/// breaks, or is a change that must stay valid.
#[test]
fn a_document_that_breaks_one_rule_reports_it() {
    use Edit::{Push, Set};
    use Expect::{Code, Valid};
    use ViolationCode::*;

    let fixture = document_value();
    let first_annotation_id = fixture["annotations"][0]["id"].clone();
    let mut second_label_of_a_target = fixture["labels"][0].clone();
    second_label_of_a_target["id"] = json!("0199c0de-0000-7000-8000-00000000aaaa");
    let mut same_label_by_another_author = second_label_of_a_target.clone();
    same_label_by_another_author["created_by"] = json!("user:bob");
    let mut option_twice = fixture["schema"]["fields"][0]["options"].clone();
    let first_option = option_twice[0].clone();
    option_twice.as_array_mut().unwrap().push(first_option);

    let cases: Vec<(&str, Vec<Edit>, Expect)> = vec![
        // Geometry and frames, through the annotation that holds them.
        (
            "rect past the last column",
            vec![Set("/annotations/0/geometry/x1", json!(241))],
            Code(OutOfBounds),
        ),
        (
            "rect with no width",
            vec![Set("/annotations/0/geometry/x1", json!(30))],
            Code(Degenerate),
        ),
        (
            "frame past the last",
            vec![Set("/annotations/1/frames", json!({ "set": [0, 5] }))],
            Code(FrameOutOfRange),
        ),
        (
            "list as written that names another frame",
            vec![Set(
                "/annotations/1/frames",
                json!({ "set": [0, 2], "as_written": [1] }),
            )],
            Code(FramesAsWrittenMismatch),
        ),
        (
            "list as written dropped by an edit",
            vec![Set("/annotations/1/frames", json!({ "set": [1] }))],
            Valid,
        ),
        (
            "mask on all frames",
            vec![Set("/annotations/7/frames", json!("all"))],
            Code(MaskFramesMismatch),
        ),
        (
            "mask on fewer frames than its tiles",
            vec![Set("/annotations/7/frames", json!({ "set": [0] }))],
            Code(MaskFramesMismatch),
        ),
        (
            "mask frames with a list as written",
            vec![Set(
                "/annotations/7/frames",
                json!({ "set": [0, 2], "as_written": [2, 0] }),
            )],
            Code(MaskFramesMismatch),
        ),
        // References.
        (
            "annotation on an unknown file",
            vec![Set("/annotations/0/file", json!("sop:9.9.9"))],
            Code(UnknownFile),
        ),
        (
            "annotation in an unknown layer",
            vec![Set("/annotations/0/layer", json!("L-nope"))],
            Code(UnknownLayer),
        ),
        (
            "annotation of an unknown class",
            vec![Set("/annotations/0/class", json!("nope"))],
            Code(UnknownClass),
        ),
        (
            "label in an unknown layer",
            vec![Set("/labels/1/layer", json!("L-nope"))],
            Code(UnknownLayer),
        ),
        (
            "label of an unknown field",
            vec![Set("/labels/2/field", json!("nope"))],
            Code(UnknownField),
        ),
        (
            "label on an unknown file",
            vec![Set("/labels/3/target/file", json!("sop:9.9.9"))],
            Code(UnknownFile),
        ),
        (
            "label on a frame past the last",
            vec![Set("/labels/4/target/index", json!(3))],
            Code(FrameOutOfRange),
        ),
        // Ids.
        (
            "annotation id that is not version 7",
            vec![Set("/annotations/0/id", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        (
            "label id that is not version 7",
            vec![Set("/labels/0/id", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        (
            "derived_from that is not version 7",
            vec![Set("/annotations/6/derived_from", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        // Version 7 by its version digit alone is not a UUIDv7: the variant
        // must be the RFC one (the first digit of the fourth group is 8 to b).
        (
            "annotation id of version 7 in the Microsoft variant",
            vec![Set(
                "/annotations/0/id",
                json!("0199c0de-0000-7000-c000-00000000bbbb"),
            )],
            Code(IdNotUuidV7),
        ),
        (
            "label id of version 7 in the NCS variant",
            vec![Set(
                "/labels/0/id",
                json!("0199c0de-0000-7000-0000-00000000bbbb"),
            )],
            Code(IdNotUuidV7),
        ),
        (
            "two annotations with one id",
            vec![Set("/annotations/1/id", first_annotation_id)],
            Code(DuplicateId),
        ),
        (
            "two layers with one id",
            vec![Set("/layers/1/id", json!("L-01"))],
            Code(DuplicateId),
        ),
        (
            "two files with one key",
            vec![
                Set("/files/1/key", json!(FILE_A)),
                Set("/files/1/sop_instance_uid", json!(&FILE_A[4..])),
            ],
            Code(DuplicateFileKey),
        ),
        (
            "two labels for one target, field, layer and author",
            vec![Push("/labels", second_label_of_a_target)],
            Code(DuplicateLabel),
        ),
        (
            "the same label by another author",
            vec![Push("/labels", same_label_by_another_author)],
            Valid,
        ),
        // Classes and attributes.
        (
            "geometry the class does not allow",
            vec![Set("/annotations/3/class", json!("clip"))],
            Code(GeometryNotAllowed),
        ),
        (
            "attribute the class does not carry",
            vec![Set("/annotations/0/attributes", json!({ "note": "x" }))],
            Code(AttributeNotAllowed),
        ),
        (
            "attribute with an unknown option",
            vec![Set("/annotations/0/attributes/clip_shape", json!("nope"))],
            Code(UnknownOption),
        ),
        (
            "attribute with a deprecated option",
            vec![Set("/annotations/0/attributes/clip_shape", json!("coil"))],
            Valid,
        ),
        (
            "attribute of the wrong type",
            vec![Set("/annotations/0/attributes/clip_shape", json!(true))],
            Code(ValueType),
        ),
        (
            "annotation of a deprecated class",
            vec![Set("/schema/classes/0/deprecated", json!(true))],
            Valid,
        ),
        (
            "a required field with no label",
            vec![
                Set("/labels/2/field", json!("note")),
                Set("/labels/2/target", json!({ "file": FILE_A })),
                Set("/labels/2/value", json!("x")),
                Set("/labels/3/layer", json!("L-import")),
            ],
            Valid,
        ),
        // Label values against their fields.
        (
            "number above its maximum",
            vec![Set("/labels/1/value", json!(101))],
            Code(ValueOutOfRange),
        ),
        (
            "number below its minimum",
            vec![Set("/labels/1/value", json!(-1))],
            Code(ValueOutOfRange),
        ),
        (
            "fraction where an integer is wanted",
            vec![Set("/labels/1/value", json!(30.5))],
            Code(ValueOutOfRange),
        ),
        (
            "number on its maximum",
            vec![Set("/labels/1/value", json!(100))],
            Valid,
        ),
        (
            "string where a number is wanted",
            vec![Set("/labels/1/value", json!("30"))],
            Code(ValueType),
        ),
        (
            "number where a boolean is wanted",
            vec![Set("/labels/0/value", json!(1))],
            Code(ValueType),
        ),
        (
            "list where one option is wanted",
            vec![Set("/labels/2/value", json!(["good"]))],
            Code(ValueType),
        ),
        (
            "one option where a list is wanted",
            vec![Set("/labels/6/value", json!("calc"))],
            Code(ValueType),
        ),
        (
            "list with an unknown option",
            vec![Set("/labels/6/value", json!(["calc", "nope"]))],
            Code(UnknownOption),
        ),
        (
            "list with an option twice",
            vec![Set("/labels/6/value", json!(["calc", "calc"]))],
            Code(DuplicateId),
        ),
        (
            "empty list of options",
            vec![Set("/labels/6/value", json!([]))],
            Valid,
        ),
        (
            "text at its length limit",
            vec![Set("/labels/3/value", json!("x".repeat(200)))],
            Valid,
        ),
        (
            "text past its length limit",
            vec![Set("/labels/3/value", json!("x".repeat(201)))],
            Code(TooLong),
        ),
        // Lengths are counted in bytes, not characters.
        (
            "text of 200 bytes in two-byte characters",
            vec![Set("/labels/3/value", json!("\u{e9}".repeat(100)))],
            Valid,
        ),
        (
            "text of 101 two-byte characters",
            vec![Set("/labels/3/value", json!("\u{e9}".repeat(101)))],
            Code(TooLong),
        ),
        (
            "field that does not apply to the target",
            vec![Set("/labels/2/target", json!({ "patient": "P1" }))],
            Code(TargetNotAllowed),
        ),
        // Targets.
        (
            "patient with an empty id",
            vec![Set("/labels/0/target/patient", json!(""))],
            Code(BadTarget),
        ),
        (
            "patient id with a control character",
            vec![Set("/labels/0/target/patient", json!("P1\u{0}"))],
            Code(BadTarget),
        ),
        (
            "patient id of 256 bytes",
            vec![Set("/labels/0/target/patient", json!("p".repeat(256)))],
            Valid,
        ),
        (
            "patient id of 257 bytes",
            vec![Set("/labels/0/target/patient", json!("p".repeat(257)))],
            Code(BadTarget),
        ),
        (
            "patient id of 129 two-byte characters",
            vec![Set("/labels/0/target/patient", json!("\u{e9}".repeat(129)))],
            Code(BadTarget),
        ),
        (
            "folder with a dot segment",
            vec![Set("/labels/5/target/folder", json!("a/./b"))],
            Code(BadTarget),
        ),
        (
            "folder with an empty segment",
            vec![Set("/labels/5/target/folder", json!("a//b"))],
            Code(BadTarget),
        ),
        (
            "folder that climbs out of its root",
            vec![Set("/labels/5/target/folder", json!("a/../../b"))],
            Code(BadTarget),
        ),
        (
            "absolute folder",
            vec![Set("/labels/5/target/folder", json!("/data/a"))],
            Code(BadTarget),
        ),
        (
            "folder with a backslash",
            vec![Set("/labels/5/target/folder", json!("a\\b"))],
            Code(BadTarget),
        ),
        (
            "folder root with a slash",
            vec![Set("/labels/5/target/root", json!("r/x"))],
            Code(BadTarget),
        ),
        (
            "the root folder itself",
            vec![Set("/labels/5/target/folder", json!(""))],
            Valid,
        ),
        // Record metadata.
        (
            "score above 1",
            vec![Set("/annotations/6/score", json!(1.5))],
            Code(BadScore),
        ),
        (
            "score below 0",
            vec![Set("/labels/6/score", json!(-0.1))],
            Code(BadScore),
        ),
        (
            "score of exactly 1",
            vec![Set("/annotations/6/score", json!(1))],
            Valid,
        ),
        // Files.
        (
            "rows past the dimension bound",
            vec![Set("/files/0/rows", json!(2_000_000))],
            Code(BadFileRef),
        ),
        (
            "sop key that is not the file's UID",
            vec![Set("/files/0/sop_instance_uid", json!("1.2.3"))],
            Code(BadFileRef),
        ),
        (
            "one pixel digest too many",
            vec![Set("/files/2/pixel_digest", json!([null, null]))],
            Code(BadFileRef),
        ),
        (
            "frame source of the wrong length",
            vec![Set("/files/2/frame_source", json!([0, 2]))],
            Code(BadFileRef),
        ),
        (
            "orientation value that does not exist",
            vec![Set("/files/2/space/exif_orientation", json!(9))],
            Code(BadFileRef),
        ),
        (
            "orientation value 0",
            vec![Set("/files/2/space/exif_orientation", json!(0))],
            Code(BadFileRef),
        ),
        (
            "the first orientation value",
            vec![Set("/files/2/space/exif_orientation", json!(1))],
            Valid,
        ),
        (
            "the last orientation value",
            vec![Set("/files/2/space/exif_orientation", json!(8))],
            Valid,
        ),
        (
            "file patient id of 256 bytes",
            vec![Set("/files/0/patient_id", json!("p".repeat(256)))],
            Valid,
        ),
        (
            "file patient id of 257 bytes",
            vec![Set("/files/0/patient_id", json!("p".repeat(257)))],
            Code(TooLong),
        ),
        (
            "spacing of zero",
            vec![Set("/files/0/spacing/0/row_mm", json!(0))],
            Code(BadFileRef),
        ),
        (
            "b3 key whose own digest is another",
            vec![Set(
                "/files/2/digest",
                json!(format!("b3:{}", "1".repeat(64))),
            )],
            Code(BadFileRef),
        ),
        (
            "b3 key beside a digest of another kind",
            vec![Set(
                "/files/2/digest",
                json!(format!("sha256:{}", "a".repeat(64))),
            )],
            Valid,
        ),
        (
            "b3 key without a digest",
            vec![Set("/files/2/digest", json!(null))],
            Valid,
        ),
        (
            "digest in uppercase",
            vec![Set(
                "/files/2/digest",
                json!(format!("b3:{}", "A".repeat(64))),
            )],
            Code(BadDigest),
        ),
        (
            "digest without a scheme",
            vec![Set("/files/2/digest", json!("0".repeat(64)))],
            Code(BadDigest),
        ),
        (
            "sha256 digest",
            vec![Set(
                "/files/0/digest",
                json!(format!("sha256:{}", "0".repeat(64))),
            )],
            Valid,
        ),
        (
            "pixel digest with the file scheme",
            vec![Set(
                "/files/2/pixel_digest",
                json!([format!("b3:{}", "0".repeat(64))]),
            )],
            Code(BadDigest),
        ),
        (
            "file without pixels, labelled as a file",
            vec![
                Set("/files/1/rows", json!(0)),
                Set("/files/1/columns", json!(0)),
                Set("/files/1/frames", json!(0)),
                Set("/annotations", json!([])),
                Set("/labels/4/target", json!({ "file": FILE_B })),
                Set("/labels/4/field", json!("note")),
                Set("/labels/4/value", json!("x")),
            ],
            Valid,
        ),
        (
            "frame of a file without pixels",
            vec![
                Set("/files/1/rows", json!(0)),
                Set("/files/1/columns", json!(0)),
                Set("/files/1/frames", json!(0)),
                Set("/annotations", json!([])),
            ],
            Code(FrameOutOfRange),
        ),
        // Layers.
        (
            "review layer that can be written",
            vec![Set("/layers/3/readonly", json!(false))],
            Code(BadLayer),
        ),
        (
            "layer without a name",
            vec![Set("/layers/0/name", json!(""))],
            Code(BadLayer),
        ),
        (
            "layer color that is not #RRGGBB",
            vec![Set("/layers/1/color", json!("#12345"))],
            Code(BadColor),
        ),
        // The schema.
        (
            "two classes with one id",
            vec![Set("/schema/classes/1/id", json!("clip"))],
            Code(DuplicateId),
        ),
        (
            "two fields with one id",
            vec![Set("/schema/fields/1/id", json!("clip_shape"))],
            Code(DuplicateId),
        ),
        (
            "one option twice in a field",
            vec![Set("/schema/fields/0/options", option_twice)],
            Code(DuplicateId),
        ),
        (
            "class id with a space",
            vec![Set("/schema/classes/1/id", json!("soft mass"))],
            Code(BadId),
        ),
        (
            "empty schema id",
            vec![Set("/schema/schema_id", json!(""))],
            Code(BadId),
        ),
        (
            "class attribute that is not a field",
            vec![Set("/schema/classes/1/attributes", json!(["nope"]))],
            Code(UnknownField),
        ),
        (
            "class that allows no geometry",
            vec![Set("/schema/classes/2/geometry", json!([]))],
            Code(BadSchema),
        ),
        (
            "category without options",
            vec![Set("/schema/fields/5/options", json!([]))],
            Code(BadSchema),
        ),
        (
            "number field with min above max",
            vec![Set("/schema/fields/2/min", json!(200))],
            Code(BadSchema),
        ),
        (
            "class color that is not #RRGGBB",
            vec![Set("/schema/classes/0/color", json!("red"))],
            Code(BadColor),
        ),
        (
            "class name past its bound",
            vec![Set("/schema/classes/0/name", json!("n".repeat(257)))],
            Code(TooLong),
        ),
        (
            "class name of 256 bytes in two-byte characters",
            vec![Set("/schema/classes/0/name", json!("\u{e9}".repeat(128)))],
            Valid,
        ),
        (
            "class name of 129 two-byte characters",
            vec![Set("/schema/classes/0/name", json!("\u{e9}".repeat(129)))],
            Code(TooLong),
        ),
    ];

    for (name, edits, expected) in cases {
        let edited = apply(fixture.clone(), &edits);
        let outcome = validate(edited.clone());
        // A client shows a violation at its path, so the path names
        // something that is in the document.
        if let Err(invalid) = &outcome {
            for violation in &invalid.violations {
                assert!(
                    edited.pointer(&violation.path).is_some(),
                    "{name}: {:?} is not in the document",
                    violation.path
                );
            }
        }
        assert_outcome(name, outcome, expected);
    }
}

/// A violation's path is a JSON Pointer from the validated value to the
/// member that is wrong: from the document for a document, from the record
/// for a record. A member name is escaped as RFC 6901 says (`~` as `~0`, `/`
/// as `~1`).
#[test]
fn a_violation_names_the_member_that_is_wrong() {
    use Edit::Set;
    use ViolationCode::*;

    let cases: Vec<(&str, Vec<Edit>, ViolationCode, &str)> = vec![
        (
            "a rect corner",
            vec![Set("/annotations/0/geometry/x1", json!(241))],
            OutOfBounds,
            "/annotations/0/geometry/x1",
        ),
        (
            "the second vertex of a polygon",
            vec![Set("/annotations/5/geometry/points/1/y", json!(9999))],
            OutOfBounds,
            "/annotations/5/geometry/points/1/y",
        ),
        (
            "the class of the fourth annotation",
            vec![Set("/annotations/3/class", json!("nope"))],
            UnknownClass,
            "/annotations/3/class",
        ),
        (
            "an attribute whose name needs escaping",
            vec![Set("/annotations/0/attributes", json!({ "a/b~c": true }))],
            AttributeNotAllowed,
            "/annotations/0/attributes/a~1b~0c",
        ),
        (
            "an attribute's value",
            vec![Set("/annotations/0/attributes/clip_shape", json!("nope"))],
            UnknownOption,
            "/annotations/0/attributes/clip_shape",
        ),
        (
            "the field of the third label",
            vec![Set("/labels/2/field", json!("nope"))],
            UnknownField,
            "/labels/2/field",
        ),
        (
            "a label's value",
            vec![Set("/labels/1/value", json!(101))],
            ValueOutOfRange,
            "/labels/1/value",
        ),
        (
            "the second option id of a value",
            vec![Set("/labels/6/value", json!(["calc", "nope"]))],
            UnknownOption,
            "/labels/6/value/1",
        ),
        (
            "a layer's color",
            vec![Set("/layers/1/color", json!("#12345"))],
            BadColor,
            "/layers/1/color",
        ),
        (
            "a class's color",
            vec![Set("/schema/classes/0/color", json!("red"))],
            BadColor,
            "/schema/classes/0/color",
        ),
        (
            "a file's orientation",
            vec![Set("/files/2/space/exif_orientation", json!(9))],
            BadFileRef,
            "/files/2/space/exif_orientation",
        ),
    ];
    for (name, edits, code, path) in cases {
        let invalid = validate(apply(document_value(), &edits)).expect_err(name);
        assert!(
            invalid
                .violations
                .iter()
                .any(|violation| violation.code == code && violation.path == path),
            "{name}: expected {code:?} at {path}, got {:?}",
            invalid.violations
        );
    }

    // The same record validated on its own is its own root.
    let fixture = super::support::fixture();
    let context = Context {
        files: &fixture.files,
        schema: &fixture.schema,
    };
    let mut annotation = document().annotations[0].clone();
    annotation.geometry = Geometry::Rect {
        x0: 30.0,
        y0: 20.0,
        x1: 241.0,
        y1: 60.0,
    };
    let invalid = annotation.validate(&context).expect_err("past the edge");
    assert_eq!(invalid.violations[0].path, "/geometry/x1");
    let invalid = annotation
        .geometry
        .validate(super::support::SIZE)
        .expect_err("past the edge");
    assert_eq!(invalid.violations[0].path, "/x1");
}

/// A session without a schema has one class, `roi`, that allows every
/// geometry type, and no fields.
#[test]
fn without_a_schema_everything_is_an_roi() {
    let mut document = document();
    document.schema = None;
    document.labels.clear();
    for annotation in &mut document.annotations {
        annotation.class = "roi".to_string();
        annotation.attributes.clear();
    }
    assert_outcome(
        "every geometry type as roi",
        document.validate(),
        Expect::Valid,
    );

    let implicit = LabelSchema::implicit();
    assert_outcome("the implicit schema", implicit.validate(), Expect::Valid);
    let files: BTreeMap<FileKey, ImageSize> = document
        .files
        .iter()
        .map(|file| (file.key.clone(), file.size()))
        .collect();
    let context = Context {
        files: &files,
        schema: &implicit,
    };
    let mut annotation = document.annotations[0].clone();
    assert_outcome("roi", annotation.validate(&context), Expect::Valid);
    annotation.class = "clip".to_string();
    assert_outcome(
        "a class the session does not have",
        annotation.validate(&context),
        Expect::Code(ViolationCode::UnknownClass),
    );

    let fixture = super::support::document();
    assert_outcome(
        "a label without any field to set",
        fixture.labels[0].validate(&context),
        Expect::Code(ViolationCode::UnknownField),
    );
}

/// A record the lenient EMBED import read stays as it was read: it is held,
/// written and read back unchanged, and only the strict check refuses it.
#[test]
fn a_record_that_breaks_the_invariants_is_still_held_and_written_unchanged() {
    let as_loaded = [
        json!({ "type": "rect", "x0": 0, "y0": 0, "x1": 241, "y1": 161 }),
        json!({ "type": "rect", "x0": 600, "y0": 500, "x1": 800, "y1": 700 }),
        json!({ "type": "rect", "x0": 50, "y0": 40, "x1": 90, "y1": 40 }),
        json!({ "type": "rect", "x0": 5, "y0": 5, "x1": 5, "y1": 5 }),
        json!({ "type": "rect", "x0": 90, "y0": 60, "x1": 30, "y1": 20 }),
    ];
    for geometry in as_loaded {
        let edited = apply(
            document_value(),
            &[Edit::Set("/annotations/0/geometry", geometry.clone())],
        );
        let document = Document::from_json_str(&edited.to_string()).expect("reads");
        assert_eq!(
            serde_json::to_value(&document).expect("serializes"),
            edited,
            "{geometry}"
        );
        assert!(document.validate().is_err(), "{geometry}");
        assert_eq!(
            serde_json::from_value::<Geometry>(geometry.clone()).expect("reads"),
            document.annotations[0].geometry
        );
    }
}

/// A validation stops once it has 32 violations, however many there are.
#[test]
fn a_validation_reports_at_most_thirty_two_violations() {
    let mut document = document();
    let template = document.annotations[0].clone();
    document.annotations = (0..500_u128)
        .map(|index| {
            let mut annotation = template.clone();
            annotation.id =
                uuid::Uuid::from_u128(0x0199c0de_0000_7000_8000_000000100000_u128 + index);
            annotation.geometry = Geometry::Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 9999.0,
                y1: 9999.0,
            };
            annotation
        })
        .collect();
    let invalid = document
        .validate()
        .expect_err("500 rectangles past the edge");
    assert_eq!(invalid.violations.len(), MAX_VIOLATIONS);
}
