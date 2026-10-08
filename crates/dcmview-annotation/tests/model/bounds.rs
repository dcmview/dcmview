//! Input past the fixed bounds, and input built to hurt a parser.
//!
//! Model data comes from other processes and from files a user picked. Every
//! case here must come back as an error: no panic, no stack overflow, and no
//! work in proportion to the oversized part.

use super::support::{
    apply, assert_outcome, document, document_value, fixture, operation, Edit, Expect, SIZE,
};
use dcmview_annotation::limits::{
    MAX_BATCH_OPS, MAX_DOCUMENT_BYTES, MAX_ENVELOPE_BYTES, MAX_FRAMES_IN_SET, MAX_MASK_TILES,
    MAX_POINTS, MAX_TEXT_BYTES, MAX_TILE_PAYLOAD_CHARS, MAX_VALUES,
};
use dcmview_annotation::{
    Author, ClassDef, Context, Document, DocumentError, EnvelopeError, FieldDef, FieldType,
    FileKey, FrameScope, Geometry, GeometryType, LabelTarget, LabelValue, LayerId, OpEnvelope,
    OptionDef, Point, TargetKind, Timestamp, ViolationCode,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The bounds are a contract: a consumer relies on a validated value staying
/// inside them, so a change here is a change for every consumer.
#[test]
fn the_bounds_are_fixed_numbers() {
    assert_eq!(MAX_DOCUMENT_BYTES, 268_435_456);
    assert_eq!(MAX_ENVELOPE_BYTES, 16_777_216);
    assert_eq!(MAX_BATCH_OPS, 10_000);
    assert_eq!(MAX_POINTS, 100_000);
    assert_eq!(MAX_FRAMES_IN_SET, 65_536);
    assert_eq!(MAX_MASK_TILES, 262_144);
    assert_eq!(MAX_TILE_PAYLOAD_CHARS, 8_192);
    assert_eq!(MAX_TEXT_BYTES, 65_536);
    assert_eq!(MAX_VALUES, 1_024);
}

#[test]
fn text_past_the_size_bound_is_refused_before_it_is_parsed() {
    // Valid JSON, padded with trailing whitespace to one byte past the bound.
    let pad = |text: String, bound: usize| {
        let mut padded = text;
        padded.push_str(&" ".repeat(bound + 1 - padded.len()));
        padded
    };

    let envelope = pad(
        operation("create_annotation").to_string(),
        MAX_ENVELOPE_BYTES,
    );
    assert_eq!(
        OpEnvelope::from_json_str(&envelope),
        Err(EnvelopeError::TooLarge {
            bytes: MAX_ENVELOPE_BYTES + 1,
            limit: MAX_ENVELOPE_BYTES,
        })
    );
    assert!(OpEnvelope::from_json_str(&envelope[..MAX_ENVELOPE_BYTES]).is_ok());

    let document = pad(document_value().to_string(), MAX_DOCUMENT_BYTES);
    assert_eq!(
        Document::from_json_str(&document),
        Err(DocumentError::TooLarge {
            bytes: MAX_DOCUMENT_BYTES + 1,
            limit: MAX_DOCUMENT_BYTES,
        })
    );
}

#[test]
fn hostile_text_is_an_error_and_never_a_panic() {
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let deep_objects = format!("{}1{}", "{\"a\":".repeat(100_000), "}".repeat(100_000));
    let valid_document = document_value().to_string();
    let valid_envelope = operation("batch").to_string();

    let mut documents: Vec<String> = vec![
        String::new(),
        "null".to_string(),
        "[]".to_string(),
        "{}".to_string(),
        "\u{feff}{}".to_string(),
        deep.clone(),
        deep_objects.clone(),
        valid_document.replacen(
            "\"extensions\":{}",
            &format!("\"extensions\":{{\"x\":{deep}}}"),
            1,
        ),
        valid_document.replacen(
            "\"format\":",
            "\"format\":\"dcmview.annotations\",\"format\":",
            1,
        ),
        valid_document.replace("\"rows\":160", "\"rows\":1e400"),
        valid_document.replace("\"rows\":160", "\"rows\":-1"),
        valid_document.replace("\"rows\":160", "\"rows\":4294967296"),
        valid_document.replace("\"x0\":30", "\"x0\":1e999"),
        valid_document.replace("\"x0\":30", "\"x0\":NaN"),
        format!("{valid_document}{valid_document}"),
        format!("{valid_document}\u{0}"),
    ];
    // Every prefix that ends inside the first annotation.
    let cut = valid_document
        .find("\"annotations\"")
        .expect("annotations member");
    documents.extend((cut..cut + 300).map(|end| valid_document[..end].to_string()));
    for text in &documents {
        assert!(
            Document::from_json_str(text).is_err(),
            "document: {text:.60}"
        );
    }

    let mut envelopes: Vec<String> = vec![
        String::new(),
        "null".to_string(),
        deep,
        deep_objects,
        valid_envelope.replace("\"type\":\"batch\"", "\"type\":\"undo\""),
        valid_envelope.replace("\"base_rev\":4", "\"base_rev\":-4"),
        valid_envelope.replace("\"base_rev\":4", "\"base_rev\":4.5"),
        valid_envelope.replace("\"base_rev\":4", "\"base_rev\":18446744073709551616"),
    ];
    // A batch nested 10,000 deep, written as text so the test itself never
    // recurses.
    envelopes.push(valid_envelope.replacen(
        "\"op\":{",
        &format!(
            "\"op\":{}{}{},\"was\":{{",
            "{\"type\":\"batch\",\"ops\":[".repeat(10_000),
            "{\"type\":\"batch\",\"ops\":[]}",
            "]}".repeat(10_000)
        ),
        1,
    ));
    for text in &envelopes {
        assert!(
            OpEnvelope::from_json_str(text).is_err(),
            "envelope: {text:.60}"
        );
    }

    // The validated strings refuse megabytes of anything without reading on.
    let huge = "9".repeat(4 * 1024 * 1024);
    assert!(FileKey::parse(&format!("sop:{huge}")).is_err());
    assert!(FileKey::parse(&format!("b3:{huge}")).is_err());
    assert!(FileKey::sop(&huge).is_err());
    assert!(LayerId::parse(&huge).is_err());
    assert!(Author::parse(&format!("user:{huge}")).is_err());
    assert!(Timestamp::parse(&huge).is_err());
}

/// Lists and strings one past their bound are refused by the strict check,
/// with the code that names the bound.
#[test]
fn values_past_their_bound_are_refused_by_the_strict_check() {
    use Expect::{Code, Valid};
    use ViolationCode::*;

    let ring = |count: usize| Geometry::Polygon {
        points: (0..count)
            .map(|index| Point {
                x: (index % 240) as f64,
                y: (index % 160) as f64,
            })
            .collect(),
    };
    assert_outcome(
        "polygon at the bound",
        ring(MAX_POINTS).validate(SIZE),
        Valid,
    );
    assert_outcome(
        "polygon past the bound",
        ring(MAX_POINTS + 1).validate(SIZE),
        Code(TooManyPoints),
    );
    assert_outcome(
        "polyline far past the bound, none of it inside the image",
        Geometry::Polyline {
            points: vec![Point { x: -1.0, y: -1.0 }; 4_000_000],
        }
        .validate(SIZE),
        Code(TooManyPoints),
    );

    let frames = |count: u32| FrameScope::set((0..count).collect());
    assert_outcome(
        "frame set at the bound",
        frames(MAX_FRAMES_IN_SET as u32).validate(u32::MAX),
        Valid,
    );
    assert_outcome(
        "frame set past the bound",
        frames(MAX_FRAMES_IN_SET as u32 + 1).validate(u32::MAX),
        Code(TooManyFrames),
    );
    let mut written = FrameScope::from_written(vec![0; MAX_FRAMES_IN_SET + 1]);
    assert_outcome(
        "list as written past the bound",
        written.validate(u32::MAX),
        Code(TooManyFrames),
    );
    written = FrameScope::from_written(vec![0; MAX_FRAMES_IN_SET]);
    assert_outcome(
        "list as written at the bound",
        written.validate(u32::MAX),
        Valid,
    );

    let mask = |payload: String| -> Geometry {
        serde_json::from_value(json!({
            "type": "mask", "encoding": "tiles-v1", "tile": 64, "depth": 8,
            "frames": { "0": { "0,0": payload } }
        }))
        .expect("mask reads")
    };
    assert_outcome(
        "tile payload at the bound",
        mask("A".repeat(MAX_TILE_PAYLOAD_CHARS)).validate(SIZE),
        Valid,
    );
    assert_outcome(
        "tile payload past the bound",
        mask("A".repeat(MAX_TILE_PAYLOAD_CHARS + 4)).validate(SIZE),
        Code(MaskPayload),
    );

    // A mask with one tile more than the bound, on an image large enough to
    // hold them: 513 tile columns by 512 tile rows.
    let wide = dcmview_annotation::ImageSize {
        columns: 513 * 64,
        rows: 512 * 64,
        frames: 1,
    };
    let tiles: serde_json::Map<String, Value> = (0..=MAX_MASK_TILES)
        .map(|index| (format!("{},{}", index % 513, index / 513), json!("AAAA")))
        .collect();
    let many: Geometry = serde_json::from_value(json!({
        "type": "mask", "encoding": "tiles-v1", "tile": 64, "depth": 1, "frames": { "0": tiles }
    }))
    .expect("mask reads");
    assert_outcome(
        "mask past the tile bound",
        many.validate(wide),
        Code(TooManyTiles),
    );

    let fixture = fixture();
    let context = Context {
        files: &fixture.files,
        schema: &fixture.schema,
    };
    let envelope = |name: &str, edits: &[Edit]| {
        OpEnvelope::from_json_str(&apply(operation(name), edits).to_string())
            .expect("envelope reads")
    };
    let inner = operation("set_label_study")["op"].clone();
    let batch = |count: usize| {
        envelope(
            "batch",
            &[Edit::Set(
                "/op/ops",
                Value::Array(vec![inner.clone(); count]),
            )],
        )
    };
    assert_outcome(
        "batch at the bound",
        batch(MAX_BATCH_OPS).validate(&context),
        Valid,
    );
    assert_outcome(
        "batch past the bound",
        batch(MAX_BATCH_OPS + 1).validate(&context),
        Code(TooManyOps),
    );

    // The note field allows 200 bytes; a field without max_length allows
    // 65,536 and no schema can raise that.
    let mut unbounded = super::support::fixture();
    let note = unbounded
        .schema
        .fields
        .iter_mut()
        .find(|field| field.id == "note")
        .expect("the fixture schema has a note field");
    note.field_type = dcmview_annotation::FieldType::Text {
        max_length: Some(u32::MAX),
    };
    let context = Context {
        files: &unbounded.files,
        schema: &unbounded.schema,
    };
    let note = |bytes: usize| {
        envelope(
            "set_label_clear",
            &[Edit::Set("/op/after", json!("x".repeat(bytes)))],
        )
    };
    assert_outcome(
        "text at the bound",
        note(MAX_TEXT_BYTES).validate(&context),
        Valid,
    );
    assert_outcome(
        "text past the bound",
        note(MAX_TEXT_BYTES + 1).validate(&context),
        Code(TooLong),
    );

    let findings = |count: usize| {
        let ids: Vec<String> = (0..count).map(|index| format!("option-{index}")).collect();
        envelope("set_label_multi", &[Edit::Set("/op/after", json!(ids))])
    };
    assert_outcome(
        "more option ids than a value may hold",
        findings(MAX_VALUES + 1).validate(&context),
        Code(TooManyItems),
    );
}

/// A document for the cost test: `annotations` annotations of one class that
/// carries `fields` attributes, each annotation holding the last `each` of
/// them. A scan of the class's list from the front passes the others first.
fn document_with_attributes(fields: usize, annotations: usize, each: usize) -> Document {
    let ids: Vec<String> = (0..fields).map(|index| format!("f{index:04}")).collect();
    let mut document = document();
    let template = document.annotations[2].clone();
    let schema = document.schema.as_mut().expect("fixture has a schema");
    schema.fields = ids
        .iter()
        .map(|id| FieldDef {
            id: id.clone(),
            name: "Flag".to_string(),
            field_type: FieldType::Boolean,
            applies_to: Vec::new(),
            required: false,
            unknown: BTreeMap::new(),
        })
        .collect();
    schema.classes = vec![ClassDef {
        id: "wide".to_string(),
        name: "Wide".to_string(),
        color: None,
        geometry: vec![GeometryType::Point],
        attributes: ids.clone(),
        code: None,
        deprecated: false,
        unknown: BTreeMap::new(),
    }];
    document.labels.clear();
    document.annotations = (0..annotations)
        .map(|index| {
            let mut annotation = template.clone();
            annotation.id = cost_test_id(index);
            annotation.class = "wide".to_string();
            annotation.attributes = ids[fields - each..]
                .iter()
                .map(|id| (id.clone(), LabelValue::Bool(true)))
                .collect();
            annotation
        })
        .collect();
    document
}

/// A document for the cost test: `labels` labels of one multi-category field
/// with `options` options, each label naming the last `each` of them.
fn document_with_options(options: usize, labels: usize, each: usize) -> Document {
    let ids: Vec<String> = (0..options).map(|index| format!("o{index:04}")).collect();
    let mut document = document();
    let template = document.labels[0].clone();
    let schema = document.schema.as_mut().expect("fixture has a schema");
    schema.classes.clear();
    schema.fields = vec![FieldDef {
        id: "findings".to_string(),
        name: "Findings".to_string(),
        field_type: FieldType::MultiCategory {
            options: ids
                .iter()
                .map(|id| OptionDef {
                    id: id.clone(),
                    name: "Finding".to_string(),
                    code: None,
                    deprecated: false,
                    unknown: BTreeMap::new(),
                })
                .collect(),
        },
        applies_to: vec![TargetKind::Patient],
        required: false,
        unknown: BTreeMap::new(),
    }];
    document.annotations.clear();
    document.labels = (0..labels)
        .map(|index| {
            let mut label = template.clone();
            label.id = cost_test_id(index);
            label.target = LabelTarget::Patient {
                patient: format!("P{index}"),
            };
            label.field = "findings".to_string();
            label.value = LabelValue::Many(ids[options - each..].to_vec());
            label
        })
        .collect();
    document
}

fn cost_test_id(index: usize) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x0199c0de_0000_7000_8000_000000200000_u128 + index as u128)
}

/// `Document::validate` promises work linear in the size of the document. A
/// schema may hold 4,096 fields of 4,096 options each, and a record may name
/// 1,024 of them. Finding each name by scanning the schema list makes the
/// work the product of the two: seconds for a document of a few megabytes.
///
/// Nothing in the public API counts lookups (the ids are plain strings), so
/// this compares two times, which is the one measure left. It is not a time
/// budget: each wide document is compared with a narrow one, timed just
/// before it in the same process, that holds the same number of names in
/// more records against a schema of 32 items, where a scan costs nothing.
/// With an index the two take about the same time; with a scan per name the
/// wide one took 55 and 90 times as long (debug build, when this was
/// written). The factor allowed, 10, is far from both, so neither the speed
/// of the machine nor a busy one decides the outcome.
#[test]
fn validation_cost_does_not_multiply_records_by_schema_size() {
    const FACTOR: u32 = 10;
    type Build = fn(usize, usize, usize) -> Document;
    // (name, builder, records and names per record: wide, then narrow)
    let cases: [(&str, Build, usize, usize); 2] = [
        (
            "attributes of a class",
            document_with_attributes,
            200,
            6_400,
        ),
        ("options of a field", document_with_options, 400, 12_800),
    ];
    for (name, build, wide_records, narrow_records) in cases {
        let timed = |document: Document| {
            let start = Instant::now();
            let outcome = document.validate();
            let taken = start.elapsed();
            // Valid, so the whole document was walked: nothing ended early.
            assert_outcome(name, outcome, Expect::Valid);
            taken
        };
        let narrow: Duration = timed(build(32, narrow_records, 32));
        let wide = timed(build(4_096, wide_records, 1_024));
        assert!(
            wide < narrow * FACTOR,
            "{name}: {wide:?} for the wide schema, {narrow:?} for the same names \
             against a narrow one; allowed {FACTOR} times"
        );
    }
}
