# Repository Guidelines

## Project Overview

`dcmview` is a fast, ephemeral DICOM inspection tool for developers, data
scientists, and medical imaging researchers. It starts a temporary local web
server with an embedded browser viewer, exposes image frames, tags, and ROI
annotations through a small HTTP API, and exits cleanly when stopped.

The core workflow is quick inspection of DICOM data where the files already
live, especially on remote servers. It is meant to avoid slow notebook rendering
for multi-frame studies and avoid the setup, firewall, transfer, and annotation
round-trip costs of external viewers when the user only needs research review.

`dcmview` is intended for developer and research inspection, not clinical
diagnosis.

**Status:** Core implementation complete.

**Design axioms**:

- **Ephemeral** - no persistent state, no config files, no database.
- **Fast** - startup, first-frame render, and multi-frame navigation are primary
  performance targets.
- **Self-contained binary** - release builds embed the Svelte frontend with
  `rust-embed`; the Python package is a wrapper/bundling path for the same
  binary on supported platforms.
- **Remote-friendly** - bind to loopback by default and use SSH forwarding for
  remote-server workflows.

---

## Intended Features and Known Gaps

Every feature listed here exists on purpose. Reviews and cleanups judge *how* a
feature is implemented (duplication, needless layers, test-only seams, brittle
design), never *whether* it should exist. Fix or simplify a poor
implementation; do not delete it.

Removing or narrowing user-facing behavior needs the owner's explicit sign-off
before the change, noted in the commit body. That covers features, CLI flags,
Python keywords, wire fields, supported pixel layouts, and the rules that
decide when a launch routes into VS Code. "Duplicated", "unused by
production", or "out of scope for a quick viewer" is not sign-off. If a finding
would remove behavior, raise it as a question instead of acting.

**Intended features:**

- **Local viewer** - CLI over files and directories, recursive or top-level
  scan, `--filter`, `--timeout`, and EMBED-style ROI load, edit, and export.
- **Remote use** - loopback bind plus the printed `ssh -L` hint. (`--tunnel`
  was removed with the owner's agreement on 2026-09-25.)
- **Python package** - `view()` with blocking and non-blocking handles,
  `python -m dcmview_py`, and the `dcmview`/`dcmview-py` console scripts, with
  bundled, `DCMVIEW_BINARY`, or `PATH` binary resolution.
- **VS Code integration** - extension viewer, readonly DICOM custom editor,
  terminal interception through PATH shims, and routing into the VS Code viewer
  from the binary *and* from Python, including notebook kernels that only see
  the bridge registry. The Rust, Python, and TypeScript bridge code may be
  consolidated, but no entry point may lose its routing.
- **VS Code routing rule (owner decision, 2026-09-25)** - every entry point
  routes into VS Code when the process has the bridge environment (a VS Code
  terminal) or its working directory is inside a registered workspace folder.
  Otherwise it runs the local viewer. Python's `view(vscode_bridge=False)` and
  `DCMVIEW_VSCODE_BYPASS=1` opt out. The rule lives only in
  `src/bridge/registry.rs`; the Python wrapper and terminal shims go through the
  binary's hidden `--vscode-bridge-client` form.
- **Viewer** - cine playback, server- and client-side window/level, presets,
  per-tab zoom/pan/orientation, series and stack navigation, tag panel,
  reference navigation, and the codec coverage in the pixel pipeline table.
- **Semantic context** - SEG (with a drawn overlay), Parametric Map, RT Dose,
  and WSI context.

**Known gaps (intended work, not settled scope):**

- **RT Dose overlay is not drawn.** `rt_dose_overlay` computes eligibility and
  a source file, but nothing consumes it. Drawing it needs:
  - dose-grid resampling onto each source frame (Grid Frame Offset Vector and
    patient coordinates);
  - Dose Grid Scaling;
  - a color map or isodose bands with a legend;
  - a check that the grid covers the displayed slice.
- **Parametric Map values are not applied.** Real World Value Mappings are
  summarized in the panel only. The display windows stored values even though
  `displayed_value_kind` reports `mapped`. Intended, in order:
  1. a pixel readout of stored and mapped values under the cursor, for every
     modality;
  2. window/level in mapped units, with a color bar or legend;
  3. a Parametric Map overlay on its source image, like the SEG overlay. This
     was part of the original feature and was never finished.
- **Color layouts reported unsupported because their decoder lacks a color
  transform:**
  - RLE YBR_FULL_422;
  - JPEG Lossless YBR_FULL;
  - JPEG XL YBR_FULL.
- **No pixel-value readout.** Neither stored nor mapped values (Parametric
  Map, RT Dose) can be read per pixel in the viewport; see Parametric Map above.
- **API data the UI never renders:**
  - RT Dose grid geometry and references;
  - WSI companions and relationships;
  - the SEG recommended color, which is used for the overlay but not shown in
    the panel.
- **Display shutters are rectangular only.** Circular, polygonal, and bitmap
  shutters are not applied.

`docs/planned/` temporarily holds uncommitted proposals, such as the JupyterLab
integration and the original compatibility plan. They are not specs and not
current behavior. Do not implement them unless the owner asks.

---

## Git Commit Policy

Every completed task **MUST** be tracked in a descriptive, granular git commit.
This requirement is **absolutely critical** and must be followed under all
circumstances - no exceptions.

**Rules:**

- Commit after every distinct logical unit of work, not at the end of a session.
- Each commit covers exactly one coherent change (one module, one component, one
  test suite, one docs section). Do not batch unrelated changes into a single
  commit.
- Commit messages must be informative: use `type(scope): subject` format,
  include a blank line, then a body describing *what* changed and *why*.
  - Types: `feat`, `fix`, `test`, `docs`, `refactor`, `chore`
  - Scope: the module, file, or subsystem affected, such as `backend`,
    `frontend`, `pixels`, `server`, `types`, or `tests`
  - Subject: imperative mood, 72 characters or fewer
  - Body: explain the design decision, the invariant being established, or the
    behavior being changed, not a restatement of the diff
- Stage files selectively (`git add <file>`) rather than `git add -A`. Only
  commit files that belong to the current logical unit.
- Never amend or force-push commits that have been logged here.

**Verification:** After each task, run `git log --oneline -3` to confirm the
commit was recorded before moving to the next task.

## Architecture & Data Flow

[`docs/architecture.md`](docs/architecture.md) is the normative current-state
module, contract, lifecycle, and test-profile model. Update it with structural
changes.

```text
CLI / Python wrapper / VS Code
  -> src/main.rs
       -> application.rs   hidden bridge -> workspace bridge -> local viewer
            -> bridge/     protocol, registry discovery, HTTP/process client
            -> startup/
                 -> BoundServer::bind before discovery
                 -> discovery coordinator
                      -> loader.rs spawn_blocking + rayon
                      -> server/catalog.rs FileRegistry
                      -> annotations.rs AnnotationStore
                 -> BoundServer::serve
                      -> server/api/ routes, handlers, state, errors
                      -> pixels/ display/raw service, codecs, caches
                      -> server/tags.rs and server/web.rs

Frontend (Svelte 5, compiled into the binary via rust-embed):
  App.svelte
    -> FileNavigator | OpenImageTabs | ViewerToolbar | ImageViewport
    -> FrameSlider | TagPanel | StatusBar
    -> api.ts -> generated/api-types.ts
```

### Contract and state ownership

- `src/api/contracts.rs` is the source of truth for HTTP operations, wire
  structs, query names, response header names, media types, statuses, and the
  JSON error envelope.
- `frontend/src/generated/api-types.ts` is generated from the Rust contract;
  `frontend/src/api.ts` owns browser fetches.
- `src/types.rs` owns internal DICOM, cache-key, transfer-syntax, and windowing
  types. It re-exports selected wire types for compatibility but does not own
  them.
- `server/api/state.rs` owns private `AppState` resources: `FileRegistry`,
  display/raw/tag caches, `AnnotationStore`, server start
  time, and `RequestActivity`. Construct it through `AppState::new`.
- `server/catalog.rs` owns progressive registry contents and scan counters.
- `startup/discovery.rs` owns discovery cancellation, task handles, typed
  outcomes, registry completion, and failure notification.

### Pixel pipeline

Display-frame endpoints return PNG for every supported image transfer syntax.
Do not rely on browser-native DICOM fragment decoding for viewer correctness.

| Class | Transfer syntaxes | Display action |
|---|---|---|
| JPEG Baseline | `1.2.840.10008.1.2.4.50` | Decode the requested frame server-side with `dicom-pixeldata`; PNG encode |
| JPEG Lossless | `1.2.840.10008.1.2.4.57`, `.70` | Decode server-side with `dicom-pixeldata`; PNG encode |
| JPEG 2000 Lossless | `1.2.840.10008.1.2.4.90` | Read encapsulated fragment with `DicomCollector`; decode via `jpeg2k`; PNG encode |
| JPEG-LS Lossless | `1.2.840.10008.1.2.4.80` | Decode grayscale server-side with statically linked CharLS; PNG encode |
| JPEG XL Lossless | `1.2.840.10008.1.2.4.110` | Decode RGB server-side with `dicom-pixeldata`; PNG encode |
| RLE Lossless | `1.2.840.10008.1.2.5` | Decode Annex G header/PackBits byte planes server-side; PNG encode |
| Native dataset | Implicit LE, Explicit LE, Explicit BE, Deflated Explicit LE | Read decoded/native samples, rescale/window, PNG encode |
| JPEG Extended, JPEG 2000 lossy, JPEG-LS Near-Lossless, JPEG XL variants | `.51`, `.91`, `.81`, `.111`, `.112` | HTTP 422 unsupported transfer syntax |
| Other | anything else | HTTP 422 unsupported transfer syntax |

Raw-frame endpoints return decoded sample bytes plus metadata headers for
native datasets, JPEG Baseline, JPEG Lossless, JPEG-LS Lossless,
JPEG XL Lossless RGB, RLE Lossless, and grayscale JPEG 2000 paths. Unsupported
syntaxes and unsupported raw component layouts return 422 or a decode error.

Both display and raw frame endpoints must include `X-Cache: HIT` or
`X-Cache: MISS`.

---

## Key Directories

```text
dcmview/
|-- src/
|   |-- main.rs          Clap shape and process exit
|   |-- application.rs   bridge/local dispatch and tracing seam
|   |-- bridge/          binary-private bridge protocol, registry, client
|   |-- startup/         local assembly and owned discovery lifecycle
|   |-- api/contracts.rs canonical HTTP endpoint and wire contract
|   |-- loader.rs        cancellable DICOM discovery and FileEntry creation
|   |-- annotations.rs   EMBED-style ROI parsing, validation, memory store
|   |-- pixels/          service, caches, codecs, rendering, windowing
|   |-- server/          API, catalog, lifecycle, runtime, tags, web assets
|   `-- types.rs         internal domain and cache-key types
|-- frontend/
|   |-- src/
|   |   |-- App.svelte
|   |   |-- api.ts                    typed fetch boundary
|   |   |-- generated/api-types.ts    generated Rust wire contract
|   |   `-- lib/
|   |       |-- FileNavigator.svelte
|   |       |-- OpenImageTabs.svelte
|   |       |-- ViewerToolbar.svelte
|   |       |-- ImageViewport.svelte
|   |       |-- TagPanel.svelte
|   |       |-- FrameSlider.svelte
|   |       |-- StatusBar.svelte
|   |       |-- annotationGeometry.ts
|   |       |-- viewerTools.ts
|   |       `-- workers/wlRenderer.worker.ts
|   |-- dist/           Build output consumed by rust-embed
|   |-- package.json
|   |-- svelte.config.js
|   `-- vite.config.ts
|-- python/dcmview_py/  Python subprocess wrapper and package entrypoint
|-- vscode/             VS Code extension and Electron integration tests
|-- tests/
|   |-- integration.rs  Integration test module root
|   |-- integration/    Axum and pixel-path integration tests
|   `-- fixtures/       Small generated DICOM fixtures
|-- scripts/check.py    Canonical local and CI check profiles
|-- examples/generate_test_fixtures.rs
|-- build.rs
|-- Cargo.toml
`-- pyproject.toml
```

---

## Development Commands

```bash
# Fast feedback: contracts, frontend, Rust lint, Python unit
python scripts/check.py quick --install

# Deterministic core: quick layers + fixtures/default-feature Rust + VS Code
python scripts/check.py core --install

# Real debug binary, wrapper/smoke, and VS Code Electron integration
python scripts/check.py e2e --install

# Independent upstream remote fixtures; may download/cache data
python scripts/check.py external

# Targeted iteration remains valid
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo test --locked
npm --prefix frontend run test
npm --prefix frontend run typecheck
```

**Prerequisites:**

- Rust 1.88+
- Node.js 20.19+ and npm at build time
- CMake and a C++ toolchain at Rust build time for statically linked CharLS
- Python 3.9+ for wrappers and check profiles

`quick` does not run Rust tests or VS Code tests. `core` adds fixture
regeneration that must leave the current fixture tree unchanged, the
default-feature, non-ignored locked Rust suite, and VS Code compilation. `e2e`
adds real-process coverage. `external` is separate and runs exactly the
feature-gated ignored remote-fixture tests. See `docs/architecture.md` for the
complete profile model.

`build.rs` runs `npm ci` only when `frontend/package-lock.json` changes since
the last successful install stamp, then runs `npm run build`. `DCMVIEW_NODE_PATH`
and `DCMVIEW_NPM_PATH` may point to absolute tool paths. `DCMVIEW_SKIP_FRONTEND_BUILD=1`
requires an existing `frontend/dist/index.html`.

---

## Code Conventions & Common Patterns

### Rust

**Async / blocking boundary**

- `loader.rs` discovery uses `tokio::task::spawn_blocking`; keep rayon work out
  of the async executor.
- Pixel decode/encode and tag tree construction use `spawn_blocking` where they
  can do filesystem, codec, or CPU-heavy work.
- Display and raw LRU cache locks are held only for lookup/insert. Never hold a
  cache lock while decoding, encoding, reading DICOM files, or serializing tags.

**Error handling**

- Use `anyhow` for fallible non-API internals.
- Convert `PixelError` through `server/api/error.rs`; all API errors must use
  the shared JSON `ErrorResponse` envelope.
- Path, query, and JSON extractor rejections must also use the JSON envelope.
- Frame decode errors return HTTP 500 JSON and the server continues.
- Unsupported transfer syntax returns HTTP 422 JSON and must never panic.
- Missing pixel data returns 404 for frame endpoints.
- Tag serialization errors for individual values should emit `TagValue::Error`
  and continue serializing the response where possible.
- Zero valid files after scan is a non-zero CLI error.

**Caches**

- `FrameCacheKey` uses `f64::to_bits()` for window center/width because those
  values come directly from UI/query/DICOM inputs.
- Display cache entries are budgeted by `FRAME_CACHE_MAX_BYTES`; raw cache
  entries are budgeted by `RAW_CACHE_MAX_BYTES`.
- Tag trees are cached per file index behind private `AppState` methods.

**Windowing**

Window resolution order is:

1. `mode=full_dynamic`, which uses current-frame min/max and ignores explicit
   and DICOM window values.
2. Explicit `?wc=&ww=` query parameters.
3. DICOM Window Center/Width from loader metadata.
4. 1st/99th percentile fallback from current-frame samples.

The display pipeline applies rescale slope/intercept before windowing for
uncompressed paths. The frontend raw-frame renderer receives rescale metadata in
headers and applies the same convention client-side.

**DICOM collector use**

JPEG 2000 display decoding reads encapsulated fragments through
`DicomCollector`. The current implementation reads fragments sequentially up to
the requested frame. Do not document or rely on a cached BOT/frame-offset index
unless one is actually implemented.

**Annotations**

- `--annotations` loads EMBED-style CSV rows into memory only.
- The input CSV and DICOM files must not be modified.
- API edits replace the in-memory annotations for one file and are validated
  against image bounds and frame count.
- Export writes a fresh EMBED-style CSV from the current in-memory store.

### Svelte 5 / TypeScript frontend

- Use Svelte 5 runes (`$state`, `$derived`, `$effect`); avoid legacy `$:`
  reactive declarations.
- `src/api/contracts.rs` is the HTTP source of truth. Regenerate
  `frontend/src/generated/api-types.ts`; never hand-edit it.
- Shared root state lives in `App.svelte`: active file/frame, window settings,
  open tabs, active tool, selected preset, orientation, reset count, navigator,
  and tag panel layout.
- All backend calls go through `frontend/src/api.ts`; do not add raw `fetch`
  calls in components when a typed wrapper belongs there.
- The viewport supports two render paths: display PNG blobs for cine mode and
  raw-frame client-side rendering for interactive diagnostic/window-level work.
- Window/level interactions should avoid flooding requests; prefer local raw
  rendering or debounced/networked updates depending on the mode being changed.
- Zoom and pan use canvas/CSS transform state and should not refetch frames.
- Zoom/pan state is per open tab (navigation scope). Moving through a tab's
  frames, including the single-frame files of a stack, preserves the viewport
  transform; opening a different tab starts from a fitted view.
- Orientation state is also per open tab and supports horizontal flip, vertical
  flip, and 90-degree rotation.
- ROI editing lives in `ImageViewport.svelte` with geometry helpers in
  `annotationGeometry.ts`; keep frame-scoping semantics consistent with backend
  validation.
- No external CSS frameworks. Use scoped Svelte styles.
- Theme tokens live as CSS variables in `App.svelte`; reuse them instead of
  introducing component-local chrome palettes.
- Use the shared monospace stack for tag values and the shared UI stack for
  viewer chrome.

**Frontend design iteration**

- Prefer the real Svelte app against `tests/fixtures` over standalone HTML/CSS
  mockups, Storybook, or a duplicate mocked API. This preserves actual canvas,
  tag, annotation, layout, and interaction behavior while keeping startup fast.
- Run one shared fixture backend on port 8888 with
  `dcmview --no-browser --host 127.0.0.1 --port 8888 tests/fixtures`, then run
  each frontend variant in its own Git worktree and on a unique Vite port with
  `npm --prefix frontend run dev -- --host 127.0.0.1 --port <port>`. All variants
  may use the existing Vite proxy to the shared backend.
- Give parallel design agents narrow visual directions and keep each variant on
  its own branch. Review screenshots before translating the chosen direction
  into the main implementation; do not mix unrelated alternatives in one
  worktree or commit.
- For representative visual review, explicitly open
  `golden-jpeg-baseline-large-single-frame.dcm`; most other committed image
  fixtures are intentionally codec-test-sized. Also check a multiframe fixture
  and a no-pixel fixture when the affected UI includes playback or metadata-only
  states.
- Capture consistent desktop, compact, and narrow viewport states when comparing
  variants. Add screenshot automation only after repeated manual capture becomes
  a bottleneck; add a frontend-only mock mode only if running the real backend is
  demonstrably impractical.

### CLI

```text
dcmview [OPTIONS] <PATH> [PATH ...]
  -p, --port <u16>          default: 0 (auto-assign)
  --host <str>              default: 127.0.0.1
  --no-browser
  --timeout <u64>           seconds; no timeout if absent
  --no-recursive
  --annotations <csv>
  --filter <FIELD=VALUE>    repeatable metadata filter
```

The server is unauthenticated. Keep loopback binding as the default and prefer
SSH forwarding for remote use. If a public bind is added or changed, preserve
the warning path in `server/runtime.rs`.

---

## Important Files

| File | Role |
|---|---|
| `README.md` | Public documentation and PyPI long description |
| `docs/architecture.md` | Normative architecture, lifecycle, contracts, and check profiles |
| `src/main.rs` | CLI shape and process exit |
| `src/application.rs` | Bridge/local dispatch and injectable test seam |
| `src/startup/` | Local viewer assembly and discovery ownership |
| `src/api/contracts.rs` | Canonical HTTP endpoint and wire contract |
| `src/server/` | Axum runtime, lifecycle, catalog, API, tags, and web assets |
| `src/loader.rs` | Cancellable DICOM discovery and metadata extraction |
| `src/pixels/` | Pixel service, codecs, display/raw paths, caches, and windowing |
| `src/annotations.rs` | ROI CSV import/export, validation, in-memory store |
| `src/types.rs` | Internal domain, transfer-syntax, and cache-key types |
| `build.rs` | Frontend build integration and Cargo fingerprints |
| `scripts/check.py` | Canonical check profiles used locally and in CI |
| `frontend/src/api.ts` | Typed frontend fetch wrappers |
| `frontend/src/generated/api-types.ts` | Generated TypeScript HTTP contract |
| `frontend/src/App.svelte` | Root frontend state and layout |
| `frontend/src/lib/ImageViewport.svelte` | Viewer rendering, tools, ROI editing |
| `python/dcmview_py/wrapper.py` | Python subprocess wrapper |
| `examples/generate_test_fixtures.rs` | Synthetic fixture generator |

---

## Runtime / Tooling

- **Runtime:** Rust 1.88+, Tokio async runtime (`features = ["full"]`).
- **Frontend toolchain:** Vite + Svelte 5 + TypeScript; Node 20.19+, npm.
- **Wrapper/check runner:** Python 3.9+.
- **Rust package manager:** Cargo.
- **Frontend package manager:** npm. Do not switch to bun or pnpm because
  `build.rs` calls npm.
- **Build integration:** `build.rs` builds `frontend/dist/`; release binaries
  embed those assets through `rust-embed`.
- **Python package:** `dcmview-py` exposes `dcmview` and `dcmview-py` console
  scripts and resolves a bundled binary, `DCMVIEW_BINARY`, or `PATH`.

### Cargo feature flags

- `debug-api`: enables permissive CORS for separate-origin API debugging only.
- `debug-embed`: enables `rust-embed/debug-embed` so development builds can
  serve `frontend/dist/` from disk.
- `remote-fixtures`: enables tests that use the `dicom-test-files` crate.

---

## Testing & QA

Use the `scripts/check.py` profiles above; their exact composition and test
seams are normative in `docs/architecture.md`.

Rust uses unit tests plus `axum-test` HTTP integration tests. Frontend behavior
uses Vitest, Python separates mock/unit coverage from real-binary integration,
and VS Code separates compilation from Electron-hosted integration.

Committed synthetic fixtures cover native, JPEG Baseline, JPEG Lossless, JPEG
2000, multiframe, and no-pixel objects. They are generated by:

```bash
cargo run --example generate_test_fixtures
```

Two upstream loader/API and JPEG 2000 display/cache cases are behind the
`remote-fixtures` feature and ignored by default because they may
download/cache files through `dicom-test-files`. Run them only through
`python scripts/check.py external`; committed JPEG 2000 coverage remains in the
default suite.

**Key integration test assertions:**

- `X-Cache: MISS` on first frame request; `X-Cache: HIT` on identical repeat.
- Every declared HTTP endpoint matches its status, media type, response header,
  and shared JSON error contract at runtime.
- Cache misses when window parameters or window mode change.
- Display frames for supported image syntaxes return `Content-Type: image/png`.
- JPEG 2000 display paths decode server-side rather than returning raw
  compressed fragments.
- Raw-frame endpoints return decoded samples with metadata headers.
- Uncompressed pixel values match fixture expectations after windowing.
- Files without pixel data appear with `has_pixels: false`; frame requests for
  them return 404.
- Port `0` auto-assign reports the actual listener port.
- `--timeout` exits after the configured idle period.
- Mixed DICOM/non-DICOM discovery reports valid files and skip counts.
- Annotation load, edit, validation, and CSV export preserve the EMBED-style
  contract.

Do not mock the DICOM layer for integration coverage. Use generated fixtures or
feature-gated remote fixtures so codec and metadata behavior stay exercised.

**Performance targets** should be verified with timing instrumentation, not
mocks:

- Startup for a small file set should stay well under interactive latency
  thresholds.
- First decoded frame should be fast enough for iterative inspection when codec
  cost permits.
- Memory usage should remain bounded across sequential multi-frame requests by
  cache budgets and one-frame decode behavior.
