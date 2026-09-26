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
    write_rt_dose_overlay_fixtures(&fixture_dir);
    write_parametric_map_overlay_fixtures(&fixture_dir);
    write_real_world_value_mapping_instance(&fixture_dir.join("golden-rwvm-ct-hounsfield.dcm"));
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
