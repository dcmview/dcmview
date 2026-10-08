//! Input past the fixed bounds, and input built to hurt a parser.
//!
//! Model data comes from other processes and from files a user picked. Every
//! case here must come back as an error: no panic, no stack overflow, and no
//! work in proportion to the oversized part.

use super::support::{
    apply, assert_outcome, document_value, fixture, operation, Edit, Expect, SIZE,
};
use dcmview_annotation::limits::{
    MAX_BATCH_OPS, MAX_DOCUMENT_BYTES, MAX_ENVELOPE_BYTES, MAX_FRAMES_IN_SET, MAX_MASK_TILES,
    MAX_POINTS, MAX_TEXT_BYTES, MAX_TILE_PAYLOAD_CHARS, MAX_VALUES,
};
use dcmview_annotation::{
    Author, Context, Document, DocumentError, EnvelopeError, FileKey, FrameScope, Geometry,
    LayerId, OpEnvelope, Point, Timestamp, ViolationCode,
};
use serde_json::{json, Value};

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
