use super::metadata::{
    normalize_pixel_aspect, read_exact_f64s, read_frame_patient_geometry, read_lut_sequence,
    read_positive_f64_pair, read_positive_u32_pair, read_presentation_metadata,
    read_sequence_strings,
};
use super::DiscoveryReason;
use crate::api::contracts::WindowPreset;
use crate::dicom_values::{read_first_string, read_number, read_strings};
use crate::types::{FileEntry, NativePixelDataKind, NativePixelMetadata, SeriesMetadata};
use anyhow::{Context, Result};
use dicom_core::header::HasLength;
use dicom_dictionary_std::{tags, uids};
use dicom_encoding::text::SpecificCharacterSet;
use dicom_encoding::{TransferSyntax, TransferSyntaxIndex};
use dicom_object::{FileMetaTable, OpenFileOptions};
use dicom_parser::dataset::read::DataSetReader;
use dicom_parser::dataset::DataToken;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;

pub(super) enum EntryInspection {
    Selected(Box<FileEntry>),
    Skipped(DiscoveryReason),
}

pub(super) fn build_entry(path: &Path) -> Result<EntryInspection> {
    if !has_dicm_preamble(path)? {
        return Ok(EntryInspection::Skipped(
            DiscoveryReason::MissingPart10Preamble,
        ));
    }

    let obj = match read_discovery_metadata(path) {
        Ok(obj) => obj,
        Err(_) => return Ok(EntryInspection::Skipped(DiscoveryReason::DicomParseFailed)),
    };
    if !valid_discovery_structure(&obj) {
        return Ok(EntryInspection::Skipped(DiscoveryReason::DicomParseFailed));
    }

    let transfer_syntax_uid = obj.meta().transfer_syntax().to_string();
    let patient_id = read_first_string(&obj, tags::PATIENT_ID).unwrap_or_default();
    let patient_name = read_first_string(&obj, tags::PATIENT_NAME).unwrap_or_default();
    let modality = read_first_string(&obj, tags::MODALITY).unwrap_or_default();
    let sop_instance_uid = read_first_string(&obj, tags::SOP_INSTANCE_UID).unwrap_or_default();
    let sop_class_uid = read_first_string(&obj, tags::SOP_CLASS_UID).unwrap_or_default();
    if sop_class_uid == uids::MEDIA_STORAGE_DIRECTORY_STORAGE {
        return Ok(EntryInspection::Skipped(
            DiscoveryReason::UnsupportedMediaDirectory,
        ));
    }
    let study_instance_uid = read_first_string(&obj, tags::STUDY_INSTANCE_UID).unwrap_or_default();
    let study_date = read_first_string(&obj, tags::STUDY_DATE).unwrap_or_default();
    let study_description = read_first_string(&obj, tags::STUDY_DESCRIPTION).unwrap_or_default();
    let series_instance_uid =
        read_first_string(&obj, tags::SERIES_INSTANCE_UID).unwrap_or_default();
    let series_number = read_first_string(&obj, tags::SERIES_NUMBER).unwrap_or_default();
    let series_description = read_first_string(&obj, tags::SERIES_DESCRIPTION).unwrap_or_default();
    let instance_number = read_first_string(&obj, tags::INSTANCE_NUMBER).unwrap_or_default();
    let frame_count = read_number::<u32>(&obj, tags::NUMBER_OF_FRAMES).unwrap_or(1);
    let frame_of_reference_uid =
        read_first_string(&obj, tags::FRAME_OF_REFERENCE_UID).unwrap_or_default();
    let image_position_patient = read_exact_f64s(&obj, tags::IMAGE_POSITION_PATIENT);
    let image_orientation_patient = read_exact_f64s(&obj, tags::IMAGE_ORIENTATION_PATIENT);
    let top_level_pixel_spacing = read_positive_f64_pair(&obj, tags::PIXEL_SPACING);
    let (
        frame_image_positions_patient,
        frame_image_orientations_patient,
        frame_pixel_spacings,
        shared_pixel_spacing,
    ) = read_frame_patient_geometry(
        &obj,
        frame_count,
        image_position_patient,
        image_orientation_patient,
        top_level_pixel_spacing,
    );
    let concatenation_uid = read_first_string(&obj, tags::CONCATENATION_UID);
    let in_concatenation_number = read_number::<u32>(&obj, tags::IN_CONCATENATION_NUMBER);
    let in_concatenation_total_number =
        read_number::<u32>(&obj, tags::IN_CONCATENATION_TOTAL_NUMBER);
    let concatenation_frame_offset_number =
        read_number::<u32>(&obj, tags::CONCATENATION_FRAME_OFFSET_NUMBER);
    let sop_instance_uid_of_concatenation_source =
        read_first_string(&obj, tags::SOP_INSTANCE_UID_OF_CONCATENATION_SOURCE);
    let image_type = read_strings(&obj, tags::IMAGE_TYPE);
    let pyramid_uid = read_first_string(&obj, tags::PYRAMID_UID);
    let dimension_organization_type = read_first_string(&obj, tags::DIMENSION_ORGANIZATION_TYPE);
    let dimension_organization_uids = read_sequence_strings(
        &obj,
        tags::DIMENSION_ORGANIZATION_SEQUENCE,
        tags::DIMENSION_ORGANIZATION_UID,
    );
    let image_orientation_slide = read_exact_f64s(&obj, tags::IMAGE_ORIENTATION_SLIDE);
    let total_pixel_matrix_rows = read_number::<u32>(&obj, tags::TOTAL_PIXEL_MATRIX_ROWS);
    let total_pixel_matrix_columns = read_number::<u32>(&obj, tags::TOTAL_PIXEL_MATRIX_COLUMNS);
    let total_pixel_matrix_focal_planes =
        read_number::<u32>(&obj, tags::TOTAL_PIXEL_MATRIX_FOCAL_PLANES);
    let number_of_optical_paths = read_number::<u32>(&obj, tags::NUMBER_OF_OPTICAL_PATHS);
    let container_identifier = read_first_string(&obj, tags::CONTAINER_IDENTIFIER);
    let specimen_uids = read_sequence_strings(
        &obj,
        tags::SPECIMEN_DESCRIPTION_SEQUENCE,
        tags::SPECIMEN_UID,
    );
    let optical_path_identifiers = read_sequence_strings(
        &obj,
        tags::OPTICAL_PATH_SEQUENCE,
        tags::OPTICAL_PATH_IDENTIFIER,
    );
    let rows = read_number::<u32>(&obj, tags::ROWS).unwrap_or(0);
    let columns = read_number::<u32>(&obj, tags::COLUMNS).unwrap_or(0);
    let bits_allocated = read_number::<u32>(&obj, tags::BITS_ALLOCATED).unwrap_or(8);
    let planar_configuration = read_number::<u32>(&obj, tags::PLANAR_CONFIGURATION);
    let bits_stored = read_number::<u32>(&obj, tags::BITS_STORED);
    let high_bit = read_number::<u32>(&obj, tags::HIGH_BIT);
    let pixel_spacing = top_level_pixel_spacing.or(shared_pixel_spacing);
    let pixel_aspect_ratio = read_positive_u32_pair(&obj, tags::PIXEL_ASPECT_RATIO);
    let normalized_pixel_aspect = normalize_pixel_aspect(pixel_spacing, pixel_aspect_ratio);
    let modality_lut = read_lut_sequence(&obj, tags::MODALITY_LUT_SEQUENCE);
    let voi_lut = read_lut_sequence(&obj, tags::VOILUT_SEQUENCE);
    let presentation = read_presentation_metadata(&obj, frame_count);
    let pixel_representation = read_number::<u32>(&obj, tags::PIXEL_REPRESENTATION).unwrap_or(0);
    let samples_per_pixel = read_number::<u32>(&obj, tags::SAMPLES_PER_PIXEL)
        .unwrap_or(1)
        .max(1);
    let photometric_interpretation = read_first_string(&obj, tags::PHOTOMETRIC_INTERPRETATION)
        .unwrap_or_else(|| "MONOCHROME2".to_string());
    let rescale_slope = read_number::<f64>(&obj, tags::RESCALE_SLOPE).unwrap_or(1.0);
    let rescale_intercept = read_number::<f64>(&obj, tags::RESCALE_INTERCEPT).unwrap_or(0.0);
    let pixel_data_kind = find_native_pixel_data_kind(path, &transfer_syntax_uid)?;
    let has_pixels = pixel_data_kind.is_some();
    let default_window = match (
        read_number::<f64>(&obj, tags::WINDOW_CENTER),
        read_number::<f64>(&obj, tags::WINDOW_WIDTH),
    ) {
        (Some(center), Some(width)) => Some(WindowPreset { center, width }),
        _ => None,
    };

    let fallback_label = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let label = build_label(&patient_id, &modality, &study_date, &fallback_label);

    Ok(EntryInspection::Selected(Box::new(FileEntry {
        index: 0,
        path: path.to_path_buf(),
        label,
        patient_id,
        patient_name,
        study_instance_uid,
        study_date,
        study_description,
        series_instance_uid,
        series_number,
        series_description,
        modality,
        instance_number,
        sop_instance_uid,
        sop_class_uid,
        series_metadata: Box::new(SeriesMetadata {
            native_pixel: NativePixelMetadata {
                planar_configuration,
                bits_stored,
                high_bit,
                pixel_data_kind,
                pixel_spacing,
                pixel_aspect_ratio,
                normalized_pixel_aspect,
                modality_lut,
                voi_lut,
            },
            presentation,
            frame_of_reference_uid,
            image_position_patient,
            image_orientation_patient,
            frame_image_positions_patient,
            frame_image_orientations_patient,
            frame_pixel_spacings,
            concatenation_uid,
            in_concatenation_number,
            in_concatenation_total_number,
            concatenation_frame_offset_number,
            sop_instance_uid_of_concatenation_source,
            image_type,
            pyramid_uid,
            dimension_organization_type,
            dimension_organization_uids,
            image_orientation_slide,
            total_pixel_matrix_rows,
            total_pixel_matrix_columns,
            total_pixel_matrix_focal_planes,
            number_of_optical_paths,
            container_identifier,
            specimen_uids,
            optical_path_identifiers,
        }),
        has_pixels,
        frame_count,
        rows,
        columns,
        bits_allocated,
        pixel_representation,
        samples_per_pixel,
        photometric_interpretation,
        rescale_slope,
        rescale_intercept,
        transfer_syntax_uid,
        default_window,
    })))
}

fn valid_discovery_structure(object: &dicom_object::DefaultDicomObject) -> bool {
    let character_set_valid = object
        .element(tags::SPECIFIC_CHARACTER_SET)
        .ok()
        .and_then(|element| element.to_str().ok())
        .map(|value| valid_specific_character_set(&value))
        .unwrap_or(true);
    character_set_valid && !has_odd_defined_item_length(object)
}

fn valid_specific_character_set(value: &str) -> bool {
    value.split('\\').all(|component| {
        let component = component.trim();
        component.is_empty() || SpecificCharacterSet::from_code(component).is_some()
    })
}

fn has_odd_defined_item_length(
    object: &dicom_object::InMemDicomObject<dicom_dictionary_std::StandardDataDictionary>,
) -> bool {
    object.iter().any(|element| {
        element.items().is_some_and(|items| {
            items.iter().any(|item| {
                item.length().get().is_some_and(|length| length % 2 != 0)
                    || has_odd_defined_item_length(item)
            })
        })
    })
}

fn read_discovery_metadata(path: &Path) -> Result<dicom_object::DefaultDicomObject> {
    Ok(OpenFileOptions::new()
        // Float and double-float pixel data precede the conventional Pixel Data
        // tag, so stopping there would materialize those payloads during scan.
        .read_until(tags::FLOAT_PIXEL_DATA)
        .open_file(path)?)
}

fn has_dicm_preamble(path: &Path) -> Result<bool> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut preamble = [0_u8; 132];
    match file.read_exact(&mut preamble) {
        Ok(()) => Ok(&preamble[128..132] == b"DICM"),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// The kind of the data set's own pixel data element, if it has one.
///
/// Walks element headers without reading pixel values and counts only
/// top-level elements, so a pixel element nested in a sequence (an Icon Image
/// Sequence, for example) or pixel-tag bytes inside another value never make
/// a no-pixel object look like an image.
fn find_native_pixel_data_kind(
    path: &Path,
    transfer_syntax_uid: &str,
) -> Result<Option<NativePixelDataKind>> {
    let mut reader = io::BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    reader
        .seek(io::SeekFrom::Start(128))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    FileMetaTable::from_reader(&mut reader)
        .with_context(|| format!("failed to read file meta: {}", path.display()))?;
    let transfer_syntax = TransferSyntaxRegistry
        .get(transfer_syntax_uid)
        .with_context(|| format!("unknown transfer syntax {transfer_syntax_uid}"))?;
    let kind = if transfer_syntax_uid == uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN {
        top_level_pixel_data_kind(flate2::read::DeflateDecoder::new(reader), transfer_syntax)
    } else {
        top_level_pixel_data_kind(reader, transfer_syntax)
    };
    kind.with_context(|| format!("failed to parse {}", path.display()))
}

fn top_level_pixel_data_kind(
    source: impl Read,
    transfer_syntax: &TransferSyntax,
) -> Result<Option<NativePixelDataKind>> {
    let tokens = DataSetReader::new_with_ts(source, transfer_syntax)?;
    let mut depth = 0_usize;
    for token in tokens {
        match token? {
            DataToken::PixelSequenceStart if depth == 0 => {
                return Ok(Some(NativePixelDataKind::Integer));
            }
            DataToken::SequenceStart { .. } | DataToken::PixelSequenceStart => depth += 1,
            DataToken::SequenceEnd => depth = depth.saturating_sub(1),
            DataToken::ElementHeader(header) if depth == 0 => {
                let kind = match header.tag {
                    tags::PIXEL_DATA => NativePixelDataKind::Integer,
                    tags::FLOAT_PIXEL_DATA => NativePixelDataKind::Float32,
                    tags::DOUBLE_FLOAT_PIXEL_DATA => NativePixelDataKind::Float64,
                    _ => continue,
                };
                return Ok(Some(kind));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn build_label(patient_id: &str, modality: &str, study_date: &str, fallback: &str) -> String {
    let mut fields = Vec::new();
    if !patient_id.is_empty() {
        fields.push(patient_id);
    }
    if !modality.is_empty() {
        fields.push(modality);
    }
    if !study_date.is_empty() {
        fields.push(study_date);
    }

    if fields.is_empty() {
        fallback.to_string()
    } else {
        fields.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_fixtures::base_object;
    use super::{
        build_entry, read_discovery_metadata, valid_specific_character_set, EntryInspection,
    };
    use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::meta::FileMetaTableBuilder;
    use tempfile::tempdir;

    use crate::types::NativePixelDataKind;

    #[test]
    fn validates_each_component_of_multi_valued_character_sets() {
        assert!(valid_specific_character_set(
            "\\ISO 2022 IR 87\\ISO 2022 IR 13"
        ));
        assert!(!valid_specific_character_set(
            "ISO_IR 100\\DTS_UNKNOWN_CHARACTER_SET"
        ));
    }

    #[test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    fn recognizes_prepared_deflated_image_frame_segmentation_metadata() {
        let path = crate::loader::prepared_corpus_case(
            "derived/seg/binary_multiframe_deflated_image_frame/instance.dcm",
        );

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect prepared SEG")
        else {
            panic!("prepared SEG should be selected");
        };

        assert_eq!(file.sop_class_uid, "1.2.840.10008.5.1.4.1.1.66.4");
        assert_eq!(file.transfer_syntax_uid, "1.2.840.10008.1.2.8.1");
        assert_eq!(file.modality, "SEG");
        assert_eq!(file.frame_count, 2);
        assert_eq!((file.rows, file.columns), (2, 2));
        assert_eq!(file.bits_allocated, 1);
        assert!(file.has_pixels);
    }

    #[test]
    fn recognizes_float_and_double_float_pixel_elements() {
        let directory = tempdir().expect("temp directory");
        let cases = [
            (
                "float.dcm",
                DataElement::new(
                    Tag(0x7fe0, 0x0008),
                    VR::OF,
                    PrimitiveValue::F32(vec![0.0_f32, 1.0].into()),
                ),
                NativePixelDataKind::Float32,
            ),
            (
                "double.dcm",
                DataElement::new(
                    Tag(0x7fe0, 0x0009),
                    VR::OD,
                    PrimitiveValue::F64(vec![0.0_f64, 1.0].into()),
                ),
                NativePixelDataKind::Float64,
            ),
        ];

        for (name, pixel_element, expected_kind) in cases {
            let path = directory.path().join(name);
            let mut object = base_object();
            object.put(pixel_element);
            let object = object
                .with_meta(
                    FileMetaTableBuilder::new()
                        .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                        .media_storage_sop_class_uid(uids::PARAMETRIC_MAP_STORAGE)
                        .media_storage_sop_instance_uid("2.25.300"),
                )
                .expect("file meta");
            object.write_to_file(&path).expect("write fixture");

            let metadata = read_discovery_metadata(&path).expect("read discovery metadata");
            let pixel_tag = match expected_kind {
                NativePixelDataKind::Integer => tags::PIXEL_DATA,
                NativePixelDataKind::Float32 => tags::FLOAT_PIXEL_DATA,
                NativePixelDataKind::Float64 => tags::DOUBLE_FLOAT_PIXEL_DATA,
            };
            assert!(metadata.element(pixel_tag).is_err());

            let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture")
            else {
                panic!("fixture should be selected");
            };
            assert!(file.has_pixels);
            assert_eq!(
                file.series_metadata.native_pixel.pixel_data_kind,
                Some(expected_kind)
            );
        }
    }

    #[test]
    fn nested_icon_pixels_and_tag_bytes_do_not_count_as_pixel_data() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("icon-only.dcm");
        let icon = dicom_object::InMemDicomObject::from_element_iter([
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(
                tags::PIXEL_DATA,
                VR::OB,
                PrimitiveValue::from(vec![0_u8, 0]),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::ICON_IMAGE_SEQUENCE,
            VR::SQ,
            dicom_core::value::DataSetSequence::from(vec![icon]),
        ));
        // Little-endian (7FE0,0010) bytes inside an unrelated value.
        object.put(DataElement::new(
            Tag(0x0009, 0x1010),
            VR::OB,
            PrimitiveValue::from(vec![0xe0_u8, 0x7f, 0x10, 0x00]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.301"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        assert!(!file.has_pixels);
        assert_eq!(file.series_metadata.native_pixel.pixel_data_kind, None);
    }
}
