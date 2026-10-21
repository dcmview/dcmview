//! Native 32-bit Pixel Data shows the same pixels in Explicit VR Big Endian
//! as in Explicit VR Little Endian.
//!
//! A value of VR OW is a stream of 16-bit words, each in the byte order of
//! the transfer syntax, and a 32-bit sample is two of them, the low word
//! first (PS3.5 section 8.2), so a big-endian file stores the sample
//! `0x11223344` as `33 44 11 22`. Float and Double Float Pixel Data (VR OF
//! and OD) hold whole 32- and 64-bit values in the byte order of the
//! transfer syntax. The fixtures under `tests/fixtures/native-word-order/`
//! hold one set of samples in each byte order. DCMTK 3.7.0 `dcmconv +tb` of
//! each little-endian fixture writes Pixel Data byte for byte equal to the
//! `be` fixture of the same name, and DCMTK reads both files of a name as
//! the same samples. Nothing here needs DCMTK.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server::{self, FileRegistry};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native-word-order")
        .join(name)
}

/// A server over `paths`, served in that order, and its registry.
async fn serve(paths: &[PathBuf]) -> (TestServer, FileRegistry) {
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
    let server = TestServer::new(server::router(support::app_state_with_registry(
        registry.clone(),
    )));
    (server, registry)
}

async fn get(server: &TestServer, path: &str) -> Vec<u8> {
    let response = server.get(path).await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "{path}: {}",
        response.text()
    );
    response.as_bytes().to_vec()
}

/// What every path that reads a frame answers for one frame of one file.
#[derive(Debug, PartialEq)]
struct Frame {
    raw: Vec<u8>,
    display: Vec<u8>,
    thumbnail: Vec<u8>,
    readout: Vec<Vec<u8>>,
    digest: Option<[u8; 32]>,
}

async fn frame(server: &TestServer, registry: &FileRegistry, file: usize, frame: u32) -> Frame {
    let at = format!("/api/file/{file}/frame/{frame}");
    let raw = get(server, &format!("{at}/raw")).await;
    let mut readout = Vec::new();
    for row in 0..2 {
        for column in 0..3 {
            readout.push(get(server, &format!("{at}/raw/pixel?row={row}&column={column}")).await);
        }
    }
    Frame {
        raw,
        display: get(server, &format!("{at}?mode=full_dynamic")).await,
        thumbnail: get(server, &format!("{at}/thumbnail?size=64")).await,
        readout,
        digest: registry.pixel_digests().get(file, frame),
    }
}

/// Sample `index` of the integer fixtures, as the generator writes it.
fn sample(index: usize) -> u32 {
    (index as u32).wrapping_mul(0x9e37_79b1).rotate_left(7) ^ 0x5a3c_96e1
}

/// Sample `index` of the float fixtures, as the generator writes it.
fn float_sample(index: usize) -> f64 {
    (index as f64 - 7.25) * 1234.5678
}

const PIXELS: usize = 6;

#[tokio::test]
async fn big_endian_thirty_two_bit_frames_equal_their_little_endian_frames() {
    let mut compared = 0;
    for (kind, mask) in [
        ("u32", u32::MAX),
        ("s32", u32::MAX),
        ("u32-stored24", 0x00ff_ffff),
    ] {
        for frames in [1_u32, 3] {
            let name = |encoding: &str| {
                fixture(&format!(
                    "golden-native-{kind}-2x3-f{frames}-{encoding}.dcm"
                ))
            };
            let (server, registry) = serve(&[name("le"), name("be")]).await;
            for index in 0..frames {
                let what = format!("{kind}, {frames} frame(s), frame {index}");
                let little = frame(&server, &registry, 0, index).await;
                let samples = (0..PIXELS)
                    .flat_map(|pixel| {
                        (sample(index as usize * PIXELS + pixel) & mask).to_le_bytes()
                    })
                    .collect::<Vec<_>>();
                assert_eq!(little.raw, samples, "{what}");
                assert!(little.digest.is_some(), "{what}");
                let big = frame(&server, &registry, 1, index).await;
                assert_eq!(big, little, "{what}");
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 12);
}

/// Float and Double Float Pixel Data are whole values in the byte order of
/// the transfer syntax, not words, and read as before.
#[tokio::test]
async fn big_endian_float_frames_equal_their_little_endian_frames() {
    for (kind, width) in [("f32", 4), ("f64", 8)] {
        let name = |encoding: &str| fixture(&format!("golden-native-{kind}-2x3-f3-{encoding}.dcm"));
        let (server, registry) = serve(&[name("le"), name("be")]).await;
        for index in 0..3 {
            let what = format!("{kind}, frame {index}");
            let little = frame(&server, &registry, 0, index).await;
            let samples = (0..PIXELS)
                .flat_map(|pixel| {
                    let value = float_sample(index as usize * PIXELS + pixel);
                    if width == 4 {
                        (value as f32).to_le_bytes().to_vec()
                    } else {
                        value.to_le_bytes().to_vec()
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(little.raw, samples, "{what}");
            let big = frame(&server, &registry, 1, index).await;
            assert_eq!(big, little, "{what}");
        }
    }
}

/// The fixture that is not conforming on purpose: Explicit VR Big Endian,
/// VR OW, the four bytes of each 32-bit sample reversed, as software writes
/// it that treats such samples as 32-bit values. Read as the words its VR
/// says it holds, each sample has its two words exchanged, as DCMTK 3.7.0
/// reads it.
#[tokio::test]
async fn reversed_samples_show_their_words_exchanged() {
    let path = fixture("golden-native-u32-2x3-f3-be-reversed-samples-nonconforming.dcm");
    let on_disk = (0..3 * PIXELS)
        .flat_map(|index| sample(index).to_be_bytes())
        .collect::<Vec<_>>();
    assert!(std::fs::read(&path)
        .expect("read fixture")
        .ends_with(&on_disk));
    let (server, _) = serve(&[path]).await;
    for index in 0..3 {
        let read = (0..PIXELS)
            .flat_map(|pixel| sample(index * PIXELS + pixel).rotate_left(16).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            get(&server, &format!("/api/file/0/frame/{index}/raw")).await,
            read,
            "frame {index}"
        );
    }
}
