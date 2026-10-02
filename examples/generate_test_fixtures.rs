use dicom_core::value::fragments::Fragments;
use dicom_core::value::{DataSetSequence, PixelFragmentSequence};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use image::{GrayImage, Luma};
use std::fs;
use std::path::{Path, PathBuf};

// Keep generated fixtures byte-for-byte stable across dicom-rs upgrades. These
// values are fixture provenance, not the implementation identity of dcmview.
const FIXTURE_IMPLEMENTATION_CLASS_UID: &str = "2.25.214312761802046835989399652652980912193";
const FIXTURE_IMPLEMENTATION_VERSION_NAME: &str = "DICOM-rs 0.9.0";

fn main() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&fixture_dir).expect("create fixture directory");

    write_uncompressed_multiframe(&fixture_dir.join("golden-uncompressed-u16-multiframe.dcm"));
    write_jpeg_single_frame(&fixture_dir.join("golden-jpeg-baseline-single-frame.dcm"));
    write_large_jpeg_single_frame(&fixture_dir.join("golden-jpeg-baseline-large-single-frame.dcm"));
    write_jpeg_multiframe_with_bot(&fixture_dir.join("golden-jpeg-baseline-multiframe-bot.dcm"));
    write_jpeg_lossless_single_frame(
        &fixture_dir.join("golden-jpeg-lossless-u16-single-frame.dcm"),
    );
    write_jpeg2000_lossless_single_frame(
        &fixture_dir.join("golden-jpeg2000-lossless-u8-single-frame.dcm"),
    );
    write_color_fixture(
        &fixture_dir.join("golden-rle-ybr-full-422-u8-single-frame.dcm"),
        ColorFixtureSpec {
            sop_instance_uid: "2.25.2000009",
            patient_id: "GOLDEN-RLE-YBR422",
            transfer_syntax_uid: uids::RLE_LOSSLESS,
            photometric_interpretation: "YBR_FULL_422",
        },
        rle_ybr_fragment_4x2(),
    );
    write_color_fixture(
        &fixture_dir.join("golden-jpeg-lossless-ybr-full-u8-single-frame.dcm"),
        ColorFixtureSpec {
            sop_instance_uid: "2.25.2000010",
            patient_id: "GOLDEN-JPEG-LOSSLESS-YBR",
            transfer_syntax_uid: "1.2.840.10008.1.2.4.70",
            photometric_interpretation: "YBR_FULL",
        },
        jpeg_lossless_ybr_fragment_4x2(),
    );
    write_color_fixture(
        &fixture_dir.join("golden-jpegxl-lossless-ybr-full-u8-single-frame.dcm"),
        ColorFixtureSpec {
            sop_instance_uid: "2.25.2000011",
            patient_id: "GOLDEN-JPEGXL-YBR",
            transfer_syntax_uid: "1.2.840.10008.1.2.4.110",
            photometric_interpretation: "YBR_FULL",
        },
        jpeg_xl_lossless_ybr_fragment_4x2(),
    );
    write_color_fixture(
        &fixture_dir.join("golden-jpegxl-lossless-ybr-rct-u8-single-frame.dcm"),
        ColorFixtureSpec {
            sop_instance_uid: "2.25.2000016",
            patient_id: "GOLDEN-JPEGXL-RCT",
            transfer_syntax_uid: "1.2.840.10008.1.2.4.110",
            photometric_interpretation: "YBR_RCT",
        },
        jpeg_xl_lossless_rct_fragment_4x2(),
    );
    write_display_shutter_fixtures(&fixture_dir);
    write_sr_without_pixels(&fixture_dir.join("golden-no-pixels-sr.dcm"));
    write_image_without_pixels(&fixture_dir.join("golden-image-no-pixels.dcm"));
    write_segmentation_fixtures(&fixture_dir);
    write_rt_dose_overlay_fixtures(&fixture_dir);
    write_parametric_map_overlay_fixtures(&fixture_dir);
    write_real_world_value_mapping_instance(&fixture_dir.join("golden-rwvm-ct-hounsfield.dcm"));
    write_presentation_state_fixtures(&fixture_dir);
    write_masking_fixtures(&fixture_dir);
}

// Semantic-overlay fixtures share one patient and study; each overlay pair
// shares its own Frame of Reference with its image series.
const OVERLAY_PATIENT_ID: &str = "GOLDEN-OVERLAY";
const OVERLAY_STUDY_UID: &str = "2.25.2000100";
const AXIAL: &str = "1\\0\\0\\0\\1\\0";

struct NativeU16Spec<'a> {
    sop_class_uid: &'a str,
    sop_instance_uid: &'a str,
    modality: &'a str,
    series_instance_uid: &'a str,
    frame_of_reference_uid: &'a str,
    rows: u16,
    columns: u16,
    /// Every frame's samples in frame, row, column order.
    samples: Vec<u16>,
}

/// An uncompressed 16-bit unsigned monochrome object with `elements` added.
fn write_native_u16(
    path: &Path,
    spec: NativeU16Spec<'_>,
    elements: Vec<DataElement<InMemDicomObject>>,
) {
    let pixel_bytes = spec
        .samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, spec.sop_class_uid),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, spec.sop_instance_uid),
        DataElement::new(tags::PATIENT_ID, VR::LO, OVERLAY_PATIENT_ID),
        DataElement::new(tags::STUDY_DATE, VR::DA, "20260926"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, OVERLAY_STUDY_UID),
        DataElement::new(tags::MODALITY, VR::CS, spec.modality),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, spec.series_instance_uid),
        DataElement::new(
            tags::FRAME_OF_REFERENCE_UID,
            VR::UI,
            spec.frame_of_reference_uid,
        ),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(spec.rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(spec.columns)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::from(pixel_bytes)),
    ]);
    for element in elements {
        obj.put(element);
    }
    obj.with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(spec.sop_class_uid)
            .media_storage_sop_instance_uid(spec.sop_instance_uid),
    )
    .expect("build semantic overlay fixture meta")
    .write_to_file(path)
    .expect("write semantic overlay fixture");
}

fn fixture_sequence(tag: Tag, items: Vec<InMemDicomObject>) -> DataElement<InMemDicomObject> {
    DataElement::new(tag, VR::SQ, DataSetSequence::from(items))
}

fn fixture_code(value: &str, scheme: &str, meaning: &str) -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::CODE_VALUE, VR::SH, value),
        DataElement::new(tags::CODING_SCHEME_DESIGNATOR, VR::SH, scheme),
        DataElement::new(tags::CODE_MEANING, VR::LO, meaning),
    ])
}

/// Two-frame SEG/source pairs. The genuine fractional object's first frame
/// contains only 0/1: only its second frame distinguishes it from a binary mask.
fn write_segmentation_fixtures(dir: &Path) {
    for (case, name, segmentation_type, samples) in [
        (
            0,
            "binary-valued-fractional",
            "FRACTIONAL",
            vec![0, 1, 1, 0, 1, 0, 0, 1],
        ),
        (
            1,
            "fractional",
            "FRACTIONAL",
            vec![0, 1, 1, 0, 0, 64, 128, 255],
        ),
        (2, "binary", "BINARY", vec![0, 1, 1, 0, 1, 0, 0, 1]),
    ] {
        let source_uid = format!("2.25.200030{}", case * 2);
        let seg_uid = format!("2.25.200030{}", case * 2 + 1);
        let for_uid = format!("2.25.200032{case}");
        let positions = || {
            (0..2)
                .map(|frame| {
                    InMemDicomObject::from_element_iter([fixture_sequence(
                        tags::PLANE_POSITION_SEQUENCE,
                        vec![InMemDicomObject::from_element_iter([DataElement::new(
                            tags::IMAGE_POSITION_PATIENT,
                            VR::DS,
                            format!("0\\0\\{frame}"),
                        )])],
                    )])
                })
                .collect::<Vec<_>>()
        };
        let geometry = || {
            vec![
                DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
                DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, AXIAL),
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "1\\1"),
                fixture_sequence(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, positions()),
            ]
        };
        write_native_u16(
            &dir.join(format!("golden-seg-{name}-source.dcm")),
            NativeU16Spec {
                sop_class_uid: uids::ENHANCED_CT_IMAGE_STORAGE,
                sop_instance_uid: &source_uid,
                modality: "CT",
                series_instance_uid: &source_uid,
                frame_of_reference_uid: &for_uid,
                rows: 2,
                columns: 2,
                samples: vec![100, 200, 300, 400, 500, 600, 700, 800],
            },
            geometry(),
        );
        let reference = |frame: Option<u32>| {
            let mut item = InMemDicomObject::from_element_iter([
                DataElement::new(
                    tags::REFERENCED_SOP_CLASS_UID,
                    VR::UI,
                    uids::ENHANCED_CT_IMAGE_STORAGE,
                ),
                DataElement::new(
                    tags::REFERENCED_SOP_INSTANCE_UID,
                    VR::UI,
                    source_uid.as_str(),
                ),
            ]);
            if let Some(frame) = frame {
                item.put(DataElement::new(
                    tags::REFERENCED_FRAME_NUMBER,
                    VR::IS,
                    frame.to_string(),
                ));
            }
            item
        };
        let mut frames = positions();
        for (frame, group) in frames.iter_mut().enumerate() {
            group.put(fixture_sequence(
                tags::SEGMENT_IDENTIFICATION_SEQUENCE,
                vec![InMemDicomObject::from_element_iter([DataElement::new(
                    tags::REFERENCED_SEGMENT_NUMBER,
                    VR::US,
                    PrimitiveValue::from(1_u16),
                )])],
            ));
            group.put(fixture_sequence(
                tags::DERIVATION_IMAGE_SEQUENCE,
                vec![InMemDicomObject::from_element_iter([fixture_sequence(
                    tags::SOURCE_IMAGE_SEQUENCE,
                    vec![reference(Some(frame as u32 + 1))],
                )])],
            ));
        }
        let bits = if segmentation_type == "BINARY" {
            1_u16
        } else {
            8
        };
        let pixels = if bits == 1 { vec![0x96, 0] } else { samples };
        let mut elements = geometry();
        elements.extend([
            DataElement::new(tags::SEGMENTATION_TYPE, VR::CS, segmentation_type),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(bits)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(bits)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(bits - 1)),
            fixture_sequence(
                tags::SEGMENT_SEQUENCE,
                vec![InMemDicomObject::from_element_iter([
                    DataElement::new(tags::SEGMENT_NUMBER, VR::US, PrimitiveValue::from(1_u16)),
                    DataElement::new(tags::SEGMENT_LABEL, VR::LO, "Fixture mask"),
                    DataElement::new(tags::SEGMENT_ALGORITHM_TYPE, VR::CS, "MANUAL"),
                ])],
            ),
            fixture_sequence(tags::SOURCE_IMAGE_SEQUENCE, vec![reference(None)]),
            fixture_sequence(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, frames),
            DataElement::new(tags::PIXEL_DATA, VR::OB, PrimitiveValue::U8(pixels.into())),
        ]);
        if bits == 8 {
            elements.extend([
                DataElement::new(tags::SEGMENTATION_FRACTIONAL_TYPE, VR::CS, "OCCUPANCY"),
                DataElement::new(
                    tags::MAXIMUM_FRACTIONAL_VALUE,
                    VR::US,
                    PrimitiveValue::from(255_u16),
                ),
            ]);
        }
        write_native_u16(
            &dir.join(format!("golden-seg-{name}.dcm")),
            NativeU16Spec {
                sop_class_uid: uids::SEGMENTATION_STORAGE,
                sop_instance_uid: &seg_uid,
                modality: "SEG",
                series_instance_uid: &seg_uid,
                frame_of_reference_uid: &for_uid,
                rows: 2,
                columns: 2,
                samples: vec![],
            },
            elements,
        );
    }
}

/// A 3-plane RT Dose grid (4x4 voxels of 4 mm at z = 0, 4, 8 mm) and three
/// 10x10 CT slices of 2 mm pixels at z = 0, 6, and 20 mm in its Frame of
/// Reference. The z = 6 slice lies halfway between the last two planes and
/// the z = 20 slice beyond the grid. Stored dose is
/// `1000 * plane + 100 * row + 10 * column`, scaled by 0.01 Gy.
///
/// An oblique grid holds the same samples, tilted about the x axis: its
/// rows advance along (0, 0.6, 0.8) from (0, 10, 1), so its planes (normal
/// (0, -0.8, 0.6)) cut through the axial slices. Being linear in each
/// index, its dose at a point inside the grid is exactly
/// `250 * normal + 25 * row_mm + 2.5 * column_mm` stored, in millimeters
/// from that origin along the normal, the row direction, and x.
fn write_rt_dose_overlay_fixtures(fixture_dir: &Path) {
    const FRAME_OF_REFERENCE: &str = "2.25.2000120";
    let dose_samples = (0..3_u16)
        .flat_map(|plane| {
            (0..4_u16).flat_map(move |row| {
                (0..4_u16).map(move |column| 1000 * plane + 100 * row + 10 * column)
            })
        })
        .collect::<Vec<_>>();
    for (name, sop_instance_uid, series_instance_uid, position, orientation) in [
        (
            "golden-rtdose-u16-grid.dcm",
            "2.25.2000101",
            "2.25.2000111",
            "0\\0\\0",
            AXIAL,
        ),
        (
            "golden-rtdose-oblique-u16-grid.dcm",
            "2.25.2000106",
            "2.25.2000113",
            "0\\10\\1",
            "1\\0\\0\\0\\0.6\\0.8",
        ),
    ] {
        write_native_u16(
            &fixture_dir.join(name),
            NativeU16Spec {
                sop_class_uid: uids::RT_DOSE_STORAGE,
                sop_instance_uid,
                modality: "RTDOSE",
                series_instance_uid,
                frame_of_reference_uid: FRAME_OF_REFERENCE,
                rows: 4,
                columns: 4,
                samples: dose_samples.clone(),
            },
            vec![
                DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "3"),
                DataElement::new(
                    tags::FRAME_INCREMENT_POINTER,
                    VR::AT,
                    PrimitiveValue::Tags(vec![tags::GRID_FRAME_OFFSET_VECTOR].into()),
                ),
                DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, position),
                DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, orientation),
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "4\\4"),
                DataElement::new(tags::DOSE_UNITS, VR::CS, "GY"),
                DataElement::new(tags::DOSE_TYPE, VR::CS, "PHYSICAL"),
                DataElement::new(tags::DOSE_SUMMATION_TYPE, VR::CS, "PLAN"),
                DataElement::new(tags::GRID_FRAME_OFFSET_VECTOR, VR::DS, "0\\4\\8"),
                DataElement::new(tags::DOSE_GRID_SCALING, VR::DS, "0.01"),
            ],
        );
    }
    for (slice, (sop_instance_uid, z)) in [
        ("2.25.2000102", 0),
        ("2.25.2000103", 6),
        ("2.25.2000104", 20),
    ]
    .into_iter()
    .enumerate()
    {
        write_native_u16(
            &fixture_dir.join(format!("golden-rtdose-ct-source-z{z}.dcm")),
            NativeU16Spec {
                sop_class_uid: uids::CT_IMAGE_STORAGE,
                sop_instance_uid,
                modality: "CT",
                series_instance_uid: "2.25.2000110",
                frame_of_reference_uid: FRAME_OF_REFERENCE,
                rows: 10,
                columns: 10,
                samples: (0..100).map(|index| 1000 + 10 * index).collect(),
            },
            vec![
                DataElement::new(tags::INSTANCE_NUMBER, VR::IS, (slice + 1).to_string()),
                DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, format!("0\\0\\{z}")),
                DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, AXIAL),
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "2\\2"),
                DataElement::new(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
                DataElement::new(tags::RESCALE_SLOPE, VR::DS, "1"),
            ],
        );
    }
}

/// A Real World Value Mapping instance (no pixel data) whose one Referenced
/// Image Real World Value Mapping item maps the stored values of the z = 0
/// RT Dose CT slice to Hounsfield units, `stored - 1024`, for every frame.
fn write_real_world_value_mapping_instance(path: &Path) {
    const SOP_INSTANCE_UID: &str = "2.25.2000105";
    let mapping = InMemDicomObject::from_element_iter([
        DataElement::new(tags::LUT_LABEL, VR::SH, "HU"),
        DataElement::new(
            tags::REAL_WORLD_VALUE_FIRST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_LAST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(u16::MAX),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_SLOPE,
            VR::FD,
            PrimitiveValue::from(1.0_f64),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_INTERCEPT,
            VR::FD,
            PrimitiveValue::from(-1024.0_f64),
        ),
        fixture_sequence(
            tags::MEASUREMENT_UNITS_CODE_SEQUENCE,
            vec![fixture_code("[hnsf'U]", "UCUM", "Hounsfield unit")],
        ),
    ]);
    let referenced_image = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::REFERENCED_SOP_CLASS_UID,
            VR::UI,
            uids::CT_IMAGE_STORAGE,
        ),
        DataElement::new(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, "2.25.2000102"),
    ]);
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::REAL_WORLD_VALUE_MAPPING_STORAGE,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, SOP_INSTANCE_UID),
        DataElement::new(tags::PATIENT_ID, VR::LO, OVERLAY_PATIENT_ID),
        DataElement::new(tags::STUDY_DATE, VR::DA, "20260926"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, OVERLAY_STUDY_UID),
        DataElement::new(tags::MODALITY, VR::CS, "RWV"),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, "2.25.2000112"),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, "1"),
        fixture_sequence(
            tags::REFERENCED_IMAGE_REAL_WORLD_VALUE_MAPPING_SEQUENCE,
            vec![InMemDicomObject::from_element_iter([
                fixture_sequence(tags::REFERENCED_IMAGE_SEQUENCE, vec![referenced_image]),
                fixture_sequence(tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE, vec![mapping]),
            ])],
        ),
    ]);
    obj.with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(uids::REAL_WORLD_VALUE_MAPPING_STORAGE)
            .media_storage_sop_instance_uid(SOP_INSTANCE_UID),
    )
    .expect("build RWVM fixture meta")
    .write_to_file(path)
    .expect("write RWVM fixture");
}

/// A 2-frame Parametric Map (4x4 pixels of 2 mm at z = 0 and 2 mm) with a
/// shared linear Real World Value Mapping `0.5 * stored - 10` in um2/s, and
/// two MR source slices on the same grid at z = 0 and 1 mm. Stored values
/// are `20 + 1000 * frame + 100 * row + 10 * column`.
fn write_parametric_map_overlay_fixtures(fixture_dir: &Path) {
    const FRAME_OF_REFERENCE: &str = "2.25.2000220";
    let sources = [("2.25.2000202", 0), ("2.25.2000203", 1)];
    let mapping = InMemDicomObject::from_element_iter([
        DataElement::new(tags::LUT_LABEL, VR::SH, "ADC"),
        DataElement::new(
            tags::REAL_WORLD_VALUE_FIRST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_LAST_VALUE_MAPPED,
            VR::US,
            PrimitiveValue::from(4095_u16),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_SLOPE,
            VR::FD,
            PrimitiveValue::from(0.5_f64),
        ),
        DataElement::new(
            tags::REAL_WORLD_VALUE_INTERCEPT,
            VR::FD,
            PrimitiveValue::from(-10.0_f64),
        ),
        fixture_sequence(
            tags::MEASUREMENT_UNITS_CODE_SEQUENCE,
            vec![fixture_code("um2/s", "UCUM", "um2/s")],
        ),
        fixture_sequence(
            tags::QUANTITY_DEFINITION_SEQUENCE,
            vec![InMemDicomObject::from_element_iter([fixture_sequence(
                tags::CONCEPT_CODE_SEQUENCE,
                vec![fixture_code(
                    "113041",
                    "DCM",
                    "Apparent Diffusion Coefficient",
                )],
            )])],
        ),
    ]);
    let shared = InMemDicomObject::from_element_iter([
        fixture_sequence(
            tags::PLANE_ORIENTATION_SEQUENCE,
            vec![InMemDicomObject::from_element_iter([DataElement::new(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                AXIAL,
            )])],
        ),
        fixture_sequence(
            tags::PIXEL_MEASURES_SEQUENCE,
            vec![InMemDicomObject::from_element_iter([
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "2\\2"),
                DataElement::new(tags::SLICE_THICKNESS, VR::DS, "2"),
            ])],
        ),
        fixture_sequence(tags::REAL_WORLD_VALUE_MAPPING_SEQUENCE, vec![mapping]),
    ]);
    let per_frame = [0, 2]
        .map(|z| {
            InMemDicomObject::from_element_iter([fixture_sequence(
                tags::PLANE_POSITION_SEQUENCE,
                vec![InMemDicomObject::from_element_iter([DataElement::new(
                    tags::IMAGE_POSITION_PATIENT,
                    VR::DS,
                    format!("0\\0\\{z}"),
                )])],
            )])
        })
        .to_vec();
    let source_references = sources
        .map(|(uid, _)| {
            InMemDicomObject::from_element_iter([
                DataElement::new(
                    tags::REFERENCED_SOP_CLASS_UID,
                    VR::UI,
                    uids::MR_IMAGE_STORAGE,
                ),
                DataElement::new(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, uid),
            ])
        })
        .to_vec();
    let map_samples = (0..2_u16)
        .flat_map(|frame| {
            (0..4_u16).flat_map(move |row| {
                (0..4_u16).map(move |column| 20 + 1000 * frame + 100 * row + 10 * column)
            })
        })
        .collect();
    write_native_u16(
        &fixture_dir.join("golden-parametric-map-u16-linear.dcm"),
        NativeU16Spec {
            sop_class_uid: uids::PARAMETRIC_MAP_STORAGE,
            sop_instance_uid: "2.25.2000201",
            modality: "MR",
            series_instance_uid: "2.25.2000211",
            frame_of_reference_uid: FRAME_OF_REFERENCE,
            rows: 4,
            columns: 4,
            samples: map_samples,
        },
        vec![
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
            fixture_sequence(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, vec![shared]),
            fixture_sequence(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, per_frame),
            fixture_sequence(tags::SOURCE_IMAGE_SEQUENCE, source_references),
        ],
    );
    for (slice, (sop_instance_uid, z)) in sources.into_iter().enumerate() {
        write_native_u16(
            &fixture_dir.join(format!("golden-parametric-map-mr-source-z{z}.dcm")),
            NativeU16Spec {
                sop_class_uid: uids::MR_IMAGE_STORAGE,
                sop_instance_uid,
                modality: "MR",
                series_instance_uid: "2.25.2000210",
                frame_of_reference_uid: FRAME_OF_REFERENCE,
                rows: 4,
                columns: 4,
                samples: (0..16).map(|index| 200 + 10 * index).collect(),
            },
            vec![
                DataElement::new(tags::INSTANCE_NUMBER, VR::IS, (slice + 1).to_string()),
                DataElement::new(tags::IMAGE_POSITION_PATIENT, VR::DS, format!("0\\0\\{z}")),
                DataElement::new(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, AXIAL),
                DataElement::new(tags::PIXEL_SPACING, VR::DS, "2\\2"),
            ],
        );
    }
}

fn write_uncompressed_multiframe(path: &Path) {
    let samples: Vec<u16> = vec![
        0, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1100, 1200, 1300, 1400, 1500, 50,
        150, 250, 350, 450, 550, 650, 750, 850, 950, 1050, 1150, 1250, 1350, 1450, 1550, 1500,
        1400, 1300, 1200, 1100, 1000, 900, 800, 700, 600, 500, 400, 300, 200, 100, 0,
    ];

    let mut pixel_bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        pixel_bytes.extend_from_slice(&sample.to_le_bytes());
    }

    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.2000001"),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from("GOLDEN-UNCOMP"),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(4_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(4_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from("MONOCHROME2"),
        ),
        DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, PrimitiveValue::from("3")),
        DataElement::new(tags::WINDOW_CENTER, VR::DS, PrimitiveValue::from("750")),
        DataElement::new(tags::WINDOW_WIDTH, VR::DS, PrimitiveValue::from("1500")),
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::from(pixel_bytes)),
    ]);
    obj.put(DataElement::new(
        tags::RESCALE_SLOPE,
        VR::DS,
        PrimitiveValue::from("1"),
    ));
    obj.put(DataElement::new(
        tags::RESCALE_INTERCEPT,
        VR::DS,
        PrimitiveValue::from("0"),
    ));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.2000001"),
        )
        .expect("build uncompressed fixture meta");

    file_object
        .write_to_file(path)
        .expect("write uncompressed golden fixture");
}

fn write_jpeg_single_frame(path: &Path) {
    write_jpeg_fixture(
        path,
        "2.25.2000002",
        "GOLDEN-JPEG",
        vec![Fragments::new(grayscale_jpeg_fragment_16x16(24), 0)],
    );
}

fn write_large_jpeg_single_frame(path: &Path) {
    let columns = 3328_u16;
    let rows = 2560_u16;
    let fragment = large_grayscale_jpeg_fragment(columns.into(), rows.into());

    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::DIGITAL_MAMMOGRAPHY_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.2000005"),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from("GOLDEN-JPEG-LARGE"),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("MG")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(columns)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from("MONOCHROME2"),
        ),
        DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, PrimitiveValue::from("1")),
        DataElement::new(tags::WINDOW_CENTER, VR::DS, PrimitiveValue::from("128")),
        DataElement::new(tags::WINDOW_WIDTH, VR::DS, PrimitiveValue::from("256")),
    ]);

    let pixel_sequence: PixelFragmentSequence<Vec<u8>> = vec![Fragments::new(fragment, 0)].into();
    obj.put(DataElement::new(tags::PIXEL_DATA, VR::OB, pixel_sequence));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(uids::JPEG_BASELINE8_BIT)
                .media_storage_sop_class_uid(
                    uids::DIGITAL_MAMMOGRAPHY_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION,
                )
                .media_storage_sop_instance_uid("2.25.2000005"),
        )
        .expect("build large JPEG fixture meta");

    file_object
        .write_to_file(path)
        .expect("write large JPEG golden fixture");
}

fn write_jpeg_multiframe_with_bot(path: &Path) {
    write_jpeg_fixture(
        path,
        "2.25.2000003",
        "GOLDEN-JPEG-MF",
        vec![
            Fragments::new(grayscale_jpeg_fragment_16x16(15), 0),
            Fragments::new(grayscale_jpeg_fragment_16x16(90), 0),
            Fragments::new(grayscale_jpeg_fragment_16x16(165), 0),
        ],
    );
}

fn write_jpeg_lossless_single_frame(path: &Path) {
    write_grayscale_encapsulated_fixture(
        path,
        GrayscaleEncapsulatedSpec {
            sop_instance_uid: "2.25.2000007",
            patient_id: "GOLDEN-JPEG-LOSSLESS",
            transfer_syntax_uid: "1.2.840.10008.1.2.4.70",
            rows: 4,
            columns: 4,
            bits_allocated: 16,
            default_window: Some(("750", "1500")),
        },
        vec![Fragments::new(jpeg_lossless_fragment_4x4_u16(), 0)],
    );
}

fn write_jpeg2000_lossless_single_frame(path: &Path) {
    write_grayscale_encapsulated_fixture(
        path,
        GrayscaleEncapsulatedSpec {
            sop_instance_uid: "2.25.2000008",
            patient_id: "GOLDEN-JPEG2000",
            transfer_syntax_uid: "1.2.840.10008.1.2.4.90",
            rows: 16,
            columns: 16,
            bits_allocated: 8,
            default_window: Some(("127.5", "255")),
        },
        vec![Fragments::new(jpeg2000_lossless_fragment_16x16_u8(), 0)],
    );
}

struct GrayscaleEncapsulatedSpec<'a> {
    sop_instance_uid: &'a str,
    patient_id: &'a str,
    transfer_syntax_uid: &'a str,
    rows: u16,
    columns: u16,
    bits_allocated: u16,
    default_window: Option<(&'a str, &'a str)>,
}

fn write_grayscale_encapsulated_fixture(
    path: &Path,
    spec: GrayscaleEncapsulatedSpec<'_>,
    frames: Vec<Fragments>,
) {
    let frame_count = frames.len().max(1);
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, spec.sop_instance_uid),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from(spec.patient_id),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(spec.rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(spec.columns)),
        DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(spec.bits_allocated),
        ),
        DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(spec.bits_allocated),
        ),
        DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(spec.bits_allocated - 1),
        ),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from("MONOCHROME2"),
        ),
        DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            PrimitiveValue::from(frame_count.to_string()),
        ),
        DataElement::new(tags::RESCALE_SLOPE, VR::DS, PrimitiveValue::from("1")),
        DataElement::new(tags::RESCALE_INTERCEPT, VR::DS, PrimitiveValue::from("0")),
    ]);

    if let Some((center, width)) = spec.default_window {
        obj.put(DataElement::new(
            tags::WINDOW_CENTER,
            VR::DS,
            PrimitiveValue::from(center),
        ));
        obj.put(DataElement::new(
            tags::WINDOW_WIDTH,
            VR::DS,
            PrimitiveValue::from(width),
        ));
    }

    let pixel_sequence: PixelFragmentSequence<Vec<u8>> = frames.into();
    obj.put(DataElement::new(tags::PIXEL_DATA, VR::OB, pixel_sequence));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(spec.transfer_syntax_uid)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                .media_storage_sop_instance_uid(spec.sop_instance_uid),
        )
        .expect("build compressed grayscale fixture meta");

    file_object
        .write_to_file(path)
        .expect("write compressed grayscale golden fixture");
}

/// Y, Cb, Cr samples of a 4x2 image: red, green / blue, muted orange. Each
/// horizontal pair shares its chroma, so the samples are also a valid 4:2:2
/// source upsampled to full resolution.
const YBR_4X2: [[u8; 3]; 8] = [
    [76, 85, 255],
    [76, 85, 255],
    [150, 44, 21],
    [150, 44, 21],
    [29, 255, 107],
    [29, 255, 107],
    [128, 100, 160],
    [128, 100, 160],
];

struct ColorFixtureSpec<'a> {
    sop_instance_uid: &'a str,
    patient_id: &'a str,
    transfer_syntax_uid: &'a str,
    photometric_interpretation: &'a str,
}

/// Writes a single-frame 4x2 8-bit three-sample Secondary Capture image.
fn write_color_fixture(path: &Path, spec: ColorFixtureSpec<'_>, fragment: Vec<u8>) {
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, spec.sop_instance_uid),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from(spec.patient_id),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("OT")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(4_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(3_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from(spec.photometric_interpretation),
        ),
        DataElement::new(
            tags::PLANAR_CONFIGURATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, PrimitiveValue::from("1")),
    ]);

    let pixel_sequence: PixelFragmentSequence<Vec<u8>> = vec![Fragments::new(fragment, 0)].into();
    obj.put(DataElement::new(tags::PIXEL_DATA, VR::OB, pixel_sequence));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(spec.transfer_syntax_uid)
                .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                .media_storage_sop_instance_uid(spec.sop_instance_uid),
        )
        .expect("build color fixture meta");

    file_object
        .write_to_file(path)
        .expect("write color golden fixture");
}

/// Writes one 8x8 mid-gray native DX image per display shutter shape. Every
/// stored sample is 128 under a 128/256 window, so a displayed pixel is 128
/// and a shuttered pixel takes the shutter's presentation value.
fn write_display_shutter_fixtures(fixture_dir: &Path) {
    let shutter_value = |value: u16| {
        DataElement::new(
            tags::SHUTTER_PRESENTATION_VALUE,
            VR::US,
            PrimitiveValue::from(value),
        )
    };
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-circular-u8.dcm"),
        "2.25.2000012",
        ShutterFixtureImage::GRAY,
        vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR"),
            // Row 4, column 5: off the diagonal so a row/column swap shows.
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "4\\5"),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "3"),
            shutter_value(0),
        ],
    );
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-polygonal-u8.dcm"),
        "2.25.2000013",
        ShutterFixtureImage::GRAY,
        vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "POLYGONAL"),
            DataElement::new(
                tags::VERTICES_OF_THE_POLYGONAL_SHUTTER,
                VR::IS,
                "1\\1\\1\\8\\6\\1",
            ),
            shutter_value(0),
        ],
    );
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-rectangular-circular-u8.dcm"),
        "2.25.2000014",
        ShutterFixtureImage::GRAY,
        vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "RECTANGULAR\\CIRCULAR"),
            DataElement::new(tags::SHUTTER_LEFT_VERTICAL_EDGE, VR::IS, "2"),
            DataElement::new(tags::SHUTTER_RIGHT_VERTICAL_EDGE, VR::IS, "8"),
            DataElement::new(tags::SHUTTER_UPPER_HORIZONTAL_EDGE, VR::IS, "1"),
            DataElement::new(tags::SHUTTER_LOWER_HORIZONTAL_EDGE, VR::IS, "5"),
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "4\\4"),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "3"),
            shutter_value(0xFFFF),
        ],
    );
    // Overlay 6000 covers the image and occludes rows 1-2 and column 8.
    // Overlay 6002 stays a visible one-pixel overlay at row 5, column 4.
    let mut bitmap =
        overlay_plane_elements(0x6000, [8, 8], [1, 1], &[0xFFFF, 0x8080, 0x8080, 0x8080]);
    bitmap.extend(overlay_plane_elements(0x6002, [1, 1], [5, 4], &[0x0001]));
    bitmap.extend([
        DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "BITMAP"),
        DataElement::new(
            tags::SHUTTER_OVERLAY_GROUP,
            VR::US,
            PrimitiveValue::from(0x6000_u16),
        ),
        shutter_value(0),
    ]);
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-bitmap-u8.dcm"),
        "2.25.2000015",
        ShutterFixtureImage::GRAY,
        bitmap,
    );
    // Color frames: the circle again, filled with the sRGB of the CIELab
    // color rather than the (black) gray value.
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-circular-cielab-rgb-u8.dcm"),
        "2.25.2000017",
        ShutterFixtureImage::Rgb {
            transfer_syntax_uid: uids::EXPLICIT_VR_LITTLE_ENDIAN,
        },
        vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR"),
            DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "4\\5"),
            DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "3"),
            shutter_value(0),
            // D50 CIELab of sRGB red as PCS-values.
            DataElement::new(
                tags::SHUTTER_PRESENTATION_COLOR_CIE_LAB_VALUE,
                VR::US,
                PrimitiveValue::U16(vec![35_579, 53_663, 50_858].into()),
            ),
        ],
    );
    // A compressed color path with only a gray value: white fill.
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-polygonal-rle-rgb-u8.dcm"),
        "2.25.2000018",
        ShutterFixtureImage::Rgb {
            transfer_syntax_uid: uids::RLE_LOSSLESS,
        },
        vec![
            DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "POLYGONAL"),
            DataElement::new(
                tags::VERTICES_OF_THE_POLYGONAL_SHUTTER,
                VR::IS,
                "1\\1\\1\\8\\6\\1",
            ),
            shutter_value(0xFFFF),
        ],
    );
    // Enhanced multi-frame shutters: the shared group's rectangle applies to
    // frame 1, whose per-frame item declares none; frames 2 and 3 carry the
    // circle and the triangle in their own Frame Display Shutter Sequence.
    let frame_display_shutter = |shutter: Vec<DataElement<InMemDicomObject>>| {
        InMemDicomObject::from_element_iter([DataElement::new(
            tags::FRAME_DISPLAY_SHUTTER_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![InMemDicomObject::from_element_iter(shutter)]),
        )])
    };
    write_display_shutter_fixture(
        &fixture_dir.join("golden-shutter-per-frame-u8.dcm"),
        "2.25.2000019",
        ShutterFixtureImage::Gray {
            sop_class_uid: uids::ENHANCED_XA_IMAGE_STORAGE,
            frames: 3,
        },
        vec![
            DataElement::new(
                tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![frame_display_shutter(vec![
                    DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "RECTANGULAR"),
                    DataElement::new(tags::SHUTTER_LEFT_VERTICAL_EDGE, VR::IS, "3"),
                    DataElement::new(tags::SHUTTER_RIGHT_VERTICAL_EDGE, VR::IS, "6"),
                    DataElement::new(tags::SHUTTER_UPPER_HORIZONTAL_EDGE, VR::IS, "2"),
                    DataElement::new(tags::SHUTTER_LOWER_HORIZONTAL_EDGE, VR::IS, "7"),
                    shutter_value(0),
                ])]),
            ),
            DataElement::new(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(vec![
                    InMemDicomObject::new_empty(),
                    frame_display_shutter(vec![
                        DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "CIRCULAR"),
                        DataElement::new(tags::CENTER_OF_CIRCULAR_SHUTTER, VR::IS, "4\\5"),
                        DataElement::new(tags::RADIUS_OF_CIRCULAR_SHUTTER, VR::IS, "3"),
                        shutter_value(0),
                    ]),
                    frame_display_shutter(vec![
                        DataElement::new(tags::SHUTTER_SHAPE, VR::CS, "POLYGONAL"),
                        DataElement::new(
                            tags::VERTICES_OF_THE_POLYGONAL_SHUTTER,
                            VR::IS,
                            "1\\1\\1\\8\\6\\1",
                        ),
                        shutter_value(0),
                    ]),
                ]),
            ),
        ],
    );
}

/// Pixels of an 8x8 display shutter fixture.
enum ShutterFixtureImage {
    /// Native MONOCHROME2 samples of 128 under a 128/256 window.
    Gray {
        sop_class_uid: &'static str,
        frames: u16,
    },
    /// Every pixel RGB `SHUTTER_FIXTURE_RGB`, native or RLE Lossless.
    Rgb { transfer_syntax_uid: &'static str },
}

impl ShutterFixtureImage {
    const GRAY: Self = Self::Gray {
        sop_class_uid: uids::DIGITAL_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION,
        frames: 1,
    };
}

const SHUTTER_FIXTURE_RGB: [u8; 3] = [40, 80, 160];

fn overlay_plane_elements(
    group: u16,
    [rows, columns]: [u16; 2],
    [origin_row, origin_column]: [i16; 2],
    data: &[u16],
) -> Vec<DataElement<InMemDicomObject>> {
    vec![
        DataElement::new(Tag(group, 0x0010), VR::US, PrimitiveValue::from(rows)),
        DataElement::new(Tag(group, 0x0011), VR::US, PrimitiveValue::from(columns)),
        DataElement::new(Tag(group, 0x0040), VR::CS, "G"),
        DataElement::new(
            Tag(group, 0x0050),
            VR::SS,
            PrimitiveValue::I16(vec![origin_row, origin_column].into()),
        ),
        DataElement::new(Tag(group, 0x0100), VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(Tag(group, 0x0102), VR::US, PrimitiveValue::from(0_u16)),
        DataElement::new(
            Tag(group, 0x3000),
            VR::OW,
            PrimitiveValue::U16(data.to_vec().into()),
        ),
    ]
}

fn write_display_shutter_fixture(
    path: &Path,
    sop_instance_uid: &str,
    image: ShutterFixtureImage,
    shutter: Vec<DataElement<InMemDicomObject>>,
) {
    let (sop_class_uid, transfer_syntax_uid) = match image {
        ShutterFixtureImage::Gray { sop_class_uid, .. } => {
            (sop_class_uid, uids::EXPLICIT_VR_LITTLE_ENDIAN)
        }
        ShutterFixtureImage::Rgb {
            transfer_syntax_uid,
        } => (uids::SECONDARY_CAPTURE_IMAGE_STORAGE, transfer_syntax_uid),
    };
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, sop_class_uid),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, sop_instance_uid),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from("GOLDEN-SHUTTER"),
        ),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260926")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
    ]);
    match image {
        ShutterFixtureImage::Gray { frames, .. } => {
            for element in [
                DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("DX")),
                DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
                DataElement::new(
                    tags::PHOTOMETRIC_INTERPRETATION,
                    VR::CS,
                    PrimitiveValue::from("MONOCHROME2"),
                ),
                DataElement::new(tags::WINDOW_CENTER, VR::DS, PrimitiveValue::from("128")),
                DataElement::new(tags::WINDOW_WIDTH, VR::DS, PrimitiveValue::from("256")),
                DataElement::new(
                    tags::PIXEL_DATA,
                    VR::OB,
                    PrimitiveValue::from(vec![128_u8; 64 * usize::from(frames)]),
                ),
            ] {
                obj.put(element);
            }
            if frames > 1 {
                obj.put(DataElement::new(
                    tags::NUMBER_OF_FRAMES,
                    VR::IS,
                    PrimitiveValue::from(frames.to_string()),
                ));
            }
        }
        ShutterFixtureImage::Rgb {
            transfer_syntax_uid,
        } => {
            for element in [
                DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("OT")),
                DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(3_u16)),
                DataElement::new(
                    tags::PHOTOMETRIC_INTERPRETATION,
                    VR::CS,
                    PrimitiveValue::from("RGB"),
                ),
                DataElement::new(
                    tags::PLANAR_CONFIGURATION,
                    VR::US,
                    PrimitiveValue::from(0_u16),
                ),
            ] {
                obj.put(element);
            }
            if transfer_syntax_uid == uids::RLE_LOSSLESS {
                let planes = SHUTTER_FIXTURE_RGB.map(|sample| vec![sample; 64]);
                let pixel_sequence: PixelFragmentSequence<Vec<u8>> =
                    vec![Fragments::new(rle_literal_fragment(&planes), 0)].into();
                obj.put(DataElement::new(tags::PIXEL_DATA, VR::OB, pixel_sequence));
            } else {
                obj.put(DataElement::new(
                    tags::PIXEL_DATA,
                    VR::OB,
                    PrimitiveValue::from(SHUTTER_FIXTURE_RGB.repeat(64)),
                ));
            }
        }
    }
    for element in shutter {
        obj.put(element);
    }

    obj.with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(transfer_syntax_uid)
            .media_storage_sop_class_uid(sop_class_uid)
            .media_storage_sop_instance_uid(sop_instance_uid),
    )
    .expect("build display shutter fixture meta")
    .write_to_file(path)
    .expect("write display shutter golden fixture");
}

fn rle_ybr_fragment_4x2() -> Vec<u8> {
    let planes = (0..3)
        .map(|sample| YBR_4X2.iter().map(|pixel| pixel[sample]).collect())
        .collect::<Vec<Vec<u8>>>();
    rle_literal_fragment(&planes)
}

/// PS3.5 Annex G frame: a 64-byte header, then one PackBits literal-run
/// segment per sample plane, each at most 128 bytes.
fn rle_literal_fragment(planes: &[Vec<u8>]) -> Vec<u8> {
    let mut fragment = vec![0_u8; 64];
    fragment[0..4].copy_from_slice(&(planes.len() as u32).to_le_bytes());
    for (index, plane) in planes.iter().enumerate() {
        let offset = fragment.len() as u32;
        fragment[4 + index * 4..8 + index * 4].copy_from_slice(&offset.to_le_bytes());
        fragment.push((plane.len() - 1) as u8);
        fragment.extend_from_slice(plane);
    }
    fragment
}

fn write_jpeg_fixture(
    path: &Path,
    sop_instance_uid: &str,
    patient_id: &str,
    frames: Vec<Fragments>,
) {
    let frame_count = frames.len().max(1);
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::DIGITAL_MAMMOGRAPHY_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, sop_instance_uid),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from(patient_id)),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("MG")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from("MONOCHROME2"),
        ),
        DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            PrimitiveValue::from(frame_count.to_string()),
        ),
    ]);

    let pixel_sequence: PixelFragmentSequence<Vec<u8>> = frames.into();
    obj.put(DataElement::new(tags::PIXEL_DATA, VR::OB, pixel_sequence));

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(uids::JPEG_BASELINE8_BIT)
                .media_storage_sop_class_uid(
                    uids::DIGITAL_MAMMOGRAPHY_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION,
                )
                .media_storage_sop_instance_uid(sop_instance_uid),
        )
        .expect("build JPEG fixture meta");

    file_object
        .write_to_file(path)
        .expect("write JPEG golden fixture");
}

fn write_sr_without_pixels(path: &Path) {
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::BASIC_TEXT_SR_STORAGE),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.2000004"),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("GOLDEN-SR")),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("SR")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(
            tags::SERIES_DESCRIPTION,
            VR::LO,
            PrimitiveValue::from("No pixel fixture"),
        ),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, PrimitiveValue::from("1")),
    ]);

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::BASIC_TEXT_SR_STORAGE)
                .media_storage_sop_instance_uid("2.25.2000004"),
        )
        .expect("build SR fixture meta");

    file_object
        .write_to_file(path)
        .expect("write SR golden fixture");
}

fn write_image_without_pixels(path: &Path) {
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.2000006"),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from("GOLDEN-NO-PIXELS"),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
        DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::from("20260608")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from("MONOCHROME2"),
        ),
        DataElement::new(
            tags::SERIES_DESCRIPTION,
            VR::LO,
            PrimitiveValue::from("Image metadata without pixel data"),
        ),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, PrimitiveValue::from("1")),
    ]);

    let file_object = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.2000006"),
        )
        .expect("build no-pixels image fixture meta");

    file_object
        .write_to_file(path)
        .expect("write no-pixels image golden fixture");
}

fn grayscale_jpeg_fragment_16x16(seed: u8) -> Vec<u8> {
    let image = GrayImage::from_fn(16, 16, |x, y| {
        let value = seed
            .wrapping_add((x as u8).wrapping_mul(7))
            .wrapping_add((y as u8).wrapping_mul(11));
        Luma([value])
    });
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90);
    encoder
        .encode_image(&image)
        .expect("encode grayscale jpeg fixture");
    encoded
}

fn large_grayscale_jpeg_fragment(width: u32, height: u32) -> Vec<u8> {
    let center_x = width as i32 / 2;
    let center_y = height as i32 / 2;
    let image = GrayImage::from_fn(width, height, |x, y| {
        let x = x as i32;
        let y = y as i32;
        let dx = x - center_x;
        let dy = y - center_y;
        let distance_sq = dx * dx + dy * dy;
        let value = if dx.abs() < 8 || dy.abs() < 8 {
            210
        } else if distance_sq < 180_i32.pow(2) {
            170
        } else if x < center_x && y < center_y {
            72
        } else if x >= center_x && y < center_y {
            96
        } else if x < center_x {
            120
        } else {
            144
        };
        Luma([value])
    });
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 80);
    encoder
        .encode_image(&image)
        .expect("encode large grayscale jpeg fixture");
    encoded
}

fn jpeg_lossless_fragment_4x4_u16() -> Vec<u8> {
    // Lossless JPEG process 14, selection value 1: 0, 100, ... 1500.
    decode_hex(concat!(
        "ffd8ffe000104a46494600010100000100010000ffc3000b100004000401011100",
        "ffc400160001010100000000000000000000000000070910ffda000801010001",
        "0000cc8c8c9641919192c8323232590646464fffd9"
    ))
}

fn jpeg_lossless_ybr_fragment_4x2() -> Vec<u8> {
    // Lossless JPEG process 14, selection value 1, three 8-bit components
    // holding YBR_4X2 unchanged: one DC table codes SSSS 0-8 with 4 bits.
    decode_hex(concat!(
        "ffd8ffc30011080002000403011100021100031100ffc4001c00000000090000",
        "00000000000000000000000102030405060708ffda000c030100020003000100",
        "0062d947fe000f29968150006422aa1ac001f1c3236a001fffd9"
    ))
}

fn jpeg_xl_lossless_ybr_fragment_4x2() -> Vec<u8> {
    // Lossless JPEG XL codestream (zune-jpegxl 0.4, modular, no XYB) of
    // YBR_4X2 stored as three plain 8-bit channels.
    decode_hex(concat!(
        "ff0a080006804808020100dc024b189b9c71840338800338204ac03905010020",
        "4480081001224084fff7eff9efa131e79c6bed736f922409015555555555d5ff",
        "ffff73efebeeeeee86fff7eff9efa131e79c6bed736f922409015555555555d5",
        "ffffff73efebeeeeee86fff7eff9efa131e79c6bed736f922409015555555555",
        "d5ffffff73efebeeeeee86fff7eff9efa131e79c6bed736f9224090155555555",
        "55d5ffffff73efebeeeeee3e00979e93731096f128e389864bc92388ebc37178",
        "5ae0d10c"
    ))
}

fn jpeg_xl_lossless_rct_fragment_4x2() -> Vec<u8> {
    // Lossless JPEG XL codestream (zune-jpegxl 0.4, modular, no XYB) of the
    // rounded RGB that YBR_4X2 converts to under YBR_FULL. The encoder
    // codes three channels through the YCoCg reversible color transform, the
    // RCT that PhotometricInterpretation YBR_RCT declares; decoders invert it.
    decode_hex(concat!(
        "ff0a080006804808020100d4024b189b9c71840338800338204ac03905010020",
        "4480081001224084fff7eff9efa131e79c6bed736f922409015555555555d5ff",
        "ffff73efebeeeeee86fff7eff9efa131e79c6bed736f922409015555555555d5",
        "ffffff73efebeeeeee86fff7eff9efa131e79c6bed736f922409015555555555",
        "d5ffffff73efebeeeeee86fff7eff9efa131e79c6bed736f9224090155555555",
        "55d5ffffff73efebeeeeee3e00c7e702002d0f7e1efd3ceff3d4c5d5e7c90f78",
        "4600"
    ))
}

fn jpeg2000_lossless_fragment_16x16_u8() -> Vec<u8> {
    // Lossless J2K codestream: sixteen rows with values 0, 17, ... 255.
    decode_hex(concat!(
        "ff4fff5100290000000000100000001000000000000000000000001000000010",
        "00000000000000000001070101ff52000c00000001000104040001ff5c000740",
        "40484850ff640025000143726561746564206279204f70656e4a504547207665",
        "7273696f6e20322e352e34ff90000a00000000004b0001ff93df8178128e2ccf",
        "87f90eb3644a78e066e3b11b89613bda74543e1be0ca9c8c9252739ce666d0d",
        "f1f932000000000000ac01fa0f9c3803da6345f370603ffd9"
    ))
}

fn decode_hex(encoded: &str) -> Vec<u8> {
    assert_eq!(encoded.len() % 2, 0, "hex fixture must contain byte pairs");
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]))
        .collect()
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => panic!("invalid hexadecimal fixture byte"),
    }
}

// Presentation-state fixtures: two target images with painted shapes, and
// Grayscale Softcopy Presentation States whose graphic annotations outline
// those shapes exactly, so a misplaced annotation is visible.
const GSPS_PATIENT_ID: &str = "GOLDEN-GSPS";
const GSPS_STUDY_UID: &str = "2.25.2000200";
const GSPS_STATE_SERIES_UID: &str = "2.25.2000203";
/// Non-square, so a column/row swap shows.
const GSPS_COLUMNS: usize = 240;
const GSPS_ROWS: usize = 160;
const GSPS_BACKGROUND: u8 = 40;
const GSPS_TARGET: u8 = 200;

/// A target image a presentation state references.
#[derive(Clone, Copy)]
struct GspsImage {
    sop_class_uid: &'static str,
    sop_instance_uid: &'static str,
    series_instance_uid: &'static str,
    modality: &'static str,
    frames: u32,
}

const GSPS_SINGLE: GspsImage = GspsImage {
    sop_class_uid: uids::DIGITAL_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION,
    sop_instance_uid: "2.25.2000210",
    series_instance_uid: "2.25.2000201",
    modality: "DX",
    frames: 1,
};
const GSPS_MULTIFRAME: GspsImage = GspsImage {
    sop_class_uid: uids::X_RAY_ANGIOGRAPHIC_IMAGE_STORAGE,
    sop_instance_uid: "2.25.2000211",
    series_instance_uid: "2.25.2000202",
    modality: "XA",
    frames: 3,
};

// The painted shapes, in PS3.3 C.10.5 PIXEL coordinates (column\row, origin
// at the top-left corner of the top-left pixel). Only +, -, *, / are used on
// them so the painted pixels are identical on every platform.
const GSPS_ELLIPSE: [f32; 8] = [20.0, 40.0, 80.0, 40.0, 50.0, 22.0, 50.0, 58.0];
/// Major axis along (4, 3)/5, semi-axes 30 and 15, centre (130, 45).
const GSPS_ROTATED_ELLIPSE: [f32; 8] = [106.0, 27.0, 154.0, 63.0, 139.0, 33.0, 121.0, 57.0];
/// Centre, then a point on the circumference: radius 22.
const GSPS_DISC: [f32; 4] = [200.0, 40.0, 222.0, 40.0];
const GSPS_POLYGON: [f32; 12] = [
    20.0, 90.0, 70.0, 85.0, 80.0, 130.0, 40.0, 145.0, 15.0, 120.0, 20.0, 90.0,
];
const GSPS_ZIGZAG: [f32; 8] = [100.0, 90.0, 120.0, 130.0, 140.0, 90.0, 160.0, 130.0];
/// A closed curve's control points; each is painted as a dot the curve must cross.
const GSPS_CURVE: [f32; 14] = [
    185.0, 95.0, 215.0, 90.0, 228.0, 115.0, 210.0, 140.0, 183.0, 135.0, 175.0, 112.0, 185.0, 95.0,
];
/// The centre of pixel (column 120, row 150), marked with a plus.
const GSPS_POINT: [f32; 2] = [120.5, 150.5];
const GSPS_FILLED_DISC: [f32; 4] = [232.0, 150.0, 237.0, 150.0];
/// The outline of the single bright pixel at column 232, row 8.
const GSPS_PIXEL_BOX: [f32; 10] = [232.0, 8.0, 233.0, 8.0, 233.0, 9.0, 232.0, 9.0, 232.0, 8.0];
/// The image border: 0\0 to Columns\Rows.
const GSPS_BORDER: [f32; 10] = [0.0, 0.0, 240.0, 0.0, 240.0, 160.0, 0.0, 160.0, 0.0, 0.0];
/// The disc on frame 1, 2, and 3 of the multi-frame image: radius 25.
const GSPS_FRAME_DISCS: [[f32; 4]; 3] = [
    [60.0, 100.0, 85.0, 100.0],
    [120.0, 100.0, 145.0, 100.0],
    [180.0, 100.0, 205.0, 100.0],
];

fn write_presentation_state_fixtures(fixture_dir: &Path) {
    write_gsps_image(
        &fixture_dir.join("golden-gsps-target-u8.dcm"),
        GSPS_SINGLE,
        gsps_single_frame_pixels(),
    );
    write_gsps_image(
        &fixture_dir.join("golden-gsps-target-multiframe-u8.dcm"),
        GSPS_MULTIFRAME,
        (0..3).flat_map(gsps_multiframe_pixels).collect(),
    );

    let shapes = "SHAPES";
    let labels = "LABELS";
    let marks = "MARKS";
    let layers = vec![
        // Yellow, as CIELab L* 90, a* -5, b* 85.
        gsps_layer(
            shapes,
            1,
            "Outlines of the painted shapes",
            GspsColor::CieLab([58982, 31611, 54741]),
        ),
        gsps_layer(labels, 2, "Text labels", GspsColor::Gray(0xFFFF)),
        // No recommended value: the viewer's fallback color.
        gsps_layer(
            marks,
            3,
            "Points, the pixel box and the border",
            GspsColor::None,
        ),
    ];
    let single = [gsps_reference(GSPS_SINGLE, &[])];

    // One annotation item per shape, each naming its image.
    write_gsps_state(
        &fixture_dir.join("golden-gsps-conforming.dcm"),
        GspsState {
            sop_instance_uid: "2.25.2000220",
            instance_number: "1",
            label: "CONFORMING",
            description: "Every graphic type outlining its painted shape",
            images: &[GSPS_SINGLE],
            layers: layers.clone(),
            annotations: vec![
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("ELLIPSE", &GSPS_ELLIPSE, Some(false))],
                    vec![],
                ),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("ELLIPSE", &GSPS_ROTATED_ELLIPSE, Some(false))],
                    vec![],
                ),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("CIRCLE", &GSPS_DISC, Some(false))],
                    vec![],
                ),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("POLYLINE", &GSPS_POLYGON, Some(false))],
                    vec![],
                ),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("POLYLINE", &GSPS_ZIGZAG, None)],
                    vec![],
                ),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("INTERPOLATED", &GSPS_CURVE, Some(false))],
                    vec![],
                ),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("CIRCLE", &GSPS_FILLED_DISC, Some(true))],
                    vec![],
                ),
                gsps_annotation(
                    marks,
                    &single,
                    vec![
                        gsps_graphic("POINT", &GSPS_POINT, None),
                        gsps_graphic("POLYLINE", &GSPS_PIXEL_BOX, Some(false)),
                        gsps_graphic("POLYLINE", &GSPS_BORDER, Some(false)),
                    ],
                    vec![],
                ),
                gsps_annotation(
                    labels,
                    &single,
                    vec![],
                    vec![
                        gsps_boxed_text("Rotated ellipse", [100.0, 66.0], [165.0, 76.0], None),
                        // Boxed below the disc, with a visible anchor on its edge.
                        gsps_boxed_text("Disc", [185.0, 66.0], [215.0, 76.0], Some([200.0, 62.0])),
                        gsps_anchored_text("Point\r\n120.5, 150.5", [120.5, 150.5]),
                    ],
                ),
            ],
        },
    );

    // The same image again, as a second state to switch to: bounding boxes.
    write_gsps_state(
        &fixture_dir.join("golden-gsps-second-state.dcm"),
        GspsState {
            sop_instance_uid: "2.25.2000221",
            instance_number: "2",
            label: "BOXES",
            description: "Bounding boxes of the two ellipses and the disc",
            images: &[GSPS_SINGLE],
            layers: layers.clone(),
            annotations: [
                [20.0, 22.0, 80.0, 58.0],
                // The rotated ellipse's extent is +-sqrt(657) by +-sqrt(468).
                [104.368, 23.367, 155.632, 66.633],
                [178.0, 18.0, 222.0, 62.0],
            ]
            .into_iter()
            .map(|[x0, y0, x1, y1]: [f32; 4]| {
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic(
                        "POLYLINE",
                        &[x0, y0, x1, y0, x1, y1, x0, y1, x0, y0],
                        Some(false),
                    )],
                    vec![],
                )
            })
            .collect(),
        },
    );

    // Items scoped to one frame each, and one for every frame.
    let mut frame_annotations = (0..3)
        .map(|frame| {
            let [x, y, ..] = GSPS_FRAME_DISCS[frame];
            gsps_annotation(
                shapes,
                &[gsps_reference(GSPS_MULTIFRAME, &[frame as u32 + 1])],
                vec![gsps_graphic(
                    "CIRCLE",
                    &GSPS_FRAME_DISCS[frame],
                    Some(false),
                )],
                vec![gsps_boxed_text(
                    &format!("Frame {} only", frame + 1),
                    [x - 25.0, y + 28.0],
                    [x + 25.0, y + 38.0],
                    None,
                )],
            )
        })
        .collect::<Vec<_>>();
    frame_annotations.push(gsps_annotation(
        marks,
        &[gsps_reference(GSPS_MULTIFRAME, &[])],
        vec![gsps_graphic("POLYLINE", &GSPS_BORDER, Some(false))],
        vec![gsps_boxed_text(
            "All frames",
            [90.0, 10.0],
            [150.0, 20.0],
            None,
        )],
    ));
    write_gsps_state(
        &fixture_dir.join("golden-gsps-frames.dcm"),
        GspsState {
            sop_instance_uid: "2.25.2000222",
            instance_number: "3",
            label: "FRAMES",
            description: "One item per frame and one for all frames",
            images: &[GSPS_MULTIFRAME],
            layers: layers.clone(),
            annotations: frame_annotations,
        },
    );

    // An item without a Referenced Image Sequence applies to every image and
    // frame of the Referenced Series Sequence.
    write_gsps_state(
        &fixture_dir.join("golden-gsps-unscoped.dcm"),
        GspsState {
            sop_instance_uid: "2.25.2000223",
            instance_number: "4",
            label: "UNSCOPED",
            description: "One item with no Referenced Image Sequence",
            images: &[GSPS_SINGLE, GSPS_MULTIFRAME],
            layers: layers.clone(),
            annotations: vec![gsps_annotation(
                marks,
                &[],
                vec![
                    gsps_graphic("POLYLINE", &GSPS_BORDER, Some(false)),
                    gsps_graphic("POLYLINE", &[0.0, 0.0, 240.0, 160.0], None),
                ],
                vec![gsps_boxed_text(
                    "Every referenced image",
                    [60.0, 148.0],
                    [180.0, 158.0],
                    None,
                )],
            )],
        },
    );

    // DISPLAY-unit objects beside one PIXEL-unit circle.
    let mut display_ellipse = gsps_graphic(
        "ELLIPSE",
        &[0.25, 0.5, 0.75, 0.5, 0.5, 0.25, 0.5, 0.75],
        Some(false),
    );
    display_ellipse.put(DataElement::new(
        tags::GRAPHIC_ANNOTATION_UNITS,
        VR::CS,
        "DISPLAY",
    ));
    let mut display_text = gsps_boxed_text("Display units", [0.1, 0.9], [0.5, 0.95], None);
    display_text.put(DataElement::new(
        tags::BOUNDING_BOX_ANNOTATION_UNITS,
        VR::CS,
        "DISPLAY",
    ));
    write_gsps_state(
        &fixture_dir.join("golden-gsps-display-units.dcm"),
        GspsState {
            sop_instance_uid: "2.25.2000224",
            instance_number: "5",
            label: "DISPLAY",
            description: "DISPLAY-unit ellipse and text, PIXEL-unit circle",
            images: &[GSPS_SINGLE],
            layers,
            annotations: vec![
                gsps_annotation(shapes, &single, vec![display_ellipse], vec![display_text]),
                gsps_annotation(
                    shapes,
                    &single,
                    vec![gsps_graphic("CIRCLE", &GSPS_DISC, Some(false))],
                    vec![],
                ),
            ],
        },
    );
}

/// Paints the pixels whose centres satisfy `inside`.
fn gsps_paint(pixels: &mut [u8], inside: impl Fn(f64, f64) -> bool) {
    for row in 0..GSPS_ROWS {
        for column in 0..GSPS_COLUMNS {
            if inside(column as f64 + 0.5, row as f64 + 0.5) {
                pixels[row * GSPS_COLUMNS + column] = GSPS_TARGET;
            }
        }
    }
}

/// Whether a point is inside the ellipse given as PS3.3 C.10.5 ELLIPSE data.
fn gsps_in_ellipse(data: &[f32; 8], x: f64, y: f64) -> bool {
    let [ax, ay, bx, by, cx, cy, dx, dy] = data.map(f64::from);
    let (centre_x, centre_y) = ((ax + bx) / 2.0, (ay + by) / 2.0);
    let (major_x, major_y) = ((bx - ax) / 2.0, (by - ay) / 2.0);
    let (minor_x, minor_y) = ((dx - cx) / 2.0, (dy - cy) / 2.0);
    let major = major_x * major_x + major_y * major_y;
    let minor = minor_x * minor_x + minor_y * minor_y;
    let u = ((x - centre_x) * major_x + (y - centre_y) * major_y) / major;
    let v = ((x - centre_x) * minor_x + (y - centre_y) * minor_y) / minor;
    u * u + v * v <= 1.0
}

fn gsps_in_circle(data: &[f32; 4], x: f64, y: f64) -> bool {
    let [cx, cy, px, py] = data.map(f64::from);
    (x - cx) * (x - cx) + (y - cy) * (y - cy) <= (px - cx) * (px - cx) + (py - cy) * (py - cy)
}

fn gsps_points(data: &[f32]) -> Vec<(f64, f64)> {
    data.chunks_exact(2)
        .map(|point| (f64::from(point[0]), f64::from(point[1])))
        .collect()
}

/// Even-odd test against a closed polyline.
fn gsps_in_polygon(data: &[f32], x: f64, y: f64) -> bool {
    let points = gsps_points(data);
    let mut inside = false;
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if (y0 > y) != (y1 > y) && x < x0 + (y - y0) * (x1 - x0) / (y1 - y0) {
            inside = !inside;
        }
    }
    inside
}

/// Whether a point is within `reach` of a polyline.
fn gsps_near_polyline(data: &[f32], reach: f64, x: f64, y: f64) -> bool {
    gsps_points(data).windows(2).any(|pair| {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let along = (((x - x0) * dx + (y - y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (nx, ny) = (x - (x0 + along * dx), y - (y0 + along * dy));
        nx * nx + ny * ny <= reach * reach
    })
}

fn gsps_single_frame_pixels() -> Vec<u8> {
    let mut pixels = vec![GSPS_BACKGROUND; GSPS_COLUMNS * GSPS_ROWS];
    gsps_paint(&mut pixels, |x, y| {
        gsps_in_ellipse(&GSPS_ELLIPSE, x, y)
            || gsps_in_ellipse(&GSPS_ROTATED_ELLIPSE, x, y)
            || gsps_in_circle(&GSPS_DISC, x, y)
            || gsps_in_circle(&GSPS_FILLED_DISC, x, y)
            || gsps_in_polygon(&GSPS_POLYGON, x, y)
            || gsps_near_polyline(&GSPS_ZIGZAG, 0.75, x, y)
            // A 3x3 dot on each control point of the curve.
            || gsps_points(&GSPS_CURVE)
                .iter()
                .any(|(px, py)| (x - px).abs() <= 1.5 && (y - py).abs() <= 1.5)
            // A plus centred on the marked pixel.
            || ((x - 120.5).abs() < 0.5 && (y - 150.5).abs() <= 3.0)
            || ((y - 150.5).abs() < 0.5 && (x - 120.5).abs() <= 3.0)
            || (x == 232.5 && y == 8.5)
    });
    pixels
}

/// Frame `frame` (zero-based): its number as a block digit, and its disc.
fn gsps_multiframe_pixels(frame: usize) -> Vec<u8> {
    const DIGITS: [[u8; 7]; 3] = [
        [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        [
            0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111,
        ],
        [
            0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110,
        ],
    ];
    let mut pixels = vec![GSPS_BACKGROUND; GSPS_COLUMNS * GSPS_ROWS];
    gsps_paint(&mut pixels, |x, y| {
        // 5x7 cells of 6 pixels from (10, 10).
        let (cell_x, cell_y) = (((x - 10.0) / 6.0).floor(), ((y - 10.0) / 6.0).floor());
        let digit = (0.0..5.0).contains(&cell_x)
            && (0.0..7.0).contains(&cell_y)
            && DIGITS[frame][cell_y as usize] >> (4 - cell_x as usize) & 1 == 1;
        digit || gsps_in_circle(&GSPS_FRAME_DISCS[frame], x, y)
    });
    pixels
}

fn write_gsps_image(path: &Path, image: GspsImage, pixels: Vec<u8>) {
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, image.sop_class_uid),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, image.sop_instance_uid),
        DataElement::new(tags::PATIENT_ID, VR::LO, GSPS_PATIENT_ID),
        DataElement::new(tags::STUDY_DATE, VR::DA, "20260930"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, GSPS_STUDY_UID),
        DataElement::new(tags::MODALITY, VR::CS, image.modality),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, image.series_instance_uid),
        DataElement::new(
            tags::SERIES_DESCRIPTION,
            VR::LO,
            "Presentation state target",
        ),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, "1"),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(GSPS_ROWS as u16)),
        DataElement::new(
            tags::COLUMNS,
            VR::US,
            PrimitiveValue::from(GSPS_COLUMNS as u16),
        ),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::WINDOW_CENTER, VR::DS, "128"),
        DataElement::new(tags::WINDOW_WIDTH, VR::DS, "256"),
        DataElement::new(tags::PIXEL_DATA, VR::OB, PrimitiveValue::from(pixels)),
    ]);
    if image.frames > 1 {
        obj.put(DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            image.frames.to_string(),
        ));
    }
    obj.with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(image.sop_class_uid)
            .media_storage_sop_instance_uid(image.sop_instance_uid),
    )
    .expect("build presentation state target meta")
    .write_to_file(path)
    .expect("write presentation state target fixture");
}

#[derive(Clone, Copy)]
enum GspsColor {
    CieLab([u16; 3]),
    Gray(u16),
    None,
}

struct GspsState<'a> {
    sop_instance_uid: &'a str,
    instance_number: &'a str,
    label: &'a str,
    description: &'a str,
    /// The images of the Referenced Series Sequence, every frame of each.
    images: &'a [GspsImage],
    layers: Vec<InMemDicomObject>,
    annotations: Vec<InMemDicomObject>,
}

fn gsps_layer(name: &str, order: i32, description: &str, color: GspsColor) -> InMemDicomObject {
    let mut layer = InMemDicomObject::from_element_iter([
        DataElement::new(tags::GRAPHIC_LAYER, VR::CS, name),
        DataElement::new(tags::GRAPHIC_LAYER_ORDER, VR::IS, order.to_string()),
        DataElement::new(tags::GRAPHIC_LAYER_DESCRIPTION, VR::LO, description),
    ]);
    match color {
        GspsColor::CieLab(lab) => layer.put(DataElement::new(
            tags::GRAPHIC_LAYER_RECOMMENDED_DISPLAY_CIE_LAB_VALUE,
            VR::US,
            PrimitiveValue::U16(lab.to_vec().into()),
        )),
        GspsColor::Gray(value) => layer.put(DataElement::new(
            tags::GRAPHIC_LAYER_RECOMMENDED_DISPLAY_GRAYSCALE_VALUE,
            VR::US,
            PrimitiveValue::from(value),
        )),
        GspsColor::None => None,
    };
    layer
}

/// A Referenced Image Sequence item; no frame numbers means every frame.
fn gsps_reference(image: GspsImage, frames: &[u32]) -> InMemDicomObject {
    let mut reference = InMemDicomObject::from_element_iter([
        DataElement::new(tags::REFERENCED_SOP_CLASS_UID, VR::UI, image.sop_class_uid),
        DataElement::new(
            tags::REFERENCED_SOP_INSTANCE_UID,
            VR::UI,
            image.sop_instance_uid,
        ),
    ]);
    if !frames.is_empty() {
        let frames = frames.iter().map(u32::to_string).collect::<Vec<_>>();
        reference.put(DataElement::new(
            tags::REFERENCED_FRAME_NUMBER,
            VR::IS,
            frames.join("\\"),
        ));
    }
    reference
}

/// A Graphic Annotation Sequence item; no `images` leaves out its
/// Referenced Image Sequence.
fn gsps_annotation(
    layer: &str,
    images: &[InMemDicomObject],
    graphics: Vec<InMemDicomObject>,
    texts: Vec<InMemDicomObject>,
) -> InMemDicomObject {
    let mut annotation =
        InMemDicomObject::from_element_iter([DataElement::new(tags::GRAPHIC_LAYER, VR::CS, layer)]);
    if !images.is_empty() {
        annotation.put(fixture_sequence(
            tags::REFERENCED_IMAGE_SEQUENCE,
            images.to_vec(),
        ));
    }
    if !texts.is_empty() {
        annotation.put(fixture_sequence(tags::TEXT_OBJECT_SEQUENCE, texts));
    }
    if !graphics.is_empty() {
        annotation.put(fixture_sequence(tags::GRAPHIC_OBJECT_SEQUENCE, graphics));
    }
    annotation
}

fn gsps_floats(values: &[f32]) -> PrimitiveValue {
    PrimitiveValue::F32(values.to_vec().into())
}

/// A PIXEL-unit graphic object; `filled` is given for closed graphics only.
fn gsps_graphic(graphic_type: &str, data: &[f32], filled: Option<bool>) -> InMemDicomObject {
    let mut graphic = InMemDicomObject::from_element_iter([
        DataElement::new(tags::GRAPHIC_ANNOTATION_UNITS, VR::CS, "PIXEL"),
        DataElement::new(
            tags::GRAPHIC_DIMENSIONS,
            VR::US,
            PrimitiveValue::from(2_u16),
        ),
        DataElement::new(
            tags::NUMBER_OF_GRAPHIC_POINTS,
            VR::US,
            PrimitiveValue::from((data.len() / 2) as u16),
        ),
        DataElement::new(tags::GRAPHIC_DATA, VR::FL, gsps_floats(data)),
        DataElement::new(tags::GRAPHIC_TYPE, VR::CS, graphic_type),
    ]);
    if let Some(filled) = filled {
        graphic.put(DataElement::new(
            tags::GRAPHIC_FILLED,
            VR::CS,
            if filled { "Y" } else { "N" },
        ));
    }
    graphic
}

/// A PIXEL-unit text object in a bounding box, with an optional visible anchor.
fn gsps_boxed_text(
    text: &str,
    top_left: [f32; 2],
    bottom_right: [f32; 2],
    anchor: Option<[f32; 2]>,
) -> InMemDicomObject {
    let mut object = InMemDicomObject::from_element_iter([
        DataElement::new(tags::BOUNDING_BOX_ANNOTATION_UNITS, VR::CS, "PIXEL"),
        DataElement::new(tags::UNFORMATTED_TEXT_VALUE, VR::ST, text),
        DataElement::new(
            tags::BOUNDING_BOX_TOP_LEFT_HAND_CORNER,
            VR::FL,
            gsps_floats(&top_left),
        ),
        DataElement::new(
            tags::BOUNDING_BOX_BOTTOM_RIGHT_HAND_CORNER,
            VR::FL,
            gsps_floats(&bottom_right),
        ),
        DataElement::new(
            tags::BOUNDING_BOX_TEXT_HORIZONTAL_JUSTIFICATION,
            VR::CS,
            "CENTER",
        ),
    ]);
    if let Some(anchor) = anchor {
        object.put(DataElement::new(
            tags::ANCHOR_POINT_ANNOTATION_UNITS,
            VR::CS,
            "PIXEL",
        ));
        object.put(DataElement::new(
            tags::ANCHOR_POINT,
            VR::FL,
            gsps_floats(&anchor),
        ));
        object.put(DataElement::new(tags::ANCHOR_POINT_VISIBILITY, VR::CS, "Y"));
    }
    object
}

/// A PIXEL-unit text object placed only by an anchor point that is not drawn.
fn gsps_anchored_text(text: &str, anchor: [f32; 2]) -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::ANCHOR_POINT_ANNOTATION_UNITS, VR::CS, "PIXEL"),
        DataElement::new(tags::UNFORMATTED_TEXT_VALUE, VR::ST, text),
        DataElement::new(tags::ANCHOR_POINT, VR::FL, gsps_floats(&anchor)),
        DataElement::new(tags::ANCHOR_POINT_VISIBILITY, VR::CS, "N"),
    ])
}

fn write_gsps_state(path: &Path, state: GspsState<'_>) {
    let referenced_series = state
        .images
        .iter()
        .map(|image| {
            InMemDicomObject::from_element_iter([
                fixture_sequence(
                    tags::REFERENCED_IMAGE_SEQUENCE,
                    vec![gsps_reference(*image, &[])],
                ),
                DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, image.series_instance_uid),
            ])
        })
        .collect();
    // The whole image, for every referenced image.
    let displayed_area = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::DISPLAYED_AREA_TOP_LEFT_HAND_CORNER,
            VR::SL,
            PrimitiveValue::I32(vec![1, 1].into()),
        ),
        DataElement::new(
            tags::DISPLAYED_AREA_BOTTOM_RIGHT_HAND_CORNER,
            VR::SL,
            PrimitiveValue::I32(vec![GSPS_COLUMNS as i32, GSPS_ROWS as i32].into()),
        ),
        DataElement::new(tags::PRESENTATION_SIZE_MODE, VR::CS, "SCALE TO FIT"),
        DataElement::new(tags::PRESENTATION_PIXEL_ASPECT_RATIO, VR::IS, "1\\1"),
    ]);
    let sop_class_uid = uids::GRAYSCALE_SOFTCOPY_PRESENTATION_STATE_STORAGE;
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, sop_class_uid),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, state.sop_instance_uid),
        DataElement::new(tags::PATIENT_ID, VR::LO, GSPS_PATIENT_ID),
        DataElement::new(tags::STUDY_DATE, VR::DA, "20260930"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, GSPS_STUDY_UID),
        DataElement::new(tags::MODALITY, VR::CS, "PR"),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, GSPS_STATE_SERIES_UID),
        DataElement::new(tags::SERIES_DESCRIPTION, VR::LO, "Presentation states"),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, state.instance_number),
        DataElement::new(tags::CONTENT_LABEL, VR::CS, state.label),
        DataElement::new(tags::CONTENT_DESCRIPTION, VR::LO, state.description),
        DataElement::new(tags::CONTENT_CREATOR_NAME, VR::PN, "dcmview^fixtures"),
        DataElement::new(tags::PRESENTATION_CREATION_DATE, VR::DA, "20260930"),
        DataElement::new(tags::PRESENTATION_CREATION_TIME, VR::TM, "120000"),
        DataElement::new(tags::PRESENTATION_LUT_SHAPE, VR::CS, "IDENTITY"),
        fixture_sequence(tags::REFERENCED_SERIES_SEQUENCE, referenced_series),
        fixture_sequence(
            tags::DISPLAYED_AREA_SELECTION_SEQUENCE,
            vec![displayed_area],
        ),
        fixture_sequence(tags::GRAPHIC_ANNOTATION_SEQUENCE, state.annotations),
        fixture_sequence(tags::GRAPHIC_LAYER_SEQUENCE, state.layers),
    ])
    .with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(sop_class_uid)
            .media_storage_sop_instance_uid(state.sop_instance_uid),
    )
    .expect("build presentation state meta")
    .write_to_file(path)
    .expect("write presentation state fixture");
}

// Display-masking fixtures: two patients whose headers carry every kind of
// identifier the masking rules act on, images with a burned-in banner for
// redaction boxes, and a slide label image.
const MASKING_COLUMNS: usize = 320;
const MASKING_ROWS: usize = 240;
/// Rows the burned-in banner occupies, from the top of the image.
const MASKING_BANNER_ROWS: usize = 40;

struct MaskingImage<'a> {
    file_name: &'a str,
    sop_class_uid: &'a str,
    sop_instance_uid: &'a str,
    series_instance_uid: &'a str,
    study_instance_uid: &'a str,
    modality: &'a str,
    instance_number: &'a str,
    patient_id: &'a str,
    patient_name: &'a str,
    birth_date: &'a str,
    age: &'a str,
    study_date: &'a str,
    study_description: &'a str,
    series_description: &'a str,
    /// The two lines painted into the banner.
    banner: [&'a str; 2],
    /// Seeds the image below the banner, so files differ visibly.
    seed: usize,
}

/// A 3x5 glyph, one row per element, most significant bit on the left.
fn masking_glyph(character: char) -> [u8; 5] {
    match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        _ => [0; 5],
    }
}

/// A dark banner with two lines of bright block text over a bright wedge on
/// a mid-gray ground, painted with integer arithmetic only.
fn masking_pixels(banner: [&str; 2], seed: usize) -> Vec<u8> {
    const SCALE: usize = 3;
    let mut pixels = vec![0_u8; MASKING_COLUMNS * MASKING_ROWS];
    for row in MASKING_BANNER_ROWS..MASKING_ROWS {
        for column in 0..MASKING_COLUMNS {
            // A wedge opening downwards from the middle of the banner edge.
            let depth = row - MASKING_BANNER_ROWS;
            let offset = column.abs_diff(MASKING_COLUMNS / 2);
            let inside = offset * 5 <= depth * 4 + 20;
            let texture = (row * 7 + column * 13 + seed * 29) % 48;
            pixels[row * MASKING_COLUMNS + column] = if inside {
                (96 + texture + depth / 4) as u8
            } else {
                24
            };
        }
    }
    for (line, text) in banner.iter().enumerate() {
        let top = 6 + line * (5 * SCALE + 4);
        for (position, character) in text.chars().enumerate() {
            let left = 6 + position * (3 * SCALE + SCALE);
            for (glyph_row, bits) in masking_glyph(character).iter().enumerate() {
                for glyph_column in 0..3 {
                    if bits >> (2 - glyph_column) & 1 == 0 {
                        continue;
                    }
                    for y in 0..SCALE {
                        for x in 0..SCALE {
                            let row = top + glyph_row * SCALE + y;
                            let column = left + glyph_column * SCALE + x;
                            pixels[row * MASKING_COLUMNS + column] = 255;
                        }
                    }
                }
            }
        }
    }
    pixels
}

fn write_masking_image(fixture_dir: &Path, image: MaskingImage<'_>) {
    let referring_physician = InMemDicomObject::from_element_iter([
        DataElement::new(tags::INSTITUTION_NAME, VR::LO, "Lakeside Clinic"),
        fixture_sequence(
            tags::PERSON_IDENTIFICATION_CODE_SEQUENCE,
            vec![fixture_code("NPI-4471", "99LOCAL", "Okafor, Dana")],
        ),
    ]);
    let referenced_study = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::REFERENCED_SOP_CLASS_UID,
            VR::UI,
            "1.2.840.10008.3.1.2.3.1",
        ),
        DataElement::new(
            tags::REFERENCED_SOP_INSTANCE_UID,
            VR::UI,
            image.study_instance_uid,
        ),
    ]);
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, image.sop_class_uid),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, image.sop_instance_uid),
        DataElement::new(tags::STUDY_DATE, VR::DA, image.study_date),
        DataElement::new(tags::STUDY_TIME, VR::TM, "101500"),
        DataElement::new(
            tags::ACQUISITION_DATE_TIME,
            VR::DT,
            format!("{}101742.250000", image.study_date),
        ),
        DataElement::new(tags::ACCESSION_NUMBER, VR::SH, "ACC-7731905"),
        DataElement::new(tags::MODALITY, VR::CS, image.modality),
        DataElement::new(tags::MANUFACTURER, VR::LO, "dcmview fixtures"),
        DataElement::new(
            tags::INSTITUTION_NAME,
            VR::LO,
            "Northfield General Hospital",
        ),
        DataElement::new(
            tags::INSTITUTION_ADDRESS,
            VR::ST,
            "400 Example Avenue, Northfield, GA 30301",
        ),
        DataElement::new(tags::REFERRING_PHYSICIAN_NAME, VR::PN, "Okafor^Dana^^Dr"),
        fixture_sequence(
            tags::REFERRING_PHYSICIAN_IDENTIFICATION_SEQUENCE,
            vec![referring_physician],
        ),
        DataElement::new(tags::STATION_NAME, VR::SH, "US-ROOM-3"),
        DataElement::new(tags::STUDY_DESCRIPTION, VR::LO, image.study_description),
        DataElement::new(tags::SERIES_DESCRIPTION, VR::LO, image.series_description),
        DataElement::new(tags::OPERATORS_NAME, VR::PN, "Lindqvist^Per"),
        fixture_sequence(tags::REFERENCED_STUDY_SEQUENCE, vec![referenced_study]),
        // A private creator and one private value.
        DataElement::new(Tag(0x0009, 0x0010), VR::LO, "DCMVIEW FIXTURE"),
        DataElement::new(Tag(0x0009, 0x1001), VR::LO, "ward 5 bed 12"),
        DataElement::new(tags::PATIENT_NAME, VR::PN, image.patient_name),
        DataElement::new(tags::PATIENT_ID, VR::LO, image.patient_id),
        DataElement::new(tags::PATIENT_BIRTH_DATE, VR::DA, image.birth_date),
        DataElement::new(tags::PATIENT_SEX, VR::CS, "F"),
        DataElement::new(tags::PATIENT_AGE, VR::AS, image.age),
        DataElement::new(
            tags::PATIENT_ADDRESS,
            VR::LO,
            "12 Sample Street, Northfield, GA 30301",
        ),
        DataElement::new(tags::DEVICE_SERIAL_NUMBER, VR::LO, "SN-558213"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, image.study_instance_uid),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, image.series_instance_uid),
        DataElement::new(tags::SERIES_NUMBER, VR::IS, "1"),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, image.instance_number),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(
            tags::ROWS,
            VR::US,
            PrimitiveValue::from(MASKING_ROWS as u16),
        ),
        DataElement::new(
            tags::COLUMNS,
            VR::US,
            PrimitiveValue::from(MASKING_COLUMNS as u16),
        ),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::BURNED_IN_ANNOTATION, VR::CS, "YES"),
        DataElement::new(tags::WINDOW_CENTER, VR::DS, "128"),
        DataElement::new(tags::WINDOW_WIDTH, VR::DS, "256"),
        DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::from(masking_pixels(image.banner, image.seed)),
        ),
    ]);
    obj.with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(image.sop_class_uid)
            .media_storage_sop_instance_uid(image.sop_instance_uid),
    )
    .expect("build masking fixture meta")
    .write_to_file(fixture_dir.join(image.file_name))
    .expect("write masking fixture");
}

fn write_masking_fixtures(fixture_dir: &Path) {
    // Patient A is over 89 at the study date; two files of one series share
    // the banner position.
    for (file_name, sop_instance_uid, instance_number, seed) in [
        ("golden-masking-patient-a-us-1.dcm", "2.25.2000801", "1", 1),
        ("golden-masking-patient-a-us-2.dcm", "2.25.2000802", "2", 2),
    ] {
        write_masking_image(
            fixture_dir,
            MaskingImage {
                file_name,
                sop_class_uid: uids::ULTRASOUND_IMAGE_STORAGE,
                sop_instance_uid,
                series_instance_uid: "2.25.2000811",
                study_instance_uid: "2.25.2000821",
                modality: "US",
                instance_number,
                patient_id: "MRN-0042417",
                patient_name: "Rivera^Alma",
                birth_date: "19300214",
                age: "096Y",
                study_date: "20260520",
                study_description: "Abdominal ultrasound",
                series_description: "Liver sweep",
                banner: ["RIVERA ALMA", "ID 0042417  DOB 1930 02 14"],
                seed,
            },
        );
    }
    write_masking_image(
        fixture_dir,
        MaskingImage {
            file_name: "golden-masking-patient-b-us.dcm",
            sop_class_uid: uids::ULTRASOUND_IMAGE_STORAGE,
            sop_instance_uid: "2.25.2000803",
            series_instance_uid: "2.25.2000812",
            study_instance_uid: "2.25.2000822",
            modality: "US",
            instance_number: "1",
            patient_id: "MRN-0077120",
            patient_name: "Solberg^Britta",
            birth_date: "19810903",
            age: "045Y",
            study_date: "20260811",
            study_description: "Thyroid ultrasound",
            series_description: "Transverse",
            banner: ["SOLBERG BRITTA", "ID 0077120  DOB 1981 09 03"],
            seed: 3,
        },
    );

    // A slide label image: its pixels are a photograph of the label.
    let label_uid = "2.25.2000804";
    let label = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::VL_WHOLE_SLIDE_MICROSCOPY_IMAGE_STORAGE,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, label_uid),
        DataElement::new(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\LABEL\\NONE"),
        DataElement::new(tags::PATIENT_ID, VR::LO, "MRN-0077120"),
        DataElement::new(tags::PATIENT_NAME, VR::PN, "Solberg^Britta"),
        DataElement::new(tags::STUDY_DATE, VR::DA, "20260812"),
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, "2.25.2000823"),
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, "2.25.2000813"),
        DataElement::new(tags::SERIES_DESCRIPTION, VR::LO, "Slide label"),
        DataElement::new(tags::MODALITY, VR::CS, "SM"),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, "1"),
        DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "1"),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(
            tags::ROWS,
            VR::US,
            PrimitiveValue::from(MASKING_ROWS as u16),
        ),
        DataElement::new(
            tags::COLUMNS,
            VR::US,
            PrimitiveValue::from(MASKING_COLUMNS as u16),
        ),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::BURNED_IN_ANNOTATION, VR::CS, "YES"),
        DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::from(masking_pixels(["SOLBERG BRITTA", "S26 10442 A1"], 4)),
        ),
    ]);
    label
        .with_meta(
            FileMetaTableBuilder::new()
                .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
                .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::VL_WHOLE_SLIDE_MICROSCOPY_IMAGE_STORAGE)
                .media_storage_sop_instance_uid(label_uid),
        )
        .expect("build slide label fixture meta")
        .write_to_file(fixture_dir.join("golden-masking-wsi-label.dcm"))
        .expect("write slide label fixture");
}
