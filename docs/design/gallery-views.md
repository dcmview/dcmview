# Gallery views: scoping

Read against `dcmview/dcmview` at `b78536f` (0.3.1 dev), `dcmview-docs`
(v0.3.0 sync) and `dcmview-test-corpus`. Date: 2026-09-29. This is a
discussion of approaches, not a spec. Nothing was changed in any repository.

Taken as decided (the owner's brief): dcmview stays ephemeral and the hub owns
durable state; one neutral annotation model with EMBED as an adapter;
standalone dcmview gets a URL token for TCP plus an optional Unix socket mode.

Amended on 2026-09-30 after two external reviews ("no decision needed" fixes
for this area; no review decision changes the gallery); see the list below.

---

## Review amendments (2026-09-30)

- **3.2, 3.4, 9 item 2**: thumbnails are rendered in the stored pixel grid, never rotated or flipped, for rasters too; the client applies EXIF/display orientation with the viewer's transform state, so stored coordinates map by one scale per axis and orientations 5 to 8 are handled client-side.
- **3.4, 3.5, 9 item 3, 10 item 5**: one presentation policy for every thumbnail source; a display-cache hit that has shutters or overlays burned in is skipped (or the thumbnail is taken from the pre-encode render buffer before they are applied).
- **0, 4, 4.2, 8, 9 item 5**: "never delays" replaced by a measured interactive-latency target; one permit is reserved for interactive work, and background work is limited on one-core hosts.
- **5.1**: the element-height limit uses Firefox's ~17.9M px, not Chrome's
  ~33M px.
- **1, 7.4, 9 item 8 (new), 10 item 9, 11**: `?since=<count>` replaced by a revision cursor that returns inserted and updated entries (key and alias changes) with `reset: true` when the client must refetch; the first frame response carries `X-File-Key` once a pending raster key resolves.
- **8, 9**: tests for all eight EXIF orientations, warm/cold cache equality and the latency target; the integration seams lines gain the live scope update and the `X-Dcmview-Background: 1` header on background requests, the image formats doc lines the stored-grid rule.

---

## 0. Summary

- **The hard part is decoding, not the grid.** A 3328x4096 12-bit JPEG 2000
  lossless mammogram takes about 2.6 s to decode at full resolution with the
  decoder dcmview already ships. Asking the same decoder for 1/8 resolution
  takes 35 ms (measured, section 3.3). Without reduced-resolution decoding, a
  screen of 200 mammography thumbnails costs minutes of CPU; with it, seconds.
- **Everything the grid needs is already in the browser.** `/api/files` and
  `/api/series` already give the client every file, its hierarchy fields and
  the server's stack grouping. The gallery needs one new server endpoint (a
  thumbnail) and a background decode class in the pixel service. Grouping,
  selection, virtual scrolling and hover scrubbing are client work.
- **Recommendations in one line each:** per-frame `GET` thumbnail endpoint
  returning small JPEGs; its own byte-budgeted cache that never evicts the
  viewer's caches; a background decode class that yields to the viewer and
  is held to a measured latency target (4.2); a client-side request scheduler (at most 4 in flight, visible tiles
  first, aborting what scrolls away); a hand-written virtualized grouped grid
  instead of pages; a tile per file except for real slice volumes, which
  collapse into one expandable stack tile; pointer-position scrubbing over 9
  sampled frames; one Gallery tab in the tab strip.
- **Decisions for the owner** are collected in section 10. The most
  consequential are the navigator click change (10.1) and what counts as one
  tile (10.2).

---

## 1. What exists today

**Navigator** (`frontend/src/lib/FileNavigator.svelte`, `lib/fileTree.ts`):
two organizations, clinical (patient, study, series, file) and directory
(folders, files), built client-side from the full `FileSummary` list on every
catalog change. Header rows only toggle collapse; file rows open a tab. There
is no selection state at all: no focus model, no shift/ctrl handling, nothing
that marks a patient, study, series or folder as chosen. The tree renders
every expanded row (no virtualization); it auto-collapses above 500 files.
Node keys come from UIDs or paths, but fall back to `file-${index}` when the
UIDs are empty, so a few keys are not stable across processes.

**Catalog** (`lib/app/catalog.svelte.ts`, `server/catalog.rs`): the client
polls `/api/files` (the full summary list, every time it changes) and
`/api/series` until the scan completes, then stops polling
(`catalog.svelte.ts:74`), so a later change to an entry (a raster key that
resolves after first decode, a rekey) never reaches it (see 7.4). So the gallery can group, sort and
count without new list endpoints. The whole list is one JSON document,
several hundred bytes per file; at 100k files that is tens of MB per changed
poll. That is a pre-existing ceiling, not a gallery problem, but it is the
real limit on "huge file numbers" (section 7.4).

**Stacks** (`src/series.rs`): the server groups files strictly by Study and
Series UID, then into stacks: `Ordinary` (all ordinary files of the series,
ordered by patient geometry when available), `Concatenation`,
`WsiPyramidLevel` and `WsiCompanion`. Each stack lists every
`(file_index, frame_index)` in order, with geometry warnings
(`MissingPositions`, `DuplicatePositions`, `InconsistentOrientation`,
`NonuniformSpacing`, `GantryTilt`). Tabs already open a whole stack
(`lib/app/tabNavigation.svelte.ts`).

**Pixel service** (`src/pixels/`): display PNGs and raw frames, each with a
byte-budgeted LRU (256 MiB display, 384 MiB raw) and shared in-flight
decodes. One global semaphore sized to the core count (`DECODE_PERMITS` in
`service.rs`) gates every decode, first come first served. A decode runs as
its own task and finishes (and caches) even when the client disconnects.
There is no notion of priority and no thumbnail or downscaled output
anywhere. Every render path goes straight from samples to an encoded PNG.

**Codecs relevant to speed**: JPEG 2000 goes through `jpeg2k` 0.10.1 directly
on the fragment (`pixels/jpeg2000.rs`); JPEG Baseline, Lossless, JPEG-LS and
JPEG XL go through `dicom-pixeldata`'s frame decode
(`pixels/pixeldata_frame.rs`), which uses `jpeg-decoder` 0.3.2 for JPEG.

**Icon Image Sequence**: an earlier scoping read said the loader "already
recognises it". More precisely, discovery knows to *skip* it (nested pixel
elements must not count as the object's pixel data, `loader/entry.rs`,
`pixels/native.rs`). Nothing reads or records the icon today.

**Frontend conventions that constrain the design**: zero runtime npm
dependencies (everything, including the prefetch scheduler, is hand-written);
components fetch only through `api.ts`; viewer prefetch already caps itself
at 3 to 4 concurrent requests (`viewport/prefetchScheduling.ts`); API
responses send no `Cache-Control`.

**Test corpus**: MG cases exist (uncompressed and RLE, MONOCHROME1 and 2),
plus `stress/study/high_instance_count_ct`. There is no JPEG 2000 mammogram,
no tomosynthesis multiframe, and no Icon Image Sequence case.

---

## 2. Selection model

### 2.1 What gets selected

The owner's roadmap: a gallery appears when a hierarchy level is selected
(patient, study, series, folder) or when several files or levels are selected.

Two separate selections are needed, and conflating them causes confusion:

1. **Scope selection** (navigator): which part of the hierarchy the main area
   shows. One file selected opens the viewer as today; one node, or more than
   one item, shows the gallery.
2. **Action selection** (inside the gallery): which tiles the next action
   applies to: open, open as stack, and later "apply label", "mark done",
   "export". This is the natural surface for bulk hierarchy labelling.

An earlier scoping read proposed only (1). Without (2), every bulk action
would have to go back through the tree.

### 2.2 Interaction rules (both surfaces)

Standard desktop semantics, the same in the tree and the grid:

- Click: select only this item (and in the tree, focus it).
- Ctrl/Cmd-click: toggle this item.
- Shift-click: range from the anchor to this item, over the **visible order**
  (visible tree rows, or grid order). Ctrl+Shift adds the range.
- Keyboard: arrows move focus, Shift+arrows extend, Space toggles,
  Ctrl/Cmd+A selects all in scope, Escape clears.
- Mixed levels are allowed (a study plus two loose files). A selected node
  subsumes its descendants; the resolved content is the de-duplicated union of
  files under the selection, in navigator order.
- The navigator filter applies: the gallery shows only matching files.

### 2.3 The navigator click change (needs the owner)

Today a header click only toggles collapse. To make levels selectable there
are three options:

| Option | Behaviour | Cost |
|---|---|---|
| A. Row selects and toggles (VS Code explorer) | Clicking a study row selects it (gallery shows it) and expands or collapses it; the chevron only toggles | Closest to today; one click does two things, and collapsing a selected node while looking at its gallery can feel odd |
| B. Row selects, chevron toggles (Finder list view) | Row click selects and expands if collapsed, never collapses; chevron toggles | Clean separation; changes the current "click header to collapse" habit |
| C. Keep rows as toggles, add a gallery button per row | Nothing changes for existing users | Extra affordance on every row; multi-select still needs a new gesture |

**Recommendation: B.** It makes selection the primary meaning of a click,
which multi-select needs anyway, and it never hides what you just selected.
This changes existing interaction, so per AGENTS.md it needs the owner's
explicit sign-off.

### 2.4 Selection identity

Selection must survive catalog polls during a progressive scan, so it is keyed
by node key, not array position. Current keys are fine for UIDs and paths but
fall back to file index when UIDs are empty. The gallery needs the same
stable target keys that hierarchy labels will use, so it should take them
from the annotation model rather than invent its own (see section 9).

Switching the navigator between clinical and directory organization: the
selection resolves to its file set, and that file set becomes the selection in
the other organization, regrouped. Simpler than keeping two selections, and
the user keeps looking at the same images.

---

## 3. Thumbnails: endpoint, rendering, cost

### 3.1 Where thumbnails are made

| Option | Verdict |
|---|---|
| Browser fetches full display PNGs and shrinks them | Rejected. A mammogram display PNG is several MB; over `ssh -L` this is unusable, and the server still does the full decode |
| Browser fetches raw frames and windows them | Rejected for the same reason, and worse (16-bit samples) |
| Server renders small images | **Recommended.** The only option where a reduced decode can help |

### 3.2 Endpoint shape

Options considered:

- **(a) Per-frame GET** `GET /api/file/{index}/frame/{frame}/thumbnail?size=256`
- **(b) Batch** (a POST with many keys, streamed back as length-prefixed
  images)
- **(c) Filmstrip** (one image containing N sampled frames of a multiframe
  file, for hover scrubbing)

Per-request overhead is not the bottleneck: on loopback it is negligible, and
over an SSH tunnel with 50 ms RTT, 4 parallel connections still move about 80
cached thumbnails a second. Decoding is the bottleneck (3.3). So:

**Recommendation: (a) only, for v1.** It fits the existing contract pattern
(endpoint table, route, generated TS, `api.ts` wrapper, runtime conformance
test), it is independently cacheable and abortable, and it serves
multiframe files and multi-file stacks alike, because the client picks which
frames to ask for. (b) and (c) stay open as optimizations if measurements over
real tunnels show round trips mattering.

Proposed contract:

```
GET /api/file/{index}/frame/{frame}/thumbnail?size=<px>[&window_mode=default|full_dynamic]
200 image/jpeg
X-Cache: HIT | MISS
X-Thumbnail-Source: thumbnail_cache | display_cache | raw_cache | reduced_decode | full_decode
Cache-Control: no-store
Errors: the shared JSON envelope. 404 not_found / frame_out_of_range,
        422 unsupported_transfer_syntax / unsupported_pixel_layout,
        400 invalid_query (size out of range), 500 pixel_decode_failed.
```

- `size` is the longest edge in device pixels. The server snaps it up to a
  bucket (128, 256, 512, 1024) so cache keys stay few; the browser scales
  down.
- The image is the frame's **whole field of view** in the **stored pixel
  grid**, resampled to physical aspect (`pixel_aspect_ratio`), never cropped,
  rotated or flipped, rasters included. EXIF and other display orientation
  is applied by the client, with the same transform state the viewer uses
  (amended 2026-09-30). That makes it trivial to draw annotations
  on a thumbnail later (section 9).
- `Cache-Control: no-store` because file indexes are only valid within one
  server process: after a restart on the same port, `/api/file/3/...` can be a
  different file, and a browser cache would show the wrong image. Once the
  annotation model gives stable file keys, a key-addressed URL could be
  `immutable`.
- The client never requests thumbnails for files the catalog marks
  `unsupported` or without pixels; those tiles are drawn from `FileSummary`
  alone (icon, object kind, reason).

### 3.3 Decode cost and fast paths

Measured in this container (4 cores, release build, the same `jpeg2k` 0.10.1
and `jpeg-decoder` 0.3.2 dcmview locks, single decode per call). Input: a
synthetic 3328x4096 12-bit mammography-like image with noise, JPEG 2000
lossless with 6 resolution levels (11.4 MB, typical of real lossless
mammograms).

| JPEG 2000 `reduce` | Output | Decode time |
|---|---|---|
| 0 (what the viewer does now) | 3328x4096 | 2596 ms |
| 1 | 1664x2048 | 599 ms |
| 2 | 832x1024 | 138 ms |
| 3 | 416x512 | 35 ms |
| 4 | 208x256 | 11 ms |

A 256 px thumbnail needs `reduce` 3 or 4: **70 to 230 times faster**, and the
transient decode memory drops from about 55 MB per decode to under 1 MB.
Downscaling an 8-bit frame to 256 px and JPEG-encoding it took 10 ms; the 256
px JPEG was 3.7 KB against 18 KB as PNG.

Per-codec plan:

| Codec | Fast path | Notes |
|---|---|---|
| JPEG 2000 | `jpeg2k::Image::from_bytes_with(buf, DecodeParameters::new().reduce(r))` (API confirmed in the locked crate) | Clamp `r` to the codestream's decomposition levels (read from the COD marker). The whole fragment is still read from disk; reading only the needed packets is possible in theory, not worth it now |
| JPEG Baseline (8-bit) | `jpeg-decoder`'s `Decoder::scale()` (DCT scaling to 1/2, 1/4, 1/8; confirmed in 0.3.2) | Needs a direct call on the fragment, like the JPEG 2000 path, instead of `dicom-pixeldata`'s frame decode |
| JPEG Lossless, JPEG-LS, RLE, JPEG XL, deflated | None practical: full decode, then downscale | JPEG Lossless is common in older mammography archives; these rely on the cache and the background class |
| Native (uncompressed) | Full frame read, then downscale | Cheap on local disk. On network filesystems a strided row read would cut I/O; later, if measured |
| WSI | Use the `THUMBNAIL` companion image when the series has one (already classified by `series.rs`), else the lowest pyramid level if it is a single tile, else an icon | Never stitch; matches the WSI contract |
| Icon Image Sequence | Placeholder only, later | Icons are producer-rendered (often 64x64, sometimes palette), too small for the size slider and windowed differently from the viewer. Useful as an instant blurry preview while the real thumbnail decodes. Needs discovery to record the icon's offset. **Recommend deferring** until the rest is measured on real data |

### 3.4 Rendering pipeline

Source preference, cheapest first:

1. Thumbnail cache hit (or a larger cached bucket, decoded and shrunk: a few
   ms).
2. The display cache already holds this frame at the default window (the user
   viewed it): decode that PNG and shrink, **only if the frame has no shutter
   and no overlay planes**. The display PNG has them burned in
   (`render.rs:185-203`, `:421`), which thumbnails omit (decision 10.5), so
   such a hit is incompatible and skipped. Once the render seam below exists,
   this step reads the pre-encode 8-bit buffer instead, taken before
   shutters and overlays are applied.
3. The raw cache holds the frame: window at full resolution through the
   existing path, shrink.
4. Reduced-resolution decode where the codec allows (3.3), window, shrink.
5. Full decode, window, shrink.

Rules:

- **One presentation policy for every source** (amended 2026-09-30).
  Whichever step produces a thumbnail, the result has the same presentation:
  the default window (or full dynamic), no shutter, no overlay graphics,
  stored grid, physical aspect. A source that cannot give that (a burned-in
  display PNG, an already-oriented EXIF thumbnail that cannot be mapped back
  to the stored grid) is skipped, never used "close enough". So the cache key
  needs no source component, and `X-Thumbnail-Source` is diagnostic only.
- **Thumbnails never insert into the display or raw caches.** Those hold the
  viewer's working set (the 384 MiB raw cache holds roughly 14 uncompressed
  mammograms); a gallery scroll would evict it. Thumbnails read those caches
  but only write their own.
- Presentation: the viewer's **default** presentation (DICOM window, else VOI
  LUT, else automatic histogram window), Modality transform, MONOCHROME1
  inversion, palette and YBR conversion. `full_dynamic` as an optional query
  value. No arbitrary window in v1: it multiplies cache keys. "Apply this
  preset to the gallery" (CT lung or bone) is a natural later extension, keyed
  by window.
- **Shutters and overlay graphics are omitted in v1.** Both are defined in
  full-resolution coordinates and would need rescaling through
  `shutter.rs`/`overlay.rs`. A thumbnail without a shutter shows the
  collimated area unmasked; that seems acceptable for a preview. (Decision
  10.5.)
- Automatic windows computed from a reduced image differ slightly from the
  full-resolution histogram window. Negligible at thumbnail size, but it means
  a thumbnail is not bit-comparable to a shrunk display frame; tests compare
  within a tolerance.
- Downscale in display space (8-bit gray or RGB) with an area filter, after
  palette/colour conversion, to physical aspect.
- Encode as JPEG, quality about 85. Thumbnails are previews; the viewer stays
  lossless. RGB ICC profiles are not carried into thumbnails.
- Rasters go through the image formats doc's normalization (stored sample
  semantics, image-formats 5.2) like the viewer, so a thumbnail of a 1-bit PNG
  or a WhiteIsZero TIFF matches the viewer's default presentation.

The main refactor this needs: every render path currently ends in PNG encoding
(`render.rs` `encode_*_png`). Splitting "render to an 8-bit display buffer"
from "encode" gives thumbnails (and later raster formats, the image formats
doc) one shared seam. This is the largest server-side change in the area.

### 3.5 Caching

- A new `ThumbnailCache` (`BudgetedLru`), 64 MiB of encoded JPEGs, which holds
  roughly 10k to 15k thumbnails at 256 px. Key: `(file_index, frame, bucket,
  window_mode)`; no source component is needed because every source yields
  the same presentation (3.4). In-flight sharing as for other caches.
- **No disk cache in dcmview.** A persistent cache would break the ephemeral
  axiom. A hub, which owns durable state, could keep one.
- Browser side: a byte-budgeted cache of thumbnail blobs (object URLs revoked
  on eviction), about 64 MiB, keyed the same way plus server instance. It
  survives tab switches and gallery re-entry within the page's life.

---

## 4. Scheduling: keep the viewer fast

This is where a naive gallery hurts the rest of the app. The goal is a
measured bound on how much the gallery delays the viewer, not "never"
(amended 2026-09-30): once a background decode holds a core it runs to
completion, so it can always delay a click by up to one decode.

### 4.1 The browser connection limit

Browsers allow 6 concurrent HTTP/1.1 connections per origin, and dcmview (also
when reached through a hub over `ssh -L`) is plain HTTP/1.1; browsers speak
HTTP/2 only over TLS. 300 `<img src>` tags would queue behind each other and
behind the catalog poll, tag fetches and any viewer frame request.

**Recommendation: a client-side thumbnail scheduler**, not bare `<img src>`:

- `fetch()` with `AbortController`, at most 4 in flight while the gallery is
  visible (reusing `prefetchConcurrencyFor` for slow connections), 0 while a
  viewer tab is active.
- Priority: hover samples of the tile under the pointer, then visible tiles in
  reading order, then about one screen ahead in the scroll direction, then
  nothing. The newest scroll position wins.
- Abort requests for tiles more than about two screens away.
- Blob into an `<img>` with `decoding="async"`; the browser handles image
  memory.
- This is also required by auth: the API token travels as an
  `Authorization` header (`seams.md` 2), which `<img src>` cannot
  send.

### 4.2 Server-side priority

`DECODE_PERMITS` is first come, first served, so 100 queued thumbnail decodes
would sit in front of the frame the user just clicked.

| Option | Verdict |
|---|---|
| Separate semaphore for thumbnails | Simple, but both pools run at once, so thumbnails still take cores from the viewer |
| Share the global semaphore | Viewer waits behind thumbnails. Rejected |
| **Two-class scheduler**: interactive and background; background capped at `max(1, cores/2)`, never more than `cores - 1` (at least one permit reserved for interactive work), and granted only when no interactive request is waiting | **Recommended.** A small hand-written scheduler (tokio has no priority semaphore) |

Two related rules:

- A thumbnail request waits for its permit **inside the request future**, so a
  browser abort drops it before any work starts. Only once a permit is held
  does the decode spawn and finish (and cache) regardless, as today. The
  current detached-task pattern would keep burning CPU on tiles the user
  scrolled past.
- Discovery runs on rayon and `spawn_blocking`, so thumbnails during a scan
  compete with it. The background class makes that tolerable; tiles appear
  and fill as files arrive.

**Latency target, not a guarantee** (amended 2026-09-30). A background decode
is not preemptible, so the scheduler bounds, rather than removes, the delay.
Target: while the gallery is loading, the extra wait for an interactive decode
stays within **p95 100 ms on a 4-core host**, checked by the opt-in timing
test (section 8). On a **one-core host** reserving a permit is impossible
(`max(1, cores/2)` is the whole pool), so there background work is limited to
cheap steps (cache hits and reduced decodes whose estimated cost is under
about 50 ms), and full decodes run only after no interactive request has
arrived for a short idle window (about 1 s). The worst case is then one cheap
decode.

The background class is a general seam: any later non-interactive pixel work
(pre-label generation, for instance) can use it.

---

## 5. Gallery layout and scrolling

### 5.1 Pagination versus virtual scrolling

| Option | For | Against |
|---|---|---|
| Pagination | Simple; bounded DOM and requests; "page 12" is a stable reference | Breaks groups across pages; slow to scan; page size fights the size slider |
| Infinite append | Simple | DOM grows without bound; no jumping to the end. Rejected |
| **Virtualized continuous scroll** | Natural scanning; the scrollbar shows position; only visible rows exist in the DOM | Hand-written (no dependencies); group headers make row heights uneven; scroll anchoring on resize |

**Recommendation: virtual scrolling.** Flatten the groups into a row list
(header rows and tile rows, columns from width and tile size), keep cumulative
offsets, binary-search the visible range, render with overscan, sticky
breadcrumb headers. When the size slider or the width changes, keep the tile
at the top of the viewport anchored. Page Up/Down and Home/End jump by screens
and to ends, which covers what people want from pagination.

One edge: browsers cap element height, and the lowest cap sets the
threshold: **Firefox at about 17.9 million px** (Chrome is near 33 million).
100k files at the largest tile size, one per row, would exceed it; above a
threshold below 17.9M px the scroller maps its range onto a capped height.
Rare, but worth a test in both engines. (Amended 2026-09-30.)

Accessibility: `role="grid"` with `aria-rowcount` and `aria-rowindex`, roving
focus, labels from the same text the navigator uses.

### 5.2 Size control

A slider from about 64 to 512 CSS px (plus Ctrl+wheel and +/- keys), stored
for the page's life. The request size is CSS size times device pixel ratio,
snapped up to the next server bucket.

### 5.3 Tiles

- The image, fit into a square cell with its aspect preserved.
- A caption (default: instance number and file name, or series description for
  a stack). Caption fields must be configurable, because campaigns may want
  PHI hidden (section 9).
- Badges: frame count (multiframe), file count (stack), unsupported or
  metadata-only state, and reserved slots for annotation status later.
- A selection checkbox on hover or when anything is selected.

Opening: double-click or Enter opens a viewer tab (a stack tile opens its
stack tab; a scrubbed multiframe tile opens at the scrubbed frame). Single
click selects.

### 5.4 Where the gallery lives

| Option | Verdict |
|---|---|
| Gallery replaces the main area while a node is selected | Loses the gallery's scroll position every time the user opens an image |
| Every selection opens a new gallery tab | Tab clutter |
| **One Gallery tab**, first in the tab strip, retargeted by the navigator | **Recommended.** Viewer tabs stay intact; returning to the gallery keeps its scroll and selection |

---

## 6. Grouping

### 6.1 What counts as one tile (needs the owner)

The roadmap: multiframe files are one tile; a multi-file series renders as one
unit that can be expanded into its files.

The catch is that the server's `Ordinary` stack is simply every ordinary file
in a series. For CT and MR that is a volume. For mammography, a series often
holds several separate 2D views (L CC, L MLO, R CC, R MLO); for CR, US and
secondary capture it is a set of unrelated images. Collapsing those into one
tile would hide exactly what a mammography reviewer wants to see.

Proposed rule, as a pure client function that is easy to tune:

- A stack becomes **one stack tile** when it is `Ordinary` with 2 or more
  files, every file is single-frame, and its geometry is a volume: positions
  present and orientation consistent (no `MissingPositions` or
  `InconsistentOrientation` warnings). Duplicate positions (multi-echo,
  diffusion) still count as a volume.
- `Concatenation`: one tile.
- WSI pyramid: one tile (thumbnail as in 3.3); companions (label, overview)
  are their own tiles.
- Everything else: **one tile per file**. Multiframe files (tomosynthesis,
  cine) are always a single scrubbable tile.

Mammography 2D views have no Image Position (Patient), so they land as
separate tiles, which is the intent. Toolbar toggles: "Group slices into
stacks" (on by default) and "Expand all stacks".

Expanding a stack replaces its tile with its file tiles in place, under a
small sub-header. Expanding a multiframe file into frame tiles is possible
with the same machinery; offer it, not as the default.

The tile's frame is the centre: the middle frame of a multiframe file, or the
middle virtual position of a stack.

### 6.2 Clinical grouping

Groups follow the navigator: patient, study, series, tiles, in the
navigator's sort order (so the viewer's Up/Down order and the gallery agree).
Headers show only the levels below the selection: selecting a series shows no
headers; selecting a patient shows study and series headers.

A mammography problem: when each view is its own series, a study shows 4 to 8
series headers with one tile each, and the grid fragments into single-tile
rows. **Recommendation:** series sub-headers appear only when a series has 2
or more tiles; single-tile series flow into the study group with the series
description as the caption. A "Group by: Study | Series" control covers users
who want strict grouping. (Decision 10.7.)

### 6.3 Directory grouping

**Recommended:** one section per folder that directly contains files, in
depth-first order. The header shows the path relative to the selected node,
indented by depth, and is collapsible, so nesting is visible without
drilling in. The alternative, folders as tiles that you open (like Finder's
icon view), hides content behind clicks and suits browsing less than
scanning. Single-tile folders get the same flattening as single-tile series.

### 6.4 Non-image objects

SR, PR, RT Structure Set and other metadata-only objects get icon tiles from
`FileSummary`. A toolbar toggle hides them. **Default: shown, dimmed**, so
counts in the gallery match the navigator. SEG shows its own pixel preview;
composing the source image with the overlay in a thumbnail is a later nicety.

---

## 7. Multiframe hover scrubbing

### 7.1 Behaviour (needs the owner)

The roadmap says tiles "cycle through a selection of thumbnail frames" when
hovered. Two ways to do that:

- **Auto-cycle**: a timer steps through the frames while hovered. Matches the
  text; the user can't stop on the frame they care about, and a grid full of
  cycling tiles is busy.
- **Pointer scrub**: the pointer's horizontal position picks the frame, as in
  video-site previews. Deterministic and quick; the user controls it.

**Recommendation: pointer scrub**, with a thin position bar on the tile, and
auto-cycle only for a keyboard-focused tile after a short pause (so keyboard
users get something). Touch devices: press and drag.

### 7.2 Mechanics

- 9 samples (odd, so the centre is one of them), evenly spaced over the
  tile's frames (the multiframe file's frames, or the stack's virtual
  positions).
- Requested after a 150 ms dwell, so sweeping the mouse across the grid does
  not fire hundreds of requests; top priority in the scheduler.
- Until samples arrive, the tile shows the centre frame and the position bar
  fills as samples land.
- Optional idle prefetch of samples for visible multiframe tiles, after all
  visible centres are done.
- On leave, the tile returns to the centre frame; the scrubbed position is
  remembered while the tile stays mounted, so opening uses it.

### 7.3 Cost

A tomosynthesis volume in JPEG 2000 costs 9 reduced decodes per hover (on
the order of 100 to 300 ms total on the background class); uncompressed or
RLE costs 9 full frame reads or decodes. Acceptable because it is driven by
the user's pointer, one tile at a time.

### 7.4 Scale limits outside the gallery

The gallery renders any number of tiles cheaply, but two existing paths
limit how many files dcmview can hold comfortably:

- `/api/files` sends the full list on every changed poll (section 1). An
  incremental form fixes it. **Amended 2026-09-30:** entries
  do not only append (pending raster keys resolve, collisions rekey, aliases
  change), so `?since=<count>` is replaced by **`?since=<revision>`**: the
  catalog keeps a monotonically increasing revision, and a delta returns
  every entry **inserted or updated** after it (key and alias changes
  included) plus the new revision, or `reset: true` when the client must
  refetch the full list (for example after a server restart). Polling
  continues after the scan completes, as a background request
  (`X-Dcmview-Background: 1`), so late changes arrive. The first frame
  response for a file whose raster key was pending carries `X-File-Key`
  once the key resolves, so the open tab learns it without waiting for a
  poll. Small, independent, worth doing with the gallery.
- The navigator tree is not virtualized. The gallery's virtual-row code could
  be reused for it later.

---

## 8. Tests and fixtures

Fixtures (committed small, generated by the existing fixture script where
possible):

- A JPEG 2000 lossless 16-bit single frame, large enough for several
  decomposition levels (for example 2048x1536 with 5 levels), to check the
  reduced path is taken and matches a shrunk full decode within a tolerance.
- A multiframe JPEG 2000 frame set (tomosynthesis-like) for centre and sample
  selection.
- A JPEG Baseline frame for the DCT-scaling path.
- Later, an Icon Image Sequence case (none exists in the corpus).
- A mixed tree: metadata-only objects, an unsupported syntax, a mammography
  study with one view per series, a CT volume, a localizer series with mixed
  orientations.
- For scale, the corpus's `stress/study/high_instance_count_ct`.

Test layers, following the repository's pattern:

- Rust: thumbnail equals the shrunk default display frame within a tolerance,
  for every codec fixture; `X-Thumbnail-Source` shows the reduced path for
  JPEG 2000 and JPEG Baseline; thumbnails do not insert into the raw or
  display caches; the scheduler grants interactive before background and drops
  aborted waiters; the endpoint is in `endpoints::ALL` and the runtime
  contract test.
- Vitest: the tile-unit rule (6.1) over fixture catalogs, grouping and
  flattening, selection resolution (subsumption, ranges, mode switches),
  virtual-row math and scroll anchoring, sample-frame selection, the client
  scheduler's priorities and aborts.
- Manual acceptance (no profile drives a real browser): scroll a few thousand
  files over an `ssh -L` tunnel; open an image while thumbnails load and check
  the viewer's extra delay is within the 4.2 target; check the tall-scroll
  cap in Firefox.
- Performance: an opt-in timing check for reduced versus full JPEG 2000
  decode, and one for interactive latency under background load (the 4.2
  target, on 4 cores and on 1 core), per the architecture doc's rule that
  performance targets need explicit instrumentation.
- Thumbnail orientation: all eight EXIF orientations, checking that the
  server thumbnail is in the stored grid and the client transform matches
  the viewer's; a frame with a shutter or overlay gives the same thumbnail
  whether the display cache is warm or cold.

---

## 9. Interfaces for downstream areas

What this area fixes, and what it needs from other areas.

### Fixed here (later areas can rely on these)

1. **Thumbnail endpoint.** `GET
   /api/file/{index}/frame/{frame}/thumbnail?size=<px>[&window_mode=]`,
   `image/jpeg`, headers `X-Cache` and `X-Thumbnail-Source`, `Cache-Control:
   no-store`, the shared JSON error envelope. If the annotation model moves
   file addressing to stable keys, this endpoint follows the same addressing
   as every other file endpoint.
2. **Thumbnail geometry.** A thumbnail is the frame's whole field of view **in
   the stored pixel grid**, resampled to physical aspect, fitted inside `size`
   x `size`, never cropped, rotated or flipped, rasters included. So a
   stored-grid coordinate maps to a thumbnail coordinate by one scale per
   axis, computed from `rows`, `columns` and `pixel_aspect_ratio` (applied
   with the annotation model's pixel-centre convention). **Amended
   2026-09-30:** display orientation (raster EXIF orientation, image-formats
   6.1) is applied by the client with the same transform state the viewer
   uses, after the scale, so orientations 5 to 8, which swap axes, need no
   special case. Shapes drawn on thumbnails go through the same transform.
3. **Presentation.** Default presentation (or full dynamic), no shutters or
   overlay graphics in v1, lossy JPEG, no ICC profile. The same for every
   source; incompatible cache hits are skipped (3.4).
4. **Render seam.** The pixel service gains "render to an 8-bit display
   buffer" separate from encoding, used by the display PNG and the thumbnail.
   The image formats doc's raster decoders should produce into this seam.
5. **Decode classes.** The pixel service gets an interactive and a background
   class; background work yields to interactive decodes within the measured
   latency target in 4.2 (at least one permit reserved for interactive work
   on multi-core hosts; cheap steps only on one-core hosts) and is dropped if
   its request is aborted before starting. (Amended 2026-09-30:
   previously "never delays".)
8. **Catalog deltas.** `/api/files?since=<revision>` returns inserted and
   updated entries, or `reset: true` (7.4). Every catalog consumer (gallery,
   navigator, tabs, selection) applies updates by file key, not by index.
6. **Tile units.** The rule in 6.1: stack tiles only for real slice volumes,
   concatenations and WSI pyramids; one tile per file otherwise; multiframe
   files are always one tile.
7. **Gallery selection output.** The gallery exposes its action selection as a
   list of targets, each `{ level, key }` where level is one of patient,
   study, series, stack, file, frame, folder. Any bulk action (open, label,
   mark done, export) takes this list.

### Needed from the annotation model (`annotation-model.md`)

- **Stable target keys for every hierarchy level**, including patient (today's
  tree key falls back to file index when IDs are empty) and folder (a
  directory-mode target). The gallery and navigator selection will use them
  directly, so selection and hierarchy labels share one identity.
- Whether a **stack** is a label target in its own right or just "every file
  in it". The gallery's stack tile needs to know what a label on it means.
- A **batch annotation summary** for tile badges: per file (and per frame),
  counts of shapes and the label values present, for the current user's
  layers only. One request for the whole scope, not one per tile.

### Needed from integration (`seams.md`)

- A hub in front of a spoke must not serialize one user's concurrent requests
  (the gallery scheduler relies on at least 4 in parallel).
- Auth: the integration seams (`seams.md` 2) settled on a fragment token sent
  as `Authorization: Bearer` with no cookie (confirmed 2026-09-30). `<img
  src>` cannot send that header, so thumbnails **must** load through `api.ts`
  `fetch()` into blobs, which is what the scheduler in 4.1 already does. No
  bare `<img src>` to `/api` URLs anywhere in the gallery.
- `--file-list` scope is what the gallery shows; a worklist "done" flag is a
  tile badge. In hub mode that scope can change live (a versioned scope update
  over the spoke channel, `seams.md` 6); the gallery treats it as a catalog
  update and drops tiles that left the scope.
- Background requests from the gallery (catalog polls, thumbnail prefetch
  beyond the visible screen) carry `X-Dcmview-Background: 1`, so a hub
  does not count them as user activity (`seams.md` 5).
- A **display config** input (via the annotation-config seam or its own) for
  gallery captions, so a campaign can hide patient names and IDs.

### Needed from image formats (`image-formats.md`)

- A reduced decode per raster format where available: JPEG through DCT
  scaling; TIFF through an embedded reduced-resolution page or thumbnail IFD
  when present; PNG through full decode. All of them in the stored grid,
  with the same normalization as the viewer (image-formats 5.2, 11).
- Rasters appear wherever the navigator puts them (the proposed "Other
  images" group in clinical mode) and are per-file tiles; multi-page TIFF is a
  multiframe tile with scrubbing.

### For annotation tools and UX (`annotation-tools-ux.md`)

- The gallery is the natural surface for **bulk hierarchy labels**: select
  tiles, press a number key to apply a category from the label schema to every
  target. For a campaign like "is there a biopsy clip in this image", this
  could be most of the work. The tools doc should design this alongside
  in-viewer labelling.
- Opening from the gallery lands on the scrubbed frame; returning to the
  Gallery tab keeps scroll and selection, which makes review loops fast.
- Drawing a file's shapes on its thumbnail is possible with the geometry in
  item 2; whether to show them by default is a UX call.

### For the hub

- Studio-side; not part of the public design. See `seams.md` for the dcmview
  side.

---

## 10. Decisions that need the owner

**Status: all nine confirmed by the owner on 2026-09-30, as recommended.** The
recommendations below are now decisions.

1. **Navigator clicks** (2.3): row click selects and expands, the chevron
   toggles (recommended), instead of today's "header click toggles". Changes
   existing interaction, so needs sign-off.
2. **What is one tile** (6.1): collapse only real slice volumes; mammography
   views and other projection images stay separate tiles (recommended).
3. **Gallery placement** (5.4): one Gallery tab in the tab strip
   (recommended), or the gallery replacing the main area.
4. **Hover behaviour** (7.1): pointer scrub (recommended) or timed
   auto-cycle as the roadmap describes.
5. **Thumbnail fidelity** (3.4): lossy JPEG, default window, shutters and
   overlay graphics omitted in v1 (recommended). **Amended 2026-09-30**: the
   same presentation from every thumbnail source (burned-in display-cache hits
   skipped), rendered in the stored grid with orientation applied by the
   client.
6. **Virtual scrolling instead of pagination** (5.1): recommended; the
   roadmap said "pagination or similar".
7. **Series headers** (6.2): hide single-tile series headers and flow them
   into the study group (recommended), with a strict "Group by series"
   option.
8. **Icon Image Sequence** (3.3): defer; use it later as a placeholder only
   (recommended).
9. **Incremental file list** (7.4): do the small `/api/files?since=` change
   with the gallery (recommended), or leave the catalog as is.
   **Amended 2026-09-30**: the cursor is a catalog revision,
   not a count; deltas carry inserted and updated entries (key and alias
   changes) or `reset: true`, polling continues after the scan as a
   background request, and the first frame response carries `X-File-Key`
   when a pending raster key resolves. This implements decision 9; it does
   not change it.

## 11. Suggested build order

1. Pixel service: render-to-buffer seam, interactive and background decode
   classes, thumbnail cache, the endpoint with full decode only. Contract,
   route, generated TS, `api.ts` wrapper, tests.
2. Reduced decodes for JPEG 2000 and JPEG Baseline, with the timing check.
3. Frontend: selection model in the navigator (after decision 10.1); Gallery
   tab; virtualized grouped grid; client scheduler; size slider.
4. Tile-unit rule, stack expansion, hover scrubbing.
5. Incremental file list with the revision cursor (7.4).
6. Later: icon placeholders, gallery-wide window presets, batch or filmstrip
   endpoint if tunnels prove it necessary, annotation badges (after the
   annotation model), bulk labelling (with the tools doc).

Steps 1 and 2 are independent of the annotation work and ship value in a
normal dcmview release.


---

## Re-baseline amendment (0.3.2, confirmed by the owner 2026-10-05)

This doc was written against dcmview 0.3.1 dev. Since then 0.3.1 added presentation-state graphic annotations drawn over the frame, and 0.3.2 added `--mask` display masking and in-memory redaction boxes edited with a Redact tool. The owner confirmed these resolutions on 2026-10-05 :

- **Thumbnails honour redactions and masking.** The render-to-buffer seam applies redaction boxes and `--mask` before a thumbnail is encoded, and the thumbnail cache invalidates when a file's redaction boxes change, so the gallery cannot show a redacted region.
