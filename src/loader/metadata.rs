use crate::dicom_values::{read_number, read_string, read_strings, sequence_item, sequence_items};
use crate::types::{
    DicomLut, DisplayShutter, OverlayPlane, PatientOrientation, PatientPosition,
    PresentationMetadata, ShutterShape,
};
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;

pub(super) fn read_positive_f64_pair(
    obj: &dicom_object::DefaultDicomObject,
    tag: dicom_core::Tag,
) -> Option<[f64; 2]> {
    let values = read_exact_f64s(obj, tag)?;
    values.iter().all(|value| *value > 0.0).then_some(values)
}

pub(super) fn read_positive_u32_pair(
    obj: &dicom_object::DefaultDicomObject,
    tag: dicom_core::Tag,
) -> Option<[u32; 2]> {
    let values = read_strings(obj, tag)
        .into_iter()
        .map(|value| value.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    let values: [u32; 2] = values.try_into().ok()?;
    values.iter().all(|value| *value > 0).then_some(values)
}

pub(super) fn normalize_pixel_aspect(
    pixel_spacing: Option<[f64; 2]>,
    pixel_aspect_ratio: Option<[u32; 2]>,
) -> Option<[f64; 2]> {
    let [row, column] = pixel_spacing.or_else(|| {
        pixel_aspect_ratio.map(|[vertical, horizontal]| [vertical as f64, horizontal as f64])
    })?;
    Some([row / column, 1.0])
}

pub(super) fn read_lut_sequence(
    obj: &dicom_object::DefaultDicomObject,
    sequence_tag: dicom_core::Tag,
) -> Option<DicomLut> {
    let item = sequence_item(obj, sequence_tag, 0)?;
    let descriptor = item
        .element(tags::LUT_DESCRIPTOR)
        .ok()?
        .to_multi_int::<i32>()
        .ok()?;
    let [entry_count, first_mapped_value, bits_per_entry]: [i32; 3] = descriptor.try_into().ok()?;
    let entry_count = if entry_count == 0 {
        65_536
    } else {
        usize::try_from(entry_count).ok()?
    };
    let bits_per_entry = u16::try_from(bits_per_entry).ok()?;
    if !matches!(bits_per_entry, 8 | 16) {
        return None;
    }
    let data = item.element(tags::LUT_DATA).ok()?;
    let entries = if bits_per_entry == 8 {
        let bytes = data.to_bytes().ok()?;
        if bytes.len() >= entry_count && bytes.len() <= entry_count.saturating_add(1) {
            bytes
                .iter()
                .take(entry_count)
                .map(|value| u16::from(*value))
                .collect()
        } else {
            data.to_multi_int::<u16>()
                .ok()?
                .into_iter()
                .map(|value| value & 0x00FF)
                .collect()
        }
    } else {
        data.to_multi_int::<u16>().ok()?
    };
    if entries.len() < entry_count {
        return None;
    }
    Some(DicomLut {
        first_mapped_value,
        bits_per_entry,
        entries: entries.into_iter().take(entry_count).collect(),
    })
}

pub(super) fn read_presentation_metadata(
    obj: &dicom_object::DefaultDicomObject,
    frame_count: u32,
) -> PresentationMetadata {
    let mut overlay_planes = read_overlay_planes(obj);
    let module_shutter = read_display_shutter(obj, &mut overlay_planes);
    let shared_shutter = sequence_item(obj, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, 0)
        .and_then(read_frame_display_shutter);
    let frame_display_shutters = sequence_items(obj, tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
        .iter()
        .take(frame_count as usize)
        .map(read_frame_display_shutter)
        .collect::<Vec<_>>();
    PresentationMetadata {
        overlay_planes,
        display_shutter: shared_shutter.or(module_shutter),
        frame_display_shutters: if frame_display_shutters.iter().any(Option::is_some) {
            frame_display_shutters
        } else {
            Vec::new()
        },
    }
}

/// The Frame Display Shutter macro of one functional group item: Display
/// Shutter attributes inside a one-item sequence, which cannot reference an
/// overlay plane.
fn read_frame_display_shutter(group: &InMemDicomObject) -> Option<DisplayShutter> {
    let item = sequence_item(group, tags::FRAME_DISPLAY_SHUTTER_SEQUENCE, 0)?;
    read_display_shutter(item, &mut Vec::new())
}

fn read_overlay_planes(obj: &dicom_object::DefaultDicomObject) -> Vec<OverlayPlane> {
    (0x6000_u16..=0x601e)
        .step_by(2)
        .filter_map(|group| read_overlay_plane(obj, group))
        .collect()
}

fn read_overlay_plane(obj: &dicom_object::DefaultDicomObject, group: u16) -> Option<OverlayPlane> {
    let rows = read_u32_tag(obj, dicom_core::Tag(group, 0x0010))?;
    let columns = read_u32_tag(obj, dicom_core::Tag(group, 0x0011))?;
    let origin = obj
        .element(dicom_core::Tag(group, 0x0050))
        .ok()?
        .to_multi_int::<i32>()
        .ok()?
        .try_into()
        .ok()?;
    if rows == 0
        || columns == 0
        || read_u32_tag(obj, dicom_core::Tag(group, 0x0100))? != 1
        || read_u32_tag(obj, dicom_core::Tag(group, 0x0102))? != 0
    {
        return None;
    }
    let number_of_frames = read_u32_tag(obj, dicom_core::Tag(group, 0x0015)).unwrap_or(1);
    let image_frame_origin = read_u32_tag(obj, dicom_core::Tag(group, 0x0051)).unwrap_or(1);
    if number_of_frames == 0 || image_frame_origin == 0 {
        return None;
    }
    let required_words = u64::from(rows)
        .checked_mul(u64::from(columns))?
        .checked_mul(u64::from(number_of_frames))?
        .checked_add(15)?
        / 16;
    let required_words = usize::try_from(required_words).ok()?;
    let mut data = obj
        .element(dicom_core::Tag(group, 0x3000))
        .ok()?
        .to_multi_int::<u16>()
        .ok()?;
    if data.len() < required_words {
        return None;
    }
    data.truncate(required_words);

    Some(OverlayPlane {
        group,
        rows,
        columns,
        origin,
        overlay_type: read_string(obj, dicom_core::Tag(group, 0x0040)).unwrap_or_default(),
        number_of_frames,
        image_frame_origin,
        data,
    })
}

/// Reads the Display Shutter and Bitmap Display Shutter modules. A declared
/// shape whose attributes are missing or invalid is skipped, so a malformed
/// shape never hides more of the image than the valid ones. A bitmap
/// shutter's overlay plane moves out of `overlay_planes`: it masks the image
/// and is not drawn as an overlay.
fn read_display_shutter(
    obj: &InMemDicomObject,
    overlay_planes: &mut Vec<OverlayPlane>,
) -> Option<DisplayShutter> {
    let mut shapes = Vec::new();
    for shape in read_strings(obj, tags::SHUTTER_SHAPE) {
        let shape = match shape.to_ascii_uppercase().as_str() {
            "RECTANGULAR" => read_rectangular_shutter(obj),
            "CIRCULAR" => read_circular_shutter(obj),
            "POLYGONAL" => read_polygonal_shutter(obj),
            "BITMAP" => take_bitmap_shutter(obj, overlay_planes),
            _ => None,
        };
        shapes.extend(shape);
    }
    if shapes.is_empty() {
        return None;
    }
    let presentation_color_cielab =
        read_exact_u16s::<3>(obj, tags::SHUTTER_PRESENTATION_COLOR_CIE_LAB_VALUE);
    Some(DisplayShutter {
        shapes,
        // Grayscale falls back to the color's L*: both are perceptual
        // 0-FFFFH scales. Without either the shutter is black.
        presentation_value: obj
            .element(tags::SHUTTER_PRESENTATION_VALUE)
            .ok()
            .and_then(|element| element.to_int::<u16>().ok())
            .or(presentation_color_cielab.map(|[lightness, _, _]| lightness))
            .unwrap_or(0),
        presentation_color_cielab,
    })
}

fn read_rectangular_shutter(obj: &InMemDicomObject) -> Option<ShutterShape> {
    let left_vertical_edge = read_number::<i32>(obj, tags::SHUTTER_LEFT_VERTICAL_EDGE)?;
    let right_vertical_edge = read_number::<i32>(obj, tags::SHUTTER_RIGHT_VERTICAL_EDGE)?;
    let upper_horizontal_edge = read_number::<i32>(obj, tags::SHUTTER_UPPER_HORIZONTAL_EDGE)?;
    let lower_horizontal_edge = read_number::<i32>(obj, tags::SHUTTER_LOWER_HORIZONTAL_EDGE)?;
    (left_vertical_edge <= right_vertical_edge && upper_horizontal_edge <= lower_horizontal_edge)
        .then_some(ShutterShape::Rectangular {
            left_vertical_edge,
            right_vertical_edge,
            upper_horizontal_edge,
            lower_horizontal_edge,
        })
}

fn read_circular_shutter(obj: &InMemDicomObject) -> Option<ShutterShape> {
    let center = read_exact_i32s::<2>(obj, tags::CENTER_OF_CIRCULAR_SHUTTER)?;
    let radius = read_number::<i32>(obj, tags::RADIUS_OF_CIRCULAR_SHUTTER)?;
    (radius >= 0).then_some(ShutterShape::Circular { center, radius })
}

fn read_polygonal_shutter(obj: &InMemDicomObject) -> Option<ShutterShape> {
    let values = read_strings(obj, tags::VERTICES_OF_THE_POLYGONAL_SHUTTER)
        .into_iter()
        .map(|value| value.parse::<i32>().ok())
        .collect::<Option<Vec<_>>>()?;
    // Row/column pairs: an origin vertex and at least two more.
    if values.len() < 6 || values.len() % 2 != 0 {
        return None;
    }
    Some(ShutterShape::Polygonal {
        vertices: values
            .chunks_exact(2)
            .map(|pair| [pair[0], pair[1]])
            .collect(),
    })
}

fn take_bitmap_shutter(
    obj: &InMemDicomObject,
    overlay_planes: &mut Vec<OverlayPlane>,
) -> Option<ShutterShape> {
    let group = read_u32_tag(obj, tags::SHUTTER_OVERLAY_GROUP)?;
    let index = overlay_planes
        .iter()
        .position(|plane| u32::from(plane.group) == group)?;
    Some(ShutterShape::Bitmap(overlay_planes.remove(index)))
}

fn read_exact_u16s<const N: usize>(
    obj: &InMemDicomObject,
    tag: dicom_core::Tag,
) -> Option<[u16; N]> {
    obj.element(tag)
        .ok()?
        .to_multi_int::<u16>()
        .ok()?
        .try_into()
        .ok()
}

fn read_exact_i32s<const N: usize>(
    obj: &InMemDicomObject,
    tag: dicom_core::Tag,
) -> Option<[i32; N]> {
    read_strings(obj, tag)
        .into_iter()
        .map(|value| value.parse::<i32>().ok())
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

fn read_u32_tag(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<u32> {
    obj.element(tag).ok()?.to_int::<u32>().ok()
}

pub(super) fn read_exact_f64s<const N: usize>(
    obj: &dicom_object::InMemDicomObject,
    tag: dicom_core::Tag,
) -> Option<[f64; N]> {
    let values = read_strings(obj, tag)
        .into_iter()
        .map(|value| value.parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let values: [f64; N] = values.try_into().ok()?;
    values
        .iter()
        .all(|value| value.is_finite())
        .then_some(values)
}

pub(super) fn read_sequence_strings(
    obj: &dicom_object::DefaultDicomObject,
    sequence_tag: dicom_core::Tag,
    value_tag: dicom_core::Tag,
) -> Vec<String> {
    sequence_items(obj, sequence_tag)
        .iter()
        .flat_map(|item| read_strings(item, value_tag))
        .filter(|value| !value.is_empty())
        .collect()
}

type FramePatientGeometry = (
    Vec<Option<PatientPosition>>,
    Vec<Option<PatientOrientation>>,
    Vec<Option<[f64; 2]>>,
    Option<[f64; 2]>,
);

pub(super) fn read_frame_patient_geometry(
    obj: &dicom_object::DefaultDicomObject,
    frame_count: u32,
    top_level_position: Option<PatientPosition>,
    top_level_orientation: Option<PatientOrientation>,
    top_level_pixel_spacing: Option<[f64; 2]>,
) -> FramePatientGeometry {
    let shared_orientation = sequence_item(obj, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, 0)
        .and_then(|item| {
            read_nested_exact_f64s(
                item,
                tags::PLANE_ORIENTATION_SEQUENCE,
                tags::IMAGE_ORIENTATION_PATIENT,
            )
        });
    let shared_pixel_spacing = sequence_item(obj, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, 0)
        .and_then(|item| {
            read_nested_exact_f64s(item, tags::PIXEL_MEASURES_SEQUENCE, tags::PIXEL_SPACING)
        })
        .filter(|values: &[f64; 2]| values.iter().all(|value| value.is_finite() && *value > 0.0));
    let per_frame_items = obj
        .element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
        .ok()
        .and_then(|element| element.items());

    if per_frame_items.is_none() && shared_orientation.is_none() {
        return (Vec::new(), Vec::new(), Vec::new(), shared_pixel_spacing);
    }

    let frame_count = frame_count as usize;
    let mut positions = Vec::with_capacity(frame_count);
    let mut orientations = Vec::with_capacity(frame_count);
    let mut pixel_spacings = Vec::with_capacity(frame_count);
    for frame_index in 0..frame_count {
        let frame_item = per_frame_items.and_then(|items| items.get(frame_index));
        positions.push(
            frame_item
                .and_then(|item| {
                    read_nested_exact_f64s(
                        item,
                        tags::PLANE_POSITION_SEQUENCE,
                        tags::IMAGE_POSITION_PATIENT,
                    )
                })
                .or(top_level_position),
        );
        orientations.push(
            frame_item
                .and_then(|item| {
                    read_nested_exact_f64s(
                        item,
                        tags::PLANE_ORIENTATION_SEQUENCE,
                        tags::IMAGE_ORIENTATION_PATIENT,
                    )
                })
                .or(shared_orientation)
                .or(top_level_orientation),
        );
        pixel_spacings.push(
            frame_item
                .and_then(|item| {
                    read_nested_exact_f64s(item, tags::PIXEL_MEASURES_SEQUENCE, tags::PIXEL_SPACING)
                })
                .filter(|values: &[f64; 2]| {
                    values.iter().all(|value| value.is_finite() && *value > 0.0)
                })
                .or(shared_pixel_spacing)
                .or(top_level_pixel_spacing),
        );
    }
    (
        positions,
        orientations,
        pixel_spacings,
        shared_pixel_spacing,
    )
}

fn read_nested_exact_f64s<const N: usize>(
    item: &dicom_object::InMemDicomObject,
    sequence_tag: dicom_core::Tag,
    value_tag: dicom_core::Tag,
) -> Option<[f64; N]> {
    read_exact_f64s(sequence_item(item, sequence_tag, 0)?, value_tag)
}

#[cfg(test)]
mod tests {
    use super::super::entry::{build_entry, EntryInspection};
    use super::super::test_fixtures::base_object;
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
    use tempfile::tempdir;

    use super::read_presentation_metadata;
    use crate::types::{
        DisplayShutter, NativePixelDataKind, OverlayPlane, PresentationMetadata, ShutterShape,
    };

    #[test]
    fn extracts_classic_enhanced_concatenation_and_wsi_identity() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("series-identity.dcm");
        write_series_identity_fixture(&path);

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let metadata = &file.series_metadata;

        assert_eq!(
            metadata.native_pixel.pixel_data_kind,
            Some(NativePixelDataKind::Integer)
        );
        assert_eq!(metadata.native_pixel.planar_configuration, Some(0));
        assert_eq!(metadata.native_pixel.bits_stored, Some(12));
        assert_eq!(metadata.native_pixel.high_bit, Some(11));
        assert_eq!(metadata.native_pixel.pixel_spacing, Some([0.6, 0.3]));
        assert_eq!(metadata.native_pixel.pixel_aspect_ratio, Some([3, 1]));
        assert_eq!(
            metadata.native_pixel.normalized_pixel_aspect,
            Some([2.0, 1.0])
        );
        assert_eq!(file.study_instance_uid, "2.25.100");
        assert_eq!(file.series_instance_uid, "2.25.200");
        assert_eq!(file.sop_instance_uid, "2.25.300");
        assert_eq!(file.instance_number, "7");
        assert_eq!(metadata.frame_of_reference_uid, "2.25.400");
        assert_eq!(metadata.image_position_patient, Some([10.0, 20.0, 30.0]));
        assert_eq!(
            metadata.image_orientation_patient,
            Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
        );
        assert_eq!(
            metadata.frame_image_positions_patient,
            vec![Some([0.0, 0.0, 1.0]), Some([0.0, 0.0, 2.0])]
        );
        assert_eq!(
            metadata.frame_image_orientations_patient,
            vec![
                Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                Some([0.0, 1.0, 0.0, 1.0, 0.0, 0.0]),
            ]
        );
        assert_eq!(
            metadata.frame_pixel_spacings,
            vec![Some([0.6, 0.3]), Some([0.6, 0.3])]
        );
        assert_eq!(metadata.concatenation_uid.as_deref(), Some("2.25.500"));
        assert_eq!(metadata.in_concatenation_number, Some(1));
        assert_eq!(metadata.in_concatenation_total_number, Some(2));
        assert_eq!(metadata.concatenation_frame_offset_number, Some(3));
        assert_eq!(
            metadata.sop_instance_uid_of_concatenation_source.as_deref(),
            Some("2.25.600")
        );
        assert_eq!(
            metadata.image_type,
            ["ORIGINAL", "PRIMARY", "VOLUME", "NONE"]
        );
        assert_eq!(metadata.pyramid_uid.as_deref(), Some("2.25.700"));
        assert_eq!(
            metadata.dimension_organization_type.as_deref(),
            Some("TILED_FULL")
        );
        assert_eq!(
            metadata.dimension_organization_uids,
            ["2.25.801", "2.25.802"]
        );
        assert_eq!(
            metadata.image_orientation_slide,
            Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
        );
        assert_eq!(metadata.total_pixel_matrix_rows, Some(8));
        assert_eq!(metadata.total_pixel_matrix_columns, Some(12));
        assert_eq!(metadata.total_pixel_matrix_focal_planes, Some(1));
        assert_eq!(metadata.number_of_optical_paths, Some(2));
        assert_eq!(metadata.container_identifier.as_deref(), Some("SLIDE-1"));
        assert_eq!(metadata.specimen_uids, ["2.25.901", "2.25.902"]);
        assert_eq!(metadata.optical_path_identifiers, ["RGB", "IHC"]);
    }

    #[test]
    fn rejects_partial_or_non_finite_geometry_values() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("invalid-geometry.dcm");
        let object = base_object()
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        assert_eq!(file.series_metadata.image_position_patient, None);
        assert_eq!(file.series_metadata.image_orientation_patient, None);
        assert!(file
            .series_metadata
            .frame_image_positions_patient
            .is_empty());
        assert!(file
            .series_metadata
            .frame_image_orientations_patient
            .is_empty());
    }

    #[test]
    fn falls_back_to_valid_pixel_aspect_ratio() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("pixel-aspect-ratio.dcm");
        let mut object = base_object();
        object.put(DataElement::new(tags::PIXEL_SPACING, VR::DS, "0\\0.3"));
        object.put(DataElement::new(tags::PIXEL_ASPECT_RATIO, VR::IS, "2\\1"));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8; 4].into()),
        ));
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let metadata = &file.series_metadata.native_pixel;
        assert_eq!(metadata.pixel_spacing, None);
        assert_eq!(metadata.pixel_aspect_ratio, Some([2, 1]));
        assert_eq!(metadata.normalized_pixel_aspect, Some([2.0, 1.0]));
    }

    #[test]
    fn extracts_prepared_modality_lut_sequence() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("modality-lut.dcm");
        let lut_item = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::LUT_DESCRIPTOR,
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 16].into()),
            ),
            DataElement::new(
                tags::LUT_DATA,
                VR::OW,
                PrimitiveValue::U16(vec![0, 1024, 2048, 4095].into()),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::MODALITY_LUT_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![lut_item]),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8, 1, 2, 3].into()),
        ));
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let lut = file
            .series_metadata
            .native_pixel
            .modality_lut
            .as_ref()
            .expect("modality LUT");
        assert_eq!(lut.first_mapped_value, 0);
        assert_eq!(lut.bits_per_entry, 16);
        assert_eq!(lut.entries, [0, 1024, 2048, 4095]);
    }

    #[test]
    fn extracts_prepared_voi_lut_sequence() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("voi-lut.dcm");
        let lut_item = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::LUT_DESCRIPTOR,
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 16].into()),
            ),
            DataElement::new(
                tags::LUT_DATA,
                VR::OW,
                PrimitiveValue::U16(vec![0, 21_845, 43_690, 65_535].into()),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::VOILUT_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![lut_item]),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8, 1, 2, 3].into()),
        ));
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(&path).expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let lut = file
            .series_metadata
            .native_pixel
            .voi_lut
            .as_ref()
            .expect("VOI LUT");
        assert_eq!(lut.first_mapped_value, 0);
        assert_eq!(lut.bits_per_entry, 16);
        assert_eq!(lut.entries, [0, 21_845, 43_690, 65_535]);
    }

    #[test]
    fn extracts_eight_bit_lut_entries_from_byte_storage() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("voi-lut-u8.dcm");
        let lut_item = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::LUT_DESCRIPTOR,
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 8].into()),
            ),
            DataElement::new(
                tags::LUT_DATA,
                VR::OW,
                PrimitiveValue::U8(vec![0, 85, 170, 255].into()),
            ),
        ]);
        let mut object = base_object();
        object.put(DataElement::new(
            tags::VOILUT_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![lut_item]),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8, 1, 2, 3].into()),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.301"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let lut = file
            .series_metadata
            .native_pixel
            .voi_lut
            .as_ref()
            .expect("8-bit VOI LUT");
        assert_eq!(lut.bits_per_entry, 8);
        assert_eq!(lut.entries, [0, 85, 170, 255]);
    }

    #[test]
    fn extracts_overlay_plane_and_rectangular_shutter_metadata() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("presentation-metadata.dcm");
        let mut object = base_object();
        for element in [
            DataElement::new(Tag(0x6000, 0x0010), VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(Tag(0x6000, 0x0011), VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(Tag(0x6000, 0x0040), VR::CS, "G"),
            DataElement::new(
                Tag(0x6000, 0x0050),
                VR::SS,
                PrimitiveValue::I16(vec![1, 1].into()),
            ),
            DataElement::new(Tag(0x6000, 0x0100), VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(Tag(0x6000, 0x0102), VR::US, PrimitiveValue::from(0_u16)),
            DataElement::new(
                Tag(0x6000, 0x3000),
                VR::OW,
                PrimitiveValue::U16(vec![0x0009].into()),
            ),
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "RECTANGULAR"),
            DataElement::new(tags::SHUTTER_LEFT_VERTICAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_RIGHT_VERTICAL_EDGE, VR::IS, "2"),
            DataElement::new(tags::SHUTTER_UPPER_HORIZONTAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_LOWER_HORIZONTAL_EDGE, VR::IS, "2"),
            DataElement::new(
                tags::SHUTTER_PRESENTATION_VALUE,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
        ] {
            object.put(element);
        }
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; 8].into()),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::COMPUTED_RADIOGRAPHY_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        let presentation = &file.series_metadata.presentation;
        assert_eq!(presentation.overlay_planes.len(), 1);
        let overlay = &presentation.overlay_planes[0];
        assert_eq!(overlay.group, 0x6000);
        assert_eq!((overlay.rows, overlay.columns), (2, 2));
        assert_eq!(overlay.origin, [1, 1]);
        assert_eq!(overlay.overlay_type, "G");
        assert_eq!(overlay.number_of_frames, 1);
        assert_eq!(overlay.image_frame_origin, 1);
        assert_eq!(overlay.data, [0x0009]);
        assert_eq!(
            presentation.display_shutter,
            Some(full_frame_rectangular_shutter())
        );
    }

    fn full_frame_rectangular_shutter() -> DisplayShutter {
        DisplayShutter {
            shapes: vec![ShutterShape::Rectangular {
                left_vertical_edge: 1,
                right_vertical_edge: 2,
                upper_horizontal_edge: 1,
                lower_horizontal_edge: 2,
            }],
            presentation_value: 0,
            presentation_color_cielab: None,
        }
    }

    fn presentation_of(elements: Vec<DataElement<InMemDicomObject>>) -> PresentationMetadata {
        let mut object = base_object();
        for element in elements {
            object.put(element);
        }
        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::ENHANCED_CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        read_presentation_metadata(&object, 2)
    }

    fn overlay_plane_elements(group: u16) -> Vec<DataElement<InMemDicomObject>> {
        vec![
            DataElement::new(Tag(group, 0x0010), VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(Tag(group, 0x0011), VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(Tag(group, 0x0040), VR::CS, "G"),
            DataElement::new(
                Tag(group, 0x0050),
                VR::SS,
                PrimitiveValue::I16(vec![1, 1].into()),
            ),
            DataElement::new(Tag(group, 0x0100), VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(Tag(group, 0x0102), VR::US, PrimitiveValue::from(0_u16)),
            DataElement::new(
                Tag(group, 0x3000),
                VR::OW,
                PrimitiveValue::U16(vec![0x0009].into()),
            ),
        ]
    }

    #[test]
    fn extracts_combined_circular_and_polygonal_shutter_shapes() {
        let presentation = presentation_of(vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR\\POLYGONAL"),
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "3\\4"),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "2"),
            DataElement::new(
                tags::VERTICES_OF_THE_POLYGONAL_SHUTTER,
                VR::IS,
                "1\\1\\1\\5\\5\\1",
            ),
            DataElement::new(
                tags::SHUTTER_PRESENTATION_VALUE,
                VR::US,
                PrimitiveValue::from(0xFFFF_u16),
            ),
        ]);
        assert_eq!(
            presentation.display_shutter,
            Some(DisplayShutter {
                shapes: vec![
                    ShutterShape::Circular {
                        center: [3, 4],
                        radius: 2,
                    },
                    ShutterShape::Polygonal {
                        vertices: vec![[1, 1], [1, 5], [5, 1]],
                    },
                ],
                presentation_value: 0xFFFF,
                presentation_color_cielab: None,
            })
        );
    }

    #[test]
    fn skips_invalid_shapes_and_keeps_the_valid_ones() {
        // Two vertices do not make a polygon; the rectangle still applies,
        // and without a Shutter Presentation Value the shutter is black.
        let presentation = presentation_of(vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "POLYGONAL\\RECTANGULAR"),
            DataElement::new(
                tags::VERTICES_OF_THE_POLYGONAL_SHUTTER,
                VR::IS,
                "1\\1\\2\\2",
            ),
            DataElement::new(tags::SHUTTER_LEFT_VERTICAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_RIGHT_VERTICAL_EDGE, VR::IS, "2"),
            DataElement::new(tags::SHUTTER_UPPER_HORIZONTAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_LOWER_HORIZONTAL_EDGE, VR::IS, "2"),
        ]);
        assert_eq!(
            presentation.display_shutter,
            Some(full_frame_rectangular_shutter())
        );

        let invalid_only = presentation_of(vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR"),
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "3\\4"),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "-1"),
        ]);
        assert_eq!(invalid_only.display_shutter, None);
    }

    #[test]
    fn keeps_the_cielab_color_and_falls_back_to_its_lightness_for_gray() {
        let presentation = presentation_of(vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR"),
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "1\\1"),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "1"),
            DataElement::new(
                tags::SHUTTER_PRESENTATION_COLOR_CIE_LAB_VALUE,
                VR::US,
                PrimitiveValue::U16(vec![0x8000, 0x8080, 0x8080].into()),
            ),
        ]);
        let shutter = presentation.display_shutter.expect("circular shutter");
        assert_eq!(shutter.presentation_value, 0x8000);
        assert_eq!(
            shutter.presentation_color_cielab,
            Some([0x8000, 0x8080, 0x8080])
        );
    }

    fn frame_display_shutter_group(
        shutter: Vec<DataElement<InMemDicomObject>>,
    ) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([DataElement::new(
            tags::FRAME_DISPLAY_SHUTTER_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter(shutter)]),
        )])
    }

    fn circle(center: &str) -> Vec<DataElement<InMemDicomObject>> {
        vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR"),
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, center),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "1"),
        ]
    }

    fn circle_shutter(center: [i32; 2]) -> DisplayShutter {
        DisplayShutter {
            shapes: vec![ShutterShape::Circular { center, radius: 1 }],
            presentation_value: 0,
            presentation_color_cielab: None,
        }
    }

    #[test]
    fn frame_display_shutters_come_from_per_frame_then_shared_groups() {
        // The shared group replaces the Display Shutter module for every
        // frame; frame 2 declares its own and frame 1 does not.
        let mut elements = circle("1\\1");
        elements.extend([
            DataElement::new(
                tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![frame_display_shutter_group(circle("2\\2"))]),
            ),
            DataElement::new(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![
                    InMemDicomObject::new_empty(),
                    frame_display_shutter_group(circle("3\\3")),
                ]),
            ),
        ]);
        let presentation = presentation_of(elements);

        assert_eq!(presentation.display_shutter, Some(circle_shutter([2, 2])));
        assert_eq!(
            presentation.display_shutter_for_frame(0),
            Some(&circle_shutter([2, 2]))
        );
        assert_eq!(
            presentation.display_shutter_for_frame(1),
            Some(&circle_shutter([3, 3]))
        );
        assert!(presentation.has_display_shutter());

        // Without functional groups the module shutter covers every frame.
        let module_only = presentation_of(circle("1\\1"));
        assert!(module_only.frame_display_shutters.is_empty());
        assert_eq!(
            module_only.display_shutter_for_frame(1),
            Some(&circle_shutter([1, 1]))
        );
    }

    #[test]
    fn bitmap_shutter_takes_its_overlay_plane_out_of_the_visible_overlays() {
        let mut elements = overlay_plane_elements(0x6000);
        elements.extend(overlay_plane_elements(0x6002));
        elements.extend([
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "BITMAP"),
            DataElement::new(
                tags::SHUTTER_OVERLAY_GROUP,
                VR::US,
                PrimitiveValue::from(0x6002_u16),
            ),
        ]);
        let presentation = presentation_of(elements);

        let groups = |planes: &[OverlayPlane]| planes.iter().map(|p| p.group).collect::<Vec<_>>();
        assert_eq!(groups(&presentation.overlay_planes), [0x6000]);
        let shutter = presentation.display_shutter.expect("bitmap shutter");
        let [ShutterShape::Bitmap(plane)] = shutter.shapes.as_slice() else {
            panic!("expected one bitmap shape: {:?}", shutter.shapes);
        };
        assert_eq!(plane.group, 0x6002);
        assert_eq!(plane.data, [0x0009]);

        // A Shutter Overlay Group naming no overlay plane declares nothing.
        let mut elements = overlay_plane_elements(0x6000);
        elements.extend([
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "BITMAP"),
            DataElement::new(
                tags::SHUTTER_OVERLAY_GROUP,
                VR::US,
                PrimitiveValue::from(0x6004_u16),
            ),
        ]);
        let unmatched = presentation_of(elements);
        assert_eq!(groups(&unmatched.overlay_planes), [0x6000]);
        assert_eq!(unmatched.display_shutter, None);
    }

    #[test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    fn prepared_overlay_and_shutter_metadata_match_locked_cases() {
        let overlay_path = crate::loader::prepared_corpus_case(
            "classic/cr/overlay_modality_voi_explicit_le/instance.dcm",
        );
        let shutter_path = crate::loader::prepared_corpus_case(
            "classic/dx/display_shutter_mono2_u16_explicit_le/instance.dcm",
        );

        let EntryInspection::Selected(overlay_file) =
            build_entry(&overlay_path).expect("inspect prepared CR")
        else {
            panic!("prepared CR should be selected");
        };
        let overlay = &overlay_file.series_metadata.presentation.overlay_planes[0];
        assert_eq!((overlay.rows, overlay.columns), (2, 2));
        assert_eq!(overlay.origin, [1, 1]);
        assert_eq!(overlay.data, [0x0009]);

        let EntryInspection::Selected(shutter_file) =
            build_entry(&shutter_path).expect("inspect prepared DX")
        else {
            panic!("prepared DX should be selected");
        };
        assert_eq!(
            shutter_file.series_metadata.presentation.display_shutter,
            Some(full_frame_rectangular_shutter())
        );
    }

    fn write_series_identity_fixture(path: &std::path::Path) {
        let mut shared_orientation = nested_sequence_item(
            tags::PLANE_ORIENTATION_SEQUENCE,
            tags::IMAGE_ORIENTATION_PATIENT,
            "1\\0\\0\\0\\1\\0",
        );
        shared_orientation.put(DataElement::new(
            tags::PIXEL_MEASURES_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter([
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "0.6\\0.3"),
            ])]),
        ));
        let frame_one = nested_sequence_item(
            tags::PLANE_POSITION_SEQUENCE,
            tags::IMAGE_POSITION_PATIENT,
            "0\\0\\1",
        );
        let mut frame_two = nested_sequence_item(
            tags::PLANE_POSITION_SEQUENCE,
            tags::IMAGE_POSITION_PATIENT,
            "0\\0\\2",
        );
        frame_two.put(DataElement::new(
            tags::PLANE_ORIENTATION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter([
                DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "0\\1\\0\\1\\0\\0"),
            ])]),
        ));

        let dimension_items = ["2.25.801", "2.25.802"]
            .map(|uid| {
                InMemDicomObject::from_element_iter([DataElement::new(
                    tags::DIMENSION_ORGANIZATION_UID,
                    VR::UI,
                    uid,
                )])
            })
            .to_vec();
        let specimen_items = ["2.25.901", "2.25.902"]
            .map(|uid| {
                InMemDicomObject::from_element_iter([DataElement::new(
                    tags::SPECIMEN_UID,
                    VR::UI,
                    uid,
                )])
            })
            .to_vec();
        let optical_path_items = ["RGB", "IHC"]
            .map(|identifier| {
                InMemDicomObject::from_element_iter([DataElement::new(
                    tags::OPTICAL_PATH_IDENTIFIER,
                    VR::SH,
                    identifier,
                )])
            })
            .to_vec();

        let mut object = base_object();
        for element in [
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(12_u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(11_u16)),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(3_u16)),
            DataElement::new(
                tags::PLANAR_CONFIGURATION,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
            DataElement::new(tags::PIXEL_ASPECT_RATIO, VR::IS, "3\\1"),
            DataElement::new(tags::FRAME_OF_REFERENCE_UID, VR::UI, "2.25.400"),
            DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, "10\\20\\30"),
            DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
            DataElement::new(tags::CONCATENATION_UID, VR::UI, "2.25.500"),
            DataElement::new(
                tags::IN_CONCATENATION_NUMBER,
                VR::US,
                PrimitiveValue::from(1_u16),
            ),
            DataElement::new(
                tags::IN_CONCATENATION_TOTAL_NUMBER,
                VR::US,
                PrimitiveValue::from(2_u16),
            ),
            DataElement::new(
                tags::CONCATENATION_FRAME_OFFSET_NUMBER,
                VR::UL,
                PrimitiveValue::from(3_u32),
            ),
            DataElement::new(
                tags::SOP_INSTANCE_UID_OF_CONCATENATION_SOURCE,
                VR::UI,
                "2.25.600",
            ),
            DataElement::new(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\VOLUME\\NONE"),
            DataElement::new(tags::PYRAMID_UID, VR::UI, "2.25.700"),
            DataElement::new(tags::DIMENSION_ORGANIZATION_TYPE, VR::CS, "TILED_FULL"),
            DataElement::new(tags::IMAGE_ORIENTATION_SLIDE, VR::DS, "1\\0\\0\\0\\1\\0"),
            DataElement::new(
                tags::TOTAL_PIXEL_MATRIX_ROWS,
                VR::UL,
                PrimitiveValue::from(8_u32),
            ),
            DataElement::new(
                tags::TOTAL_PIXEL_MATRIX_COLUMNS,
                VR::UL,
                PrimitiveValue::from(12_u32),
            ),
            DataElement::new(
                tags::TOTAL_PIXEL_MATRIX_FOCAL_PLANES,
                VR::UL,
                PrimitiveValue::from(1_u32),
            ),
            DataElement::new(
                tags::NUMBER_OF_OPTICAL_PATHS,
                VR::UL,
                PrimitiveValue::from(2_u32),
            ),
            DataElement::new(tags::CONTAINER_IDENTIFIER, VR::LO, "SLIDE-1"),
        ] {
            object.put(element);
        }
        object.put(DataElement::new(
            tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![shared_orientation]),
        ));
        object.put(DataElement::new(
            tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![frame_one, frame_two]),
        ));
        object.put(DataElement::new(
            tags::DIMENSION_ORGANIZATION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(dimension_items),
        ));
        object.put(DataElement::new(
            tags::SPECIMEN_DESCRIPTION_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(specimen_items),
        ));
        object.put(DataElement::new(
            tags::OPTICAL_PATH_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(optical_path_items),
        ));
        // Two 2x2 three-sample frames.
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; 24].into()),
        ));

        let object = object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::ENHANCED_CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta");
        object.write_to_file(path).expect("write fixture");
    }

    fn nested_sequence_item(
        sequence_tag: dicom_core::Tag,
        value_tag: dicom_core::Tag,
        value: &str,
    ) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([DataElement::new(
            sequence_tag,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter([
                DataElement::new(value_tag, VR::DS, value),
            ])]),
        )])
    }
}
