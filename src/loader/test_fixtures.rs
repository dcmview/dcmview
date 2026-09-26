use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::InMemDicomObject;

/// A two-frame Enhanced CT data set whose top-level position and orientation
/// are deliberately invalid, shared by the entry and metadata unit tests.
pub(super) fn base_object() -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::ENHANCED_CT_IMAGE_STORAGE),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.300"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, "2.25.100"),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, "2.25.200"),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, "7"),
        DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(2_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, "1\\2"),
        DataElement::new(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            "NaN\\0\\0\\0\\1\\0",
        ),
    ])
}
