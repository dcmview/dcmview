# dcmview Internal HTTP API

The `dcmview` browser UI talks to the local Rust server through this HTTP API.
The API is internal to the viewer and intended for `dcmview` debugging, smoke
tests, and local automation only. It is not a stable public integration
contract.

The server is unauthenticated. Keep it bound to loopback for normal use and use
SSH forwarding for remote workflows. API responses can expose DICOM metadata,
file paths, annotations, and rendered or decoded pixel data.

The authoritative Rust wire declarations and endpoint operations—including
method, route, query/body types, success status, response media type, response
header kind, and error type—live in `src/api/contracts.rs`. Browser-facing
TypeScript types and typed endpoint metadata are generated from that source into
`frontend/src/generated/api-types.ts`; `frontend/src/api.ts` is the fetch
boundary used by Svelte components. See the
[architecture and test model](architecture.md#executable-http-contract) for
ownership and dependency direction.

## Contract Enforcement

The endpoint registry is executable rather than documentation-only:

- `src/server/api/routes.rs` registers each declared operation and constrains
  its handler-facing request, response, header, and error types.
- Rust unit tests check operation, method/path, query, media, header, and wire
  type invariants.
- Axum integration tests execute every declared endpoint and compare its
  success status, content type, and required headers with the registry.
- `npm --prefix frontend run check:contracts` rejects generated TypeScript
  drift and tests the Rust-to-TypeScript generator.

When adding or changing an endpoint, update the Rust declaration first,
regenerate the checked-in TypeScript, use the typed wrapper in `api.ts`, and
extend the runtime contract tests.

## Cross-Origin Debugging

Normal builds do not enable cross-origin browser access to the API. To inspect
the API from a separate browser origin while debugging `dcmview`, build with:

```bash
cargo build --features debug-api
```

The `debug-api` feature enables permissive CORS and prints a build warning. Do
not use it for normal research workflows or public-facing deployments.

## Endpoints

Static frontend assets are served at `/` and `/assets/*`.

| Method | Path | Description |
|---|---|---|
| GET | `/api/health` | Ready-state probe with viewer build identity, file count, and server start time. |
| GET | `/api/files` | File registry and progressive scan status. |
| GET | `/api/series` | Server-owned logical series and ordered virtual frame stacks. |
| GET | `/api/file/:index/info` | Frame metadata for one file. |
| GET | `/api/file/:index/references` | Typed DICOM relationships plus resolved local file/frame targets. |
| GET | `/api/file/:index/semantic-context` | Declared SEG, Parametric Map, or RT Dose semantics and validated source mappings. |
| GET | `/api/file/:index/frame/:frame` | Display frame; supported image transfer syntaxes return PNG. |
| GET | `/api/file/:index/frame/:frame/raw` | Decoded frame sample bytes plus rendering metadata headers. |
| GET | `/api/file/:index/frame/:frame/segmentation-overlay` | Transparent, source-sized SEG mask PNG for a validated source mapping. |
| GET | `/api/file/:index/tags` | Lazy DICOM tag tree. |
| GET | `/api/file/:index/tags/select` | Selective tag-path retrieval with sequence pagination. |
| GET | `/api/file/:index/annotations` | Current in-memory ROI annotations for one file. |
| PUT | `/api/file/:index/annotations` | Replace in-memory ROI annotations for one file. |
| GET | `/api/annotations/export.csv` | Download current annotations as EMBED-style CSV. |

## Health

`GET /api/health` returns JSON:

```json
{
  "status": "ok",
  "viewer": {
    "name": "dcmview",
    "version": "0.2.13",
    "build_git_sha": "0123456789abcdef...",
    "build_target": "aarch64-apple-darwin",
    "build_profile": "release"
  },
  "file_count": 2,
  "server_start_ms": 1714300000000
}
```

`file_count` is the number of valid DICOM files currently visible to the
registry. During progressive directory scans, it can grow after the server is
ready.

## File Registry

`GET /api/files` returns file summaries and scan progress:

```json
{
  "files": [
    {
      "index": 0,
      "path": "/path/to/scan.dcm",
      "label": "PATIENT - MG - 20240101",
      "patient_id": "PATIENT",
      "patient_name": "PATIENT",
      "study_instance_uid": "1.2.3",
      "study_date": "20240101",
      "study_description": "Screening",
      "series_instance_uid": "1.2.3.4",
      "series_number": "1",
      "series_description": "MLO",
      "modality": "MG",
      "instance_number": "1",
      "sop_instance_uid": "1.2.3.4.5",
      "sop_class_uid": "1.2.840.10008.5.1.4.1.1.4",
      "object_kind": "classic_image",
      "support_state": "renderable",
      "support_reason": null,
      "has_pixels": true,
      "frame_count": 60,
      "rows": 3000,
      "columns": 2500,
      "pixel_aspect_ratio": 1.0,
      "transfer_syntax_uid": "1.2.840.10008.1.2.4.50",
      "default_window": { "center": 200.0, "width": 4000.0 }
    }
  ],
  "discovery": [
    {
      "path": "/path/to/scan.dcm",
      "disposition": "selected",
      "reason": "valid_dicom"
    }
  ],
  "server_start_ms": 1714300000000,
  "scan_complete": true,
  "scanned": 2,
  "skipped": 0,
  "filtered": 0
}
```

Progress fields:

| Field | Meaning |
|---|---|
| `scan_complete` | `true` after all requested paths have been scanned. |
| `scanned` | Count of valid DICOM files accepted into the registry. |
| `skipped` | Count of files skipped because they could not be read as supported DICOM objects. |
| `filtered` | Count of readable DICOM files excluded by `--filter` metadata filters. |
| `discovery` | Up to the 256 most recently observed ephemeral ledger entries, sorted by normalized path, with disposition and stable reason code. Exact totals remain in `scanned`, `skipped`, and `filtered`. |

`support_state` is `renderable`, `metadata_only`, or `unsupported`. A non-null
`support_reason` is a stable machine identifier such as
`transfer_syntax.jpeg_ls_not_supported`; it describes current viewer
capability and does not judge DICOM conformance.

`pixel_aspect_ratio` is the effective physical row-to-column pixel extent used
by the viewport. It is derived from Pixel Spacing when available, otherwise
Pixel Aspect Ratio, and is `null` when neither yields a finite positive value.

Scripts should poll `/api/files` while `scan_complete` is `false` if they need a
complete scan result:

```js
async function waitForCompleteFiles() {
  while (true) {
    const response = await fetch("/api/files");
    const body = await response.json();
    if (body.scan_complete) return body.files;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}
```

For workflows that only need the first available file, poll until
`files.length > 0` or `scan_complete` becomes `true`.

## Series Catalog

`GET /api/series` returns Study/Series-owned navigation stacks. Each
`FrameRef` maps a zero-based virtual position to the original file index and
source frame, so clients keep metadata, cache, and annotation operations scoped
to the actual DICOM object:

```json
{
  "series": [{
    "id": "study:1.2.3|series:1.2.3.4",
    "study_instance_uid": "1.2.3",
    "series_instance_uid": "1.2.3.4",
    "frame_of_reference_uids": ["1.2.3.9"],
    "stacks": [{
      "id": "study:1.2.3|series:1.2.3.4|stack:ordinary",
      "kind": "ordinary",
      "concatenation_uid": null,
      "pyramid_uid": null,
      "image_type_role": null,
      "total_pixel_matrix_rows": null,
      "total_pixel_matrix_columns": null,
      "frames": [{
        "virtual_index": 0,
        "file_index": 0,
        "frame_index": 0,
        "source_path": "/path/to/slice-001.dcm",
        "sop_instance_uid": "1.2.3.4.5",
        "instance_number": 30,
        "position_along_normal_mm": 0.0
      }],
      "warnings": []
    }]
  }],
  "scan_complete": true
}
```

Ordinary CT/MR stacks prefer Image Position Patient projected onto the normal
derived from Image Orientation Patient, then fall back deterministically to
Instance Number and path. Warning codes include `missing_positions`,
`duplicate_positions`, `nonuniform_spacing`, `inconsistent_orientation`, and
`gantry_tilt`. Enhanced concatenations use their Concatenation UID and frame
offset. WSI Pyramid UID members are exposed as separate level stacks, while
same-series LABEL and other non-member companions remain isolated stacks.
Files without both Study Instance UID and Series Instance UID are omitted from
the logical series catalog; the frontend keeps them navigable as independent
files instead of combining incomplete identities across unrelated objects.

## Typed References

`GET /api/file/:index/references` extracts declared relationships without
loading Pixel Data, then resolves them against the current ephemeral registry
snapshot by SOP Instance UID or, for series-only declarations, Series Instance
UID. A response retains unresolved identities rather than silently dropping
them:

```json
{
  "source_file_index": 4,
  "source_sop_instance_uid": "1.2.3.derived",
  "references": [{
    "relationship": "source_image_for_segmentation",
    "target": {
      "sop_class_uid": "1.2.840.10008.5.1.4.1.1.2",
      "sop_instance_uid": "1.2.3.source",
      "series_instance_uid": "1.2.3.series",
      "frame_numbers": [1, 4],
      "segment_numbers": [1]
    },
    "matches": [{
      "file_index": 1,
      "path": "/path/to/source.dcm",
      "sop_instance_uid": "1.2.3.source",
      "frame_indices": [0, 3]
    }]
  }]
}
```

`target.frame_numbers` preserves DICOM one-based declarations.
`matches[].frame_indices` contains only validated zero-based frames suitable
for viewer navigation. Empty `matches` means the declaration was understood
but its target is not present in the current scan. Relationship names describe
identity and navigation only; they do not claim SEG, SR, presentation-state,
registration, RT, or other object-specific rendering semantics.

## Semantic Context And SEG Overlays

`GET /api/file/:index/semantic-context` interprets supported derived-object
metadata without changing its ordinary pixel preview. A SEG response includes
segment definitions, references, and one mapping record per SEG frame. Mapping
records expose `mapping_method`, `mapping_status`, `mapping_reason`, and the
resolved zero-based `source_frames`.

The SEG resolver first uses an explicit per-frame derivation source when one is
declared. If the object instead declares source instances at the top level, it
can resolve the SEG frame by patient geometry. This path requires compatible
Frame of Reference, Image Position (Patient), Image Orientation (Patient), and
Pixel Spacing. It supports a source series represented by many classic
single-frame instances and does not require identical source and SEG matrix
dimensions. Missing, non-coplanar, non-overlapping, or multiply matching grids
remain unavailable or ambiguous in the response.

`GET /api/file/:index/frame/:frame/segmentation-overlay` renders a frame only
after that same mapping validation succeeds. Binary SEG samples are treated as
a mask; fractional samples are normalized by Maximum Fractional Value. The
mask is nearest-neighbor resampled through patient coordinates and returned as
a transparent, source-sized `image/png`, ready to composite over the resolved
source display frame. A stable per-segment fallback palette supplies the mask
color; interpreting Recommended Display CIELab values is not yet implemented.

A successful overlay response includes `X-Cache: HIT` or `X-Cache: MISS` for
the decoded SEG frame. An absent or ambiguous mapping, incompatible geometry,
or a missing local source returns HTTP 422 with the
`semantic_mapping_unavailable` error code. A non-SEG object returns 400, and an
out-of-range SEG frame returns 404.

## File Info

`GET /api/file/:index/info` is the specialized single-file metadata endpoint.
It is useful when a client already knows the file index and does not need to
fetch the complete registry payload. It returns:

```json
{
  "frame_count": 60,
  "rows": 3000,
  "columns": 2500,
  "transfer_syntax_uid": "1.2.840.10008.1.2.4.50",
  "has_pixels": true,
  "sop_class_uid": "1.2.840.10008.5.1.4.1.1.4",
  "object_kind": "classic_image",
  "support_state": "renderable",
  "support_reason": null,
  "default_window": { "center": 200.0, "width": 4000.0 }
}
```

An unknown file index returns
`404 {"code":"not_found","error":"file index out of range"}`.

## Display Frames

`GET /api/file/:index/frame/:frame` returns `image/png` for supported display
paths. Supported compressed and native transfer syntaxes are decoded server-side
before PNG encoding; the endpoint does not return original compressed DICOM
fragments.

Query parameters:

| Parameter | Description |
|---|---|
| `wc` | Window center. Used with `ww` in default mode. |
| `ww` | Window width. Used with `wc` in default mode. |
| `mode` | `default` or `full_dynamic`. |

Window selection order:

1. `mode=full_dynamic`, which uses current-frame min/max and ignores explicit
   and DICOM window values.
2. Explicit `wc` and `ww`.
3. DICOM Window Center/Width from loader metadata.
4. 1st/99th percentile fallback from current-frame samples.

Transfer syntax behavior:

| Transfer syntax | Display behavior |
|---|---|
| JPEG Baseline / Extended | Decoded server-side and PNG-encoded. |
| JPEG Lossless / Lossless SV1 | Decoded server-side and PNG-encoded. |
| JPEG 2000 lossless/lossy | Decoded server-side and PNG-encoded. |
| JPEG-LS Lossless (`.80`) | Grayscale decoded server-side through statically linked CharLS and PNG-encoded. |
| JPEG XL Lossless (`.110`) | RGB decoded server-side and PNG-encoded without discarding channels. |
| RLE Lossless | Decoded server-side and PNG-encoded for 8/16-bit monochrome plus 8-bit RGB, YBR_FULL, and palette-color layouts. |
| Implicit LE / Explicit LE / Explicit BE / Deflated Explicit LE | Windowed server-side and PNG-encoded for 1/8/16/32-bit monochrome integer, float, double-float, RGB planar 0/1, YBR_FULL, YBR_FULL_422, and palette-color layouts. |
| JPEG-LS Near-Lossless (`.81`), JPEG XL `.111`/`.112`, other | `422 {"code":"unsupported_transfer_syntax","error":"unsupported transfer syntax: ..."}`. |

Every successful display-frame response includes `X-Cache: HIT` or
`X-Cache: MISS`. The display cache key includes file index, frame index, window
center, window width, and window mode.

## Raw Frames

`GET /api/file/:index/frame/:frame/raw` returns `application/octet-stream`. This
endpoint transports decoded samples for client-side rendering; for compressed
syntaxes it is not a byte-for-byte copy of the DICOM Pixel Data element.

Supported raw paths:

| Transfer syntax | Raw behavior |
|---|---|
| Uncompressed | Native samples normalized to little endian. Stored planar ordering is retained; integer padding bits are masked and signed values extended from High Bit; one-bit samples are expanded to one byte each. Integer samples through 32 bits plus Float Pixel Data and Double Float Pixel Data are supported. |
| JPEG Baseline / Extended | Decoded to 8-bit grayscale samples. |
| JPEG Lossless | Decoded to 8-bit or 16-bit grayscale samples when supported by the codec stack. |
| Grayscale JPEG 2000 | Decoded to 8-bit or 16-bit samples. |
| JPEG-LS Lossless (`.80`) | Decoded to unsigned 8-bit grayscale samples. |
| JPEG XL Lossless (`.110`) | Decoded to unsigned, interleaved 8-bit RGB samples. |
| RLE Lossless | Decoded to interleaved, little-endian native sample bytes for supported layouts. |
| JPEG-LS Near-Lossless (`.81`), JPEG XL `.111`/`.112`, unsupported | `422` or a decode error. |
| Multi-component JPEG 2000 raw decode | `422` or a decode error. |

Successful raw-frame responses include `X-Cache: HIT` or `X-Cache: MISS`. The
raw cache key includes file index and frame index only; window parameters do not
change raw sample bytes.

Required metadata headers:

| Header | Meaning |
|---|---|
| `X-Frame-Rows` | Frame row count. |
| `X-Frame-Columns` | Frame column count. |
| `X-Frame-Bits-Allocated` | Bits allocated per sample. |
| `X-Frame-Pixel-Representation` | `0` for unsigned samples, `1` for signed samples. |
| `X-Frame-Samples-Per-Pixel` | Samples per pixel in the raw response. |
| `X-Frame-Photometric-Interpretation` | Photometric interpretation used by the renderer. |
| `X-Frame-Rescale-Slope` | DICOM rescale slope. |
| `X-Frame-Rescale-Intercept` | DICOM rescale intercept. |

For expanded one-bit responses, `X-Frame-Bits-Allocated` remains `1` because it
describes the DICOM sample type; each response byte contains one canonical
sample value (`0` or `1`). Float and double-float responses contain IEEE 754
little-endian values. Their pixel rendering does not imply application of Real
World Value Mapping semantics.

Optional metadata headers:

| Header | Meaning |
|---|---|
| `X-Frame-Default-Wc` | DICOM default window center, when available. |
| `X-Frame-Default-Ww` | DICOM default window width, when available. |

## Tags

`GET /api/file/:index/tags` returns an array of tag nodes:

```json
[
  {
    "tag": "(0028,0010)",
    "vr": "US",
    "keyword": "Rows",
    "value": { "type": "number", "value": 3000 }
  },
  {
    "tag": "(7FE0,0010)",
    "vr": "OW",
    "keyword": "PixelData",
    "value": { "type": "binary", "length": 15000000 }
  }
]
```

Pixel Data and other binary VRs are represented by byte length, not by full
values. Long numeric arrays and sequences can include `truncated` and `total`
fields. Individual serialization failures are returned as
`{"type": "error", "message": "..."}` values so one bad tag does not prevent
the rest of the tag tree from rendering.

For exhaustive targeted retrieval, use
`GET /api/file/:index/tags/select?path=(GGGG,EEEE)&offset=0&limit=64`.
Selectors alternate tag and zero-based sequence item components, for example
`(0008,2218)/69/(0008,0100)`. When the selected tag is a sequence, `offset`
and `limit` page its items; `limit` must be between 1 and 256. A deep selector
opens the original object and traverses directly, so it is not constrained by
the legacy full-tree endpoint's preview depth or item caps.

## Annotations

`GET /api/file/:index/annotations` returns the current in-memory EMBED-style ROI
payload for one file:

```json
{
  "num_roi": 2,
  "roi_coords": [[120, 340, 220, 430], [400, 510, 480, 590]],
  "roi_frames": [[0, 1, 2], [5, 6]]
}
```

`PUT /api/file/:index/annotations` replaces one file's in-memory annotations and
returns the canonicalized payload. Coordinates and frame indices are validated
against the file dimensions and frame count. Invalid payloads return
`400 {"code":"bad_request","error":"..."}`.

`GET /api/annotations/export.csv` returns `text/csv; charset=utf-8` with
`Content-Disposition: attachment; filename="dcmview-annotations.csv"`. The CSV
is generated from current in-memory annotations; source DICOM files and input
CSV files are not modified.

## Error Semantics

All API failures—including path, query, and JSON extractor rejections—use a JSON
body:

```json
{ "code": "not_found", "error": "file index out of range" }
```

Common statuses:

| Status | Typical cause |
|---|---|
| `400` | Malformed path/query input, an invalid window pair, or invalid annotation values. |
| `404` | Unknown API route or file index, missing pixel data, or out-of-range frame. |
| `405` | Unsupported method on a known API route. |
| `422` | Structurally invalid JSON, unsupported transfer syntax, or unsupported raw component layout. |

All API failures use the shared envelope
`{"code":"stable_machine_code","error":"human-readable detail"}`. Clients
should branch on `code`; the `error` text is diagnostic context and is not a
stable programmatic interface.
| `500` | Decode, filesystem, tag serialization task, or export failure. |

Frame decode errors are returned for the request that failed; the server remains
running.
