//! File keys as a client and the rest of the server see them
//! (`docs/design/annotation-model.md` 1.3, 1.7, 1.8): the key state of each
//! catalog entry, the catalog cursor, key replacement, and the hashing
//! behind `b3:` keys, counted in bytes and never in time.
//!
//! Files are found by the production loader and registered the way startup
//! registers them. Each path is scanned on its own so that indexes follow
//! the order the test names them in; a real scan is parallel and its order
//! is not fixed.

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
use std::time::Duration;
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
    let registry = FileRegistry::new();
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

    // The file is as discovery saw it again: asking retries, and succeeds.
    fs::write(&changed, &original).expect("restore the file");
    let key = registry.ensure_key(0).await.expect("key after restore");
    assert_eq!(key.as_str(), b3(&changed));
    let listed = catalog(&server, "").await;
    assert_eq!(shown(&entries(&listed)[0]), Shown::Key(b3(&changed)));
    assert!(entries(&listed)[0].get("key_error").is_none());
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
