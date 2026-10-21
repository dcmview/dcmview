# Output adapters and templating

This is a scoping analysis, not a spec, and nothing was changed in any
repository. Read against `dcmview/dcmview` at `b78536f` (0.3.1 dev),
`dcmview-docs`, `dcmview-test-corpus`, `annotation-model.md` (section 12
confirmed by the owner on 2026-09-29), the integration seams (`seams.md`) and
`image-formats.md`. Date: 2026-09-29.

The following are already decided and not re-argued here. dcmview stays
ephemeral and the hub owns durable state. There is one neutral annotation
model with EMBED as an adapter at parity. Standalone dcmview gets a URL token
and an optional Unix socket. From the annotation model, as the owner
confirmed:

- EMBED boxes are `[ymin, xmin, ymax, xmax]`, which is `{y0, x0, y1, x1}`.
- No EMBED ROI extension draft exists yet, so EMBED-style export handles
  `rect` annotations only. Other shapes are skipped with a report by default,
  with `bbox` as an option. This area gets revisited once an EMBED convention
  for non-box shapes exists.
- A per-ROI `[]` means all frames, and export writes `"[]"` only when every
  ROI on the file is all-frames.
- Byte-identical duplicates share one key, and EMBED keeps exporting one row
  per path.

Decisions that need the owner are collected in section 12. What later areas can
rely on is in section 13.

Amended on 2026-09-30 after two external reviews (the resulting decisions
confirmed by the owner, plus "no decision needed" fixes); see the list below.

---

## Review amendments (2026-09-30)

- **1, 6.2, 10 step 1, 12 item 4**: today's EMBED row order is rayon completion order, not walk order; the EMBED adapter sorts rows by path, and goldens compare rows order-insensitively until the sort lands.
- **1, 8, 9.1, 9.3, 12 item 8**: EMBED CSV import stays lenient: out-of-range and zero-area rows load as in 0.3 and are listed in the import report; native formats validate strictly.
- **3.2, 11**: minijinja fuel is an instruction budget, not a resource sandbox; templates also get source-size, output-size and job-concurrency caps.
- **4.1, 4.2, 13**: `meta.exported_at` in hub exports is a state time the hub supplies, so output bytes do not depend on the wall clock.
- **4.1, 7.4, 7.5, 13**: `pixel_spacing` is per frame, from `dcmview inventory` in the hub; SEG and NIfTI exports that need DICOM headers run in a supervised `dcmview export` subprocess, so the hub links no DICOM reader.
- **5.1, 5.2, 9.4**: generated output names are untrusted: absolute, parent-traversing and link paths are rejected, and collisions and duplicate normalized names are errors, not overwrites.
- **5.4, 8, 9.2, 13**: export and import permissions are enforced by the spoke (and again by the hub) through `exports.allowed` and `imports` in the annotation config; with `blind_metadata` on, identifier-bearing exports (and all imports) are denied to annotators.
- **7.2**: COCO results import needs the originating manifest (or dcmview's own COCO dataset export) or an explicit image, category and frame mapping; unknown ids are rejected.
- **7.3**: class-map PNG export is lossy for instances; overlap precedence is an explicit, stable overlap-priority list in the export options and report, since layer order is never persisted.
- **9.2, 9.3**: `/export/check` returns a snapshot token that `/export` must present or be rejected as stale; the final report comes from the actual render.
- **10, 11, 12, 13**: suggested order, risks, decisions 3, 4, 8, 10 and 11 amended; downstream interfaces updated.

---

## 0. Summary

| # | Decision | Recommendation (all confirmed 2026-09-30) | Needs the owner |
|---|---|---|---|
| A1 | Kinds of adapter | Three kinds: **code adapters** in Rust, **table specs** (declarative JSON, which is what the visual editor edits), and **free-form templates** (minijinja). No plugins or external commands in v1. | Yes |
| A2 | Template language | **minijinja** (Jinja2 syntax, pure Rust, sandboxed, fuel-limited) | Yes |
| A3 | What templates see | A versioned, precomputed **export view** built by Rust code, not the raw model. Loss policy is applied before the template runs. | Yes |
| A4 | EMBED | The code adapter stays the parity reference for import, export and `Export ROIs`. A bundled **EMBED table spec** is the shipped example, and a golden test keeps it byte-identical to the code adapter. | Yes |
| A5 | EMBED in the hub | One CSV per annotator. The path mode (absolute or campaign-relative) is chosen at export. | Yes |
| A6 | Which formats are code, and which are templates | Code: native JSON/JSONL, EMBED, COCO, PNG masks, DICOM SEG, NIfTI and SR. Table specs and templates: any CSV/TSV/JSONL, YOLO txt, Pascal VOC XML, lab-specific sheets. | Yes |
| A7 | Import | Code only: native, EMBED, COCO (including detector results with scores, loaded into a model layer), then SEG and PNG masks. Templates are export-only. A CSV column-mapping importer comes later. | Yes |
| A8 | Visual editor | A form over the table spec with a live preview, built after the spec itself. It doesn't edit free-form templates. | Yes |
| A9 | Export UX and API | Keep one-click **Export ROIs**, which produces today's EMBED CSV byte for byte (rows now sorted by path), plus an adapter menu, a pre-export capability check and a post-export report. `GET /api/annotations/export.csv` stays. | Yes |
| A10 | Multi-file outputs | Zip (new `zip` dependency), streamed | Small |

---

## 1. What exists today (checked)

- **Export** is `AnnotationStore::export_embed_csv`
  (`src/annotations.rs:179`). It uses the `csv` crate with default settings, a
  fixed header `anon_dicom_path,num_ROI,ROI_coords,ROI_frames`, and one row
  per file with `num_roi > 0`, in `files_snapshot()` order. That is registry
  insertion order, which is rayon completion order from the parallel discovery
  (`src/loader/discovery.rs:276`, `src/server/catalog.rs:82`), so today's row
  order can differ between runs. Path is `file.path`. `ROI_coords` and
  `ROI_frames` are `serde_json::to_string` (compact, no spaces). The handler
  (`handlers.rs:268`) waits for the CSV import to finish, then answers
  `text/csv` with `Content-Disposition: attachment;
  filename="dcmview-annotations.csv"` (`contracts.rs:26`).
- **Exact bytes (verified with csv 1.4.0, the locked version).** Records end
  in `\n`, not `\r\n`. Fields are quoted only when needed: `ROI_coords` with
  two or more numbers always contains a comma and is quoted, but `ROI_frames`
  values like `[]` or `[[0]]` are written **unquoted**. `docs/annotations.md`
  shows `"[]"` quoted in its example, so the docs and the real output already
  differ cosmetically. Paths with commas get quoted. Any reimplementation,
  template included, has to reproduce this quoting rule, not "quote JSON
  columns".
- **`file.path` is the absolute path.** Discovery stores
  `normalize_input_path`, which is canonicalized when possible and otherwise
  lexically normalized (`src/loader/discovery.rs:399`). So exported
  `anon_dicom_path` is absolute, with symlinks resolved.
- **The frontend** triggers export with an `<a href=api/annotations/export.csv
  download>` (`App.svelte:147`). The token (`seams.md` 2) turns that into
  fetch-then-blob.
- **Import** is the EMBED CSV only (`--annotations`, Python `annotations=`).
  It is strict about the header at startup, streams in the background, and is
  all-or-nothing on invalid matching rows (section 9.1 of the model doc).
  "Invalid" today means JSON shape, `num_ROI`, `ROI_frames` length and frame
  ranges only: coordinate bounds and zero area are **not** checked on import
  (`canonicalize_annotations` runs only on PUT, `src/annotations.rs:165`), so
  out-of-range boxes load. `docs/annotations.md` (lines 58 and 100)
  overstates this.
- **DICOM writing** already exists in tests only (`tags.rs:607`,
  `pixels/jpeg.rs`, both using `dicom_object` with `FileMetaTableBuilder`), so
  writing DICOM objects with dicom-rs is proven in this codebase. There is no
  SEG or SR writer.
- **SEG reading** exists for display (`pixels/segmentation.rs`, binary,
  labelmap and fractional).
- **The test corpus** has SEG recipes (binary, labelmap, fractional, WSI tile
  reference) and SR TID 1500, plus a conformance pipeline (dciodvfy and
  dicom-validator backends, reviewed dispositions in
  `conformance/accepted-findings.json`). That is a ready way to validate a
  future SEG or SR writer.
- **Dependencies**: `image` has `png` and `jpeg` features (enough for PNG mask
  output), plus `flate2`, `serde_json` and `csv`. There is no template engine,
  zip crate or NIfTI crate.

---

## 2. Kinds of adapter (A1)

Output formats fall into three groups with different needs:

1. **Row-shaped text**: EMBED-like CSVs, lab spreadsheets, JSONL, "one line
   per box". Users want to change these often: column names, order, which
   attributes, and coordinate order.
2. **Text with nesting or per-file structure**: YOLO (one `.txt` per image),
   Pascal VOC (one XML per image), small custom JSON.
3. **Structured or binary formats with rules**: COCO (global id tables,
   RLE), DICOM SEG and SR (IODs, UIDs, codes, source references), NIfTI
   (affine), PNG masks (raster encoding, overlap precedence), and the native
   format (must be lossless).

### Options

**(a) Code adapters only.** Every format is Rust in `crates/dcmview-adapters`.
This is the most robust option, but each lab's spreadsheet becomes a feature
request, and it misses the owner's "definable using a template language or
visual editor".

**(b) One free-form template language for everything text.** This is flexible,
but it pushes coordinate arithmetic, rounding and CSV quoting into user
templates, which is where silent errors come from. A visual editor over
free-form Jinja is not realistic.

**(c) Plugins**: dynamic libraries, WASM (wasmtime adds several MB and a new
security surface), or external commands (`--export-command python my.py`).
These are powerful but run user code inside a viewer that is meant to be a
self-contained binary. In the hub they would mean an admin-configured command
running as the hub owner. That is fine in principle, but it's a new class of
thing to audit.

**(d) Three tiers (recommended):**

| Tier | For | Who writes it | Edited visually |
|---|---|---|---|
| **Code adapter** (Rust trait from model doc 8.1) | Group 3, plus EMBED import and the parity export | us | no |
| **Table spec** (declarative JSON: row granularity, filter, columns as expressions, output format) | Group 1, and most of what users will want | users, or the visual editor | **yes** |
| **Free-form template** (minijinja file with a small manifest) | Group 2 and anything odd | power users | no, but it has the same live preview |

All three implement `ExportAdapter` and share the capability check, the loss
policy, the report and the export dialog. A table spec compiles to
expressions in the same engine that templates use, so there is one language
to learn.

**No plugins in v1.** The escape hatch for anything else is the **native
JSONL plus its JSON Schema** (model doc 6.3). Users convert with their own
script, and `dcmview_py` can ship a small reader
(`dcmview_py.annotations.load(path)` returning dataclasses, with an optional
pandas frame). The ML adapters repo needs this reader anyway. External-command
adapters can be revisited in the hub later if a real case appears.

---

## 3. Template language (A2)

### 3.1 Candidates

| | **minijinja** | Handlebars (`handlebars` crate) | Tera | Liquid (`liquid` crate) | Custom mini-format | Rhai (scripting) |
|---|---|---|---|---|---|---|
| Syntax users know | Jinja2: Python, Ansible, dbt. This is our users' world. | Mustache/JS | Jinja-like but not compatible | Shopify | none | Rust/JS-like |
| Expressions and arithmetic | full (filters, tests, math, inline if, `map`, `selectattr`) | logic-less, needs helpers for everything | full | moderate | whatever we build | full language |
| Sandbox | no I/O unless we add functions; **fuel limit** stops runaway loops (verified); recursion limit | no I/O; no fuel | no I/O; no fuel | no I/O | total | no I/O by default; operation limits |
| Standalone expressions (for table specs) | `compile_expression` (verified) | no | no | no | yes | yes |
| Streaming output | `render_to_write` | yes | no | yes | yes | n/a |
| Same template in Python | **yes**: Jinja2 runs most minijinja templates, and `minijinja-py` exists | pybars (stale) | no | python-liquid | no | no |
| Error quality | line and column, named template | ok | ok | ok | ours | ok |
| Dependencies | serde only by default | several | many (chrono, regex, ...) | several | none | moderate |

### 3.2 Recommendation: minijinja

- It's the syntax our users already know from Python, and the same template
  can be rendered from `dcmview_py` with Jinja2 over the same JSON context.
  That matters for the ML repo and for people who want to run an export
  without starting the viewer.
- `compile_expression` gives table-spec columns for free: a column is just a
  Jinja expression such as `annotation.bbox.x0` or `file.path_rel`.
- Fuel and recursion limits stop runaway template loops. Fuel is a VM
  instruction budget, not a resource sandbox: one host-helper call
  (`polygon(a, n=…)`, `tojson` or `csv_row` over a large list, a lazy
  store-backed object) counts as one instruction, and streamed output has no
  byte cap. So templates also get explicit limits: a **source-size cap** on
  specs and templates, an **output-size cap** per export (bytes written,
  checked by the writer), fuel scaled to the selection size rather than one
  fixed budget, bounded helper arguments (for example `polygon` n), and a
  **job-concurrency cap** on exports per process. The threat is narrow
  (hub specs come from the trusted admin; standalone users own their
  process), but specs inside a foreign artifact and third-party templates are
  untrusted input.

**Prototype check (scratch code, not in any repo).** An EMBED template over
minijinja 2, with a `csv_row()` function backed by the `csv` crate and a
compact `tojson`, reproduced the exact quoting from section 1 (`[]`
unquoted, coordinate lists quoted). It rendered **10,000 files with two ROIs
each (594 KB) in 96 ms** in a release build, context construction included.
A 100,000-iteration loop with a small fuel budget stopped with "engine ran
out of fuel". The binary cost is small (a few hundred KB, to measure in the
real build).

**Gotchas to design around:**

- **Autoescape off by default.** minijinja autoescapes by file extension
  (HTML, and JSON when the feature is on). Templates declare their output
  kind in the manifest, and we set escaping explicitly: none for text and
  CSV, JSON mode for `.json`, XML escaping for `.xml`.
- **Jinja2 compatibility is high but not complete.** We document which
  filters and functions are dcmview's own (`csv_row`, `bbox`, `mm`, ...).
  `dcmview_py` registers the same names so templates stay portable.
- **Numbers**: floats print the way Rust formats `f64`. Precomputing
  coordinates quantized to 1/1000 px (model doc 2.1) and offering an explicit
  `fmt(n, decimals)` filter keeps outputs deterministic.

---

## 4. The export view: what templates and table specs see (A3)

This is the most important interface in this area. If templates reached into
the raw model, every template would reimplement "bbox of an ellipse, rounded
outward, in EMBED order". So Rust code builds a **view** with the derived
values computed once and correctly. The view is versioned
(`view_version: 1`) independently of the model.

### 4.1 Shape

```text
export
  .meta        { dcmview_version, exported_at, adapter {id, version},
                 view_version, campaign? (hub only) }
               -- exported_at: server clock in standalone; in hub exports a state time
               -- the hub supplies, never the wall clock
  .schema      { classes[{id, name, color, code?}], fields[{id, name, type, options[{id,name,code?}]}] }
  .layers      [{ id, name, kind, author }]
  .files       [ File ]            -- only files with something to export, unless the spec asks for all
  .annotations [ Annotation ]      -- flat, each with .file back-reference
  .labels      [ Label ]           -- flat, each with resolved .target
  .patients / .studies / .series   -- grouped views for hierarchy-label rows

File
  key, kind ("dicom"|"raster"), format, path_abs, path_rel, paths (all alias paths),
  sop_instance_uid, sop_class_uid, study_uid, series_uid, patient_id,  -- "" for rasters
  rows, columns, frames, pixel_spacing? {row_mm, col_mm, source}, orientation (1-8),
  frame_pixel_spacing? [{row_mm, col_mm, source}]  -- per frame; pixel_spacing = the
                                                   -- common value when all frames agree, else none
  annotations [Annotation], labels [Label]

Annotation
  id, file, layer, layer_name, class, class_name, author, created_at, modified_at,
  score?, derived_from?, attributes {id: value}, attribute_names {id: display},
  frames_all (bool), frames (list; the full range when all), frame_scope ("all"|[...]),
  type ("point"|"line"|"polyline"|"polygon"|"rect"|"ellipse"|"mask"),
  geometry (the model's fields, unchanged),
  points [[x,y],...]      -- vertices; rect = 4 corners; ellipse = none (use polygon())
  bbox { x0, y0, x1, y1, w, h, cx, cy }         -- tight, float
  bbox_int { x0, y0, x1, y1 }                   -- rounded outward (floor min, ceil max)
  bbox_embed [ymin, xmin, ymax, xmax]           -- = bbox_int in EMBED order
  area_px, length_px (line/polyline), perimeter_px

Label
  id, field, field_name, value, value_name, target_kind, target_id, frame?, layer, author, score?
```

**Helpers** (functions and filters, the same in Rust and `dcmview_py`):

- `csv_row(list)` and `tsv_row(list)`: the `csv` crate's writer, so quoting
  matches section 1 exactly.
- `tojson` (compact, serde_json-identical) and `fmt(x, n)`.
- `mm(annotation_or_value, axis)`: pixel to millimetres using the spacing of
  the annotation's frame (per-frame spacing; a multi-frame annotation whose
  frames disagree returns none with a report warning). It reports which
  spacing source (tag name) was used, since mammography has
  `ImagerPixelSpacing` versus `PixelSpacing`. If a file has no spacing, it
  returns none and a report warning. In the hub the spacing comes from
  `dcmview inventory`'s per-frame `spacing` on the FileRef, since the hub
  reads no DICOM itself.
- `centre_origin(x)`, meaning `x - 0.5`, for NIfTI/ITK-style consumers
  (model doc 2.1).
- `normalized(x, file)` for YOLO-style 0 to 1 coordinates.
- `stem` and `without_ext` for output names; `without_ext` keeps the folders
  of `path_rel`, so it is the safe default for `split: file` names (5.1).
- `displayed(points, file)` maps stored-grid coordinates to the EXIF-oriented
  display grid for rasters (image-formats 6.1).
- `polygon(annotation, n=32)` approximates ellipses (and returns vertices for
  everything else).
- `voc(bbox_int)` gives Pascal VOC's 1-based inclusive box
  (`x0+1, y0+1, x1, y1` for integer edges). This is an example of a
  convention we get right once so users don't have to.

### 4.2 Rules

- **Loss policy runs before the template.** A spec declares what it accepts
  (`accept: ["rect"]`, or all types) and what to do with the rest (`skip` or
  `bbox`). The view it receives already has non-accepted shapes removed or
  converted, and the counts go in the report. So a template can't forget to
  handle ellipses. It never sees them unless it asked for them.
- **Selection before the template**: which layers (default: the current
  user's layers; in hub review, the chosen annotators), which classes, and
  "files with nothing" included or not.
- **Masks are not exposed to templates as pixels.** `type: "mask"` rows expose
  bbox, area and frame set only. Pixel output is for code adapters.
- **Lazy, not materialized.** For hub-scale exports (100k files), the view is
  a set of minijinja dynamic objects over the store, and output streams with
  `render_to_write`. The prototype number above materialized everything and
  was still fast, so this is about memory, not speed.
- **Privacy switch.** `path_abs`, `patient_id` and UIDs are present in the
  view (EMBED needs `path_abs`). A spec or template that references `path_abs`
  or `patient_id` is flagged in the export dialog ("this export includes
  absolute paths and patient IDs"). For the admin and standalone users this is
  a nudge, not enforcement. For hub annotators it is enforced: a spec whose
  output references `path_abs`, `patient_id`, UIDs or other identifier fields
  is **identifier-bearing**, and with `blind_metadata` on it is denied to
  annotators (see 5.4).
- **Stable ordering**: files in path order, and annotations in creation order
  (UUIDv7 order). Discovery order is rayon completion order and not stable
  even between two runs of the same process, and today's EMBED export follows
  it. EMBED now uses path order too (see 6.2).
- **Determinism of output bytes.** Nothing in the view depends on the wall
  clock in a hub export: `meta.exported_at` is a state time the hub
  supplies, so the same state renders the same bytes.

---

## 5. Table specs and free-form templates (A1, A8)

### 5.1 Table spec

```json
{
  "kind": "dcmview.table-export", "spec_version": 1,
  "id": "embed", "name": "EMBED ROI CSV", "version": 1,
  "rows": "file",
  "where": "annotations | length > 0",
  "accept": { "types": ["rect"], "otherwise": "skip" },
  "output": { "format": "csv", "header": true, "extension": "csv",
              "filename": "dcmview-annotations.csv" },
  "columns": [
    { "name": "anon_dicom_path", "value": "file.path_abs" },
    { "name": "num_ROI",         "value": "annotations | length" },
    { "name": "ROI_coords",      "value": "annotations | map(attribute='bbox_embed') | tojson" },
    { "name": "ROI_frames",      "value": "'[]' if annotations | rejectattr('frames_all') | list | length == 0 else annotations | map(attribute='frames') | tojson" }
  ]
}
```

- `rows` is one of `file`, `annotation`, `label`, `frame` (one row per file
  and frame with anything on it), `series`, `study` or `patient`. Inside a row,
  the names `file`, `annotation`, `annotations`, `labels` and so on are bound
  as appropriate.
- `output.format` is one of `csv`, `tsv`, `jsonl` (each column becomes a
  typed key) or `json` (an array of row objects). Parquet can come later if
  asked for. It's a natural fit, but it's a heavy dependency.
- `output.split: "file"` writes one output per file into a zip. This is how a
  spec produces "one CSV per image" layouts without free-form templates.
- **Generated names are untrusted**. Every rendered `filename` is
  normalized (UTF-8, `/` separators) and rejected if it is absolute, contains
  `..` or an empty or `.` segment, or would create a link. Two outputs whose
  normalized names are equal (for example `a/lcc.dcm` and `b/lcc.dcm` both
  rendered to `lcc.txt`), or a name equal to a reserved member (`report.json`),
  fail the export with a report listing the colliding sources; nothing is
  overwritten or silently renamed. The capability check runs the filename
  expressions, so collisions show up before export.
- A column value is a minijinja expression. The visual editor never shows it
  unless asked (5.3).

### 5.2 Free-form template

A `.j2` file with a front-matter manifest (the same keys as the spec, minus
`columns`):

```jinja
{#- dcmview-template
id: yolo-detect
name: YOLO detection labels
accept: { types: [rect, ellipse, polygon], otherwise: skip }
output: { extension: txt, split: file, filename: "{{ file.path_rel | without_ext }}.txt" }
-#}
{%- for a in annotations %}
{{ class_index(a.class) }} {{ normalized(a.bbox.cx, file) | fmt(6) }} {{ normalized(a.bbox.cy, file) | fmt(6) }} {{ (a.bbox.w / file.columns) | fmt(6) }} {{ (a.bbox.h / file.rows) | fmt(6) }}
{%- endfor %}
```

Pascal VOC XML, YOLO and a "one row per box" CSV ship as bundled examples.
They double as tests of the view. The examples name outputs after
`path_rel` without its extension (folders kept as zip folders), not after the
bare stem, so same-named files in different folders don't collide (5.1).

### 5.3 The visual editor (after the spec ships)

It's a form over the table spec, and it doesn't edit free-form templates:

- Pick the **row granularity** ("one row per image / per box / per label /
  per series").
- Build **columns** by picking fields from a tree of the view (File > path,
  Annotation > box > x0, Attribute > BI-RADS, and so on). Each column can take
  one transform from a short list: order of box coordinates, rounding, units
  px or mm, join a list, and JSON. Columns can be renamed and reordered by
  dragging.
- Set **filters** by layer and class, and choose the shape policy (skip or
  box), with the current counts shown ("12 ellipses would be skipped").
- **Live preview**: the first 20 rows rendered from the current data as you
  edit. This is the feature that makes templating pleasant. Users see the CSV
  they will get, not a language.
- An "advanced" toggle shows the expression behind each column, so power
  users can type one and still use the form.
- Save the spec as a JSON file.

The editor is built in dcmview's frontend, because standalone users need it
too. It is a reusable Svelte component.

### 5.4 Where specs and templates come from

- **Built in**: EMBED (the spec example plus the code adapter), native JSON
  and JSONL, COCO, and the bundled examples.
- **Standalone dcmview**: a repeatable `--export-template PATH` (a spec or a
  `.j2`), and an `exports` key inside `--annotation-config` (a top-level
  key in the `seams.md` 7 envelope). They are validated at startup like
  `--annotations`: a bad template exits before binding. The export dialog can
  also load a spec from disk for this session only, since the browser user
  owns the process. **Config shape (settled here, 2026-09-30):**

  ```json
  "exports": { "allowed": false, "specs": [ "embed.table.json", { "kind": "dcmview.table-export", … } ] },
  "imports": false
  ```

  `exports.specs` is the spec and template list (paths or inline specs).
  `exports.allowed` says whether annotators may export (default `false` in
  hub mode; ignored in standalone, where the user owns the process); with
  `blind_metadata` on it is forced `false` for identifier-bearing formats.
  `imports` is a boolean: whether annotators may import (default `false` in
  hub mode; the admin always may; ignored in standalone). `exports`,
  `imports` and `display` are on the envelope's list of allowed top-level
  keys, and adding a required key bumps `protocol` (`seams.md` 12).
- **Hub mode**: a spoke gets its specs read-only through `exports.specs` in
  the config the hub writes. A spoke's user can export only their own layer,
  and only if `exports.allowed` is on. The spoke enforces this for every
  export, check and import route (the hub enforces it again on its side):
  export routes are refused unless `exports.allowed` is on, import routes
  unless `imports` is true, and with `blind_metadata` on, identifier-bearing
  specs (4.2) and all imports are refused for annotators regardless. Previews
  and reports from `/export/check` follow the same rule and the blinding
  field allowlist.

---

## 6. EMBED (A4, A5)

### 6.1 Code adapter plus a template example, not one or the other

The owner wants EMBED to be the shipped template example. Parity is protected by
AGENTS.md, so it needs owner sign-off to narrow. The options:

- **EMBED export only as a table spec.** This dogfoods the template system,
  but a minijinja upgrade or a view change could shift bytes in a
  parity-protected output. Golden tests would catch it, but then parity
  depends on third-party formatting.
- **EMBED only as code**, with the template system shipping other examples.
  This is safe, but it misses the point of "EMBED as the example".
- **Both (recommended).** `embed` (code) is what `--annotations`, **Export
  ROIs**, `GET /api/annotations/export.csv` and the hub use. The
  bundled `embed.table.json` (5.1) is the example users copy and adapt. A
  golden test renders both over a fixture set and asserts identical bytes, so
  the example can never drift into being wrong. Each is about 50 lines.
  Import is code only either way.

### 6.2 Parity details the adapter must keep

- The header is `anon_dicom_path,num_ROI,ROI_coords,ROI_frames`. The quoting
  and LF rules are as in section 1, and `serde_json` compact arrays are used.
- `anon_dicom_path` is the absolute canonical path in standalone dcmview, as
  today, with one row per loaded path, including aliases (model doc 9.2).
- **Row order (amended 2026-09-30).** Today it follows `files_snapshot()`
  order, which is **not** walk order: discovery runs `par_iter`, and the
  registry assigns indices in rayon completion order
  (`src/loader/discovery.rs:276`, `src/server/catalog.rs:82`), so row order
  varies between runs. The EMBED adapter therefore **sorts rows by source
  path** (byte order of the UTF-8 path string as recorded in
  `anon_dicom_path`), the same path order 4.2 uses for the other adapters.
  This changes a parity-protected output and has the owner's sign-off under
  AGENTS.md. Until the sort lands, golden tests compare rows
  order-insensitively (header and each row's bytes still exact). This is a
  small point, but it's exactly the kind of thing that breaks a byte-parity
  test.
- **Box order `[ymin, xmin, ymax, xmax]`** (the owner: `{y0, x0, y1, x1}`). It
  comes from `bbox_int`, which equals the rect exactly when coordinates are
  integers. Non-integer rects round outward and are counted in the report.
- **Frames**: `"[]"` when every exported ROI on the file is all-frames.
  Otherwise there is one explicit list per ROI, and an all-frames ROI in a
  mixed file expands to `0..frames-1`. That is the annotation model's
  confirmed rule. I kept the expansion rather than a per-ROI `[]`, because
  dcmview 0.3.x hides a per-ROI `[]` (parity finding 1), so explicit lists
  stay correct for older readers.
- **Only `rect` annotations** (the owner, 2026-09-29). Other types are `skip` by
  default and `bbox` on request, and both are reported. This stays marked to
  revisit when the EMBED convention for non-box shapes exists.
- **Classes and attributes are dropped** with a report ("3 classes merged
  into one CSV; attributes not exported"). The dialog offers a class filter,
  so "EMBED CSV of just the `mass` class" is one click. **Labels are never
  exported by EMBED.**
- **Rasters**: exported under `anon_dicom_path` unchanged. The column name
  becomes a misnomer (image-formats 10.9), but renaming would break every
  EMBED reader. A `path` column belongs to the future extended mode, not to
  classic EMBED.

### 6.3 EMBED in the hub

- **Two annotators on one image means two rows with the same path.** On
  re-import that is "duplicate matching anon_dicom_path", which fails the
  whole import (parity rule). So a combined CSV is unusable in dcmview, and
  hub exports are **one CSV per annotator**. A combined CSV with an extra
  `annotator` column can be offered for analysis, labelled as not reloadable.
- **Path mode**: an adapter option. Absolute paths reload on the same server;
  relative paths reload only when dcmview is started from the same root.
  Model doc 1.3 keeps absolute paths out of exported model data by default,
  but EMBED needs a path that matches on load, so **EMBED written by a hub
  uses absolute paths by default, with a `relative` option**. The native
  JSONL carries keys and relative paths, so a move to another server
  re-resolves by UID or digest anyway.

### 6.4 The future extended mode (not designed, only kept possible)

Model doc 9.4 describes `embed-extended`: boxes still in `ROI_coords` for
every ROI, plus parallel JSON columns. This design keeps it cheap. It would be
a second mode on the same code adapter, with its own import and golden tests,
and the table-spec example would gain columns like `annotations |
map(attribute='type') | tojson`. Nothing here blocks on the owner's draft.

---

## 7. Code adapters: what each format needs (A6)

Each entry lists fidelity, conventions and what it needs from
`ExportContext`. Priority is in section 10.

### 7.1 Native `dcmview.annotations` JSON and JSONL

- **Lossless** import and export. This is what the hub consumes and
  what `dcmview_py` and the ML repo read. It is validated against the
  generated JSON Schema. Unknown fields round-trip (model doc 6.2).
- JSON is one document. JSONL is one record per line with a header line
  (`format`, `version`, `schema`, `layers`, `files`), so a large campaign
  streams.
- Needed from day one: the hub and every round-trip test depend on it.

### 7.2 COCO (export and import)

- **Boxes**: `[x, y, w, h]` from `bbox` (float), with corner-origin
  coordinates used as is. COCO doesn't state a pixel-centre convention. The
  common toolchains (pycocotools rasterization, detectron2) are consistent
  with corner origin, and we say so in the report.
- **Polygons** become `segmentation: [[x1,y1,...]]`. Ellipses become polygons
  (`polygon()`, n=32 by default, reported). Lines, polylines and points are
  skipped by default. Points can be exported as keypoints if the schema
  class has a keypoint definition, which is out of v1.
- **Masks** become RLE in **column-major (Fortran) order**, uncompressed
  `counts`. This is pycocotools' order, and getting it wrong transposes every
  mask. We test it against pycocotools in Python CI.
- **Categories** come from schema classes, with ids assigned 1..n in schema
  order and the stable class id kept in an extra field. `images[]` has one
  entry per file, or per (file, frame) for multi-frame files, with
  `file_name` as the relative path plus `frame` and `file_key` as extra
  fields. Multi-frame is not in the COCO standard, so this is declared.
- **Labels** go into image-level extras (`images[].attributes`), which is
  non-standard and reported.
- **Import**: both the full dataset format and the **results format**
  (`[{image_id, category_id, bbox|segmentation, score}]`). That is what most
  detectors emit, so COCO results import into a `model` layer with `score`
  is the cheapest route to pre-labels later (model doc 8.2). Dataset-format
  files are matched through the resolver by `file_key`, then `file_name` as a
  relative path. **Results records carry only numeric `image_id` and
  `category_id`**, so results import requires the **originating
  dataset or manifest** (dcmview's own COCO export, which carries the
  `file_key`, `frame` and stable class id extras) or an **explicit image,
  category and frame mapping** supplied with the import. Numeric category ids
  are never interpreted against the current schema order, since 1..n drifts
  when the schema changes. Any `image_id` or `category_id` the mapping doesn't
  know is rejected (a matched-record error, 8).

### 7.3 PNG masks (export and import, once masks exist)

- One PNG per (file, frame, layer). It's a **label map** (8-bit, or 16-bit
  above 255 classes) with pixel value = class index, plus a sidecar
  `classes.json` mapping values to class ids. Optionally there's one binary
  PNG per segment (instance style).
- The class label map is **lossy for instances**: exclusive layers
  keep pixel-to-class exactly, but two same-class segments merge, and
  per-segment attributes and authorship are lost. The per-segment PNGs (with
  an instance-id sidecar mapping each PNG to its annotation id, class and
  attributes) are the instance-preserving option; the report says which was
  used.
- Overlap in non-exclusive layers is resolved by an **explicit, stable
  overlap-priority list** of class ids in the export options (default: schema
  class order), then newest wins within a class. Layer order is view state
  and never persisted, so it plays no part. The list used and the
  overlapping pixel count go in the report (model doc 3.2).
- Stored pixel grid, the same as annotations. An "as displayed" option for
  EXIF-rotated rasters.
- Import: palette or grey PNGs become one segment per value, into an import
  layer, matched to the source by the resolver (by name pattern
  `<stem>[_<frame>].png` or a mapping CSV).
- Uses the `image` crate's PNG encoder, which is already a dependency.

### 7.4 DICOM SEG (export first, import later)

- **Why code**: it's an IOD, not a text format. It needs new UIDs, the
  source's patient, study and frame of reference attributes (from
  `ExportContext`), a Segment Sequence, per-frame functional groups
  referencing source instances and frames, and packing.
- **SOP class choice**: binary SEG for non-exclusive layers (one segment per
  mask annotation, overlap allowed). **Labelmap SEG** (the newer SOP class,
  which dcmview already displays) for exclusive layers. Fractional SEG for
  `depth: 8` model outputs.
- **Codes are required**: Segmented Property Category and Type code sequences
  are Type 1. Schema classes carry an optional `code` (model doc 4.2). Classes
  without one get a documented generic placeholder pair, and the pre-export
  check warns ("3 classes have no code; SEG will use a placeholder").
- **Scope**: one SEG per source image for projection images such as MG and
  DX. One SEG per series for CT and MR stacks, which needs the series'
  geometry and is the harder half.
- **Where it runs**: the hub links no DICOM reader and its FileRefs carry no
  headers, so SEG export, in the hub and in standalone alike, runs inside
  dcmview. In hub mode it is a `dcmview export` subprocess the hub starts
  (with `DCMVIEW_VSCODE_BYPASS=1`, like `dcmview inventory`), given the export
  view's input (native JSONL for the selection plus the file list with keys)
  and writing into a fresh output directory. It reads the source headers and
  **per-frame pixel spacing** itself, so geometry is never copied through the
  hub. Any other adapter that needs DICOM headers takes the same route.
- **Writer**: dicom-rs (`dicom_object`), which dcmview already links. There's
  no maintained Rust SEG writer, so we build a small one and **validate it
  through the test corpus's conformance pipeline** (dciodvfy plus
  dicom-validator), which already covers SEG. highdicom in Python is the
  reference to compare against in tests, not a runtime dependency.
- **Import**: dcmview already decodes SEG frames for overlays, so importing
  into mask annotations is mostly mapping (segment to annotation, referenced
  frame to file key and frame, pixels to tiles). It's worth doing once masks
  exist, because existing SEGs are the most common masks in DICOM datasets.

### 7.5 NIfTI (later)

This only makes sense for 3D series (CT and MR). It needs the affine from DICOM
geometry, with the LPS to RAS flip, centre-origin voxels, and slice order.
For 2D mammography it's the wrong format. It also needs a small writer (the
format is simple) or a new crate. I'd do it on demand. Like SEG, it runs in
the `dcmview export` subprocess in hub mode, which reads the
geometry and per-frame spacing from the source headers; a series whose
frames disagree in spacing or orientation is refused with a report rather
than resampled.

### 7.6 DICOM SR TID 1500 (later, on demand)

This is vector shapes as SCOORD measurement groups, for PACS-bound workflows.
Our corner-origin convention is SCOORD's (model doc 0), so there's no
conversion. It's a large IOD to get right. The corpus has a TID 1500 recipe
to validate against.

### 7.7 Fidelity matrix (what the capability check uses)

| Adapter | point | line/polyline | polygon | rect | ellipse | mask | class | attributes | hierarchy labels | multi-frame | score |
|---|---|---|---|---|---|---|---|---|---|---|---|
| native | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| EMBED | skip | skip/bbox | skip/bbox | ✓ | skip/bbox | skip/bbox | dropped | dropped | dropped | ✓ | dropped |
| COCO | opt. | skip | ✓ | ✓ | polygon | RLE | ✓ | extra | extra | extra | results fmt |
| PNG masks | – | – | fill | fill | fill | ✓ | ✓ | – | – | per frame | – |
| DICOM SEG | – | – | fill | fill | fill | ✓ | codes | – | – | ✓ | fractional |
| table spec / template | as declared by its `accept` | | | | | bbox only | ✓ | ✓ | ✓ | ✓ | ✓ |

"Fill" means the model's fill-from-shape (the tools doc's tool). The mask
adapters can offer it as an export option ("rasterize polygons and boxes into
the mask"), which is reported.

---

## 8. Import (A7)

- **Templates don't import.** There's no reliable "reverse template". Import
  is code adapters only.
- **v1**: native (lossless) and EMBED (unchanged parity path).
- **Next**: COCO (dataset and results). With masks: PNG masks and DICOM SEG.
- **Later**: a **CSV column-mapping importer**. It's the inverse of a table
  spec restricted to simple column references: which column is the path or
  key, which holds boxes in which order (`yxyx`, `xyxy`, `xywh`) or a JSON
  list, which is the class, and which are attributes. It's worth doing because
  every lab has a CSV. It fits the same visual editor.
- **EMBED CSV import stays lenient on geometry.** As in 0.3,
  out-of-range boxes and zero-area boxes on a matched row **load**; they are
  not canonicalized away or rejected. Each such row is listed in the import
  report (`out_of_range`, `zero_area` warnings with row numbers and file
  keys), and such rows go into the golden fixtures. The rest of the EMBED
  checks (header, JSON shape, `num_ROI`, `ROI_frames` length, frame ranges)
  stay fatal as today, with one exception (amended 2026-10-08, model doc
  2.1 and its EMBED parity amendment): a negative coordinate becomes 0 and
  a non-integer coordinate is rounded to the nearest pixel, each with a
  warning in the report, instead of failing the import. A coordinate past
  the image edge is never changed on load. The model's "rejects out-of-range geometry" (model
  doc 2.1) applies to PUT/ops and to the **new native formats**, which
  validate strictly. The overstated `docs/annotations.md` wording is a
  separate shipped-docs fix, on the owner's word.
- **Semantics for every importer** (model doc 8.1): the import goes into a
  chosen layer (`import` kind by default, the default layer for EMBED
  parity) with author `import:<adapter>`. Files are matched through the
  resolver, and each row reports which matcher hit. The report is
  mandatory.
- **Failure policy**: invalid data on a matched record fails the whole import
  (the EMBED rule, applied to every format so behaviour is predictable).
  Unmatched records are counted, not fatal. For EMBED CSV, "invalid" keeps
  today's meaning, so out-of-range and zero-area geometry is reported, not
  invalid.
- **Where import happens**: `--annotations PATH` sniffs the format. An EMBED
  header takes today's exact path, and anything else uses the adapter whose
  `sniff` is most confident. `--annotations-format` forces one. In the UI,
  "Import annotations…" or drag and drop onto the viewer posts the file to
  `POST /api/annotations/import`. In hub mode, the spoke refuses the import
  route for its user unless the config's `imports` key is `true`, and always
  with `blind_metadata` on (5.4). Imported files from another system
  (artifacts, zips of masks) are untrusted: members are extracted into a fresh
  directory with the same path, link, duplicate and size rules as generated
  names (5.1) and validated before anything is imported.

---

## 9. Export UX, API and reports (A9, A10)

### 9.1 UX

- **Export ROIs stays one click and produces today's EMBED CSV byte for
  byte**, except that rows are now sorted by path (today's order was not
  reproducible anyway). Current users see no other change.
- Next to it, an **Export as…** menu lists the built-in adapters and any
  configured specs and templates. It opens a small dialog with:
  - selection (layers, classes, files: all, the current gallery selection, or
    the current image);
  - adapter options (shape policy, path mode, units, orientation for
    rasters);
  - the **capability check before export**, in plain words: "EMBED can't hold
    4 polygons and 2 ellipses; they'll be skipped (or exported as boxes)",
    and "this export includes absolute paths";
  - the live preview for text formats.
- After export, a short **report** toast: "Exported 214 boxes from 88 images;
  6 shapes skipped". A "details" link opens the full report, which can be
  downloaded as JSON. The toast and details come from the report the actual
  render produced (9.2), not from the earlier check.

### 9.2 API (declared in `contracts.rs` as usual; all behind the token)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/annotations/export.csv` | **Unchanged**: the EMBED code adapter with default options |
| GET | `/api/annotations/adapters` | Built-in and configured adapters, with `AdapterInfo` (id, name, kind, direction, extension, capabilities, options schema) |
| POST | `/api/annotations/export/check` | `{adapter, selection, options}` returns the report without output: counts, losses, privacy flags, first N preview rows, and a **`snapshot` token** (the committed store revision the check read) |
| POST | `/api/annotations/export` | Same body plus the `snapshot` token. Renders from that snapshot or, if records in the selection changed since, refuses with 409 `stale_snapshot` so the dialog re-runs the check. Streams the file (or zip); the response carries a short `X-Dcmview-Export-Id`, and the zip also holds `report.json`. |
| GET | `/api/annotations/export/{id}/report` | The report **produced by that render**, so what the toast shows describes the downloaded bytes. No custom header size problem. |
| POST | `/api/annotations/import` | Raw body plus `?adapter=&layer=`. Returns an import report; the records arrive as ops. |

The frontend fetches the export and saves it as a blob (`seams.md` 14).

### 9.3 Report shape (shared by all adapters)

```json
{
  "adapter": { "id": "embed", "version": 1 },
  "counts": { "files": 88, "annotations_in": 222, "annotations_out": 214, "labels_out": 0 },
  "losses": [
    { "code": "type_skipped", "detail": "polygon", "count": 4, "examples": ["01J9Z3..."] },
    { "code": "rounded_outward", "count": 2 },
    { "code": "attributes_dropped", "count": 31 }
  ],
  "warnings": [ { "code": "absolute_paths" }, { "code": "class_code_placeholder", "detail": "mass" } ],
  "matched_by": { "key": 0, "sop_instance_uid": 0, "path": 88 }   // import only
}
```

Codes are a closed enum in the crate (stable wire strings), so the hub can
aggregate them and the UI can phrase them. The review added import warnings
`out_of_range` and `zero_area` (EMBED leniency), export errors
`unsafe_output_name`, `duplicate_output_name` and `stale_snapshot`, loss code
`instances_merged` (class label map, 7.3) and warnings `overlap_priority` (the
list used) and `denied_identifier_bearing`.

### 9.4 Multi-file outputs

A spec with `split: "file"`, PNG masks and SEG sets produce several files.
Member names follow 5.1's rules (no absolute, traversing or link paths, no
duplicate normalized names), and the writer enforces the output-size cap
(3.2). **Recommend zip**, streamed with the `zip` crate (a new dependency,
pure Rust, deflate through the existing `flate2`). tar is an alternative, but
zip opens everywhere.

---

## 10. Suggested order (for later, not now)

1. **With the model foundations**: `crates/dcmview-adapters` with the traits,
   the registry, the report types and the native JSON/JSONL adapter. The
   EMBED code adapter is moved over the new store, with golden byte tests
   frozen from today's output **before** the refactor (including the
   unquoted `[]`). Today's row order is rayon completion order, so goldens
   compare rows **order-insensitively** (header and row bytes exact) until
   the path sort lands; after that they compare byte for byte in path order.
   Out-of-range and zero-area CSV rows are in the fixtures, loading
   leniently with a report.
2. **With annotation tools v1**: the export view v1, minijinja, table specs
   and templates, the adapters, check and export endpoints, the Export as…
   dialog with preview, the EMBED table-spec example plus its byte-equality
   test, COCO export (boxes and polygons), YOLO and VOC examples. This comes
   earlier than first planned (templates after masks), because the table spec
   is small and every new shape type needs somewhere to export to.
3. **Spoke mode**: `exports.allowed` and `imports` enforcement in the spoke,
   and the `dcmview export` subprocess once SEG or NIfTI is needed. (The
   studio side of this step is not part of the public design.)
4. **With masks**: PNG masks export and import, COCO RLE, DICOM SEG export
   (validated through the corpus conformance pipeline), then SEG import. COCO
   results import can move earlier if pre-labels are wanted sooner.
5. **Then**: the visual editor over table specs, the CSV column-mapping
   importer, a `dcmview_py` reader and template renderer, and NIfTI and SR on
   demand.

---

## 11. Risks and things left out

- **Parity by two implementations.** EMBED exists as code and as a spec. The
  byte-equality test is what keeps that honest. If it's ever skipped, the
  example silently rots.
- **Template resource use.** Fuel bounds instructions, not memory or output
  bytes; the source, output and job-concurrency caps in 3.2 cover the rest.
- **Template portability to Jinja2** is "high, not total". The shared helper
  names are the contract, and a test renders the bundled examples in both
  engines.
- **SEG writer effort** is the largest item here, and mostly IOD detail. The
  corpus conformance pipeline reduces the risk a lot.
- **No plugin mechanism in v1.** Formats we don't ship go through native
  JSONL and a script. That's the right trade for a self-contained binary, but
  some users will ask.
- **ML**: model outputs fit without changes. Scores are in the view and in
  COCO results, and fractional masks go to SEG. Nothing ML-specific is built
  here.
- **Not decided here**: the dialog's visual design details (the tools doc
  owns annotation UX in general) and transport (`seams.md`).

---

## 12. Decisions (all confirmed by the owner, 2026-09-30)

1. **Adapter kinds (A1)**: code adapters, declarative table specs and
   free-form templates, with no plugins or external commands in v1. Custom
   conversions go through native JSONL plus a `dcmview_py` reader.
   **Confirmed.**
2. **Template language (A2)**: minijinja (Jinja2 syntax). **Confirmed.**
   The alternatives were Handlebars, Tera, Liquid, a custom format and Rhai.
3. **Export view (A3)**: templates see a precomputed, versioned view (boxes in
   several conventions, mm, normalized, and so on) with the shape-loss policy
   applied before they run, never the raw model. **Confirmed.** **Amended
   2026-09-30**: in hub exports `meta.exported_at` is a state time, not the
   wall clock, so the same state gives the same bytes; spacing is per frame
   (4.1).
4. **EMBED (A4)**: the code adapter stays the parity path. The shipped
   example is an EMBED table spec, kept byte-identical to it by a test.
   **Confirmed.** **Amended 2026-09-30**: EMBED rows are sorted by
   source path (today's order is rayon completion order, not walk order);
   goldens compare rows order-insensitively until the sort lands.
5. **EMBED in the hub (A5)**: one CSV per annotator, because a combined file
   can't be reloaded. Absolute paths by default, with a relative option.
   **Confirmed.**
6. **EMBED classes**: all `rect` classes go into one CSV with a report, and a
   class filter is available in the dialog. **Confirmed.**
7. **Format list and priority (A6, section 10)**: native, EMBED, COCO boxes
   and polygons, and the templates in the tools-v1 release. PNG masks, COCO
   RLE and DICOM SEG with masks. NIfTI and SR on demand. **Confirmed.**
8. **Import (A7)**: code only; COCO (including results with scores into a
   model layer) next, and a CSV column-mapping importer later. The failure
   policy is all-or-nothing on invalid matched records for every format.
   **Confirmed.** **Amended 2026-09-30**: EMBED CSV import is
   lenient on geometry: out-of-range and zero-area rows load as in 0.3 and
   are listed in the import report; native formats validate strictly.
   **Amended 2026-10-08**: on that import a negative coordinate becomes 0
   and a non-integer one is rounded, each with a warning; a coordinate past
   the edge is kept as loaded. COCO
   results import requires the originating manifest or an explicit mapping.
   Hub-mode import permission is the `imports` key, enforced in the spoke,
   and all imports are denied to annotators under `blind_metadata`.
9. **Visual editor (A8)**: a form with live preview over table specs only,
   after the specs ship, built in dcmview. **Confirmed.**
10. **Export UX (A9)**: keep one-click Export ROIs unchanged, add Export as…
    with a check, preview and report. **Confirmed.** **Amended 2026-09-30**:
    Export ROIs is unchanged except for path-sorted rows; the
    check returns a snapshot token that the export must present, and the
    final report comes from the actual render.
11. **Zip for multi-file outputs (A10)**, which adds the `zip` crate.
    **Confirmed.** **Amended 2026-09-30**: generated member names are
    untrusted (no absolute, traversing or link paths; collisions and
    duplicate normalized names fail the export).
12. **Docs nit**: `docs/annotations.md` shows `"[]"` quoted, but the real
    output is unquoted `[]`. Should I fix the docs example (not the output)
    when this work starts? **Confirmed.**

---

## 13. Interfaces for downstream areas

What this doc fixes. Section 12 was confirmed by the owner on 2026-09-30 and
amended the same day after the reviews.

**All areas**
- Adapters live in `crates/dcmview-adapters`, which depends on
  `crates/dcmview-annotation`. It uses no axum, tokio or pixel pipeline, apart
  from what `ExportContext` passes in. dcmview's export endpoint and the hub
  call the same code, so outputs are identical.
- Every adapter reports through one `Report` type with a closed set of loss
  and warning codes (9.3). Exports and imports are never silent.
- A **table spec** (`dcmview.table-export`, `spec_version: 1`) and a
  **template** (`.j2` plus a manifest) are plain files with stable `id` and
  `version`. They are portable between dcmview, the hub and `dcmview_py`.
- The **export view** (`view_version: 1`, section 4) is the only contract
  templates depend on. It changes by adding fields, and removing one bumps
  the version. `frame_pixel_spacing` is such an addition.
- EMBED export rows are sorted by source path. EMBED CSV import is
  lenient on geometry with a report; every other importer is strict.
- Generated output names and imported archive members are untrusted (5.1,
  8): no absolute, traversing or link paths, no duplicate normalized names.
- Export is a check/render pair bound by a snapshot token; the report comes
  from the render (9.2).

**Annotation tools and UX (`annotation-tools-ux.md`)**
- The export and import dialogs, the pre-export capability check and the
  report toast are in this doc (9.1). The tools doc owns their look, and should
  keep one-click Export ROIs where it is.
- Fill-from-shape is also offered by mask adapters as an export option, so
  the rasterization rule for polygons, rects and ellipses (which pixels count
  as inside, by pixel centre) must be one shared function in the model crate,
  not one in the tool and another in the exporter.

**The hub**
- Studio-side; not part of the public design. See `seams.md` for the dcmview
  side.

**Integration (`seams.md`)** (for information; nothing contradicts its contract)
- The new endpoints are in 9.2. All are `fetch`-based and behind the token.
  Downloads are fetch-then-blob.
- `--annotation-config` gains top-level `exports: { allowed: bool, specs:
  [spec or template paths or inline specs] }` and `imports: bool` (5.4 has
  defaults), both on the envelope's allowed-keys list; a new required key
  bumps `protocol`. There's
  a new repeatable `--export-template PATH` and a `--annotations-format`
  override. Bad templates fail at startup.
- New seams for the consolidated seam list: the `dcmview export`
  subprocess (supervised, `DCMVIEW_VSCODE_BYPASS=1`), per-frame `spacing` in
  the inventory FileRef, and the export snapshot token.

**Future ML adapters repo**
- It consumes native JSONL through the JSON Schema and the `dcmview_py`
  reader. Model outputs arrive as COCO results or native JSONL into a `model`
  layer with `score`. Fractional masks export as fractional SEG.
