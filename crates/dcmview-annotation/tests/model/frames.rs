//! Frame scopes.

use super::support::{assert_outcome, Expect};
use dcmview_annotation::{FrameScope, FrameSet, ViolationCode};
use serde_json::{json, Value};

/// Owner decision (EMBED parity amendment, 2026-10-05): frame lists are kept
/// as written, and an unedited explicit list such as `[[0]]` stays explicit.
/// The set names the frames; the list as written survives beside it and
/// through the wire.
#[test]
fn a_frame_list_is_kept_as_written() {
    // (list as read, set, kept as written, wire)
    type Case = (Vec<u32>, Vec<u32>, Option<Vec<u32>>, Value);
    let cases: Vec<Case> = vec![
        (
            vec![2, 0, 0],
            vec![0, 2],
            Some(vec![2, 0, 0]),
            json!({ "set": [0, 2], "as_written": [2, 0, 0] }),
        ),
        (
            vec![2, 1, 0],
            vec![0, 1, 2],
            Some(vec![2, 1, 0]),
            json!({ "set": [0, 1, 2], "as_written": [2, 1, 0] }),
        ),
        (
            vec![1, 1],
            vec![1],
            Some(vec![1, 1]),
            json!({ "set": [1], "as_written": [1, 1] }),
        ),
        // Already sorted and distinct: nothing more to keep.
        (
            vec![0, 1, 2],
            vec![0, 1, 2],
            None,
            json!({ "set": [0, 1, 2] }),
        ),
        // A full explicit list on a single-frame file is not "all".
        (vec![0], vec![0], None, json!({ "set": [0] })),
    ];
    for (written, set, as_written, wire) in cases {
        let scope = FrameScope::from_written(written.clone());
        assert_eq!(
            scope,
            FrameScope::Set(FrameSet {
                set: set.clone(),
                as_written
            }),
            "{written:?}"
        );
        assert_eq!(
            scope.written(),
            Some(written.as_slice()),
            "{written:?}: written back"
        );
        assert_eq!(
            serde_json::to_value(&scope).expect("serializes"),
            wire,
            "{written:?}: wire"
        );
        let read: FrameScope = serde_json::from_value(wire).expect("reads");
        assert_eq!(
            read.written(),
            Some(written.as_slice()),
            "{written:?}: after a round trip"
        );
        for frame in 0..4 {
            assert_eq!(
                scope.contains(frame),
                set.contains(&frame),
                "{written:?} contains {frame}"
            );
        }
    }

    assert_eq!(
        serde_json::to_value(FrameScope::All).expect("serializes"),
        json!("all")
    );
    assert_eq!(
        serde_json::from_value::<FrameScope>(json!("all")).expect("reads"),
        FrameScope::All
    );
    assert_eq!(FrameScope::All.written(), None);
    assert!(FrameScope::All.contains(0) && FrameScope::All.contains(u32::MAX));
    // A set built from frames rather than read from a list keeps nothing.
    assert_eq!(
        FrameScope::set(vec![2, 0, 0]).written(),
        Some([0, 2].as_slice())
    );
}

/// The strict check, for a file of three frames.
#[test]
fn a_frame_scope_is_checked_against_the_frame_count() {
    use Expect::{Code, Valid};
    use ViolationCode::*;

    let cases: Vec<(&str, Value, u32, Expect)> = vec![
        ("all", json!("all"), 3, Valid),
        ("all on a file without frames", json!("all"), 0, Valid),
        ("a set", json!({ "set": [0, 2] }), 3, Valid),
        (
            "every frame, explicitly",
            json!({ "set": [0, 1, 2] }),
            3,
            Valid,
        ),
        ("an empty set", json!({ "set": [] }), 3, Code(FramesEmpty)),
        (
            "a set out of order",
            json!({ "set": [2, 0] }),
            3,
            Code(FramesNotNormalized),
        ),
        (
            "a set with a repeat",
            json!({ "set": [0, 0, 2] }),
            3,
            Code(FramesNotNormalized),
        ),
        (
            "a frame equal to the count",
            json!({ "set": [0, 3] }),
            3,
            Code(FrameOutOfRange),
        ),
        (
            "any frame of a file without frames",
            json!({ "set": [0] }),
            0,
            Code(FrameOutOfRange),
        ),
        (
            "a list as written",
            json!({ "set": [0, 2], "as_written": [2, 0, 0] }),
            3,
            Valid,
        ),
        (
            "a list as written equal to the set",
            json!({ "set": [0, 2], "as_written": [0, 2] }),
            3,
            Valid,
        ),
        (
            "a list as written that drops a frame",
            json!({ "set": [0, 2], "as_written": [2, 2] }),
            3,
            Code(FramesAsWrittenMismatch),
        ),
        (
            "a list as written that adds a frame",
            json!({ "set": [0, 2], "as_written": [2, 1, 0] }),
            3,
            Code(FramesAsWrittenMismatch),
        ),
        (
            "an empty list as written",
            json!({ "set": [0, 2], "as_written": [] }),
            3,
            Code(FramesAsWrittenMismatch),
        ),
    ];
    for (name, value, frame_count, expected) in cases {
        let scope: FrameScope = serde_json::from_value(value).expect("frame scope reads");
        assert_outcome(name, scope.validate(frame_count), expected);
    }
}
