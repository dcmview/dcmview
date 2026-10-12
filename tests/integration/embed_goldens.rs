//! Golden bytes of the EMBED-style ROI CSV that dcmview 0.3.x reads and writes
//! (`docs/design/annotation-model.md` section 11 step 1).
//!
//! Each case in `tests/fixtures/embed-goldens/` runs the real binary with
//! `--annotations` over copies of committed fixtures and reads
//! `GET /api/annotations/export.csv`, so nothing here names the annotation
//! store's Rust API and the cases survive its replacement unchanged.
//!
//! The comparison is on bytes: the whole export must equal the expected
//! file. Rows are sorted by path (the byte order of the path as written),
//! whatever order discovery found the files in
//! (`docs/design/annotation-model.md` 12.11).
//!
//! Each case also states what the import reports
//! (`GET /api/annotations/import`): a negative coordinate read as 0 and one
//! that is not whole rounded (12.12, as amended 2026-10-08), and boxes kept
//! as read although they reach past the image, have no area or have their
//! corners out of order.
//!
//! Later work changes some of these bytes on purpose, with the owner's
//! sign-off. Each is marked `EXPECTED CHANGE` at the case it will touch,
//! with the decision in `docs/design/annotation-model.md` section 12 that
//! signs it off: frame lists after an edit and a per-ROI `[]` (12.6); two
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
    /// The `warnings` of the import's report, as JSON.
    warnings: &'static str,
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
        warnings: r#"[]"#,
    },
    // `num_ROI` and `ROI_frames` omitted: the count is derived and the
    // frames column is written as `[]`.
    ExportCase {
        name: "columns-minimal",
        launch: Launch::Root,
        edits: &[],
        warnings: r#"[]"#,
    },
    // Dataset-style input: extra columns, another column order, spaces
    // inside the JSON and around values. Export has the four columns only,
    // with compact JSON. A row with no ROIs and a row whose path matches no
    // loaded file (its payload is never parsed) produce no output row.
    ExportCase {
        name: "columns-extra",
        launch: Launch::Root,
        edits: &[],
        warnings: r#"[]"#,
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
        warnings: r#"[]"#,
    },
    // What the loader does not check today: boxes past the image edge and
    // far outside it, zero-height and zero-area boxes, and a box with its
    // corners in the wrong order. All load and export exactly as written,
    // and each is listed in the import's report (12.12). Saving the loaded
    // out-of-range row back through the API is refused and leaves it in
    // place. The bytes stay: the owner confirmed on 2026-10-08 that a
    // coordinate past the image edge is kept as loaded and exported as
    // written.
    ExportCase {
        name: "unchecked-geometry",
        launch: Launch::Root,
        edits: &[Edit {
            file: "single-frame.dcm",
            body: r#"{"num_roi":5,"roi_coords":[[0,0,161,241],[500,600,700,800],[40,50,40,90],[5,5,5,5],[60,90,20,30]],"roi_frames":[]}"#,
            status: 400,
        }],
        warnings: r#"[
            {"code": "out_of_range", "count": 2, "rows": [{"row": 2, "roi": 0}, {"row": 2, "roi": 1}]},
            {"code": "zero_area", "count": 2, "rows": [{"row": 2, "roi": 2}, {"row": 2, "roi": 3}]},
            {"code": "corners_out_of_order", "count": 1, "rows": [{"row": 2, "roi": 4}]}
        ]"#,
    },
    // An empty frame list for one ROI of a file whose other ROI has frames:
    // it loads and exports as `[]`, and the import reports it.
    // EXPECTED CHANGE (12.6, `docs/design/output-adapters.md` 6.2): a
    // per-ROI `[]` will mean every frame, and in a file that also has
    // explicit lists it will be exported as every frame index.
    ExportCase {
        name: "frames-empty-for-one-roi",
        launch: Launch::Root,
        edits: &[],
        warnings: r#"[{"code": "frames_empty", "count": 1, "rows": [{"row": 2, "roi": 1}]}]"#,
    },
    // A frame list out of order and with a repeat is kept as written. The
    // owner confirmed this stays (EMBED parity amendment, 2026-10-05): an
    // unedited list round-trips unchanged.
    ExportCase {
        name: "frames-unsorted",
        launch: Launch::Root,
        edits: &[],
        warnings: r#"[]"#,
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
        warnings: r#"[]"#,
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
        warnings: "[]",
    },
    // A negative coordinate is not a pixel edge. It is read as 0 and the
    // box is reported; nothing else of the row changes. In 0.3 this value
    // failed the whole import (owner decision of 2026-10-08, EMBED parity
    // amendment: "keep as loaded and clamp only negative/non-integer
    // values").
    ExportCase {
        name: "corrected-negative-coordinate",
        launch: Launch::Root,
        edits: &[],
        warnings: r#"[{"code": "negative_coordinate", "count": 1, "rows": [{"row": 3, "roi": 0}]}]"#,
    },
    // A coordinate that is not a whole number is rounded to the nearest
    // pixel, halves away from zero, and the box is reported; a whole number
    // written as a decimal or with an exponent is that number and is not
    // reported. A value that the rounding leaves past the image edge stays
    // there, and is reported as such.
    ExportCase {
        name: "corrected-non-integer-coordinate",
        launch: Launch::Root,
        edits: &[],
        warnings: r#"[
            {"code": "non_integer_coordinate", "count": 3,
             "rows": [{"row": 2, "roi": 0}, {"row": 2, "roi": 1}, {"row": 3, "roi": 0}]},
            {"code": "out_of_range", "count": 1, "rows": [{"row": 2, "roi": 1}]},
            {"code": "negative_coordinate", "count": 1, "rows": [{"row": 3, "roi": 0}]}
        ]"#,
    },
];

/// Inputs with one valid row and one invalid matching row. The viewer starts
/// and keeps serving, and export answers 500.
const REJECTED_IMPORTS: &[&str] = &[
    // A frame index equal to the matched file's frame count.
    "rejected-frame-past-end",
    // Two rows naming the same path, with different boxes.
    "rejected-duplicate-path",
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

        let import: Value = viewer
            .get("/api/annotations/import")
            .await
            .json()
            .await
            .expect("import status");
        assert_eq!(import["state"], "ready", "{}: {import}", case.name);
        assert_eq!(import["report"]["adapter"]["id"], "embed", "{}", case.name);
        assert_eq!(import["report"]["direction"], "import", "{}", case.name);
        let warnings: Value = serde_json::from_str(case.warnings).expect("expected warnings");
        assert_eq!(
            import["report"]["warnings"], warnings,
            "{}: what the import reports",
            case.name
        );

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
        assert!(
            actual == expected.as_bytes(),
            "{}: export differs from the golden\n--- exported\n{}--- expected\n{expected}",
            case.name,
            String::from_utf8_lossy(&actual),
        );
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

        // The import's own message is what the export answered with, and
        // nothing of the file was taken.
        let import: Value = viewer
            .get("/api/annotations/import")
            .await
            .json()
            .await
            .expect("import status");
        assert_eq!(import["state"], "failed", "{name}: {import}");
        assert_eq!(import["error"], body["error"], "{name}");
        assert!(import.get("report").is_none(), "{name}: {import}");

        let health = viewer.get("/api/health").await;
        assert_eq!(health.status(), 200, "{name}: health");
    }
}

/// What a session holds can be written as a document and loaded again:
/// the real binary reads a `dcmview.annotations` file given to
/// `--annotations`, in either form and whatever it is named, and then
/// exports the CSV the first session did, byte for byte. That includes
/// boxes the CSV import kept as read and frame lists kept as written.
#[tokio::test]
async fn a_session_reloaded_from_its_document_exports_the_same_csv() {
    // (golden case, export to take, name to load it under, extra arguments)
    let cases: [(&str, &str, &str, &[&str]); 6] = [
        ("unchecked-geometry", "export.json", "annotations.json", &[]),
        // Files with the same bytes each keep the rows written for them.
        ("identical-copies", "export.json", "annotations.json", &[]),
        ("identical-copies", "export.jsonl", "annotations.jsonl", &[]),
        ("frames-unsorted", "export.jsonl", "annotations.jsonl", &[]),
        // The name says nothing: the start of the file does.
        ("frames-empty-for-one-roi", "export.json", "rois.csv", &[]),
        (
            "corrected-non-integer-coordinate",
            "export.jsonl",
            "rois.txt",
            &["--annotations-format", "native"],
        ),
    ];
    for (case, export, name, arguments) in cases {
        let first = Viewer::start(case, Launch::Root).await;
        let document = first.get(&format!("/api/annotations/{export}")).await;
        assert_eq!(document.status(), 200, "{case}: {export}");
        let document = document.bytes().await.expect("document");
        drop(first);

        let files = CaseFiles::with_annotations(name, &document, arguments);
        let second = Viewer::over(case, files, Launch::Root).await;
        let exported = second.get("/api/annotations/export.csv").await;
        assert_eq!(exported.status(), 200, "{case}: reloaded from {name}");
        let actual = exported.text().await.expect("export body");
        // The document names files by key and by path below the loaded
        // directory, so it fits the second session's copy of the files.
        let expected = second.files.golden(&format!("{case}.expected.csv"));
        assert_eq!(actual, expected, "{case}: reloaded from {name}");

        let import: Value = second
            .get("/api/annotations/import")
            .await
            .json()
            .await
            .expect("import status");
        assert_eq!(import["state"], "ready", "{case}: {import}");
        assert_eq!(import["report"]["adapter"]["id"], "native", "{case}");
        assert_eq!(import["report"]["counts"]["unmatched"], 0, "{case}");
    }
}

/// `--annotations-format` decides, whatever the file holds: a document
/// read as EMBED CSV stops the process as any CSV without the columns
/// does, and a CSV read as a document fails its import while the viewer
/// serves.
#[tokio::test]
async fn a_forced_format_is_the_format_the_file_is_read_as() {
    let document = br#"{"format":"dcmview.annotations","version":"1.0"}"#;
    let files = CaseFiles::with_annotations(
        "annotations.json",
        document,
        &["--annotations-format", "embed"],
    );
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        files
            .command(Launch::Root)
            .spawn()
            .expect("spawn dcmview")
            .wait_with_output(),
    )
    .await
    .unwrap_or_else(|_| panic!("dcmview kept running\n{}", files.stderr()))
    .expect("dcmview output");
    assert_eq!(output.status.code(), Some(1), "{}", files.stderr());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(STARTUP_EVENT));

    let csv = b"anon_dicom_path,ROI_coords\nnowhere.dcm,[]\n";
    let files =
        CaseFiles::with_annotations("annotations.csv", csv, &["--annotations-format", "native"]);
    let viewer = Viewer::over("a CSV read as a document", files, Launch::Root).await;
    let exported = viewer.get("/api/annotations/export.csv").await;
    assert_eq!(exported.status(), 500);
    let import: Value = viewer
        .get("/api/annotations/import")
        .await
        .json()
        .await
        .expect("import status");
    assert_eq!(
        import,
        serde_json::json!({
            "state": "failed",
            "error": "annotations record 1 is not JSON of the dcmview.annotations format",
        })
    );
    assert_eq!(viewer.get("/api/health").await.status(), 200);

    // Without the flag the same CSV is the CSV it is.
    let files = CaseFiles::with_annotations("annotations.csv", csv, &[]);
    let viewer = Viewer::over("a CSV", files, Launch::Root).await;
    assert_eq!(
        viewer.get("/api/annotations/export.csv").await.status(),
        200
    );
}

/// Without `--annotations-format`, a file is a document when it starts as
/// one: a `{`, after an optional byte order mark, and the format's name in
/// its first bytes. Any other file is EMBED CSV, and one that is not stops
/// the process as it always has.
#[tokio::test]
async fn a_file_is_a_document_only_when_it_starts_as_one() {
    let case = "frames-unsorted";
    let first = Viewer::start(case, Launch::Root).await;
    let document = first.get("/api/annotations/export.json").await;
    let document = document.bytes().await.expect("document");
    drop(first);

    let mut marked = b"\xEF\xBB\xBF".to_vec();
    marked.extend_from_slice(&document);
    let files = CaseFiles::with_annotations("annotations.json", &marked, &[]);
    let second = Viewer::over(case, files, Launch::Root).await;
    let exported = second.get("/api/annotations/export.csv").await;
    assert_eq!(exported.status(), 200, "after a byte order mark");
    assert_eq!(
        exported.text().await.expect("export body"),
        second.files.golden(&format!("{case}.expected.csv"))
    );
    let import: Value = second
        .get("/api/annotations/import")
        .await
        .json()
        .await
        .expect("import status");
    assert_eq!(import["report"]["adapter"]["id"], "native", "{import}");
    drop(second);

    // JSON that is not a document is not read as one.
    for other in [&br#"{"images":[]}"#[..], b"[]"] {
        let files = CaseFiles::with_annotations("annotations.json", other, &[]);
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            files
                .command(Launch::Root)
                .spawn()
                .expect("spawn dcmview")
                .wait_with_output(),
        )
        .await
        .unwrap_or_else(|_| panic!("dcmview kept running\n{}", files.stderr()))
        .expect("dcmview output");
        assert_eq!(output.status.code(), Some(1), "{}", files.stderr());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(STARTUP_EVENT));
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

const STARTUP_EVENT: &str = "{\"type\":\"server_started\"";

/// A fresh copy of the matched files, a symbolic link to their directory,
/// and one golden input written out as the `--annotations` CSV.
struct CaseFiles {
    root: PathBuf,
    link: PathBuf,
    csv_path: PathBuf,
    stderr_path: PathBuf,
    /// Arguments put before `--annotations`.
    arguments: Vec<&'static str>,
    _directory: TempDir,
}

impl CaseFiles {
    /// The matched files and one golden input as the `--annotations` CSV.
    fn create(case: &str) -> Self {
        let files = Self::with_annotations("annotations.csv", b"", &[]);
        fs::write(&files.csv_path, files.golden(&format!("{case}.input.csv")))
            .expect("write annotations CSV");
        files
    }

    /// The matched files and `contents` as the `--annotations` file `name`,
    /// started with `arguments` as well.
    fn with_annotations(name: &str, contents: &[u8], arguments: &[&'static str]) -> Self {
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
            csv_path: base.join(name),
            stderr_path: base.join("stderr.log"),
            root,
            link,
            arguments: arguments.to_vec(),
            _directory: directory,
        };
        fs::write(&files.csv_path, contents).expect("write annotations file");
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
            .args(&self.arguments)
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
        Self::over(case, CaseFiles::create(case), launch).await
    }

    async fn over(case: &str, files: CaseFiles, launch: Launch) -> Self {
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
