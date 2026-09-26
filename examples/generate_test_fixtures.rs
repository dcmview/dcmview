use dicom_core::value::fragments::Fragments;
use dicom_core::value::PixelFragmentSequence;
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
    write_display_shutter_fixtures(&fixture_dir);
    write_sr_without_pixels(&fixture_dir.join("golden-no-pixels-sr.dcm"));
    write_image_without_pixels(&fixture_dir.join("golden-image-no-pixels.dcm"));
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
        bitmap,
    );
}

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
    shutter: Vec<DataElement<InMemDicomObject>>,
) {
    let sop_class_uid = uids::DIGITAL_X_RAY_IMAGE_STORAGE_FOR_PRESENTATION;
    let mut obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, sop_class_uid),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, sop_instance_uid),
        DataElement::new(
            tags::PATIENT_ID,
            VR::LO,
            PrimitiveValue::from("GOLDEN-SHUTTER"),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("DX")),
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
            PrimitiveValue::from(vec![128_u8; 64]),
        ),
    ]);
    for element in shutter {
        obj.put(element);
    }

    obj.with_meta(
        FileMetaTableBuilder::new()
            .implementation_class_uid(FIXTURE_IMPLEMENTATION_CLASS_UID)
            .implementation_version_name(FIXTURE_IMPLEMENTATION_VERSION_NAME)
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(sop_class_uid)
            .media_storage_sop_instance_uid(sop_instance_uid),
    )
    .expect("build display shutter fixture meta")
    .write_to_file(path)
    .expect("write display shutter golden fixture");
}

/// PS3.5 Annex G frame: a 64-byte header, then one PackBits literal-run
/// segment per sample plane (Y, Cb, Cr), each at full resolution.
fn rle_ybr_fragment_4x2() -> Vec<u8> {
    let planes = (0..3)
        .map(|sample| {
            YBR_4X2
                .iter()
                .map(|pixel| pixel[sample])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
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
