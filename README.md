![dcmview](https://raw.githubusercontent.com/dcmview/dcmview/main/dcmview-wordmark-darkmode-opaque-background.png)

# dcmview

`dcmview` is a fast, temporary DICOM viewer for research and development work.
Point it at one or more DICOM files from the command line or Python, and it
starts a local browser viewer for images, tags, cine playback, and rectangular
ROI annotations. Stop the process and the server is gone.

The main problem it solves is remote-server inspection. Medical imaging research
often happens where the data already live: an SSH session, a shared compute
server, or a locked-down institutional network. Viewing those images usually
means choosing between slow notebook plots, setting up a web viewer on the
server, opening firewall ports, or uploading data and annotations into a
third-party cloud tool. `dcmview` keeps the workflow local to the machine with
the files: start the viewer, forward the loopback port over SSH when needed, and
inspect the study in seconds.

`dcmview` is intended for developer and research inspection on secure networks,
not clinical diagnosis. Avoid public-facing server binds; use the default
loopback binding and SSH forwarding for remote workflows.

<!-- dcmview-marketing:start -->
## Viewer gallery

### Cine playback and semantic context

![Chest CT cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/chest-ct-cine.gif)

![DICOM SEG semantic overlay in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/mr-seg-cine.gif)

### Modality coverage

![Chest radiograph in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/radiograph.png)

![Mammography study in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/mammography.gif)

![PET cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/pet-cine.gif)

![Ultrasound cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/ultrasound-cine.gif)

![RT Dose semantic context in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/rt-dose-context.png)

![DICOM whole-slide microscopy context in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/wsi-context.png)

[Source imagery attribution](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/ATTRIBUTION.md)
<!-- dcmview-marketing:end -->

## Why use it?

- Inspect DICOM files where they already are, including remote servers.
- Avoid notebook-based frame rendering for multi-frame studies.
- Keep data off third-party viewers when all you need is quick review.
- Open a browser UI with familiar viewer tools: pan, zoom, scroll,
  window/level, flips, rotation, tags, and cine playback.
- Load, edit, and export rectangular ROI annotations without modifying source
  DICOM files.
- Use the same tool from a shell command, Python script, notebook, VS Code, or
  Cursor.
- Run as an ephemeral server with no database, config file, or persistent state.

## Install

The current public install channels are the Python package, GitHub Releases,
the VS Code Marketplace, and Open VSX for Cursor:

| Platform | Recommended channel | Notes |
|---|---|---|
| Linux x64 | `dcmview-py` or GitHub Releases | PyPI wheels bundle the `dcmview` binary. |
| macOS x64 | `dcmview-py` or GitHub Releases | PyPI wheels bundle the `dcmview` binary. |
| macOS arm64 | `dcmview-py` or GitHub Releases | PyPI wheels bundle the `dcmview` binary. |
| Windows x64 | `dcmview-py`, GitHub Releases, or VS Code Marketplace | PyPI wheels bundle `dcmview.exe`. |
| VS Code | VS Code Marketplace | The extension bundles platform-specific binaries for supported hosts. |
| Cursor | Open VSX | Cursor installs the same target-specific extension packages through its extension marketplace. |
| Other platforms | Source build | Build the Rust binary locally and point wrappers at it when needed. |

Install the Python package:

```bash
python -m pip install --user dcmview-py
dcmview --help
```

The package installs both `dcmview` and `dcmview-py`; `dcmview` is the primary
command. If you are using an unsupported platform or a local debug binary, set
`DCMVIEW_BINARY` to an absolute path to a compatible `dcmview` executable.

Tagged releases always include a generated Homebrew formula. Publishing that
formula to a tap is conditional on the maintainers configuring a separate tap
repository; this repository does not currently advertise a public tap command.
Use PyPI, the VS Code or Cursor extension, GitHub Releases, or a source build
unless a release announcement names a working tap.

Source builds are available for contributors and unsupported platforms:

```bash
cargo install --path .
```

Build prerequisites for source installs:

- Rust 1.88+
- Node.js 20.19+ and npm at build time
- `ssh` on `PATH` only when using SSH forwarding helpers

If install or binary discovery fails, see the
[troubleshooting guide](docs/troubleshooting.md). For all CLI, Python, VS Code,
and environment settings, see the
[configuration reference](docs/configuration.md). For the full documentation
map, see the [documentation index](docs/index.md).

## Quick Start

Open one file:

```bash
dcmview ./scan.dcm
```

Scan a study directory recursively:

```bash
dcmview ./study_dir
```

Run without opening a browser, useful on a remote server:

```bash
dcmview --no-browser ./study_dir
```

When ready, `dcmview` prints a URL:

```text
dcmview: server running at http://127.0.0.1:<port>
```

Press Ctrl+C to stop the server.

If startup reports skipped files, no valid DICOM files, a port conflict, or a
browser launch failure, see the
[troubleshooting guide](docs/troubleshooting.md).

## Remote Server Workflow

The safest default is to keep `dcmview` bound to loopback on the remote machine
and access it through SSH port forwarding. `dcmview` is intended for research
and development use on secure networks; do not expose the server directly to a
public network.

On the remote server:

```bash
dcmview --no-browser --port 8888 /path/to/dicom_or_study_dir
```

On your local machine:

```bash
ssh -L 8888:localhost:8888 user@remote-server
```

Then open:

```text
http://localhost:8888
```

You can also let `dcmview` use an auto-assigned port by omitting `--port`; copy
the printed port into your SSH command. The optional `--tunnel` flags are
available for environments where the `dcmview` process can start the SSH helper
itself.

The HTTP server is unauthenticated. It binds to `127.0.0.1` by default. If you
bind to `0.0.0.0` or another public interface, use your own network access
controls. Anyone who can reach the server may be able to access image pixels,
DICOM tags, file paths, patient identifiers, study identifiers, and in-memory
annotations.

## Python Usage

`dcmview-py` is a small subprocess wrapper around the Rust binary. It is useful
when a script or notebook has already selected the cases to inspect. For the
full parameter reference, lifecycle details, VS Code bridge behavior, and
notebook-oriented examples, see the [Python reference](docs/python.md).

```python
from dcmview_py import view

# Blocking call; returns when dcmview exits.
view(["./scan.dcm"], browser=False, timeout=300)

# Non-blocking call.
handle = view(["./study_dir"], browser=False, block=False)
print(handle.url)
handle.stop()
```

Context manager:

```python
from dcmview_py import view

with view(["./study_dir"], browser=False, block=False) as handle:
    print(handle.url)
```

The module CLI mirrors the Rust options:

```bash
python -m dcmview_py --no-browser --timeout 120 ./study_dir
```

## VS Code and Cursor

The editor extension opens DICOM files or folders in a webview backed by the
same local `dcmview` server in VS Code and Cursor. It can also intercept
`dcmview`, `dcmview-py`, and `python -m dcmview_py` launches from new integrated
terminals so terminal-based workflows appear inside the editor.

See the [editor extension README](vscode/README.md) for install channels,
supported platforms, settings, terminal interception behavior, and local
testing notes.

## Viewer Features

The embedded browser viewer includes:

- File tabs labeled from `PatientID`, `Modality`, and `StudyDate` when present.
- Canvas-based image viewing with pan, zoom, scroll, window/level, reset,
  horizontal/vertical flips, and 90-degree rotation.
- Window presets including DICOM defaults, full dynamic range, and common CT
  presets.
- Multi-frame controls with previous/next, cine playback, FPS selection, loop,
  and sweep.
- Study and directory navigation with typed links between locally resolved
  referenced objects and frames.
- Default pixel preview plus opt-in declared semantic context for SEG,
  Parametric Map, and RT Dose. Validated SEG mappings can compose binary or
  fractional masks over referenced source images even when compatible patient
  geometry uses different matrix dimensions. Semantic panels identify missing,
  ambiguous, or incompatible geometry and do not claim clinical interpretation.
- Positioned single-tile WSI inspection with matrix, row/column, optical-path,
  focal-plane, and minimap context. Tiles are not stitched and the Total Pixel
  Matrix is not reconstructed.
- Lazy DICOM tag browsing with filtering, sequence expansion, binary length
  display, resizable columns, and click-to-copy values.
- Rectangular ROI annotation display and editing, including draw, select, move,
  resize, delete, frame scoping, and CSV export.

DICOMDIR inputs are recognized and skipped with a stable unsupported-media
reason while recursive discovery continues for ordinary DICOM objects. The
viewer does not parse the DICOM file-set hierarchy or advertise DICOM media
support. Unsupported transfer syntaxes and out-of-scope object classes remain
request- or object-scoped so a failure does not poison later viewing.

Common shortcuts:

| Action | Shortcut |
|---|---|
| Previous/next file in the active Explorer ordering | Up/Down arrows |
| Previous/next frame | Left/Right arrows or `[` / `]` |
| Play/pause cine | Space |
| Window/level tool | `W` |
| Pan tool | `P` |
| Zoom tool | `Z` |
| Scroll tool | `S` |
| ROI tool | `R` |
| Reset viewport | Double-click |

Right-drag always zooms, middle-drag always pans, the wheel scrolls frames, and
Ctrl/Cmd+wheel zooms.

## Annotations

`--annotations` loads an EMBED-style rectangular ROI CSV into memory:

```bash
dcmview --annotations ./embed_annotations.csv ./study_dir
```

`dcmview` never modifies the input CSV or DICOM files. Viewer edits stay in
memory and can be downloaded with **Export ROIs**. For the required columns,
coordinate format, frame scoping rules, validation behavior, and examples, see
the [annotation reference](docs/annotations.md).

## CLI Reference

```text
dcmview [OPTIONS] <PATH> [PATH ...]
python -m dcmview_py [OPTIONS] <PATH> [PATH ...]
```

The Python module CLI forwards the same options to the underlying `dcmview`
binary. Run `dcmview --help` or `python -m dcmview_py --help` for command-line
help. For Python wrapper parameters, VS Code settings, environment variables,
filter fields, hidden integration flags, and binary resolution order, see the
[configuration reference](docs/configuration.md) and
[Python reference](docs/python.md).

## HTTP API

The browser UI uses a small local HTTP API. This API is internal to the viewer
and is not a stable public integration surface; use it only for `dcmview`
debugging, smoke tests, and local automation.

See the [internal API reference](docs/api.md) for endpoint summaries,
progressive scan fields, polling guidance, cache headers, transfer syntax
behavior, raw-frame metadata headers, annotation endpoints, and error semantics.

Production builds do not enable cross-origin browser API access for external
debugging tools. To debug the viewer API from another browser origin, build with
`cargo build --features debug-api`; this enables permissive CORS and prints a
build warning. Do not enable `debug-api` outside `dcmview` debugging contexts.

## Development

For local development, install frontend dependencies, run the Rust backend, and
optionally start the standalone Vite frontend:

```bash
npm --prefix frontend ci
dcmview --no-browser --host 127.0.0.1 --port 8888 tests/fixtures
npm --prefix frontend run dev
```

The repository check driver mirrors CI profiles. For a fast development pass
and the full local core suite:

```bash
python scripts/check.py quick --install
python scripts/check.py core --install
```

`quick` checks version parity, generated frontend contracts, frontend types and
behavior, the production frontend build, strict Rust formatting/lints, and
Python unit tests; it does not run the Rust test suite. `core` additionally
regenerates and verifies DICOM fixtures, runs the locked Rust suite, and
compiles the VS Code extension.

See the [development reference](docs/development.md) for source builds, frontend
proxy behavior, fixture policy, test commands, architecture notes, and cache
budget guidance.

## Reporting Issues

Use GitHub issues for DICOM compatibility problems, install failures, and feature
requests. Do not attach DICOM files, screenshots, logs, paths, patient
identifiers, or other sensitive information unless you have fully de-identified
them and have approval to share them publicly.

Before filing an issue, check the
[troubleshooting guide](docs/troubleshooting.md) for common install, startup,
decode, tunnel, VS Code, and annotation CSV problems.

Report suspected security vulnerabilities privately to the maintainers before
public disclosure; see [SECURITY.md](SECURITY.md).

`dcmview` is not for clinical use, clinical diagnosis, or clinical
decision-making.

## License

MIT
