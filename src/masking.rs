//! Display masking: replaces patient identifiers in what the viewer shows for
//! one session (`--mask`). It is a screen-sharing aid, not de-identification:
//! files are never modified, nothing is persisted, and burned-in pixel text,
//! free-text values and file paths are outside its reach.
//!
//! Every replacement is derived from keys drawn at process start, so values
//! are stable within a session and unrelated across sessions.

mod profile;
mod raster;

use crate::api::contracts::{
    FileSummary, GraphicAnnotationsResponse, SemanticContext, SemanticContextResponse,
    SupportState, TagNode, TagValue,
};
use crate::types::FileEntry;
use chrono::{Days, Local, NaiveDate};
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::Mutex;

/// Shown in place of a masked value.
pub const MASKED: &str = "[masked]";

/// PS3.15 E.3.9 treats ages above this as identifying.
const MAX_AGE_YEARS: u32 = 89;
/// Dates move by up to this many days in either direction, never by zero.
const MAX_DATE_SHIFT_DAYS: u64 = 365;

/// UIDs the standard registers (SOP classes, transfer syntaxes, coding
/// schemes, ...) identify no patient or study and stay readable.
const DICOM_UID_ROOT: &str = "1.2.840.10008.";

/// `support_reason` of a file whose frames a masked session withholds.
pub const LABEL_IMAGE_REASON: &str = "masked_label_image";

const PATIENT_ID: (u16, u16) = (0x0010, 0x0020);
const PATIENT_NAME: (u16, u16) = (0x0010, 0x0010);
const PATIENT_BIRTH_DATE: (u16, u16) = (0x0010, 0x0030);

pub struct Masker {
    keys: RandomState,
    /// Patient keys in the order first seen; the position is the pseudonym.
    patients: Mutex<HashMap<String, usize>>,
}

/// The replacements for one file's patient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatientMask {
    number: usize,
    shift_days: i64,
    /// The unshifted study date, else today: the date a birth date is
    /// measured against.
    reference_date: NaiveDate,
}

impl PatientMask {
    pub fn name(&self) -> String {
        format!("Patient {:04}", self.number)
    }

    pub fn id(&self) -> String {
        format!("MASKED-{:04}", self.number)
    }

    /// A DA or DT value moved by the patient's offset, keeping any time and
    /// zone suffix; a value without a full calendar date is masked.
    pub fn shift_date(&self, value: &str) -> String {
        let value = value.trim();
        if value.is_empty() {
            return String::new();
        }
        let Some(date) = value
            .get(..8)
            .and_then(|digits| NaiveDate::parse_from_str(digits, "%Y%m%d").ok())
        else {
            return MASKED.to_string();
        };
        match shifted(date, self.shift_days) {
            Some(date) => format!("{}{}", date.format("%Y%m%d"), &value[8..]),
            None => MASKED.to_string(),
        }
    }

    /// A birth date moved like any date, or blank when it implies an age
    /// above 89, which a shifted date would still reveal.
    fn birth_date(&self, value: &str) -> String {
        let born = value
            .trim()
            .get(..8)
            .and_then(|digits| NaiveDate::parse_from_str(digits, "%Y%m%d").ok());
        match born {
            Some(born) if self.reference_date.years_since(born) > Some(MAX_AGE_YEARS) => {
                String::new()
            }
            _ => self.shift_date(value),
        }
    }
}

fn shifted(date: NaiveDate, days: i64) -> Option<NaiveDate> {
    if days >= 0 {
        date.checked_add_days(Days::new(days as u64))
    } else {
        date.checked_sub_days(Days::new(days.unsigned_abs()))
    }
}

/// An AS value in years above 89 becomes `089Y`.
fn cap_age(value: &str) -> String {
    let value = value.trim();
    match value.strip_suffix('Y').map(str::parse::<u32>) {
        Some(Ok(years)) if years > MAX_AGE_YEARS => format!("{MAX_AGE_YEARS:03}Y"),
        _ => value.to_string(),
    }
}

impl Default for Masker {
    fn default() -> Self {
        Self::new()
    }
}

/// Set once a masked session exists in the process. The mode is fixed for
/// a viewer, and a log line cannot ask which viewer it belongs to.
static MASKED_SESSION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The cause of a failure as a log line may show it: a library's or a
/// parser's text, which can quote the file it was reading. Escaped, and
/// left out altogether once the process has a masked session, which logs
/// nothing a file holds. Every log call that formats an error from reading
/// a file goes through this.
pub fn logged_cause(cause: &dyn std::fmt::Display) -> String {
    if MASKED_SESSION.load(std::sync::atomic::Ordering::Relaxed) {
        "not logged in a masked session".to_string()
    } else {
        format!("{cause:#}").escape_debug().to_string()
    }
}

impl Masker {
    pub fn new() -> Self {
        MASKED_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
        Self {
            keys: RandomState::new(),
            patients: Mutex::new(HashMap::new()),
        }
    }

    fn digest(&self, domain: &str, value: &str) -> u64 {
        self.keys.hash_one((domain, value))
    }

    /// The replacements for the file's patient. Patients are told apart by
    /// Patient ID, else Patient's Name, else study.
    pub fn patient(&self, file: &FileEntry) -> PatientMask {
        let key = [
            &file.patient_id,
            &file.patient_name,
            &file.study_instance_uid,
        ]
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| format!("file {}", file.index));
        let number = {
            let mut patients = self.patients.lock().expect("patient table lock poisoned");
            let next = patients.len() + 1;
            *patients.entry(key.clone()).or_insert(next)
        };
        let offset = self.digest("date shift", &key) % (2 * MAX_DATE_SHIFT_DAYS);
        let shift_days = if offset < MAX_DATE_SHIFT_DAYS {
            offset as i64 - MAX_DATE_SHIFT_DAYS as i64
        } else {
            (offset - MAX_DATE_SHIFT_DAYS) as i64 + 1
        };
        let reference_date = file
            .study_date
            .get(..8)
            .and_then(|digits| NaiveDate::parse_from_str(digits, "%Y%m%d").ok())
            .unwrap_or_else(|| Local::now().date_naive());
        PatientMask {
            number,
            shift_days,
            reference_date,
        }
    }

    /// An instance-level UID as a `2.25.` UID of its keyed hash; registered
    /// DICOM UIDs and blank values are returned unchanged.
    pub fn uid(&self, uid: &str) -> String {
        let trimmed = uid.trim();
        if trimmed.is_empty() || trimmed.starts_with(DICOM_UID_ROOT) {
            return uid.to_string();
        }
        let high = self.digest("uid high", trimmed) as u128;
        let low = self.digest("uid low", trimmed) as u128;
        format!("2.25.{}", (high << 64) | low)
    }

    /// The catalog entry of a masked session.
    pub fn summary(&self, file: &FileEntry) -> FileSummary {
        let mut summary = FileSummary::from(file);
        summary.display_name = format!("File {}", file.index + 1);
        if !file.format.is_raster() {
            let patient = self.patient(file);
            summary.patient_id = patient.id();
            summary.patient_name = patient.name();
            summary.study_date = patient.shift_date(&file.study_date);
        }
        summary.label = crate::loader::build_label(
            &summary.patient_id,
            &summary.modality,
            &summary.study_date,
            &summary.display_name,
        );
        summary.study_instance_uid = self.uid(&file.study_instance_uid);
        summary.series_instance_uid = self.uid(&file.series_instance_uid);
        summary.sop_instance_uid = self.uid(&file.sop_instance_uid);
        if hides_pixels(file) {
            summary.support_state = SupportState::Unsupported;
            summary.support_reason = Some(LABEL_IMAGE_REASON.to_string());
        }
        summary
    }

    /// A presentation state's creator and creation date, and the text of its
    /// annotation items, which is free text drawn on the image.
    pub fn semantic_context(&self, file: &FileEntry, response: &mut SemanticContextResponse) {
        let SemanticContext::PresentationState(state) = &mut response.context else {
            return;
        };
        let patient = self.patient(file);
        if let Some(name) = &mut state.content_creator_name {
            *name = MASKED.to_string();
        }
        if let Some(date) = &mut state.presentation_creation_date {
            *date = patient.shift_date(date);
        }
        for item in &mut state.items {
            state.skipped.masked_text += item.texts.len();
            item.texts.clear();
        }
    }

    /// Withholds the text objects a presentation state draws on a frame.
    pub fn graphic_annotations(&self, response: &mut GraphicAnnotationsResponse) {
        response.skipped.masked_text += response.texts.len();
        response.texts.clear();
    }

    /// Hashes every UID of a wire response: string values, alone or in
    /// arrays, under a field named `*_uid` or `*_uids`.
    pub fn uids_in_json(&self, value: &mut serde_json::Value) {
        use serde_json::Value;
        match value {
            Value::Array(items) => items.iter_mut().for_each(|item| self.uids_in_json(item)),
            Value::Object(fields) => {
                for (name, field) in fields.iter_mut() {
                    let names_uids = name.ends_with("_uid") || name.ends_with("_uids");
                    match field {
                        Value::String(uid) if names_uids => *uid = self.uid(uid),
                        Value::Array(uids) if names_uids => {
                            for uid in uids {
                                if let Value::String(uid) = uid {
                                    *uid = self.uid(uid);
                                }
                            }
                        }
                        other => self.uids_in_json(other),
                    }
                }
            }
            _ => {}
        }
    }

    /// Masks a serialized tag tree, or one selected element, of `file`.
    pub fn tags(&self, file: &FileEntry, nodes: &mut [TagNode]) {
        let patient = self.patient(file);
        for node in nodes {
            self.mask_tag(&patient, node, false);
        }
    }

    /// Masks the metadata tree of a raster image file: the tree
    /// `pixels::read_raster_tags` reads, with the server's `File` and `Note`
    /// leaves. `masking/raster.rs` has the rule: only listed values that
    /// describe the pixel grid are shown, and all text of the file, every
    /// unknown tag, and everything that says who, where, when or with what
    /// shows `[masked]`. A node selected from a tree masked here needs no
    /// further masking.
    pub fn raster_tags(&self, nodes: &mut [TagNode]) {
        raster::mask_tree(nodes);
    }

    /// Masks one element selected by a `tag/item/tag...` path. The tags above
    /// it decide as they would in the whole tree: below a private element
    /// everything is hidden, and below a sequence on the confidentiality
    /// profile list every value counts as listed.
    pub fn selected_tag(&self, file: &FileEntry, selector: &str, node: &mut TagNode) {
        let mut ancestors = selector
            .split('/')
            .step_by(2)
            .map(|tag| parse_tag(&format!("({})", tag.trim().trim_matches(['(', ')']))))
            .collect::<Vec<_>>();
        ancestors.pop();
        let private = |tag: &Option<(u16, u16)>| tag.is_none_or(|(group, _)| group % 2 == 1);
        if ancestors.iter().any(private) {
            node.value = masked_value();
            return;
        }
        let within_listed = ancestors.into_iter().flatten().any(is_listed);
        self.mask_tag(&self.patient(file), node, within_listed);
    }

    /// One element, by these rules in order:
    ///
    /// 1. private elements are hidden, except private creators;
    /// 2. sequences are kept and their items masked by the same rules; in a
    ///    sequence on the confidentiality profile list every value counts as
    ///    listed (`within_listed`);
    /// 3. Patient ID and Patient's Name show the pseudonym;
    /// 4. by value representation: UI hashed, DA and DT shifted, TM kept,
    ///    AS capped at 89 years, PN masked;
    /// 5. anything else on the confidentiality profile list, curve data and
    ///    overlay comments are masked.
    fn mask_tag(&self, patient: &PatientMask, node: &mut TagNode, within_listed: bool) {
        let Some((group, element)) = parse_tag(&node.tag) else {
            node.value = masked_value();
            return;
        };
        if group % 2 == 1 {
            let private_creator = (0x0010..=0x00FF).contains(&element);
            if !private_creator {
                node.value = masked_value();
            }
            return;
        }
        let listed = within_listed || is_listed((group, element));
        let text = match &mut node.value {
            TagValue::Sequence { items, .. } => {
                for item in items {
                    for child in item {
                        self.mask_tag(patient, child, listed);
                    }
                }
                return;
            }
            // A length or a read error shows no value.
            TagValue::Binary { .. } | TagValue::Error { .. } => return,
            TagValue::String { value } => Some(value),
            TagValue::Number { .. } | TagValue::Numbers { .. } => None,
        };
        let Some(text) = text else {
            if listed {
                node.value = masked_value();
            }
            return;
        };
        if text.trim().is_empty() {
            return;
        }
        // Multiple values were joined with "; " by the tag serializer.
        let each = |mask: &dyn Fn(&str) -> String| {
            text.split("; ").map(mask).collect::<Vec<_>>().join("; ")
        };
        *text = match ((group, element), node.vr.as_str()) {
            (PATIENT_ID, _) => patient.id(),
            (PATIENT_NAME, _) => patient.name(),
            (PATIENT_BIRTH_DATE, _) => patient.birth_date(text),
            (_, "UI") => each(&|uid| self.uid(uid)),
            (_, "DA" | "DT") => each(&|date| patient.shift_date(date)),
            (_, "TM") => return,
            (_, "AS") => each(&cap_age),
            (_, "PN") => MASKED.to_string(),
            _ if listed => MASKED.to_string(),
            _ => return,
        };
    }
}

/// Whether the confidentiality profile removes or replaces the attribute:
/// the profile list, curve data (50xx) and overlay comments (60xx,4000).
fn is_listed((group, element): (u16, u16)) -> bool {
    profile::PROFILE_TAGS
        .binary_search(&(group, element))
        .is_ok()
        || group & 0xFF00 == 0x5000
        || (group & 0xFF00 == 0x6000 && element == 0x4000)
}

fn masked_value() -> TagValue {
    TagValue::String {
        value: MASKED.to_string(),
    }
}

/// Whether the file's frames are withheld: a slide label or overview image
/// (Image Type value 3) photographs the label, with its identifiers.
pub fn hides_pixels(file: &FileEntry) -> bool {
    file.series_metadata
        .image_type
        .get(2)
        .is_some_and(|role| matches!(role.trim(), "LABEL" | "OVERVIEW"))
}

/// `(gggg,eeee)` as the tag serializer writes it.
fn parse_tag(tag: &str) -> Option<(u16, u16)> {
    let (group, element) = tag.strip_prefix('(')?.strip_suffix(')')?.split_once(',')?;
    Some((
        u16::from_str_radix(group, 16).ok()?,
        u16::from_str_radix(element, 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(patient_id: &str, study_date: &str) -> FileEntry {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        let mut file = crate::loader::test_entry(&fixture);
        file.patient_id = patient_id.to_string();
        file.patient_name = format!("{patient_id}^Name");
        file.study_date = study_date.to_string();
        file.study_instance_uid = "1.2.3.4".to_string();
        file
    }

    fn node(tag: &str, vr: &str, value: &str) -> TagNode {
        TagNode {
            tag: tag.to_string(),
            vr: vr.to_string(),
            keyword: String::new(),
            value: TagValue::String {
                value: value.to_string(),
            },
        }
    }

    fn masked(masker: &Masker, file: &FileEntry, mut node: TagNode) -> TagValue {
        masker.tags(file, std::slice::from_mut(&mut node));
        node.value
    }

    fn text(value: TagValue) -> String {
        match value {
            TagValue::String { value } => value,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn profile_table_is_sorted_for_binary_search() {
        assert!(profile::PROFILE_TAGS
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn patients_are_numbered_in_order_seen_and_keep_their_number() {
        let masker = Masker::new();
        let first = masker.patient(&file("MRN-1", "20200101"));
        let second = masker.patient(&file("MRN-2", "20200101"));
        let again = masker.patient(&file("MRN-1", "20210101"));

        assert_eq!(first.name(), "Patient 0001");
        assert_eq!(first.id(), "MASKED-0001");
        assert_eq!(second.name(), "Patient 0002");
        assert_eq!(again.number, first.number);
        assert_eq!(again.shift_days, first.shift_days);
    }

    #[test]
    fn dates_shift_by_one_nonzero_offset_within_a_year_and_keep_their_time() {
        let masker = Masker::new();
        for patient in 0..200 {
            let mask = masker.patient(&file(&format!("MRN-{patient}"), "20200615"));
            assert!(
                mask.shift_days != 0 && mask.shift_days.abs() <= 365,
                "{mask:?}"
            );
            let date = mask.shift_date("20200615");
            let moved = NaiveDate::parse_from_str(&date, "%Y%m%d").expect("shifted date");
            let original = NaiveDate::from_ymd_opt(2020, 6, 15).expect("date");
            assert_eq!((moved - original).num_days(), mask.shift_days);
            assert_eq!(
                mask.shift_date("20200615101500.5+0100"),
                format!("{date}101500.5+0100")
            );
        }
    }

    #[test]
    fn dates_without_a_calendar_day_are_masked_and_blank_stays_blank() {
        let mask = Masker::new().patient(&file("MRN", "20200615"));

        assert_eq!(mask.shift_date("2020"), MASKED);
        assert_eq!(mask.shift_date("not a date"), MASKED);
        assert_eq!(mask.shift_date(""), "");
    }

    #[test]
    fn uids_hash_consistently_and_registered_uids_stay() {
        let masker = Masker::new();
        let hashed = masker.uid("1.2.826.0.1.3680043.8.498.1");

        assert!(hashed.starts_with("2.25."), "{hashed}");
        assert!(hashed.len() <= 64);
        assert_eq!(hashed, masker.uid("1.2.826.0.1.3680043.8.498.1"));
        assert_ne!(hashed, masker.uid("1.2.826.0.1.3680043.8.498.2"));
        assert_ne!(hashed, Masker::new().uid("1.2.826.0.1.3680043.8.498.1"));
        assert_eq!(
            masker.uid("1.2.840.10008.5.1.4.1.1.2"),
            "1.2.840.10008.5.1.4.1.1.2"
        );
        assert_eq!(masker.uid(""), "");
    }

    #[test]
    fn json_uid_fields_are_hashed_at_any_depth() {
        let masker = Masker::new();
        let mut value = json!({
            "series_instance_uid": "1.2.3",
            "sop_class_uid": "1.2.840.10008.5.1.4.1.1.2",
            "frame_of_reference_uids": ["1.2.4", "1.2.5"],
            "path": "1.2.3",
            "stacks": [{ "pyramid_uid": null, "frames": [{ "sop_instance_uid": "1.2.6" }] }],
        });
        masker.uids_in_json(&mut value);

        assert_eq!(value["series_instance_uid"], json!(masker.uid("1.2.3")));
        assert_eq!(value["sop_class_uid"], json!("1.2.840.10008.5.1.4.1.1.2"));
        assert_eq!(
            value["frame_of_reference_uids"],
            json!([masker.uid("1.2.4"), masker.uid("1.2.5")])
        );
        assert_eq!(value["path"], json!("1.2.3"));
        assert_eq!(value["stacks"][0]["pyramid_uid"], json!(null));
        assert_eq!(
            value["stacks"][0]["frames"][0]["sop_instance_uid"],
            json!(masker.uid("1.2.6"))
        );
    }

    #[test]
    fn summary_shows_the_pseudonym_shifted_date_and_hashed_uids() {
        let masker = Masker::new();
        let file = file("MRN-1", "20200615");
        let summary = masker.summary(&file);
        let patient = masker.patient(&file);

        assert_eq!(summary.patient_id, "MASKED-0001");
        assert_eq!(summary.patient_name, "Patient 0001");
        assert_eq!(summary.study_date, patient.shift_date("20200615"));
        assert_eq!(summary.study_instance_uid, masker.uid("1.2.3.4"));
        assert_eq!(summary.display_name, format!("File {}", file.index + 1));
        assert!(!summary.label.contains("MRN-1"), "{}", summary.label);
        assert!(
            summary.label.starts_with("MASKED-0001"),
            "{}",
            summary.label
        );
    }

    #[test]
    fn tag_rules_follow_tag_then_value_representation_then_profile() {
        let masker = Masker::new();
        let file = file("MRN-1", "20200615");
        let patient = masker.patient(&file);
        let mask =
            |tag: &str, vr: &str, value: &str| text(masked(&masker, &file, node(tag, vr, value)));

        assert_eq!(mask("(0010,0020)", "LO", "MRN-1"), "MASKED-0001");
        assert_eq!(mask("(0010,0010)", "PN", "Doe^Jane"), "Patient 0001");
        assert_eq!(mask("(0008,0090)", "PN", "Smith^Alex"), MASKED);
        assert_eq!(
            mask("(0008,0020)", "DA", "20200615"),
            patient.shift_date("20200615")
        );
        assert_eq!(mask("(0008,0030)", "TM", "101500"), "101500");
        assert_eq!(mask("(0010,1010)", "AS", "093Y"), "089Y");
        assert_eq!(mask("(0010,1010)", "AS", "045Y"), "045Y");
        assert_eq!(mask("(0010,1010)", "AS", "018M"), "018M");
        assert_eq!(mask("(0020,000D)", "UI", "1.2.3.4"), masker.uid("1.2.3.4"));
        assert_eq!(
            mask("(0008,0016)", "UI", "1.2.840.10008.5.1.4.1.1.2"),
            "1.2.840.10008.5.1.4.1.1.2"
        );
        assert_eq!(
            mask("(0008,1155)", "UI", "1.2.3; 1.2.4"),
            format!("{}; {}", masker.uid("1.2.3"), masker.uid("1.2.4"))
        );
        // Profile list: institution, accession number, postal address.
        assert_eq!(mask("(0008,0080)", "LO", "General Hospital"), MASKED);
        assert_eq!(mask("(0008,0050)", "SH", "ACC123"), MASKED);
        assert_eq!(mask("(0010,1040)", "LO", "1 Main St 30301"), MASKED);
        // Kept: descriptions by decision, technical attributes, blanks.
        assert_eq!(mask("(0008,1030)", "LO", "CT CHEST"), "CT CHEST");
        assert_eq!(mask("(0008,103E)", "LO", "AXIAL"), "AXIAL");
        assert_eq!(mask("(0008,0060)", "CS", "CT"), "CT");
        assert_eq!(mask("(0010,0040)", "CS", "F"), "F");
        assert_eq!(mask("(0008,0080)", "LO", ""), "");
    }

    #[test]
    fn a_selected_element_is_masked_as_it_is_in_the_tree() {
        let masker = Masker::new();
        let file = file("MRN-1", "20200615");
        let select = |selector: &str, tag: &str, vr: &str, value: &str| {
            let mut node = node(tag, vr, value);
            masker.selected_tag(&file, selector, &mut node);
            text(node.value)
        };

        assert_eq!(
            select("(0010,0010)", "(0010,0010)", "PN", "Doe^Jane"),
            "Patient 0001"
        );
        assert_eq!(select("0008,0060", "(0008,0060)", "CS", "CT"), "CT");
        // A code value below Person Identification Code Sequence.
        assert_eq!(
            select(
                "(0008,0096)/0/(0040,1101)/0/(0008,0100)",
                "(0008,0100)",
                "SH",
                "NPI-4471"
            ),
            MASKED
        );
        // The same code value below an unlisted sequence.
        assert_eq!(
            select("(0008,2218)/0/(0008,0100)", "(0008,0100)", "SH", "T-D1100"),
            "T-D1100"
        );
        assert_eq!(
            select("(0009,1010)/0/(0008,0060)", "(0008,0060)", "CS", "CT"),
            MASKED
        );
    }

    #[test]
    fn birth_date_is_blank_when_it_implies_an_age_over_89() {
        let masker = Masker::new();
        let file = file("MRN-1", "20200615");
        let patient = masker.patient(&file);
        let mask = |value: &str| text(masked(&masker, &file, node("(0010,0030)", "DA", value)));

        assert_eq!(mask("19200101"), "");
        assert_eq!(mask("19300616"), patient.shift_date("19300616"));
        assert_eq!(mask("19800101"), patient.shift_date("19800101"));
    }

    #[test]
    fn private_values_are_hidden_and_sequences_are_masked_in_place() {
        let masker = Masker::new();
        let file = file("MRN-1", "20200615");

        assert_eq!(
            text(masked(&masker, &file, node("(0009,0010)", "LO", "VENDOR"))),
            "VENDOR"
        );
        assert_eq!(
            text(masked(&masker, &file, node("(0009,1001)", "LO", "secret"))),
            MASKED
        );
        let mut private_number = node("(0009,1002)", "DS", "");
        private_number.value = TagValue::Number { value: 7.0 };
        assert_eq!(text(masked(&masker, &file, private_number)), MASKED);

        let mut sequence = node("(5200,9229)", "SQ", "");
        sequence.value = TagValue::Sequence {
            items: vec![vec![
                node("(0008,1155)", "UI", "1.2.3"),
                node("(0008,0090)", "PN", "Smith^Alex"),
                node("(0008,0060)", "CS", "CT"),
            ]],
            truncated: false,
            total: None,
        };
        let TagValue::Sequence { items, .. } = masked(&masker, &file, sequence) else {
            panic!("sequence stays a sequence");
        };
        let values = items[0]
            .iter()
            .map(|child| text(child.value.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [masker.uid("1.2.3"), MASKED.to_string(), "CT".to_string()]
        );

        // Person Identification Code Sequence is on the profile list.
        let mut listed = node("(0040,1101)", "SQ", "");
        listed.value = TagValue::Sequence {
            items: vec![vec![
                node("(0008,0100)", "SH", "NPI-4471"),
                node("(0008,010C)", "UI", "1.2.3"),
            ]],
            truncated: false,
            total: None,
        };
        let TagValue::Sequence { items, .. } = masked(&masker, &file, listed) else {
            panic!("sequence stays a sequence");
        };
        assert_eq!(text(items[0][0].value.clone()), MASKED);
        assert_eq!(text(items[0][1].value.clone()), masker.uid("1.2.3"));
    }
}
