//! Per-frame stored-value conversions: the Modality transform the display
//! pipeline applies, and the Real World Value Mappings (or RT Dose scaling)
//! that give stored samples their physical units.
//!
//! Nothing here changes frame bytes. The viewer uses these conversions for
//! value readouts and mapped-unit windowing, and the semantic overlays use
//! them to colorize values in real-world units.

use crate::api::contracts::{
    FrameValueMapping, ModalityValueTransform, RealWorldValueMap, RealWorldValueTransform,
    ValueLookupTable,
};
use crate::dicom_values::{read_number, read_numbers, read_string, sequence_items};
use crate::semantic::{rwvm_mapping, valid_mapping};
use crate::types::{FileEntry, NativePixelDataKind};
use anyhow::Result;
use dicom_dictionary_std::{tags, uids};
use dicom_object::InMemDicomObject;

/// A LUT-based mapping covers at most a 16-bit stored range.
const MAX_REAL_WORLD_LUT_VALUES: usize = 65_536;
const MAX_MAPPINGS_PER_FRAME: usize = 16;

pub const REAL_WORLD_VALUE_MAPPING_SOURCE: &str = "real_world_value_mapping";
pub const DOSE_GRID_SCALING_SOURCE: &str = "dose_grid_scaling";

/// Every frame's conversions, read once per file.
#[derive(Debug, Clone)]
pub struct FileValueMappings {
    stored_value_type: &'static str,
    modality: ModalityValueTransform,
    /// Mappings of every frame whose per-frame functional group declares none.
    default: Vec<RealWorldValueMap>,
    /// Per-frame functional group mappings; empty when no frame declares any.
    per_frame: Vec<Option<Vec<RealWorldValueMap>>>,
}

impl FileValueMappings {
    /// Reads the header of `file`, stopping before any pixel data.
    pub fn read(file: &FileEntry) -> Result<Self> {
        let object = crate::pixels::open_header(&file.path)?;
        Ok(Self::from_object(file, &object))
    }

    /// Mappings follow functional-group precedence: a frame's own Real World
    /// Value Mapping functional group, else the shared group, else a
    /// top-level Real World Value Mapping Sequence. An RT Dose object's Dose
    /// Grid Scaling comes first.
    pub fn from_object(file: &FileEntry, object: &InMemDicomObject) -> Self {
        let shared = sequence_items(object, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE).first();
        let mut default = shared
            .and_then(declared_mappings)
            .or_else(|| declared_mappings(object))
            .unwrap_or_default();
        if file.sop_class_uid == uids::RT_DOSE_STORAGE {
            if let Some(dose) = dose_grid_scaling_map(object) {
                default.insert(0, dose);
            }
        }
        let per_frame = sequence_items(object, tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
            .iter()
            .take(file.frame_count as usize)
            .map(declared_mappings)
            .collect::<Vec<_>>();
        let per_frame = if per_frame.iter().any(Option::is_some) {
            per_frame
        } else {
            Vec::new()
        };
        let native = &file.series_metadata.native_pixel;
        Self {
            stored_value_type: stored_value_type(file),
            modality: ModalityValueTransform {
                rescale_slope: file.rescale_slope,
                rescale_intercept: file.rescale_intercept,
                rescale_type: read_string(object, tags::RESCALE_TYPE),
                lut: native
                    .modality_lut
                    .as_ref()
                    .filter(|lut| !lut.entries.is_empty())
                    .map(|lut| ValueLookupTable {
                        first_value_mapped: f64::from(lut.first_mapped_value),
                        values: lut.entries.iter().copied().map(f64::from).collect(),
                    }),
            },
            default,
            per_frame,
        }
    }

    /// The real-world conversions that apply to `frame`, preferred first.
    pub fn real_world(&self, frame: u32) -> &[RealWorldValueMap] {
        self.per_frame
            .get(frame as usize)
            .and_then(Option::as_deref)
            .unwrap_or(&self.default)
    }

    pub fn frame(&self, file_index: usize, frame: u32) -> FrameValueMapping {
        FrameValueMapping {
            file_index,
            frame_index: frame,
            stored_value_type: self.stored_value_type.to_string(),
            modality: self.modality.clone(),
            real_world: self.real_world(frame).to_vec(),
        }
    }
}

/// `integer`, `float32`, or `float64`, from the file's native pixel element.
pub fn stored_value_type(file: &FileEntry) -> &'static str {
    match file.series_metadata.native_pixel.pixel_data_kind {
        Some(NativePixelDataKind::Float32) => "float32",
        Some(NativePixelDataKind::Float64) => "float64",
        _ => "integer",
    }
}

/// The real-world value of one stored sample, or `None` outside the mapped
/// range.
pub fn map_value(map: &RealWorldValueMap, stored: f64) -> Option<f64> {
    if map.first_value_mapped.is_some_and(|first| stored < first)
        || map.last_value_mapped.is_some_and(|last| stored > last)
    {
        return None;
    }
    match &map.transform {
        RealWorldValueTransform::Linear { slope, intercept } => Some(stored * slope + intercept),
        RealWorldValueTransform::Lut { values } => {
            let offset = (stored - map.first_value_mapped.unwrap_or(0.0)).round();
            (offset >= 0.0)
                .then(|| values.get(offset as usize).copied())
                .flatten()
        }
    }
}

/// The mappings of one Real World Value Mapping Sequence, or `None` when
/// `group` does not declare the sequence.
fn declared_mappings(group: &InMemDicomObject) -> Option<Vec<RealWorldValueMap>> {
    group
        .element(tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE)
        .ok()?;
    Some(
        sequence_items(group, tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE)
            .iter()
            .filter_map(real_world_value_map)
            .take(MAX_MAPPINGS_PER_FRAME)
            .collect(),
    )
}

fn real_world_value_map(item: &InMemDicomObject) -> Option<RealWorldValueMap> {
    let summary = rwvm_mapping(item, REAL_WORLD_VALUE_MAPPING_SOURCE, None);
    if !valid_mapping(&summary) {
        return None;
    }
    let transform = match (summary.slope, summary.intercept) {
        (Some(slope), Some(intercept)) if slope.is_finite() && intercept.is_finite() => {
            RealWorldValueTransform::Linear { slope, intercept }
        }
        _ => {
            // The summary truncates its LUT for display; conversion needs
            // every entry.
            let values = read_numbers::<f64>(item, tags::REAL_WORLD_VALUE_LUT_DATA);
            if values.is_empty() || values.len() > MAX_REAL_WORLD_LUT_VALUES {
                return None;
            }
            RealWorldValueTransform::Lut { values }
        }
    };
    let units = summary.units?;
    let unit_label = if units.scheme.eq_ignore_ascii_case("UCUM") || units.meaning.is_empty() {
        units.value.clone()
    } else {
        units.meaning.clone()
    };
    Some(RealWorldValueMap {
        source: REAL_WORLD_VALUE_MAPPING_SOURCE.to_string(),
        label: summary.label,
        first_value_mapped: summary.first_value_mapped,
        last_value_mapped: summary.last_value_mapped,
        transform,
        unit_label,
        units: Some(units),
        quantity: summary.quantity,
    })
}

fn dose_grid_scaling_map(object: &InMemDicomObject) -> Option<RealWorldValueMap> {
    let scaling = read_number::<f64>(object, tags::DOSE_GRID_SCALING)
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let unit_label = match read_string(object, tags::DOSE_UNITS) {
        Some(units) if units.eq_ignore_ascii_case("GY") => "Gy".to_string(),
        Some(units) => units,
        None => String::new(),
    };
    Some(RealWorldValueMap {
        source: DOSE_GRID_SCALING_SOURCE.to_string(),
        label: Some("Dose".to_string()),
        first_value_mapped: None,
        last_value_mapped: None,
        transform: RealWorldValueTransform::Linear {
            slope: scaling,
            intercept: 0.0,
        },
        unit_label,
        units: None,
        quantity: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use std::path::Path;

    fn entry(frame_count: u32) -> FileEntry {
        let mut file = crate::loader::test_entry(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm"),
        );
        file.frame_count = frame_count;
        file
    }

    fn mapping(slope: f64, units: &str) -> InMemDicomObject {
        let code = InMemDicomObject::from_element_iter([
            DataElement::new(tags::CODE_VALUE, VR::SH, units),
            DataElement::new(tags::CODING_SCHEME_DESIGNATOR, VR::SH, "UCUM"),
            DataElement::new(tags::CODE_MEANING, VR::LO, units),
        ]);
        InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::REAL_WORLD_VALUE_FIRST_VALUE_MAPPED,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
            DataElement::new(
                tags::REAL_WORLD_VALUE_LAST_VALUE_MAPPED,
                VR::US,
                PrimitiveValue::from(100_u16),
            ),
            DataElement::new(
                tags::REAL_WORLD_VALUE_SLOPE,
                VR::FD,
                PrimitiveValue::from(slope),
            ),
            DataElement::new(
                tags::REAL_WORLD_VALUE_INTERCEPT,
                VR::FD,
                PrimitiveValue::from(1.0_f64),
            ),
            sequence(tags::MEASUREMENT_UNITS_CODE_SEQUENCE, vec![code]),
        ])
    }

    fn sequence(
        tag: dicom_core::Tag,
        items: Vec<InMemDicomObject>,
    ) -> DataElement<InMemDicomObject> {
        DataElement::new(tag, VR::SQ, DataSetSequence::from(items))
    }

    fn group(mappings: Vec<InMemDicomObject>) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([sequence(
            tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE,
            mappings,
        )])
    }

    #[test]
    fn per_frame_mappings_override_shared_ones() {
        let object = InMemDicomObject::from_element_iter([
            sequence(
                tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![group(vec![mapping(2.0, "s")])],
            ),
            sequence(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![
                    InMemDicomObject::new_empty(),
                    group(vec![mapping(3.0, "ms")]),
                ],
            ),
        ]);
        let mappings = FileValueMappings::from_object(&entry(2), &object);

        assert_eq!(mappings.real_world(0)[0].unit_label, "s");
        assert_eq!(mappings.real_world(1)[0].unit_label, "ms");
        assert_eq!(map_value(&mappings.real_world(1)[0], 10.0), Some(31.0));
        assert_eq!(map_value(&mappings.real_world(1)[0], 101.0), None);
        let frame = mappings.frame(4, 1);
        assert_eq!(frame.file_index, 4);
        assert_eq!(frame.stored_value_type, "integer");
    }

    #[test]
    fn lut_mappings_index_from_the_first_mapped_value() {
        let mut lut = mapping(1.0, "1");
        lut.remove_element(tags::REAL_WORLD_VALUE_SLOPE);
        lut.remove_element(tags::REAL_WORLD_VALUE_INTERCEPT);
        lut.put(DataElement::new(
            tags::REAL_WORLD_VALUE_FIRST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(10_u16),
        ));
        lut.put(DataElement::new(
            tags::REAL_WORLD_VALUE_LUT_DATA,
            VR::FD,
            PrimitiveValue::F64(vec![0.5, 1.5, 2.5].into()),
        ));
        let object = group(vec![lut]);
        let mappings = FileValueMappings::from_object(&entry(1), &object);
        let map = &mappings.real_world(0)[0];

        assert_eq!(map_value(map, 11.0), Some(1.5));
        assert_eq!(map_value(map, 9.0), None);
    }

    #[test]
    fn rt_dose_scaling_is_the_preferred_mapping() {
        let mut file = entry(1);
        file.sop_class_uid = uids::RT_DOSE_STORAGE.to_string();
        let object = InMemDicomObject::from_element_iter([
            DataElement::new(tags::DOSE_GRID_SCALING, VR::DS, "0.01"),
            DataElement::new(tags::DOSE_UNITS, VR::CS, "GY"),
        ]);
        let mappings = FileValueMappings::from_object(&file, &object);
        let dose = &mappings.real_world(0)[0];

        assert_eq!(dose.source, DOSE_GRID_SCALING_SOURCE);
        assert_eq!(dose.unit_label, "Gy");
        assert_eq!(map_value(dose, 250.0), Some(2.5));
    }
}
