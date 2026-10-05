//! Golden bytes of the EMBED-style ROI CSV that dcmview 0.3.x reads and writes
//! (`docs/design/annotation-model.md` section 11 step 1).
//!
//! Each case in `tests/fixtures/embed-goldens/` runs the real binary with
//! `--annotations` over copies of committed fixtures and reads
//! `GET /api/annotations/export.csv`, so nothing here names the annotation
//! store's Rust API and the cases survive its replacement unchanged.
//!
//! The comparison is on bytes: the header and every row, line terminator
//! included, must equal the expected file. Rows are compared as a multiset
//! because today's row order is discovery completion order and varies
//! between runs.
//!
//! Later work changes some of these bytes on purpose, with the owner's
//! sign-off. Each is marked `EXPECTED CHANGE` at the case it will touch:
//! rows sorted by path; `[]` written for all-frames files after an edit;
//! lenient import with a report; the first of conflicting duplicate rows
//! loading.
//!
//! Unix only: the goldens hold `/`-separated paths and LF line endings, and
//! nothing pins either on a Windows checkout.
#![cfg(unix)]

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};

/// Stands for the canonical absolute directory the matched files live in.
/// It is substituted into the input CSV before the viewer starts and into
/// the expected CSV before comparison, so both sides hold real paths and
/// are compared as bytes.
const ROOT_PLACEHOLDER: &str = "{{ROOT}}";

/// The matched files: a committed fixture and the name it is copied to under
/// the root. `single-frame.dcm` is 160 rows by 240 columns with one frame,
/// `multiframe.dcm` 160 by 240 with three, `comma, name.dcm` 16 by 16 with
/// one, and `nested/relative.dcm` 4 by 4 with three.
const MATCHED_FILES: [(&str, &str); 4] = [
    ("golden-gsps-target-u8.dcm", "single-frame.dcm"),
    ("golden-gsps-target-multiframe-u8.dcm", "multiframe.dcm"),
    ("golden-jpeg-baseline-single-frame.dcm", "comma, name.dcm"),
    (
        "golden-uncompressed-u16-multiframe.dcm",
        "nested/relative.dcm",
    ),
];

/// One `PUT /api/file/{index}/annotations` made after the CSV has loaded.
struct Edit {
    file: &'static str,
    body: &'static str,
    status: u16,
}

struct ExportCase {
    name: &'static str,
    edits: &'static [Edit],
}

const EXPORT_CASES: &[ExportCase] = &[
    // The four documented columns over a single-frame file, a multiframe
    // file with one frame list per ROI, a path that needs CSV quoting, and a
    // path relative to the working directory (exported absolute). A quoted
    // `"[]"` or `"[[0]]"` in the input is written back unquoted: only fields
    // containing a comma are quoted.
    ExportCase {
        name: "columns-full",
        edits: &[],
    },
    // `num_ROI` and `ROI_frames` omitted: the count is derived and the
    // frames column is written as `[]`.
    ExportCase {
        name: "columns-minimal",
        edits: &[],
    },
    // Dataset-style input: extra columns, another column order, spaces
    // inside the JSON and around values. Export has the four columns only,
    // with compact JSON. A row with no ROIs and a row whose path matches no
    // loaded file (its payload is never parsed) produce no output row.
    ExportCase {
        name: "columns-extra",
        edits: &[],
    },
    // What the loader does not check today: boxes past the image edge and
    // far outside it, zero-height and zero-area boxes, a box with its
    // corners in the wrong order, an unsorted frame list with a repeat, and
    // an empty frame list for one ROI. All load and export exactly as
    // written. Saving the loaded out-of-range row back through the API is
    // refused and leaves it in place.
    // EXPECTED CHANGE: these rows will also be listed in an import report,
    // and the first edit of such a file will clamp it.
    ExportCase {
        name: "unchecked-geometry",
        edits: &[Edit {
            file: "single-frame.dcm",
            body: r#"{"num_roi":5,"roi_coords":[[0,0,161,241],[500,600,700,800],[40,50,40,90],[5,5,5,5],[60,90,20,30]],"roi_frames":[]}"#,
            status: 400,
        }],
    },
    // Edits through the API, then export. The first two bodies are what the
    // 0.3 viewer sends after one ROI is added or moved on a file loaded with
    // `ROI_frames` `[]`: it expands `[]` to every frame index for every ROI,
    // and the server exports the lists it was given.
    // EXPECTED CHANGE: a file whose ROIs all cover every frame will export
    // `[]` again, so those two rows end in `,[]`.
    ExportCase {
        name: "edited",
        edits: &[
            Edit {
                file: "single-frame.dcm",
                body: r#"{"num_roi":2,"roi_coords":[[20,30,60,90],[100,200,120,220]],"roi_frames":[[0],[0]]}"#,
                status: 200,
            },
            // Corners in the wrong order and a wrong count are corrected on
            // the way in.
            Edit {
                file: "multiframe.dcm",
                body: r#"{"num_roi":99,"roi_coords":[[55,85,15,25],[100,120,160,240]],"roi_frames":[[0,1,2],[0,1,2]]}"#,
                status: 200,
            },
            // Deleting a file's last ROI removes its row.
            Edit {
                file: "comma, name.dcm",
                body: r#"{"num_roi":0,"roi_coords":[],"roi_frames":[]}"#,
                status: 200,
            },
            // A file with no CSV row gains one; `[]` from a client stays `[]`.
            Edit {
                file: "nested/relative.dcm",
                body: r#"{"num_roi":1,"roi_coords":[[0,0,4,4]],"roi_frames":[]}"#,
                status: 200,
            },
        ],
    },
];

/// Inputs whose import fails as a whole today: the viewer starts and keeps
/// serving, no row is committed (the valid first row included), and export
/// answers 500.
/// EXPECTED CHANGE: with conflicting duplicate rows the first will load.
const REJECTED_IMPORTS: &[&str] = &[
    // A frame index equal to the matched file's frame count.
    "rejected-frame-past-end",
    // Two rows for one loaded file, with different boxes.
    "rejected-duplicate-path",
    // A negative coordinate, unlike one past the far edge, is not an
    // unsigned integer and fails the row.
    "rejected-negative-coordinate",
];

#[tokio::test]
async fn export_bytes_match_the_embed_goldens() {
    for case in EXPORT_CASES {
        let viewer = Viewer::start(case.name).await;

        // The first export waits for the CSV import, so every edit lands on
        // loaded state.
        let loaded = viewer.get("/api/annotations/export.csv").await;
        assert_eq!(loaded.status(), 200, "{}: export after load", case.name);

        let indexes = viewer.file_indexes().await;
        for edit in case.edits {
            let index = indexes[&viewer.root.join(edit.file)];
            let response = viewer
                .request(
                    reqwest::Method::PUT,
                    &format!("/api/file/{index}/annotations"),
                )
                .header("content-type", "application/json")
                .body(edit.body)
                .send()
                .await
                .expect("send edit");
            assert_eq!(
                response.status(),
                edit.status,
                "{}: edit of {}",
                case.name,
                edit.file
            );
        }

        let exported = viewer.get("/api/annotations/export.csv").await;
        assert_eq!(exported.status(), 200, "{}: export", case.name);
        let actual = exported.bytes().await.expect("export body");
        let expected = viewer.golden(&format!("{}.expected.csv", case.name));
        assert_same_rows(case.name, &actual, expected.as_bytes());
    }
}

#[tokio::test]
async fn rejected_imports_fail_export_and_keep_the_viewer_serving() {
    for name in REJECTED_IMPORTS {
        let viewer = Viewer::start(name).await;

        let exported = viewer.get("/api/annotations/export.csv").await;
        assert_eq!(exported.status(), 500, "{name}: export");
        let body: Value = exported.json().await.expect("error envelope");
        assert_eq!(body["code"], "internal_error", "{name}");

        let health = viewer.get("/api/health").await;
        assert_eq!(health.status(), 200, "{name}: health");
    }
}

/// Header first and exact, then the same rows in any order, each compared
/// with its line terminator.
/// EXPECTED CHANGE: once export sorts rows by path, compare the whole body.
fn assert_same_rows(case: &str, actual: &[u8], expected: &[u8]) {
    let split = |bytes: &'_ [u8]| -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
        let header = lines.next().unwrap_or_default().to_vec();
        let mut rows: Vec<Vec<u8>> = lines.map(<[u8]>::to_vec).collect();
        rows.sort();
        (header, rows)
    };
    assert!(
        split(actual) == split(expected),
        "{case}: export differs from the golden\n--- exported\n{}--- expected\n{}",
        String::from_utf8_lossy(actual),
        String::from_utf8_lossy(expected),
    );
}

/// A real `dcmview` process over a fresh copy of the matched files, started
/// with one golden input as its `--annotations` CSV.
struct Viewer {
    root: PathBuf,
    base_url: String,
    token: String,
    client: reqwest::Client,
    _stdout: Lines<BufReader<ChildStdout>>,
    _child: Child,
    _directory: TempDir,
}

impl Viewer {
    async fn start(case: &str) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        // Canonical, because export writes canonical paths.
        let root = directory
            .path()
            .canonicalize()
            .expect("canonical temp dir")
            .join("data");
        assert!(
            !root
                .to_string_lossy()
                .contains([',', '"', '\n', '\r', '{', '}']),
            "the temp directory {} would change how paths are quoted",
            root.display()
        );
        for (fixture, name) in MATCHED_FILES {
            let target = root.join(name);
            fs::create_dir_all(target.parent().expect("parent")).expect("create directory");
            fs::copy(fixtures().join(fixture), target).expect("copy fixture");
        }

        let csv_path = root.with_file_name("annotations.csv");
        let input = fs::read_to_string(golden_path(&format!("{case}.input.csv")))
            .expect("read golden input")
            .replace(ROOT_PLACEHOLDER, &root.to_string_lossy());
        fs::write(&csv_path, input).expect("write annotations CSV");

        // The working directory is the root so that relative CSV paths
        // resolve against it.
        let mut child = Command::new(env!("CARGO_BIN_EXE_dcmview"))
            .arg("--startup-json")
            .arg("--no-browser")
            .arg("--annotations")
            .arg(&csv_path)
            .arg(&root)
            .current_dir(&root)
            .env_remove("DCMVIEW_TOKEN")
            .env("DCMVIEW_VSCODE_BYPASS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn dcmview");

        let mut stdout = BufReader::new(child.stdout.take().expect("stdout")).lines();
        let started = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let line = stdout
                    .next_line()
                    .await
                    .expect("read stdout")
                    .unwrap_or_else(|| panic!("{case}: dcmview exited before it started"));
                if line.starts_with("{\"type\":\"server_started\"") {
                    return serde_json::from_str::<Value>(&line).expect("startup JSON");
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{case}: no startup event"));

        Self {
            root,
            base_url: started["base_url"].as_str().expect("base_url").to_string(),
            token: started["token"].as_str().expect("token").to_string(),
            client: reqwest::Client::new(),
            _stdout: stdout,
            _child: child,
            _directory: directory,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(30))
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap_or_else(|error| panic!("GET {path}: {error}"))
    }

    /// The catalog index of every loaded file by path, once the scan is done.
    async fn file_indexes(&self) -> HashMap<PathBuf, usize> {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let catalog: Value = self.get("/api/files").await.json().await.expect("catalog");
                if catalog["scan_complete"] == true {
                    return catalog["files"]
                        .as_array()
                        .expect("files")
                        .iter()
                        .map(|file| {
                            (
                                PathBuf::from(file["path"].as_str().expect("path")),
                                file["index"].as_u64().expect("index") as usize,
                            )
                        })
                        .collect();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("scan completion")
    }

    fn golden(&self, name: &str) -> String {
        fs::read_to_string(golden_path(name))
            .expect("read golden")
            .replace(ROOT_PLACEHOLDER, &self.root.to_string_lossy())
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden_path(name: &str) -> PathBuf {
    fixtures().join("embed-goldens").join(name)
}
