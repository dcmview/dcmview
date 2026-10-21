//! Operations: what the strict check refuses, and what patches do.

use super::support::{
    apply, assert_outcome, document, fixture, operation, read_envelope, Edit, Expect, UUID_V4,
    UUID_V7_OTHER,
};
use dcmview_annotation::{Context, FrameScope, Op, ViolationCode};
use serde_json::json;

/// An envelope from `operations.json` with one thing changed. Each row names
/// the rule it breaks, or is a change that must stay valid.
#[test]
fn an_operation_that_breaks_one_rule_reports_it() {
    use Edit::Set;
    use Expect::{Code, Valid};
    use ViolationCode::*;

    let nested = operation("batch")["op"].clone();
    let outside = json!({ "type": "rect", "x0": 600, "y0": 500, "x1": 800, "y1": 700 });
    let tile = |tx: u32, after: &str| json!({ "tx": tx, "ty": 0, "before": null, "after": after });

    let cases: Vec<(&str, &str, Vec<Edit>, Expect)> = vec![
        (
            "operation id that is not version 7",
            "create_annotation",
            vec![Set("/op_id", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        // create_annotation
        (
            "created outside the image",
            "create_annotation",
            vec![Set("/op/annotation/geometry", outside.clone())],
            Code(OutOfBounds),
        ),
        (
            "created with an unknown class",
            "create_annotation",
            vec![Set("/op/annotation/class", json!("nope"))],
            Code(UnknownClass),
        ),
        (
            "created on an unknown file",
            "create_annotation",
            vec![Set("/op/annotation/file", json!("sop:9.9.9"))],
            Code(UnknownFile),
        ),
        (
            "created under an id that is not version 7",
            "create_annotation",
            vec![Set("/op/annotation/id", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        (
            "created in a layer the validation cannot know",
            "create_annotation",
            vec![Set("/op/annotation/layer", json!("L-unknown-here"))],
            Valid,
        ),
        // update_annotation
        (
            "update that changes nothing",
            "update_annotation",
            vec![Set("/op/before", json!({})), Set("/op/after", json!({}))],
            Code(EmptyPatch),
        ),
        (
            "update whose before sets another member",
            "update_annotation",
            vec![Set("/op/before", json!({ "class": "clip" }))],
            Code(PatchFieldsMismatch),
        ),
        (
            "update whose before sets one member more",
            "update_annotation",
            vec![Set("/op/before/class", json!("clip"))],
            Code(PatchFieldsMismatch),
        ),
        (
            "update to outside the image",
            "update_annotation",
            vec![Set("/op/after/geometry", outside.clone())],
            Code(OutOfBounds),
        ),
        (
            "update on an unknown file",
            "update_annotation",
            vec![Set("/op/file", json!("sop:9.9.9"))],
            Code(UnknownFile),
        ),
        (
            "update to a frame past the last",
            "update_annotation_every_member",
            vec![Set("/op/after/frames", json!({ "set": [3] }))],
            Code(FrameOutOfRange),
        ),
        (
            "update under an id that is not version 7",
            "update_annotation",
            vec![Set("/op/id", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        // The first edit of a record the lenient import kept as loaded: what
        // it was is not judged, only what it becomes.
        (
            "update from outside the image",
            "update_annotation",
            vec![Set("/op/before/geometry", outside.clone())],
            Valid,
        ),
        (
            "update from a frame list as written",
            "update_annotation_every_member",
            vec![Set(
                "/op/before/frames",
                json!({ "set": [0, 9], "as_written": [9, 0] }),
            )],
            Valid,
        ),
        // delete_annotation, restore_annotation
        (
            "delete whose snapshot is another record",
            "delete_annotation",
            vec![Set("/op/id", json!(UUID_V7_OTHER))],
            Code(SnapshotIdMismatch),
        ),
        (
            "restore whose snapshot is another record",
            "restore_annotation",
            vec![Set("/op/id", json!(UUID_V7_OTHER))],
            Code(SnapshotIdMismatch),
        ),
        (
            "delete of a record kept as loaded",
            "delete_annotation",
            vec![Set("/op/snapshot/geometry", outside.clone())],
            Valid,
        ),
        (
            "restore of a record kept as loaded",
            "restore_annotation",
            vec![Set("/op/snapshot/geometry", outside.clone())],
            Valid,
        ),
        // mask_tiles
        (
            "tiles on a frame past the last",
            "mask_tiles",
            vec![Set("/op/frame", json!(3))],
            Code(FrameOutOfRange),
        ),
        (
            "no tiles",
            "mask_tiles",
            vec![Set("/op/tiles", json!([]))],
            Code(EmptyPatch),
        ),
        (
            "one tile twice",
            "mask_tiles",
            vec![Set("/op/tiles", json!([tile(1, "AAAA"), tile(1, "AAAA")]))],
            Code(DuplicateId),
        ),
        (
            "tile right of the image",
            "mask_tiles",
            vec![Set("/op/tiles", json!([tile(4, "AAAA")]))],
            Code(MaskTileOutOfBounds),
        ),
        (
            "tile that is not base64",
            "mask_tiles",
            vec![Set("/op/tiles", json!([tile(1, "!!!!")]))],
            Code(MaskPayload),
        ),
        (
            "tile whose content before the stroke is not base64",
            "mask_tiles",
            vec![Set(
                "/op/tiles",
                json!([{ "tx": 1, "ty": 0, "before": "!!!!", "after": null }]),
            )],
            Code(MaskPayload),
        ),
        (
            "tiles on an unknown file",
            "mask_tiles",
            vec![Set("/op/file", json!("sop:9.9.9"))],
            Code(UnknownFile),
        ),
        // set_label
        (
            "label of an unknown field",
            "set_label_study",
            vec![Set("/op/field", json!("nope"))],
            Code(UnknownField),
        ),
        (
            "label on a target its field does not apply to",
            "set_label_study",
            vec![Set("/op/target", json!({ "patient": "P1" }))],
            Code(TargetNotAllowed),
        ),
        (
            "label value of the wrong type",
            "set_label_study",
            vec![Set("/op/after", json!("45"))],
            Code(ValueType),
        ),
        (
            "label value above its maximum",
            "set_label_study",
            vec![Set("/op/after", json!(101))],
            Code(ValueOutOfRange),
        ),
        (
            "label with an unknown option",
            "set_label_create",
            vec![Set("/op/after", json!("nope"))],
            Code(UnknownOption),
        ),
        (
            "label on an unknown file",
            "set_label_clear",
            vec![Set("/op/target/file", json!("sop:9.9.9"))],
            Code(UnknownFile),
        ),
        (
            "label on a frame past the last",
            "set_label_frame",
            vec![Set("/op/target/index", json!(3))],
            Code(FrameOutOfRange),
        ),
        (
            "label on a folder outside its root",
            "set_label_folder",
            vec![Set("/op/target/folder", json!("../other"))],
            Code(BadTarget),
        ),
        (
            "label under an id that is not version 7",
            "set_label_study",
            vec![Set("/op/id", json!(UUID_V4))],
            Code(IdNotUuidV7),
        ),
        (
            "label whose old value no longer fits its field",
            "set_label_study",
            vec![Set("/op/before", json!("thirty"))],
            Valid,
        ),
        // Layer operations.
        (
            "layer created without a name",
            "create_layer",
            vec![Set("/op/layer/name", json!(""))],
            Code(BadLayer),
        ),
        (
            "layer created with a bad color",
            "create_layer",
            vec![Set("/op/layer/color", json!("teal"))],
            Code(BadColor),
        ),
        (
            "review layer created writable",
            "create_layer",
            vec![Set("/op/layer/kind", json!("review"))],
            Code(BadLayer),
        ),
        (
            "layer update that changes nothing",
            "update_layer",
            vec![Set("/op/before", json!({})), Set("/op/after", json!({}))],
            Code(EmptyPatch),
        ),
        (
            "layer update whose before sets fewer members",
            "update_layer",
            vec![Set("/op/before", json!({ "name": "EMBED import" }))],
            Code(PatchFieldsMismatch),
        ),
        (
            "layer update to a bad color",
            "update_layer",
            vec![Set("/op/after/color", json!("#GGGGGG"))],
            Code(BadColor),
        ),
        (
            "layer update to an empty name",
            "update_layer",
            vec![Set("/op/after/name", json!(""))],
            Code(BadLayer),
        ),
        (
            "layer delete whose snapshot is another layer",
            "delete_layer",
            vec![Set("/op/id", json!("L-01"))],
            Code(SnapshotIdMismatch),
        ),
        // batch
        (
            "empty batch",
            "batch",
            vec![Set("/op/ops", json!([]))],
            Code(EmptyBatch),
        ),
        (
            "batch inside a batch",
            "batch",
            vec![Set("/op/ops/1", nested)],
            Code(NestedBatch),
        ),
        (
            "batch with one invalid operation",
            "batch",
            vec![Set("/op/ops/1/after/geometry", outside)],
            Code(OutOfBounds),
        ),
    ];

    let fixture = fixture();
    let context = Context {
        files: &fixture.files,
        schema: &fixture.schema,
    };
    for (name, operation_name, edits, expected) in cases {
        let envelope = read_envelope(&apply(operation(operation_name), &edits));
        assert_outcome(name, envelope.validate(&context), expected);
    }
}

/// `after` applied to the record gives the new state, and `before` applied
/// to that gives the record back: an update carries its own inverse.
#[test]
fn a_patch_sets_only_the_members_it_carries() {
    let document = document();

    for name in ["update_annotation", "update_annotation_every_member"] {
        let Op::UpdateAnnotation {
            id, before, after, ..
        } = read_envelope(&operation(name)).op
        else {
            panic!("{name} is an update");
        };
        let original = document
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .expect("the fixture document holds the updated record");

        let mut changed = original.clone();
        after.apply_to(&mut changed);
        assert_ne!(&changed, original, "{name}: after changes the record");
        assert_eq!(changed.id, original.id);
        assert_eq!(changed.file, original.file);
        assert_eq!(
            changed.meta, original.meta,
            "{name}: the store owns the metadata"
        );
        assert_eq!(changed.extensions, original.extensions);
        if let Some(geometry) = &after.geometry {
            assert_eq!(&changed.geometry, geometry);
        } else {
            assert_eq!(changed.geometry, original.geometry);
        }
        if after.class.is_none() {
            assert_eq!(changed.class, original.class);
        }

        let mut restored = changed.clone();
        before.apply_to(&mut restored);
        assert_eq!(&restored, original, "{name}: before restores the record");
    }

    // An edit of the frames replaces the scope, so the list kept as written
    // goes with it.
    let Op::UpdateAnnotation { id, after, .. } =
        read_envelope(&operation("update_annotation_every_member")).op
    else {
        panic!("an update");
    };
    let mut imported = document
        .annotations
        .iter()
        .find(|annotation| annotation.id == id)
        .expect("the imported record")
        .clone();
    assert_eq!(imported.frames.written(), Some([2, 0, 0].as_slice()));
    after.apply_to(&mut imported);
    assert_eq!(imported.frames, FrameScope::set(vec![1]));

    let Op::UpdateLayer {
        id, before, after, ..
    } = read_envelope(&operation("update_layer")).op
    else {
        panic!("update_layer is a layer update");
    };
    let original = document
        .layers
        .iter()
        .find(|layer| layer.id == id)
        .expect("the fixture document holds the updated layer");
    let mut changed = original.clone();
    after.apply_to(&mut changed);
    assert_eq!(changed.name, "Imported");
    assert_eq!(changed.color, None, "a null color clears it");
    assert!(changed.exclusive_masks && changed.readonly);
    assert_eq!(changed.rev, original.rev, "the store owns the revision");
    assert_eq!(changed.unknown, original.unknown);
    let mut restored = changed.clone();
    before.apply_to(&mut restored);
    assert_eq!(&restored, original);

    // A patch without a color leaves the color alone.
    let name_only: dcmview_annotation::LayerPatch =
        serde_json::from_value(json!({ "name": "Renamed" })).expect("reads");
    let mut renamed = original.clone();
    name_only.apply_to(&mut renamed);
    assert_eq!(renamed.color, original.color);
    assert_eq!(renamed.name, "Renamed");
}
