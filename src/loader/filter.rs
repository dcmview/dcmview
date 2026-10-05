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
    /// The file's format, by its `FileFormat` name.
    Format,
    /// The file's path as the catalog reports it.
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFilter {
    pub field: ScanFilterField,
    pub value: String,
}

/// Each filterable field with its accepted spellings: the snake_case name
/// (used when printing a filter) and, for a DICOM attribute, its keyword.
/// Both parse case-insensitively. `format` and `path` describe the file
/// rather than an attribute and have the one spelling.
const FIELDS: &[(ScanFilterField, &str, Option<&str>)] = &[
    (ScanFilterField::PatientId, "patient_id", Some("PatientID")),
    (
        ScanFilterField::PatientName,
        "patient_name",
        Some("PatientName"),
    ),
    (
        ScanFilterField::StudyDescription,
        "study_description",
        Some("StudyDescription"),
    ),
    (ScanFilterField::StudyDate, "study_date", Some("StudyDate")),
    (
        ScanFilterField::StudyUid,
        "study_uid",
        Some("StudyInstanceUID"),
    ),
    (
        ScanFilterField::SeriesDescription,
        "series_description",
        Some("SeriesDescription"),
    ),
    (
        ScanFilterField::SeriesNumber,
        "series_number",
        Some("SeriesNumber"),
    ),
    (
        ScanFilterField::SeriesUid,
        "series_uid",
        Some("SeriesInstanceUID"),
    ),
    (ScanFilterField::Modality, "modality", Some("Modality")),
    (ScanFilterField::Format, "format", None),
    (ScanFilterField::Path, "path", None),
];

impl ScanFilter {
    /// Whether `entry` passes this filter.
    ///
    /// The DICOM fields match when the entry's value contains the filter's
    /// value, ignoring case. A raster's DICOM fields are empty, so any DICOM
    /// filter excludes rasters.
    ///
    /// `format` matches when the filter's value equals the entry's
    /// `FileFormat` name, ignoring case: `format=png` selects PNG files and
    /// `format=dicom` DICOM files. It is an equality, not a substring.
    ///
    /// `path` matches when the entry's path, written as the catalog reports
    /// it (`FileEntry::path` through `Path::display`, not canonicalized),
    /// contains the filter's value, ignoring case.
    pub fn matches(&self, entry: &FileEntry) -> bool {
        let haystack = match self.field {
            ScanFilterField::Format => todo!("FMT1: match the file format name"),
            ScanFilterField::Path => todo!("FMT1: match the reported path"),
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

/// Parses `FIELD=VALUE`. A `format` filter's value must be a `FileFormat`
/// name (ignoring case); any other value is an error that lists the names.
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
                field.eq_ignore_ascii_case(name)
                    || keyword.is_some_and(|keyword| field.eq_ignore_ascii_case(keyword))
            })
            .ok_or_else(|| scan_filter_parse_error(raw))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(scan_filter_parse_error(raw));
        }
        if *field == ScanFilterField::Format {
            todo!("FMT1: accept only a FileFormat name as the value")
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
        .map(|(_, name, keyword)| match keyword {
            Some(keyword) => format!("{name} ({keyword})"),
            None => name.to_string(),
        })
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
            // A format filter only accepts a format name as its value.
            let value = if *field == ScanFilterField::Format {
                "png"
            } else {
                "value"
            };
            let mut spellings = vec![name.to_string(), name.to_ascii_uppercase()];
            if let Some(keyword) = keyword {
                spellings.extend([keyword.to_string(), keyword.to_ascii_uppercase()]);
            }
            for spelling in spellings {
                let filter: ScanFilter =
                    format!(" {spelling} = {value} ").parse().expect(&spelling);
                assert_eq!(filter.field, *field);
                assert_eq!(filter.to_string(), format!("{name}={value}"));
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
            ScanFilterField::Format,
            ScanFilterField::Path,
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
