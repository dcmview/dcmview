//! Native 8-bit Pixel Data shows the same pixels whichever way a conforming
//! file stores it.
//!
//! A value of VR OW is a stream of 16-bit words, each in the byte order of
//! the transfer syntax, so Explicit VR Big Endian stores every pair of 8-bit
//! samples exchanged; a value of VR OB is a byte stream in sample order. The
//! fixtures under `tests/fixtures/native-u8-order/` hold one set of samples
//! each way, and beside them as Explicit VR Little Endian. DCMTK 3.6.9
//! `dcmconv +tb` of each little-endian fixture writes Pixel Data byte for
//! byte equal to the `be-ow` fixture of the same name, and pydicom 3.0.2 and
//! DCMTK read all three files of a name as the same pixels. Nothing here
//! needs either tool.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server::{self, FileRegistry};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use std::path::{Path, PathBuf};

const EXPLICIT_VR_BIG_ENDIAN: &str = "1.2.840.10008.1.2.2";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native-u8-order")
}

/// A server over `paths`, served in that order.
async fn serve(paths: &[PathBuf]) -> TestServer {
    let report = support::discover(
        paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discover files");
    let registry = FileRegistry::new();
    for path in paths {
        let file = report
            .files
            .iter()
            .find(|file| &file.path == path)
            .unwrap_or_else(|| panic!("{} was not discovered", path.display()))
            .clone();
        registry.insert(file);
    }
    registry.mark_scan_complete();
    TestServer::new(server::router(support::app_state_with_registry(registry)))
}

async fn raw(server: &TestServer, file: usize, frame: usize) -> Vec<u8> {
    let path = format!("/api/file/{file}/frame/{frame}/raw");
    let response = server.get(&path).await;
    assert_eq!(response.status_code(), StatusCode::OK, "{path}");
    response.as_bytes().to_vec()
}

/// The display frame as interleaved RGB, and its PNG.
async fn display(server: &TestServer, file: usize, frame: usize) -> (Vec<u8>, Vec<u8>) {
    let path = format!("/api/file/{file}/frame/{frame}?mode=full_dynamic");
    let response = server.get(&path).await;
    assert_eq!(response.status_code(), StatusCode::OK, "{path}");
    let png = response.as_bytes().to_vec();
    let rgb = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .unwrap_or_else(|error| panic!("{path} is not a PNG: {error}"))
        .into_rgb8()
        .into_raw();
    (rgb, png)
}

async fn thumbnail(server: &TestServer, file: usize, frame: usize) -> Vec<u8> {
    let path = format!("/api/file/{file}/frame/{frame}/thumbnail?size=64");
    let response = server.get(&path).await;
    assert_eq!(response.status_code(), StatusCode::OK, "{path}");
    response.as_bytes().to_vec()
}

/// Sample `index` of every fixture, as the generator writes it.
fn sample(index: usize) -> u8 {
    ((index * 7 + 13) % 251) as u8
}

/// Exchanges the bytes of every 16-bit word of `value`, padded to whole
/// words: what changing the byte order of a value of VR OW does.
fn swapped_words(value: &[u8]) -> Vec<u8> {
    let mut words = value.to_vec();
    if words.len() % 2 == 1 {
        words.push(0);
    }
    for word in words.chunks_exact_mut(2) {
        word.swap(0, 1);
    }
    words
}

#[tokio::test]
async fn big_endian_eight_bit_frames_equal_their_little_endian_frames() {
    let mut compared = 0;
    for (layout, samples_per_pixel, planar) in [
        ("gray", 1, false),
        ("rgb", 3, false),
        ("rgb-planar", 3, true),
    ] {
        for (rows, columns) in [(2_usize, 4_usize), (3, 3)] {
            for frames in [1_usize, 3] {
                let name = |encoding: &str| {
                    fixtures().join(format!(
                        "golden-native-u8-{layout}-{rows}x{columns}-f{frames}-{encoding}.dcm"
                    ))
                };
                let case = format!("{layout} {rows}x{columns}, {frames} frame(s)");
                let server = serve(&[name("le"), name("be-ow"), name("be-ob")]).await;
                let frame_bytes = rows * columns * samples_per_pixel;
                for frame in 0..frames {
                    // Raw frames keep the stored sample order, planes included.
                    let stored = (frame * frame_bytes..(frame + 1) * frame_bytes)
                        .map(sample)
                        .collect::<Vec<_>>();
                    let little_endian = raw(&server, 0, frame).await;
                    assert_eq!(little_endian, stored, "{case}, frame {frame}");
                    let (shown, png) = display(&server, 0, frame).await;
                    if samples_per_pixel == 3 {
                        let pixels = rows * columns;
                        let interleaved = (0..pixels * 3)
                            .map(|index| {
                                let (pixel, channel) = (index / 3, index % 3);
                                if planar {
                                    stored[channel * pixels + pixel]
                                } else {
                                    stored[index]
                                }
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(shown, interleaved, "{case}, frame {frame}");
                    }
                    let small = thumbnail(&server, 0, frame).await;
                    for (file, encoding) in [(1, "OW"), (2, "OB")] {
                        let what = format!("{case}, frame {frame}, big endian {encoding}");
                        assert_eq!(raw(&server, file, frame).await, little_endian, "{what}");
                        let (pixels, encoded) = display(&server, file, frame).await;
                        assert_eq!(pixels, shown, "{what}");
                        assert_eq!(encoded, png, "{what}");
                        assert_eq!(thumbnail(&server, file, frame).await, small, "{what}");
                    }
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 24);
}

/// The fixture that is not conforming on purpose: Explicit VR Big Endian,
/// VR OW, its samples in order on disk. Read as the words its VR says it
/// holds, every pair of samples is exchanged, across the whole value (a
/// frame is 9 bytes, so words span frames). pydicom 3.0.2 and DCMTK 3.6.9
/// read it the same way.
#[tokio::test]
async fn words_written_in_sample_order_show_exchanged_pairs() {
    let path = fixtures().join("golden-native-u8-gray-3x3-f3-be-ow-sample-order-nonconforming.dcm");
    let on_disk = (0..27).map(sample).collect::<Vec<_>>();
    assert!(std::fs::read(&path)
        .expect("read fixture")
        .ends_with(&[on_disk.as_slice(), &[0]].concat()));
    let read = swapped_words(&on_disk);
    let server = serve(&[path]).await;
    for frame in 0..3 {
        assert_eq!(
            raw(&server, 0, frame).await,
            read[frame * 9..(frame + 1) * 9],
            "frame {frame}"
        );
    }
    // The first frame starts with the second sample, the last ends with the
    // value's padding byte.
    assert_eq!(raw(&server, 0, 0).await[..2], [sample(1), sample(0)]);
    assert_eq!(raw(&server, 0, 2).await[8], 0);
}

fn unsigned(tag: Tag, value: u16) -> DataElement<InMemDicomObject> {
    DataElement::new(tag, VR::US, PrimitiveValue::from(value))
}

/// A monochrome image of `bits_allocated` with the given pixel elements.
fn write_image(
    path: &Path,
    transfer_syntax: &str,
    (rows, columns, frames): (u16, u16, u16),
    bits_allocated: u16,
    elements: Vec<DataElement<InMemDicomObject>>,
) {
    let mut object = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.4200"),
        DataElement::new(tags::MODALITY, VR::CS, "OT"),
        unsigned(tags::SAMPLES_PER_PIXEL, 1),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, frames.to_string()),
        unsigned(tags::ROWS, rows),
        unsigned(tags::COLUMNS, columns),
        unsigned(tags::BITS_ALLOCATED, bits_allocated),
        unsigned(tags::BITS_STORED, bits_allocated),
        unsigned(tags::HIGH_BIT, bits_allocated - 1),
        unsigned(tags::PIXEL_REPRESENTATION, 0),
    ]);
    for element in elements {
        object.put(element);
    }
    object
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(transfer_syntax)
                .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.4200"),
        )
        .expect("file meta")
        .write_to_file(path)
        .expect("write image");
}

/// `bytes` as the 16-bit words a value of VR OW holds, first byte low.
/// dicom-rs writes each word in the byte order of the transfer syntax.
fn words(bytes: &[u8]) -> PrimitiveValue {
    PrimitiveValue::U16(
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect(),
    )
}

/// One-bit pixels are a bit stream through the words of a value of VR OW and
/// through the bytes of one of VR OB. Two 3x3 frames are 18 bits, so the
/// second frame starts inside the second byte. `dcmconv +tb` writes the
/// same four bytes for the words (`2d 4d 00 02`).
#[tokio::test]
async fn one_bit_frames_are_read_through_big_endian_words_and_bytes() {
    let bits: [u8; 18] = [1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0, 1];
    let packed = [0x4d_u8, 0x2d, 0x02, 0x00];
    let directory = tempfile::tempdir().expect("temp dir");
    let pixel_data = [
        (
            "words.dcm",
            VR::OW,
            words(&packed),
            [0x2d_u8, 0x4d, 0x00, 0x02],
        ),
        (
            "bytes.dcm",
            VR::OB,
            PrimitiveValue::from(packed.to_vec()),
            packed,
        ),
    ];
    let mut paths = Vec::new();
    for (name, vr, value, on_disk) in pixel_data {
        let path = directory.path().join(name);
        write_image(
            &path,
            EXPLICIT_VR_BIG_ENDIAN,
            (3, 3, 2),
            1,
            vec![DataElement::new(tags::PIXEL_DATA, vr, value)],
        );
        assert!(std::fs::read(&path).expect("read back").ends_with(&on_disk));
        paths.push(path);
    }
    let server = serve(&paths).await;
    for file in 0..2 {
        for frame in 0..2 {
            assert_eq!(
                raw(&server, file, frame).await,
                bits[frame * 9..(frame + 1) * 9],
                "file {file}, frame {frame}"
            );
        }
    }
}

/// Overlay Data of VR OW is read as words in the file's byte order: a
/// big-endian file draws the same overlay as a little-endian one.
#[tokio::test]
async fn overlay_planes_are_the_same_in_either_byte_order() {
    let directory = tempfile::tempdir().expect("temp dir");
    let pixels = [40_u8; 16];
    // A 4x4 plane with bits 0, 5, 10 and 15 set: the diagonal.
    let overlay = || {
        let group = 0x6000;
        vec![
            unsigned(Tag(group, 0x0010), 4),
            unsigned(Tag(group, 0x0011), 4),
            DataElement::new(Tag(group, 0x0040), VR::CS, "G"),
            DataElement::new(
                Tag(group, 0x0050),
                VR::SS,
                PrimitiveValue::I16([1, 1].into()),
            ),
            unsigned(Tag(group, 0x0100), 1),
            unsigned(Tag(group, 0x0102), 0),
            DataElement::new(
                Tag(group, 0x3000),
                VR::OW,
                PrimitiveValue::U16(vec![0x8421_u16].into()),
            ),
        ]
    };
    let mut paths = Vec::new();
    for (name, transfer_syntax, overlay_on_disk) in [
        (
            "little.dcm",
            uids::EXPLICIT_VR_LITTLE_ENDIAN,
            [0x21_u8, 0x84],
        ),
        ("big.dcm", EXPLICIT_VR_BIG_ENDIAN, [0x84, 0x21]),
    ] {
        let path = directory.path().join(name);
        let mut elements = overlay();
        elements.push(DataElement::new(tags::PIXEL_DATA, VR::OW, words(&pixels)));
        write_image(&path, transfer_syntax, (4, 4, 1), 8, elements);
        let stored = std::fs::read(&path).expect("read back");
        assert!(
            stored
                .windows(2)
                .any(|window| window == overlay_on_disk.as_slice()),
            "{name}"
        );
        paths.push(path);
    }
    let server = serve(&paths).await;
    let (little, _) = display(&server, 0, 0).await;
    let (big, _) = display(&server, 1, 0).await;
    assert_eq!(big, little);
    let gray = little
        .chunks_exact(3)
        .map(|pixel| pixel[0])
        .collect::<Vec<_>>();
    for (index, value) in gray.iter().enumerate() {
        let on_diagonal = index % 5 == 0;
        assert_eq!(*value == 255, on_diagonal, "pixel {index}");
    }
}
