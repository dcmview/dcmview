//! File keys, the other validated strings, and canonical label target ids.

use dcmview_annotation::{
    Author, AuthorKind, FileKey, KeyScheme, LabelTarget, LayerId, Timestamp, KEY_RULES,
};
use serde_json::json;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Key rules version 1: `sop:<uid>` or `b3:<64 lowercase hex>`, nothing else.
/// A change to either table is a change of `KEY_RULES`.
#[test]
fn a_file_key_has_exactly_two_forms() {
    assert_eq!(KEY_RULES, 1);

    let long_uid = "1".repeat(128);
    let valid: Vec<(String, KeyScheme, String)> = vec![
        (
            "sop:1.2.840.113681.2863050711.1286.3688.28".to_string(),
            KeyScheme::Sop,
            "1.2.840.113681.2863050711.1286.3688.28".to_string(),
        ),
        // Not a conforming UID, but what some de-identifiers write.
        (
            "sop:anon-3F2A_77.b".to_string(),
            KeyScheme::Sop,
            "anon-3F2A_77.b".to_string(),
        ),
        (format!("sop:{long_uid}"), KeyScheme::Sop, long_uid),
        (
            format!("b3:{DIGEST}"),
            KeyScheme::Blake3,
            DIGEST.to_string(),
        ),
    ];
    for (text, scheme, body) in &valid {
        let key = FileKey::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        assert_eq!(key.as_str(), text);
        assert_eq!(key.scheme(), *scheme, "{text}");
        assert_eq!(key.body(), body, "{text}");
        assert_eq!(serde_json::to_value(&key).expect("serializes"), json!(text));
        assert_eq!(
            serde_json::from_value::<FileKey>(json!(text)).expect("reads"),
            key
        );
    }

    let invalid: Vec<String> = vec![
        String::new(),
        "sop".to_string(),
        "sop:".to_string(),
        "1.2.840.113681".to_string(),
        "SOP:1.2.3".to_string(),
        "sop: 1.2.3".to_string(),
        "sop:1.2.3 ".to_string(),
        "sop:1.2.3\0".to_string(),
        "sop:1.2/3".to_string(),
        "sop:1.2#3".to_string(),
        "sop:1.2:3".to_string(),
        "sop:1.2.\u{e9}".to_string(),
        format!("sop:{}", "1".repeat(129)),
        "b3:".to_string(),
        format!("b3:{}", &DIGEST[..63]),
        format!("b3:{DIGEST}0"),
        format!("b3:{}", DIGEST.to_uppercase()),
        format!("b3:{}g", &DIGEST[..63]),
        format!("sha256:{DIGEST}"),
        format!("px:{DIGEST}"),
        "path:/data/a.dcm".to_string(),
        "missing:sop:1.2.3".to_string(),
    ];
    for text in &invalid {
        assert!(FileKey::parse(text).is_err(), "{text:?} must not parse");
        assert!(
            serde_json::from_value::<FileKey>(json!(text)).is_err(),
            "{text:?} must not read"
        );
    }

    assert_eq!(
        FileKey::sop("1.2.3").expect("valid uid").as_str(),
        "sop:1.2.3"
    );
    assert!(FileKey::sop("").is_err());
    assert!(FileKey::sop("1.2 3").is_err());

    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = (index as u8) * 8 + 1;
    }
    assert_eq!(
        FileKey::blake3(&digest).as_str(),
        "b3:0109111921293139414951596169717981899199a1a9b1b9c1c9d1d9e1e9f1f9"
    );
}

#[test]
fn layer_ids_authors_and_timestamps_each_have_one_syntax() {
    for text in ["L-01", "annotations", "a.b_c-9", &"x".repeat(64)] {
        assert_eq!(LayerId::parse(text).expect(text).as_str(), text);
    }
    for text in [
        "",
        "layer one",
        "study:1.2",
        "a/b",
        "l\u{e9}",
        &"x".repeat(65),
    ] {
        assert!(LayerId::parse(text).is_err(), "layer id {text:?}");
    }

    let authors = [
        ("user:alice", AuthorKind::User),
        ("user:jos\u{e9}.garc\u{ed}a", AuthorKind::User),
        ("model:detector@2.1", AuthorKind::Model),
        ("model:org/detector@v2@2026-01", AuthorKind::Model),
        ("import:embed", AuthorKind::Import),
    ];
    for (text, kind) in authors {
        let author = Author::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        assert_eq!(author.as_str(), text);
        assert_eq!(author.kind(), kind, "{text}");
    }
    for text in [
        "",
        "alice",
        "user:",
        "User:alice",
        "user:alice smith",
        "user:alice\n",
        "user:al\u{7f}ice",
        "model:detector",
        "model:@2.1",
        "model:detector@",
        "import:",
        "hub:admin",
        &format!("user:{}", "a".repeat(252)),
    ] {
        assert!(Author::parse(text).is_err(), "author {text:?}");
    }

    for text in ["2026-09-29T21:04:11.120Z", "1999-12-31T23:59:59.999Z"] {
        assert_eq!(Timestamp::parse(text).expect(text).as_str(), text);
    }
    for text in [
        "",
        "2026-09-29",
        "2026-09-29T21:04:11Z",
        "2026-09-29T21:04:11.12Z",
        "2026-09-29T21:04:11.1200Z",
        "2026-09-29T21:04:11.120",
        "2026-09-29T21:04:11.120+00:00",
        "2026-09-29t21:04:11.120z",
        "2026-09-29 21:04:11.120Z",
        "2026-13-01T00:00:00.000Z",
        "2026-00-10T00:00:00.000Z",
        "2026-01-32T00:00:00.000Z",
        "2026-01-00T00:00:00.000Z",
        "2026-01-01T24:00:00.000Z",
        "2026-01-01T00:60:00.000Z",
        "2026-01-01T00:00:60.000Z",
        "2026-01-01T00:00:00.00\u{662}Z",
        "+026-01-01T00:00:00.000Z",
    ] {
        assert!(Timestamp::parse(text).is_err(), "timestamp {text:?}");
    }
    assert!(
        Timestamp::parse("2026-09-29T21:04:11.120Z").unwrap()
            < Timestamp::parse("2026-09-29T21:04:11.121Z").unwrap()
    );
}

/// One string per target: what labels are indexed by and queued under.
#[test]
fn a_label_target_has_one_canonical_id() {
    let file = FileKey::parse("sop:1.2.3").unwrap();
    let raster = FileKey::parse(&format!("b3:{DIGEST}")).unwrap();
    let cases: Vec<(LabelTarget, String)> = vec![
        (
            LabelTarget::Patient {
                patient: "P 0001".to_string(),
            },
            "patient:P 0001".to_string(),
        ),
        (
            LabelTarget::Study {
                study: "1.2.840.1".to_string(),
            },
            "study:1.2.840.1".to_string(),
        ),
        (
            LabelTarget::Series {
                series: "1.2.840.1.5".to_string(),
            },
            "series:1.2.840.1.5".to_string(),
        ),
        // A file without a StudyInstanceUID is its own study.
        (
            LabelTarget::Study {
                study: LabelTarget::missing_id(&raster),
            },
            format!("study:missing:b3:{DIGEST}"),
        ),
        (
            LabelTarget::File { file: file.clone() },
            "sop:1.2.3".to_string(),
        ),
        (
            LabelTarget::Frame {
                frame: file.clone(),
                index: 12,
            },
            "sop:1.2.3#12".to_string(),
        ),
        (
            LabelTarget::Frame {
                frame: raster,
                index: 0,
            },
            format!("b3:{DIGEST}#0"),
        ),
        (
            LabelTarget::Folder {
                folder: "cohort_a/patient 1".to_string(),
                root: "0".to_string(),
            },
            "folder:0/cohort_a/patient 1".to_string(),
        ),
        // The same path under another root is another target.
        (
            LabelTarget::Folder {
                folder: "cohort_a/patient 1".to_string(),
                root: "1".to_string(),
            },
            "folder:1/cohort_a/patient 1".to_string(),
        ),
        (
            LabelTarget::Folder {
                folder: String::new(),
                root: "campaign-7".to_string(),
            },
            "folder:campaign-7/".to_string(),
        ),
    ];
    for (target, id) in cases {
        assert_eq!(target.canonical_id(), id);
        assert!(target.validate().is_ok(), "{id}");
    }
}
