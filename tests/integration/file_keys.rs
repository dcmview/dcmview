//! File keys as a client and the rest of the server see them
//! (`docs/design/annotation-model.md` 1.3, 1.7, 1.8): the key state of each
//! catalog entry, the catalog cursor, key replacement, and the hashing
//! behind `b3:` keys, counted in bytes and never in time.
//!
//! Files are found by the production loader and registered the way startup
//! registers them. Each path is scanned on its own so that indexes follow
//! the order the test names them in; a real scan is parallel and its order
//! is not fixed.
//!
//! Nothing here waits for a length of time to see that something happened:
//! a test waits on the registry's own change signal (`hashing_done`), holds
//! the decode permits hashing needs, or asks for a key. `let_tasks_run` is
//! only ever followed by assertions that something has not happened.

use super::support;
use axum::http::StatusCode;
use axum_test::TestServer;
use dcmview::keys::{FileKey, KeyFailure, KEY_HASH_SLICE_BYTES};
use dcmview::loader::{self, DiscoverOptions};
use dcmview::pixels::{DecodeClass, DecodeScheduler};
use dcmview::server::{self, FileRegistry, KeyError, KeyStats};
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{meta::FileMetaTableBuilder, InMemDicomObject};
use image::{ExtendedColorType, ImageEncoder};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Files

/// A small uncompressed 16-bit image. `uid` is its SOP Instance UID, absent
/// for `None`; `fill` is every pixel's value; `rows` sets the file's length
/// (8 columns, two bytes a pixel).
fn write_dicom(path: &Path, uid: Option<&str>, fill: u16, rows: u16) {
    write_dicom_sized(path, uid, fill, rows, 8);
}

fn write_dicom_sized(path: &Path, uid: Option<&str>, fill: u16, rows: u16, columns: u16) {
    let pixels = usize::from(rows) * usize::from(columns);
    let mut pixel_bytes = Vec::with_capacity(pixels * 2);
    for _ in 0..pixels {
        pixel_bytes.extend_from_slice(&fill.to_le_bytes());
    }
    let mut object = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("KEYS")),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(rows)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(columns)),
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
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::from(pixel_bytes)),
    ]);
    if let Some(uid) = uid {
        object.put(DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, uid));
    }
    object
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                // The file meta UID is not the instance's identity; every
                // file here carries the same one.
                .media_storage_sop_instance_uid("2.25.511000"),
        )
        .expect("build file meta")
        .write_to_file(path)
        .expect("write DICOM file");
}

fn write_png(path: &Path, shade: u8) {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[shade; 16 * 16], 16, 16, ExtendedColorType::L8)
        .expect("encode PNG");
    fs::write(path, bytes).expect("write PNG");
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).expect("file metadata").len()
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .expect("file metadata")
        .modified()
        .expect("modification time")
}

fn set_modified(path: &Path, time: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("reopen")
        .set_modified(time)
        .expect("set modification time");
}

/// The key a file's bytes hash to.
fn b3(path: &Path) -> String {
    let bytes = fs::read(path).expect("read file");
    FileKey::blake3(blake3::hash(&bytes).as_bytes())
        .as_str()
        .to_string()
}

// ---------------------------------------------------------------------------
// Sessions

/// Runs the production loader over `path` into `registry`, as startup does.
async fn scan(registry: &FileRegistry, path: &Path) {
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let paths = [path.to_path_buf()];
    let discover = loader::discover_progressive(
        &paths,
        DiscoverOptions {
            recursive: true,
            filters: Vec::new(),
            formats: Default::default(),
        },
        events_tx,
        loader::DiscoveryCancellation::new(),
    );
    let record = async {
        while let Some(event) = events_rx.recv().await {
            match event {
                loader::DiscoveryEvent::Selected { file, record } => {
                    registry.record_selected(*file, record);
                }
                loader::DiscoveryEvent::SkippedInput(record)
                | loader::DiscoveryEvent::FilteredInput(record) => {
                    registry.record_discovery(record)
                }
            }
        }
    };
    let (report, ()) = tokio::join!(discover, record);
    report.expect("discovery completes");
}

/// A server over `paths`, scanned one after another so that the file at
/// `paths[i]` has index `i`.
async fn serve(paths: &[PathBuf]) -> (TestServer, FileRegistry) {
    serve_with(FileRegistry::new(), paths).await
}

/// [`serve`] into a registry the test made, for one that hashes under a
/// decode scheduler of its own.
async fn serve_with(registry: FileRegistry, paths: &[PathBuf]) -> (TestServer, FileRegistry) {
    for path in paths {
        scan(&registry, path).await;
    }
    assert_eq!(registry.status().file_count, paths.len());
    registry.mark_scan_complete();
    let server = TestServer::new(server::router(support::app_state_with_registry(
        registry.clone(),
    )));
    (server, registry)
}

/// A registry that hashes under `scheduler`, and every permit of it held:
/// hashing queues and reads nothing until the permits are dropped.
async fn held(
    permits: usize,
) -> (
    FileRegistry,
    Arc<DecodeScheduler>,
    Vec<dcmview::pixels::DecodePermit>,
) {
    let scheduler = DecodeScheduler::new(permits);
    let registry = FileRegistry::new().with_decode_scheduler(scheduler.clone());
    let mut viewer = Vec::new();
    for _ in 0..permits {
        viewer.push(scheduler.acquire(DecodeClass::Interactive).await);
    }
    (registry, scheduler, viewer)
}

/// Returns once no file is being hashed or queued for it, woken by the
/// registry's change signal and never by a timer.
async fn hashing_done(registry: &FileRegistry) {
    loop {
        let changed = registry.changed();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if registry.files_page(None, None).keys_hashing == 0 {
            return;
        }
        changed.await;
    }
}

/// Returns once `count` files are being hashed or queued. Queueing is not
/// announced, so this lets the tasks that queue run until they have.
async fn hashing_queued(registry: &FileRegistry, count: usize) {
    while registry.files_page(None, None).keys_hashing < count {
        tokio::task::yield_now().await;
    }
}

fn key_text(registry: &FileRegistry, index: usize) -> Option<String> {
    registry
        .key_status(index)
        .expect("registered file")
        .key
        .map(|key| key.as_str().to_string())
}

async fn catalog(server: &TestServer, query: &str) -> Value {
    let response = server.get(&format!("/api/files{query}")).await;
    response.assert_status_ok();
    response.json()
}

fn entries(catalog: &Value) -> &Vec<Value> {
    catalog["files"].as_array().expect("files array")
}

/// What an entry says about its key: `Implied` when `file_key` is left out,
/// which stands for `sop:` plus the entry's `sop_instance_uid`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    Implied,
    Pending,
    Key(String),
}

fn shown(entry: &Value) -> Shown {
    match entry.get("file_key") {
        None => Shown::Implied,
        Some(Value::Null) => Shown::Pending,
        Some(Value::String(key)) => Shown::Key(key.clone()),
        Some(other) => panic!("file_key is {other}"),
    }
}

fn alias_of(entry: &Value) -> Option<u64> {
    entry.get("alias_of").map(|alias| {
        alias
            .as_u64()
            .expect("alias_of is an index when it is present")
    })
}

/// The `X-File-Key` of a display frame of the file.
async fn frame_key(server: &TestServer, index: usize) -> Option<String> {
    let response = server.get(&format!("/api/file/{index}/frame/0")).await;
    response.assert_status_ok();
    response
        .maybe_header("x-file-key")
        .map(|value| value.to_str().expect("header text").to_string())
}

// ---------------------------------------------------------------------------
// Key state

#[tokio::test]
async fn each_catalog_entry_shows_the_key_its_file_has() {
    let dir = tempfile::tempdir().expect("temp dir");
    let at = |name: &str| dir.path().join(name);
    write_dicom(
        &at("unique.dcm"),
        Some("1.2.826.0.1.3680043.10.511.1"),
        1,
        8,
    );
    write_dicom(
        &at("copied.dcm"),
        Some("1.2.826.0.1.3680043.10.511.2"),
        2,
        8,
    );
    fs::copy(at("copied.dcm"), at("copy.dcm")).expect("copy");
    write_dicom(
        &at("shared.dcm"),
        Some("1.2.826.0.1.3680043.10.511.3"),
        3,
        8,
    );
    write_dicom(
        &at("differs.dcm"),
        Some("1.2.826.0.1.3680043.10.511.3"),
        3,
        9,
    );
    write_dicom(&at("no-uid.dcm"), None, 4, 8);
    write_dicom(&at("odd-uid.dcm"), Some("1.2.3 4"), 5, 8);
    write_png(&at("image.png"), 0x40);
    let mut paths = [
        "unique.dcm",
        "copied.dcm",
        "copy.dcm",
        "shared.dcm",
        "differs.dcm",
        "no-uid.dcm",
        "odd-uid.dcm",
        "image.png",
    ]
    .map(at)
    .to_vec();
    // The first file again, by the same path: one file reached twice.
    paths.push(at("unique.dcm"));
    // And through a symbolic link, where the platform lets a test make one.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(at("unique.dcm"), at("link.dcm")).expect("symlink");
        paths.push(at("link.dcm"));
    }
    let (server, registry) = serve(&paths).await;

    // Before anything is hashed. Discovery hashed nothing.
    assert_eq!(registry.key_stats(), KeyStats::default());
    let listed = catalog(&server, "").await;
    let state = entries(&listed)
        .iter()
        .map(|entry| (shown(entry), alias_of(entry)))
        .collect::<Vec<_>>();
    let mut expected = vec![
        (Shown::Implied, None),    // unique
        (Shown::Implied, None),    // copied
        (Shown::Implied, Some(1)), // its copy: same UID, same size
        (Shown::Implied, None),    // shared: keeps its key until hashed
        (Shown::Pending, None),    // differs: same UID, another size
        (Shown::Pending, None),    // no UID
        (Shown::Pending, None),    // a UID that cannot be a key
        (Shown::Pending, None),    // raster
        (Shown::Implied, Some(0)), // the first file again
    ];
    if cfg!(unix) {
        expected.push((Shown::Implied, Some(0))); // and through a link
    }
    assert_eq!(state, expected);
    for entry in entries(&listed) {
        assert!(entry.get("key_error").is_none());
    }
    assert_eq!(listed["keys_hashing"], 0);

    // Asked for, every key settles. What is read is exactly the files whose
    // key needs their bytes: both files of the copied pair, the two that
    // share a UID, and the three without one. The first file and its two
    // other paths are never read.
    let mut keys = Vec::new();
    for index in 0..paths.len() {
        keys.push(
            registry
                .ensure_key(index)
                .await
                .unwrap_or_else(|error| panic!("key of file {index}: {error}"))
                .as_str()
                .to_string(),
        );
    }
    let mut expected_keys = vec![
        "sop:1.2.826.0.1.3680043.10.511.1".to_string(),
        "sop:1.2.826.0.1.3680043.10.511.2".to_string(),
        "sop:1.2.826.0.1.3680043.10.511.2".to_string(),
        b3(&at("shared.dcm")),
        b3(&at("differs.dcm")),
        b3(&at("no-uid.dcm")),
        b3(&at("odd-uid.dcm")),
        b3(&at("image.png")),
        "sop:1.2.826.0.1.3680043.10.511.1".to_string(),
    ];
    if cfg!(unix) {
        expected_keys.push("sop:1.2.826.0.1.3680043.10.511.1".to_string());
    }
    assert_eq!(keys, expected_keys);
    let hashed = [
        "copied.dcm",
        "copy.dcm",
        "shared.dcm",
        "differs.dcm",
        "no-uid.dcm",
        "odd-uid.dcm",
        "image.png",
    ];
    let stats = registry.key_stats();
    assert_eq!(stats.files_hashed, hashed.len() as u64);
    assert_eq!(
        stats.bytes_hashed,
        hashed.iter().map(|name| size(&at(name))).sum::<u64>()
    );

    // The catalog now shows the keys, in full where the UID does not imply
    // them, and the frame header carries the same key.
    let listed = catalog(&server, "").await;
    for (index, entry) in entries(&listed).iter().enumerate() {
        let full = match shown(entry) {
            Shown::Implied => format!(
                "sop:{}",
                entry["sop_instance_uid"].as_str().expect("UID text")
            ),
            Shown::Key(key) => key,
            Shown::Pending => panic!("file {index} still has no key"),
        };
        assert_eq!(full, expected_keys[index], "file {index}");
        // Rasters have no decoder on this path; DICOM frames carry the key.
        if entry["file_format"] == "dicom" {
            assert_eq!(
                frame_key(&server, index).await.as_deref(),
                Some(full.as_str()),
                "file {index}"
            );
        }
        assert_eq!(
            registry.file_for_shown_key(&full),
            Some(
                expected_keys
                    .iter()
                    .position(|key| *key == full)
                    .expect("first holder")
            ),
            "the key of file {index} names the first file that held it"
        );
    }
    assert_eq!(registry.file_for_shown_key("sop:1.2.3"), None);
    assert_eq!(registry.file_for_shown_key("not a key"), None);
    // Settled keys are not hashed again.
    registry.ensure_key(4).await.expect("settled key");
    assert_eq!(registry.key_stats(), stats);
    assert_eq!(registry.ensure_key(99).await, Err(KeyError::NotFound(99)));
}

#[tokio::test]
async fn viewing_files_hashes_only_those_without_a_key() {
    const UNIQUE: usize = 24;
    let dir = tempfile::tempdir().expect("temp dir");
    fs::create_dir(dir.path().join("all")).expect("create dir");
    fs::create_dir(dir.path().join("train")).expect("create dir");
    let mut paths = Vec::new();
    for number in 0..UNIQUE {
        let path = dir.path().join("all").join(format!("{number:02}.dcm"));
        write_dicom(
            &path,
            Some(&format!("1.2.826.0.1.3680043.10.512.{number}")),
            number as u16,
            8,
        );
        paths.push(path);
    }
    // A byte-identical copy of the whole folder, as `train/` beside `all/`.
    for number in 0..UNIQUE {
        let path = dir.path().join("train").join(format!("{number:02}.dcm"));
        fs::copy(&paths[number], &path).expect("copy");
        paths.push(path);
    }
    let pending = dir.path().join("no-uid.dcm");
    write_dicom(&pending, None, 7, 8);
    paths.push(pending.clone());
    let pending_index = paths.len() - 1;
    let (server, registry) = serve(&paths).await;

    // Every file is opened in the viewer.
    for index in 0..paths.len() {
        let key = frame_key(&server, index).await;
        if index == pending_index {
            assert_eq!(key, None, "a file without a key sends no key header");
        } else {
            assert_eq!(
                key,
                Some(format!("sop:1.2.826.0.1.3680043.10.512.{}", index % UNIQUE))
            );
        }
    }
    // The one file without a key is hashed in the background once its frame
    // was served; waiting for its key waits for exactly that.
    let key = registry.ensure_key(pending_index).await.expect("key");
    assert_eq!(key.as_str(), b3(&pending));
    let stats = registry.key_stats();
    assert_eq!(
        (stats.files_hashed, stats.bytes_hashed),
        (1, size(&pending)),
        "48 files with a UID key cost no read, copies included"
    );

    // The next frame of that file carries the key, and nothing is rehashed.
    assert_eq!(frame_key(&server, pending_index).await, Some(b3(&pending)));
    let raw = server
        .get(&format!("/api/file/{pending_index}/frame/0/raw"))
        .await;
    raw.assert_status_ok();
    assert_eq!(raw.header("x-file-key"), b3(&pending).as_str());
    assert_eq!(registry.key_stats(), stats);
    let listed = catalog(&server, "").await;
    assert_eq!(
        shown(&entries(&listed)[pending_index]),
        Shown::Key(b3(&pending))
    );
    assert_eq!(listed["keys_hashing"], 0);
}

#[tokio::test]
async fn a_changed_or_missing_file_has_no_key_until_it_can_be_read() {
    let dir = tempfile::tempdir().expect("temp dir");
    let changed = dir.path().join("changed.dcm");
    let missing = dir.path().join("missing.dcm");
    write_dicom(&changed, None, 1, 8);
    write_dicom(&missing, None, 2, 8);
    let (server, registry) = serve(&[changed.clone(), missing.clone()]).await;

    let original = fs::read(&changed).expect("read");
    let seen = modified(&changed);
    let mut longer = original.clone();
    longer.extend_from_slice(&[0; 4096]);
    fs::write(&changed, &longer).expect("grow the file");
    fs::remove_file(&missing).expect("remove the file");

    assert_eq!(
        registry.ensure_key(0).await,
        Err(KeyError::Unavailable(KeyFailure::Changed))
    );
    assert_eq!(
        registry.ensure_key(1).await,
        Err(KeyError::Unavailable(KeyFailure::Unreadable))
    );
    assert_eq!(
        registry.key_stats().bytes_hashed,
        0,
        "a file of another length is refused before it is read"
    );
    let listed = catalog(&server, "").await;
    let state = entries(&listed)
        .iter()
        .map(|entry| (shown(entry), entry["key_error"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        state,
        vec![
            (Shown::Pending, Value::from("changed")),
            (Shown::Pending, Value::from("unreadable")),
        ]
    );

    // The file is as discovery saw it again, its modification time
    // included: asking retries, and succeeds.
    fs::write(&changed, &original).expect("restore the file");
    set_modified(&changed, seen);
    let key = registry.ensure_key(0).await.expect("key after restore");
    assert_eq!(key.as_str(), b3(&changed));
    let listed = catalog(&server, "").await;
    assert_eq!(shown(&entries(&listed)[0]), Shown::Key(b3(&changed)));
    assert!(entries(&listed)[0].get("key_error").is_none());
}

/// Discovery reads a header and a `stat`. A file replaced afterwards by
/// other bytes of the same length is not the file the catalog lists, and
/// only its modification time says so.
#[tokio::test]
async fn a_file_rewritten_at_the_same_length_after_discovery_has_no_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rewritten.dcm");
    write_dicom(&path, None, 1, 8);
    let original = fs::read(&path).expect("read");
    let seen = modified(&path);
    let (server, registry) = serve(std::slice::from_ref(&path)).await;

    // Other pixels, the same length, written after discovery. The time is
    // set outright so the test does not depend on the filesystem's tick.
    write_dicom(&path, None, 0x7777, 8);
    assert_eq!(size(&path), original.len() as u64);
    assert_ne!(fs::read(&path).expect("read"), original);
    set_modified(&path, seen + Duration::from_secs(10));

    assert_eq!(
        registry.ensure_key(0).await,
        Err(KeyError::Unavailable(KeyFailure::Changed)),
        "the bytes on disk are not the file discovery saw"
    );
    assert_eq!(
        registry.key_stats().bytes_hashed,
        0,
        "refused from its modification time, before any read"
    );
    let listed = catalog(&server, "").await;
    assert_eq!(
        (
            shown(&entries(&listed)[0]),
            entries(&listed)[0]["key_error"].clone()
        ),
        (Shown::Pending, Value::from("changed"))
    );

    // As discovery saw it again: the key is the one of the original bytes.
    fs::write(&path, &original).expect("restore the bytes");
    set_modified(&path, seen);
    let key = registry.ensure_key(0).await.expect("key after restore");
    assert_eq!(
        key.as_str(),
        FileKey::blake3(blake3::hash(&original).as_bytes()).as_str()
    );
}

// ---------------------------------------------------------------------------
// Keys that are relied on

/// Every order of `0..count`.
fn orders(count: usize) -> Vec<Vec<usize>> {
    if count == 0 {
        return vec![Vec::new()];
    }
    let mut all = Vec::new();
    for shorter in orders(count - 1) {
        for position in 0..=shorter.len() {
            let mut order = shorter.clone();
            order.insert(position, count - 1);
            all.push(order);
        }
    }
    all
}

/// `docs/design/annotation-model.md` 1.7: a key shared by UID and size is
/// verified "before the first annotation write on either file". Whichever
/// file of such a group is asked about first, in whatever order the files
/// were found, the key `ensure_key` returns is the one the file ends the
/// session with; two files given one key hold the same bytes; and every
/// file of the group was read once to know it.
#[tokio::test]
async fn a_key_that_was_asked_for_is_final_whatever_order_files_are_found_and_asked_in() {
    const UID: &str = "1.2.826.0.1.3680043.10.516.1";
    let dir = tempfile::tempdir().expect("temp dir");
    let at = |name: &str| dir.path().join(name);
    write_dicom(&at("a.dcm"), Some(UID), 1, 8);
    write_dicom(&at("b.dcm"), Some(UID), 2, 8);
    write_dicom(&at("c.dcm"), Some(UID), 3, 8);
    fs::copy(at("a.dcm"), at("a-copy.dcm")).expect("copy");
    fs::copy(at("a.dcm"), at("a-copy-2.dcm")).expect("copy");
    assert_eq!(size(&at("a.dcm")), size(&at("b.dcm")));
    assert_eq!(size(&at("a.dcm")), size(&at("c.dcm")));

    // Each group is one UID and one size.
    let groups: Vec<(&str, Vec<&str>)> = vec![
        ("two copies", vec!["a.dcm", "a-copy.dcm"]),
        ("two files that differ", vec!["a.dcm", "b.dcm"]),
        (
            "two copies and one that differs",
            vec!["a.dcm", "b.dcm", "a-copy.dcm"],
        ),
        ("three copies", vec!["a.dcm", "a-copy.dcm", "a-copy-2.dcm"]),
        ("three that differ", vec!["a.dcm", "b.dcm", "c.dcm"]),
    ];
    for (name, group) in groups {
        let identical = group.iter().all(|file| b3(&at(file)) == b3(&at(group[0])));
        let group_bytes = group.iter().map(|file| size(&at(file))).sum::<u64>();
        for order in orders(group.len()) {
            let paths = order
                .iter()
                .map(|&position| at(group[position]))
                .collect::<Vec<_>>();
            let expected = paths
                .iter()
                .map(|path| {
                    if identical {
                        format!("sop:{UID}")
                    } else {
                        b3(path)
                    }
                })
                .collect::<Vec<_>>();
            for first_asked in 0..group.len() {
                let context = format!("{name}, found {order:?}, asked {first_asked} first");
                let (server, registry) = serve(&paths).await;
                // Found, listed, and nothing read.
                assert_eq!(registry.key_stats(), KeyStats::default(), "{context}");

                let key = registry
                    .ensure_key(first_asked)
                    .await
                    .unwrap_or_else(|error| panic!("{context}: {error}"));
                assert_eq!(key.as_str(), expected[first_asked], "{context}");
                // A key the files share is returned only when every one of
                // them was read. A key of the file's own content is known as
                // soon as one file is seen to differ.
                let stats = registry.key_stats();
                if identical {
                    assert_eq!(
                        (stats.files_hashed, stats.bytes_hashed),
                        (group.len() as u64, group_bytes),
                        "{context}"
                    );
                } else {
                    assert!(
                        (2..=group.len() as u64).contains(&stats.files_hashed),
                        "{context}: {stats:?}"
                    );
                }
                let returned_at = registry.files_page(None, None).revision;
                let named = registry
                    .file_for_shown_key(key.as_str())
                    .expect("the key names a file");
                assert_eq!(
                    b3(&paths[named]),
                    b3(&paths[first_asked]),
                    "{context}: the key names a file with the same bytes"
                );

                // Each file ends with the key of its content, or the UID key
                // when all agree, and the whole group cost one read of each
                // file however many keys were asked for.
                for (index, expected) in expected.iter().enumerate() {
                    let key = registry.ensure_key(index).await.expect("key");
                    assert_eq!(key.as_str(), expected, "{context}: file {index}");
                }
                hashing_done(&registry).await;
                let stats = registry.key_stats();
                assert_eq!(
                    (stats.files_hashed, stats.bytes_hashed),
                    (group.len() as u64, group_bytes),
                    "{context}: every file is read once"
                );
                for (index, expected) in expected.iter().enumerate() {
                    let status = registry.key_status(index).expect("registered");
                    assert_eq!(
                        (status.key.as_ref().map(FileKey::as_str), status.settled),
                        (Some(expected.as_str()), true),
                        "{context}: file {index}"
                    );
                    assert_eq!(
                        frame_key(&server, index).await.as_deref(),
                        Some(expected.as_str()),
                        "{context}: file {index}"
                    );
                }
                // The key that was returned is not replaced afterwards: a
                // replacement the catalog logged for that file happened
                // before it was returned. No file's key is replaced twice.
                let page = registry.files_page(Some(0), None);
                let mut replaced = page
                    .rekeys
                    .iter()
                    .map(|rekey| rekey.index)
                    .collect::<Vec<_>>();
                replaced.sort_unstable();
                replaced.dedup();
                assert_eq!(
                    replaced.len(),
                    page.rekeys.len(),
                    "{context}: {:?}",
                    page.rekeys
                );
                assert!(
                    page.rekeys
                        .iter()
                        .all(|rekey| rekey.index != first_asked || rekey.revision <= returned_at),
                    "{context}: {:?}",
                    page.rekeys
                );
                assert_eq!(
                    registry.file_for_shown_key(key.as_str()),
                    Some(named),
                    "{context}: the key still names the same file"
                );
            }
        }
    }
}

/// What the late file gets is a decision the design leaves open, and the
/// owner's to confirm: 1.7 would rekey the earlier file ("or even
/// annotated"); here the key that was returned stands, the late file has no
/// key until it was compared with the first file, and it then shares the UID
/// key or takes a content key of its own.
#[tokio::test]
async fn a_file_found_after_a_key_was_returned_never_changes_that_key() {
    const UID: &str = "1.2.826.0.1.3680043.10.517.1";
    let dir = tempfile::tempdir().expect("temp dir");
    let first = dir.path().join("first.dcm");
    write_dicom(&first, Some(UID), 1, 8);
    let copy = dir.path().join("copy.dcm");
    fs::copy(&first, &copy).expect("copy");
    let same_size = dir.path().join("same-size.dcm");
    write_dicom(&same_size, Some(UID), 2, 8);
    let other_size = dir.path().join("other-size.dcm");
    write_dicom(&other_size, Some(UID), 1, 9);

    // (late file, whether it holds the first file's bytes, whether the
    // first file has to be read to know)
    for (name, late, same_bytes, compares) in [
        ("a copy", &copy, true, true),
        ("the same size, other bytes", &same_size, false, true),
        ("another size", &other_size, false, false),
    ] {
        for viewed in [false, true] {
            let context = format!("{name}, viewed first: {viewed}");
            let (server, registry) = serve(std::slice::from_ref(&first)).await;
            // The only file with its UID: its key is returned at once, and
            // not a byte is read for it.
            let key = registry.ensure_key(0).await.expect("key");
            assert_eq!(key.as_str(), format!("sop:{UID}"), "{context}");
            assert_eq!(registry.key_stats(), KeyStats::default(), "{context}");
            let since = registry.files_page(None, None).revision;

            // Discovery then finds a second file with the UID.
            scan(&registry, late).await;
            let listed = catalog(&server, "").await;
            assert_eq!(
                (shown(&entries(&listed)[0]), shown(&entries(&listed)[1])),
                (Shown::Implied, Shown::Pending),
                "{context}: the late file waits, the first shows what it showed"
            );
            assert_eq!(alias_of(&entries(&listed)[1]), None, "{context}");

            let expected = if same_bytes {
                format!("sop:{UID}")
            } else {
                b3(late)
            };
            let read = size(late) + if compares { size(&first) } else { 0 };
            if viewed {
                // Viewing it is enough: what its comparison needs is read
                // in the background, the first file included.
                assert_eq!(frame_key(&server, 1).await, None, "{context}");
                hashing_done(&registry).await;
                assert_eq!(registry.key_stats().bytes_hashed, read, "{context}");
                assert_eq!(key_text(&registry, 1), Some(expected.clone()), "{context}");
            }
            let late_key = registry.ensure_key(1).await.expect("late key");
            assert_eq!(late_key.as_str(), expected, "{context}");
            assert_eq!(
                registry.key_stats().bytes_hashed,
                read,
                "{context}: the late file and what it is compared with, each once"
            );

            // The key that was returned is the first file's still, was not
            // replaced, and names the first file.
            assert_eq!(registry.ensure_key(0).await, Ok(key.clone()), "{context}");
            assert_eq!(
                frame_key(&server, 0).await,
                Some(format!("sop:{UID}")),
                "{context}"
            );
            assert_eq!(
                registry.file_for_shown_key(&format!("sop:{UID}")),
                Some(0),
                "{context}"
            );
            let after = catalog(&server, &format!("?since={since}")).await;
            assert_eq!(
                after["rekeys"],
                Value::Array(Vec::new()),
                "{context}: no key of the group is replaced"
            );
            let listed = catalog(&server, "").await;
            assert_eq!(shown(&entries(&listed)[0]), Shown::Implied, "{context}");
            assert_eq!(
                (shown(&entries(&listed)[1]), alias_of(&entries(&listed)[1])),
                if same_bytes {
                    (Shown::Implied, Some(0))
                } else {
                    (Shown::Key(expected.clone()), None)
                },
                "{context}"
            );
        }
    }
}

#[tokio::test]
async fn a_file_that_cannot_be_read_fails_only_the_keys_that_depend_on_it() {
    const UID: &str = "1.2.826.0.1.3680043.10.518.1";
    /// What asking for the key gives.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Answer {
        /// The key the copies share.
        Shared,
        /// No key: the file's own bytes cannot be read.
        None,
        /// The file's own content key: the first file, whose bytes the
        /// shared key would name, cannot be read.
        Content,
    }
    // (which file of three copies is gone, the file asked about, the answer)
    for (gone, asked, answer) in [
        (2_usize, 0_usize, Answer::Shared),
        (1, 2, Answer::Shared),
        (2, 2, Answer::None),
        (0, 0, Answer::None),
        (0, 1, Answer::Content),
    ] {
        let context = format!("file {gone} gone, file {asked} asked");
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = ["first.dcm", "copy.dcm", "copy-2.dcm"]
            .map(|name| dir.path().join(name))
            .to_vec();
        write_dicom(&paths[0], Some(UID), 1, 8);
        fs::copy(&paths[0], &paths[1]).expect("copy");
        fs::copy(&paths[0], &paths[2]).expect("copy");
        let content = b3(&paths[1]);
        let (server, registry) = serve(&paths).await;
        fs::remove_file(&paths[gone]).expect("remove a file");

        let key = registry.ensure_key(asked).await;
        match answer {
            Answer::Shared => {
                assert_eq!(
                    key.as_ref().map(FileKey::as_str),
                    Ok(format!("sop:{UID}").as_str()),
                    "{context}"
                );
                // The file that could not be read no longer shows the key
                // the others were given: nothing says it is the same image.
                let listed = catalog(&server, "").await;
                for (index, entry) in entries(&listed).iter().enumerate() {
                    let state = (shown(entry), entry.get("key_error").cloned());
                    if index == gone {
                        assert_eq!(
                            state,
                            (Shown::Pending, Some(Value::from("unreadable"))),
                            "{context}: file {index}"
                        );
                    } else {
                        assert_eq!(state, (Shown::Implied, None), "{context}: file {index}");
                    }
                }
                assert_eq!(
                    registry.ensure_key(gone).await,
                    Err(KeyError::Unavailable(KeyFailure::Unreadable)),
                    "{context}"
                );
                assert_eq!(registry.ensure_key(asked).await, key, "{context}");
            }
            Answer::None => {
                // Its own bytes cannot be read: no key, and nothing shown
                // changes but the failure.
                assert_eq!(
                    key,
                    Err(KeyError::Unavailable(KeyFailure::Unreadable)),
                    "{context}"
                );
                let listed = catalog(&server, "").await;
                for (index, entry) in entries(&listed).iter().enumerate() {
                    assert_eq!(shown(entry), Shown::Implied, "{context}: file {index}");
                    assert_eq!(
                        entry.get("key_error").is_some(),
                        index == gone,
                        "{context}: file {index}"
                    );
                }
            }
            Answer::Content => {
                // The first file is gone, so no file can be said to hold
                // its bytes: the files that can be read are keyed by their
                // own, and the first file has no key.
                assert_eq!(
                    key.as_ref().map(FileKey::as_str),
                    Ok(content.as_str()),
                    "{context}"
                );
                for index in 1..paths.len() {
                    assert_eq!(
                        registry
                            .ensure_key(index)
                            .await
                            .as_ref()
                            .map(FileKey::as_str),
                        Ok(content.as_str()),
                        "{context}: file {index}"
                    );
                }
                assert_eq!(
                    registry.ensure_key(gone).await,
                    Err(KeyError::Unavailable(KeyFailure::Unreadable)),
                    "{context}"
                );
                let listed = catalog(&server, "").await;
                let states = entries(&listed)
                    .iter()
                    .map(|entry| {
                        (
                            shown(entry),
                            alias_of(entry),
                            entry.get("key_error").cloned(),
                        )
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    states,
                    vec![
                        (Shown::Pending, None, Some(Value::from("unreadable"))),
                        (Shown::Key(content.clone()), None, None),
                        (Shown::Key(content.clone()), Some(1), None),
                    ],
                    "{context}"
                );
                // Each replaced key reached the client once, and the old
                // key still names the file it named.
                let rekeys = listed["rekeys"].as_array().expect("rekeys");
                assert_eq!(
                    rekeys
                        .iter()
                        .map(|rekey| rekey["index"].as_u64())
                        .collect::<Vec<_>>(),
                    vec![Some(1), Some(2)],
                    "{context}"
                );
                assert_eq!(
                    registry.file_for_shown_key(&format!("sop:{UID}")),
                    Some(0),
                    "{context}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key replacement

#[tokio::test]
async fn a_replaced_key_reaches_the_client_as_one_event() {
    const UID: &str = "1.2.826.0.1.3680043.10.513.1";
    let dir = tempfile::tempdir().expect("temp dir");
    let first = dir.path().join("first.dcm");
    let second = dir.path().join("second.dcm");
    write_dicom(&first, Some(UID), 1, 8);
    write_dicom(&second, Some(UID), 1, 9);

    // The first file is found, listed and viewed under its UID key.
    let (server, registry) = serve(std::slice::from_ref(&first)).await;
    let before = catalog(&server, "").await;
    assert_eq!(shown(&entries(&before)[0]), Shown::Implied);
    assert_eq!(before["rekeys"], Value::Array(Vec::new()));
    let since = before["revision"].as_u64().expect("revision");
    assert_eq!(frame_key(&server, 0).await, Some(format!("sop:{UID}")));

    // Discovery then finds a second file with the UID and other bytes. The
    // first was served, so it is hashed without anyone asking.
    scan(&registry, &second).await;
    let key = registry.ensure_key(0).await.expect("replacement key");
    assert_eq!(key.as_str(), b3(&first));

    let after = catalog(&server, &format!("?since={since}")).await;
    let revision = after["revision"].as_u64().expect("revision");
    assert!(revision > since);
    assert_eq!(after["reset"], false);
    let rekeys = after["rekeys"].as_array().expect("rekeys");
    assert_eq!(rekeys.len(), 1, "{rekeys:?}");
    assert_eq!(rekeys[0]["index"], 0);
    assert_eq!(rekeys[0]["old_key"], format!("sop:{UID}"));
    assert_eq!(rekeys[0]["new_key"], b3(&first));
    let at = rekeys[0]["revision"].as_u64().expect("rekey revision");
    assert!(since < at && at <= revision);
    // The same response carries both changed entries as they are now.
    let changed = entries(&after)
        .iter()
        .map(|entry| (entry["index"].as_u64().expect("index"), shown(entry)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        changed,
        BTreeMap::from([(0, Shown::Key(b3(&first))), (1, Shown::Pending)])
    );
    assert_eq!(frame_key(&server, 0).await, Some(b3(&first)));
    // An operation still in flight under the old key finds its file.
    assert_eq!(registry.file_for_shown_key(&format!("sop:{UID}")), Some(0));
    assert_eq!(registry.file_for_shown_key(&b3(&first)), Some(0));

    // The second file's first key replaces nothing, and a client that is up
    // to date is told nothing twice.
    registry.ensure_key(1).await.expect("second key");
    let later = catalog(&server, &format!("?since={revision}")).await;
    assert_eq!(later["rekeys"], Value::Array(Vec::new()));
    assert_eq!(entries(&later).len(), 1);
    assert_eq!(shown(&entries(&later)[0]), Shown::Key(b3(&second)));
    let all = catalog(&server, "").await;
    assert_eq!(all["rekeys"].as_array().expect("rekeys").len(), 1);
    let done = catalog(&server, &format!("?since={}", all["revision"])).await;
    assert!(entries(&done).is_empty());
    assert_eq!(done["revision"], all["revision"]);
}

// ---------------------------------------------------------------------------
// The catalog cursor

/// Applies one page to a client's copy of the catalog, by index.
fn apply(client: &mut BTreeMap<u64, Value>, page: &Value) {
    for entry in entries(page) {
        client.insert(entry["index"].as_u64().expect("index"), entry.clone());
    }
}

#[tokio::test]
async fn paging_ends_with_the_current_catalog_while_it_grows_and_keys_change() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut paths = Vec::new();
    for number in 0..5_u16 {
        let path = dir.path().join(format!("{number}.dcm"));
        let uid = format!("1.2.826.0.1.3680043.10.514.{number}");
        // Files 1 and 3 have no UID, so their keys can change later.
        write_dicom(&path, (number % 2 == 0).then_some(uid.as_str()), number, 8);
        paths.push(path);
    }
    let (server, registry) = serve(&paths).await;

    // A plain listing is every entry in index order.
    let plain = catalog(&server, "").await;
    assert_eq!(
        entries(&plain)
            .iter()
            .map(|entry| entry["index"].as_u64().expect("index"))
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        (&plain["reset"], &plain["more"]),
        (&false.into(), &false.into())
    );

    let mut client = BTreeMap::new();
    let mut listed = Vec::new();
    let mut since = 0;
    let mut pages = 0;
    loop {
        let page = catalog(&server, &format!("?since={since}&limit=2")).await;
        let revision = page["revision"].as_u64().expect("revision");
        assert!(entries(&page).len() <= 2);
        assert_eq!(page["reset"], false);
        listed.extend(
            entries(&page)
                .iter()
                .map(|entry| entry["index"].as_u64().expect("index")),
        );
        apply(&mut client, &page);
        pages += 1;
        if pages == 1 {
            assert_eq!(listed, vec![0, 1], "oldest change first");
            assert_eq!(page["more"], true);
            // Between two pages: an entry already delivered gains its key,
            // and discovery finds two more files.
            registry.ensure_key(1).await.expect("key");
            for number in 5..7_u16 {
                let path = dir.path().join(format!("{number}.dcm"));
                write_dicom(
                    &path,
                    Some(&format!("1.2.826.0.1.3680043.10.514.{number}")),
                    number,
                    8,
                );
                scan(&registry, &path).await;
                paths.push(path);
            }
        }
        if page["more"] == false {
            // The last page is complete up to the catalog's revision.
            assert_eq!(page["revision"], catalog(&server, "").await["revision"]);
            break;
        }
        assert!(revision > since, "a page that ends early moves the cursor");
        assert!(pages < 20, "paging does not end");
        since = revision;
    }
    assert_eq!(
        listed,
        vec![0, 1, 2, 3, 4, 1, 5, 6],
        "the entry that changed is listed again, after what was unchanged"
    );
    let current = catalog(&server, "").await;
    assert_eq!(
        client.into_values().collect::<Vec<_>>(),
        *entries(&current),
        "the pages add up to the catalog as it is now"
    );

    // Up to date: nothing more, at any page size.
    let revision = current["revision"].as_u64().expect("revision");
    for query in [
        format!("?since={revision}"),
        format!("?since={revision}&limit=1"),
        format!("?since={revision}&limit=4294967295"),
    ] {
        let page = catalog(&server, &query).await;
        assert!(entries(&page).is_empty(), "{query}");
        assert_eq!(
            (&page["revision"], &page["more"], &page["reset"]),
            (&current["revision"], &false.into(), &false.into()),
            "{query}"
        );
    }

    // A cursor from another process is told to start over, and is given
    // everything.
    for since in [revision + 1, u64::MAX] {
        let page = catalog(&server, &format!("?since={since}")).await;
        assert_eq!(page["reset"], true, "since={since}");
        assert_eq!(page["revision"], current["revision"]);
        let mut restarted = BTreeMap::new();
        apply(&mut restarted, &page);
        assert_eq!(
            restarted.into_values().collect::<Vec<_>>(),
            *entries(&current)
        );
    }
    let page = catalog(&server, &format!("?since={}&limit=3", u64::MAX)).await;
    assert_eq!(
        (&page["reset"], &page["more"]),
        (&true.into(), &true.into())
    );
    assert_eq!(entries(&page).len(), 3);

    // Malformed cursors are refused, not guessed at.
    for query in [
        "?limit=0",
        "?since=-1",
        "?since=abc",
        "?limit=many",
        "?since=18446744073709551616",
        "?limit=4294967296",
    ] {
        let response = server.get(&format!("/api/files{query}")).await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["code"], "invalid_query", "{query}");
    }
}

/// One event can change several entries: a file that splits a group takes
/// the alias from every copy at once. Each changed entry has a revision of
/// its own, so a client that pages one entry at a time is handed every one
/// of them and can stop after any.
#[tokio::test]
async fn entries_changed_by_one_event_are_paged_one_at_a_time_without_loss() {
    const UID: &str = "1.2.826.0.1.3680043.10.519.1";
    const COPIES: usize = 4;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut paths = vec![dir.path().join("0.dcm")];
    write_dicom(&paths[0], Some(UID), 1, 8);
    for number in 1..=COPIES {
        let path = dir.path().join(format!("{number}.dcm"));
        fs::copy(&paths[0], &path).expect("copy");
        paths.push(path);
    }
    let (server, registry) = serve(&paths).await;
    let before = catalog(&server, "").await;
    for entry in &entries(&before)[1..] {
        assert_eq!(alias_of(entry), Some(0));
    }
    let since = before["revision"].as_u64().expect("revision");

    // A file with the UID and another length: the copies are no longer
    // known to be one image, all in one registration.
    let other = dir.path().join("other.dcm");
    write_dicom(&other, Some(UID), 1, 9);
    scan(&registry, &other).await;
    let current = catalog(&server, "").await;
    assert_eq!(
        current["revision"].as_u64().expect("revision"),
        since + COPIES as u64 + 1,
        "one revision for each entry the event changed"
    );

    let mut client = entries(&before)
        .iter()
        .map(|entry| (entry["index"].as_u64().expect("index"), entry.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut listed = Vec::new();
    let mut cursor = since;
    loop {
        let page = catalog(&server, &format!("?since={cursor}&limit=1")).await;
        let revision = page["revision"].as_u64().expect("revision");
        apply(&mut client, &page);
        listed.extend(
            entries(&page)
                .iter()
                .map(|entry| entry["index"].as_u64().expect("index")),
        );
        if page["more"] == false {
            assert_eq!(revision, since + COPIES as u64 + 1);
            break;
        }
        assert_eq!(entries(&page).len(), 1);
        assert_eq!(
            revision,
            cursor + 1,
            "each page moves the cursor by one entry"
        );
        assert!(listed.len() <= COPIES + 1, "paging does not end");
        cursor = revision;
    }
    assert_eq!(
        listed,
        (1..=COPIES as u64 + 1).collect::<Vec<_>>(),
        "every changed entry, each once, in the order it changed"
    );
    assert_eq!(
        client.into_values().collect::<Vec<_>>(),
        *entries(&current),
        "the pages add up to the catalog as it is now"
    );
    for entry in &entries(&current)[1..=COPIES] {
        assert_eq!(alias_of(entry), None);
    }
}

// ---------------------------------------------------------------------------
// Background hashing

/// A file without a UID a little over two and a half hashing slices long.
fn write_large_dicom(path: &Path) -> u64 {
    // 2560 x 4096 16-bit pixels are 20,971,520 bytes: two and a half slices.
    write_dicom_sized(path, None, 0x0123, 2560, 4096);
    let len = size(path);
    assert_eq!(len.div_ceil(KEY_HASH_SLICE_BYTES), 3);
    len
}

/// Lets tasks that are able to run do so, without asserting on how long
/// anything takes: the assertions after it are that hashing has not started,
/// which more waiting could only confirm.
async fn let_tasks_run() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashing_takes_background_permits_one_slice_at_a_time() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("large.dcm");
    let len = write_large_dicom(&path);
    let scheduler = DecodeScheduler::new(2);
    let registry = FileRegistry::new().with_decode_scheduler(scheduler.clone());
    scan(&registry, &path).await;

    // The viewer holds every permit: hashing waits and reads nothing.
    let viewer = [
        scheduler.acquire(DecodeClass::Interactive).await,
        scheduler.acquire(DecodeClass::Interactive).await,
    ];
    let waiting = tokio::spawn({
        let registry = registry.clone();
        async move { registry.ensure_key(0).await }
    });
    let_tasks_run().await;
    assert_eq!(registry.key_stats(), KeyStats::default());
    let listed = registry.files_page(None, None);
    assert_eq!(listed.keys_hashing, 1, "the queued file is counted");

    // The viewer is done: the file is hashed in three slices.
    drop(viewer);
    let key = waiting.await.expect("task").expect("key");
    assert_eq!(key.as_str(), b3(&path));
    assert_eq!(
        registry.key_stats(),
        KeyStats {
            files_hashed: 1,
            bytes_hashed: len,
            slices: 3,
        }
    );
    assert_eq!(registry.files_page(None, None).keys_hashing, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_key_work_reads_nothing_more_and_records_no_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("large.dcm");
    write_large_dicom(&path);
    let scheduler = DecodeScheduler::new(2);
    let registry = FileRegistry::new().with_decode_scheduler(scheduler.clone());
    scan(&registry, &path).await;

    let viewer = [
        scheduler.acquire(DecodeClass::Interactive).await,
        scheduler.acquire(DecodeClass::Interactive).await,
    ];
    let waiting = tokio::spawn({
        let registry = registry.clone();
        async move { registry.ensure_key(0).await }
    });
    let_tasks_run().await;
    registry.stop_key_work();
    registry.stop_key_work();
    drop(viewer);

    assert_eq!(waiting.await.expect("task"), Err(KeyError::Stopped));
    let_tasks_run().await;
    assert_eq!(registry.key_stats().bytes_hashed, 0);
    let status = registry.key_status(0).expect("registered");
    assert_eq!((status.key, status.failure), (None, None));
    assert_eq!(registry.ensure_key(0).await, Err(KeyError::Stopped));
}

/// The design's rule for a file without a key: "Its key is computed after
/// its first frame is sent". Nobody asks for it here.
#[tokio::test]
async fn a_viewed_file_without_a_key_is_hashed_without_being_asked_for() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pending = dir.path().join("no-uid.dcm");
    let never_viewed = dir.path().join("never-viewed.dcm");
    write_dicom(&pending, None, 7, 8);
    write_dicom(&never_viewed, None, 8, 8);
    let (server, registry) = serve(&[pending.clone(), never_viewed]).await;

    assert_eq!(frame_key(&server, 0).await, None);
    hashing_done(&registry).await;
    let stats = registry.key_stats();
    assert_eq!(
        (stats.files_hashed, stats.bytes_hashed),
        (1, size(&pending)),
        "the viewed file is read once, the other never"
    );
    let listed = catalog(&server, "").await;
    assert_eq!(
        (shown(&entries(&listed)[0]), shown(&entries(&listed)[1])),
        (Shown::Key(b3(&pending)), Shown::Pending)
    );
    assert_eq!(frame_key(&server, 0).await, Some(b3(&pending)));
}

/// Only a frame served for viewing starts a hash or carries the key. A
/// thumbnail, a pixel probe and a frame that could not be served do
/// neither: scrolling a gallery must not read a folder a second time.
#[tokio::test]
async fn thumbnails_probes_and_failed_frames_start_no_hash_and_send_no_key() {
    let dir = tempfile::tempdir().expect("temp dir");
    let keyed = dir.path().join("keyed.dcm");
    let pending = dir.path().join("pending.dcm");
    write_dicom(&keyed, Some("1.2.826.0.1.3680043.10.520.1"), 1, 8);
    write_dicom(&pending, None, 2, 8);
    let (server, registry) = serve(&[keyed, pending]).await;

    for index in 0..2 {
        // (request, whether it is answered)
        for (request, answered) in [
            (format!("/api/file/{index}/frame/0/thumbnail"), true),
            (
                format!("/api/file/{index}/frame/0/raw/pixel?row=0&column=0"),
                true,
            ),
            (format!("/api/file/{index}/frame/7"), false),
            (format!("/api/file/{index}/frame/7/raw"), false),
        ] {
            let response = server.get(&request).await;
            assert_eq!(
                response.status_code().is_success(),
                answered,
                "{request}: {}",
                response.status_code()
            );
            assert!(
                response.maybe_header("x-file-key").is_none(),
                "{request} carries a file key"
            );
        }
    }
    // A request that had queued a file would show here, hashed or waiting.
    assert_eq!(registry.key_stats(), KeyStats::default());
    assert_eq!(registry.files_page(None, None).keys_hashing, 0);
    // The frame itself does both.
    assert!(frame_key(&server, 0).await.is_some());
    assert_eq!(frame_key(&server, 1).await, None);
    hashing_done(&registry).await;
    assert_eq!(registry.key_stats().files_hashed, 1);
}

/// One file reached by two paths is two entries and one file. Both entries
/// are viewed while hashing cannot start, so both are queued; the second
/// finds its digest known and is not read.
#[tokio::test]
async fn a_file_whose_digest_is_known_is_not_hashed_again() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("no-uid.dcm");
    write_dicom(&path, None, 3, 8);
    let (registry, _scheduler, viewer) = held(2).await;
    let (server, registry) = serve_with(registry, &[path.clone(), path.clone()]).await;

    assert_eq!(frame_key(&server, 0).await, None);
    assert_eq!(frame_key(&server, 1).await, None);
    assert_eq!(registry.files_page(None, None).keys_hashing, 2);
    assert_eq!(registry.key_stats(), KeyStats::default());

    drop(viewer);
    hashing_done(&registry).await;
    let stats = registry.key_stats();
    assert_eq!((stats.files_hashed, stats.bytes_hashed), (1, size(&path)));
    for index in 0..2 {
        assert_eq!(key_text(&registry, index), Some(b3(&path)), "entry {index}");
    }
}

/// A key somebody waits for is hashed before the keys viewing queued. The
/// order hashing finished in is read from the catalog: each file's entry
/// takes the next revision when its key arrives.
#[tokio::test]
async fn a_key_that_is_asked_for_is_hashed_before_those_viewing_queued() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut paths = Vec::new();
    for number in 0..4_u16 {
        let path = dir.path().join(format!("{number}.dcm"));
        write_dicom(&path, None, number, 8);
        paths.push(path);
    }
    let (registry, _scheduler, viewer) = held(2).await;
    let (server, registry) = serve_with(registry, &paths).await;

    // Three files are viewed, then the key of the fourth is asked for.
    for index in 0..3 {
        assert_eq!(frame_key(&server, index).await, None);
    }
    let asked = tokio::spawn({
        let registry = registry.clone();
        async move { registry.ensure_key(3).await }
    });
    hashing_queued(&registry, 4).await;
    assert_eq!(registry.key_stats(), KeyStats::default());
    let since = registry.files_page(None, None).revision;

    drop(viewer);
    asked.await.expect("task").expect("key");
    hashing_done(&registry).await;
    let order = registry
        .files_page(Some(since), None)
        .files
        .iter()
        .map(|entry| entry.index)
        .collect::<Vec<_>>();
    let position = |index: usize| {
        order
            .iter()
            .position(|listed| *listed == index)
            .unwrap_or_else(|| panic!("file {index} was not hashed: {order:?}"))
    };
    // The worker may already hold the first viewed file; nothing else goes
    // ahead of the request.
    assert!(
        position(3) < position(1) && position(3) < position(2),
        "hashed in the order {order:?}"
    );
    assert!(position(1) < position(2), "viewed files keep their order");
}

/// Hashing asks for background permits, which a pool of four grants two of
/// at a time. With both of those held nothing is read, though two permits
/// are free for a viewer's decode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashing_waits_for_a_background_permit_and_takes_no_viewer_permit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("no-uid.dcm");
    write_dicom(&path, None, 4, 8);
    let scheduler = DecodeScheduler::new(4);
    assert_eq!(dcmview::pixels::background_limit(4), 2);
    let registry = FileRegistry::new().with_decode_scheduler(scheduler.clone());
    scan(&registry, &path).await;
    let background = [
        scheduler.acquire(DecodeClass::Background).await,
        scheduler.acquire(DecodeClass::Background).await,
    ];

    let waiting = tokio::spawn({
        let registry = registry.clone();
        async move { registry.ensure_key(0).await }
    });
    hashing_queued(&registry, 1).await;
    let_tasks_run().await;
    assert_eq!(
        registry.key_stats(),
        KeyStats::default(),
        "read under a permit that is not a background one"
    );
    // A viewer's decode is not held up by the waiting hash.
    drop(scheduler.acquire(DecodeClass::Interactive).await);

    drop(background);
    assert_eq!(
        waiting.await.expect("task").expect("key").as_str(),
        b3(&path)
    );
}

/// Callers that wait on one file share one attempt, also a failing one:
/// thirty-two requests for the key of a file that is gone read (or try to
/// read) it once.
#[tokio::test]
async fn callers_waiting_on_one_file_share_one_attempt_that_fails() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("gone.dcm");
    write_dicom(&path, None, 5, 8);
    let (_server, registry) = serve(std::slice::from_ref(&path)).await;
    fs::remove_file(&path).expect("remove the file");

    // On this single-threaded runtime every caller has asked before the
    // worker's first turn, so none of them arrives after the attempt.
    let callers = (0..32)
        .map(|_| {
            let registry = registry.clone();
            tokio::spawn(async move { registry.ensure_key(0).await })
        })
        .collect::<Vec<_>>();
    for caller in callers {
        assert_eq!(
            caller.await.expect("task"),
            Err(KeyError::Unavailable(KeyFailure::Unreadable))
        );
    }
    assert_eq!(registry.key_stats().files_hashed, 1);
    assert_eq!(registry.files_page(None, None).keys_hashing, 0);
    // A caller that arrives afterwards tries again, once.
    assert_eq!(
        registry.ensure_key(0).await,
        Err(KeyError::Unavailable(KeyFailure::Unreadable))
    );
    assert_eq!(registry.key_stats().files_hashed, 2);
}

/// A file can be queued where no runtime is running to hash it (a registry
/// filled by plain code). The queue is kept, and hashing starts with the
/// next call that arrives inside a runtime, whatever that call is about.
#[test]
fn a_queue_filled_outside_a_runtime_is_hashed_once_a_runtime_calls() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pending = dir.path().join("no-uid.dcm");
    let keyed = dir.path().join("uid.dcm");
    write_dicom(&pending, None, 6, 8);
    write_dicom(&keyed, Some("1.2.826.0.1.3680043.10.521.1"), 6, 8);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let registry = FileRegistry::new();
    runtime.block_on(async {
        scan(&registry, &pending).await;
        scan(&registry, &keyed).await;
    });

    // Outside any runtime.
    registry.frame_sent(0);
    assert_eq!(registry.files_page(None, None).keys_hashing, 1);
    assert_eq!(registry.key_stats(), KeyStats::default());

    // A frame of another file, which has its key, served inside a runtime.
    runtime.block_on(async {
        registry.frame_sent(1);
        hashing_done(&registry).await;
    });
    assert_eq!(key_text(&registry, 0), Some(b3(&pending)));
    assert_eq!(registry.key_stats().files_hashed, 1);
}

/// A runtime that goes away takes the hashing worker with it, wherever the
/// worker was: not polled yet, or holding a file and waiting for a permit.
/// Nothing was wrong with any file, so none has a failure, none is dropped
/// from the queue, and all are hashed once a call arrives inside a runtime
/// again, without their keys being asked for.
#[test]
fn files_queued_when_the_workers_runtime_goes_away_are_still_hashed() {
    for mid_file in [false, true] {
        let dir = tempfile::tempdir().expect("temp dir");
        let paths = [dir.path().join("a.dcm"), dir.path().join("b.dcm")];
        write_dicom(&paths[0], None, 1, 8);
        write_dicom(&paths[1], None, 2, 8);
        let scheduler = DecodeScheduler::new(1);
        let registry = FileRegistry::new().with_decode_scheduler(scheduler.clone());
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
        };

        let first = runtime();
        let viewer = first.block_on(async {
            scan(&registry, &paths[0]).await;
            scan(&registry, &paths[1]).await;
            let viewer = scheduler.acquire(DecodeClass::Interactive).await;
            registry.frame_sent(0);
            registry.frame_sent(1);
            if mid_file {
                // The worker takes the first file and waits for a permit.
                let_tasks_run().await;
            }
            viewer
        });
        drop(first);
        drop(viewer);
        assert_eq!(
            registry.files_page(None, None).keys_hashing,
            2,
            "mid_file={mid_file}"
        );
        assert_eq!(registry.key_stats().files_hashed, 0);

        runtime().block_on(async {
            // A frame of a file that is already queued: it asks for no key.
            registry.frame_sent(1);
            tokio::time::timeout(Duration::from_secs(30), hashing_done(&registry))
                .await
                .unwrap_or_else(|_| panic!("mid_file={mid_file}: a queued file is never hashed"));
        });
        for (index, path) in paths.iter().enumerate() {
            assert_eq!(
                key_text(&registry, index),
                Some(b3(path)),
                "mid_file={mid_file}"
            );
        }
        assert_eq!(registry.key_stats().files_hashed, 2);
    }
}

/// A hub that computes keys with other tooling has to read the UID the way
/// discovery does, or it builds another key for the same file. This is that
/// reading: the data set's SOP Instance UID, its first value, without
/// padding or surrounding white space, and otherwise exactly as written.
#[tokio::test]
async fn a_uid_key_is_built_from_the_uid_as_discovery_reads_it() {
    // (the value as written in the file, the key it gives)
    let cases: Vec<(&str, Option<&str>)> = vec![
        (
            "1.2.826.0.1.3680043.10.522.1",
            Some("sop:1.2.826.0.1.3680043.10.522.1"),
        ),
        // An odd length is padded in the file; the padding is not the UID.
        (
            "1.2.826.0.1.3680043.10.522.10",
            Some("sop:1.2.826.0.1.3680043.10.522.10"),
        ),
        (
            "  1.2.826.0.1.3680043.10.522.2 ",
            Some("sop:1.2.826.0.1.3680043.10.522.2"),
        ),
        // Two values: the first.
        (
            "1.2.826.0.1.3680043.10.522.3\\1.2.826.0.1.3680043.10.522.99",
            Some("sop:1.2.826.0.1.3680043.10.522.3"),
        ),
        // Not a conforming UID, and kept as written: no case folding.
        ("Anon-0007_aB", Some("sop:Anon-0007_aB")),
        // White space inside is not removed, so the value cannot be a key.
        ("1.2.826 0.1", None),
        ("", None),
    ];
    let dir = tempfile::tempdir().expect("temp dir");
    let mut paths = Vec::new();
    for (number, (written, _)) in cases.iter().enumerate() {
        let path = dir.path().join(format!("{number}.dcm"));
        write_dicom(&path, Some(written), number as u16, 8);
        paths.push(path);
    }
    let (server, registry) = serve(&paths).await;
    let listed = catalog(&server, "").await;
    for (index, (written, expected)) in cases.iter().enumerate() {
        assert_eq!(
            key_text(&registry, index).as_deref(),
            *expected,
            "written as {written:?}"
        );
        let entry = &entries(&listed)[index];
        match expected {
            // The entry's own UID is the one the key is built from.
            Some(key) => {
                assert_eq!(shown(entry), Shown::Implied, "written as {written:?}");
                assert_eq!(
                    format!("sop:{}", entry["sop_instance_uid"].as_str().expect("UID")),
                    *key
                );
            }
            None => assert_eq!(shown(entry), Shown::Pending, "written as {written:?}"),
        }
    }
    assert_eq!(registry.key_stats(), KeyStats::default());
}
