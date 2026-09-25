# dcmview Configuration Reference

This page centralizes the user-facing configuration surfaces for `dcmview`.
`dcmview` is intentionally ephemeral: it does not read project config files,
write viewer state, or use a database. Configuration comes from command-line
flags, Python wrapper arguments, VS Code settings, and a small set of
environment variables.

Keep the server bound to `127.0.0.1` unless you have added your own network
access controls. The viewer server is unauthenticated.

## Rust CLI

The Rust binary is the source of truth for viewer startup:

```text
dcmview [OPTIONS] <PATH> [PATH ...]
```

| Option | Default | Description |
|---|---:|---|
| `<PATH>...` | required | DICOM file or directory to inspect; repeat for multiple inputs. |
| `-p, --port <PORT>` | `0` | Local HTTP port to bind. `0` asks the OS for an available port. |
| `--host <ADDR>` | `127.0.0.1` | Local interface to bind. Keep the default for normal and SSH-forwarded use. |
| `--no-browser` | `false` | Print the viewer URL instead of opening a browser automatically. |
| `--timeout <SECONDS>` | none | Exit after this many seconds without API or browser requests. |
| `--no-recursive` | `false` | Scan only the top level of input directories. |
| `--annotations <CSV>` | none | Load EMBED-style ROI annotations from CSV without modifying the file. |
| `--filter <FIELD=VALUE>` | none | Include only files whose metadata field contains the value; repeatable. |

Filter fields are `patient_id`, `patient_name`, `study_description`,
`study_date`, `study_uid`, `series_description`, `series_number`,
`series_uid`, and `modality`. Matching is case-insensitive substring matching;
multiple filters are combined with AND semantics.

`--startup-json` and `--vscode-bridge-client` are hidden integration flags for
wrappers and VS Code terminal interception. They are not part of the normal user
interface.

## Python Module CLI

The module CLI mirrors the Rust CLI and forwards options to the resolved
`dcmview` binary:

```text
python -m dcmview_py [OPTIONS] <PATH> [PATH ...]
```

| Python CLI option | Forwarded Rust option |
|---|---|
| `<PATH>...` | `<PATH>...` |
| `-p, --port <PORT>` | `--port <PORT>` |
| `--host <ADDR>` | `--host <ADDR>` |
| `--no-browser` | `--no-browser` |
| `--timeout <SECONDS>` | `--timeout <SECONDS>` |
| `--no-recursive` | `--no-recursive` |
| `--annotations <CSV>` | `--annotations <CSV>` |
| `--filter <FIELD=VALUE>` | `--filter <FIELD=VALUE>` |

The module CLI runs in blocking mode and returns the binary exit code.

## Python `view()` Parameters

`dcmview_py.view()` is a subprocess wrapper around the Rust binary. It accepts a
single path-like value or an iterable of path-like values.

| Parameter | Default | Behavior |
|---|---:|---|
| `files` | required | One path or an iterable of paths to DICOM files or directories. |
| `port` | `0` | Forwards to `--port`. |
| `host` | `"127.0.0.1"` | Forwards to `--host`. |
| `browser` | `True` | When `False`, forwards `--no-browser`. |
| `block` | `True` | When `True`, waits for `dcmview` to exit and returns `None`; when `False`, returns a handle with `.url`, `.stop()`, and context-manager support. |
| `recursive` | `True` | When `False`, forwards `--no-recursive`. |
| `timeout` | `None` | Forwards to `--timeout` when set. |
| `annotations` | `None` | Path to an EMBED-style ROI CSV; forwards `--annotations` when set. |
| `filters` | `None` | Iterable of `FIELD=VALUE` filters; each value forwards as `--filter`. |
| `vscode_bridge` | `True` | When `True`, the wrapper may route launches into an active VS Code dcmview bridge. |

The wrapper adds `--startup-json` when launching the binary directly so it can
discover the server URL reliably. If the binary does not support that hidden
flag, the wrapper retries without it.

## VS Code Settings

VS Code settings are read from the `dcmview` configuration namespace.

| Setting | Default | Behavior |
|---|---:|---|
| `dcmview.binaryPath` | `""` | Absolute path to a `dcmview` binary override. |
| `dcmview.defaultRecursive` | `true` | Scan selected folders recursively when launching from VS Code. |
| `dcmview.extraArgs` | `[]` | Additional command-line arguments passed to `dcmview` before selected paths. |
| `dcmview.startupTimeoutSeconds` | `20` | Seconds to wait for the local server URL. |
| `dcmview.terminalInterception.enabled` | `true` | Route `dcmview`, `dcmview-py`, and `python -m dcmview_py` launched from new integrated terminals into VS Code webviews. |

The extension launches selected paths with:

```text
dcmview --no-browser --port 0 --host 127.0.0.1 --startup-json [extra args] <PATH>...
```

If `dcmview.defaultRecursive` is `false`, it also adds `--no-recursive`.
Arguments from `dcmview.extraArgs` are appended before selected paths, so they
can set filters, annotations, timeouts, and similar Rust CLI options.

## Binary Resolution

Python and VS Code resolve binaries independently.

Python `dcmview_py` resolution order:

1. `DCMVIEW_BINARY`, when set. The value must point to an existing file.
2. The bundled wheel binary under `python/dcmview_py/bin/`.
3. `dcmview` or `dcmview.exe` on `PATH`.

VS Code extension resolution order:

1. `dcmview.binaryPath`, when set.
2. A repository debug binary at `target/debug/dcmview` or
   `target/debug/dcmview.exe`, useful during extension development.
3. The Marketplace-bundled binary under
   `resources/bin/<platform>-<arch>/`.
4. `dcmview` or `dcmview.exe` on `PATH`.

When terminal interception is active and VS Code cannot resolve a local binary,
the bridge may accept a trusted absolute client binary path from a Python
wrapper launch. Trusted client paths must be absolute, named `dcmview` or
`dcmview.exe`, point to a file, and on Unix must be owned by the current user
and not group- or world-writable.

## Runtime Environment Variables

These variables affect viewer launch and VS Code bridge routing at runtime.

| Variable | Used by | Behavior |
|---|---|---|
| `DCMVIEW_BINARY` | Python wrapper | Absolute or user-expanded path to the Rust binary. Overrides bundled wheels and `PATH`. |
| `DCMVIEW_VSCODE_BYPASS` | Rust binary, Python wrapper, VS Code shims | Set to `1` to bypass VS Code bridge discovery and launch a normal local process. |
| `DCMVIEW_VSCODE_BRIDGE_URL` | Rust binary, Python wrapper, VS Code extension | Explicit VS Code bridge URL for terminal interception. Usually managed by the extension. |
| `DCMVIEW_VSCODE_BRIDGE_TOKEN` | Rust binary, Python wrapper, VS Code extension | Bearer token for the explicit bridge URL. Usually managed by the extension. |
| `DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR` | Rust binary, Python wrapper, VS Code extension | Override the bridge registry directory used for out-of-band discovery. |
| `DCMVIEW_VSCODE_BRIDGE_DEBUG` | Rust binary, Python wrapper | Set to `1` to print bridge discovery diagnostics to stderr. |
| `XDG_STATE_HOME` | Rust binary, Python wrapper | Preferred base directory for bridge registry files on Unix-like systems when absolute. |
| `XDG_RUNTIME_DIR` | Rust binary, Python wrapper | Legacy bridge registry fallback when absolute. |

Bridge registry entries expire after three hours. Registry directories must be
trusted on Unix: owned by the current user and not group- or world-writable.

## Build and Development Environment Variables

Source builds require Rust 1.88+ and Node.js 20.19+ with npm. These variables
affect source builds only; `build.rs` reads them while Cargo prepares embedded
frontend assets.

| Variable | Behavior |
|---|---|
| `DCMVIEW_SKIP_FRONTEND_BUILD` | When set to `1`, `true`, `TRUE`, `yes`, or `YES`, skips `npm run build` and requires `frontend/dist/index.html` to already exist. |
| `DCMVIEW_NODE_PATH` | Absolute path to a `node` executable to use during frontend build checks. |
| `DCMVIEW_NPM_PATH` | Absolute path to an `npm` executable to use for `npm ci` and `npm run build`. |

`DCMVIEW_NODE_PATH` and `DCMVIEW_NPM_PATH` must be absolute paths when set, and
the referenced tools must run with `--version`.

## Debug API Feature

The `debug-api` Cargo feature enables permissive CORS for the local viewer API.
It exists for dcmview debugging and test automation, not for normal
distribution:

```bash
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo run --features debug-api -- ./study_dir
```

Builds with this feature emit a warning. Do not enable it for ordinary local or
remote inspection workflows.
