# VS Code extension local testing

The VS Code extension is a local development wrapper around the existing
`dcmview` binary. Marketplace VSIX builds are target-specific and bundle release
binaries for Linux x64, macOS x64, macOS arm64, and Windows x64.

## One-time setup

```bash
npm --prefix vscode install
cargo build
```

The extension resolves binaries in this order:

1. `dcmview.binaryPath` VS Code setting.
2. `target/debug/dcmview` from this repository, or `target/debug/dcmview.exe`
   on Windows.
3. `vscode/resources/bin/<platform>-<arch>/dcmview` bundled in Marketplace VSIX
   builds, where supported platform directories are `linux-x64`, `darwin-x64`,
   `darwin-arm64`, and `win32-x64`. Windows uses `dcmview.exe`.
4. `dcmview` on `PATH`.

When `dcmview.terminalInterception.enabled` is true, the extension starts a
private loopback bridge, publishes it in a per-user registry under
`$XDG_STATE_HOME/dcmview/vscode-bridges` or
`~/.local/state/dcmview/vscode-bridges`, and prepends generated shims to new
integrated terminals when a local `dcmview` binary is available. Open a fresh
terminal after the Extension Development Host starts so the terminal receives
the bridge environment and PATH shims.

The registry lets processes that did not inherit the integrated-terminal
environment route into VS Code. For one release, readers also scan the legacy
`$XDG_RUNTIME_DIR/dcmview/vscode-bridges` and
`/tmp/dcmview-vscode-bridges-$USER` locations so updated wrappers can still find
an older extension. The extension refreshes the registry hourly, re-publishes it
when terminals open, and checks once per minute that the file still exists;
entries expire after 3 hours so crash leftovers do not affect future sessions
indefinitely.

Support matrix:

| Launcher | Discovery mechanism | Notes |
|---|---|---|
| New integrated terminal `dcmview` / `dcmview-py` | PATH shim plus bridge env | Requires a fresh terminal after extension activation. |
| Existing integrated terminal | Bridge env if already present, then registry fallback | Long-lived shells heal stale env endpoints by trying the registry next. |
| tmux/screen started from an old terminal | Stale inherited env, then registry fallback | Restart the tmux server if you need refreshed PATH shims. |
| Plain SSH shell outside VS Code | Registry fallback | Routes only when cwd is inside a VS Code workspace root. |
| VS Code Jupyter kernel | Registry fallback | Routes only when the kernel's cwd is inside a VS Code workspace root. |

Pass `vscode_bridge=False` to `dcmview_py.view(...)` or set
`DCMVIEW_VSCODE_BYPASS=1` to opt out. Set
`DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR` only for testing custom registry locations.
Set `DCMVIEW_VSCODE_BRIDGE_DEBUG=1` before launching Python or Rust clients to
print registry directories, accepted entries, skipped entries, and connection
failures to stderr. Use the `dcmview: Show Bridge Status` command in VS Code to
show the active bridge URL, registry path, last publish time, and session count.
For long-running Remote-SSH deployments, `loginctl enable-linger` can keep the
VS Code remote server alive across login-session churn, but the bridge registry
no longer depends on `/run/user/<uid>` being present.

## Run in an Extension Development Host

1. Open this repository in VS Code.
2. Run the `Run dcmview VS Code Extension` launch configuration.
3. In the Extension Development Host, right-click a fixture such as
   `tests/fixtures/golden-uncompressed-u16-multiframe.dcm`.
4. Choose `Open with dcmview`.
5. Confirm the dcmview panel opens, frame navigation works, and closing the
   panel stops the spawned server.
6. Open the same fixture with `Reopen With...` and choose `dcmview`.
7. Confirm the readonly editor tab opens, frame navigation works, and closing
   the tab stops the spawned server.

Folder testing uses the same context menu action. Use `tests/fixtures/` to
verify multi-file discovery.

## Terminal interception testing

In a fresh integrated terminal in the Extension Development Host:

```bash
dcmview tests/fixtures/golden-uncompressed-u16-multiframe.dcm
python -m dcmview_py --no-browser tests/fixtures/golden-uncompressed-u16-multiframe.dcm
dcmview-py --no-browser tests/fixtures/golden-uncompressed-u16-multiframe.dcm
```

Each command should open a `dcmview` webview panel instead of opening an
external browser. Closing the panel should unblock the terminal command. Pressing
Ctrl+C in the terminal should stop the extension-managed session.

Python API calls inherit the same bridge when run from a fresh integrated
terminal:

```bash
python - <<'PY'
from dcmview_py import view
with view(["tests/fixtures/golden-uncompressed-u16-multiframe.dcm"], block=False) as handle:
    print(handle.url)
PY
```

Set `DCMVIEW_VSCODE_BYPASS=1` in the terminal to run the normal local CLI or
Python wrapper path without extension interception.

## Command testing checklist

- `dcmview: Open with dcmview` opens selected files and folders.
- `dcmview: Open Workspace with dcmview` opens the selected workspace folder.
- `dcmview: Stop All dcmview Sessions` terminates all running child processes.
- `dcmview: Show Bridge Status` reports the bridge URL and registry path.
- `Reopen With...` offers `dcmview` for `*.dcm`, `*.dicom`, and `*.ima` files.
- Setting `dcmview` as the default editor for matching DICOM extensions makes
  double-click open those files in a dcmview editor tab.
- Extensionless DICOM files still open through the explicit context menu
  command.
- Annotation export from the iframe downloads or prompts correctly in the local
  VS Code build.
- `dcmview.binaryPath` overrides the repo debug binary.
- `dcmview.defaultRecursive=false` passes `--no-recursive`.
- `dcmview.extraArgs` are appended before selected paths.
- Fresh integrated terminal `dcmview ...` opens a webview panel and blocks until
  the extension-managed session exits.
- Fresh integrated terminal `python -m dcmview_py ...` and `dcmview-py ...` open
  webview panels.
- VS Code Jupyter kernels connected to the same remote workspace discover the
  bridge registry and open webview panels for `dcmview_py.view(...)` calls.
- Python `dcmview_py.view(..., block=False)` returns a handle whose `stop()`
  closes the extension-managed session.
- `DCMVIEW_VSCODE_BYPASS=1` disables interception for the current command.

## Automated checks

```bash
npm --prefix vscode run compile
npm --prefix vscode test
```

`npm --prefix vscode test` uses `@vscode/test-electron` and may download a VS
Code test build into `vscode/.vscode-test/`.

## Target-specific VSIX packaging

Release automation stages one binary at a time from the platform release
archives into `vscode/resources/bin/**` and packages target-specific VSIX
artifacts:

```bash
npm --prefix vscode run package:release
```

That command expects downloaded release artifacts under `artifacts/`. For local
packaging experiments, create matching `dcmview-*-<target>.tar.gz` archives for
`x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, and
`aarch64-apple-darwin`, plus a `dcmview-*-x86_64-pc-windows-msvc.zip` archive
containing `dcmview.exe`, or use `dcmview.binaryPath` to test a local binary
without bundling.

Install or update a target VSIX from VS Code with `Extensions: Install from
VSIX...`. If the bundled binary is not right for the host, set
`dcmview.binaryPath` to an absolute path and reload the extension host.
