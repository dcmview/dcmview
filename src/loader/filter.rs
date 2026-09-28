use crate::types::FileEntry;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFilterField {
    PatientId,
    PatientName,
    StudyDescription,
    StudyDate,
    StudyUid,
    SeriesDescription,
    SeriesNumber,
    SeriesUid,
    Modality,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFilter {
    pub field: ScanFilterField,
    pub value: String,
}

/// Each filterable field with its two accepted spellings: the snake_case name
/// (used when printing a filter) and the DICOM keyword. Both parse
/// case-insensitively.
const FIELDS: &[(ScanFilterField, &str, &str)] = &[
    (ScanFilterField::PatientId, "patient_id", "PatientID"),
    (ScanFilterField::PatientName, "patient_name", "PatientName"),
    (
        ScanFilterField::StudyDescription,
        "study_description",
        "StudyDescription",
    ),
    (ScanFilterField::StudyDate, "study_date", "StudyDate"),
    (ScanFilterField::StudyUid, "study_uid", "StudyInstanceUID"),
    (
        ScanFilterField::SeriesDescription,
        "series_description",
        "SeriesDescription",
    ),
    (
        ScanFilterField::SeriesNumber,
        "series_number",
        "SeriesNumber",
    ),
    (
        ScanFilterField::SeriesUid,
        "series_uid",
        "SeriesInstanceUID",
    ),
    (ScanFilterField::Modality, "modality", "Modality"),
];

impl ScanFilter {
    pub fn matches(&self, entry: &FileEntry) -> bool {
        let haystack = match self.field {
            ScanFilterField::PatientId => &entry.patient_id,
            ScanFilterField::PatientName => &entry.patient_name,
            ScanFilterField::StudyDescription => &entry.study_description,
            ScanFilterField::StudyDate => &entry.study_date,
            ScanFilterField::StudyUid => &entry.study_instance_uid,
            ScanFilterField::SeriesDescription => &entry.series_description,
            ScanFilterField::SeriesNumber => &entry.series_number,
            ScanFilterField::SeriesUid => &entry.series_instance_uid,
            ScanFilterField::Modality => &entry.modality,
        };
        haystack.to_lowercase().contains(&self.value.to_lowercase())
    }
}

impl std::fmt::Display for ScanFilter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}={}", self.field, self.value)
    }
}

impl std::fmt::Display for ScanFilterField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (_, name, _) = FIELDS
            .iter()
            .find(|(field, _, _)| field == self)
            .expect("every filter field has a spelling");
        formatter.write_str(name)
    }
}

impl FromStr for ScanFilter {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let (field, value) = raw
            .split_once('=')
            .ok_or_else(|| scan_filter_parse_error(raw))?;
        let field = field.trim();
        let (field, _, _) = FIELDS
            .iter()
            .find(|(_, name, keyword)| {
                field.eq_ignore_ascii_case(name) || field.eq_ignore_ascii_case(keyword)
            })
            .ok_or_else(|| scan_filter_parse_error(raw))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(scan_filter_parse_error(raw));
        }
        Ok(Self {
            field: *field,
            value: value.to_string(),
        })
    }
}

fn scan_filter_parse_error(raw: &str) -> String {
    let fields = FIELDS
        .iter()
        .map(|(_, name, keyword)| format!("{name} ({keyword})"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "invalid scan filter `{raw}`; expected FIELD=VALUE where FIELD is one of: {fields} \
         (case-insensitive)"
    )
}

pub(super) fn matches_filters(entry: &FileEntry, filters: &[ScanFilter]) -> bool {
    filters.iter().all(|filter| filter.matches(entry))
}

#[cfg(test)]
mod tests {
    use super::{ScanFilter, ScanFilterField, FIELDS};

    #[test]
    fn both_spellings_parse_case_insensitively_and_print_snake_case() {
        for (field, name, keyword) in FIELDS {
            for spelling in [
                name.to_string(),
                keyword.to_string(),
                keyword.to_ascii_uppercase(),
                name.to_ascii_uppercase(),
            ] {
                let filter: ScanFilter = format!(" {spelling} = value ").parse().expect(&spelling);
                assert_eq!(filter.field, *field);
                assert_eq!(filter.to_string(), format!("{name}=value"));
            }
        }
    }

    #[test]
    fn every_field_has_one_spelling_row() {
        let fields = [
            ScanFilterField::PatientId,
            ScanFilterField::PatientName,
            ScanFilterField::StudyDescription,
            ScanFilterField::StudyDate,
            ScanFilterField::StudyUid,
            ScanFilterField::SeriesDescription,
            ScanFilterField::SeriesNumber,
            ScanFilterField::SeriesUid,
            ScanFilterField::Modality,
        ];
        assert_eq!(FIELDS.len(), fields.len());
        for field in fields {
            assert_eq!(FIELDS.iter().filter(|(row, _, _)| *row == field).count(), 1);
        }
    }

    #[test]
    fn parse_error_lists_both_spellings() {
        let error = "PatientBirthDate=1970".parse::<ScanFilter>().unwrap_err();
        assert!(error.contains("patient_id (PatientID)"), "{error}");
        assert!(error.contains("study_uid (StudyInstanceUID)"), "{error}");
    }
}
