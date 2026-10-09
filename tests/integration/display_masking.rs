//! A masked session (`--mask`) over the committed masking and presentation
//! state fixtures, loaded through the production discovery path.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
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

/// Every identifier the masking fixtures carry in their headers.
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
    "19300214",
    "19810903",
    "20260520",
    "20260811",
    "096Y",
    "2.25.20008",
];

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

fn assert_no_identifiers(what: &str, text: &str) {
    let text = without_hashed_uids(text);
    for identifier in IDENTIFIERS {
        assert!(
            !text.contains(identifier),
            "{what} shows {identifier}: {text}"
        );
    }
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

#[tokio::test]
async fn catalog_shows_pseudonyms_shifted_dates_and_hashed_uids() {
    let server = serve_masked(&[PATIENT_A_1, PATIENT_A_2, PATIENT_B]).await;

    let health: Value = server.get("/api/health").await.json();
    assert_eq!(health["masked"], true);

    let catalog: Value = server.get("/api/files").await.json();
    assert_eq!(catalog["masked"], true);
    assert_no_identifiers("catalog", &without_paths(catalog.clone()).to_string());
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
    assert_no_identifiers("tag tree", &text);
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
