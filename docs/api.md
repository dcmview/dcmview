# dcmview Internal HTTP API

The browser UI talks to the local Rust server through this API. It is internal
to the viewer and meant for `dcmview` debugging, smoke tests, and local
automation. It is not a stable public integration contract.

The server is unauthenticated. Keep it bound to loopback and use SSH forwarding
for remote work. Responses can expose DICOM metadata, file paths, annotations,
and pixel data.

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

| Method | Path | Success response |
|---|---|---|
| GET | `/health` | `HealthResponse`: `status: "ok"`, viewer name/version/build identity, `file_count`, `server_start_ms`. |
| GET | `/files` | `FilesResponse`: file summaries plus scan progress. |
| GET | `/series` | `SeriesCatalogResponse`: logical series and ordered frame stacks. |
| GET | `/file/{index}/info` | `FrameInfo` for one file. |
| GET | `/file/{index}/references` | `ReferenceCatalogResponse`: declared DICOM relationships and their local matches. |
| GET | `/file/{index}/semantic-context` | `SemanticContextResponse`: SEG, Parametric Map, or RT Dose context, or `not_applicable`. |
| GET | `/file/{index}/frame/{frame}` | Display frame as `image/png`, with `X-Cache`. Query: `wc`, `ww`, `mode`. |
| GET | `/file/{index}/frame/{frame}/raw` | Decoded samples as `application/octet-stream`, with `X-Cache` and `X-Frame-*` metadata headers. |
| GET | `/file/{index}/frame/{frame}/segmentation-overlay` | Transparent source-sized SEG mask as `image/png`, with `X-Cache`. |
| GET | `/file/{index}/frame/{frame}/wsi-context` | `WsiFrameContextResponse`: position of one Whole Slide Microscopy tile. |
| GET | `/file/{index}/tags` | `TagNode[]`: preview tag tree. |
| GET | `/file/{index}/tags/select` | One `TagNode`. Query: `path`, `offset`, `limit`. |
| GET | `/file/{index}/annotations` | `EmbedRoiAnnotations` for one file. |
| PUT | `/file/{index}/annotations` | Replaces one file's annotations with a JSON `EmbedRoiAnnotations` body and returns the canonical result. |
| GET | `/annotations/export.csv` | `text/csv; charset=utf-8` with `Content-Disposition: attachment; filename="dcmview-annotations.csv"`. |

Every success status is `200`.

## Files And Scan Progress

`/api/files` is available before the scan finishes. Its progress fields are:

| Field | Meaning |
|---|---|
| `scan_complete` | `true` after every requested path has been scanned. |
| `scanned` | Valid DICOM files accepted into the registry. |
| `skipped` | Files that could not be read as supported DICOM objects. |
| `filtered` | Readable files excluded by `--filter`. |
| `discovery` | Up to 256 recent discovery entries (`path`, `disposition`, `reason`). Totals stay in the counters above. |

Poll while `scan_complete` is `false` if you need the complete file list.

Each file summary carries identity and geometry fields plus
`support_state` (`renderable`, `metadata_only`, or `unsupported`) and a stable
`support_reason` such as `transfer_syntax.not_supported`. These describe what
the viewer can do, not DICOM conformance. `raw_windowing_compatible` is `false`
when client-side windowing would drop a presentation transform, and
`raw_windowing_reason` then says why.

## Display And Raw Frames

Display frames are always decoded server-side and PNG-encoded; the endpoint
never returns compressed DICOM fragments. The window is chosen in this order:

1. `mode=full_dynamic`: current-frame min/max; `wc`/`ww` are ignored.
2. Explicit `wc` and `ww`, which must be sent together with a positive width.
3. DICOM Window Center/Width.
4. The current frame's 1st/99th percentile.

The display cache key includes file, frame, `wc`, `ww`, and `mode`; the raw
cache key is file and frame only. Both endpoints send `X-Cache: HIT` or
`X-Cache: MISS`.

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
| `X-Frame-Padding-Low`, `X-Frame-Padding-High` | Only for grayscale integer frames with Pixel Padding: the inclusive stored-value range to exclude from automatic windows and draw black. |

Transfer syntax coverage:

| Transfer syntax | Display | Raw |
|---|---|---|
| JPEG Baseline (`.50`) | PNG | 8-bit grayscale or interleaved RGB. |
| JPEG Lossless (`.57`, `.70`) | PNG; 8-bit RGB, and YBR_FULL converted to RGB. | 8- or 16-bit grayscale. |
| JPEG 2000 Lossless (`.90`) | PNG | 8- or 16-bit grayscale; multi-component is `422`. |
| JPEG-LS Lossless (`.80`) | Grayscale PNG | Unsigned 8-bit grayscale. |
| JPEG XL Lossless (`.110`) | PNG; 8-bit RGB, and YBR_FULL converted to RGB. | Unsigned interleaved 8-bit samples: RGB, or a YBR_FULL frame's stored channels labelled `YBR_FULL`. |
| RLE Lossless (`.5`) | 8/16-bit monochrome, 8-bit RGB, YBR_FULL, YBR_FULL_422 (full resolution, shown as YBR_FULL), palette color. | Interleaved native samples; YBR_FULL_422 frames are labelled `YBR_FULL`. |
| Implicit LE, Explicit LE/BE, Deflated Explicit LE | 1/8/16/32-bit monochrome integer, float, double float, RGB (planar 0/1), YBR_FULL, YBR_FULL_422, palette color. | Native samples; planar order kept, padding bits masked, signed values sign-extended. |
| JPEG Extended (`.51`), JPEG 2000 lossy (`.91`), JPEG-LS Near-Lossless (`.81`), JPEG XL `.111`/`.112`, anything else | `422 unsupported_transfer_syntax` | `422` |

Real World Value Mapping is not applied to display or raw pixels, including
float and double-float data.

## Semantic Context, SEG Overlay, And WSI

`semantic-context` interprets declared SEG, Parametric Map, and RT Dose
metadata without changing the ordinary pixel preview. For SEG it returns one
mapping record per frame with `mapping_method`, `mapping_status`
(`resolved`, `missing`, or `ambiguous`), `mapping_reason`, and the resolved
zero-based `source_frames`. A mapping comes from an explicit per-frame
derivation source or, failing that, from compatible patient geometry (same
Frame of Reference, positions, orientation, and spacing); source and SEG
matrices may differ in size.

`segmentation-overlay` renders only a frame whose mapping resolved. Binary
samples are a mask, fractional samples are scaled by Maximum Fractional Value,
and the mask is nearest-neighbor resampled onto the source frame. Colors come
from a fixed per-segment palette. It answers `400` for a non-SEG object, `404`
for an out-of-range frame, and `422 semantic_mapping_unavailable` when the
mapping, geometry, or source file is missing or ambiguous.

`wsi-context` positions one tile of a Whole Slide Microscopy object in its Total
Pixel Matrix without stitching. It answers `400` for other objects.

## Tags

`/tags` returns a preview tree. Binary values are reported by byte length, long
numeric arrays and sequences may carry `truncated` and `total`, and a value
that fails to serialize becomes `{"type": "error", "message": "..."}` without
failing the response.

`/tags/select?path=...` reads one element directly from the file, without the
preview's depth and item caps. `path` alternates tags and zero-based item
indices, such as `(0008,2218)/69/(0008,0100)`. For a sequence, `offset`
(default 0) and `limit` (default 64, at most 256) page its items.

## Annotations

Annotations are EMBED-style rectangles held in memory:

```json
{ "num_roi": 1, "roi_coords": [[120, 340, 220, 430]], "roi_frames": [[0, 1, 2]] }
```

`PUT` validates coordinates and frame indices against the file and answers
`400 bad_request` when they do not fit. The CSV export is built from the
current in-memory store; source DICOM and CSV files are never modified. See
[annotations](annotations.md) for the CSV format.

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
| `404` | `not_found`, `route_not_found`, `asset_not_found`, `no_pixel_data`, `frame_out_of_range` |
| `405` | `method_not_allowed` |
| `415` | `invalid_json` (missing `Content-Type: application/json`) |
| `422` | `invalid_json` (valid JSON of the wrong shape), `unsupported_transfer_syntax`, `unsupported_pixel_layout`, `semantic_mapping_unavailable` |
| `500` | `pixel_decode_failed`, `internal_error` |

A failed decode affects only that request; the server keeps running.
