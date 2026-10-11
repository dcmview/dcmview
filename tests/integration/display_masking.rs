//! A masked session (`--mask`) over the committed masking and presentation
//! state fixtures, loaded through the production discovery path.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use chrono::{Duration, NaiveDate};
use dcmview::loader::DiscoverOptions;
use dcmview::masking::Masker;
use dcmview::server::{self, FileRegistry};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

const PATIENT_A_1: &str = "golden-masking-patient-a-us-1.dcm";
const PATIENT_A_2: &str = "golden-masking-patient-a-us-2.dcm";
const PATIENT_B: &str = "golden-masking-patient-b-us.dcm";
const SLIDE_LABEL: &str = "golden-masking-wsi-label.dcm";

/// Every identifier the masking fixtures carry in their headers, except
/// their dates, which are in `FIXTURE_DATES`.
const IDENTIFIERS: &[&str] = &[
    "Rivera",
    "Alma",
    "Solberg",
    "MRN-00",
    "Okafor",
    "Lindqvist",
    "Northfield",
    "Lakeside",
    "30301",
    "ACC-7731905",
    "NPI-4471",
    "US-ROOM-3",
    "SN-558213",
    "ward 5",
    "096Y",
    "2.25.20008",
];

/// The dates in the header of one masking fixture.
#[derive(Debug, PartialEq)]
struct FixtureDates {
    study: &'static str,
    birth: &'static str,
}

const PATIENT_A_DATES: FixtureDates = FixtureDates {
    study: "20260520",
    birth: "19300214",
};
const PATIENT_B_DATES: FixtureDates = FixtureDates {
    study: "20260811",
    birth: "19810903",
};
const FIXTURE_DATES: &[&FixtureDates] = &[&PATIENT_A_DATES, &PATIENT_B_DATES];

/// A masked session moves a patient's dates by at most this many days, and
/// never by none.
const MAX_DATE_SHIFT_DAYS: i64 = 365;

/// A masked server over `names`, which are served in that order.
async fn serve_masked(names: &[&str]) -> TestServer {
    let paths = names
        .iter()
        .map(|name| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name)
        })
        .collect::<Vec<_>>();
    let report = support::discover(
        &paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discover masking fixtures");
    assert_eq!(report.files.len(), names.len());
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    for path in &paths {
        let file = report
            .files
            .iter()
            .find(|file| &file.path == path)
            .expect("fixture discovered")
            .clone();
        registry.insert(file);
    }
    registry.mark_scan_complete();
    TestServer::new(server::router(support::app_state_with_registry(registry)))
}

fn date(digits: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(digits, "%Y%m%d").ok()
}

/// The first identifier `text` shows, if it shows any.
///
/// `about` is given for text that is about one fixture and so may show its
/// dates, moved: the fixture's real dates, and the study date the session
/// shows for it. The offset is random for each session and patient, so a
/// moved date can be the real date of another patient (the two study dates
/// are 83 days apart). That is no leak, and such a date is passed over; a
/// real date of the fixture itself never is, and neither is an offset of no
/// days. Text that is about no one fixture may show none of these dates.
fn identifier_shown(text: &str, about: Option<(&FixtureDates, &str)>) -> Option<String> {
    let text = without_hashed_uids(text);
    if let Some(identifier) = IDENTIFIERS.iter().find(|name| text.contains(**name)) {
        return Some(identifier.to_string());
    }
    let mut moved = Vec::new();
    if let Some((real, shown_study_date)) = about {
        let Some(shown) = date(shown_study_date) else {
            return Some(format!("{shown_study_date:?} for a study date"));
        };
        let days = (shown - date(real.study).expect("fixture study date")).num_days();
        if days == 0 || days.abs() > MAX_DATE_SHIFT_DAYS {
            return Some(format!("a study date moved by {days} days"));
        }
        let birth = date(real.birth).expect("fixture birth date") + Duration::days(days);
        moved = vec![
            shown_study_date.to_string(),
            birth.format("%Y%m%d").to_string(),
        ];
    }
    for fixture in FIXTURE_DATES {
        let own = about.is_some_and(|(real, _)| real == *fixture);
        for real in [fixture.study, fixture.birth] {
            let coincidence = !own && moved.iter().any(|date| date == real);
            if !coincidence && text.contains(real) {
                return Some(real.to_string());
            }
        }
    }
    None
}

/// Text that is about no one fixture: it shows no identifier and no date.
fn assert_no_identifiers(what: &str, text: &str) {
    if let Some(identifier) = identifier_shown(text, None) {
        panic!("{what} shows {identifier}: {text}");
    }
}

/// Text about the fixture whose dates are `real`, in a session that shows
/// `shown_study_date` for it.
fn assert_no_identifiers_of(what: &str, text: &str, real: &FixtureDates, shown_study_date: &str) {
    if let Some(identifier) = identifier_shown(text, Some((real, shown_study_date))) {
        panic!("{what} shows {identifier}: {text}");
    }
}

/// A masked catalog whose files are, in order, the fixtures of `dates`:
/// each entry against its own fixture, and the rest against them all.
fn assert_catalog_masked(catalog: &Value, dates: &[&FixtureDates]) {
    let files = catalog["files"].as_array().expect("files");
    assert_eq!(files.len(), dates.len());
    for (index, (entry, real)) in files.iter().zip(dates).enumerate() {
        assert_no_identifiers_of(
            &format!("catalog entry {index}"),
            &without_paths(entry.clone()).to_string(),
            real,
            entry["study_date"].as_str().expect("study date"),
        );
    }
    let mut rest = catalog.clone();
    rest["files"] = Value::Null;
    assert_no_identifiers("catalog", &rest.to_string());
}

/// `text` with each hashed UID replaced by a marker. A masked session
/// sends `2.25.` and a random 128-bit number in place of a UID, and about
/// one such number in three thousand happens to contain one of the short
/// numeric identifiers above. The fixtures' identifying UIDs begin
/// `2.25.20008` and have at most 20 digits after `2.25.`, so they are left
/// in and still fail the check.
fn without_hashed_uids(text: &str) -> String {
    const PREFIX: &str = "2.25.";
    const FIXTURE_UID_DIGITS: usize = 20;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(PREFIX) {
        let after = &rest[at + PREFIX.len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > FIXTURE_UID_DIGITS {
            out.push_str(&rest[..at]);
            out.push_str("<hashed uid>");
        } else {
            out.push_str(&rest[..at + PREFIX.len() + digits]);
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// The response with every `path` field removed: paths are the one
/// identifier a masked session still sends, for the directory tree.
fn without_paths(mut value: Value) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(strip),
            Value::Object(fields) => {
                fields.remove("path");
                fields.values_mut().for_each(strip);
            }
            _ => {}
        }
    }
    strip(&mut value);
    value
}

/// The date check without a session: what one patient's entry shows when its
/// dates moved onto the other patient's study date, and when they did not
/// move as they should have.
#[test]
fn a_date_moved_onto_another_fixtures_date_is_not_a_leak_and_a_real_one_is() {
    let shown =
        |text: &str, study_date: &str| identifier_shown(text, Some((&PATIENT_A_DATES, study_date)));
    let leak = |date: &str| Some(date.to_string());

    // Patient A's study date, 83 days later, is patient B's.
    let entry = r#"{"label":"MASKED-0001 US 20260811","study_date":"20260811"}"#;
    assert_eq!(shown(entry, "20260811"), None);
    assert_eq!(identifier_shown(entry, None), leak("20260811"));
    // In patient B's own entry the same text is the real date, unmoved.
    assert_eq!(
        identifier_shown(entry, Some((&PATIENT_B_DATES, "20260811"))),
        Some("a study date moved by 0 days".to_string())
    );

    // The coincidence excuses that one date and nothing else.
    for real in ["20260520", "19300214", "19810903"] {
        let entry = format!(r#"{{"study_date":"20260811","other":"{real}101742.25"}}"#);
        assert_eq!(shown(&entry, "20260811"), leak(real));
    }
    assert_eq!(
        shown(r#"{"study_date":"20260811","name":"Rivera"}"#, "20260811"),
        leak("Rivera")
    );
    // Without the coincidence, the other patient's date is a leak too.
    let entry = r#"{"study_date":"20260601","other":"20260811"}"#;
    assert_eq!(shown(entry, "20260601"), leak("20260811"));

    // A date must move, by a year at most, and be a date.
    let moved_by = |days: i64| Some(format!("a study date moved by {days} days"));
    assert_eq!(
        shown(r#"{"study_date":"20260520"}"#, "20260520"),
        moved_by(0)
    );
    assert_eq!(shown("{}", "20270521"), moved_by(366));
    assert_eq!(shown("{}", "20250519"), moved_by(-366));
    assert_eq!(shown("{}", "20270520"), None);
    assert_eq!(shown("{}", "20250520"), None);
    assert!(shown("{}", "[masked]").is_some());
}

#[tokio::test]
async fn catalog_shows_pseudonyms_shifted_dates_and_hashed_uids() {
    let server = serve_masked(&[PATIENT_A_1, PATIENT_A_2, PATIENT_B]).await;

    let health: Value = server.get("/api/health").await.json();
    assert_eq!(health["masked"], true);

    let catalog: Value = server.get("/api/files").await.json();
    assert_eq!(catalog["masked"], true);
    assert_catalog_masked(
        &catalog,
        &[&PATIENT_A_DATES, &PATIENT_A_DATES, &PATIENT_B_DATES],
    );
    let files = catalog["files"].as_array().expect("files");
    assert_eq!(files[0]["patient_name"], "Patient 0001");
    assert_eq!(files[0]["patient_id"], "MASKED-0001");
    assert_eq!(files[1]["patient_name"], "Patient 0001");
    assert_eq!(files[2]["patient_name"], "Patient 0002");
    assert_eq!(files[0]["display_name"], "File 1");
    assert_eq!(files[2]["display_name"], "File 3");
    assert!(files[0]["path"]
        .as_str()
        .expect("path")
        .ends_with(PATIENT_A_1));
    assert_eq!(files[0]["study_description"], "Abdominal ultrasound");
    assert_eq!(files[0]["series_description"], "Liver sweep");
    assert_eq!(files[0]["burned_in_annotation"], true);

    // One patient's files move by one offset; the series still groups them.
    assert_eq!(files[0]["study_date"], files[1]["study_date"]);
    assert_eq!(files[0]["study_date"].as_str().expect("date").len(), 8);
    assert_eq!(
        files[0]["series_instance_uid"],
        files[1]["series_instance_uid"]
    );
    assert_ne!(
        files[0]["series_instance_uid"],
        files[2]["series_instance_uid"]
    );
    assert_ne!(files[0]["sop_instance_uid"], files[1]["sop_instance_uid"]);

    let series: Value = server.get("/api/series").await.json();
    assert_no_identifiers("series catalog", &series.to_string());
    let series = series["series"].as_array().expect("series");
    assert_eq!(series.len(), 2);
    let first = series
        .iter()
        .find(|series| series["series_instance_uid"] == files[0]["series_instance_uid"])
        .expect("series of the masked catalog UID");
    assert_eq!(first["study_instance_uid"], files[0]["study_instance_uid"]);
    assert_eq!(
        first["stacks"][0]["frames"][0]["sop_instance_uid"],
        files[0]["sop_instance_uid"]
    );
}

#[tokio::test]
async fn tag_tree_masks_identifiers_and_keeps_what_the_rules_keep() {
    let server = serve_masked(&[PATIENT_A_1]).await;
    let catalog: Value = server.get("/api/files").await.json();
    let file = &catalog["files"][0];

    let tags: Value = server.get("/api/file/0/tags").await.json();
    let text = tags.to_string();
    assert_no_identifiers_of(
        "tag tree",
        &text,
        &PATIENT_A_DATES,
        file["study_date"].as_str().expect("study date"),
    );
    let value = |tag: &str| {
        tags.as_array()
            .expect("tag list")
            .iter()
            .find(|node| node["tag"] == tag)
            .unwrap_or_else(|| panic!("tag {tag} is listed"))["value"]["value"]
            .clone()
    };
    assert_eq!(value("(0010,0010)"), "Patient 0001");
    assert_eq!(value("(0010,0020)"), "MASKED-0001");
    assert_eq!(value("(0010,1010)"), "089Y");
    // Born 1930, studied 2026: a shifted birth date would still show the age.
    assert_eq!(value("(0010,0030)"), "");
    assert_eq!(value("(0010,0040)"), "F");
    assert_eq!(value("(0008,0020)"), file["study_date"]);
    assert_eq!(value("(0008,0030)"), "101500");
    assert_eq!(
        value("(0008,002A)"),
        format!(
            "{}101742.250000",
            file["study_date"].as_str().expect("study date")
        )
    );
    assert_eq!(value("(0008,0080)"), "[masked]");
    assert_eq!(value("(0008,0090)"), "[masked]");
    assert_eq!(value("(0010,1040)"), "[masked]");
    assert_eq!(value("(0008,1030)"), "Abdominal ultrasound");
    assert_eq!(value("(0008,103E)"), "Liver sweep");
    assert_eq!(value("(0008,0070)"), "dcmview fixtures");
    assert_eq!(value("(0009,0010)"), "DCMVIEW FIXTURE");
    assert_eq!(value("(0009,1001)"), "[masked]");
    assert_eq!(value("(0020,000D)"), file["study_instance_uid"]);
    assert_eq!(value("(0020,000E)"), file["series_instance_uid"]);
    assert_eq!(value("(0008,0018)"), file["sop_instance_uid"]);
    assert_eq!(value("(0008,0016)"), "1.2.840.10008.5.1.4.1.1.6.1");

    // The cached tree and a selected element show the same values.
    let again: Value = server.get("/api/file/0/tags").await.json();
    assert_eq!(again, tags);
    let selected: Value = server
        .get("/api/file/0/tags/select")
        .add_query_param("path", "(0010,0010)")
        .await
        .json();
    assert_eq!(selected["value"]["value"], "Patient 0001");
    let nested: Value = server
        .get("/api/file/0/tags/select")
        .add_query_param("path", "(0008,0096)")
        .await
        .json();
    assert_no_identifiers("selected sequence", &nested.to_string());
    let code: Value = server
        .get("/api/file/0/tags/select")
        .add_query_param("path", "(0008,0096)/0/(0040,1101)/0/(0008,0100)")
        .await
        .json();
    assert_eq!(code["value"]["value"], "[masked]");
}

#[tokio::test]
async fn slide_label_frames_are_withheld() {
    let server = serve_masked(&[SLIDE_LABEL, PATIENT_B]).await;

    let catalog: Value = server.get("/api/files").await.json();
    assert_eq!(catalog["files"][0]["support_state"], "unsupported");
    assert_eq!(catalog["files"][0]["support_reason"], "masked_label_image");
    assert_eq!(catalog["files"][1]["support_state"], "renderable");

    for path in [
        "/api/file/0/frame/0",
        "/api/file/0/frame/0/raw",
        "/api/file/0/frame/0/raw/pixel?row=0&column=0",
    ] {
        let response = server.get(path).await;
        response.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(response.json::<Value>()["code"], "masked", "{path}");
    }
    server.get("/api/file/1/frame/0").await.assert_status_ok();
    server
        .get("/api/file/1/frame/0/raw")
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn presentation_state_text_is_withheld_and_graphics_are_kept() {
    let server = serve_masked(&["golden-gsps-target-u8.dcm", "golden-gsps-conforming.dcm"]).await;

    let context: Value = server.get("/api/file/1/semantic-context").await.json();
    let state = &context["context"];
    assert_eq!(state["kind"], "presentation_state");
    let items = state["items"].as_array().expect("items");
    assert!(items
        .iter()
        .all(|item| item["texts"].as_array().expect("texts").is_empty()));
    assert_eq!(state["skipped"]["masked_text"], 3);

    let annotations: Value = server
        .get("/api/file/0/frame/0/graphic-annotations")
        .add_query_param("state", 1)
        .await
        .json();
    assert!(annotations["texts"].as_array().expect("texts").is_empty());
    assert_eq!(annotations["skipped"]["masked_text"], 3);
    assert!(!annotations["graphics"]
        .as_array()
        .expect("graphics")
        .is_empty());

    // The state's reference to its target carries the target's catalog UID.
    let catalog: Value = server.get("/api/files").await.json();
    let references: Value = server.get("/api/file/1/references").await.json();
    assert!(references.to_string().contains(
        catalog["files"][0]["sop_instance_uid"]
            .as_str()
            .expect("masked SOP Instance UID")
    ));
    assert_eq!(
        references["source_sop_instance_uid"],
        catalog["files"][1]["sop_instance_uid"]
    );
}

/// A file key built from a SOP Instance UID is an identifier: a masked
/// session sends it built from the masked UID, in the catalog, in the frame
/// header and in a key replacement, and accepts it back in that form only.
#[tokio::test]
async fn file_keys_carry_the_masked_uid_and_never_the_real_one() {
    let fixture = |name: &str| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    };
    // A second file with the first one's UID and other bytes, so that the
    // first file's key is replaced by a digest during the session.
    let dir = tempfile::tempdir().expect("temp dir");
    let twin = dir.path().join("twin.dcm");
    let mut bytes = std::fs::read(fixture(PATIENT_A_1)).expect("read fixture");
    bytes.extend_from_slice(&[0; 64]);
    std::fs::write(&twin, bytes).expect("write twin");

    let options = || DiscoverOptions {
        recursive: false,
        filters: Vec::new(),
        formats: Default::default(),
    };
    let registry = FileRegistry::masked(Arc::new(Masker::new()));
    for path in [fixture(PATIENT_A_1), fixture(PATIENT_B)] {
        let report = support::discover(&[path], options())
            .await
            .expect("discover fixture");
        registry.insert(report.files.into_iter().next().expect("one file"));
    }
    registry.mark_scan_complete();
    let server = TestServer::new(server::router(support::app_state_with_registry(
        registry.clone(),
    )));
    let real_uid = registry
        .get(0)
        .expect("first file")
        .sop_instance_uid
        .clone();
    assert!(real_uid.starts_with("2.25.20008"), "{real_uid}");

    // A UID key is implied by the entry's masked UID, so it is not sent
    // twice; the frame header spells it out, masked.
    let catalog: Value = server.get("/api/files").await.json();
    let mut shown_keys = Vec::new();
    for (index, entry) in catalog["files"]
        .as_array()
        .expect("files")
        .iter()
        .enumerate()
    {
        assert!(entry.get("file_key").is_none(), "{entry}");
        let masked_uid = entry["sop_instance_uid"].as_str().expect("masked UID");
        let frame = server.get(&format!("/api/file/{index}/frame/0")).await;
        frame.assert_status_ok();
        let header = frame
            .header("x-file-key")
            .to_str()
            .expect("key header")
            .to_string();
        assert_eq!(header, format!("sop:{masked_uid}"));
        assert_no_identifiers("frame key header", &header);
        assert_eq!(registry.file_for_shown_key(&header), Some(index));
        shown_keys.push(header);
    }
    // The real UID is not a key this session ever sent, so it names nothing.
    assert_eq!(
        registry.file_for_shown_key(&format!("sop:{real_uid}")),
        None
    );
    assert_eq!(
        registry
            .key_status(0)
            .expect("first file")
            .key
            .expect("key")
            .as_str(),
        format!("sop:{real_uid}"),
        "the server keeps the real key"
    );

    // The twin arrives; both files end with content keys.
    let report = support::discover(&[twin], options())
        .await
        .expect("discover twin");
    registry.insert(report.files.into_iter().next().expect("one file"));
    for index in [0, 2] {
        registry.ensure_key(index).await.expect("content key");
    }
    let catalog: Value = server.get("/api/files").await.json();
    assert_catalog_masked(
        &catalog,
        &[&PATIENT_A_DATES, &PATIENT_B_DATES, &PATIENT_A_DATES],
    );
    let rekeys = catalog["rekeys"].as_array().expect("rekeys");
    assert_eq!(rekeys.len(), 1, "{rekeys:?}");
    assert_eq!(rekeys[0]["old_key"], shown_keys[0].as_str());
    let new_key = rekeys[0]["new_key"].as_str().expect("new key");
    // A content key holds no header value and is sent as it is.
    assert_eq!(
        new_key,
        format!(
            "b3:{}",
            blake3::hash(&std::fs::read(fixture(PATIENT_A_1)).expect("read fixture")).to_hex()
        )
    );
    assert_eq!(catalog["files"][0]["file_key"], new_key);
    assert!(catalog["files"][2]["file_key"]
        .as_str()
        .expect("twin key")
        .starts_with("b3:"));
    // The replaced key is still accepted in the form it was sent in.
    assert_eq!(registry.file_for_shown_key(&shown_keys[0]), Some(0));
    assert_eq!(
        registry.file_for_shown_key(&format!("sop:{real_uid}")),
        None
    );
    let frame = server.get("/api/file/0/frame/0").await;
    assert_eq!(frame.header("x-file-key"), new_key);
}
