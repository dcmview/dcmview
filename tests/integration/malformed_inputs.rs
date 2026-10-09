//! Malformed inputs are skipped during discovery or answered with a JSON error,
//! and neither stops the viewer from serving healthy files.

use super::support;
use axum_test::TestServer;
use dcmview::loader::DiscoverOptions;
use dcmview::server;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use serde_json::Value;
use std::path::Path;
use tempfile::tempdir;

const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";

async fn discover(dir: &Path) -> support::LoadReport {
    support::discover(
        &[dir.to_path_buf()],
        DiscoverOptions {
            recursive: true,
            filters: Vec::new(),
            formats: Default::default(),
        },
    )
    .await
    .expect("discovery completes")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[tokio::test]
async fn malformed_pixel_payloads_fail_per_request_and_the_server_keeps_serving() {
    let dir = tempdir().expect("temp dir");
    support::write_uncompressed_u16_dicom(
        &dir.path().join("healthy.dcm"),
        EXPLICIT_LE,
        2,
        2,
        vec![1, 2, 3, 4],
        None,
        None,
    );
    // An RLE header must declare at most 15 segments (PS3.5 Annex G.4).
    let mut rle_header = vec![0_u8; 64];
    rle_header[0] = 99;
    support::write_encapsulated_dicom(
        &dir.path().join("bad-rle-header.dcm"),
        "1.2.840.10008.1.2.5",
        vec![rle_header],
    );
    let jpeg = support::grayscale_jpeg_fragment_16x16(7);
    support::write_encapsulated_dicom(
        &dir.path().join("truncated-jpeg.dcm"),
        "1.2.840.10008.1.2.4.50",
        vec![jpeg[..jpeg.len() / 3].to_vec()],
    );
    // Declares 4x4 16-bit samples but carries only four of them.
    support::write_uncompressed_u16_dicom(
        &dir.path().join("short-native.dcm"),
        EXPLICIT_LE,
        4,
        4,
        vec![1, 2, 3, 4],
        None,
        None,
    );

    let report = discover(dir.path()).await;
    assert_eq!(
        report.files.len(),
        4,
        "malformed pixels still parse as DICOM"
    );
    let healthy = report
        .files
        .iter()
        .find(|file| file_name(&file.path.to_string_lossy()) == "healthy.dcm")
        .map(|file| file.index)
        .expect("healthy file discovered");
    let malformed = report
        .files
        .iter()
        .filter(|file| file.index != healthy)
        .map(|file| (file.index, file.path.display().to_string()))
        .collect::<Vec<_>>();
    let server = TestServer::new(server::router(support::app_state(report.files)));

    for (index, path) in malformed {
        for endpoint in [
            format!("/api/file/{index}/frame/0"),
            format!("/api/file/{index}/frame/0/raw"),
        ] {
            let response = server.get(&endpoint).await;
            let status = response.status_code().as_u16();
            assert!(
                (400..600).contains(&status),
                "{path} {endpoint} returned {status}"
            );
            let body: Value = response.json();
            assert!(
                body["code"].is_string() && body["error"].is_string(),
                "{path} {endpoint} must use the JSON error envelope: {body}"
            );
        }
        server
            .get(&format!("/api/file/{healthy}/frame/0/raw"))
            .await
            .assert_status_ok();
    }
}

#[tokio::test]
async fn unreadable_files_are_skipped_without_stopping_discovery() {
    let dir = tempdir().expect("temp dir");
    let healthy = dir.path().join("healthy.dcm");
    support::write_uncompressed_u16_dicom(
        &healthy,
        EXPLICIT_LE,
        2,
        2,
        vec![1, 2, 3, 4],
        None,
        None,
    );

    let bytes = std::fs::read(&healthy).expect("read healthy fixture");
    std::fs::write(dir.path().join("truncated.dcm"), &bytes[..bytes.len() / 2])
        .expect("write truncated file");
    std::fs::write(dir.path().join("not-dicom.dcm"), b"plain text, not DICOM")
        .expect("write non-DICOM file");

    let charset = dir.path().join("unknown-charset.dcm");
    support::write_uncompressed_u16_dicom(
        &charset,
        EXPLICIT_LE,
        2,
        2,
        vec![1, 2, 3, 4],
        None,
        None,
    );
    let mut object = dicom_object::open_file(&charset).expect("reopen charset fixture");
    object.put(DataElement::new(
        tags::SPECIFIC_CHARACTER_SET,
        VR::CS,
        PrimitiveValue::from("ISO_IR 999"),
    ));
    object
        .write_to_file(&charset)
        .expect("write charset fixture");

    let report = discover(dir.path()).await;

    let loaded = report
        .files
        .iter()
        .map(|file| file_name(&file.path.to_string_lossy()).to_string())
        .collect::<Vec<_>>();
    assert_eq!(loaded, ["healthy.dcm"]);
    assert_eq!(report.skipped, 3);
}

/// The lines the viewer logs for the requests of
/// [`a_failed_decode_is_answered_in_the_viewers_words_whatever_the_file_holds`],
/// which name themselves with a `probe` query the endpoints ignore.
///
/// The subscriber is the process's, installed once: a subscriber scoped to
/// one request misses events whose call sites another test's thread
/// reached first. It keeps nothing of any other request.
#[derive(Clone, Default)]
struct ProbeLog(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

const PROBE: &str = "probe=decode-error-";

/// The tag of the element the test's DICOM files are cut in, as a parser's
/// account of such a file writes it. Lines that hold it are kept too,
/// whichever request or task logged them.
const MARKED_TAG: &str = "0BAD,0F0D";

impl std::io::Write for ProbeLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(bytes);
        if line.contains(PROBE) || line.to_uppercase().contains(MARKED_TAG) {
            self.0.lock().expect("log").push(line.into_owned());
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ProbeLog {
    type Writer = ProbeLog;

    fn make_writer(&'a self) -> ProbeLog {
        self.clone()
    }
}

fn probe_log() -> &'static ProbeLog {
    static LOG: std::sync::OnceLock<ProbeLog> = std::sync::OnceLock::new();
    LOG.get_or_init(|| {
        let log = ProbeLog::default();
        tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .try_init()
            .expect("no other test installs a subscriber");
        log
    })
}

/// One answer to a decoding endpoint: its status, its JSON body and the
/// log lines of the request, at every level.
struct Answer {
    status: u16,
    body: Value,
    log: String,
}

/// Lists `files` (in name order), applies `damage` to the directory, and
/// asks a viewer over them, masked or not, for each of `requests`, which
/// `probe` tells apart from every other request of the process.
async fn answers(
    files: &[(&str, Vec<u8>)],
    damage: &dyn Fn(&Path),
    requests: &[String],
    masked: bool,
    probe: &str,
) -> Vec<Answer> {
    let log = probe_log();
    let dir = tempdir().expect("temp dir");
    for (name, bytes) in files {
        std::fs::write(dir.path().join(name), bytes).expect("write file");
    }
    let report = discover(dir.path()).await;
    assert_eq!(report.files.len(), files.len(), "every file is listed");
    damage(dir.path());
    let registry = if masked {
        dcmview::server::FileRegistry::masked(std::sync::Arc::new(dcmview::masking::Masker::new()))
    } else {
        dcmview::server::FileRegistry::default()
    };
    for file in report.files {
        registry.insert(file);
    }
    registry.mark_scan_complete();
    let server = TestServer::new(server::router(support::app_state_with_registry(registry)));
    let mut answers = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        let token = format!("{PROBE}{probe}-{index}-end");
        let separator = if request.contains('?') { '&' } else { '?' };
        let response = server.get(&format!("{request}{separator}{token}")).await;
        let log = log
            .0
            .lock()
            .expect("log")
            .iter()
            .filter(|line| line.contains(&token))
            .cloned()
            .collect();
        answers.push(Answer {
            status: response.status_code().as_u16(),
            body: response.json(),
            log,
        });
    }
    answers
}

/// A decode that fails says so in the viewer's own words. Two files damaged
/// in different ways, so that a decoder's own account of them differs, get
/// the same answer from every endpoint that decodes; a masked session logs
/// nothing more than that answer, and an unmasked one keeps the cause at
/// debug level, without what a raster decoder quotes of its file.
#[tokio::test]
async fn a_failed_decode_is_answered_in_the_viewers_words_whatever_the_file_holds() {
    use super::raster_files::{png_from_chunks, png_idat};

    let fixture = |name: &str| {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .expect("read fixture")
    };
    // A 4 x 4 gray PNG whose image data carries the wrong checksum.
    let png = |checksum: u32| {
        let mut idat = png_idat(&[7; 16], 4);
        let end = idat.len();
        idat[end - 4..].copy_from_slice(&checksum.to_be_bytes());
        png_from_chunks((4, 4), 8, 0, false, &[idat])
    };
    // A 4 x 4 16-bit image holding four of its 16 samples, each `value`.
    let short_native = |value: u16| {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("short.dcm");
        support::write_uncompressed_u16_dicom(&path, EXPLICIT_LE, 4, 4, vec![value; 4], None, None);
        std::fs::read(&path).expect("read file")
    };
    let cut = |bytes: u64| {
        move |dir: &Path| {
            let dose = std::fs::OpenOptions::new()
                .write(true)
                .open(dir.join("0-dose.dcm"))
                .expect("open dose");
            let length = dose.metadata().expect("stat dose").len();
            dose.set_len(length - bytes).expect("cut dose");
        }
    };
    let untouched = |_: &Path| {};
    // Every file of the directory cut `bytes` into the value of its
    // marked element, so that a parser's account of the file names that
    // element.
    let cut_all = |bytes: u64| {
        move |dir: &Path| {
            for entry in std::fs::read_dir(dir).expect("list files") {
                let path = entry.expect("entry").path();
                let content = std::fs::read(&path).expect("read file");
                let marker = [0xad, 0x0b, 0x0d, 0x0f, b'L', b'O'];
                let at = content
                    .windows(marker.len())
                    .position(|window| window == marker)
                    .expect("the marked element");
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("open file");
                file.set_len(at as u64 + 8 + bytes).expect("cut file");
            }
        }
    };
    // A fixture with a marked element among its others.
    let marked = |name: &str| {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("marked.dcm");
        let mut object = dicom_object::open_file(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .expect("open fixture");
        object.put(DataElement::new(
            dicom_core::Tag(0x0bad, 0x0f0d),
            VR::LO,
            PrimitiveValue::from("x".repeat(64)),
        ));
        object.write_to_file(&path).expect("write file");
        std::fs::read(&path).expect("read file")
    };
    let metadata_files = || {
        vec![
            ("0-state.dcm", marked("golden-gsps-conforming.dcm")),
            ("1-image.dcm", marked("golden-gsps-target-u8.dcm")),
            ("2-slide.dcm", marked("golden-masking-wsi-label.dcm")),
            // An object the value mappings of every image are read from.
            ("3-mapping.dcm", marked("golden-rwvm-ct-hounsfield.dcm")),
        ]
    };
    let metadata = vec![
        "/api/file/1/frame/0/value-mapping".to_string(),
        "/api/file/1/frame/0/graphic-annotations?state=0".to_string(),
        "/api/file/1/references".to_string(),
        "/api/file/1/semantic-context".to_string(),
        "/api/file/2/frame/0/wsi-context".to_string(),
        "/api/file/1/tags".to_string(),
        "/api/file/1/tags/select?path=(0008,0018)".to_string(),
    ];
    let frames = |index: usize| {
        ["", "/raw", "/thumbnail"]
            .map(|endpoint| format!("/api/file/{index}/frame/0{endpoint}"))
            .to_vec()
    };
    let dose_files = || {
        vec![
            ("0-dose.dcm", fixture("golden-rtdose-u16-grid.dcm")),
            ("1-ct.dcm", fixture("golden-rtdose-ct-source-z6.dcm")),
        ]
    };
    let overlays = vec![
        "/api/file/1/frame/0/dose-overlay?dose=0".to_string(),
        "/api/file/1/frame/0/dose-overlay/values?dose=0".to_string(),
    ];
    type Damage = Box<dyn Fn(&Path)>;
    type Variant = (Vec<(&'static str, Vec<u8>)>, Damage);
    // A name, two files that fail differently, the requests, and what the
    // first file holds that its decoder's own account of it would repeat.
    type Row = (
        &'static str,
        [Variant; 2],
        Vec<String>,
        &'static [&'static str],
    );
    let rows: Vec<Row> = vec![
        (
            "a PNG with a wrong checksum",
            [
                (vec![("a.png", png(0xdead_beef))], Box::new(untouched)),
                (vec![("a.png", png(0x0bad_f00d))], Box::new(untouched)),
            ],
            frames(0),
            &["deadbeef", "IDAT"],
        ),
        (
            "native pixel data shorter than a frame",
            [
                (vec![("a.dcm", short_native(1))], Box::new(untouched)),
                (vec![("a.dcm", short_native(2))], Box::new(untouched)),
            ],
            frames(0),
            &[],
        ),
        (
            "an RT Dose cut short after it was listed",
            [
                (dose_files(), Box::new(cut(2))),
                (dose_files(), Box::new(cut(6))),
            ],
            overlays,
            &[],
        ),
        (
            "files cut inside their data sets after they were listed",
            [
                (metadata_files(), Box::new(cut_all(10))),
                (metadata_files(), Box::new(cut_all(30))),
            ],
            metadata,
            &[MARKED_TAG, "0bad,0f0d"],
        ),
    ];
    for (row, (name, [first, second], requests, planted)) in rows.into_iter().enumerate() {
        let probe = |label: &str| format!("{row}-{label}");
        let unmasked = answers(&first.0, &first.1, &requests, false, &probe("first")).await;
        let other = answers(&second.0, &second.1, &requests, false, &probe("second")).await;
        let marked_lines = || {
            let lines = probe_log().0.lock().expect("log");
            lines
                .iter()
                .filter(|line| line.to_uppercase().contains(MARKED_TAG))
                .count()
        };
        let before_masked = marked_lines();
        let masked = answers(&first.0, &first.1, &requests, true, &probe("masked")).await;
        assert_eq!(
            marked_lines(),
            before_masked,
            "{name}: a masked session logged a parser's account of a file"
        );
        for (index, request) in requests.iter().enumerate() {
            let (answer, other, masked) = (&unmasked[index], &other[index], &masked[index]);
            let context = format!("{name}, {request}");
            assert!(answer.status >= 400, "{context}: {}", answer.body);
            // A reason the viewer states may carry numbers it computed, so
            // the answers are compared without their digits.
            let words = |body: &Value| {
                body.to_string()
                    .chars()
                    .filter(|character| !character.is_ascii_digit())
                    .collect::<String>()
            };
            assert_eq!(
                (answer.status, words(&answer.body)),
                (other.status, words(&other.body)),
                "{context}: the answer depends on how the file is damaged"
            );
            assert_eq!(
                (answer.status, &answer.body),
                (masked.status, &masked.body),
                "{context}: masked"
            );
            for text in planted {
                // An unmasked session may log a parser's account of a
                // DICOM file at debug level; no response and no masked
                // session's log repeats it, and nothing repeats a raster
                // decoder's.
                let unmasked_log = name.contains("PNG") && answer.log.contains(text);
                let repeated = [&answer.body, &other.body, &masked.body]
                    .iter()
                    .any(|body| body.to_string().contains(text));
                assert!(
                    !unmasked_log && !repeated && !masked.log.contains(text),
                    "{context}: {text} from the file is repeated: {} {} {}",
                    answer.body,
                    answer.log,
                    masked.log
                );
            }
            if answer.status < 500 {
                continue;
            }
            let error = answer.body["error"].as_str().expect("error text");
            assert!(
                answer.log.contains(error) && answer.log.contains("DEBUG"),
                "{context}: the answer is logged, and its cause at debug level: {}",
                answer.log
            );
            assert!(
                masked.log.contains(error) && !masked.log.contains("DEBUG"),
                "{context}: a masked session logs the answer and nothing else: {}",
                masked.log
            );
        }
    }

    // A file read on another file's behalf: an image's value mappings are
    // also read from the mapping objects beside it, and one that cannot be
    // read is only logged. A masked session does not log why.
    let cut_mapping = |dir: &Path| {
        let path = dir.join("3-mapping.dcm");
        let content = std::fs::read(&path).expect("read file");
        let marker = [0xad, 0x0b, 0x0d, 0x0f, b'L', b'O'];
        let at = content
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("the marked element");
        std::fs::write(&path, &content[..at + 18]).expect("cut file");
    };
    let before_masked = {
        let lines = probe_log().0.lock().expect("log");
        lines.len()
    };
    let mapped = answers(
        &metadata_files(),
        &cut_mapping,
        &["/api/file/1/frame/0/value-mapping".to_string()],
        true,
        "mapping",
    )
    .await;
    assert_eq!(mapped[0].status, 200, "{}", mapped[0].body);
    {
        let lines = probe_log().0.lock().expect("log");
        let logged: Vec<_> = lines[before_masked..]
            .iter()
            .filter(|line| line.to_uppercase().contains(MARKED_TAG))
            .collect();
        assert!(logged.is_empty(), "a masked session logged {logged:?}");
    }

    // The reasons that are the viewer's own are shown, whatever their
    // numbers. Each file is damaged after it was listed.
    let cut_at = |marker: &'static [u8], keep: usize| {
        move |dir: &Path| {
            let path = dir.join("a.dcm");
            let content = std::fs::read(&path).expect("read file");
            let at = content
                .windows(marker.len())
                .rposition(|window| window == marker)
                .expect("the pixel element");
            std::fs::write(&path, &content[..at + keep]).expect("cut file");
        }
    };
    const PIXEL_DATA: &[u8] = &[0xe0, 0x7f, 0x10, 0x00];
    let stated: Vec<(&str, Vec<u8>, Damage, &str)> = vec![
        (
            "native pixel data shorter than a frame",
            short_native(1),
            Box::new(untouched),
            "native pixel data frame  extends beyond  source bytes",
        ),
        (
            "a fragment cut short",
            fixture("golden-jpeg2000-lossless-u8-single-frame.dcm"),
            // The element's header, an empty offset table, the fragment's
            // item header and a little of the fragment.
            Box::new(cut_at(PIXEL_DATA, 12 + 8 + 8 + 20)),
            "encapsulated fragment is truncated",
        ),
        (
            "no pixel data element",
            short_native(1),
            Box::new(cut_at(PIXEL_DATA, 0)),
            "native pixel data element",
        ),
    ];
    for (index, (name, file, damage, reason)) in stated.into_iter().enumerate() {
        let request = ["/api/file/0/frame/0/raw".to_string()];
        let answer = answers(
            &[("a.dcm", file)],
            &damage,
            &request,
            false,
            &format!("stated-{index}"),
        )
        .await;
        let error = answer[0].body["error"]
            .as_str()
            .expect("error text")
            .chars()
            .filter(|character| !character.is_ascii_digit())
            .collect::<String>();
        assert!(error.contains(reason), "{name}: {error}");
    }
}
