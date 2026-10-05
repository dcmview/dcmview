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
  reference navigation, a pixel-value readout (stored, Modality, and
  real-world values) for every modality, window/level in real-world units
  with a legend, and the codec coverage in the pixel pipeline table.
- **Semantic context** - SEG (with a drawn overlay), Parametric Map, RT Dose,
  and WSI context; RT Dose and Parametric Map colorwash overlays on the
  images they cover (opacity control and color bar), and per-frame value
  mappings for any modality.
- **Presentation state annotations** - PIXEL-unit graphic and text objects of
  Grayscale and Color Softcopy Presentation States drawn on the images they
  reference, one state at a time (off until chosen), with stepping through
  the state's annotation items (the current one highlighted, the rest
  dimmed). Items match images through their Referenced Image Sequence, else
  the state's Referenced Series Sequence. Standard conventions only: the
  owner ruled institution-specific deviations out of scope on 2026-09-30.

- **Display masking (owner decisions, 2026-10-02)** - `--mask` (and Python
  `view(mask=True)`) starts a session that replaces patient identifiers in
  every response the viewer displays, for screen sharing. It is display only
  and claims no de-identification: files are never modified and nothing is
  persisted. The mode is fixed for the process; there is no runtime toggle.
  Patients get numbered pseudonyms, dates move by one per-patient offset
  within a year, ages over 89 are capped, PN values, private elements and the
  PS3.15 Table E.1-1 basic-profile attributes show `[masked]`, and instance
  UIDs are hashed consistently in the tag tree and every wire field. Kept on
  purpose: Study and Series Description, patient characteristics, and real
  folder and file names in the directory tree (under a "Not masked" note;
  tabs follow the tree). Presentation state text and slide label and
  overview frames are withheld.
- **Redaction boxes** - rectangles drawn with the Redact tool over burned-in
  text, per file, covering every frame unless scoped, in server memory for
  the session and never exported. The server applies them in both frame
  endpoints and the presentation layer, so a redacted region is never sent.
  "Apply to series" copies a file's boxes to the same-sized files of its
  series. Available with or without `--mask`.

**Known gaps (intended work, not settled scope):**

- **VOI LUT Function is not interpreted.** DICOM VOI LUT Function
  (0028,1056), including `LINEAR_EXACT` and `SIGMOID`, is currently ignored.
  Supporting these declared functions is deferred until after v0.3.0. The
  exact window formula used for non-integer samples and real-world units does
  not imply support for this attribute.
- **Presentation states apply annotations only.** DISPLAY-unit objects are
  counted and not drawn, because placing them needs the state's Displayed
  Area and Spatial Transformation, which are not applied. The state's
  Softcopy VOI LUT window is not offered as a preset, and its shutter,
  Modality and Presentation LUTs are ignored. Both are deferred by the owner
  (2026-09-30). MATRIX units, line and fill styles, and text reading
  direction are not interpreted either.
- **Real-world windows need integer samples.** A window in the unit of a LUT
  mapping (or of a mapping behind a Modality LUT) is exact on the raw path
  and, through the display endpoint's `unit` query, in cine, for 8- and
  16-bit (and one-bit) single-sample frames. Frames with other samples show
  their default window for such a window on the server.

- **Redaction boxes are not saved or loaded.** They are lost when the viewer
  exits. A `--redactions <csv>` load and an export, in the EMBED ROI layout,
  are the agreed follow-up (owner, 2026-10-02).
- **Masking does not reach pixels, free text or paths.** Burned-in text needs
  redaction boxes; names and dates inside kept free-text values, and the real
  paths the directory tree shows, are outside the rules.

`docs/planned/` is gitignored and holds local proposals, briefs and review
notes, such as the original compatibility plan.
They are not specs and not current behavior. Do not implement them unless the
owner asks.

`docs/design/` is the confirmed plan for upcoming releases: design, not current
behavior. An implementing change cites the document and section it implements,
and `docs/architecture.md` stays the normative description of what exists.

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
                      -> loader/ spawn_blocking + rayon
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

- The repository is a Cargo workspace. The root package is the `dcmview`
  binary and library; `crates/dcmview-protocol` owns the launch and startup
  contract (`StartupEvent`, `launch_url`, `STARTUP_PROTOCOL`,
  `TOKEN_FRAGMENT_PARAM`, `TOKEN_ENV_VAR`) and depends on `serde` only.
  `src/api/contracts.rs` re-exports those items. Fields of the startup event
  are only added, and member crates carry the viewer's version.
- `src/api/contracts.rs` is the source of truth for the HTTP contract: the
  `endpoints` table (method, path, response media type, response headers,
  success status) that the router and `tests/integration/api_contract.rs`
  both read, header names, and every wire struct. Query parameter names are
  the serde fields of `FrameQuery` and `TagQuery`.
- Wire types derive `ts_rs::TS`. `cargo run --example generate_api_types`
  writes `frontend/src/generated/api-types.ts` (types, endpoint paths, and
  raw-frame header names); `--check` fails on drift. `frontend/src/api.ts`
  owns browser fetches and builds URLs from the generated table.
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

`pixels/syntax.rs` owns the codec table. `codec_for_syntax` picks the decoder
that both frame endpoints dispatch to and that support classification reads;
`Codec::color_samples` states, per codec and photometric interpretation,
whether decoded three-sample frames are RGB or full-resolution YCbCr still to
be converted with the PS3.3 C.7.6.3.1.2 YBR_FULL equations. Add a color layout
there, not in a decoder, so what is advertised is what is converted.

| Class | Transfer syntaxes | Display action |
|---|---|---|
| JPEG Baseline | `1.2.840.10008.1.2.4.50` | Decode the requested frame server-side with `dicom-pixeldata`; PNG encode |
| JPEG Lossless | `1.2.840.10008.1.2.4.57`, `.70` | Decode server-side with `dicom-pixeldata`; convert YBR_FULL components to RGB (the lossless process has no color transform); PNG encode |
| JPEG 2000 Lossless | `1.2.840.10008.1.2.4.90` | Read the frame's encapsulated fragments (`pixels/encapsulated.rs`); decode via `jpeg2k`; PNG encode |
| JPEG-LS Lossless | `1.2.840.10008.1.2.4.80` | Decode 8- or 16-bit grayscale server-side with statically linked CharLS; PNG encode |
| JPEG XL Lossless | `1.2.840.10008.1.2.4.110` | Decode server-side with `dicom-pixeldata`; RGB and YBR_RCT as decoded (the decoder inverts the RCT), YBR_FULL channels converted to RGB; PNG encode |
| RLE Lossless | `1.2.840.10008.1.2.5` | Decode Annex G header/PackBits byte planes server-side; RGB, YBR_FULL, and full-resolution YBR_FULL_422 color; PNG encode |
| Deflated Image Frame Compression | `1.2.840.10008.1.2.8.1` | Inflate the one-bit monochrome frame (binary segmentations); window; PNG encode |
| Native dataset | Implicit LE, Explicit LE, Explicit BE, Deflated Explicit LE | Read the requested frame's native samples (a deflated data set is inflated up to it), rescale/window, PNG encode |
| JPEG Extended, JPEG 2000 lossy, JPEG-LS Near-Lossless, JPEG XL variants | `.51`, `.91`, `.81`, `.111`, `.112` | HTTP 422 unsupported transfer syntax |
| Other | anything else | HTTP 422 unsupported transfer syntax |

Raw-frame endpoints return decoded sample bytes plus metadata headers for
native datasets, JPEG Baseline, grayscale JPEG Lossless, JPEG-LS Lossless,
JPEG XL Lossless, RLE Lossless, and grayscale JPEG 2000 paths. Unsupported
syntaxes and unsupported raw component layouts return 422 or a decode error.

Both display and raw frame endpoints must include `X-Cache: HIT` or
`X-Cache: MISS`.

---

## Key Directories

```text
dcmview/
|-- src/
|   |-- main.rs          Clap shape and process exit
|   |-- application.rs   bridge/local dispatch
|   |-- bridge/          binary-private bridge protocol, registry, client
|   |-- startup/         local assembly and owned discovery lifecycle
|   |-- api/contracts.rs canonical HTTP endpoint and wire contract
|   |-- loader/          cancellable DICOM discovery and FileEntry creation
|   |-- annotations.rs   EMBED-style ROI parsing, validation, memory store
|   |-- masking.rs       --mask display masking rules and the PS3.15 profile list
|   |-- redactions.rs    in-memory redaction boxes and their revisions
|   |-- signals.rs       stop-signal listeners registered before startup output
|   |-- pixels/          service, caches, codecs, rendering, windowing, shutters,
|   |                    overlay colorwash
|   |-- server/          API, catalog, lifecycle, runtime, tags, web assets
|   |-- dicom_values.rs  shared lenient attribute readers
|   |-- object_kind.rs   SOP class to object-kind classification
|   |-- geometry.rs      patient geometry and frame-to-frame transforms
|   |-- series.rs        series/stack catalog ordering
|   |-- references.rs    typed DICOM reference extraction and resolution
|   |-- semantic.rs      SEG, Parametric Map, and RT Dose context
|   |-- plane_stack.rs   dose/PM plane stacks and resampling onto frames
|   |-- presentation_state.rs  softcopy presentation state graphic annotations
|   |-- value_mapping.rs per-frame Modality and real-world value mappings
|   |-- wsi.rs           WSI tile placement and companions
|   `-- types.rs         internal domain and cache-key types
|-- frontend/
|   |-- src/
|   |   |-- App.svelte
|   |   |-- api.ts                    typed fetch boundary
|   |   |-- theme.css                 Bea · dcmview tokens, light and dark
|   |   |-- generated/api-types.ts    generated Rust wire contract
|   |   |-- testing/fixtures.ts       component-test catalog and frame builders
|   |   `-- lib/
|   |       |-- app/                  App-owned controllers: catalog, tabs,
|   |       |                         window settings, sidebar layout
|   |       |-- annotation/tools/     one state machine per pointer tool behind
|   |       |                         the Tool interface (pan, zoom, scroll, W/L,
|   |       |                         rectangle for ROI and Redact)
|   |       |-- viewport/             ImageViewport units: ToolHost (pointer and
|   |       |                         wheel dispatch), frame sources, W/L worker
|   |       |                         client, view state, overlays, ROIs
|   |       |-- ui/                   Bea · dcmview controls: Button, ButtonGroup,
|   |       |                         SegmentedControl, Select, Range, SearchField,
|   |       |                         StatusBadge, Icon and its line icons
|   |       |-- FileNavigator.svelte
|   |       |-- OpenImageTabs.svelte
|   |       |-- ViewerToolbar.svelte
|   |       |-- ImageViewport.svelte
|   |       |-- TagPanel.svelte
|   |       |-- FrameSlider.svelte
|   |       |-- StatusBar.svelte
|   |       |-- SemanticContextPanel.svelte
|   |       |-- WsiTileContext.svelte
|   |       |-- ReferenceNavigator.svelte / ReferenceEdge.svelte
|   |       |-- annotationGeometry.ts
|   |       |-- keyboardShortcuts.ts
|   |       |-- keyedAsyncResource.ts
|   |       |-- viewerTools.ts
|   |       `-- workers/wlRenderer.worker.ts
|   |-- dist/           Build output consumed by rust-embed
|   |-- package.json
|   |-- svelte.config.js
|   `-- vite.config.ts
|-- crates/
|   `-- dcmview-protocol/  workspace member: launch and startup contract
|                          (serde only; no axum, tokio or DICOM crates)
|-- python/dcmview_py/  Python subprocess wrapper and package entrypoint
|-- vscode/             VS Code extension (src/: activation, sessions, custom
|                       editor, bridge server/registry, terminal shims) and
|                       Electron integration tests
|-- tests/
|   |-- integration.rs  Integration test module root
|   |-- integration/    Axum and pixel-path integration tests
|   |-- windowing-cases.json  windowing oracle shared with rawWindowing.test.ts
|   `-- fixtures/       Small generated DICOM fixtures
|-- scripts/check.py    Canonical local and CI check profiles
|-- examples/generate_test_fixtures.rs
|-- examples/generate_api_types.rs
|-- build.rs
|-- Cargo.toml          workspace root and the dcmview package
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

# Ignored prepared-corpus tests against a local prepared corpus (flat `all`
# or per-profile layout); fails if the corpus path is missing
python scripts/check.py corpus --corpus /path/to/prepared-corpus

# Targeted iteration remains valid
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo test --workspace --locked
npm --prefix frontend run generate:types   # after changing src/api/contracts.rs
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

- `loader/` discovery uses `tokio::task::spawn_blocking`; keep rayon work out
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
- The raw cache is the display path's decoded tier for grayscale integer
  frames (`pixels/service.rs` `raw_samples_for_display`); a new display
  decoder's integer layout must be mirrored in `display_integer_layout`.
- Tag trees are cached per file index in a bounded LRU behind private
  `AppState` methods. Tag reads parse only up to pixel data and describe the
  pixel element from its header, seeking past its value (a deflated data set
  is inflated through it into a sink); pixel values are never kept.

**Windowing**

Window resolution order is:

1. `mode=full_dynamic`, which uses current-frame min/max and ignores explicit
   and DICOM window values.
2. Explicit `?wc=&ww=` query parameters.
3. DICOM Window Center/Width from loader metadata.
4. 1st/99th percentile fallback from current-frame samples.

The display pipeline applies the Modality LUT or rescale before windowing for
every grayscale path; 8- and 16-bit frames go through a per-stored-value lookup
table and automatic windows come from a histogram of stored values. The
frontend raw-frame renderer (`rawWindowing.ts`) applies the same pipeline
client-side from the raw headers and the frame's value mapping (stored value
type, Modality LUT, VOI LUT): a per-stored-value table for 1-, 8- and 16-bit
integers, one sample at a time for 32-bit and float samples.

**Encapsulated frame access**

`pixels/encapsulated.rs` locates a frame for every encapsulated codec. With a
valid Extended or Basic Offset Table it seeks to the frame's first item;
without one it steps over item headers and reads only each fragment's end to
find a JPEG end marker (RLE is one fragment per frame). No frame-offset index
is cached between requests.

**Masking and redaction**

- `src/masking.rs` is the only place that decides what a masked session
  shows. A new wire field that carries an instance UID must be named `*_uid`
  or `*_uids` (the response-wide hash keys on the name), and any other new
  field that can carry an identifier must be masked in `Masker` and asserted
  in `tests/integration/display_masking.rs`, which fails when a fixture
  identifier appears in a response.
- Masked values are computed from the process's random keys; never persist
  them or derive them from anything stable across runs.
- Redaction boxes are applied where frames leave the pixel service
  (`load_redacted_frame`, `load_redacted_raw_frame`, the presentation
  layer). A new endpoint that returns source pixels must apply them too.

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
  `frontend/src/generated/api-types.ts` with `npm run generate:types`; never
  hand-edit it.
- Shared root state is owned by `App.svelte`, which instantiates its
  controllers and passes their state down: `lib/app/` `Catalog` (file and
  series catalogs), `TabNavigation` (open tabs, active file/frame/stack
  position), `WindowSettings` (window, its real-world unit, mode, preset,
  manual adjustment), `ValueOverlays` (which RT Dose or Parametric Map
  colorwash is shown, its opacity, and which volumes cover the active tab),
  `GraphicAnnotations` (which presentation state's annotations are shown,
  the annotation item stepped to, and which states annotate the active tab),
  and `SidebarLayout` (navigator and tag panel layout, compact drawers), plus
  `lib/viewport/` `ViewStates` (per-tab zoom, pan, and orientation). Active
  tool, cine settings, and semantic mode are plain `App.svelte` state.
- Global keyboard shortcuts go through the single dispatcher in
  `lib/keyboardShortcuts.ts` and App's `svelte:window` handler; do not add
  per-component window keydown listeners.
- Keyed fetches share and cancel requests through `lib/keyedAsyncResource.ts`
  (`SharedRequestRegistry`, and `KeyedAsyncResource` for per-key status).
- Use `$effect` for genuine reactive synchronization and subscriptions; when a
  parent wants a child to act, call an exported function or a callback prop
  instead of bumping a counter prop for an effect to notice.
- All backend calls go through `frontend/src/api.ts`; do not add raw `fetch`
  calls in components when a typed wrapper belongs there.
- The viewport supports two render paths: display PNG blobs for cine mode and
  raw-frame client-side rendering for interactive diagnostic/window-level work.
  The raw path renders with the file's value mapping (float samples, Modality
  and VOI LUTs) and draws the `presentation-layer` (shutter and overlays) over
  the image, so every grayscale frame the browser can hold is windowed live.
- Window/level interactions should avoid flooding requests: local raw rendering
  draws at most once per animation frame, and a drag over a server-windowed
  frame (over `MAX_RENDER_PIXELS`) sends `preview` requests, one in flight and
  only the newest window queued (`viewport/liveWindowPreview.ts`), which the
  server never caches; the released window is fetched as usual.
- Zoom and pan use canvas/CSS transform state and should not refetch frames.
- Zoom/pan state is per open tab (navigation scope). Moving through a tab's
  frames, including the single-frame files of a stack, preserves the viewport
  transform; opening a different tab starts from a fitted view.
- Orientation state is also per open tab and supports horizontal flip, vertical
  flip, and 90-degree rotation.
- The Redact tool edits redaction boxes with the ROI rectangle editing: the
  viewport's `edited` store is the ROI `AnnotationStore` or a second one bound
  to the redaction endpoints. The server blanks the boxes, so a saved change
  reloads the frames (`reloadFrames`); outlines show only while the tool is
  active. Do not draw a client-side cover instead.
- A masked session's file names come from `Catalog.shownFiles`, whose `path`
  is the synthetic `display_name` unless the directory tree is showing. Pass
  those files (or `catalog.filesById`) to anything that displays a path; only
  `FileNavigator` receives the raw catalog.
- Pointer and wheel handling lives in `viewport/ToolHost.svelte.ts`, which owns
  capture, the shared gestures and the active tool; each tool is a state
  machine in `annotation/tools/`. `ImageViewport.svelte` forwards events to
  the host and keeps rendering. Add a tool there, not in the component. ROI
  editing is `annotation/tools/rectangleTool.ts`; annotation state in
  `viewport/annotationStore.svelte.ts`, hit testing in `viewport/roiEditing.ts`,
  and geometry helpers in `annotationGeometry.ts`. Keep frame-scoping semantics
  consistent with backend validation.
- Map cursor positions to image pixels with `clientToImagePoint` in
  `viewport/viewTransform.ts`; per-frame layers over a source image go through
  `viewport/frameOverlay.ts` (a SEG `FrameOverlay` replaces the displayed
  image, a `ValueOverlay` colorwash is drawn on its own canvas above it).
  Presentation state annotations are vectors, not a raster layer:
  `viewport/GraphicAnnotationOverlay.svelte` draws shapes in image pixel
  coordinates and `GraphicAnnotationLabels.svelte` text and points in
  viewport pixels, with the geometry in `viewport/graphicAnnotations.ts`.
  Pixel values come from `viewport/pixelProbe.svelte.ts` and the per-frame
  `value-mapping` conversions in `viewport/valueMapping.ts`.
- No external CSS frameworks. Use scoped Svelte styles.
- Use the `lib/ui` controls for buttons, segmented choices, selects, ranges, search
  fields and status badges, and `lib/ui/icons.ts` for icons; do not draw
  glyphs with text characters or restyle controls per component.
- Theme tokens are the Bea · dcmview design system's, in `src/theme.css`:
  light by default, dark from `prefers-color-scheme` or `data-theme`. Use the
  role tokens (`paper`, `ink`, `line`, `accent`, `selection-fill`, ...) instead
  of component-local palettes, and never branch a component on the theme. The
  viewport is always a `data-theme="dark"` island on the `viewport` ground.
- Use the shared monospace stack for tag values and the shared UI stack for
  viewer chrome.
- Fonts are bundled: Inter and JetBrains Mono come from the
  `@fontsource-variable/*` packages imported in `main.ts`, and their OFL texts
  ship in `public/assets/licenses/`. Keep the bundled face first in
  `--font-ui` and `--font-mono`; never load fonts from a CDN.

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
  --timeout <u64>           idle seconds after the scan finishes; none if absent
  --no-recursive
  --annotations <csv>
  --filter <FIELD=VALUE>    repeatable metadata filter
  --mask                    mask patient identifiers on screen; fixed for the session
  --unix-socket <path>      listen on a private Unix socket instead of TCP (Unix only)
  --no-token                serve the API without the bearer token (warns)
  --cache-budget <bytes>    total size of the frame caches, e.g. 256MiB
```

Hidden and experimental, for a supervising parent process, and outside the
sign-off rule until that integration ships: `--exit-with-parent` (stop on end
of file on a stdin pipe the parent keeps open) and the
`X-Dcmview-Background: 1` request header (served, but not counted as activity
for `--timeout`).

Every `/api` request needs the session's bearer token unless the process runs
with `--no-token`; `DCMVIEW_TOKEN` fixes the token, and it is never taken from
the command line. New endpoints inherit the check and none is exempt. Load API
resources in the frontend through `send()` only. Keep loopback binding as the
default and prefer SSH forwarding for remote use. If a public bind is added or changed, preserve
the warning path in `server/runtime.rs`.

---

## Important Files

| File | Role |
|---|---|
| `README.md` | Public documentation and PyPI long description |
| `docs/architecture.md` | Normative architecture, lifecycle, contracts, and check profiles |
| `src/main.rs` | CLI shape and process exit |
| `src/application.rs` | Bridge/local dispatch |
| `src/startup/` | Local viewer assembly and discovery ownership |
| `src/api/contracts.rs` | Canonical HTTP endpoint and wire contract |
| `src/server/` | Axum runtime, lifecycle, catalog, API, tags, and web assets |
| `src/loader/` | Cancellable DICOM discovery and metadata extraction |
| `src/pixels/` | Pixel service, codecs, display/raw paths, caches, and windowing |
| `src/annotations.rs` | ROI CSV import/export, validation, in-memory store |
| `src/types.rs` | Internal domain, transfer-syntax, and cache-key types |
| `build.rs` | Frontend build integration and Cargo fingerprints |
| `scripts/check.py` | Canonical check profiles used locally and in CI |
| `frontend/src/api.ts` | Typed frontend fetch wrappers |
| `frontend/src/generated/api-types.ts` | Generated TypeScript HTTP contract |
| `frontend/src/App.svelte` | Root frontend state and layout |
| `frontend/src/lib/ImageViewport.svelte` | Viewport composition and render pipelines |
| `frontend/src/lib/viewport/ToolHost.svelte.ts` | Pointer and wheel dispatch to the tools in `lib/annotation/tools/` |
| `python/dcmview_py/wrapper.py` | Python subprocess wrapper |
| `examples/generate_test_fixtures.rs` | Synthetic fixture generator |
| `examples/generate_api_types.rs` | TypeScript contract generator and drift check |

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
uses Vitest (Node module tests, plus happy-dom component tests that opt in with
a `@vitest-environment happy-dom` docblock), Python separates mock/unit
coverage from real-binary integration, and VS Code separates compilation from
Electron-hosted integration.

Committed synthetic fixtures cover native, JPEG Baseline, JPEG Lossless, JPEG
2000, color, display shutter, multiframe, no-pixel, RT Dose and
Parametric Map overlay objects with their source images, and Grayscale
Softcopy Presentation States with graphic annotations over their target
images, and two patients' identifier-laden images with a burned-in banner
(plus a slide label image) for display masking and redaction. They are generated
by:

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
- With a token set, every declared endpoint answers `401` without it and its
  declared status with it.
- A Unix socket is `0600` in a directory only the launching user can write,
  judged at its resolved location, and a live socket is never replaced.
- Cache misses when window parameters or window mode change.
- Display frames for supported image syntaxes return `Content-Type: image/png`.
- JPEG 2000 display paths decode server-side rather than returning raw
  compressed fragments.
- Raw-frame endpoints return decoded samples with metadata headers.
- Uncompressed pixel values match fixture expectations after windowing.
- Files without pixel data appear with `has_pixels: false`; frame requests for
  them return 404.
- Port `0` auto-assign reports the actual listener port.
- `--timeout` exits after the configured idle period, counted from scan
  completion.
- Mixed DICOM/non-DICOM discovery reports valid files and skip counts.
- Annotation load, edit, validation, and CSV export preserve the EMBED-style
  contract.
- The EMBED-style CSV export of the real binary equals the committed goldens in
  `tests/fixtures/embed-goldens/` byte for byte, rows in any order.
- A masked session shows no fixture identifier in the catalog, series catalog,
  tag tree or selected elements, and its hashed UIDs agree across endpoints.
- A redaction box blanks the display and raw frame, is a cache `MISS` after a
  change, copies to the same-sized files of the series, and stays out of the
  ROI export.

**Test policy:**

- A test earns its place by catching a regression that a user or a downstream
  consumer would notice: wire contracts, golden bytes, invariants, security
  boundaries, cross-component behavior.
- Do not test private helpers, exact log or error strings, or internal struct
  layout, and do not snapshot whole responses unless a contract requires the
  bytes.
- Prefer one table-driven test over many near-identical ones.
- When working from a brief, write only the tests it names. Suggest any others
  in the report instead of adding them.

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
