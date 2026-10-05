# dcmview Python Reference

The `dcmview-py` package exposes a small Python wrapper around the `dcmview`
Rust binary. It is intended for scripts and notebooks that have already selected
local DICOM files or directories and need a temporary viewer for research or
development inspection.

`dcmview` is not for clinical diagnosis. The local HTTP API requires the
session's bearer token by default; the launch URL carries it in `#token=...`.
Keep the server bound to `127.0.0.1` and use SSH forwarding for remote work:
plain HTTP does not encrypt the token or DICOM data. The binary inherits
`DCMVIEW_TOKEN` when a fixed token is needed. Direct API clients must send
`Authorization: Bearer <token>`; see the [API reference](api.md).

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
| `url` | Read-only launch URL, verbatim from startup, including the token fragment when enabled. |
| `token` | Read-only `str \| None`: bearer token reported at startup; `None` when absent or under `--no-token`. |
| `base_url` | Read-only `str \| None`: bare HTTP origin reported at startup, without the token fragment; `None` when absent. |
| `stop(timeout=5.0)` | Ask the viewer to stop, wait for exit, and return the exit code. |
| Context manager | Calls `stop()` automatically on context exit. |

Older binaries that report only `url` leave `token` and `base_url` as `None`.
For a local launch with a current binary, call the API with the bearer header:

```python
import json
from urllib.request import Request, urlopen
from dcmview_py import view

with view("./study_dir", browser=False, block=False, vscode_bridge=False) as handle:
    request = Request(
        handle.base_url + "/api/files",
        headers={"Authorization": f"Bearer {handle.token}"},
    )
    with urlopen(request) as response:
        catalog = json.load(response)
```

Viewers started with `block=False` are stopped when the Python interpreter
exits, whether or not the handle is still referenced, so a finished script or a
restarted notebook kernel does not leave a server running. Use a blocking call
or the `dcmview` command when the viewer should outlive the script.

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
    mask=False,
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
| `timeout` | `None` | Exit after this many seconds without API or browser requests once the scan has finished. |
| `annotations` | `None` | Load an EMBED-style ROI annotation CSV into memory without modifying the file. |
| `filters` | `None` | Iterable of `FIELD=VALUE` metadata filters, where `FIELD` is a snake_case name or DICOM keyword (`modality` or `Modality`; see the [configuration reference](configuration.md) for the list). Values are forwarded as repeatable `--filter` flags and combined with AND semantics. |
| `mask` | `False` | Replace patient identifiers in everything the viewer displays, for screen sharing. Display only: files are not modified and this is not de-identification. |
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
    filters=["Modality=CT", "patient_id=phantom"],
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

Open the printed `http://localhost:8010/#token=...` launch URL locally,
keeping its token fragment.

## Return Values and Errors

Blocking calls return `None` after a successful viewer exit. Non-blocking calls
return a shutdown handle once the viewer has found DICOM files (or VS Code has
taken the launch), usually within milliseconds; if the scan finds none, the
call raises `subprocess.CalledProcessError` instead of returning a handle whose
viewer has already exited.

The wrapper may raise:

| Exception | When it can happen |
|---|---|
| `ValueError` | No files were provided. |
| `TypeError` | File, annotation, or filter arguments have invalid types. |
| `RuntimeError` | No binary can be resolved. |
| `subprocess.CalledProcessError` | The underlying viewer exits with a non-zero status. |

## Binary Resolution

The Python wrapper resolves the binary in this order:

1. `DCMVIEW_BINARY`, when set. The value may include `~`, but must point to an
   existing file.
2. The bundled wheel binary under `dcmview_py/bin/`.
3. `dcmview` or `dcmview.exe` on `PATH`.

With `vscode_bridge=False`, the wrapper sets `DCMVIEW_VSCODE_BYPASS=1` for the
child process so that the binary runs the local viewer instead of routing into
VS Code.

## VS Code Bridge

With the dcmview VS Code extension active, `view()` opens the viewer in a VS
Code webview panel in two cases: when Python runs in a VS Code integrated
terminal, or when its working directory is inside an open workspace folder.
The second case covers notebooks started from the workspace. Anywhere else, it
launches the local viewer. The wrapper hands this to the `dcmview` binary, so
Python, terminal, and shell launches follow the same rule. `url`, `stop()`,
and blocking calls behave the same for a VS Code-managed viewer; `stop()`
closes the VS Code viewer.

The bridge reports a launch URL with its token fragment, so `handle.url`
continues to work. Its session event currently has no separate `token` or
`base_url` fields, so those handle properties remain `None`. A `DCMVIEW_TOKEN`
set only in the launching terminal or notebook is not forwarded through the
bridge: the viewer inherits the extension host's environment and otherwise
generates its own token.

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
