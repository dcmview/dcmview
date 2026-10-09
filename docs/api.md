# dcmview Internal HTTP API

The browser UI talks to the local Rust server through this API. It is internal
to the viewer and meant for `dcmview` debugging, smoke tests, and local
automation. It is not a stable public integration contract.

Every request under `/api` requires `Authorization: Bearer <token>` unless
started with `--no-token`. No endpoint is exempt, including `/api/health`,
exports, and unknown API routes. The scheme is case-insensitive; the token is
case-sensitive and compared in constant time. Cookies and query parameters
are never accepted as credentials. `index.html` and hashed `assets/*` files
remain public; they contain the viewer build, not DICOM data.

A missing, malformed, or incorrect header returns HTTP `401`, with
`WWW-Authenticate: Bearer` and the same JSON envelope for every API path:

```json
{"code":"unauthorized","error":"missing or invalid access token: send Authorization: Bearer <token>"}
```

The process generates one token from 32 OS-random bytes (43 unpadded base64url
characters), valid until it exits, with no expiry or rotation. `DCMVIEW_TOKEN`
can fix the value; `--no-token` disables authentication and warns on stderr.
The launch URL carries `#token=<token>`; fragments are not sent in HTTP
requests. Treat the printed launch URL and startup JSON as credentials.
Keep the listener on loopback and use SSH forwarding for remote work: plain
HTTP does not encrypt bearer headers or DICOM data.

For scripts, start with `dcmview --no-browser --startup-json ./study_dir`.
Its `server_started` JSON line provides `base_url`, `token`, `protocol`, and
`key_rules` (the file-key rules version), along with the launch `url`. In another shell, paste that JSON line when
`read` waits, then request the API (requires `jq` and `curl`):

```bash
read -r startup
base_url=$(printf '%s' "$startup" | jq -r '.base_url')
token=$(printf '%s' "$startup" | jq -r '.token')
printf 'Authorization: Bearer %s\n' "$token" |
  curl --fail --header @- "$base_url/api/files"
```

Use `base_url` when appending API paths; never append them after the launch
URL's fragment. Unix-socket startup reports `socket` and `token`, with
`url: null` and no `base_url`; send the same header with curl's
`--unix-socket <path>` and an `http://localhost/api/...` URL.

Every API response, including errors and unknown API routes, includes
`X-Server-Instance`: the server's Unix-millisecond start time, identical to
`server_start_ms` in the files and health responses. Clients with a loaded
catalog must discard that session when a later response carries a different
identity. The viewer reloads the page in that case; detection remains driven
by ordinary requests, without keepalive polling.

Every response, including the viewer page, assets, and errors, carries
`X-Content-Type-Options: nosniff`, so a browser never reinterprets raw
samples, JSON, or CSV that quote file contents as another document type.

## Source Of Truth

`src/api/contracts.rs` defines every endpoint (`endpoints::ALL`: method, path,
response media type, response headers, success status), the header names, and
every JSON wire type. The router registers those paths, and
`tests/integration/api_contract.rs` requests each declared endpoint and checks
its status, media type, headers, and JSON error envelope.

`frontend/src/generated/api-types.ts` is generated from that module (wire types
through `ts-rs`, plus the endpoint paths and raw-frame header names). After a
contract change, regenerate it and commit the result:

```bash
npm --prefix frontend run generate:types   # cargo run --example generate_api_types
npm --prefix frontend run check:contracts  # fails when the checked-in file is stale
```

The JSON shapes below are summarized; the generated TypeScript lists every
field.

Normal builds reject cross-origin browser access. `cargo build --features
debug-api` enables permissive CORS for debugging from another origin only.

## Endpoints

All paths are under `/api`; `{index}` is a file index from `/api/files` and
`{frame}` a zero-based frame. Static assets are served at `/` and `/assets/*`.

The viewer loads its assets and calls the API with URLs relative to its page
(`assets/...`, `api/...`), so a reverse proxy can serve it under a path prefix
such as `/user/alice/proxy/8888/`. The proxy strips the prefix before forwarding
and redirects the bare prefix to its trailing-slash form.

| Method | Path | Success response |
|---|---|---|
| GET | `/health` | `HealthResponse`: `status: "ok"`, viewer name/version/build identity, `file_count`, `server_start_ms`, `masked`. |
| GET | `/files` | `FilesResponse`: file summaries plus scan progress. Query: `since`, `limit`. |
| GET | `/series` | `SeriesCatalogResponse`: logical series and ordered frame stacks. |
| GET | `/file/{index}/info` | `FrameInfo` for one file. |
| GET | `/file/{index}/references` | `ReferenceCatalogResponse`: declared DICOM relationships and their local matches. |
| GET | `/file/{index}/semantic-context` | `SemanticContextResponse`: SEG, Parametric Map, RT Dose, or softcopy presentation state context, or `not_applicable`. |
| GET | `/file/{index}/frame/{frame}` | Display frame as `image/png`, with `X-Cache`, `X-File-Key` when resolved, and, for linearly windowed frames, `X-Frame-Window-Center`/`X-Frame-Window-Width`. Query: `wc`, `ww`, `mode`, `unit`, `preview`. |
| GET | `/file/{index}/frame/{frame}/thumbnail` | Small `image/jpeg` preview, with `X-Cache`, `X-Thumbnail-Source` and `Cache-Control: no-store`. Query: `size`, `window_mode`. |
| GET | `/file/{index}/frame/{frame}/raw` | Decoded samples as `application/octet-stream`, with `X-Cache`, `X-File-Key` when resolved, and `X-Frame-*` metadata headers. |
| GET | `/file/{index}/frame/{frame}/raw/pixel?row=&column=` | One pixel of the raw frame as a 1x1 raw frame: its stored samples in color-by-pixel order (planar and subsampled YBR_FULL_422 resolved), with the same headers. `400` outside the frame. |
| GET | `/file/{index}/frame/{frame}/presentation-layer` | The display shutter fill and overlay graphics of a grayscale display frame as an RGBA `image/png` of the frame's size, opaque gray where drawn and transparent elsewhere (fully transparent without a shutter or overlay), with `X-Cache`. |
| GET | `/file/{index}/frame/{frame}/segmentation-overlay` | Transparent source-sized SEG mask as `image/png`, with `X-Cache`. |
| GET | `/file/{index}/frame/{frame}/dose-overlay` | RT Dose colorwash sized to this frame as `image/png`, with `X-Cache`. Query: `dose` (RT Dose file index). |
| GET | `/file/{index}/frame/{frame}/dose-overlay/values` | The same resampled dose as little-endian `f32` values, `application/octet-stream`, with `X-Cache`. Query: `dose`. |
| GET | `/file/{index}/frame/{frame}/parametric-map-overlay` | Parametric Map colorwash sized to this frame as `image/png`, with `X-Cache`. Query: `map` (Parametric Map file index). |
| GET | `/file/{index}/frame/{frame}/parametric-map-overlay/values` | The same resampled mapped values as little-endian `f32` values, `application/octet-stream`, with `X-Cache`. Query: `map`. |
| GET | `/file/{index}/frame/{frame}/graphic-annotations` | `GraphicAnnotationsResponse`: the PIXEL-unit graphic and text objects one Grayscale or Color Softcopy Presentation State draws on this image frame, as `[column, row]` image pixel coordinates. Query: `state` (presentation state file index). Empty lists for a frame the state does not annotate; `400` when `state` is missing or is not such a presentation state. |
| GET | `/file/{index}/frame/{frame}/value-mapping` | `FrameValueMapping`: how this frame's stored samples convert to modality and real-world values. |
| GET | `/file/{index}/frame/{frame}/wsi-context` | `WsiFrameContextResponse`: position of one Whole Slide Microscopy tile. |
| GET | `/file/{index}/tags` | `TagNode[]`: preview tag tree. |
| GET | `/file/{index}/tags/select` | One `TagNode`. Query: `path`, `offset`, `limit`. |
| GET | `/file/{index}/annotations` | `EmbedRoiAnnotations` for one file. |
| PUT | `/file/{index}/annotations` | Replaces one file's annotations with a JSON `EmbedRoiAnnotations` body and returns the canonical result. |
| GET | `/file/{index}/redactions` | `EmbedRoiAnnotations`: one file's redaction boxes. |
| PUT | `/file/{index}/redactions` | Replaces one file's redaction boxes with a JSON `EmbedRoiAnnotations` body and returns the canonical result. |
| PUT | `/file/{index}/redactions/series` | `RedactionSeriesResponse`: copies the file's redaction boxes to every file of its series with the same rows and columns, and lists those files. Takes no body. |
| GET | `/annotations/export.csv` | `text/csv; charset=utf-8` with `Content-Disposition: attachment; filename="dcmview-annotations.csv"`. |

Every success status is `200`.

### Decode admission

Display, raw, raw-pixel, thumbnail and presentation-layer requests, segmentation
and value overlays (PNG and values), and the semantic context of an RT Dose or
Parametric Map may answer `503 decode_busy` with `Retry-After: 1` when their
class's decode queue is full, or when the viewer is stopping while they wait
for decode capacity. All but semantic context may also answer
`422 decode_memory_exceeded` when the work needs more than the decode memory
budget (`--decode-memory`), or more than half of it for a thumbnail. An
overlay is one piece of work: the frames it decodes, the resampling and the
encoding are reserved together, by the overlay's object and by the size of
the displayed frame it is drawn on, so an overlay may be refused on a large
displayed frame although every frame involved can be shown on its own.

Neither refusal is cached, and neither changes the file's `support_state`.
After a 503, wait the `Retry-After` number of seconds and repeat the request.
Do not repeat a 422 in the same session: its message names the required bytes
and `--decode-memory`; restart with a larger budget. Cache hits take no decode
permit and remain available while the budget is held. No other endpoints
answer these two admission errors.

## Files And Scan Progress

`/api/files` is available before the scan finishes. Its progress fields are:

| Field | Meaning |
|---|---|
| `scan_complete` | `true` after every requested path has been scanned. |
| `scanned` | DICOM and image files accepted into the registry. |
| `skipped` | Unrecognized or unreadable files, unsupported DICOM objects, invalid raster headers, and recognized formats excluded by `--formats`. |
| `filtered` | Readable files excluded by `--filter`. |
| `discovery` | Up to 256 recent skipped or filtered paths (`path`, `disposition` of `skipped` or `filtered`, `reason`). Accepted files appear only in `files`; totals stay in the counters above. |

Poll while `scan_complete` is `false` if you need the complete file list.

Each file summary carries identity and geometry fields plus
`support_state` (`renderable`, `metadata_only`, or `unsupported`) and a stable
`support_reason` such as `transfer_syntax.not_supported`. These describe what
the viewer can do, not DICOM conformance. `raw_windowing_compatible` said
whether client-side windowing would drop a presentation transform; the value
mapping (Modality and VOI LUTs) and `presentation-layer` (shutter and overlays)
now let a client reproduce every one, so it is always `true` and
`raw_windowing_reason` always `null`. `presentation_layer` is `true` when
grayscale display frames carry a display shutter or overlay graphics; neither
depends on the window, so `presentation-layer` drawn over a frame windowed in
the browser gives exactly the display frame for that window.

### File keys

Every file has a session-scoped identity independent of its discovery index.
`file_key` has three states:

| Value | Meaning |
|---|---|
| Left out | The key is `sop:` followed by the entry's `sop_instance_uid`. Ordinary DICOM entries need no extra member. |
| `null` | No key is available yet; the file needs a whole-file digest. |
| A string | The file's explicit key. |

Keys are at most 132 bytes: `sop:<uid>` has a body of 1–128 ASCII letters,
digits, `.`, `-` or `_`, taken as written; `b3:<digest>` has the full BLAKE3
digest of the stored file bytes, as 64 lowercase hex characters. A masked
session builds `sop:` keys from masked UIDs and accepts only that shown form;
`b3:` keys are sent unchanged.

A `sop:` key is built from the data set's SOP Instance UID (0008,0018), not
from the Media Storage SOP Instance UID of the file meta. Discovery reads it
as text and takes the first value when the element holds several separated
by a backslash, without the NUL or spaces that pad the value and without
surrounding white space. What is left is used as written, with no case
folding and no other normalisation, when it is 1 to 128 of the characters
above; a missing element, an empty value and any other value give the file a
`b3:` key. Tooling that computes keys for the same files has to read the UID
the same way to arrive at the same key: this reading is part of the rules
`key_rules` versions.

`alias_of`, when present, is the index of the first entry that held the same
key. Files with the same UID and length provisionally share a key; their
bytes are compared only when a caller needs a settled key. The file asked
about and the first file with the UID are read first. If they differ, each
file of the group takes its content key: the two that were read at once,
the others when they are viewed or asked about, and until then an existing
UID key remains, except that a failed digest removes it. If they agree,
every other file with that UID is read, whichever of them the caller asked
about, and the UID key is settled when all agree. If the first file with
the UID cannot be read, there is nothing to compare the others with: no
file gets the UID key settled while that lasts, a request that needs a key
fails with the first file's `key_error`, and the keys shown stay as they
were. Each such request tries the first file again. The one exception is
two other files of the group that were both asked about and differ from
each other: they cannot both be the first file's image, so the group takes
content keys as it would have with the first file read, and the first file
has `file_key: null` until it can be read.

A settled key is final: every `b3:` key, and a `sop:` key once it has been
returned for a write or an export. It stays the file's key for the rest of
the session and is never replaced. A file with the same UID that is found
afterwards has `file_key: null` until it has been compared with the first
file that carries the UID, and then shares the key or takes a content key
of its own; so does a file of the group that could not be read when the key
was settled. The key an entry or `X-File-Key` shows is the key as it
stands and may not be settled yet.

A `file_key: null` does not always mean a digest is on its way. When the
first file that carries the UID cannot be read after its key was returned,
a copy found afterwards has been read but cannot be compared: its entry
shows `file_key: null` with no `key_error` of its own while `keys_hashing`
is 0, and stays so for the rest of the session unless a later key request
reads the first file. The `key_error` is on the first file's entry, which
keeps its key.

`key_error`, when present, is `unreadable` or `changed`. `changed` means the
file is not the one discovery saw: its length or its modification time
differs, or it changed while it was read. A file rewritten with other bytes
of the same length is recognised by its modification time alone. A later
explicit key request retries a failed digest.

Discovery performs no reads for keys. A file needing a digest starts hashing
in the background after its first successful display or raw frame, or when a
caller explicitly requests its settled key. That holds for an image file as
for a DICOM file without a usable UID: its frames decode like any other, the
first one served starts its hashing, and later ones carry its key. Thumbnails, pixel probes and
frames that fail do not start hashing and carry no `X-File-Key`.
Display and raw frame responses include `X-File-Key` once a key is available;
the frame response never waits for hashing. The current viewer needs no change.

### Catalog revisions

`revision` starts at 0 and rises once for every inserted entry or change to
its `file_key`, `alias_of` or `key_error`. A plain `/api/files` request returns
all entries in index order. `since=<revision>` returns entries inserted or
updated after that revision, each as it stands now, least recently changed
first. `limit=<count>` caps the page; with `limit` alone, `since` defaults to
0. A zero limit returns `400 invalid_query`.

`more: true` means entries remain, and the returned `revision` is the cursor
for the next page. Otherwise it is the current catalog revision. An entry
updated during paging can appear again; replace the client's entry at that
index. A `since` greater than the current catalog revision returns
`reset: true` and starts from 0, so the client can rebuild its catalog.
Otherwise `reset` is false. `keys_hashing` counts queued files plus the file
currently being hashed. Key changes can arrive after `scan_complete`.

One response is one moment of the catalog: its entries, `scan_complete`,
the counters, `discovery` and `keys_hashing` are read together. A response
with `scan_complete: true` lists every file the scan found, and a file that
is queued for hashing when the entries are read is counted.

`rekeys` contains replacements after the requested cursor and through the
returned revision, oldest first. Each has `revision`, `index`, `old_key` and
`new_key`; a file replaces its key at most once, a `sop:` key by a `b3:`
key, and never once the key has been settled. The replacement takes the
revision of its changed entry. A plain request includes all replacements.
An old key continues to name its first holder for the rest of the session,
which can be another file than `index`: apply a replacement to what is held
for the file `index`, not to everything held under `old_key`.
Existing clients may keep requesting the full catalog without a cursor.

### Raster image summaries

`FileSummary.file_format` is `dicom`, `png`, `jpeg`, `tiff`, or `webp`, detected
from content. `raster` is `null` for DICOM and the following object for images.
Rasters have `object_kind: "image"`, empty DICOM identity and transfer-syntax
strings, `has_pixels: true`, and `support_state: "renderable"` unless a
`raster.*` reason applies. Unsupported rasters remain listed, with the first
applicable `support_reason`:

- `raster.unsupported_color`: TIFF CIELab, more than four bands, an extra
  sample that is not alpha, palette, CMYK, gray with alpha, YCbCr outside
  JPEG compression, or color stored as separate planes.
- `raster.unsupported_sample_format`: TIFF 16-bit float, 1-, 2- or 4-bit or
  64-bit integer samples, or color other than 8- or 16-bit unsigned.
- `raster.unsupported_compression`: TIFF compression other than none, LZW,
  Deflate or PackBits, including JPEG-compressed TIFF.
- `raster.jpeg_unsupported_process`: JPEG other than 8-bit baseline, extended
  sequential or progressive Huffman.
- `raster.too_large`: a frame above 268,435,456 pixels, independent of host
  memory and cache budget.
- `raster.file_too_large`: a PNG, JPEG or WebP longer than its read budget
  (64 MiB plus four times its decoded frame). It is listed as unsupported,
  rather than failing every frame request with 500. TIFF reads individual
  pages and has no whole-file length cap.

Dimensions and coordinates remain in the stored pixel grid. For gray rasters,
`default_window` covers the full stored range of integer samples of 8 bits or
fewer, or a TIFF's declared `MinSampleValue`/`MaxSampleValue` range. Otherwise
it is `null` and display uses the frame's percentiles. PNG `sBIT` does not
narrow the window.

| `RasterSummary` field | Meaning |
|---|---|
| `color_type` | Stored `gray`, `gray_alpha`, `rgb`, `rgba`, `palette`, `cmyk`, or `other` for an unsupported TIFF color layout. |
| `bit_depth` | Stored bits per sample, including low-bit palette indices. |
| `sample_format` | `uint`, `int`, or `float`. |
| `has_alpha` | Alpha channel or transparency, including PNG `tRNS`. |
| `orientation` | EXIF/TIFF orientation 1–8; defaults to 1. Reported, never applied to dimensions or coordinates. |
| `has_icc` | Whether an ICC profile is embedded. |
| `pages_total` | TIFF pages inspected; 1 for PNG, JPEG, and WebP. |
| `frame_pages` | Zero-based TIFF page index per frame; `[0]` for other formats. Its length is `frame_count`. |
| `excluded_pages` | At most 16 objects with `page` and `differs`: first difference in order `reduced_resolution`, `mask`, `width`, `height`, `samples_per_pixel`, `sample_format`, `bits_per_sample`, `photometric`, `alpha`, `orientation`. |
| `excluded_pages_total` | Count of all excluded TIFF pages, including those not listed. `pages_total` equals this plus `frame_pages.length`. |
| `animated` | APNG or animated WebP; only the first frame is represented. |
| `significant_bits` | PNG `sBIT` bytes in stored channel order, or `null`; informational only. |

TIFF page 0 is always frame 0. Later pages join the frame map only if their
layout and orientation match page 0 and they are neither reduced-resolution
nor mask pages. A damaged later IFD ends the walk; earlier pages remain listed.
A TIFF may have at most 65,535 pages; a longer chain is skipped as
`raster_header_invalid`, rather than listed with a truncated frame map.
A page that holds any tag twice cannot be read: as the first page it makes
the file `raster_header_invalid`, and as a later page it ends the walk.

Discovery reasons include `valid_image` for accepted rasters,
`unrecognized_format` for content that is neither DICOM nor a recognized image
(replaces `missing_part10_preamble`), `format_not_selected` for a recognized
format excluded from a directory walk, and `raster_header_invalid` for a
recognized image whose header cannot be read within the fixed scan budget. Accepted files are represented
in `files`, not the bounded skipped/filtered `discovery` list.

For a raster file index, the following responses require no DICOM parsing:

| Endpoint suffix under `/api/file/{index}` | Raster response |
|---|---|
| `/frame/{frame}`, `/frame/{frame}/thumbnail`, `/frame/{frame}/raw`, `/frame/{frame}/raw/pixel`, `/frame/{frame}/presentation-layer` | As for DICOM for a renderable raster. A refused raster returns `422 unsupported_pixel_layout` naming its reason, without opening the file; a damaged file that cannot decode returns `500` in the shared JSON error envelope. A renderable file may answer `422 decode_memory_exceeded` or `503 decode_busy`; neither is cached or changes `support_state`. Follow the [decode admission retry rules](#decode-admission). Missing pixels and out-of-range frames are checked first; the latter is `404 frame_out_of_range`. |
| `/tags` | `200` with the file's metadata tree; see [Raster metadata](#raster-metadata). A file that is damaged, or that holds more than is shown, still answers `200`, with `Note` leaves. `404` when the file is gone. |
| `/tags/select` | One node of that tree by its path; `400 bad_request` for a path that names nothing. |
| `/references` | `200` with `source_file_index`, empty `source_sop_instance_uid`, and `references: []`. Rasters are never reference targets. |
| `/semantic-context` | `200`, `context.kind: "not_applicable"` with a reason, `default_mode: "pixel_preview"`, and `pixel_preview_preserves_stored_values: true`. |
| `/frame/{frame}/value-mapping` | `200` identity: `stored_value_type` is `integer`, `float32`, or `float64`; `modality` has slope 1, intercept 0, `rescale_type: null`, `lut: null`; `real_world: []`, `voi_lut: null`. Frame range is checked first. |
| `PUT /redactions/series` | `200` with `file_indices: []`; no boxes are copied because rasters belong to no series. |

Rasters are absent from `/api/series`. DICOM-only overlay, WSI, and presentation
state operations retain their existing wrong-kind errors rather than attempting
to open an image as DICOM. Per-file ROI and redaction storage is available,
and redactions apply to display, raw, presentation-layer and thumbnail pixels.

## Thumbnails

`GET /api/file/{index}/frame/{frame}/thumbnail` returns a JPEG of the whole
frame. `size` requests a longest edge in device pixels, snapped up to
128, 256, 512 or 1024; omitted means 256. Zero, values above 1024 and malformed
sizes return `400 invalid_query`. `window_mode` is `default` (also when
omitted) or `full_dynamic`; other values return `400 invalid_query`.

Every successful response carries these three thumbnail/cache headers:

| Header | Values |
|---|---|
| `X-Cache` | `MISS` for a new render; `HIT` for a cached thumbnail or a shared render already underway. |
| `X-Thumbnail-Source` | `full_decode` on a miss, `thumbnail_cache` on a hit or shared render. `display_cache`, `raw_cache` and `reduced_decode` are reserved and are not sent today. |
| `Cache-Control` | `no-store`: file indexes belong to one process, so the same URL after a restart could refer to a different file. |

The thumbnail uses the default window (DICOM window, else VOI LUT, else the
automatic window), or the full dynamic range when requested. Modality
transforms, MONOCHROME1 inversion, padding and palette/colour conversion
match the display path. It omits the shutter and overlay planes, blanks
redaction boxes **before** resampling, then shrinks in display space with an
area filter and encodes at JPEG quality 85, without an ICC profile. Masked
sessions withhold slide label and overview images just as for display frames.

Geometry stays in the stored pixel grid, without cropping, rotation, flipping
or enlargement. With `ratio = pixel_aspect_ratio` (pixel height over width;
absent, non-finite or non-positive means 1), the dimensions are:

```text
width  = columns * min(1, 1 / ratio)
height = rows    * min(1, ratio)
scale  = min(1, bucket / max(width, height))
thumbnail = (max(1, round(width * scale)), max(1, round(height * scale)))
```

A stored `(row, column)` maps to `(row * thumbnail_height / rows,
column * thumbnail_width / columns)`, with no offset.

Thumbnails share concurrent renders and fill only their own cache. The key
includes file, frame, bucket, window mode and redaction revision, so edits to
redaction boxes invalidate earlier previews. Background decode permits are
awaited inside the request: aborting a queued request starts no work; once a
render starts it finishes and caches its result even after disconnection.

Errors use the shared JSON envelope: `404 not_found`, `404 frame_out_of_range`,
`404 no_pixel_data`, `400 invalid_query`, `403 masked`,
`422 unsupported_transfer_syntax`, `422 unsupported_pixel_layout`,
`422 decode_memory_exceeded`, `503 decode_busy` (with `Retry-After: 1`), or
`500 pixel_decode_failed`. Admission refusals are not cached and do not change
`support_state`: [wait and retry a 503, but do not repeat a 422](#decode-admission).
Thumbnails may reserve only half the decode memory budget between them.
The common authentication requirement also applies.

## Display And Raw Frames

Display, raw, `raw/pixel` and `presentation-layer` requests may answer
`422 decode_memory_exceeded` or `503 decode_busy` with `Retry-After: 1`.
Neither is cached or changes `support_state`; follow the
[decode admission retry rules](#decode-admission): wait the stated seconds
and repeat a 503, but do not repeat a 422 in the same session.

Raster raw frames always contain interleaved stored samples: low-bit PNG gray
values are unscaled, and WhiteIsZero TIFF values are un-inverted with
`MONOCHROME1` so display inverts once. Two samples (`MONOCHROME2`) mean gray
and alpha; four (`RGBA`) mean color with unassociated alpha. Wider samples
are little endian. Orientation never rotates the samples or coordinates.
Display flattens alpha over black, windows gray, and reduces 16-bit color to
8-bit RGB without windowing color. A valid RGB ICC profile of at most 4 MiB
is carried by color display frames; gray and converted CMYK JPEG profiles
are dropped.

Display frames are always decoded server-side and PNG-encoded; the endpoint
never returns compressed DICOM fragments. The window is chosen in this order:

1. `mode=full_dynamic`: current-frame min/max; `wc`/`ww` are ignored.
2. Explicit `wc` and `ww`, which must be sent together with a positive width.
3. DICOM Window Center/Width or the raster `default_window`.
4. The current frame's 1st/99th percentile.

`unit` (with `wc` and `ww`) puts the explicit window in a real-world unit: it
applies to the values of the frame's preferred real-world mapping (the first
entry of its `value-mapping` `real_world`) when that mapping's `unit_label` is
`unit`, the way the viewer's raw renderer windows them: each stored value is
mapped (a LUT mapping, non-monotonic ones included, or a linear one; the
Modality transform is not applied), the window follows the LINEAR function
without its integer half-unit offsets, and stored values outside the mapped
range take the window's low end. The frame's samples must be 8- or 16-bit (or
one-bit) integers. A frame whose preferred mapping has another unit, or whose
samples are not such integers, is shown with its default window (steps 3 and
4). `unit` without `wc` and `ww` is `400 invalid_window`; `mode=full_dynamic`
ignores it.

`preview=true` marks a window/level drag preview: it is served from the display
cache when that window is already there, and otherwise rendered for this
request alone, neither cached nor shared with concurrent requests, so a drag
through many windows does not evict the frames cine and the settled window
use.

The display cache key includes file, frame, `wc`, `ww`, `mode`, and the unit
of a real-world window (windows
that render alike, such as `wc=-0` and `wc=0` or widths below 1, share a key);
the raw cache key is file and frame only. Both endpoints send `X-Cache: HIT` or
`X-Cache: MISS`. A request for a frame another request is already decoding
waits for that decode and reports `HIT`. A decode that has started still
fills the cache when its client disconnects; one that is still waiting for
decode capacity when its last client disconnects is not started. A display request can fill the raw cache
too (grayscale frames are windowed from decoded samples kept there), so a raw
request after a display request of the same frame may report `HIT`.

A grayscale display frame windowed linearly reports the window it was
rendered with, in Modality values, as `X-Frame-Window-Center` and
`X-Frame-Window-Width` (the width as applied), whichever step
above chose it; a drag preview reports its window too. Color frames, frames
presented through a VOI LUT, and frames windowed in a real-world `unit` send
neither: a window over mapped values has no linear Modality equivalent. A
`unit` request whose window could not be applied reports the default window it
was shown with instead.

`X-Frame-Window-Applied` identifies the presentation on every grayscale display
response, including cache hits and previews: `linear` for a Modality window,
`real_world` for an applied unit window, or `voi_lut` for a VOI LUT. It is
omitted for color frames. Only `real_world` confirms a requested unit window
was applied; a fallback can report either `linear` (with center/width) or
`voi_lut` (without them). Integer Modality windows retain a width floor of 1;
float, fractional Modality and real-world windows use the continuous function
without that floor.

A file's `frame_count` in `/api/files` and `/api/series` is its Number of
Frames bounded by the frames it can hold (the Per-frame Functional Groups items
and the pixel data present), so a truncated or mislabelled file lists only the
frames it has; the tag panel still shows the declared Number of Frames.

Raw frames carry decoded samples in little-endian order for client-side
rendering. Metadata headers:

| Header | Meaning |
|---|---|
| `X-Frame-Rows`, `X-Frame-Columns` | Frame size. |
| `X-Frame-Bits-Allocated` | Bits per stored sample. One-bit data keeps `1` but is expanded to one byte per sample. |
| `X-Frame-Pixel-Representation` | `0` unsigned, `1` signed. |
| `X-Frame-Samples-Per-Pixel` | Samples per pixel in the response. |
| `X-Frame-Photometric-Interpretation` | Photometric interpretation for the renderer. |
| `X-Frame-Rescale-Slope`, `X-Frame-Rescale-Intercept` | Modality rescale. |
| `X-Frame-Default-Wc`, `X-Frame-Default-Ww` | Only when the file declares a default window. |
| `X-Frame-Padding-Low`, `X-Frame-Padding-High` | Only for grayscale frames with Pixel Padding (Float or Double Float Pixel Padding for float pixel data): the inclusive stored-value range to exclude from automatic windows and draw black. |

Transfer syntax coverage:

| Transfer syntax | Display | Raw |
|---|---|---|
| JPEG Baseline (`.50`) | PNG | 8-bit grayscale or interleaved RGB. |
| JPEG Lossless (`.57`, `.70`) | PNG; 8-bit RGB, and YBR_FULL converted to RGB. | 8- or 16-bit grayscale. |
| JPEG 2000 Lossless (`.90`) | PNG | 8- or 16-bit grayscale; multi-component is `422`. |
| JPEG-LS Lossless (`.80`) | Grayscale PNG | 8- or 16-bit grayscale. |
| JPEG XL Lossless (`.110`) | PNG; 8-bit RGB and YBR_RCT (RGB after the decoder inverts the RCT), and YBR_FULL converted to RGB. | Unsigned interleaved 8-bit samples: RGB (YBR_RCT frames included), or a YBR_FULL frame's stored channels labelled `YBR_FULL`. |
| RLE Lossless (`.5`) | 8/16-bit monochrome, 8-bit RGB, YBR_FULL, YBR_FULL_422 (full resolution, shown as YBR_FULL), palette color. | Interleaved native samples; YBR_FULL_422 frames are labelled `YBR_FULL`. |
| Deflated Image Frame Compression (`.8.1`) | One-bit monochrome (binary segmentation frames). | One byte per sample (0 or 1), `X-Frame-Bits-Allocated: 1`. |
| Implicit LE, Explicit LE/BE, Deflated Explicit LE | 1/8/16/32-bit monochrome integer, float, double float, RGB (planar 0/1), YBR_FULL, YBR_FULL_422, palette color. | Native samples; planar order kept, padding bits masked, signed values sign-extended. |
| JPEG Extended (`.51`), JPEG 2000 lossy (`.91`), JPEG-LS Near-Lossless (`.81`), JPEG XL `.111`/`.112`, anything else | `422 unsupported_transfer_syntax` | `422` |

Real World Value Mapping and RT Dose Grid Scaling are not applied to display
or raw pixels, including float and double-float data, except that a display
window with `unit` is applied to mapped values; `value-mapping` tells the
client how to convert them.

## Value Mapping

`value-mapping` describes one frame of any object, so a client can read
stored, modality, and real-world values out of a raw frame:

- `stored_value_type`: `integer`, `float32`, or `float64` samples.
- `modality`: the transform the display pipeline applies before windowing,
  the same one the raw-frame rescale headers carry: `rescale_slope`,
  `rescale_intercept`, `rescale_type`, and a `lut` (`first_value_mapped`,
  `values`) that replaces the rescale when present.
- `real_world`: validated conversions that apply to this frame, preferred
  first. A frame's own Real World Value Mapping functional group overrides
  the shared group, which overrides a top-level sequence. Each entry has a
  `transform` (`{kind: "linear", slope, intercept}` or `{kind: "lut",
  values}` indexed by `stored - first_value_mapped`), the inclusive stored
  range `first_value_mapped..=last_value_mapped` (stored values outside it
  have no mapped value), `unit_label` (a UCUM code or unit text), and the
  coded `units` and `quantity`. RT Dose reports its Dose Grid Scaling as
  `source: "dose_grid_scaling"`, a linear map with `unit_label` `Gy` or the
  declared Dose Units. After the file's own mappings come those of separate
  Real World Value Mapping instances in the loaded file set whose Referenced
  Image Real World Value Mapping items name this image in their Referenced
  Image Sequence, for every frame or only the listed Referenced Frame
  Numbers: `source: "rwvm_instance"` with the instance's
  `source_file_index`. The file's own mappings stay preferred. RWVM
  instances that a Parametric Map references itself are also summarized in
  its semantic context.
- `voi_lut`: the VOI LUT (`first_value_mapped`, `bits_per_entry` of 8 or 16,
  `values`) the display path presents Modality values with in default mode
  when no window is requested and no DICOM window is stored; `null` without a
  usable one. A value indexes it as `trunc(value) - first_value_mapped`,
  clamped to the table, and the output scales to 8 bits as
  `(output * 255 + max / 2) / max` in integers (`max = 2^bits_per_entry - 1`).

## Semantic Context, Overlays, And WSI

`semantic-context` interprets declared SEG, Parametric Map, and RT Dose
metadata without changing the ordinary pixel preview. For SEG it returns one
mapping record per frame with `mapping_method`, `mapping_status`
(`resolved`, `missing`, or `ambiguous`), `mapping_reason`, and the resolved
zero-based `source_frames`. A mapping comes from an explicit per-frame
derivation source or, failing that, from compatible patient geometry (same
Frame of Reference, positions, orientation, and spacing); source and SEG
matrices may differ in size. SEG context also includes `warnings: string[]`,
empty when there is nothing to report. Declared `segmentation_type`,
`segmentation_fractional_type`, and `maximum_fractional_value` remain unchanged.

A FRACTIONAL SEG with Maximum Fractional Value above 1 whose stored samples
across the entire object are all 0 or 1 is drawn as BINARY. Its context warns:
“Declared FRACTIONAL (maximum N) but every stored value is 0 or 1; shown as
binary”.
Discovery computes and caches this interpretation before serving the object;
a binary-valued frame within a genuine fractional object does not trigger it.
Ordinary display/raw frames and genuine fractional overlays are unchanged.

`segmentation-overlay` renders only a frame whose mapping resolved. Binary
samples are a mask, fractional samples are scaled by Maximum Fractional Value,
and the mask is nearest-neighbor resampled onto the source frame. Each segment
is painted in its context `display_color`: the Recommended Display CIELab
Value converted from D50 CIELab to sRGB, else the Recommended Display
Grayscale Value as a proportional gray level, else a fixed palette cycled by
segment number; `display_color_source` (`recommended_cielab`,
`recommended_grayscale`, or `palette`) says which. It answers `400` for a
non-SEG object, `404` for an out-of-range frame, and `422
semantic_mapping_unavailable` when the mapping, geometry, or source file is
missing or ambiguous.

Parametric Map and RT Dose contexts report `displayed_value_kind: "stored"`:
the frame endpoints window stored values, and mapped units are the client's
conversion.

`dose-overlay` and `parametric-map-overlay` draw a value volume on a displayed
frame: the path names the displayed image frame and the query names the RT
Dose (`dose`) or Parametric Map (`map`). The volume's frames are a stack of
parallel planes: an RT Dose grid from Image Position/Orientation, Pixel
Spacing, and Grid Frame Offset Vector (relative or absolute form); a
Parametric Map from its per-frame positions, which must share orientation,
spacing, and in-plane origin, one frame per plane. A displayed frame in the
same Frame of Reference, in any orientation, is resampled trilinearly: each
pixel center's patient position is located along the planes' normal and in
their grid, then sampled bilinearly within the two planes that bracket it
and linearly between them. A pixel is inside the volume within half a pixel
of the grid's edge pixel centers and within half a plane spacing beyond an
end plane (which is then used alone). A frame parallel to the planes
brackets the same planes at every pixel; an oblique one brackets them pixel
by pixel. Samples convert through each frame's preferred `value-mapping`
entry (Dose Grid Scaling for RT Dose).

The PNG has the displayed frame's size. Colors follow the context's `legend`:
a value `v` sits at `(v - min_value) / (max_value - min_value)`, clamped,
along the evenly spaced `color_stops` (viridis), interpolated linearly in RGB.
Colored pixels are opaque, so the viewer applies overlay opacity; pixels
outside the volume, without a mapped value, or at or below
`transparent_at_or_below` are transparent. The RT Dose legend spans 0 to the
maximum dose of the whole grid with zero dose transparent; the Parametric Map
legend spans the minimum to maximum mapped value of every frame with no
floor. Both use one scale for every slice. Encoded overlays are cached per
volume, displayed frame and file set (the legend they are colored by is
read against the files loaded, so one drawn before a file was added is
drawn again), and SEG overlays per SEG frame and resolved source frame;
every overlay endpoint's `X-Cache` reports that encoded-PNG cache, not the
decoded frames beneath it. Requests for the same overlay
that arrive while it is being drawn wait for that one drawing and report
`X-Cache: HIT`.

The `/values` form of each value overlay sends the resampled values instead
of colors, so a viewer can read the volume's value under the cursor: one
little-endian `f32` per pixel of the displayed frame, row-major, in the
legend's unit (Gy for RT Dose, the map's unit for a Parametric Map). Pixels
outside the volume or without a mapped value are NaN; no legend floor
applies. It answers the same errors as the colorwash, including `404
overlay_not_covering_frame`, and is cached and reported the same way.

In the semantic context, RT Dose and Parametric Map carry `overlay`
(eligibility), `overlay_source_frames` (the local image frames in the
volume's Frame of Reference that it covers, in file and frame order, at most
4096), and `legend` (present only when eligible). `overlay.source_file_index`
is the declared source image when exactly one covered file is declared, or
the only covered file. The Parametric Map overlay also needs a usable
mapping on every frame, all in one unit. A volume whose frames cannot be
decoded, or a dose grid without positive dose, is ineligible. The legend
spans the values of every frame of the volume; finding that range is
reserved against the decode memory budget as one piece of work, and
requests that ask for it meanwhile share it. A volume whose range needs
more than the budget is ineligible with the memory-budget reason. If the
viewer instead refuses that work because it is busy, the legend is not
computed: semantic context answers `503 decode_busy` with `Retry-After: 1`,
and the incomplete context is not cached as ineligible. Wait that many
seconds and repeat the request.

The overlay endpoints answer `400` when the query names the wrong kind of
object, `404` for an unknown file index or out-of-range frame, `404
overlay_not_covering_frame` when no pixel center of the displayed frame
lies inside the volume, and `422 semantic_mapping_unavailable` when the
volume is ineligible or the displayed frame lies in another Frame of
Reference or lacks geometry. Segmentation and value overlays also follow
[decode admission](#decode-admission): `503 decode_busy` with `Retry-After: 1`
or `422 decode_memory_exceeded`, neither cached nor changing `support_state`.
Wait and repeat a 503; do not repeat a 422 in the same session. An overlay
whose frames cannot be decoded, or that fails while it is encoded, answers
`500 pixel_decode_failed`.

`wsi-context` positions one tile of a Whole Slide Microscopy object in its Total
Pixel Matrix without stitching. It answers `400` for other objects.

## Tags

`/tags` returns a preview tree. Binary values are reported by byte length, long
numeric arrays and sequences may carry `truncated` and `total`, and a value
that fails to serialize becomes `{"type": "error", "message": "..."}` without
failing the response.

Neither endpoint reads a value it reports by its length, so both cost the
same for a file of any size. Reported as `{"type": "binary", "length": N}`,
with the length the element declares, are Pixel Data, every value of a bulk
binary representation (OB, OW, OD, OF, OL, UN) and any other value longer
than 1 MiB, whatever its representation; encapsulated Pixel Data reports the
bytes of its fragments. A tree ends early in two cases, without an error:
when the data set ends inside a value that is not read (a truncated file),
that element is the last one listed; and the tree of a Deflated Explicit VR
Little Endian data set is read within 64 MiB of inflated data, so when its
pixel data is larger than that, elements behind the pixel data are not
listed. Both endpoints answer `500` for a file that cannot be read within
the limits discovery applies (see
[troubleshooting](troubleshooting.md#files-are-reported-as-skipped)), which
discovery would not have listed.

`/tags/select?path=...` reads one element directly from the file, without the
preview's depth and item caps. `path` alternates tags and zero-based item
indices, such as `(0008,2218)/69/(0008,0100)`. For a sequence, `offset`
(default 0) and `limit` (default 64, at most 256) page its items.

### Raster metadata

For a raster image file `/tags` returns the file's metadata in the same
`TagNode` shape. A node is a leaf, or a group: a `sequence` value with one
item, the group's children, and an empty `vr` (a DICOM sequence has `SQ`).
`tag`, `keyword` and `vr` are names dcmview chooses and never hold bytes of
the file. The tree is one per file, in this order:

| Nodes | What they are |
|---|---|
| `File` leaves | From the catalog entry: `Format`, `Size` (bytes), `Extension` (lower case, when the name has a short alphanumeric one), `Pages`, `Frames`, and for a TIFF with pages that are not frames `ExcludedPages` and one `ExcludedPage` (`page 3: width`) for each the catalog lists. |
| Container leaves | `tag` names the source, `keyword` the field. PNG: `PNG:IHDR` (`Width`, `Height`, `BitDepth`, `ColorType`, `Compression`, `Filter`, `Interlace`), `PNG:pHYs` (`PixelsPerUnitX`, `PixelsPerUnitY`, `Unit`), `PNG:gAMA` `Gamma`, `PNG:cHRM` `Chromaticities`, `PNG:sRGB` `RenderingIntent`, `PNG:sBIT` `SignificantBits`, `PNG:tIME` `Time`, `PNG:acTL` (`Frames`, `Plays`), `PNG:iCCP` `ProfileName`, and `PNG:tEXt`, `PNG:zTXt`, `PNG:iTXt` `Text` (`keyword: text`), each text chunk a leaf. JPEG, up to its first scan: `JPEG:JFIF` (`Version`, `Units`, `XDensity`, `YDensity`), `JPEG:Adobe` (`Version`, `Transform`), `JPEG:COM` `Comment` for each comment, `JPEG:SOFn` (`Precision`, `Height`, `Width`, `Components`). WebP: `WEBP:VP8X` (`Flags`, `CanvasWidth`, `CanvasHeight`) or `WEBP:VP8` / `WEBP:VP8L` (`Width`, `Height`), and `WEBP:ANIM` `LoopCount`. Numbers are the stored integers. |
| `TIFF:page N` groups | One for each of a TIFF's first 16 pages: its entries in file order, then `Exif`, `GPS` and `Interop` groups for the directories the page points to. |
| `EXIF` group | The EXIF block of a PNG, JPEG or WebP: groups `IFD0`, `Exif`, `GPS`, `Interop`, `IFD1`, each present when the block has it. |
| `XMP` leaf | `Packet`: the start of a JPEG's or WebP's XMP packet, as text. |
| `ICC` leaves | `Size`, `Version`, `Class`, `ColorSpace` and `Description` of the embedded profile. |
| `Note` leaves | One sentence each: a note discovery made, a name whose extension belongs to another format, an animation of which the first frame is shown, a part that is damaged, a limit that was reached. Fixed words and numbers. |

A directory entry (in a page or an EXIF directory) has its tag number as
`tag` (`0x010F`), the tag's name as `keyword` (`Unknown` when dcmview has
none) and its TIFF type as `vr` (`ASCII`, `SHORT`, `RATIONAL`, ...). Its value
is a `string` for text, a `number` or `numbers` for integer and float types,
a `string` of `numerator/denominator` pairs for rationals, `binary` with its
length for `UNDEFINED` data (a maker note, a thumbnail, IPTC and Photoshop
blocks are never parsed), and `error` for a value that lies outside the file,
has an unknown type or is not a finite number.

Limits, the same for every file: at most 1,024 nodes, three levels deep; a
text value shows at most 1,024 characters and then `…`, and a tree at most
128 KiB of text; a numeric value shows at most 64 numbers and states its
`total`; a directory shows its first 256 entries. Text is decoded (invalid
bytes become U+FFFD), trailing NULs and white space are dropped, tabs and
line ends become spaces, and these are written as `\u{..}` with the code
point in hex: every other control character (U+0000 to U+001F, U+007F to
U+009F), the line and paragraph separators (U+2028, U+2029), every format
character (Unicode general category Cf, which holds the soft hyphen, the
zero-width and bidirectional characters, the byte order mark and the tag
characters), and every other code point Unicode makes ignorable by default
(the variation selectors U+180B to U+180D, U+180F, U+FE00 to U+FE0F and
U+E0100 to U+E01EF, the combining grapheme joiner, the Hangul fillers, and
the rest of U+E0000 to U+E0FFF; U+180E, the Mongolian vowel separator, is a
format character). A variation selector or a joiner inside an emoji
sequence is therefore shown escaped too.

`/tags/select?path=...` returns one node of this tree; nothing more is read
from the file. `path` is steps separated by `/`, at most three. A step is a
node's `tag`, optionally `.` and its `keyword`, optionally `[n]` for the
`n`-th node (from zero) of that level that matches; without `[n]`, the
first. Examples: `PNG:IHDR.Width`, `PNG:tEXt[2]`, `EXIF/GPS/0x0002`,
`TIFF:page 1/0x0100`. A group is returned with its children, and `offset`
and `limit` page its one item as they page a sequence's; a leaf takes no
`offset`. A path longer than 256 bytes, a step that matches nothing, a step
through a leaf and a `limit` outside 1 to 256 are `400 bad_request`.

## Annotations

Annotations are EMBED-style rectangles held in memory:

```json
{ "num_roi": 1, "roi_coords": [[120, 340, 220, 430]], "roi_frames": [[0, 1, 2]] }
```

`PUT` validates coordinates and frame indices against the file and answers
`400 bad_request` when they do not fit. The CSV export is built from the
current in-memory store; source DICOM and CSV files are never modified. See
[annotations](annotations.md) for the CSV format.

## Redaction Boxes

Redaction boxes are rectangles of a file's frames that are withheld, drawn by
hand over burned-in text. They use the `EmbedRoiAnnotations` shape and
validation: `roi_coords` are `[row0, column0, row1, column1]` with exclusive
ends, and `roi_frames` is empty (every box covers every frame) or lists each
box's zero-based frames. They live in memory for the session and are not part
of the annotation CSV export.

Display, raw and thumbnail endpoints apply the boxes of the requested frame:

- Display frames paint them black. The boxes' revision is part of the display
  cache key, so `X-Cache` is `MISS` for the first request after a change.
- Thumbnails paint them black before resampling. The boxes' revision is part
  of the thumbnail cache key too.
- Raw frames, and `raw/pixel`, fill them with one stored value: the frame's
  darkest (its largest for MONOCHROME1), or black for color. Automatic windows
  computed from the samples are therefore unchanged.
- `presentation-layer` paints them opaque black, over any overlay graphics.

`PUT /file/{index}/redactions/series` gives each copy the source's frames when
the file has as many frames as the source; otherwise its boxes cover every
frame.

## Masked Sessions

A session started with `--mask` reports `masked: true` in `/health` and
`/files` and masks every response for the life of the process:

- File summaries carry a pseudonym in `patient_id` and `patient_name`, a
  shifted `study_date`, hashed UIDs, a `label` built from those, and a
  synthetic `display_name` (`File N`). `path` is unchanged: the directory tree
  shows it.
- Every field named `*_uid` or `*_uids` holds a `2.25.` UID of a keyed hash;
  UIDs the standard registers (`1.2.840.10008.*`) are unchanged. The hash is
  the same in every response, including the tag tree, so identifiers still
  match across endpoints. Series and stack `id` values are built from hashed
  UIDs.
- Tag values follow the masking rules: private elements (except private
  creators) and attributes of the PS3.15 Table E.1-1 basic profile are
  `[masked]`, `PN` values are `[masked]` (Patient's Name is the pseudonym),
  `DA` and `DT` values are shifted, `AS` values are capped at `089Y`, `UI`
  values are hashed, and every value inside a sequence the profile lists is
  treated as listed. Study Description, Series Description and the patient
  characteristics (sex, age, size, weight) are kept.
- A presentation state's `content_creator_name` is `[masked]`, its
  `presentation_creation_date` is shifted, and its text objects are withheld
  and counted in `skipped.masked_text`.
- A raster image file's metadata tree keeps its shape and shows only listed
  values that describe the pixel grid and its encoding: the `File` leaves
  except `Extension`, the notes, the container's layout fields (`PNG:IHDR`,
  `pHYs`, `gAMA`, `cHRM`, `sRGB`, `sBIT`, `acTL`; `JPEG:JFIF`, `JPEG:Adobe`
  and the frame header; the WebP headers and loop count), a profile's `Size`
  and `Version`, and of directory entries the layout ones (sizes, sample
  layout, compression, strips and tiles, resolution, orientation, colour
  space, pixel dimensions, and the pointers to other directories). Of a
  directory a masked tree shows one value for each listed field and
  nothing else: the first entry of that tag, when it has a type the field
  is defined with and no more numbers than the field holds (one for a
  size, a scalar field, and strip and tile offsets and byte counts; the
  field's fixed count otherwise; at most four where there is a number for
  each sample). A repeated, longer or differently typed entry is
  `[masked]` whole. The values shown are still numbers the file chose:
  masking is a display aid for honest files, not a guarantee against a
  file built to carry something in them. Every other value is `[masked]`:
  unknown tags, colour maps, all text of the file, GPS, dates and times
  (masked, not shifted), device, software and host names, serial
  numbers, owner, artist, copyright, description and comment fields, PNG
  text chunks, XMP, maker notes, thumbnail entries, profile names and
  descriptions, document and page names, and exposure settings.
  `/tags/select` selects from the masked tree. A raster's catalog entry and
  the errors of these endpoints hold no text of the file.
- Slide label and overview images (Image Type value 3 `LABEL` or `OVERVIEW`)
  report `support_state: unsupported` with `support_reason:
  masked_label_image`, and their frame endpoints answer `403` `masked`.

`burned_in_annotation` in a file summary is `true` when the file declares
Burned In Annotation `YES`; masking does not change pixels.

## Errors

Every API failure, including malformed paths, queries, and JSON bodies, returns
the JSON envelope:

```json
{ "code": "not_found", "error": "file index out of range" }
```

Branch on `code`; `error` is diagnostic text and may change.

| Status | Codes |
|---|---|
| `400` | `invalid_path`, `invalid_query`, `invalid_json` (malformed body), `bad_request`, `invalid_window` |
| `401` | `unauthorized` (missing, malformed, or incorrect bearer token; `WWW-Authenticate: Bearer`) |
| `403` | `masked` (content a `--mask` session withholds) |
| `404` | `not_found`, `route_not_found`, `asset_not_found`, `no_pixel_data`, `frame_out_of_range`, `overlay_not_covering_frame` |
| `405` | `method_not_allowed` |
| `413` | `invalid_json` (a JSON body over 2 MiB, about 200,000 ROIs in one annotation edit) |
| `415` | `invalid_json` (missing `Content-Type: application/json`) |
| `422` | `invalid_json` (valid JSON of the wrong shape), `unsupported_transfer_syntax`, `unsupported_pixel_layout` (display frames for any layout the catalog marks unsupported; raw frames for invalid geometry or numeric precision), `semantic_mapping_unavailable`, `decode_memory_exceeded` (decode estimate exceeds the budget or thumbnail share; increase `--decode-memory`) |
| `500` | `pixel_decode_failed`, `internal_error` |
| `503` | `decode_busy` (decode queue full; `Retry-After: 1`) |

A failed decode affects only that request; the server keeps running, and logs
the failure (with the request) to stderr. The `error` of a failed decode is
the viewer's own wording: `frame decode failed` or `raw frame decode failed`,
followed by a reason only where the viewer states one in fixed words and
numbers, as it does for the cases below. It never repeats what a decoding
library said of the file, because such a message can quote the file; the
frame, raw frame, raw pixel, thumbnail and overlay endpoints answer the same
for any two files that fail at the same step. `unsupported_transfer_syntax`
names the UID only when it is written as a UID is. The library's account is
written to the log at debug level (`RUST_LOG=dcmview=debug`), with the
request, except in a `--mask` session, which logs nothing a file holds; for
PNG, JPEG, TIFF and WebP files the log names the kind of failure and not the
library's text. A compressed frame whose pixel data
declares a different image than the file's header (another size, component
count or sample depth, or a tile, packet or scan structure beyond the fixed
limits) is such a failure: `500 pixel_decode_failed` with "pixel data
disagrees with the header" in the message, on every endpoint that decodes
the frame. The file stays listed and its other frames decode. A file deleted or moved after
discovery answers `404 not_found` naming its path.

JSON, CSV and the viewer's scripts and styles are gzip-compressed for clients
that send `Accept-Encoding: gzip`; PNG frames, raw samples and fonts are not.
