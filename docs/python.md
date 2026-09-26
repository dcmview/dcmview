# dcmview Python Reference

The `dcmview-py` package exposes a small Python wrapper around the `dcmview`
Rust binary. It is intended for scripts and notebooks that have already selected
local DICOM files or directories and need a temporary viewer for research or
development inspection.

`dcmview` is not for clinical diagnosis. The local HTTP server is
unauthenticated; keep it bound to `127.0.0.1` unless you have added your own
network access controls.

## Install

```bash
python -m pip install dcmview-py
python -m dcmview_py --help
```

Release automation builds bundled wheels for Linux x86_64
(`manylinux_2_28_x86_64`), macOS x86_64, macOS arm64, and Windows x86_64. On
other platforms, or when testing a local build, set `DCMVIEW_BINARY` to an
absolute or user-expanded path to a compatible `dcmview` executable.

## Basic Use

```python
from dcmview_py import view

# Blocking call. This returns after the viewer exits.
view("./scan.dcm")
```

Use a list or tuple to inspect multiple files or directories:

```python
view(["./scan.dcm", "./study_dir"])
```

## Non-Blocking Use

Set `block=False` when a notebook or script needs to keep running while the
viewer stays open:

```python
from dcmview_py import view

handle = view("./study_dir", browser=False, block=False)
print(handle.url)

# Later, stop the viewer process.
exit_code = handle.stop()
```

The handle is a `ShutdownHandle` whether the viewer runs locally or in VS Code.
It provides:

| Attribute or method | Behavior |
|---|---|
| `url` | Viewer URL when startup has reported one. |
| `stop(timeout=5.0)` | Ask the viewer to stop, wait for exit, and return the exit code. |
| Context manager | Calls `stop()` automatically on context exit. |

`stop()` is idempotent for local handles after the process has already exited.
It first requests graceful process shutdown and waits; the Rust startup
lifecycle cancels and awaits any in-progress DICOM discovery before a normal
server exit completes. If the process does not stop within the requested
timeout, the wrapper escalates to terminate and then kill.

## Context Manager

Use a context manager when a script should always clean up the viewer:

```python
from dcmview_py import view

with view("./study_dir", browser=False, block=False) as handle:
    print(handle.url)
    # Run analysis while the viewer is available.
```

## Parameters

`view()` accepts one required argument and keyword-only launch options:

```python
view(
    files,
    *,
    port=0,
    host="127.0.0.1",
    browser=True,
    block=True,
    recursive=True,
    timeout=None,
    annotations=None,
    filters=None,
    vscode_bridge=True,
)
```

| Parameter | Default | Description |
|---|---:|---|
| `files` | required | A path-like value or iterable of path-like values to DICOM files or directories. |
| `port` | `0` | Local HTTP port. `0` asks the OS for an available port. |
| `host` | `"127.0.0.1"` | Local interface to bind. Keep the default for normal and SSH-forwarded use. |
| `browser` | `True` | Open the system browser. Use `False` to print and capture the URL instead. |
| `block` | `True` | Wait for the viewer to exit and return `None`. When `False`, return a shutdown handle. |
| `recursive` | `True` | Recursively scan input directories. |
| `timeout` | `None` | Exit after this many seconds without API or browser requests. |
| `annotations` | `None` | Load an EMBED-style ROI annotation CSV into memory without modifying the file. |
| `filters` | `None` | Iterable of `FIELD=VALUE` metadata filters. Values are forwarded as repeatable `--filter` flags and combined with AND semantics. |
| `vscode_bridge` | `True` | Open the viewer in VS Code when run from a VS Code terminal or inside an open workspace folder. |

Filter fields are the same as the Rust CLI: `patient_id`, `patient_name`,
`study_description`, `study_date`, `study_uid`, `series_description`,
`series_number`, `series_uid`, and `modality`. Matching is case-insensitive
substring matching.

## Examples

Blocking inspection with an idle timeout:

```python
from dcmview_py import view

view("./scan.dcm", browser=False, timeout=300)
```

Non-recursive directory scan:

```python
view("./study_dir", recursive=False)
```

Annotation loading:

```python
view("./study_dir", annotations="./rois.csv")
```

Metadata filters:

```python
view(
    "./study_dir",
    filters=["Modality=CT", "PatientID=phantom"],
)
```

Remote server workflow:

```python
view(
    "/data/study_dir",
    browser=False,
    host="127.0.0.1",
    port=8010,
    timeout=600,
)
```

Then forward the port from your local machine:

```bash
ssh -L 8010:127.0.0.1:8010 user@remote
```

Open `http://127.0.0.1:8010` locally.

## Return Values and Errors

Blocking calls return `None` after a successful viewer exit. Non-blocking calls
return a shutdown handle.

The wrapper may raise:

| Exception | When it can happen |
|---|---|
| `ValueError` | No files were provided. |
| `TypeError` | File, annotation, or filter arguments have invalid types. |
| `RuntimeError` | No binary can be resolved, or startup fails before a handle is available. |
| `subprocess.CalledProcessError` | The underlying viewer exits with a non-zero status. |

## Binary Resolution

The Python wrapper resolves the binary in this order:

1. `DCMVIEW_BINARY`, when set. The value may include `~`, but must point to an
   existing file.
2. The bundled wheel binary under `dcmview_py/bin/`.
3. `dcmview` or `dcmview.exe` on `PATH`.

When launching a local subprocess, the wrapper sets `DCMVIEW_VSCODE_BYPASS=1`
for the child process so that the Rust binary does not recursively route itself
back into VS Code interception.

## VS Code Bridge

With the dcmview VS Code extension active, `view()` opens the viewer in a VS
Code webview panel in two cases: when Python runs in a VS Code integrated
terminal, or when its working directory is inside an open workspace folder.
The second case covers notebooks started from the workspace. Anywhere else, it
launches the local viewer. The wrapper hands this to the `dcmview` binary, so
Python, terminal, and shell launches follow the same rule. `url`, `stop()`,
and blocking calls behave the same for a VS Code-managed viewer; `stop()`
closes the VS Code viewer.

Set `vscode_bridge=False` for one call:

```python
view("./scan.dcm", vscode_bridge=False)
```

Or set an environment variable before starting Python:

```bash
export DCMVIEW_VSCODE_BYPASS=1
```

Set `DCMVIEW_VSCODE_BRIDGE_DEBUG=1` to print bridge discovery diagnostics to
stderr. The bridge registry location and related variables are documented in the
[configuration reference](configuration.md).

## Module CLI

The package also provides a module CLI:

```bash
python -m dcmview_py --no-browser --timeout 120 ./study_dir
```

`python -m dcmview_py` and the `dcmview`/`dcmview-py` console scripts forward
their arguments unchanged to the resolved binary, so options, `--help`, and
`--version` are the Rust CLI's own. They follow the same VS Code routing rule as
`view()`, wait for the viewer to exit, and return its exit code; Ctrl+C stops
the viewer. Use the Python API when a script or notebook needs a non-blocking
handle.

## Related Documentation

- [Configuration reference](configuration.md)
- [Troubleshooting guide](troubleshooting.md)
- [VS Code extension local testing](vscode-extension-local-testing.md)
