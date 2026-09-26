# JupyterLab extension feasibility

> **Archival note:** This is a point-in-time, non-normative feasibility study,
> not a committed roadmap or description of a shipped JupyterLab integration.
> Some assumptions may have changed. See the
> [current architecture](../architecture.md) and [Python reference](../python.md).

## Summary

Building a JupyterLab extension for `dcmview` is feasible, and the architecture
can mirror the existing VS Code extension almost one-to-one: keep the Rust
binary and HTTP/Svelte viewer as the rendering engine, spawn
`dcmview --no-browser --port 0 --host 127.0.0.1 --startup-json`, parse the
structured startup event, and embed the server URL in an iframe inside a
JupyterLab main-area widget.

Jupyter is in some ways an easier target than VS Code:

- The Python package `dcmview-py` already exists, already bundles the binary on
  supported Linux platforms, and already implements subprocess launch, startup
  URL parsing, and lifecycle management (`python/dcmview_py/wrapper.py`). A
  Jupyter server extension can reuse this directly instead of reimplementing it
  in TypeScript as `vscode/src/extension.ts` does.
- Distribution is a single pip package; there is no per-platform VSIX problem.
- Jupyter offers a zero-extension integration tier (inline notebook embedding
  via `IPython.display`) that VS Code has no equivalent for.

The main challenge, as with VS Code, is the remote case — which is also
`dcmview`'s primary workflow. When JupyterLab runs on a remote server or behind
JupyterHub, the user's browser cannot reach `127.0.0.1` on the machine where
the kernel and DICOM files live. The standard solution is
`jupyter-server-proxy`, which exposes local ports under a URL prefix such as
`/user/<name>/proxy/<port>/`. This is the Jupyter analogue of VS Code's
`vscode.env.asExternalUri`, but with one important difference: it serves the
app under a path prefix rather than a dedicated origin. The current frontend
uses root-relative API paths (`fetch("/api/files")` in
`frontend/src/api.ts:56`), which break under a prefix. **A small frontend
change — switching to relative paths — is therefore required for the remote
Jupyter scenario.** No backend API changes are required.

## Recommended deployment model

### Phase 0: inline notebook embedding (no extension required)

Before any JupyterLab extension exists, `dcmview-py` can gain a notebook
embedding helper:

- `view(..., block=False)` already returns a `ShutdownHandle` with a `.url`
  property (`wrapper.py:87-119`).
- Add a `ShutdownHandle._repr_html_()` (or an explicit
  `handle.show(height=600)` helper) that renders an `<iframe>` pointing at the
  server URL, sized to the cell output area.
- When running locally, the direct `http://127.0.0.1:<port>` URL works as-is.
- When running remotely, the helper should emit a proxy-relative URL
  (`proxy/<port>/` resolved against the notebook server base URL) and document
  that `jupyter-server-proxy` must be installed. This is the same pattern used
  by TensorBoard, Dask, and pyvista for remote notebook embedding.

This tier works in JupyterLab, classic Notebook, and VS Code notebooks, costs a
few dozen lines of Python, and delivers most of the user value (the README
already names notebooks as a target surface). It should ship first.

### Phase 1: JupyterLab extension (server extension + iframe widget)

A standard two-part JupyterLab extension, distributed as one pip package
(e.g. `dcmview-jupyter`) with prebuilt frontend assets:

**Jupyter server extension (Python)** — the analogue of the VS Code extension
host side:

- REST handlers under the Jupyter server (e.g. `POST /dcmview/sessions`,
  `DELETE /dcmview/sessions/<id>`, `GET /dcmview/sessions`) that spawn and
  track `dcmview` child processes. These handlers run on the machine with the
  files, exactly like `"extensionKind": ["workspace"]` guarantees in VS Code.
- Reuse `dcmview_py.view(..., block=False)` for spawn/parse/lifecycle; the
  `--startup-json` flag added for the VS Code extension (`src/main.rs:54`,
  `src/server.rs:101-106`) is reused unchanged.
- Binary resolution policy mirrors VS Code's: user-configured path, the binary
  bundled in `dcmview-py`, then `PATH`.
- Clean up all child processes on Jupyter server shutdown and on idle timeout
  (the existing `--timeout` flag covers the orphan case).

**JupyterLab frontend extension (TypeScript)** — the analogue of the webview
panel:

- A `MainAreaWidget` wrapping an `IFrame` widget pointed at the session URL
  (direct loopback URL locally; `proxy/<port>/` via `jupyter-server-proxy`
  remotely).
- Commands registered in the command palette: open path, open current file
  browser selection, stop all sessions — mirroring `dcmview.openPath`,
  `dcmview.openWorkspaceSelection`, and `dcmview.stopAll`
  (`vscode/package.json:47-63`).
- File browser context-menu entries for `.dcm`/`.dicom` files and directories,
  the analogue of the explorer context menu contribution.
- Settings via the JupyterLab settings registry: binary path, recursive
  default, extra args — mirroring the existing VS Code settings
  (`vscode/package.json:72-105`).
- Widget disposal stops the owning child process, matching the panel-dispose →
  SIGINT linkage in `vscode/src/extension.ts:293-304`.

There is no good JupyterLab analogue of the VS Code terminal-interception
bridge; that feature should simply not be ported.

### Phase 2: document widget for DICOM files

Register a `DocumentRegistry` widget factory for `.dcm`/`.dicom` so
double-clicking a file in the JupyterLab file browser opens it in a dcmview
tab ("Open With → dcmview"). As with the VS Code custom editor, this is
deferred for the same reason given in the VS Code feasibility doc: DICOM
datasets are often directories or extensionless files, and the multi-file
study model fits the command/context-menu flow better than a one-document
editor model.

## Structural changes needed

### New extension package

```text
jupyter/
|-- pyproject.toml            (hatch-jupyter-builder, depends on dcmview-py)
|-- dcmview_jupyter/
|   |-- __init__.py           (server extension entry points)
|   |-- handlers.py           (session REST API)
|   `-- labextension/         (prebuilt frontend assets)
`-- src/
    |-- index.ts              (plugin, commands, context menu)
    `-- widget.ts             (iframe main-area widget)
```

Target JupyterLab 4.x; `jupyter-server-proxy` is an install-time dependency
(or a documented optional dependency for remote use).

### Frontend changes (required for the remote/proxied case)

This is the one place the JupyterLab effort exceeds the VS Code effort, where
no frontend changes were needed. Under `jupyter-server-proxy` the app is
served from `https://<hub>/user/<name>/proxy/<port>/`, so root-relative URLs
resolve against the hub origin and miss the proxy prefix:

- Convert API paths in `frontend/src/api.ts` from `/api/...` to relative
  `./api/...` (e.g. `api.ts:56`, `api.ts:64`, `api.ts:203`), and rework the
  absolute-URL construction at `api.ts:83`
  (`new URL(..., window.location.origin)`) to resolve against the document
  base instead.
- Set Vite `base: "./"` so embedded asset references in `index.html` are
  relative.
- The CSV export link (`api.ts:79`) needs the same treatment.

These changes are backward-compatible: relative paths resolve identically when
the app is served from the origin root, so the CLI, Python, and VS Code
surfaces are unaffected. The alternative — adding a `--base-path` flag to the
Rust server — is not needed because `jupyter-server-proxy` strips the prefix
before forwarding requests.

The frontend makes no WebSocket connections (REST only), which avoids the most
common `jupyter-server-proxy` pain point.

### Backend changes

None required. The hardening done for the VS Code extension
(`--startup-json`, `/api/health`, origin-relative routes, `--timeout`) covers
the JupyterLab wrapper's needs. One optional addition (see risks): a
`--token`-style shared-secret query parameter for multi-user hosts.

### Python changes

- Add the Phase 0 embedding helper (`_repr_html_` / `show()`) and
  proxy-URL derivation to `dcmview_py`.
- Optionally generalize the existing VS Code bridge detection
  (`wrapper.py:181-189`) into a pluggable launch-context mechanism so a
  Jupyter session manager can register itself the same way; not required for
  Phase 1.

## Challenges and risks

- **Remote URL plumbing is the core risk**, exactly as it was for VS Code.
  `jupyter-server-proxy` is the supported mechanism, but it is a third-party
  package that a JupyterHub admin must install; the extension should detect
  its absence and fail with a clear message. Local JupyterLab needs no proxy.
- **Mixed content**: if JupyterLab is served over HTTPS and the extension
  falls back to a direct `http://127.0.0.1:<port>` iframe on a remote host,
  the browser blocks it. Remote sessions must always go through the proxy
  (which inherits the hub's TLS and auth).
- **Multi-user hosts**: on a shared JupyterHub node, an unauthenticated
  loopback server is reachable by other local users. `jupyter-server-proxy`
  adds Jupyter's auth in front of the proxied path, but the raw port remains
  open locally. Per-user containers mitigate this; for shared-process hubs, a
  shared-secret token on the dcmview server would be the proper fix and is the
  only backend change worth considering.
- **Process ownership**: kernels restart and notebooks re-execute; the server
  extension (not the kernel) should own Phase 1 sessions so viewers survive
  kernel restarts, while Phase 0 handles tie servers to kernel lifetime.
  `--timeout` idle-exit is the orphan backstop in both tiers.
- **Keyboard shortcuts**: the viewer's single-letter shortcuts and arrow keys
  must be tested for conflicts with JupyterLab command-mode bindings when
  iframe focus is ambiguous; same class of risk as VS Code, already known.
- **CSV download from an iframe** under a proxied path should be tested
  explicitly; if blocked, add a JupyterLab command that fetches
  `/api/annotations/export.csv` through the server extension.
- **JupyterLab version churn**: target Lab 4.x only; supporting Lab 3.x
  doubles the frontend build matrix for little benefit in 2026.

## Feasibility verdict

Feasible, with lower packaging risk and slightly higher frontend risk than the
VS Code extension. The VS Code work already paid for the hard parts —
structured startup output, health endpoint, lifecycle flags, and a proven
subprocess + iframe model — and `dcmview-py` already provides the launch layer
in the language JupyterLab server extensions are written in.

The recommended path is incremental: ship Phase 0 (inline notebook embedding
in `dcmview-py`, days of effort) first, since it requires no extension
machinery and serves the README's stated notebook use case; then build the
Phase 1 server-extension + iframe-widget package (moderate effort, the bulk of
which is JupyterLab plumbing rather than dcmview changes). The only required
change to existing code is converting frontend API paths from root-relative to
relative, which is backward-compatible with every current deployment surface.

## References

- JupyterLab Extension Developer Guide:
  https://jupyterlab.readthedocs.io/en/stable/extension/extension_dev.html
- Jupyter Server Extensions:
  https://jupyter-server.readthedocs.io/en/latest/developers/extensions.html
- jupyter-server-proxy:
  https://jupyter-server-proxy.readthedocs.io/
- JupyterLab extension template (copier):
  https://github.com/jupyterlab/extension-template
- Existing VS Code analysis: `docs/vscode-extension-feasibility.md`
