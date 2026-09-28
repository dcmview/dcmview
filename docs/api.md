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
| GET | `/file/{index}/frame/{frame}` | Display frame as `image/png`, with `X-Cache` and, for linearly windowed frames, `X-Frame-Window-Center`/`X-Frame-Window-Width`. Query: `wc`, `ww`, `mode`, `unit`, `preview`. |
| GET | `/file/{index}/frame/{frame}/raw` | Decoded samples as `application/octet-stream`, with `X-Cache` and `X-Frame-*` metadata headers. |
| GET | `/file/{index}/frame/{frame}/raw/pixel?row=&column=` | One pixel of the raw frame as a 1x1 raw frame: its stored samples in color-by-pixel order (planar and subsampled YBR_FULL_422 resolved), with the same headers. `400` outside the frame. |
| GET | `/file/{index}/frame/{frame}/presentation-layer` | The display shutter fill and overlay graphics of a grayscale display frame as an RGBA `image/png` of the frame's size, opaque gray where drawn and transparent elsewhere (fully transparent without a shutter or overlay), with `X-Cache`. |
| GET | `/file/{index}/frame/{frame}/segmentation-overlay` | Transparent source-sized SEG mask as `image/png`, with `X-Cache`. |
| GET | `/file/{index}/frame/{frame}/dose-overlay` | RT Dose colorwash sized to this frame as `image/png`, with `X-Cache`. Query: `dose` (RT Dose file index). |
| GET | `/file/{index}/frame/{frame}/dose-overlay/values` | The same resampled dose as little-endian `f32` values, `application/octet-stream`, with `X-Cache`. Query: `dose`. |
| GET | `/file/{index}/frame/{frame}/parametric-map-overlay` | Parametric Map colorwash sized to this frame as `image/png`, with `X-Cache`. Query: `map` (Parametric Map file index). |
| GET | `/file/{index}/frame/{frame}/parametric-map-overlay/values` | The same resampled mapped values as little-endian `f32` values, `application/octet-stream`, with `X-Cache`. Query: `map`. |
| GET | `/file/{index}/frame/{frame}/value-mapping` | `FrameValueMapping`: how this frame's stored samples convert to modality and real-world values. |
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

## Display And Raw Frames

Display frames are always decoded server-side and PNG-encoded; the endpoint
never returns compressed DICOM fragments. The window is chosen in this order:

1. `mode=full_dynamic`: current-frame min/max; `wc`/`ww` are ignored.
2. Explicit `wc` and `ww`, which must be sent together with a positive width.
3. DICOM Window Center/Width.
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
waits for that decode and reports `HIT`, and a decode whose client
disconnected still fills the cache. A display request can fill the raw cache
too (grayscale frames are windowed from decoded samples kept there), so a raw
request after a display request of the same frame may report `HIT`.

A grayscale display frame windowed linearly reports the window it was
rendered with, in Modality values, as `X-Frame-Window-Center` and
`X-Frame-Window-Width` (the width at least 1, as applied), whichever step
above chose it; a drag preview reports its window too. Color frames, frames
presented through a VOI LUT, and frames windowed in a real-world `unit` send
neither: a window over mapped values has no linear Modality equivalent. A
`unit` request whose window could not be applied reports the default window it
was shown with instead, so the pair's presence on a `unit` response means the
requested window was not used.

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
matrices may differ in size.

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
volume and displayed frame, and SEG overlays per SEG frame and resolved
source frame; every overlay endpoint's `X-Cache` reports that encoded-PNG
cache, not the decoded frames beneath it.

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
decoded, or a dose grid without positive dose, is ineligible.

The overlay endpoints answer `400` when the query names the wrong kind of
object, `404` for an unknown file index or out-of-range frame, `404
overlay_not_covering_frame` when no pixel center of the displayed frame
lies inside the volume, and `422 semantic_mapping_unavailable` when the
volume is ineligible or the displayed frame lies in another Frame of
Reference or lacks geometry.

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
| `404` | `not_found`, `route_not_found`, `asset_not_found`, `no_pixel_data`, `frame_out_of_range`, `overlay_not_covering_frame` |
| `405` | `method_not_allowed` |
| `413` | `invalid_json` (a JSON body over 2 MiB, about 200,000 ROIs in one annotation edit) |
| `415` | `invalid_json` (missing `Content-Type: application/json`) |
| `422` | `invalid_json` (valid JSON of the wrong shape), `unsupported_transfer_syntax`, `unsupported_pixel_layout` (display frames for any layout the catalog marks unsupported; raw frames for invalid geometry or numeric precision), `semantic_mapping_unavailable` |
| `500` | `pixel_decode_failed`, `internal_error` |

A failed decode affects only that request; the server keeps running, and logs
the failure (with the request) to stderr. A file deleted or moved after
discovery answers `404 not_found` naming its path.

JSON, CSV and the viewer's scripts and styles are gzip-compressed for clients
that send `Accept-Encoding: gzip`; PNG frames, raw samples and fonts are not.
