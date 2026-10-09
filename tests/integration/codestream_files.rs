//! Compressed DICOM files for the tests of `pixels::codestream`: honest
//! codestreams written by the encoders named beside them, the edits that
//! make one declare another image than its file's header, and the writer
//! that puts frames under a header.
//!
//! Shared by the integration tests and `tests/codestream_cost`, so not every
//! binary uses every item.

#![allow(dead_code)]

use axum_test::TestServer;
use dcmview::annotations::AnnotationStore;
use dcmview::loader::{self, DiscoverOptions};
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::FileEntry;
use dicom_core::value::PixelFragmentSequence;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub const JPEG_BASELINE: &str = "1.2.840.10008.1.2.4.50";
pub const JPEG_LOSSLESS: &str = "1.2.840.10008.1.2.4.57";
pub const JPEG_LOSSLESS_SV1: &str = "1.2.840.10008.1.2.4.70";
pub const JPEG_LS: &str = "1.2.840.10008.1.2.4.80";
pub const JPEG_2000: &str = "1.2.840.10008.1.2.4.90";
pub const JPEG_XL: &str = "1.2.840.10008.1.2.4.110";
pub const DEFLATED_FRAME: &str = "1.2.840.10008.1.2.8.1";

/// What a file's header says about its frames.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub rows: u16,
    pub columns: u16,
    pub bits_allocated: u16,
    pub bits_stored: u16,
    pub samples: u16,
    pub photometric: &'static str,
}

impl Layout {
    pub const fn gray8(rows: u16, columns: u16) -> Self {
        Self::gray(rows, columns, 8, 8)
    }

    pub const fn gray(rows: u16, columns: u16, bits_allocated: u16, bits_stored: u16) -> Self {
        Self {
            rows,
            columns,
            bits_allocated,
            bits_stored,
            samples: 1,
            photometric: "MONOCHROME2",
        }
    }

    pub const fn color8(rows: u16, columns: u16, photometric: &'static str) -> Self {
        Self {
            rows,
            columns,
            bits_allocated: 8,
            bits_stored: 8,
            samples: 3,
            photometric,
        }
    }
}

/// Writes a file whose header says `layout` over `frames`, one fragment a
/// frame, with a Basic Offset Table when there is more than one.
pub fn write(path: &Path, transfer_syntax: &str, layout: Layout, frames: &[Vec<u8>]) {
    let mut object = InMemDicomObject::from_element_iter([
        DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
        ),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.4100"),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("OT")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(layout.rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(layout.columns)),
        DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(layout.bits_allocated),
        ),
        DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(layout.bits_stored),
        ),
        DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(layout.bits_stored - 1),
        ),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(
            tags::SAMPLES_PER_PIXEL,
            VR::US,
            PrimitiveValue::from(layout.samples),
        ),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from(layout.photometric),
        ),
        DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            PrimitiveValue::from(frames.len().to_string()),
        ),
    ]);
    if layout.samples == 3 {
        object.put(DataElement::new(
            tags::PLANAR_CONFIGURATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ));
    }
    object.put(DataElement::new(
        tags::PIXEL_DATA,
        VR::OB,
        fragments(frames),
    ));
    object
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(transfer_syntax)
                .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.4100"),
        )
        .expect("file meta")
        .write_to_file(path)
        .expect("write compressed DICOM");
}

/// Copies the DICOM file `source` to `path` with its pixel data replaced by
/// `frames` in `transfer_syntax`; everything else it says stays.
pub fn rewrite(source: &Path, path: &Path, transfer_syntax: &str, frames: &[Vec<u8>]) {
    let mut object = dicom_object::open_file(source).expect("open source DICOM");
    object.put(DataElement::new(
        tags::PIXEL_DATA,
        VR::OB,
        fragments(frames),
    ));
    object.update_meta(|meta| meta.transfer_syntax = transfer_syntax.to_string());
    object.write_to_file(path).expect("write rewritten DICOM");
}

/// One even-length fragment a frame, and their offsets when there are
/// several.
fn fragments(frames: &[Vec<u8>]) -> PixelFragmentSequence<Vec<u8>> {
    let mut offsets = Vec::new();
    let mut offset = 0_u32;
    let padded = frames
        .iter()
        .map(|frame| {
            let mut frame = frame.clone();
            if frame.len() % 2 == 1 {
                frame.push(0);
            }
            offsets.push(offset);
            offset += 8 + frame.len() as u32;
            frame
        })
        .collect::<Vec<_>>();
    if frames.len() < 2 {
        offsets.clear();
    }
    PixelFragmentSequence::new(offsets, padded)
}

/// The catalog entries the production loader makes for `paths`, in that
/// order, indexed from 0.
pub async fn list(paths: &[PathBuf]) -> Vec<FileEntry> {
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let scan = loader::discover_progressive(
        paths,
        DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
            formats: Default::default(),
        },
        events_tx,
        loader::DiscoveryCancellation::new(),
    );
    let collect = async {
        let mut files = Vec::new();
        while let Some(event) = events_rx.recv().await {
            if let loader::DiscoveryEvent::Selected { file, .. } = event {
                files.push(*file);
            }
        }
        files
    };
    let (report, mut files) = tokio::join!(scan, collect);
    report.expect("discovery completes");
    assert_eq!(files.len(), paths.len(), "every file is listed");
    files.sort_by_key(|file| paths.iter().position(|path| *path == file.path));
    for (index, file) in files.iter_mut().enumerate() {
        file.index = index;
    }
    files
}

/// The viewer's HTTP API over `files`.
pub fn serve(files: Vec<FileEntry>) -> TestServer {
    TestServer::new(server::router(AppState::new(
        FileRegistry::from_files(files),
        AnnotationStore::empty(),
    )))
}

/// A committed fixture.
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The bytes a constant of this module spells in hexadecimal.
pub fn hex(text: &str) -> Vec<u8> {
    let digits = text.as_bytes();
    assert!(digits.len().is_multiple_of(2), "whole bytes");
    digits
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("hexadecimal");
            u8::from_str_radix(text, 16).expect("hexadecimal")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Edits that make an honest codestream declare something else

/// Where the JPEG or JPEG-LS frame header's length field is.
pub fn jpeg_frame_header(frame: &[u8]) -> usize {
    let mut at = 2;
    loop {
        assert_eq!(frame[at], 0xFF, "a marker");
        let code = frame[at + 1];
        let frame_header =
            code == 0xF7 || ((0xC0..=0xCF).contains(&code) && !matches!(code, 0xC4 | 0xC8 | 0xCC));
        if frame_header {
            return at + 2;
        }
        at += 2 + usize::from(u16::from_be_bytes([frame[at + 2], frame[at + 3]]));
    }
}

/// `frame` with its JPEG or JPEG-LS frame header stating `rows` and
/// `columns`.
pub fn jpeg_sized(frame: &[u8], rows: u16, columns: u16) -> Vec<u8> {
    let at = jpeg_frame_header(frame);
    let mut frame = frame.to_vec();
    frame[at + 3..at + 5].copy_from_slice(&rows.to_be_bytes());
    frame[at + 5..at + 7].copy_from_slice(&columns.to_be_bytes());
    frame
}

/// `frame` with its JPEG or JPEG-LS frame header stating `precision`.
pub fn jpeg_precision(frame: &[u8], precision: u8) -> Vec<u8> {
    let at = jpeg_frame_header(frame);
    let mut frame = frame.to_vec();
    frame[at + 2] = precision;
    frame
}

/// `frame` with its JPEG or JPEG-LS frame header listing `components`
/// components, each like the first.
pub fn jpeg_components(frame: &[u8], components: u8) -> Vec<u8> {
    let at = jpeg_frame_header(frame);
    let old = usize::from(frame[at + 7]);
    let mut edited = frame[..at].to_vec();
    edited.extend_from_slice(&(8 + 3 * u16::from(components)).to_be_bytes());
    edited.extend_from_slice(&frame[at + 2..at + 7]);
    edited.push(components);
    for component in 0..components {
        edited.push(component + 1);
        edited.extend_from_slice(&frame[at + 9..at + 11]);
    }
    edited.extend_from_slice(&frame[at + 8 + 3 * old..]);
    edited
}

/// `frame` with `segments` empty comment segments after its start-of-image
/// marker.
pub fn jpeg_with_segments(frame: &[u8], segments: usize) -> Vec<u8> {
    let mut edited = frame[..2].to_vec();
    for _ in 0..segments {
        edited.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x02]);
    }
    edited.extend_from_slice(&frame[2..]);
    edited
}

/// `frame` with `scans` more scans before its end-of-image marker, each a
/// copy of its last.
pub fn jpeg_with_scans(frame: &[u8], scans: usize) -> Vec<u8> {
    let start = frame
        .windows(2)
        .rposition(|pair| pair == [0xFF, 0xDA])
        .expect("a scan");
    let end = frame.len() - 2;
    assert_eq!(frame[end..], [0xFF, 0xD9], "ends at the end of image");
    let mut edited = frame[..end].to_vec();
    for _ in 0..scans {
        edited.extend_from_slice(&frame[start..end]);
    }
    edited.extend_from_slice(&[0xFF, 0xD9]);
    edited
}

/// The offsets of the `SIZ` fields of a bare JPEG 2000 codestream.
pub mod siz {
    pub const XSIZ: usize = 8;
    pub const YSIZ: usize = 12;
    pub const XOSIZ: usize = 16;
    pub const YOSIZ: usize = 20;
    pub const XTSIZ: usize = 24;
    pub const YTSIZ: usize = 28;
    pub const CSIZ: usize = 40;
    pub const SSIZ: usize = 42;
    pub const XRSIZ: usize = 43;
}

/// `codestream` with the four bytes at `at` holding `value`.
pub fn j2k_u32(codestream: &[u8], at: usize, value: u32) -> Vec<u8> {
    let mut codestream = codestream.to_vec();
    codestream[at..at + 4].copy_from_slice(&value.to_be_bytes());
    codestream
}

/// `codestream` with the byte at `at` holding `value`.
pub fn j2k_u8(codestream: &[u8], at: usize, value: u8) -> Vec<u8> {
    let mut codestream = codestream.to_vec();
    codestream[at] = value;
    codestream
}

/// A bare JPEG 2000 codestream declaring an image and one tile of `rows` by
/// `columns`, otherwise `codestream`.
pub fn j2k_sized(codestream: &[u8], rows: u32, columns: u32) -> Vec<u8> {
    let mut edited = codestream.to_vec();
    for (at, value) in [
        (siz::XSIZ, columns),
        (siz::YSIZ, rows),
        (siz::XTSIZ, columns),
        (siz::YTSIZ, rows),
    ] {
        edited = j2k_u32(&edited, at, value);
    }
    edited
}

/// `codestream` declaring `components` components, each like the first.
pub fn j2k_components(codestream: &[u8], components: u16) -> Vec<u8> {
    let old = usize::from(u16::from_be_bytes([codestream[40], codestream[41]]));
    let mut edited = codestream[..4].to_vec();
    edited.extend_from_slice(&(38 + 3 * components).to_be_bytes());
    edited.extend_from_slice(&codestream[6..40]);
    edited.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        edited.extend_from_slice(&codestream[42..45]);
    }
    edited.extend_from_slice(&codestream[42 + 3 * old..]);
    edited
}

/// `codestream` with `segments` empty comment segments after its `SIZ`.
pub fn j2k_with_segments(codestream: &[u8], segments: usize) -> Vec<u8> {
    let end = 4 + usize::from(u16::from_be_bytes([codestream[4], codestream[5]]));
    let mut edited = codestream[..end].to_vec();
    for _ in 0..segments {
        edited.extend_from_slice(&[0xFF, 0x64, 0x00, 0x02]);
    }
    edited.extend_from_slice(&codestream[end..]);
    edited
}

/// Where the first `COD` marker of a bare codestream is.
pub fn j2k_cod(codestream: &[u8]) -> usize {
    let mut at = 2;
    loop {
        assert_eq!(codestream[at], 0xFF, "a marker");
        if codestream[at + 1] == 0x52 {
            return at;
        }
        at += 2 + usize::from(u16::from_be_bytes([codestream[at + 2], codestream[at + 3]]));
    }
}

/// `codestream` with its first `COD` declaring `layers` quality layers.
pub fn j2k_layers(codestream: &[u8], layers: u16) -> Vec<u8> {
    let at = j2k_cod(codestream);
    let mut edited = codestream.to_vec();
    edited[at + 6..at + 8].copy_from_slice(&layers.to_be_bytes());
    edited
}

/// `codestream` with its first `COD` declaring precincts of `2^exponent`
/// samples a side at every resolution.
pub fn j2k_precincts(codestream: &[u8], exponent: u8) -> Vec<u8> {
    let at = j2k_cod(codestream);
    let length = usize::from(u16::from_be_bytes([codestream[at + 2], codestream[at + 3]]));
    let levels = usize::from(codestream[at + 9]);
    let mut edited = codestream[..at + 2].to_vec();
    edited.extend_from_slice(&(12 + levels as u16 + 1).to_be_bytes());
    edited.push(codestream[at + 4] | 1);
    edited.extend_from_slice(&codestream[at + 5..at + 14]);
    edited.extend(std::iter::repeat_n(exponent << 4 | exponent, levels + 1));
    edited.extend_from_slice(&codestream[at + 2 + length..]);
    edited
}

/// `codestream` with a copy of its first `COD`, changed by `change`, put
/// into the header of its first tile-part (whose length grows to match).
pub fn j2k_tile_part_cod(codestream: &[u8], change: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let changed = change(codestream);
    let cod = j2k_cod(&changed);
    let length = usize::from(u16::from_be_bytes([changed[cod + 2], changed[cod + 3]]));
    let segment = &changed[cod..cod + 2 + length];
    let sot = codestream
        .windows(4)
        .position(|window| window == [0xFF, 0x90, 0x00, 0x0A])
        .expect("a tile-part");
    let psot = u32::from_be_bytes(
        codestream[sot + 6..sot + 10]
            .try_into()
            .expect("four bytes"),
    );
    let mut edited = codestream[..sot + 12].to_vec();
    edited[sot + 6..sot + 10].copy_from_slice(&(psot + segment.len() as u32).to_be_bytes());
    edited.extend_from_slice(segment);
    edited.extend_from_slice(&codestream[sot + 12..]);
    edited
}

/// A raw deflate stream of `length` zero bytes.
pub fn deflated_zeros(length: usize) -> Vec<u8> {
    use std::io::Write;
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    let block = [0_u8; 4096];
    let mut left = length;
    while left > 0 {
        let step = left.min(block.len());
        encoder.write_all(&block[..step]).expect("deflate");
        left -= step;
    }
    encoder.finish().expect("deflate")
}

// ---------------------------------------------------------------------------
// Honest codestreams, in hexadecimal

/// 16 x 16, 8-bit gray, 6 resolutions, one tile (OpenJPEG 2.5 through Pillow).
pub const J2K_GRAY8: &str = "\
    ff4fff5100290000000000100000001000000000000000000000001000000010000000000000000000010701\
    01ff52000c00000001000404040001ff5c00104040484850484850484850484850ff64002500014372656174\
    6564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a00000000006d0001ff93c7d4\
    0405dfc7da031f68100401c7c3ea0387d40900f8420d9bc50b39ff7f106fc1f38683e70d0fb41421ff600635\
    cf0371eb51817f24b7a7934fcfc0327e0193f30562584405ebc6849f6e395617624e0eba8af9fab1e294db7f\
    61b83638a9ffd9\
";

/// The same image with a single resolution level.
pub const J2K_GRAY8_ONE_RESOLUTION: &str = "\
    ff4fff5100290000000000100000001000000000000000000000001000000010000000000000000000010701\
    01ff52000c00000001000004040001ff5c00044040ff640025000143726561746564206279204f70656e4a50\
    45472076657273696f6e20322e352e34ff90000a0000000000de0001ff93df859a1237110a4619b682cea6c7\
    331cb0c0f4b3dccba787c2d5db1bc244d971f4e88a9e189fa2313dfbc4bd786d4792668bf1f94dbc1e9fa167\
    033307fe1227dcd9be53e6a7bbbf9a7508d4474ab7aa6cc3ef6b18b0aa1f413c66bd53e42c7fb1aa014d6269\
    5f3aa2d7e4025b239c26604933a9adc6b3f58e79f0489345db9ea5b2999d6018f98324cea65ad94735b14c55\
    c6d896b79df8991865d7e268833451c2106d896d47eff4b5c4b5c4d5499dfb3aaaaaaaaa9c11e67552d29c2a\
    e44b980ff7b44e227113889c44e22711387fffd9\
";

/// The same image in four 8 x 8 tiles.
pub const J2K_GRAY8_TILED: &str = "\
    ff4fff5100290000000000100000001000000000000000000000000800000008000000000000000000010701\
    01ff52000c00000001000104040001ff5c00074040484850ff640025000143726561746564206279204f7065\
    6e4a5045472076657273696f6e20322e352e34ff90000a00000000002f0001ff93df8088120a4aae6de77ba6\
    addc43340d2a6425afc07c21c0f9018022187f037fbfff90000a00010000002d0001ff93cfb43c120f33dc29\
    b7808e9e63db3c8d6c5fc07c21c0f9018022187f037fbfff90000a00020000002d0001ff93cfb43c23f31ea9\
    f2908e3dbf775e374d5975c07c21c0f9018022187f037fbfff90000a00030000003d0001ff93cfb43c04e75a\
    d88f6b7338446a5228c3e667cfc0227e0113f30524c9b2467b056ee724c00318159e985f242b7ee90fffd9\
";

/// The same image in three quality layers.
pub const J2K_GRAY8_LAYERS: &str = "\
    ff4fff5100290000000000100000001000000000000000000000001000000010000000000000000000010701\
    01ff52000c00000003000404040001ff5c00104040484850484850484850484850ff64002500014372656174\
    6564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000810001ff93c7cc\
    0805dfc7d8031f48200401c7a1f182000b3980a45383624e61b8368080c3e906000d9bc5908824b7c89c0862\
    5838f400c1d000c1f08500f842ff7f106fc1f38683e70df6018021ff600635cf0371eb51817fa7934ffde15f\
    7857da024405ebc6849f6e3956170eba8af9fab1e294db7fa9ffd9\
";

/// 64 x 64, 8-bit gray, 3 resolutions, 32 x 32 precincts.
pub const J2K_GRAY8_PRECINCTS: &str = "\
    ff4fff5100290000000000400000004000000000000000000000004000000040000000000000000000010701\
    01ff52000f01000001000204040001334455ff5c000a4040484850484850ff64002500014372656174656420\
    6279204f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000005c80001ff93df8220120a\
    b9fe019ca5c548127f4bbc6c43f659423e9b6d128bd2d5929c7cef05d4a0ace3d0c1e8d572ea476967f5c368\
    3d207818ad241a170a08cb7811074b7465f033f65a5fdf822814c16487931d67784344073087656ab66e34d0\
    e189575452adabed316fe2582d3b7537cabcbc81da855a632fb0ac2dfb26a841d249ad770c9e4b5edf2154f9\
    5f8a611d4bbfdf8228038520f980abead967842f1bbf30d7d8aeaae77c6284e35c93cd45a070cdd4ec52805c\
    df3d27e7a6e82ee73658cea49d77e173e90cc39090e56646858e751d1ea560f7449bdf822019c2e783730766\
    6c7641726befd641bdc032803ded5554b10f82cae486508b66a889758b84a79701df7a3d23607baeeca1f7c6\
    4b17b8af619cefd79e21de98e67e9d642fc7da4f3f0248fc0a402de1445995d51acccf07db18d88f091f3113\
    4f239981892d57076e41211b871b4f399694baf9b72406ad7c8777bdd9ee62d45b186747b1830796c432383f\
    3fe1be1fea5d5f5d45786bb57b2d29b74531499f664319b64a64489423a0b6f60315faa79525009f826cbf6e\
    3c6a6c4a6a1e5c946a5fc7da5b3f0288fc0a001751cce17d5d9af3e6227cbd3edd502d190e9a75989a2c3f5a\
    f45fe6461ab335d1a49bd5fe40d3b4e4e84dbeaf29300f3e2fe06c8e34ff0bd26d0a4e972af8c1cf6c2b72d9\
    aa10167754590d4c4570c3cbf5b640f73800f6945b39c894e5374d075d154471168646d499b51bd813f46293\
    55fcd38fc9e038e13ba22b7fc7da5b3f0318fc0a8004057b1ad18035f63d57c0a8440b19b7ee0c3cb61dc7e5\
    888984ca88899f7cf3d9c084917d73b79532812e90a31953c03c6d84145b0322222f5d2a6252260662476fbd\
    a42701559e15cec97b076e356c078a4128ac869747b1b1854ea05f0115e09e7851ff4aac74567e2e9c2dd86a\
    09bd86190d8fc4b198d85b9eaff20ba8862eeea717a1a3c6bfc7da5d3f0308fc0b403611ae2a2b06a76f9b09\
    065c219b703bfe906b7ada301cd14aa92ffe22cfcc2d067e088ef1d5b354e4169b35f0d71f9cef66379fd8ba\
    3a01537b90f6c9fde1a403b76649e40936e9ce04bb3d286b6d3f90254a2bccb9f0bb52f4c5898dab1e702e51\
    7fb7e439ca0844b49174543461107f45c856e69a892746306c7a0e6b96f64acd4e72bc19f7c2e87d0bc7da55\
    3f02c8fc08c0574ed0da20a59de89d049159c7319663b90735e7ebea02752da35326b8649b8ad9aab18fb6ff\
    7fff7f7f52321122ecd95a223d1a70b8dcd99b63013e47606139588bc48d0cddd00e987a7d614df45f6786bf\
    ff7fff7f522dc254c332096cff39f51166554a1af8f6e8eb0659b6f97fff56afddff7fff7fff7fcfc10a7e05\
    d3f327ad9baa985bdd6106d54899eb500c81e0d6649aa14684cce54743fb9f57116345cd34a2bd07aeaacd30\
    b85c0262bc2b0f21104b82f1fe7a30a536520af3ae4030a5031c7b563c8290cca2a86c02446d5f62feae5f86\
    66b45776c862bafd33e446c37455d9c5b9caf4d25f9512ff7fff7f62590cd263a3e1fd84b43dbc09fb16aa33\
    3facd4653afa16bf7727ea21ec1fff7fff7fff7fff7fc7da5b3f0458fc09803367c5bcab9cdf5a2babab0f5a\
    f8a8927c48aa8dcab3efe2063dabc23789ad9db5059e112b0abadb01172778ef3c8e051c1febc751c4f71e99\
    46b87d812035eef9d1bed51160934f4e758a233c8feb4d7c97a767472669382cd6b1ae53af8b94444ec7c739\
    c8167d54b3ed4e4365e7cf92972e46cf6138bc977bcf43a051f2eb1b95c18cfab1af95a9f4f73b084cad5164\
    93ff7fff7fff7fc7da813f0459f992003daa6445542ed63ce8b38f8b4e78309bd574c4687d83d8ce32b34d4c\
    f6ba266983648143c4e153949eb8c2df59ea0db1ac2ca86b748b16538564422f2139ecf7425bc04c41f9348e\
    5ec83cac0a8221aade4fbe36dc8c033da91518c412d224aca2a164163a2f9e123ac60e06a048e19dd4a16073\
    2687af794fddc055b7ac121d87d02e475f9cbe3ca4cac96fd1e289b234c0aa93e57b10dddec692efe0834f24\
    b2d47fff7fff7fff7fffd9\
";

/// The 16 x 16 image inside a JP2 file.
pub const J2K_GRAY8_JP2: &str = "\
    0000000c6a5020200d0a870a00000014667479706a703220000000006a7032200000002d6a70326800000016\
    6968647200000010000000100001070700000000000f636f6c7201000000000011000000eb6a703263ff4fff\
    510029000000000010000000100000000000000000000000100000001000000000000000000001070101ff52\
    000c00000001000404040001ff5c00104040484850484850484850484850ff64002500014372656174656420\
    6279204f70656e4a5045472076657273696f6e20322e352e34ff90000a00000000006d0001ff93c7d40405df\
    c7da031f68100401c7c3ea0387d40900f8420d9bc50b39ff7f106fc1f38683e70d0fb41421ff600635cf0371\
    eb51817f24b7a7934fcfc0327e0193f30562584405ebc6849f6e395617624e0eba8af9fab1e294db7f61b836\
    38a9ffd9\
";

/// 16 x 16, 8-bit, three components with the reversible colour transform.
pub const J2K_RGB8_RCT: &str = "\
    ff4fff51002f0000000000100000001000000000000000000000001000000010000000000000000000030701\
    01070101070101ff52000c00000001010404040001ff5c00104040484850484850484850484850ff64002500\
    0143726561746564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000002f3\
    0001ff93c1f202083fc0f84101c7d404057fc3ea0187d40507d40404049f06dfc1f38280f84180f90100097f\
    00017fc3ea018fb40a0fa80804033f017fc3ea068fb4161f682804bd52ebff7f023aba2dd90a95f6a85fc7da\
    0b3f0069f9830003fd9018a708f1094386d106167e9ae29fc7da0b3f0059f983000d298f962f077a132f510e\
    1faceb2d9ac3ea0f8fb44a1f689015e67e5ace1a09158828410c41713f1a36926c2dd38e3f808c8a28c1f829\
    e3057f190d2ff335fa907abaa00216c79c86c9866fc7da237e62b3f315096a92d40972039bdd6d801be62669\
    f9910c2132c69b3d2f43c204c871c871fd0f4379989daf1739444d382f06da87a2c7e05c00c04d6ae600771b\
    c7da233f0148fc04801da98e1f2e3b890d032a08040a0560bb570c2f6e92668cca7208c3107eed0ad2e688f0\
    45071e722ed5cba521b0c3dae2ff3e39f2c2129dcfc0c67e06f3f3276253e8f933b6a434cb21f66a5aae0844\
    66a0a52179a667b79fc67e3e5c2ca45d2da0fe186521df9acc8079594373cbec1f62500339ee6260d9f17df7\
    388bc75cb5379ba054a7ee8df632b5367ab34aec728606ec4f5f3a3d8750c5d2a6d56423e8aafcba8ca4283b\
    61b835b53a08823c23e293313c973a36edff709e8be2bd5393becc118a3893732be1669ef0be42cfc0d6fcce\
    a7e64219b1e73e7884189a93eb57cecf9c011838181a3872d70b90349f8d8dacbe318df3d025753ed7104eeb\
    d834ebd0391310631d5fff7f3184293db597403e15df9441d0634243bf523aafcb02d38f263318ce0587a23f\
    6371a76c4157bd7ecdc4243390c1e7e611e000f196115aee0acf26018d461543675e6d656a215d14d4082ac4\
    ede5a74a85cfd88734587aa3e16c27cfc0aa7e04f1f80c0038f19278db8d19e57bafdd2651d257f3e6e3b15f\
    571c17526b57f61dc577aaa19e3c23130ec3c0e1b203010a1dc581c3def216cd6b2b0e0e6c26606235c76264\
    665dd65728271c6c6bc571630e7971bae100db2e18796f35c1bc591574ff4eb45f103f6577ff7fff7fffd9\
";

/// 16 x 16, 8-bit, three components without a colour transform.
pub const J2K_RGB8_NO_MCT: &str = "\
    ff4fff51002f0000000000100000001000000000000000000000001000000010000000000000000000030701\
    01070101070101ff52000c00000001000404040001ff5c00104040484850484850484850484850ff64002500\
    0143726561746564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a00000000024b\
    0001ff93c7d40405dfc074100980c7da031f68100401c7c1f38281f20183ea02007f04081fc03a0c0f90141f\
    501003003f05cbc3ea0387d40900f8420d9bc50b39ff7f106fc3ea059f8034fcc1800324d8507f0238204aea\
    2f0d2c51d699acc7da0b0fa80e1f682803b780c109053bd505ee29c695c1f38683e70d0fb41421ff600635cf\
    0371eb51817f24b7a7934fc7da253f0148fc04801e552c82dabc43bab85ab0d3a02e5e9e8a5f0edf7692668c\
    ca76f788e962b843651603bc7b0f1dc62cdda3a905103bee1e8b5a70c9599ddfc7da253f0149f98a0021c341\
    fbb5c7239d1f3ad783d3904401d8ef0be5756303e105ede840f4a8df3819668b5f348322d3c16d48c0ab7dca\
    4e46d868c947f2ed67cfafcfc0327e0193f30562584405ebc6849f6e395617624e0eba8af9fab1e294db7f61\
    b83638a9cfc0ae7e0553f31c394a3170d88c8c576003ecee2a5ca3389cdd4901dde86bf10bee654d61223e38\
    38e179ec6bd83b0a23cea403ba25c581c3002550a3514fd1304cf18d811321afbfbafc3035a3e37aa9b5dd05\
    54d60b90977d7b801f61b64d604f5ded7d72e641b9e6e5562aad9b3eb35749ff7fff7fff7fcfc0ca7e06d3f3\
    2036220926aef7f93412fcc585570eb4d9dd2a50409c8f9f61f0a677269c1a3b82383c508e818bd832802411\
    8760119dd1f19b0738f05fb509db7efc8c58bc57ff777b720b0d7a18f2506e99087d4cf6a21caef6e1330159\
    0a06e116485cd32491069b2490e841bc0f377279670d24c8fcc51ee057fe12cd885ee7b7226a02fe3581945d\
    cca2544a1fffd9\
";

/// 16 x 16, 16-bit gray.
pub const J2K_GRAY16: &str = "\
    ff4fff5100290000000000100000001000000000000000000000001000000010000000000000000000010f01\
    01ff52000c00000001000404040001ff5c00104080888890888890888890888890ff64002500014372656174\
    6564206279204f70656e4a5045472076657273696f6e20322e352e34ff90000a00000000004d0001ff93cffc\
    300809f8c007da03001f68100401c7c003ea028007d4060d030b39f8c001f3838003e70a221a13037e93ac7f\
    c0007c224000f9020036a1853f3da63433ffd9\
";

/// 15 columns by 9 rows, 8-bit gray.
pub const J2K_GRAY8_ODD: &str = "\
    ff4fff51002900000000000f0000000900000000000000000000000f00000009000000000000000000010701\
    01ff52000c00000001000304040001ff5c000d4040484850484850484850ff64002500014372656174656420\
    6279204f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000240001ff93df802807aa\
    e8c47fc3ea02000d03c0f901801bed7d80ffd9\
";

/// 16 x 16, 12-bit gray (opj_compress).
pub const J2K_GRAY12: &str = "\
    ff4fff5100290000000000100000001000000000000000000000001000000010000000000000000000010b01\
    01ff52000c00000001000204040001ff5c000a4060686870686870ff64002500014372656174656420627920\
    4f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000460001ff93dfe058120bd35f4b\
    9321239a21e8544c878d8cc1d5f825babfc01f383803e70a221a13037e93ac7fc007c22400f9020036a1853f\
    3da63433ffd9\
";

/// 64 x 64, 8-bit gray, four tiles, a tile-part for each resolution (opj_compress).
pub const J2K_GRAY8_TILE_PARTS: &str = "\
    ff4fff5100290000000000400000004000000000000000000000002000000020000000000000000000010701\
    01ff52000c00000001000204040001ff5c000a4040484850484850ff64002500014372656174656420627920\
    4f70656e4a5045472076657273696f6e20322e352e34ff90000a0000000000550003ff93df8220120aba3629\
    3fb00b2d313ac66e698d044719989fd7fb10f1af8574f1050f25a50847f0f75e0fe7451ef34bd613ac1d78b7\
    119f5d7a48cb87e4ac6e45fbd7f5ec194991dfff90000a0000000000900103ff93c7da513f0298fc0a002dd6\
    ae5ba1898a59950e4feec484bea14d2c6f9969a234a87e7f63df46c7c9d311270738e9d7c7d22406ad7cd633\
    c6dfad4198b9f0186f19a08ef17d7827be8ab2d8dc80859406ec86d4756e4d37d95e3f2d29a79087742e93bd\
    ecf653ae813af7b0d7d8095b8123cf6b9b5781cec72741f78524ad4c9b3b7fff90000a0000000000b50203ff\
    93cfc1027e07b3f32260abc47d5cb202eb4636d9329c3be969dc87bc3e5e1aafcfb305f4ae81201c30a1df57\
    b044cc29ad6d40138215ae4d9fb8d9e7fef91de8d9e21de2f22386953f52321122ecd95a223d1a70b93c9303\
    71780a36457037ac06a5da8c1a7f3611112f43b5bbbdc084eb2e818fb71b65175874fe0ffdf80638cb7c5e24\
    bcdbaa5cdc163322d4752279f772d12ef709d7688e363e3263b6113fff7fff7fff7fff7fff90000a00010000\
    00540003ff93df821814c165dfd3d99df9f4781cac485877d1a7da6a3fc1ef638b974e1f01de5afdbf05d8cb\
    37e096be86e2f55cac8547d254fa7c7a572527df4bd39da5bc4bd136ca853bcfff90000a0001000000940103\
    ff93c7da573f02d8fc09401751cce17d5d9af3e53ea9d7893bd0345879cedcaf6d015b4421773ffd798c718f\
    5381b57e431b1c7f4b0529300f3e2fe06c97b1ca1cade99762e7fe683deb822ddaebf87267442e3ae23ea33e\
    a47856a9bd8ee2a59ee17f129c72521d37b8566f71c6f73d4b358cb72a716094c54b0bcf662c8f82caaa9965\
    4e2d5275ff90000a0001000000b70203ff93cfc1027e07b3f324ad9baa985bdd6106d54899eb500c81e0d664\
    9aa14684cce544b397834a1077c62b913276f4814b52fe4607069872710f1a19281ed61a469cd26d390bc88e\
    1a540c990c5da3d4bfa594fcc3ae8d017ad4635f068eed48336daf88c11571147f0324bdd19caef78457d042\
    47f17c30309ee6d50a10988bb263df4118f82f62576659b42102c33631a392fdd483ddf88d6d1d3b4f89ad91\
    080415ff7fff7fff7fff7fff90000a0002000000580003ff93df8238038520f9e5a2ad391380ae60498e3795\
    e2c046fed9d5e202fd82789206bf4967d55d3cf1040dd4669f829b140f57ece13644b5f92e019110ae862794\
    253f9c430221ca437a4abfff90000a0002000000a10103ff93c7da613f0308fc0a8003c73fec6e91bd368ec4\
    d7aa1d4336a2d57522ce976011f5aa21d927525aef679e69eea3e3d01e488084f398bf252a7f1953c03fddee\
    5dbb46291e62960e2558aa075a4d23e6af7d03a8e87fd29aef6af3e587a80a9e5e7a4920ae0c1d9f87bf0115\
    e09e7851ff4aaca29dc6f47629f9ceeceac6bf13e1212d29d56cfcbf5527790f039155774992de5fff90000a\
    0002000000c00203ff93c7da833f0438fc09403367c522727f5af3ff4a633dce22e598f2fef4dfafbec1a878\
    5cee3b2029cf0589ccc6419959b810efff75ca904af0f1240ee6bc09fb0e4dc72238098a08921f8b3c8e051c\
    1febc751c4f71e9946b87d812035eef9d1bed51160934f4e758a233c9d5fd8567cb32b5e425d106d860d934d\
    e7623a55b71116075f0967da8edc1e9ebe4a5f2e46cf6138bc977bcf43a051f2eb1b95c18cfab1eedcfa4cd7\
    5e3d15f2c87fff7fff7fff7fff90000a0003000000580003ff93df823819c2e78373076a8db7adf28961316d\
    b57733f7ed938de66b5f7de9fe510f393674392140f9ab47e98b8935d2d86e5b7b640fa392e9c968f57631a8\
    7efec3b784058de601f4b27fff90000a00030000009d0103ff93c7da5b3f02d8fc0b005252282c8d85292777\
    6d8cdf4c69c0c28c9044e690782346b3cc1ebeba23da18dc398aaec7fc4d9b49d8e060cf1f9ceebf50e19e48\
    0ce87147fd9ee0fe6773a2df1e1014e87bf8619b7d982843442b8e251b750ed6513bb42c691e702e517fb7e4\
    39ca0844b54e00c183b789f8421f1f56d835d29c6ca6d3d7d4e3e4bbe057e7a24dff11ddbfff90000a000300\
    0000bf0203ff93c7da833f0439f992003daa65eb285b8868b4fbf69ff6a7061f9781f6f44b82bfd972fb5813\
    d5beca2ae89a2e1d2896ce33dce3ecf9a0128d30c5c17c2dd1de5e16d0ef171b57b5c7bba7425bc04c41f934\
    8e5ec83cac0a8221aade4fbe36dc8c033da91518c412d228e6b38bf78e8d04ed00b2902ba9f1ed923c060b52\
    577bc708660802adbd60239f3dd7f44f9cbe3ca4cac96fd1e289b234c0aa93e57b10dddec692efe0834f24b2\
    d47fff7fff7fff7fffd9\
";

/// 16 x 16 gray, progressive, several scans (libjpeg through Pillow).
pub const JPEG_GRAY8_PROGRESSIVE: &str = "\
    ffd8ffe000104a46494600010100000100010000ffdb00430003020203020203030303040303040508050504\
    04050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f1718161418121415\
    14ffc2000b080010001001011100ffc4001500010100000000000000000000000000000607ffda0008010100\
    0000019452d536ffc40017100101010100000000000000000000000005000204ffda00080101000105023c68\
    f1a3c6b78e31b97fffc4001e10000104010500000000000000000000000100030421311114414251ffda0008\
    010100063f021485214b7335f6e3323b3873ce83d3585fffc4001a1001010002030000000000000000000000\
    01110010213151ffda0008010100013f21d888a9f138a8c1dc141567067fffda0008010100000010afffc400\
    18100003010100000000000000000000000000011141a1ffda0008010100013f10e0e1c1c383854502967d99\
    b28d4a33ffd9\
";

/// 15 columns by 9 rows, YCbCr 4:2:0.
pub const JPEG_RGB8_420_ODD: &str = "\
    ffd8ffe000104a46494600010100000100010000ffdb00430003020203020203030303040303040508050504\
    04050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f1718161418121415\
    14ffdb00430103040405040509050509140d0b0d141414141414141414141414141414141414141414141414\
    1414141414141414141414141414141414141414141414141414ffc00011080009000f030122000211010311\
    01ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303\
    020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f024\
    33627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a63646566\
    6768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7\
    b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc400\
    1f0100030101010101010101010000000000000102030405060708090a0bffc400b511000201020404030407\
    05040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a\
    162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a63646566676869\
    6a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9\
    bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c030100\
    02110311003f00e1b4af08f85be09c3169e960753f10a88586916e4c72856e7324a15963e30768cb1dc87015\
    b70edacbe1eeb7e3eb56d57c69a9daf877c3d024652cd6436f650100a2c84331c316661bc927e7db9c600f32\
    f811ff00213d3bfeb8affe846bd8ff0069bff8ff00f027fd73beff00d02d6bdbcd635b059a43074e77a8ea7b\
    2e769e8bd9cea3e55171e58be5b72c5a7ade729dac4e4bc374619a52c173deacd49baad5dae58b93518b768d\
    eda3d5abeada563fffd9\
";

/// 16 x 16 gray with a restart marker after every block.
pub const JPEG_GRAY8_RESTART: &str = "\
    ffd8ffe000104a46494600010100000100010000ffdb00430003020203020203030303040303040508050504\
    04050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f1718161418121415\
    14ffc0000b080010001001011100ffc4001f0000010501010101010100000000000000000102030405060708\
    090a0bffc400b5100002010303020403050504040000017d0102030004110512213141061351610722711432\
    8191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a43444546474849\
    4a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3\
    a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9ea\
    f1f2f3f4f5f6f7f8f9faffdd00040001ffda0008010100003f00f94be1ff00c1bff57fb8f4ed5fffd0c7f87f\
    f06ffd5fee3d3b57ffd1ef7e1ffc1bff0057fb8f4ed5ffd2faa2f2c3c1ff0006fc2afe24f1beb9a7f8674587\
    23ed3a84a13cd708d279512fde9652b1b958d033b6d3b549afffd9\
";

/// 16 x 16 YCbCr, progressive.
pub const JPEG_RGB8_PROGRESSIVE: &str = "\
    ffd8ffe000104a46494600010100000100010000ffdb00430003020203020203030303040303040508050504\
    04050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f1718161418121415\
    14ffdb00430103040405040509050509140d0b0d141414141414141414141414141414141414141414141414\
    1414141414141414141414141414141414141414141414141414ffc200110800100010030122000211010311\
    01ffc4001500010100000000000000000000000000000405ffc4001501010100000000000000000000000000\
    000102ffda000c0301000210031000000185694f2fffc4001910000203010000000000000000000000000204\
    01030522ffda0008010100010502a945712051b5c05ea532a497e3ffc40021110001000a0300000000000000\
    0000000001000203040511233141627182b1ffda0008010301013f017285052b3432d8dfa8c73ea7ffc4001e\
    110001020701000000000000000000000001024100030421315162a1ffda0008010201013f019b5370a072ee\
    4720379b52a3ffc4002510000102040505010000000000000000000102210003044111121351810514233271\
    d1ffda0008010100063f0212f26a5437892c7936833eb9624d32063a69f51c6ffb0108477551700b25ee6d02\
    b3aa542244946394cc61be09172df5a3ffc4001c100002020301010000000000000000000001112131004151\
    6171ffda0008010100013f2109167d41f023e2e468bc051e2042201b3267da70624fd442b2d8b846a4078329\
    f04857b44144974b3fffda000c03010002000300000010abffc4001b11000201050000000000000000000000\
    000121110031416171ffda0008010301013f100e8337dd661b368808afffc4001a1101010002030000000000\
    00000000000001110021314151ffda0008010201013f100be4108bd81802e3492f873fffc400191001010101\
    010100000000000000000000011121003141ffda0008010100013f10696606474a38118aab0821795ee082d2\
    007531538d5c41aa02dd17ee0c803dbf4241a7af548038343bffd9\
";

/// 16 x 16 gray with a comment and a 3 KiB profile segment before the frame header.
pub const JPEG_GRAY8_WITH_PROFILE: &str = "\
    ffd8ffe000104a46494600010100000100010000ffe20c104943435f50524f46494c45000101000102030405\
    060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031\
    32333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d\
    5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f80818283848586878889\
    8a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5\
    b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1\
    e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff000102030405060708090a0b0c0d\
    0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f30313233343536373839\
    3a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465\
    666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f9091\
    92939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbd\
    bebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9\
    eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff000102030405060708090a0b0c0d0e0f101112131415\
    161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f4041\
    42434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d\
    6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f90919293949596979899\
    9a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5\
    c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1\
    f2f3f4f5f6f7f8f9fafbfcfdfeff000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d\
    1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40414243444546474849\
    4a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475\
    767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1\
    a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccd\
    cecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9\
    fafbfcfdfeff000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425\
    262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f5051\
    52535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d\
    7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9\
    aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5\
    d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff0001\
    02030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d\
    2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f50515253545556575859\
    5a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485\
    868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1\
    b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdd\
    dedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff00010203040506070809\
    0a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435\
    363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f6061\
    62636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d\
    8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9\
    babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5\
    e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff000102030405060708090a0b0c0d0e0f1011\
    12131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d\
    3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f60616263646566676869\
    6a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495\
    969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1\
    c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebeced\
    eeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff000102030405060708090a0b0c0d0e0f10111213141516171819\
    1a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445\
    464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f7071\
    72737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d\
    9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9\
    cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5\
    f6f7f8f9fafbfcfdfeff000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021\
    22232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d\
    4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f70717273747576777879\
    7a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5\
    a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1\
    d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfd\
    feff000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20212223242526272829\
    2a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455\
    565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f8081\
    82838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacad\
    aeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9\
    dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff000102030405\
    060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031\
    32333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d\
    5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f80818283848586878889\
    8a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5\
    b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1\
    e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfefffffe00116d61646520666f722061\
    2074657374ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b\
    0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffc0000b080010001001011100ff\
    c4001f0000010501010101010100000000000000000102030405060708090a0bffc400b51000020103030204\
    03050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f0243362\
    7282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768\
    696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9\
    bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda000801\
    0100003f00f94be1ff00c1bff57fb8f4ed5f4afc3ff837feaff71e9dabd57e1ffc1bff0057fb8f4ed5ed7796\
    1e0ff837e157f1278df5cd3fc33a2c391f69d42509e6b84693ca897ef4b2958dcac6819db69daa4d7fffd9\
";

/// 16 x 16, 8-bit gray, lossless, selection value 1 (DCMTK).
pub const JPEG_LOSSLESS_GRAY8: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b080010001001011100ffc4001600010101000000\
    00000000000000000000030408ffda0008010100010000cfeeeeeeeeeeeeeeef5bbbbbbbbbbbbbbbd6eeeeee\
    eeeeeeeef5bbbbbbbbbbbbbbbd6eeeeeeeeeeeeeef5bbbbbbbbbbbbbbbd6eeeeeeeeeeeeeef5bbbbbbbbbbbb\
    bbbd6eeeeeeeeeeeeeef5bbbbbbbbbbbbbbbd6eeeeeeeeeeeeeef5bbbbbbbbbbbbbbbd6eeeeeeeeeeeeeef5b\
    bbbbbbbbbbbbbbd6eeeeeeeeeeeeef81ab777777777777c0ceff00ffffd9\
";

/// 16 x 16, 12 bits stored in 16, lossless, selection value 1 (DCMTK).
pub const JPEG_LOSSLESS_GRAY12: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b100010001001011100ffc4001500010100000000\
    000000000000000000000710ffda00080101000100009191919191919191919191919191919b919191919191\
    9191919191919191919b9191919191919191919191919191919b9191919191919191919191919191919b9191\
    919191919191919191919191919b9191919191919191919191919191919b9191919191919191919191919191\
    919b9191919191919191919191919191919b9191919191919191919191919191919b91919191919191919191\
    91919191919b9191919191919191919191919191919b9191919191919191919191919191919b919191919191\
    9191919191919191919b9191919191919191919191919191919b9191919191919191919191919191919b9191\
    91919191919191919191919191bfffd9\
";

/// The same with a point transform of 2.
pub const JPEG_LOSSLESS_GRAY12_POINT_TRANSFORM: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b100010001001011100ffc4001600010101000000\
    0000000000000000000004050effda0008010100060002cfff00d1a546951a546951a546951a546dbdaf6bda\
    f6bdaf6bdafb8d7b5ed7b5ed7b5ed7b6dbdaf6bdaf6bdaf6bdafb8d7b5ed7b5ed7b5ed7b6dbdaf6bdaf6bdaf\
    6bdafb8d7b5ed7b5ed7b5ed7b6dbdaf6bdaf6bdaf6bdafb8d7b5ed7b5ed7b5ed7b6dbdaf6bdaf6bdaf6bdafb\
    8d7b5ed7b5ed7b5ed7b6dbdaf6bdaf6bdaf6bdafb8d7b5ed7b5ed7b5ed7b6dbdaf6bdaf6bdaf6bdafb8d7b5e\
    d7b5ed7b5ed7b6dbdaf6bdaf6bdaf6bdafffffd9\
";

/// The same with selection value 7.
pub const JPEG_LOSSLESS_GRAY12_SV7: &str = "\
    ffd8ffe000104a46494600010100000100010000ffc3000b100010001001011100ffc4001500010100000000\
    000000000000000000000710ffda00080101000700009191919191919191919191919191919b969696969696\
    9696969696969696969b9696969696969696969696969696969b9696969696969696969696969696969b9696\
    969696969696969696969696969b9696969696969696969696969696969b9696969696969696969696969696\
    969b9696969696969696969696969696969b9696969696969696969696969696969b96969696969696969696\
    96969696969b9696969696969696969696969696969b9696969696969696969696969696969b969696969696\
    9696969696969696969b9696969696969696969696969696969b9696969696969696969696969696969b9696\
    96969696969696969696969696bfffd9\
";

/// 16 x 16, 8-bit gray, lossless (CharLS through DCMTK).
pub const JPEGLS_GRAY8: &str = "\
    ffd8fff7000b080010001001011100ffda0008010100000000851562ad3fbb77ff706198a476666db6c31a49\
    249555302aaaaaaa5055555fa003ff7f803fff7007ff7e00ff7fc01fff7807ff7fff7c2fff7fff70bfff7fff\
    42ff7fff7f0bff7fff720bff7ffea028ffd9\
";

/// 16 x 16, 12 bits stored in 16; the codestream states 16 bits.
pub const JPEGLS_GRAY12: &str = "\
    ffd8fff7000b100010001001011100fff8000d01ffff0012004301140040ffda0008010100000000a45c8b91\
    343c8b91343d0b41e06ff7dbdde6ef3724648aa2286a130580bf5f2f578bb5d2e523181ddd9d5d1cdc9c5c1b\
    db9b5b1ada922945a19d9995918d8985817d7979757144222dad2cacac2c2bababab366ad5aa8678cd9b364c\
    98b160c182f5eb972846cb562c58ad5ab56ad52a54a95304c54a850a1428509d3a74ebacb300795555555514\
    514514514517e389a69a69a69a4924924924be1a4514514514514a5294a57a04a10842108421084210bc0211\
    8c6318c6318c6318dd83d39ce739ce739ce7777400656b5bbbbbbbbbbbb9003377777776eeeeefc0ffd9\
";

/// 16 x 16, 8-bit gray, lossless (libjxl).
pub const JXL_GRAY8: &str = "\
    ff0a434050dc080804010098004b2824c64741c214a3e8f3d6ed47d918638c8e1e13901f80ffbfb3fdff1fa9\
    aaaa529473f903\
";

/// 16 x 16, 8-bit RGB.
pub const JXL_RGB8: &str = "\
    ff0a43402408040100f0034b189bedec14141494142323237632c660ed18a363309bd8ea1883598831220633\
    80ad8831b80931183e80b3010540d05ef6aab703415dc4f31dd32b3ff111503c880ff8801a187c0022f73f00\
    4b991f803fa10cc0071ac6e083691cf03f709a01ff03610ff041f43af80034047d0034633f003fa81c800ffe\
    c1c1071df51fa89a63ab5155a355851c8d56959a3ddaaad16a879c466b85038e568db63d566d6c3cb5aad1aa\
    261e4fabc60a1bab1aadea8967b4aa3107d556a34dc851a3d58aba19ad6ab46654aaa3d52b07a3555deb4a56\
    355ad562e068d5b8806ed5c8a76cad46abba811daddab8e061d588b564d56455a5b9a355d5aa020046abaa\
";

/// 16 x 16, 16-bit gray (libjxl wrote it in the box container).
pub const JXL_GRAY16: &str = "\
    0000000c4a584c200d0a870a00000014667479706a786c20000000006a786c20000000096a786c6c0a000000\
    356a786c63ff0a43f003143702080401007c004b2824c64741c2040563679c1b63703606622076fee0110034\
    f64150019e00\
";

/// 16 x 16, 8-bit gray, in the box container.
pub const JXL_GRAY8_CONTAINER: &str = "\
    0000000c4a584c200d0a870a00000014667479706a786c20000000006a786c200000003b6a786c63ff0a4340\
    50dc080804010098004b2824c64741c214a3e8f3d6ed47d918638c8e1e13901f80ffbfb3fdff1fa9aaaa5294\
    73f903\
";

/// 8 x 8, 8-bit gray.
pub const JXL_GRAY8_8X8: &str = "\
    ff0a414050dc08080401005c004b2824c6b7151424c524e9809c00071c70c001071c7000\
";

/// 32 x 32, 8-bit gray.
pub const JXL_GRAY8_32X32: &str = "\
    ff0a474050dc08080401005c014b2824c667cd7d1514241414528c8c8c9fb7463e3d2bc620c5a818c640d452\
    c600be63c3d4898449580587cce5f9d3019e1fbee2f99e349eb75ce2f97c649edf62e479de549ee73ee2f9fa\
    11fe884e0e63cbf3e5be3c0f\
";

/// 16 x 16, 8-bit RGB with an alpha channel.
pub const JXL_RGBA8: &str = "\
    ff0a43c04a0810100038054b3841d1eb8d6096cdf7573870fbc9ad7fada030a1a0a0a0a0a4ab18191919b193\
    31066bc7181d8369178341ba153106b3c2186363f0d4180c84ad8e176330c73146c6e0a93118c6ba30c66066\
    20c6e0450b311000204e52f9e600e739e557fd37f2f53f8f67ba5ce5fc422d27ff09da913eb006023f5d1200\
    80b90f9002c107510520b4f083e730f05321001816f8008d18fc82990fae20c0ffca5c00744b7fb0170ebe72\
    11005c4a1fcc49802f2b09001cfb079f50e0c70102002af3f903c07fe1026ec5aa6a54d5cef66855a3afa955\
    8db64a926bb4aab1e2d05aa3992854a35577651bad2ab947b4aac96f74aad1aa2ae045abaa9a1bad6a745787\
    d568d5198868b526f646ab1a2d99aaea689528d768559365ab558dd62f6d355a8c54355a35da015fabd189f5\
    aa46ab9465365ad540325a35b9e84ad56855a5f5b4aa31cec9aaaa63b6dd11105121052031\
";

/// 16 x 16, two frames of an animation.
pub const JXL_ANIMATED: &str = "\
    ff0a43040158130810a0020000dc004b3841f53275fce0fb474141c12830eea2ffd8c91883b5638c8ec16c1e\
    8007a03f0081aeb94bde52f278a10bc32f80912f00e2e457fd010810a0420000c0004b18938e85832930eea2\
    ff587a31064bc7181d83f93a00907d55003f517b74c9f3985dd0fd41bbd3cde60b8038f9557f\
";

/// 2,048 x 2,048, 8-bit gray, all zero.
pub const JXL_GRAY8_2048X2048: &str = "\
    ff0afa3f01417123080201001800000040000440000440000440000440000440000440000440000440000440\
    0004400004400004400004400004400004400004400004400004400004400004400004400004400004400004\
    400004400004400004400004400004400004400004400004004b188b15820103030303030303030303030303\
    0303030303030303030303030303030303030303030303030303030303030303030303030303030303030303\
    03030303030303\
";
