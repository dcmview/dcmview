# Annotation core model and file identity

Scoping analysis, not a spec. Nothing was changed in any repository. Read
against `dcmview/dcmview` at `b78536f` (0.3.1 dev) and `dcmview-test-corpus`
at `16f1d92`. Date: 2026-09-29.

Already decided and not re-argued here: dcmview stays ephemeral and the hub owns
durable state; one neutral annotation model with EMBED as an adapter at parity;
standalone gets a URL token and optional Unix socket.

Each section gives the options, what they cost against our code, and a
recommendation. Decisions that need the owner are collected in section 12, and
the contract later areas can rely on is in section 13.

**Amended 2026-09-30 after two external reviews** (the resulting decisions
were all confirmed by the owner as recommended, plus the "fixes, no decision
needed"). The changes to this doc are listed below and made in place.

## Review amendments (2026-09-30)

- **0**: `uuid` is only a dev dependency today, so it becomes a new runtime dependency; `sha2` is direct, not transitive.
- **1.3, 13**: keys are hub-authoritative in hub mode and session-scoped in standalone; persisted records resolve through `FileRef` evidence; a key-rule version; digest encoding defined.
- **1.3 resolver**: every available strong identifier is checked before a match is accepted; a digest conflict is a refusal, not a match.
- **1.6**: removed the claim that hub-passed keys satisfy "computable from the file alone"; a spoke that computes a different key refuses the file; per-frame pixel digests separate from the whole-file digest.
- **1.7**: whole-file hashing is background I/O with a visible pending state, not "free from the page cache"; standalone rekeying is one atomic event over records, queues, history and selection.
- **1.8**: pending raster keys reach the client through the catalog revision cursor (`?since=<revision>`, updates included) and an `X-File-Key` header on the first frame response.
- **2.1**: EMBED CSV import is lenient for out-of-range and zero-area rows, with a report; native formats validate strictly.
- **4.3**: folder targets carry a root id; missing hierarchy ids get non-collapsing fallback ids; each target has a canonical target id.
- **6.1**: `op_id` is UUIDv7 everywhere; imports run by the hub itself are authored `import:<adapter>`.
- **7.2 to 7.4**: every op names a versioned target (`SetLabel` gets id and `base_rev`, layer ops get id and `base_rev`, delete and restore specified); queue key per op; `Batch` is atomic and returns one result through a global ordered client queue; one transaction per op or batch.
- **9.1, 9.2**: import report lists lenient CSV rows; EMBED export rows sorted by path.
- **11 step 1**: golden tests sort-aware (order-insensitive until the path sort lands) and include out-of-range rows.
- **12**: decisions 1 and 10 amended; new items 11, 12 and 13 (op targets).
- **13**: interfaces updated for all of the above.

---

## 0. What the code does today (the facts this builds on)

- **Wire type**: `EmbedRoiAnnotations { num_roi, roi_coords: Vec<[u32;4]>, roi_frames: Vec<Vec<u32>> }`
  (`src/api/contracts.rs:991`). Rectangles only, `[ymin, xmin, ymax, xmax]`.
- **Store**: `src/annotations.rs` `AnnotationStore` is a `HashMap<usize, EmbedRoiAnnotations>`
  keyed by **`FileEntry.index`, which is discovery order** and not stable across
  processes. `user_edited` stops the background CSV import from overwriting
  edits made while it was loading.
- **File matching**: EMBED rows match by normalized absolute path, plus a
  canonicalized alias so relative or symlinked paths match
  (`build_file_lookup`, `matching_file_targets`). One path key can hit several
  loaded files (a symlink and its target), and each gets a copy.
- **Coordinates are pixel edges, half-open.** `canonicalize_annotations` allows
  `ymax == rows` and requires `ymax > ymin`. The frontend
  (`viewport/viewTransform.ts` `clientToImagePoint`) documents
  "pixel (c, r) spans [c, c + 1) x [r, r + 1)", the overlay SVG uses
  `viewBox="0 0 columns rows"`, and `canonicalRect` rounds to the nearest
  integer edge. So today's convention is **corner origin, continuous, x = column,
  y = row, in the stored pixel matrix** (view flips and rotations are view
  state only, inverted in `clientToImagePoint`). This matches DICOM SCOORD
  (top-left corner of the top-left pixel is 0,0; its centre is 0.5,0.5).
- **Frames**: zero-based indices within the file. Row-level `ROI_frames = []`
  means "every ROI on every frame".
- **Frontend store** (`viewport/annotationStore.svelte.ts`): whole-file
  replace through a revisioned save queue; ROIs are addressed by **array
  index**; selection is an index.
- **Identity fields already read at discovery** (`FileEntry`, `src/types.rs:130`):
  path, SOP Instance/Class UID, Study/Series UID, Patient ID, rows, columns,
  frame count, and in `SeriesMetadata` the frame of reference, pixel spacings,
  concatenation and WSI pyramid fields. `references.rs` already resolves
  cross-instance references by SOP UID rather than index.
- **No content hash** anywhere today. `sha2` is already a direct dependency
  (`Cargo.toml:79`). `uuid` is in `Cargo.lock` only through a dev dependency
  (`expect-json` ← `axum-test`), so UUIDv7 ids make it a **new runtime
  dependency**. `blake3` is not present. (Corrected 2026-09-30.)

### Two parity findings to settle before the refactor

These are ambiguities in current behaviour that the neutral model must pick a
side on, because "EMBED at parity" needs a definition of what parity is.

1. **An empty per-ROI frame list.** `docs/annotations.md` says "Empty frame lists
   mean the ROI applies to all frames". The backend accepts `[[0,1],[]]`. The
   frontend (`roiEditing.ts` `visibleRois`) shows a ROI with `[]` on **no**
   frame and labels it "no frame mapping". So a CSV row like that loads, is
   invisible, and exports unchanged.
2. **Editing rewrites `[]`.** `normalizeAnnotationsForEdit` expands a row-level
   `ROI_frames = []` into explicit lists the first time any ROI on the file is
   edited. An FFDM row loaded as `"[]"` exports as `"[[0]]"` after an unrelated
   edit; a 60-slice DBT row exports sixty indices per ROI.

Recommendation: the model has an explicit `FrameScope::All` (section 2.3), a
per-ROI `[]` imports as `All` (what the docs say), and EMBED export writes
`"[]"` when every ROI on the file is `All`, else explicit lists. That changes
finding 2's output for edited files, so it needs the owner's sign-off under
AGENTS.md. The alternative is byte parity with today's quirk.

---

## 1. File identity

This doc owns it. Everything else (imports, exports, label targets, syncing,
and whatever a hub stores) keys on it.

### 1.1 What identity has to survive

| Situation | Frequency | Needs |
|---|---|---|
| Same process, same files | always | anything works, even `index` |
| Relaunch on the same machine | common | not discovery order |
| Hub campaign: setup scan vs spoke processes | every campaign | same key computed independently by hub and spoke |
| Exported annotations reloaded on another server or mount point | planned | not absolute paths |
| Files moved or renamed between campaigns | occasional | not paths at all |
| Byte-identical copies in two folders | common in research dumps | detect, decide whether they are one image |
| Two different files with the same SOP Instance UID | real (bad anonymizers, the corpus has `negative/identity/meta_dataset_uid_mismatch`) | detect, never silently merge |
| Re-anonymization that regenerates UIDs | common (EMBED-style pipelines) | fall back to content or path |
| Rasters (PNG/JPEG/TIFF) | new | no intrinsic identifier at all |

### 1.2 The options

**A. Path only** (what EMBED does). Free and human readable, but breaks on
every move, differs between the hub and a laptop export, and cannot tell
copies from the same file. Fine as a *matching hint*, not as a key.

**B. SOP Instance UID for DICOM.** Free (already read at discovery), survives
moves and renames, and is what DICOM SR, SEG and presentation states use to
reference images, so DICOM adapters need it anyway. Weaknesses: absent in
malformed files, sometimes duplicated, regenerated by re-anonymization, and it
does not exist for rasters.

**C. Content hash of the whole file.** Survives moves and renames and works
for every format. Cost is the whole read: dcmview discovery reads headers only,
and a mammography set is often 30 to 60 MB per file. At a realistic 300 to
500 MB/s of storage throughput, 10,000 FFDM files is 10 to 20 minutes of pure
I/O, which breaks "fast startup". It also changes when only metadata changes
(a de-identification re-run edits headers but not pixels).

**D. Hash of the decoded pixels or of the PixelData element.** Survives header
edits, but costs a decode (JPEG 2000 is the slow case) or at least a full
PixelData read, and two encodings of the same pixels still differ for the
element hash.

**E. A composite reference with a derived key.** Store every identifier we
have (UID, digest if known, path, size, dimensions), derive one key string from
the strongest one available, and resolve imports through an ordered list of
matchers.

### 1.3 Recommendation: E, with a cheap key and expensive fallbacks only where needed

A **`FileKey`** is a short string with a scheme prefix:

| Kind | Key | When computed | Cost |
|---|---|---|---|
| DICOM with a SOP Instance UID unique among loaded files | `sop:<SOPInstanceUID>` | at discovery | free |
| DICOM duplicate UID, byte-identical | same `sop:` key, files become **aliases** | at discovery, only for the colliding files | one hash per collision |
| DICOM duplicate UID, different bytes, or no UID | `b3:<blake3 of file bytes>` | at discovery for those files only | rare |
| Raster | `b3:<blake3 of file bytes>` | on first decode (hash while reading, the bytes are already in memory), or eagerly by the hub at campaign setup | ~free at decode; rasters are small |

Why BLAKE3: several GB/s per core and parallel, so hashing is always I/O bound.
SHA-256 is fine too if we prefer a dependency that is already present; the
scheme prefix lets us change it later. Recommend `b3`, use the full 256-bit
digest in keys (64 hex chars), never a truncation.

**Who decides a key (amended 2026-09-30).** A DICOM key depends on
which *other* loaded files share its UID, so a key is not computable from the
file alone, and the model no longer claims it is:

- **Hub mode: the hub's key is authoritative.** The hub computes keys once
  over the whole pool at setup (`dcmview inventory`) and writes them into every
  spoke file list; the `key` column is **mandatory** in hub-written file lists.
  A spoke never derives or changes a key. If a spoke computes a different key
  for a file (for example a raster whose bytes changed since setup), it
  **refuses that file** and reports it; it does not rekey.
- **Standalone: keys are session-scoped.** They are stable for the life of
  the process and can change within it (1.7). Anything persisted (the `file:`
  sidecar, exports) is resolved on reload through the `FileRef`
  evidence and the resolver below, never by trusting a stored key alone.
- **Key-rule version.** The rules in this table carry a version
  (`key_rules: 1`) in the shared crate and in the protocol; a spoke whose rule
  version differs from the hub's refuses to start. Any change to how a key is
  derived bumps it.
- **Digest encoding.** Every digest is lowercase hex over a defined canonical
  byte layout: `b3:` and `FileRef.digest` hash the file bytes exactly as
  stored; SHA-256 values, where an adapter needs them, use the same encoding
  with a `sha256:` prefix. Pixel digests are defined in 1.6.

Alongside the key, every file the model knows about has a **`FileRef`**
record carrying all the evidence, so later matching does not depend on which
key won:

```json
{
  "key": "sop:1.2.840.113681.2863050711.1286.3688.28",
  "kind": "dicom",
  "sop_instance_uid": "1.2.840...28",
  "sop_class_uid": "1.2.840.10008.5.1.4.1.1.1.2",
  "study_instance_uid": "...", "series_instance_uid": "...", "patient_id": "...",
  "path": "cohort_a/patient_0001/mg/lcc.dcm",
  "size_bytes": 34819072,
  "digest": null,
  "rows": 4096, "columns": 3328, "frames": 1,
  "space": { "orientation": "stored" },
  "format": "dicom",
  "pixel_digest": [null],
  "frame_source": null,
  "external_id": null
}
```

- `path` is **relative to a root** when one is known (the hub's campaign root,
  or the common prefix of dcmview's CLI inputs), and absolute otherwise. The
  absolute path is kept separately in dcmview's session state, never in
  exported model data by default (it leaks usernames and directory
  structure).
- `digest` is optional and filled when known (always for rasters, on
  collisions for DICOM, and by the hub if an admin opts into full hashing).
- `rows`, `columns`, `frames` are a **guard**: any match that disagrees on them
  is rejected with a report, which catches a re-exported or resized file
  wearing an old path or UID.

**Resolution order for imports and reloads** (configurable per
import, reported per row): `key` → `sop_instance_uid` → `digest` → relative
path → absolute normalized path with today's canonical alias. The EMBED
adapter keeps its path matching as the default (parity) and can opt into UID
matching.

**Contradictory evidence is a refusal (amended 2026-09-30).** Whichever
matcher finds a candidate, the resolver then checks **every strong identifier
both sides already have** (`sop_instance_uid`, `digest`, per-frame
`pixel_digest`, and the dimension guard) before accepting it. A digest or
pixel-digest conflict is a **refusal**, reported per row, not a match: a
reload that finds a file by key or UID but whose recorded digest disagrees
does not attach the records. Missing evidence is not a conflict (a DICOM
`FileRef` with `digest: null` is checked on UID and dimensions only), and the
check never forces a new hash. A size difference (size is already `stat`ed)
is reported as a warning, since header-only edits change it too.

### 1.4 Duplicates: one image or two?

Byte-identical copies share a key, so annotations drawn on one appear on the
other. The argument for: an annotation is about image content; annotating the
same image twice under two paths would silently double-count it. A hub treats
aliases as one item.

The cost: today, two paths with identical bytes get independent annotation
sets and independent CSV rows. Under sharing, an EMBED CSV that gives the two
paths *different* ROIs has to pick one. Recommendation: load the first row,
report the conflict, keep exporting one row per path (so the output shape is
unchanged). This is a narrow behaviour change that needs the owner's sign-off;
the alternative is keying dcmview's store by path and only collapsing in the
hub, which keeps parity exactly but means standalone and hub disagree on what
"a file" is.

Symlinks and hard links are already one file today (canonical alias) and stay
one key.

### 1.5 Frames, stacks and sub-file identity

- The **unit of identity is the file** (one SOP instance or one raster).
  Frames are zero-based indices within it, as today. A multi-page TIFF's frames
  are its pages in IFD order, limited to pages that match page 0 and are not
  reduced-resolution or mask pages (the image formats doc, confirmed by the
  owner 2026-09-30).
- Series and stacks (`series.rs` `SeriesStack`, `FrameRef`) are **navigation**,
  not identity. A classic CT series is many files with one frame each; an
  annotation on "slice 40" is on that slice's file, frame 0. The viewer maps
  a stack position back to `(file, frame)` through `FrameRef`, which it already
  carries.
- Concatenations: frame indices are per instance, not per concatenation.
- WSI: see 3.4.

### 1.6 Raster identity (reconciled with the image formats doc)

The image formats doc's requirements (`image-formats.md` section 10) and how
this model meets them:

- **Key**: `b3:<blake3 of file bytes>`, as in 1.3. Discovery stays header-only
  and never hashes. The key is computed **lazily**: while decoding for display
  (the bytes are being read anyway), on demand before the first annotation
  write, or for export/import matching of only the matched files. The hub hashes
  once at campaign setup and **passes every key in the file list** (mandatory
  in hub mode, 1.3); the spoke uses them for addressing and checks each one
  when it next hashes that file. A mismatch (file changed since setup) makes
  the spoke **refuse that file** and report it, never re-key it. (Amended
  2026-09-30: this doc previously claimed the hub-passed key satisfied
  the earlier rule that keys are "computable by the spoke from the file alone".
  It does not, and that rule is reworded in `seams.md` 6 and 12: keys are
  hub-authoritative in hub mode and session-scoped in standalone.)
- **File hash versus pixel hash**: the file hash is the key, because the hub
  can compute it with I/O alone, while a pixel hash needs a full decode of
  every file at setup. The **pixel digest is recorded as evidence** in
  `FileRef.pixel_digest` whenever dcmview decodes a frame, which is free at
  that point. **Per-frame, not per-file** (amended 2026-09-30): the whole-file
  digest (`digest`, the key's hash) covers the stored bytes, while pixel
  digests are a list indexed by frame, `px:<blake3>` over a canonical layout
  (a header of rows, columns, samples per pixel, bits allocated, sample format
  and planar configuration as little-endian `u32`s, then the decoded samples
  row-major, interleaved, little-endian), `null` for frames not yet decoded. A
  multi-page file never gets a "first decoded frame" digest standing in for
  the whole file. The resolver order gains a pixel-digest step after the file
  digest, comparing frames both sides have, so annotations still find an image
  whose EXIF was stripped or which was re-saved losslessly. A lossy re-save
  (JPEG to JPEG) changes pixels too and falls back to path matching, reported.
- **Moves and duplicates**: moved files keep their key; byte-identical copies
  (`train/` and `all/`) share one key and one annotation set, per 1.4.
- **Format**: `FileRef.format` records the detected format (`png`, `jpeg`,
  `tiff`, and so on), detected by content. The key does not depend on the name
  or extension, so a JPEG named `.png` still matches.
- **Multi-page TIFF frames**: frames stay dense zero-based indices in
  annotations (so `FrameScope` and every tool work the same as DICOM), and
  `FileRef.frame_source` records the IFD index of each frame, for example
  `[0, 2, 4]` when reduced-resolution pages are interleaved. On load, if the
  page rule has changed and a file's IFD list differs from the recorded one,
  annotations are remapped by IFD index, not by position, and anything that no
  longer maps is reported. Single-image formats have `frame_source: [0]`.
- **Same image as DICOM and PNG**: no automatic link. `FileRef.external_id`
  (optional, from a hub file-list column) lets an adapter or the hub relate the
  two, but they stay separate keys, because their pixel grids can differ
  (windowing, bit depth, crops).
- **Label targets**: rasters get `file`, `frame` and `folder` (4.3 already has
  a folder level), plus clinical levels only if a file-list mapping supplies
  them.
- **EMBED**: path matching works for rasters unchanged; the `anon_dicom_path`
  name stays for parity, and an `embed-extended` mode may also accept a `path`
  column (9.4, the adapters doc).

### 1.7 Startup and discovery cost (the owner asked for this, 2026-09-29)

Goal: no regression in time to first file, time to `scan_complete`, or
first-frame render for standard use. What discovery does today
(`src/loader/discovery.rs`, `src/loader/entry.rs`): a rayon `par_iter` over
candidates, each opening the file, `stat`ing it, and reading the header only
(`read_until` pixel data), then streaming `Selected` events into the
progressive registry.

What the identity design adds, per case:

| Case | Added work during discovery | Estimate at 100k files |
|---|---|---|
| DICOM, unique SOP UID (the normal case) | one hash-map insert of the UID already read; file size already comes from the `stat` in `read_discovery_header` | a few ms in total, spread over the scan; no extra I/O |
| DICOM, duplicate UID | detected by the same insert; **no hashing in discovery** | same |
| DICOM, no UID | key left pending, like a raster | nothing |
| Raster | key left pending; discovery stays header-only | nothing |
| `/api/files` payload | `file_key` serialized **only when it is not `sop:` + the `sop_instance_uid` already in `FileSummary`**; `alias_of` only when set | about zero bytes for normal DICOM sets |

So discovery itself adds no I/O. The costs that do exist move off the startup
path and become on-demand:

- **Duplicate UIDs.** Same UID and same size are a *provisional alias* (shared
  key). Different sizes mean different bytes, so those files are a *collision*
  and get pending `b3:` keys. Verifying a provisional alias byte for byte (a
  full read of both files) runs **only when it matters**: before the first
  annotation write on either file, before an export that includes them, or at
  hub setup. There is no background hashing by default, because on network
  filesystems background reads compete with first-frame renders. This matters
  for datasets with a `train/` + `all/` style copy of every file: eager
  verification would make discovery read the whole dataset twice.
- **Pending keys.** A file with a pending key views normally. Its key is
  computed after its first frame is sent, as **background I/O with a visible
  pending state** (amended 2026-09-30). For a single-image raster the
  whole file was just read for the decode, so this is usually a CPU pass over
  bytes still in the page cache. For multi-page TIFF and no-UID multiframe
  DICOM it is not: those decode one frame at a time, so the whole-file hash
  is an extra full read. It runs at background priority (it must not delay
  frame requests), the UI shows the file's key as pending, and the first
  annotation write on that file waits for it with a visible "preparing file"
  state. The "well under 5% of a decode" figure is an unmeasured estimate for
  the single-image case only; the performance checks below measure it.
- **Progressive discovery and rekeying.** Discovery is parallel and
  progressive, so a UID collision can be found after the first file is already
  shown, or even annotated. **In standalone, rekeying is one atomic event**
  (amended 2026-09-30): the server swaps the key of the file's in-memory
  records (and the sidecar's, when one is in use) and bumps the catalog
  revision; the client applies the change in one step to its records, its op
  queue entries and dirty state, the undo tree and the selection, so nothing
  queued or undoable keeps the old key. Ops that were in flight under the old
  key are accepted as addressed to the new one for the rest of the session
  (the server keeps an old-to-new map). The UI reports the rekey. Records on
  a provisional alias that turns out to be a collision stay with the file
  they were drawn on. Hub spokes never rekey, because the hub fixes keys at
  setup and passes them in the file list (1.3).
- **Imports.** EMBED path matching is unchanged. UID matching (opt-in) uses the
  registry map. Digest matching hashes only rows that are still unmatched, and
  only when the user asked for it.
- **Exports.** DICOM `FileRef`s export with `digest: null` unless one is already
  known, so export never forces a full-dataset hash. Any raster with
  annotations was decoded, so it already has its key.
- **Hub setup** is where full hashing happens when needed (rasters, UID
  collisions, or an admin who opts into hashing everything): a one-off step with
  progress, not something standalone users pay for.

How to verify when this is built: add discovery timings (time to first
`Selected`, time to `scan_complete`) and first-frame latency to the existing
performance checks. Run them on the stress corpus profiles (`stress/study`,
`stress/enhanced-ct`) before and after, and on a synthetic set with a
duplicated directory. Treat any measurable regression in the unique-UID case as
a bug.

### 1.8 What the API exposes

- `FileSummary` gains `file_key: Option<String>`, omitted when it is simply
  `sop:<sop_instance_uid>` (the normal case) and `null` while a key is pending,
  plus `alias_of: Option<usize>` (see 1.7).
- **Key changes reach the client** (amended 2026-09-30). Entries are no longer
  append-only once keys resolve or rekey, so the catalog's `?since=<count>`
  becomes `?since=<revision>`: the catalog has a monotonically increasing
  revision and returns entries **inserted or updated** since it (a resolved
  pending key, an alias or rekey change), plus `reset: true` when the client
  must refetch everything. The client keeps polling at a slow rate after
  `scan_complete` while any key is pending. And **frame responses carry
  `X-File-Key: <key>` once the file's pending key has resolved** (the first
  such response is what the viewer normally sees first), so the viewer can
  address ops for the file it is looking at without waiting for a poll.
- dcmview's own routes keep `/api/file/{index}/...` (index is fine within one
  process), but every annotation payload carries `file_key`, and anything that
  leaves the process (export, hub sync) uses keys only.
- New annotations cannot be created on a file whose key is still `None`; the
  store computes it on demand first. For a single-image raster this has
  usually finished before a user draws; for multi-page TIFF and no-UID
  multiframe DICOM it can take a full read, and the tool shows the pending
  state until it resolves (1.7).

---

## 2. Coordinate conventions

### 2.1 Origin and units

Options: (a) corner origin, continuous (pixel centre at `c + 0.5`); (b) centre
origin (pixel centre at integer `c`, used by ITK, NIfTI, numpy/scikit-image);
(c) physical millimetres in the patient or slide frame.

**Recommend (a)**, because it is what the viewer, the EMBED edge convention
and DICOM SCOORD already use, and a rectangle over whole pixels has integer
corners. (b) is one `± 0.5` in the adapters that need it (NIfTI, some COCO
tooling). (c) needs geometry that rasters and many projection images do not
have; millimetre values are derived at export from `PixelSpacing`/
`ImagerPixelSpacing` when a format wants them, never stored.

- `x` = column, `y` = row, `f64` in memory.
- **Quantize on commit to 1/1000 px** and serialize with at most three
  decimals, so equality, diffs, and exports are deterministic.
- Valid range is `[0, columns] × [0, rows]`. Tools clamp; the store rejects
  out-of-range geometry on **writes** (ops), like `canonicalize_annotations`
  does on `PUT` now. **Imports differ by format** (amended 2026-09-30):
  today's CSV path does not bounds-check at all, so the **EMBED CSV
  import is lenient**: out-of-range and zero-area rows load exactly as they do
  in 0.3 and are **listed in the import report** (not rejected, and they do
  not trigger the all-or-nothing failure). The new native formats
  (`dcmview.annotations` JSON/JSONL) validate strictly. A record
  loaded this way stays as loaded until edited; the first edit clamps it.
- **Snapping is a tool property, not a model property.** The rect tool keeps
  today's snap-to-pixel-edge default, which keeps EMBED round-trips exact;
  the point tool defaults to snapping to pixel centres. Both can be turned off
  in the annotation config (the tools doc owns the UX).
- **Pixel aspect ratio** stays a display concern. Coordinates are in pixel
  indices even when pixels are not square (the viewer already stretches by
  `pixelAspectRatio` and inverts it).

### 2.2 Which pixel space

- **DICOM**: the stored pixel matrix. View flip and rotation are never baked
  into annotations (already true).
- **Rasters** (revised after the image formats doc's requirements, see 1.6):
  the **stored pixel grid**, same as DICOM. The EXIF or TIFF orientation value
  (1 to 8) is applied only as the **initial view transform**, using the flip
  and rotate view state the viewer already has, so photos still appear
  upright. `FileRef.space` records `{ "orientation": "stored",
  "exif_orientation": 6 }` so an adapter can derive "as displayed" coordinates
  when a consumer wants them. Reasons for switching from my first draft
  (EXIF-oriented space): it makes DICOM and rasters follow one rule; Pillow
  and torchvision ignore EXIF orientation by default, so stored-grid
  coordinates line up with most ML loaders without conversion; and it costs
  the user nothing visually. OpenCV applies EXIF by default, so that adapter
  path converts.
- ICC profiles and colour management do not affect coordinates.

### 2.3 Frames

Every geometric annotation has a `frames` scope:

```
FrameScope = "all" | { "set": [u32, ...] }   // sorted, unique, < frame_count
```

- `all` is a real value, not a missing list (fixes parity finding 1 and 2).
- A set with several frames means "the same 2D shape on each of these
  frames", which is EMBED's DBT use (a lesion visible over slices 12 to 18).
  It is not a 3D shape.
- Single-frame files use `all`.
- Genuinely 3D annotations (a box over a CT series, a tracked contour that
  changes per slice) are **out of scope for v1**. The forward path is a
  `group_id` linking per-frame annotations, not a 3D geometry type, so v1 data
  stays valid.

---

## 3. Geometry types

### 3.1 Vector shapes

| Type | Data | Notes |
|---|---|---|
| `point` | `{x, y}` | |
| `line` | `{points: [p0, p1]}` | measurement length derived at display/export |
| `polyline` | `{points: [...≥2]}` | open |
| `polygon` | `{points: [...≥3]}` | implicitly closed, no self-intersection check in v1 |
| `rect` | `{x0, y0, x1, y1}` | axis-aligned, `x0 < x1`, `y0 < y1`. The model is x-first; **EMBED is y-first, `[ymin, xmin, ymax, xmax]`**, and COCO is `[x, y, w, h]`, so both adapters reorder (9.2) |
| `ellipse` | `{cx, cy, rx, ry, angle}` | `angle` in degrees, 0 in v1 tools, present so rotated ellipses need no model bump |
| `mask` | see 3.2 | |

Why keep `rect` and `ellipse` rather than storing polygons: EMBED and COCO
need the exact rectangle; ellipse area and DICOM SCOORD `ELLIPSE` export need
the parameters; editing handles differ. Rotated rectangles are a polygon.
Circles are ellipses with `rx == ry`. A freehand tool produces a polygon or
polyline (with simplification, the tools doc).

### 3.2 Masks

This is the decision with the most downstream weight (tools, undo memory,
adapters, ML outputs).

Options:

1. **Dense label map per (file, frame, layer)**: one `u8`/`u16` raster where
   the value is a class. Natural for brush painting with a palette and for
   DICOM SEG `LABELMAP`, NIfTI and PNG exports. Costs: a 4096×3328 mammogram is
   13.6 MB per frame at `u8`, a 60-slice DBT is 800 MB dense, and it cannot
   express overlapping objects of the same class (two touching masses become
   one blob).
2. **Per-object binary mask** (COCO instance style): each mask is an annotation
   with its own class and attributes. Overlap is natural, matches DICOM SEG
   `BINARY` segments, and each object can carry labels and authorship like any
   shape. Costs: exporting a label map needs a declared precedence for
   overlaps.
3. **Polygon-only** ("masks" are polygons rasterized on export). Cheap, but
   brush painting, holes and erasing do not map onto it.

Storage options for 1 or 2: dense bitmap, whole-image RLE, or **sparse tiles**
(only tiles with any set pixel are stored, each compressed).

**Recommend 2 with sparse tiles, plus an "exclusive" layer flag for label-map
semantics.**

- A `mask` annotation is one segment: `{ encoding: "tiles-v1", tile: 64, depth: 1, frames: {<frame>: {<"tx,ty">: <base64 deflate(bitpacked 64×64)>}} }`.
  A 64×64 binary tile is 512 bytes uncompressed and usually tens of bytes
  deflated. A lesion mask on a 4k mammogram touches a few dozen tiles.
- Frames that contain tiles are the mask's frame set (a DBT lesion painted on
  six slices is one segment with six frames of tiles), so `frames` is derived,
  not separately edited.
- A layer can be **exclusive**: painting a segment clears those pixels from
  every other segment in the layer. That gives label-map behaviour for users
  who want it and makes a lossless label-map export possible (no overlaps by
  construction). Non-exclusive layers allow overlap and export label maps by
  layer order with a reported overlap count.
- The same 64-pixel tile is the unit of undo deltas (section 7) and of syncing,
  so a brush stroke is a small op.
- `depth: 8` (a `u8` probability per pixel) is **reserved** for model outputs
  (DICOM SEG `FRACTIONAL`, which the viewer already displays). v1 tools only
  create `depth: 1`.
- Tile size 64 is a recommendation, not load-bearing: it is in the encoding
  name so it can change without breaking stored data.

### 3.3 Where the extra memory goes

For planning, an annotator with a full mammography set: vector annotations are
a few hundred bytes each. Masks at 64-pixel tiles: a 2 cm mass at 0.07 mm/px is
about 285 px across, so roughly 25 tiles, a few KB compressed. Masks are
cheap in the model; they are expensive only if stored dense, which this
avoids.

### 3.4 Whole-slide images

WSI frames are tiles of a pyramid level. An annotation in frame space would
break at every tile edge. If WSI annotation is wanted, geometry must live in
the **total pixel matrix** of the base (highest resolution) level with
`space: {"kind": "wsi_total_pixel_matrix", "pyramid_uid": ...}`, keyed to the
base level's file key. **Recommend: WSI annotation out of scope for v1**, with
the `space` field reserved so it needs no model version bump. Needs the owner's
confirmation that nobody on the roadmap needs it soon.

---

## 4. Classes, attributes and hierarchy labels

### 4.1 The model most annotation tools converge on

Tools like CVAT and Label Studio separate:

- a **class** (what the shape is: "mass", "calcification", "clip"), chosen
  from a list, with a colour and the geometry types it allows;
- **attributes** on that shape (BI-RADS descriptor, "confidence", a
  free-text note);
- **labels on non-shape targets** (this series is "motion-degraded", this study
  is BI-RADS 4).

The alternative is one generic "field" system where the class is just another
field. It is more uniform, but it makes "draw a mass" a two-step action
(draw, then set a field), and colours and tool restrictions end up bolted on.
**Recommend the class plus attributes model**, with attributes and hierarchy
labels sharing one field type system.

### 4.2 Label schema

The schema is campaign configuration (in hub mode the hub supplies it;
standalone dcmview gets it from `--annotation-config`, `seams.md` 7). Shape:

```json
{
  "schema_id": "biopsy-clips", "schema_version": 3,
  "classes": [
    { "id": "clip", "name": "Biopsy clip", "color": "#E4572E",
      "geometry": ["point", "rect"], "attributes": ["clip_shape"],
      "code": { "scheme": "SCT", "value": "...", "meaning": "..." } }
  ],
  "fields": [
    { "id": "clip_shape", "name": "Clip shape", "type": "category",
      "options": [{ "id": "ribbon", "name": "Ribbon" }, { "id": "coil", "name": "Coil" }],
      "required": false },
    { "id": "image_quality", "name": "Image quality", "type": "category",
      "applies_to": ["series", "file"], "options": [...], "required": true },
    { "id": "density", "name": "Density", "type": "number",
      "min": 0, "max": 100, "step": 1, "unit": "%", "applies_to": ["study"] }
  ]
}
```

- Field types: `category`, `multi_category`, `boolean`, `number` (min, max,
  step, unit, integer flag), `text` (max length). Dates and free-form JSON left
  out on purpose.
- **Ids are stable; names are display text.** Renaming a class or option never
  touches data. Removing an option marks it `deprecated` so old values still
  render.
- Optional `code` (coding scheme, value, meaning) on classes and options is
  what DICOM SEG and SR exports need, and costs nothing if unused.
- `required` is advisory in the viewer (it drives "incomplete" markers and the
  campaign "done" check), never a save blocker, because blocking saves loses
  work.
- **No schema is valid**: standalone dcmview with no config gets one implicit
  class `roi` allowing every geometry, which is today's behaviour.

### 4.3 Label targets

```
LabelTarget =
  | { "patient": "<PatientID>" }
  | { "study": "<StudyInstanceUID>" }
  | { "series": "<SeriesInstanceUID>" }
  | { "file": "<FileKey>" }
  | { "frame": "<FileKey>", "index": 12 }
  | { "folder": "<relative path>", "root": "<root id>" }
```

**Namespacing and fallbacks (amended 2026-09-30).**

- A `folder` target carries the **stable root id** it is relative to (supplied
  by the hub in hub mode; in standalone, the index of the CLI input root,
  recorded in the document), so `root0/case1` and `root1/case1` are different
  targets.
- A file with a **missing hierarchy id** (no PatientID, StudyInstanceUID or
  SeriesInstanceUID) gets a **non-collapsing fallback** at that level instead
  of sharing an empty value with every other such file:
  `{ "study": "missing:<FileKey>" }` (likewise for `patient` and `series`), so
  two id-less files never become one labellable study. The gallery tree uses
  the same fallback ids in place of today's file-index fallback.
- **Canonical target id.** Every target has one kind-prefixed string form,
  used as the op queue key for non-file targets (7.2): `patient:<id>`,
  `study:<uid>`, `series:<uid>`, `folder:<root>/<path>`. `file` and `frame`
  targets use the file key; a frame target adds its index.

- Attribute values on a shape are stored **on the annotation**, not as a
  `LabelTarget`, because they share its lifecycle (delete the shape, lose its
  attributes, one undo step).
- `patient` uses PatientID only. It is not globally unique (two sites can both
  have `12345`), but campaigns are single-source in practice. Flag, not fix.
- Rasters have no patient, study or series, so their valid targets are `file`,
  `frame` and `folder`. The image formats doc decides where rasters appear in
  the clinical tree; they are not labellable at clinical levels unless it
  gives them one.
- `folder` targets exist so directory-mode users (and raster datasets) can
  label a folder from the gallery (the gallery doc).
- A label record: `{ id, target, field, value, layer, author, created_at, rev, score? }`.
  One value per `(target, field, layer, author)`, where a frame target's
  index is part of the target. `SetLabel` addresses the record by `id` and
  `base_rev` (7.2).

---

## 5. Layers

A layer groups annotations and labels. Kept to what the roadmap asks for.

```json
{ "id": "L-01J...", "name": "Alice", "kind": "user", "exclusive_masks": false,
  "color": null, "readonly": false, "source": { "author": "user:alice" } }
```

- `kind`: `user` (someone's working layer), `import` (loaded from a file),
  `model` (pre-labels or inference output), `review` (another annotator's
  work shown to an admin, always readonly).
- **Visible, locked and order are view state, not data.** They are per user
  and per session, so they never sync or export.
- Every annotation and label belongs to exactly one layer. Standalone dcmview
  starts with one layer, "Annotations", plus one import layer per
  `--annotations` file (or, for EMBED parity, the import can go straight into
  the default layer; recommend the default layer so today's single-set UX is
  unchanged).
- Moving an annotation between layers is an update op.
- Layers are **session-wide**, not per image (like layers across pages of a
  document), so "hide the model layer" hides it everywhere.

---

## 6. Authorship, provenance and versioning

### 6.1 Per-record metadata

Every annotation and label carries:

```json
{ "id": "01J9Z3...", "rev": 4,
  "created_by": "user:alice", "created_at": "2026-09-29T21:04:11.120Z",
  "modified_by": "user:alice", "modified_at": "2026-09-29T21:06:02.004Z",
  "derived_from": null, "score": null }
```

- **Ids are UUIDv7**, generated by the client that creates the record. Time
  ordered, unique across users and spokes, so records from several annotators
  merge with no renumbering, and the frontend can create a record and its undo
  entry before the server answers. Today's array-index addressing goes away
  (the EMBED endpoints keep indices as a view, section 9).
- **Op ids are UUIDv7 too** (`OpEnvelope.op_id`, 7.2), everywhere: the earlier
  "UUIDv4 or v7" is narrowed to v7 (amended 2026-09-30). `uuid` becomes a
  runtime dependency (section 0).
- **Author strings**: `user:<unix username>`, `model:<name>@<version>`,
  `import:<adapter>` (for example `import:embed`). Standalone dcmview uses
  `user:$USER` (recommend) or a fixed `user:local` if the owner prefers not to
  record usernames in exports.
- **In hub mode, the hub stamps authorship.** A spoke's own stamp is
  advisory, so a spoke cannot write as someone else (`seams.md` 10). Imports
  run by the hub itself are authored `import:<adapter>` (from
  `ImportContext`, 8.1) (amended 2026-09-30).
- Timestamps come from the dcmview server clock, not the browser.
- `rev` increments on every change; it is what syncing uses for idempotency and
  conflict checks (section 7).
- `derived_from` links an accepted pre-label or a copied annotation to its
  source id.
- `score` (0 to 1) is for model outputs. Human records leave it null.

### 6.2 Model versioning

- Documents carry `"format": "dcmview.annotations", "version": "1.0"`.
  **Same major reads fine; unknown fields are preserved on round-trip
  (not dropped) and ignored for display.** A major bump needs a migration
  function in the crate.
- An `extensions` object on documents, annotations and labels holds
  adapter-specific data (for example EMBED extra columns, if we ever preserve
  them), namespaced by adapter id.
- The label schema has its own `schema_id` and `schema_version`, independent
  of the model version.

### 6.3 Where the types live

A small crate (working name `dcmview-annotations`) with serde types, validation
and adapters, and **no dependency on dicom-rs, axum or dcmview internals**, so
the hub and a future ML package can depend on it cheaply. The repo already
generates TypeScript with `ts-rs`; the same derive covers these types, and a
generated JSON Schema (`schemars`) serves Python and ML consumers. How the
crate is shared between repos is in `seams.md` 13.

### 6.4 Document shape

```json
{
  "format": "dcmview.annotations", "version": "1.0",
  "schema": { "...label schema..." },
  "files": [ { "...FileRef..." } ],
  "layers": [ { "...Layer..." } ],
  "annotations": [ {
      "id": "01J9Z3...", "file": "sop:1.2.840...28", "frames": "all",
      "layer": "L-01J...", "class": "clip",
      "geometry": { "type": "rect", "x0": 340, "y0": 120, "x1": 430, "y1": 220 },
      "attributes": { "clip_shape": "ribbon" },
      "created_by": "user:alice", "...": "..." } ],
  "labels": [ { "id": "...", "target": { "series": "1.2..." },
                "field": "image_quality", "value": "good", "layer": "L-01J...", "...": "..." } ]
}
```

JSON for documents; the same records one per line (JSONL) for streaming.

---

## 7. The operation model (undo and syncing)

### 7.1 Why operations and not whole-file saves

Today a save replaces a file's whole ROI list. That cannot express branching
undo without snapshots, costs a full mask per brush stroke, and makes two tabs
of the same user clobber each other silently. Operations fix all three.

### 7.2 Operations

```
Op =
  | CreateAnnotation  { annotation }
  | UpdateAnnotation  { id, base_rev, before: Patch, after: Patch }
  | DeleteAnnotation  { id, base_rev, snapshot }
  | RestoreAnnotation { id, base_rev, snapshot }   // undo of a delete
  | MaskTiles         { id, base_rev, frame, tiles: [{ tx, ty, before, after }] }
  | SetLabel          { id, base_rev: Option<u64>, target, field, layer,
                        before: Option<Value>, after: Option<Value> }
  | CreateLayer       { layer }
  | UpdateLayer       { id, base_rev, before: LayerPatch, after: LayerPatch }
  | DeleteLayer       { id, base_rev, snapshot }
  | Batch             { ops: [Op] }   // one user gesture, one undo step, atomic
```

Envelope: `{ op_id (UUIDv7), actor, ts, op }`.

**Every op names a versioned target** (amended 2026-09-30):

- **`SetLabel`** carries the label record's `id` (UUIDv7, client-created on
  first set) and `base_rev` (`None` only when creating the record). Setting
  `after: None` clears the value; the record is kept as a tombstone with its
  `rev`, so a retried or stale `SetLabel` fails its `base_rev` check instead
  of double-applying (this also closes the sidecar replay gap).
- **Layer ops** carry the layer `id` and `base_rev`; layers get a `rev` like
  records. `LayerPatch` covers `name`, `color`, `exclusive_masks`,
  `readonly`. `DeleteLayer` is only valid on a layer with no live records;
  deleting a layer with content is a `Batch` of the record deletes plus the
  `DeleteLayer`, so its undo restores everything in one step.
- **Delete and restore.** A deleted annotation becomes a tombstone that keeps
  its `id` and `rev` (bumped by the delete); it is excluded from snapshots and
  exports. Its inverse is `RestoreAnnotation`, which recreates the record
  under the **same id** from `snapshot`, with `base_rev` = the tombstone's
  rev. A `CreateAnnotation` with an id that exists, live or tombstoned, is
  `Invalid`, never an upsert.

**Queue key** (what the client orders and tracks an op by, amended
2026-09-30): the **file key** for annotation ops and for `SetLabel` on `file`
and `frame` targets; the **canonical target id** (4.3) for `SetLabel` on
`patient`, `study`, `series` and `folder` targets; the **layer id** for layer
ops. A study label edited from file A and from file B therefore shares one
key. The client sends ops through **one global ordered queue**: ops keep
their enqueue order across keys, and per-key state (dirty, pending, retry)
is tracked by queue key. `seams.md` 9 owns the transport.

**`Batch` is atomic** (confirmed 2026-09-30, answering tools 7.3 and
15): all of its ops apply or none do, it returns **one** result, and it goes
through the global queue as one unit, ordered after every earlier op on any
key it touches.

- Every op carries its **before state**, so its inverse is computable without
  reading history: vector patches are tiny; mask ops carry before and after
  tiles only for tiles the stroke touched.
- `Patch` covers `geometry`, `class`, `attributes`, `frames`, `layer`.
- **Undo is not an op type.** Undoing produces the inverse as an ordinary new
  op. The store, the hub and the sync path only ever see forward ops. The
  branching undo tree (the tools doc) is a client-side structure over ops, never
  persisted and never synced, which keeps dcmview ephemeral and the hub
  simple.
- `op_id` makes applying an op idempotent (a retry after a lost response is a
  no-op).
- `base_rev` gives optimistic concurrency: if the record moved on (the same
  user in a second tab, or an admin edit), the op is rejected and the client
  refetches that record and drops or rebases its redo stack. Across users
  there are no conflicts at all, because each annotator writes only to their
  own layer (blind reading).
- A `MaskTiles` op on an exclusive layer includes the tiles it cleared from
  other segments, so undo restores them.

### 7.3 Storage and memory

- The store keeps **current state** (records by id, indexed by file key and,
  for labels, by canonical target id; tombstones kept for `rev` checks).
  dcmview keeps no op log beyond what the frontend undo tree holds.
- Undo memory is dominated by mask tiles: a 30-pixel brush stroke across a 4k
  image touches about 70 tiles, a few KB compressed with before and after. A
  per-image byte cap (the tools doc picks the number) prunes the oldest
  off-path branches first.

### 7.4 What the backend trait looks like (for the integration seams)

```rust
trait AnnotationBackend {
    fn snapshot(&self, files: &[FileKey]) -> Result<Snapshot>;   // current records
    fn apply(&self, ops: Vec<OpEnvelope>) -> Vec<ApplyResult>;    // one result per envelope
    fn export(&self) -> Result<Document>;
}
// ApplyResult = Ok { revs: [(record or layer id, new_rev)] } | Conflict(current) | Invalid(reason)
```

Amended 2026-09-30: `apply` returns **one result per envelope**, and a `Batch`
is one envelope, so it gets one result: `Ok` with **every affected revision**,
or the first `Conflict`/`Invalid` with nothing applied. Each envelope is **one
transaction** covering validation, the state change, the audit record (where
the store keeps one) and the stored dedup result for its `op_id`, so a retry
of an applied op returns the same result. The `file:` sidecar meets the same
rule through its snapshot (`seams.md` 8: writer lock, persisted op outcomes,
directory fsync, group commit before acks).

The in-memory store is the default implementation; the hub-forwarding store is
another. Push or pull, batching and retry are in `seams.md` 9.

---

## 8. Import and export adapters

### 8.1 Interface

```rust
trait ExportAdapter {
    fn info(&self) -> AdapterInfo;   // id, name, file extension, capabilities
    fn export(&self, doc: &Document, ctx: &ExportContext, out: &mut dyn Write) -> Result<ExportReport>;
}
trait ImportAdapter {
    fn info(&self) -> AdapterInfo;
    fn sniff(&self, head: &[u8]) -> Confidence;
    fn import(&self, input: &mut dyn Read, ctx: &ImportContext) -> Result<(DocumentFragment, ImportReport)>;
}
```

- **`AdapterInfo.capabilities`** declares geometry types, masks (binary,
  fractional), hierarchy labels, attributes, multi-frame, and whether output is
  one file or a directory. The UI greys out or warns before export, not after.
- **`ExportContext`** gives what formats need beyond the model: absolute and
  relative paths, pixel spacing, dimensions, UIDs, and (for DICOM SEG) a way to
  read the source dataset's header. The adapter crate defines the trait; dcmview
  and the hub supply the context from their own file registries.
- **`ImportContext`** provides the file resolver (section 1.3) and the author
  and layer to import into.
- **Reports are mandatory and never silent**: rows matched and unmatched,
  shapes converted (ellipse to bounding box), shapes dropped, rounding applied,
  overlaps resolved. dcmview shows it after export and import.
- Adapters run in dcmview (standalone export and `--annotations`) and in the
  hub (campaign export), which is why they live in the shared crate.
- Templates (the adapters doc) are one `ExportAdapter` implementation over the
  same `Document`.
- Coordinate conversions for centre-origin formats (NIfTI, some COCO
  consumers) are the adapter's job; the model's convention is fixed.

### 8.2 ML fit (kept in mind, not designed)

A model adapter reads files by key and writes a `DocumentFragment` into a
`model` layer with `created_by: model:<name>@<version>` and `score` set.
Classification outputs are labels with scores on `file` or `series` targets.
Segmentation outputs are masks, `depth: 8` for probabilities. Accepting a
pre-label copies it into the user's layer with `derived_from`. None of this
needs model changes later.

---

## 9. EMBED parity and the extended schema

### 9.1 What parity covers (from AGENTS.md and `docs/annotations.md`)

Unchanged, owned by the EMBED adapter plus a thin compatibility layer:

- `--annotations PATH` and Python `annotations=`: header validated at startup,
  background streaming ingestion after discovery, unmatched rows ignored
  without parsing, a duplicate matching path is an error, an invalid matching
  row fails the whole import (all or nothing) while viewing continues, edits
  made while loading are never overwritten.
- **Import report** (amended 2026-09-30): "invalid" keeps today's
  meaning (shape, `num_ROI` and frame-range checks). Out-of-range and
  zero-area rects, which 0.3 does not check, load as they do today and are
  **listed in the import report** with path and ROI index; they do not fail
  the import (2.1). Shipped `docs/annotations.md` overstates today's checks;
  correcting it is a separate docs fix.
- Path matching rules: normalized absolute paths, CWD-relative resolution,
  canonical alias for symlinks.
- `GET`/`PUT /api/file/{index}/annotations` with the `EmbedRoiAnnotations`
  shape, and `/api/annotations/export.csv` with columns
  `anon_dicom_path,num_ROI,ROI_coords,ROI_frames`.
- Extra columns ignored on load and not preserved.

### 9.2 Mapping

- Each ROI becomes a `rect` annotation, class `roi`, in the default layer,
  author `import:embed`: `[ymin, xmin, ymax, xmax]` → `{x0: xmin, y0: ymin, x1: xmax, y1: ymax}`.
  Integer edges map exactly, so load then export is lossless.
- `ROI_frames` → `FrameScope`, with the parity findings resolved as in
  section 0.
- ROI order is preserved by creation order, so the `PUT` endpoint's index
  view stays stable. `PUT` becomes "diff against the current rect set and emit
  ops", so the old frontend and the Python/VS Code consumers keep working
  during the transition.
- Export writes one row per loaded file path (including aliases, section 1.4)
  with at least one exportable rect. **Rows are sorted by path** (byte order of
  the UTF-8 path string as recorded) (amended 2026-09-30): today's
  order follows the parallel discovery's completion order and varies between
  runs, so there is no parity to keep; this is a sign-off under AGENTS.md.

### 9.3 What EMBED cannot express

Non-rect shapes, attributes, classes, labels and masks. Export policy is an
adapter option with a report either way:

- **`skip` (recommended default)**: non-rects are left out and counted in the
  report.
- `bbox`: non-rects are exported as their bounding box (rounded outward to
  integer edges). Useful, but a polygon silently becoming a box in a CSV that
  looks classic is a quiet data change, so not the default.

Non-integer rect coordinates (possible if snapping is off) round **outward**
(floor the minimum, ceil the maximum) so the box never shrinks.

### 9.4 The EMBED ROI schema extension

The owner mentioned planning one; its shape is not in the repos or the
roadmap. What would make it cheap for the adapter: keep `ROI_coords` holding a
bounding box for **every** ROI, so an extended CSV is still a valid classic
EMBED CSV and existing readers keep working, and add parallel JSON columns
such as `ROI_types` (`["rect","ellipse","polygon"]`), `ROI_geometry` (per ROI,
in the corner-origin convention above) and `ROI_labels` (class and attributes
per ROI). The adapter would then have two modes, `embed` (classic, parity) and
`embed-extended`, selected explicitly. Needs the owner's draft before the
adapters doc can fix it.

---

## 10. Risks and things deliberately left out

- **Hash cost for rasters in huge folders**: hashing at decode is nearly free
  for single-image rasters but a full extra read for multi-page TIFF (1.7),
  and a hub scanning 100k JPEGs at setup still reads them all once. Acceptable for a
  setup step; report progress.
- **UID-based keys change under re-anonymization.** The `FileRef` evidence and
  resolver order cover it, at the cost of a less direct match. Imports report
  which matcher hit.
- **Alias sharing** is a (small) behaviour change from today (section 1.4).
- **3D annotations, WSI annotations, fractional masks from tools, and
  time-varying contours** are out of v1 but have reserved fields.
- **Not decided here**: tool UX and undo presentation (the tools doc), template
  language (the adapters doc), transport (`seams.md`).

---

## 11. Suggested implementation order (for later, not now)

1. Golden tests that freeze today's EMBED load and export bytes over a fixture
   set, before touching anything. Amended 2026-09-30: rows compare
   **order-insensitively** until the sort by path (9.2) lands, then by bytes
   in sorted order; the fixtures **include out-of-range and zero-area rows**,
   which must load and appear in the import report.
2. The model crate: types, validation, FileKey, JSON and TS generation.
3. `FileKey` computation in discovery, `FileSummary.file_key`.
4. The in-memory store over the model with the EMBED compatibility layer; the
   existing frontend keeps working unchanged.
5. Op-based client store, then new tools (the tools doc).

---

## 12. Decisions (confirmed by the owner, 2026-09-29)

1. **File key scheme**: `sop:<uid>` for DICOM, `b3:<blake3>` for rasters and
   UID collisions, with `FileRef` evidence and the resolver order.
   **Confirmed**, on condition of no startup or discovery regression (see
   1.7). **Amended 2026-09-30 (fixes under the confirmed scheme)**: keys are
   hub-authoritative in hub mode (mandatory in hub file lists; a spoke that
   computes a different key refuses the file) and session-scoped in
   standalone, with persisted records resolved through `FileRef` evidence; a
   key-rule version; standalone rekeying is one atomic event; strong-evidence
   conflicts are refusals; key changes reach the client through a catalog
   revision cursor and `X-File-Key`; whole-file hashing is background I/O with
   a pending state, and pixel digests are per frame. The scheme itself is
   unchanged.
2. **Byte-identical duplicates share one key**: load the first conflicting CSV
   row, report the conflict, keep exporting one row per path. **Confirmed**,
   including sign-off for this narrow behaviour change.
3. **Coordinates**: corner origin, continuous, quantized to 1/1000 px, in the
   stored pixel grid for both DICOM and rasters, with EXIF/TIFF orientation
   applied only as the initial view and recorded. **Confirmed.**
4. **Masks**: per-object binary segments in sparse 64-pixel tiles, with an
   exclusive-layer flag for label-map semantics. **Confirmed.**
5. **Class plus attributes** for shapes, one field system shared with
   hierarchy labels. **Confirmed.**
6. **EMBED parity findings**: per-ROI `[]` means all frames, and export writes
   `"[]"` when every ROI is all-frames instead of expanding lists after an
   edit. **Confirmed** (sign-off for the edited-file output change).
7. **EMBED non-rect export**: `skip` with a report by default, `bbox` as an
   option. **Confirmed for now**; revisit once an EMBED-style convention for
   non-bbox annotations is decided.
8. **EMBED extension**: no draft yet. For now EMBED-style export handles bbox
   (`rect`) annotations only. The suggestion in 9.4 (keep a bbox in `ROI_coords`
   for every ROI) stays open for when a draft exists.
9. **WSI annotation out of v1**, with the space field reserved. **Confirmed.**
10. **Standalone author**: `user:$USER`. **Confirmed.** **Amended
    2026-09-30**: imports run by the hub itself are authored
    `import:<adapter>`.
11. **EMBED CSV row order** (added 2026-09-30, confirmed by the owner):
    export rows are sorted by path (9.2); goldens compare order-insensitively
    until the sort lands (11). Sign-off under AGENTS.md for a
    parity-protected output.
12. **EMBED CSV import bounds** (added 2026-09-30, confirmed by the owner):
    lenient import with a report for EMBED CSV (out-of-range and zero-area
    rows load and are listed); strict validation for the new native formats
    only (2.1, 9.1). Such rows go into the goldens.
13. **Op protocol targets** (added 2026-09-30, a review fix, no new
    decision): every op names a versioned target, queue keys are file key,
    canonical label target id or layer id, and `Batch` is atomic with one
    result through a global ordered client queue (7.2, 7.4).

---

## 13. Interfaces for downstream areas

What this doc fixes for later areas, assuming the owner confirms section 12.

**All areas**
- A file is identified by a `FileKey` string (`sop:<SOPInstanceUID>` or
  `b3:<64 hex>`), never by `FileEntry.index` outside one process. Every file
  in model data has a `FileRef` with key, kind, UIDs, relative path,
  size, optional digest, rows, columns, frames and `space`.
- Keys are **hub-authoritative in hub mode and session-scoped in
  standalone**; persisted records resolve through `FileRef` evidence, and a
  strong-evidence conflict (digest or per-frame pixel digest) is a refusal
  (1.3). Key rules carry a version (`key_rules`).
- Label targets have a canonical target id (`patient:`, `study:`, `series:`,
  `folder:<root>/<path>`, or the file key); folder targets carry a root id and
  missing hierarchy ids fall back to `missing:<FileKey>` (4.3).
- Frames are zero-based within a file. `FrameScope` is `"all"` or a sorted
  unique set.
- Coordinates: `f64`, corner origin (pixel centre at `+0.5`), x = column,
  y = row, range `[0, columns] × [0, rows]`, quantized to 1/1000 px, in the
  stored matrix (DICOM) or the recorded oriented space (rasters). Never
  millimetres, never display orientation.
- Records have UUIDv7 ids created by the client, `rev`, `created_by`/
  `modified_by` as `user:`/`model:`/`import:` strings, server timestamps,
  optional `derived_from` and `score`.

**Integration (`seams.md`)**
- The sync unit is `OpEnvelope { op_id (UUIDv7), actor, ts, op }`; ops are
  idempotent by `op_id` and checked by `base_rev`; there is no undo op. Every
  op names a versioned target, including `SetLabel` (id, `base_rev`) and layer
  ops (id, `base_rev`); delete is undone by `RestoreAnnotation` (7.2).
- Queue key per op: file key, canonical label target id, or layer id; one
  global ordered client queue; `Batch` is atomic, one envelope, one result
  (7.2).
- `AnnotationBackend { snapshot, apply, export }` as in 7.4; the in-memory
  store is the default. One transaction per envelope covering validation,
  state, audit and dedup result; `Ok` returns every affected revision.
- The earlier rule "keys computable by the spoke from the file alone" is
  replaced by 1.3: `key` is mandatory in hub-written file lists, a spoke
  refuses a file whose key it computes differently, and a `key_rules` version
  mismatch refuses the spoke. The key-rule version is in the consolidated
  seam list (`seams.md` 1 and 12).
- Catalog: `/api/files?since=<revision>` returns inserted and updated entries
  (`reset: true` to refetch); frame responses carry `X-File-Key` once a pending
  key resolves; standalone rekeys arrive as one event the client applies to
  records, queue, history and selection (1.7, 1.8).
- The hub stamps `created_by`/`modified_by` for spoke writes; spoke stamps are
  advisory.
- `--annotation-config` carries the label schema (4.2), the initial layers
  (5) and tool enablement (the tools doc defines the tool part).
- `FileSummary` gains `file_key` and `alias_of`; the hub computes the same keys
  independently with the same rules, so the rules live in the shared crate.
  The hub passes every key (mandatory) and an optional `external_id` as
  file-list columns; the spoke verifies them when it hashes and refuses a
  mismatching file.
- Imports run by the hub itself are authored `import:<adapter>` (6.1).
- The model crate has no dicom-rs, axum or dcmview dependency.

**Image formats (`image-formats.md`)**
- Rasters are keyed `b3:<blake3 of file bytes>`, computed while decoding, on
  demand before the first annotation, or by the hub at setup and passed in the
  file list. Discovery never hashes. Whole-file hashing of multi-page TIFF is
  background I/O (a full read), not free at decode (1.7). When decoding a
  frame, also produce that frame's pixel digest in the canonical layout of 1.6.
- Annotations are in the stored pixel grid; the orientation value is reported
  so it can be applied as the initial view transform and recorded in
  `FileRef.space`. `rows`/`columns` are stored dimensions.
- Multi-page TIFF frames are dense indices with `FileRef.frame_source` holding
  each frame's IFD index; report the detected `format`.
- Rasters are labellable at `file`, `frame` and `folder` targets only, unless
  the image formats hierarchy design gives them clinical parents.
- Needed from image formats: whether EXIF is honoured, whether any format has
  frames of different sizes (the model assumes one `rows × columns` per file),
  and 16-bit or alpha cases that affect masks.

**The gallery (`gallery-views.md`)**
- Labels from the gallery are `SetLabel` ops on `LabelTarget`s (`patient`,
  `study`, `series`, `file`, `frame`, `folder`); a multi-selection is one
  atomic `Batch` so it is one undo step and one result.
- Aliased files (`alias_of`) should show as one item with a duplicate marker.
- The catalog's `?since=<count>` becomes `?since=<revision>` with updates and
  `reset` (1.8); the tree uses the non-collapsing `missing:<FileKey>` fallback
  ids and root-qualified folder ids (4.3).

**Annotation tools and UX (`annotation-tools-ux.md`)**
- Geometry types and invariants in 3.1; masks as tiles in 3.2 (64 px, bitpacked,
  deflate, base64) with the exclusive-layer rule.
- Snapping is a tool property: rect snaps to pixel edges by default, point to
  centres.
- The undo tree is client-only and applies inverse ops as new forward ops;
  mask ops carry before and after tiles. The inverse of a delete is
  `RestoreAnnotation` (same id); the inverse of a `SetLabel` is a `SetLabel`
  on the same label id.
- `Batch` is atomic with one result (confirmed); queue keys and the global
  ordered queue are as in 7.2; a standalone rekey arrives as one event to
  apply to records, queue, history and selection (1.7).
- Layer visibility, lock and order are view state.
- Classes constrain which geometry types a tool may create.

**Output adapters (`output-adapters.md`)**
- `ExportAdapter`/`ImportAdapter` traits, `AdapterInfo.capabilities`,
  `ExportContext`, `ImportContext` with the resolver, and mandatory reports
  (8.1).
- EMBED mapping and loss policy (9.2, 9.3); `embed-extended` as a separate
  mode once the owner's schema is known. EMBED export rows sorted by path;
  EMBED CSV import lenient with a report for out-of-range and zero-area rows,
  native formats strict.
- Centre-origin conversions and millimetre values are adapter work.
- `code` on classes and options is what DICOM SEG/SR exports read.

**The hub**
- Studio-side; not part of the public design. See `seams.md` for the dcmview
  side.


---

## Re-baseline amendment (0.3.2, confirmed by the owner 2026-10-05)

This doc was written against dcmview 0.3.1 dev. Since then 0.3.1 added presentation-state graphic annotations drawn over the frame, and 0.3.2 added `--mask` display masking and in-memory redaction boxes edited with a Redact tool. The owner confirmed these resolutions on 2026-10-05 :

- **Redaction boxes are not annotations.** They keep their own store and endpoint, outside the neutral model, the op log, the history tree and every adapter or export. The op store does not absorb them.
