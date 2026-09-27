# Troubleshooting

Use this guide when `dcmview` does not install, start, discover files, decode
frames, open a browser, launch through VS Code, or load annotations as expected.

Do not paste DICOM files, screenshots, full logs, file paths, patient names,
patient identifiers, study identifiers, institution names, access tokens, or
other sensitive data into public issues. DICOM metadata, image pixels,
screenshots, logs, and local paths may contain PHI or sensitive research data.
Share only fully de-identified examples, synthetic fixtures, or redacted error
messages.

## Install Failures

### `pip install dcmview-py` does not provide a working `dcmview`

Symptom: `dcmview --help` is not found after installing the Python package, or
Python raises `dcmview binary not found`.

Likely cause: the selected wheel did not include a binary for the current
platform, the script install directory is not on `PATH`, or the environment is
using a different Python installation than the one used for install.

Fix: install with the Python executable you plan to use:

```bash
python -m pip install --user dcmview-py
python -m dcmview_py --help
```

If the console script directory is not on `PATH`, invoke the module form or add
that directory to `PATH`. On unsupported platforms, build or download a matching
`dcmview` binary and set `DCMVIEW_BINARY` to its absolute path.

### Source builds fail because Rust, Node, or npm is missing or outdated

Symptom: `cargo build`, `cargo install --path .`, or `cargo check` fails while
building frontend assets.

Likely cause: source builds require Rust 1.88+ plus Node.js 20.19+ and npm.
`build.rs` uses Node and npm to produce `frontend/dist/` for embedding in the
Rust binary.

Fix: confirm the active tool versions, install or select newer tools as needed,
then rerun the Cargo command:

```bash
rustc --version
node --version
npm --version
```

If `frontend/dist/index.html` already exists and you only need a Rust check,
use:

```bash
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo check --locked
```

For custom tool locations, set `DCMVIEW_NODE_PATH` and `DCMVIEW_NPM_PATH` to
absolute executable paths.

## Startup And Discovery

### `dcmview: no valid DICOM files found`

Symptom: startup exits with a non-zero status and reports that no valid DICOM
files were found.

Likely cause: the input path is wrong, the directory contains no readable DICOM
files, filters exclude every DICOM file, or the files are not valid DICOM
objects.

Fix: verify the path, try a known single DICOM file, and temporarily remove
`--filter` arguments. For directory inputs, remember that recursive scanning is
enabled by default; use `--no-recursive` only when the DICOM files are directly
inside the selected directory.

### Files are reported as skipped

Symptom: startup or the viewer file registry reports skipped files.

Likely cause: the scan encountered non-DICOM files, unreadable paths, or invalid
DICOM objects. The startup summary counts skips by reason, for example
`(3 skipped: 2 not DICOM (no DICM preamble), 1 unparsable DICOM, ...)`. Files
excluded by metadata filters are counted separately as filtered.

Fix: skipped non-DICOM sidecar files are usually harmless. To see which files
were skipped and why, run with `RUST_LOG=dcmview=debug`, which logs each skipped
path and its reason to stderr. If an expected DICOM file is skipped, check file
permissions and try opening that file directly:

```bash
dcmview ./expected-file.dcm
```

If filters are in use, confirm that the field name and value match the file's
metadata. Filter matching is case-insensitive substring matching.

### The viewer opens before every file appears

Symptom: the server URL is available and the viewer opens, but a large
directory is still adding files.

Expected behavior: `dcmview` binds the local server before starting progressive
discovery so the UI and wrappers do not wait for the entire directory scan.
The file list updates while discovery is active and reports completion when the
scan finishes. Normal server shutdown cancels and awaits any remaining
discovery work before the process exits.

### Port already in use

Symptom: startup fails with an address-in-use error.

Likely cause: another process is already listening on the requested `--port`.

Fix: omit `--port` or set `--port 0` to let the operating system choose an
available port. For remote SSH forwarding, pick a fixed unused port only when
you need a predictable forwarding command:

```bash
dcmview --no-browser --port 8888 ./study_dir
```

### Browser does not open automatically

Symptom: `dcmview` starts but no browser window appears.

Likely cause: the host is headless, the browser opener failed, or the command
was run with `--no-browser`.

Fix: copy the printed URL into a browser that can reach the machine running
`dcmview`. On remote servers, keep `--no-browser` and forward the loopback port
over SSH instead of exposing the server publicly.

## Viewer And Decode Errors

### Image frame returns unsupported transfer syntax

Symptom: the viewer cannot display a file and the API returns
`422 {"code": "unsupported_transfer_syntax", "error": "unsupported transfer syntax: ..."}`.

Likely cause: the file uses a transfer syntax that `dcmview` intentionally does
not decode yet. JPEG-LS Lossless (`.80`) grayscale and JPEG XL Lossless (`.110`)
RGB are supported; JPEG-LS Near-Lossless (`.81`) and JPEG XL `.111`/`.112`
remain unsupported. RLE Lossless is supported for 8/16-bit monochrome and the
common 8-bit RGB, YBR_FULL, YBR_FULL_422, and palette-color layouts. Its 16-bit byte planes
must follow DICOM Annex G most-significant-byte-first ordering; reversed-plane
encodings are not silently guessed.

Fix: convert the file to an uncompressed, JPEG Baseline, JPEG Lossless, or
JPEG 2000 transfer syntax with your normal DICOM tooling, or file a
compatibility issue with the transfer syntax UID and a fully de-identified or
synthetic reproduction case. Do not attach protected DICOM data.

### Tags load but the image is missing

Symptom: the file appears in the file list, but frame requests return 404 or
the UI shows no pixels.

Likely cause: the DICOM object has no Pixel Data, such as a structured report,
or the file metadata was readable but no image frames are present.

Fix: use the tag panel to inspect metadata, or choose an image object with
Pixel Data. Non-image DICOM objects may still be useful for tag inspection.

## Remote Workflows

### Cannot access a remote `dcmview` URL locally

Symptom: `dcmview` prints a remote loopback URL, but opening it on your local
machine fails.

Likely cause: `127.0.0.1` refers to the machine where the command runs. A
remote server's loopback address is not directly reachable from your local
browser.

Fix: start `dcmview` without opening a browser on the remote host, then create
an SSH local port forward from your local machine:

```bash
dcmview --no-browser --port 8888 /path/to/study
ssh -L 8888:127.0.0.1:8888 user@remote-host
```

Open `http://localhost:8888` locally. Keep the server bound to loopback unless
you have separate network access controls.

## VS Code Extension

### VS Code opens a webview when you expected a terminal process

Symptom: running `dcmview`, `dcmview-py`, or `python -m dcmview_py` in the VS
Code integrated terminal opens a VS Code webview instead of only printing a
browser URL.

Likely cause: terminal interception is enabled.

Fix: disable the `dcmview.terminalInterception.enabled` setting, or bypass the
integration for one shell session:

```bash
DCMVIEW_VSCODE_BYPASS=1 dcmview --no-browser ./study_dir
```

### VS Code cannot find or launch the bundled binary

Symptom: the `Open with dcmview` command fails before a viewer appears.

Likely cause: the extension platform is unsupported, the bundled binary is not
present, or a local test environment needs a custom binary.

Fix: set `dcmview.binaryPath` to an absolute path to a compatible `dcmview`
executable. Confirm the binary works outside VS Code with `dcmview --help`
before testing the extension again.

## Annotation CSV Errors

### Annotation load fails on startup

Symptom: `--annotations` exits with an error mentioning a CSV row, required
column, `ROI_coords`, `ROI_frames`, `num_ROI`, frame range, or bounds.

Likely cause: the CSV does not match the EMBED-style annotation contract, a
JSON-valued field is not correctly CSV-quoted, coordinates are outside the
matched image bounds, frame indices are out of range, or `anon_dicom_path` does
not match a loaded DICOM path.

Fix: confirm that the CSV includes `anon_dicom_path` and `ROI_coords`. When
present, `num_ROI` must equal the number of coordinate boxes. `ROI_coords` must
be a JSON array of `[ymin, xmin, ymax, xmax]` boxes, and `ROI_frames` must be a
JSON array of frame-index lists or `[]`. Frame indices are zero-based.

Example:

```csv
anon_dicom_path,num_ROI,ROI_coords,ROI_frames
/path/to/case.dcm,1,"[[80,150,190,260]]","[]"
```

## Reporting Issues Safely

When filing a public issue, include:

- `dcmview --version` output or the package/extension version.
- Operating system and CPU architecture.
- Install channel: PyPI, GitHub Release, VS Code Marketplace, or source build.
- The exact command and a redacted error message.
- Transfer syntax UID, modality, image dimensions, and frame count when relevant
  and safe to share.

Do not include:

- DICOM files unless they are synthetic or explicitly approved for public use.
- Screenshots of real patient or research data.
- Full logs that may contain paths, tags, patient identifiers, or tokens.
- Institution names, user names, host names, or private network details.

For security-sensitive reports, contact the maintainers privately before public
disclosure; see [SECURITY.md](../SECURITY.md).
