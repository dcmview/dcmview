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
//! sign-off. Each is marked `EXPECTED CHANGE` at the case it will touch,
//! with the decision in `docs/design/annotation-model.md` section 12 that
//! signs it off: rows sorted by path (12.11); frame lists after an edit and
//! a per-ROI `[]` (12.6); lenient import with a report (12.12); two
//! byte-identical files given different ROIs (12.2).
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

/// Stands for a symbolic link to that directory, beside it.
const LINK_PLACEHOLDER: &str = "{{LINK}}";

/// The matched files: a committed fixture and the name it is copied to under
/// the root. `single-frame.dcm` is 160 rows by 240 columns with one frame,
/// `multiframe.dcm` 160 by 240 with three, `comma, name.dcm` and its
/// byte-identical `identical-copy.dcm` 16 by 16 with one, and
/// `nested/relative.dcm` 4 by 4 with three.
const MATCHED_FILES: [(&str, &str); 5] = [
    ("golden-gsps-target-u8.dcm", "single-frame.dcm"),
    ("golden-gsps-target-multiframe-u8.dcm", "multiframe.dcm"),
    ("golden-jpeg-baseline-single-frame.dcm", "comma, name.dcm"),
    (
        "golden-jpeg-baseline-single-frame.dcm",
        "identical-copy.dcm",
    ),
    (
        "golden-uncompressed-u16-multiframe.dcm",
        "nested/relative.dcm",
    ),
];

/// The directory argument the viewer is started with.
#[derive(Clone, Copy)]
enum Launch {
    Root,
    /// Through the symbolic link to the root.
    Link,
}

/// One `PUT /api/file/{index}/annotations` made after the CSV has loaded.
struct Edit {
    file: &'static str,
    body: &'static str,
    status: u16,
}

struct ExportCase {
    name: &'static str,
    launch: Launch,
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
        launch: Launch::Root,
        edits: &[],
    },
    // `num_ROI` and `ROI_frames` omitted: the count is derived and the
    // frames column is written as `[]`.
    ExportCase {
        name: "columns-minimal",
        launch: Launch::Root,
        edits: &[],
    },
    // Dataset-style input: extra columns, another column order, spaces
    // inside the JSON and around values. Export has the four columns only,
    // with compact JSON. A row with no ROIs and a row whose path matches no
    // loaded file (its payload is never parsed) produce no output row.
    ExportCase {
        name: "columns-extra",
        launch: Launch::Root,
        edits: &[],
    },
    // The viewer is started on a symbolic link to the directory. A row
    // naming a file through the link and a row naming it by its real path
    // both match, and both are exported under the link path the viewer was
    // given: the directory part of a discovered file's path is not resolved.
    // EXPECTED CHANGE (EMBED parity amendment, 2026-10-05): export will write
    // the resolved path; both spellings keep matching on import.
    ExportCase {
        name: "symlinked-directory",
        launch: Launch::Link,
        edits: &[],
    },
    // What the loader does not check today: boxes past the image edge and
    // far outside it, zero-height and zero-area boxes, and a box with its
    // corners in the wrong order. All load and export exactly as written.
    // Saving the loaded out-of-range row back through the API is refused
    // and leaves it in place.
    // EXPECTED CHANGE (12.12): these rows will also be listed in an import
    // report, and the first edit of such a file will clamp it. The bytes
    // stay: the owner confirmed on 2026-10-08 that a coordinate past the
    // image edge is kept as loaded and exported as written.
    ExportCase {
        name: "unchecked-geometry",
        launch: Launch::Root,
        edits: &[Edit {
            file: "single-frame.dcm",
            body: r#"{"num_roi":5,"roi_coords":[[0,0,161,241],[500,600,700,800],[40,50,40,90],[5,5,5,5],[60,90,20,30]],"roi_frames":[]}"#,
            status: 400,
        }],
    },
    // An empty frame list for one ROI of a file whose other ROI has frames:
    // it loads and exports as `[]`.
    // EXPECTED CHANGE (12.6, `docs/design/output-adapters.md` 6.2): a
    // per-ROI `[]` will mean every frame, and in a file that also has
    // explicit lists it will be exported as every frame index.
    ExportCase {
        name: "frames-empty-for-one-roi",
        launch: Launch::Root,
        edits: &[],
    },
    // A frame list out of order and with a repeat is kept as written. The
    // owner confirmed this stays (EMBED parity amendment, 2026-10-05): an
    // unedited list round-trips unchanged.
    ExportCase {
        name: "frames-unsorted",
        launch: Launch::Root,
        edits: &[],
    },
    // Two files with identical bytes under different names, given different
    // boxes: each keeps its own and both rows are exported.
    // EXPECTED CHANGE (12.2, model 1.4): byte-identical files will share
    // one annotation set, the first row will load with the conflict
    // reported, and one row per path will still be exported.
    ExportCase {
        name: "identical-copies",
        launch: Launch::Root,
        edits: &[],
    },
    // Edits through the API, then export. The first two bodies are what the
    // 0.3 viewer sends after one ROI is added or moved on a file loaded with
    // `ROI_frames` `[]`: it expands `[]` to every frame index for every ROI,
    // and the server exports the lists it was given.
    // EXPECTED CHANGE (12.6): this post-edit expansion is signed off to
    // change. Whether a full explicit list that no edit produced, such as
    // `[[0]]` on a single-frame file, is also written as `[]` is not yet
    // decided.
    ExportCase {
        name: "edited",
        launch: Launch::Root,
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

/// Inputs with one valid row and one invalid matching row. The viewer starts
/// and keeps serving, and export answers 500.
const REJECTED_IMPORTS: &[&str] = &[
    // A frame index equal to the matched file's frame count.
    "rejected-frame-past-end",
    // Two rows naming the same path, with different boxes.
    "rejected-duplicate-path",
    // A negative coordinate, unlike one past the far edge, is not an
    // unsigned integer and fails the row.
    // EXPECTED CHANGE (EMBED parity amendment, 2026-10-05, as amended
    // 2026-10-08): the lenient import will make it 0 and report the row
    // instead. Only a negative or non-integer value is changed on load.
    "rejected-negative-coordinate",
    // `num_ROI` is not the number of boxes.
    "rejected-num-roi-mismatch",
    // A non-empty `ROI_frames` has fewer lists than there are boxes.
    "rejected-frames-length-mismatch",
];

#[tokio::test]
async fn export_bytes_match_the_embed_goldens() {
    for case in EXPORT_CASES {
        let viewer = Viewer::start(case.name, case.launch).await;

        // The first export waits for the CSV import, so every edit lands on
        // loaded state.
        let loaded = viewer.get("/api/annotations/export.csv").await;
        assert_eq!(loaded.status(), 200, "{}: export after load", case.name);

        let indexes = viewer.file_indexes().await;
        for edit in case.edits {
            let index = indexes[&viewer.files.root.join(edit.file)];
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
        let expected = viewer.files.golden(&format!("{}.expected.csv", case.name));
        assert_same_rows(case.name, &actual, expected.as_bytes());
    }
}

#[tokio::test]
async fn rejected_imports_fail_export_and_keep_the_viewer_serving() {
    for name in REJECTED_IMPORTS {
        let viewer = Viewer::start(name, Launch::Root).await;

        let exported = viewer.get("/api/annotations/export.csv").await;
        assert_eq!(exported.status(), 500, "{name}: export");
        let body: Value = exported.json().await.expect("error envelope");
        assert_eq!(body["code"], "internal_error", "{name}");

        let health = viewer.get("/api/health").await;
        assert_eq!(health.status(), 200, "{name}: health");
    }
}

/// A CSV without a required column stops the process before it serves:
/// exit status 1 and no startup event.
#[tokio::test]
async fn a_missing_required_column_fails_startup() {
    let case = "startup-missing-roi-coords-column";
    let files = CaseFiles::create(case);
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        files
            .command(Launch::Root)
            .spawn()
            .expect("spawn dcmview")
            .wait_with_output(),
    )
    .await
    .unwrap_or_else(|_| panic!("{case}: dcmview kept running\n{}", files.stderr()))
    .expect("dcmview output");

    assert_eq!(output.status.code(), Some(1), "{case}\n{}", files.stderr());
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(STARTUP_EVENT),
        "{case}: the viewer came up"
    );
}

/// Header first and exact, then the same rows in any order, each compared
/// with its line terminator.
/// EXPECTED CHANGE (12.11): once export sorts rows by path, compare the
/// whole body.
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

const STARTUP_EVENT: &str = "{\"type\":\"server_started\"";

/// A fresh copy of the matched files, a symbolic link to their directory,
/// and one golden input written out as the `--annotations` CSV.
struct CaseFiles {
    root: PathBuf,
    link: PathBuf,
    csv_path: PathBuf,
    stderr_path: PathBuf,
    _directory: TempDir,
}

impl CaseFiles {
    fn create(case: &str) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        // Canonical, because export writes canonical paths.
        let base = directory.path().canonicalize().expect("canonical temp dir");
        let root = base.join("data");
        let link = base.join("linked");
        assert!(
            !base
                .to_string_lossy()
                .contains([',', '"', '\n', '\r', '{', '}']),
            "the temp directory {} would change how paths are quoted",
            base.display()
        );
        for (fixture, name) in MATCHED_FILES {
            let target = root.join(name);
            fs::create_dir_all(target.parent().expect("parent")).expect("create directory");
            fs::copy(fixtures().join(fixture), target).expect("copy fixture");
        }
        std::os::unix::fs::symlink(&root, &link).expect("link the root");

        let files = Self {
            csv_path: base.join("annotations.csv"),
            stderr_path: base.join("stderr.log"),
            root,
            link,
            _directory: directory,
        };
        fs::write(&files.csv_path, files.golden(&format!("{case}.input.csv")))
            .expect("write annotations CSV");
        files
    }

    /// A golden file with its placeholders replaced by this case's paths.
    fn golden(&self, name: &str) -> String {
        fs::read_to_string(golden_path(name))
            .expect("read golden")
            .replace(ROOT_PLACEHOLDER, &self.root.to_string_lossy())
            .replace(LINK_PLACEHOLDER, &self.link.to_string_lossy())
    }

    /// The real binary over this case. The working directory is the root so
    /// that relative CSV paths resolve against it.
    fn command(&self, launch: Launch) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dcmview"));
        command
            .arg("--startup-json")
            .arg("--no-browser")
            .arg("--annotations")
            .arg(&self.csv_path)
            .arg(match launch {
                Launch::Root => &self.root,
                Launch::Link => &self.link,
            })
            .current_dir(&self.root)
            .env_remove("DCMVIEW_TOKEN")
            .env("DCMVIEW_VSCODE_BYPASS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&self.stderr_path).expect("create stderr log"))
            .kill_on_drop(true);
        command
    }

    /// What the process has written to standard error, for failure messages.
    fn stderr(&self) -> String {
        format!(
            "--- dcmview stderr\n{}",
            fs::read_to_string(&self.stderr_path).unwrap_or_default()
        )
    }
}

/// A running `dcmview` process over one case's files.
struct Viewer {
    files: CaseFiles,
    base_url: String,
    token: String,
    client: reqwest::Client,
    _stdout: Lines<BufReader<ChildStdout>>,
    _child: Child,
}

impl Viewer {
    async fn start(case: &str, launch: Launch) -> Self {
        let files = CaseFiles::create(case);
        let mut child = files.command(launch).spawn().expect("spawn dcmview");

        let mut stdout = BufReader::new(child.stdout.take().expect("stdout")).lines();
        let started = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let line = stdout
                    .next_line()
                    .await
                    .expect("read stdout")
                    .unwrap_or_else(|| {
                        panic!(
                            "{case}: dcmview exited before it started\n{}",
                            files.stderr()
                        )
                    });
                if line.starts_with(STARTUP_EVENT) {
                    return serde_json::from_str::<Value>(&line).expect("startup JSON");
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{case}: no startup event\n{}", files.stderr()));

        Self {
            files,
            base_url: started["base_url"].as_str().expect("base_url").to_string(),
            token: started["token"].as_str().expect("token").to_string(),
            client: reqwest::Client::new(),
            _stdout: stdout,
            _child: child,
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
            .unwrap_or_else(|error| panic!("GET {path}: {error}\n{}", self.files.stderr()))
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
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden_path(name: &str) -> PathBuf {
    fixtures().join("embed-goldens").join(name)
}
