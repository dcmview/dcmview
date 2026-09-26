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

impl ScanFilter {
    pub const VALID_FIELDS: &'static [&'static str] = &[
        "patient_id",
        "patient_name",
        "study_description",
        "study_date",
        "study_uid",
        "series_description",
        "series_number",
        "series_uid",
        "modality",
    ];

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
        let field = match self {
            ScanFilterField::PatientId => "patient_id",
            ScanFilterField::PatientName => "patient_name",
            ScanFilterField::StudyDescription => "study_description",
            ScanFilterField::StudyDate => "study_date",
            ScanFilterField::StudyUid => "study_uid",
            ScanFilterField::SeriesDescription => "series_description",
            ScanFilterField::SeriesNumber => "series_number",
            ScanFilterField::SeriesUid => "series_uid",
            ScanFilterField::Modality => "modality",
        };
        formatter.write_str(field)
    }
}

impl FromStr for ScanFilter {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let (field, value) = raw
            .split_once('=')
            .ok_or_else(|| scan_filter_parse_error(raw))?;
        let field_name = field.trim().to_ascii_lowercase();
        let field = match field_name.as_str() {
            "patient_id" => ScanFilterField::PatientId,
            "patient_name" => ScanFilterField::PatientName,
            "study_description" => ScanFilterField::StudyDescription,
            "study_date" => ScanFilterField::StudyDate,
            "study_uid" => ScanFilterField::StudyUid,
            "series_description" => ScanFilterField::SeriesDescription,
            "series_number" => ScanFilterField::SeriesNumber,
            "series_uid" => ScanFilterField::SeriesUid,
            "modality" => ScanFilterField::Modality,
            _ => return Err(scan_filter_parse_error(raw)),
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(scan_filter_parse_error(raw));
        }
        Ok(Self {
            field,
            value: value.to_string(),
        })
    }
}

fn scan_filter_parse_error(raw: &str) -> String {
    format!(
        "invalid scan filter `{raw}`; expected FIELD=VALUE where FIELD is one of: {}",
        ScanFilter::VALID_FIELDS.join(", ")
    )
}

pub(super) fn matches_filters(entry: &FileEntry, filters: &[ScanFilter]) -> bool {
    filters.iter().all(|filter| filter.matches(entry))
}
