# General image formats: scoping

Read against `dcmview/dcmview` at `b78536f` (0.3.1 dev), `dcmview-docs`
(synced to v0.3.0) and `dcmview-test-corpus` (DICOM-only). Date: 2026-09-29.
This is a discussion of approaches, not a spec. Nothing was changed in any
repository.

Already decided and not re-opened here: dcmview stays ephemeral and the hub
owns durable state; one neutral annotation model with EMBED as an adapter at
parity; URL token plus optional Unix socket for standalone dcmview. **File
identity belongs to `annotation-model.md`.** This document states what rasters
need from identity (section 10) and does not decide it.

Amended on 2026-09-30 after two external reviews (the display-budget decision
confirmed by the owner, plus "no decision needed" fixes); see the list below.

---

## Review amendments (2026-09-30)

- **2.3, 12 item 9**: keep the 268 Mpx cap; add a browser display budget (frames above 64 Mpx display through a downsampled canvas, full-resolution samples still served for readout) and byte-based decode admission with a bounded queue; `support_state` no longer depends on a host flag.
- **2.4, 5.1, 5.4, 11**: a TIFF page joins the frame map only if width, height, samples, sample format, bits per sample, photometric, extra-sample (alpha) type and orientation match page 0; other pages are excluded and reported; associated alpha is un-premultiplied on decode.
- **5.2, 9**: decoder output is normalized back to stored sample semantics: 1-bit and low-bit PNG are mapped back to source bit depth (image 0.25.6 expands them to 0..255), and tiff 0.9.1's WhiteIsZero inversion is undone so raw = stored samples, with MONOCHROME1 display inversion applied once; fixtures assert raw values.
- **5.2, 5.3, 12 item 5**: `sBIT` is informational (original precision), not a value range: the window uses the stored-depth range or percentile, never `0..2^sBIT-1`; an `sBIT=12` fixture is frozen.
- **5.2, 6.2**: CMYK/YCCK JPEG is converted approximately to sRGB and its CMYK ICC profile is dropped, with the conversion noted in the file report; profiles are checked for a matching colour space before use.
- **5.6, 10 item 2**: hashing is not free: whole-file digests are background I/O with a pending state (extra reads for multi-page TIFF, which is read a page at a time); per-frame pixel digests are separate from the whole-file digest, over a defined canonical byte layout.
- **11**: thumbnails are rendered in the stored pixel grid, and the client applies the orientation with the viewer's transform state; interfaces for the display budget, frame compatibility and normalization added.
- **12**: decisions 5, 6 and 9 amended.

A review's decoder-provenance note (that `jpeg-decoder` does not come
through `tiff`) was checked and is wrong; section 11's wording
stands.

---

## 0. Summary

- **Decoding is nearly free.** `dicom-pixeldata`'s `image` feature already pulls
  `image 0.25.6` with its `png`, `jpeg` (zune-jpeg 0.4), `tiff` (tiff 0.9.1),
  `webp`, `bmp`, `pnm` and `exr` codecs into today's binary (checked with
  `cargo tree -e features -i image`). We would only name those features on our
  own `image` line so we stop depending on another crate's defaults. The one
  likely new dependency is an EXIF parser for the metadata panel.
- **The real work is the entry point and the DICOM-only side paths**, not
  codecs: the `DICM` gate in discovery, a `FileEntry` with no format field,
  codec dispatch keyed on transfer-syntax UID, and five endpoints (tags,
  value-mapping, semantic context, references, WSI) that reopen every file as
  DICOM and would answer 500 for a PNG.
- **Recommended v1:** PNG (all bit depths, palette, alpha, 16-bit), JPEG
  (8-bit baseline and progressive, EXIF orientation), TIFF (strips and tiles,
  multi-page as frames, 8/16/32-bit integer and 32-bit float, BigTIFF) with a
  pixel-count cap, and still WebP. Detected by content, never by extension.
- **Decisions that need you** are listed in section 12. The ones with the most
  downstream reach: whether rasters are on by default (it changes what
  existing DICOM users see in mixed folders), how EXIF orientation relates to
  annotation coordinates, and where rasters sit in the clinical tree.

---

## 1. What exists today

### 1.1 Entry

- `loader/discovery.rs` `collect_candidates` takes every regular file (walkdir,
  no extension filter, no symlink following). Rayon inspects candidates in
  parallel and events arrive in completion order, so `FileEntry.index` is
  arrival order and not deterministic.
- `loader/entry.rs:315` `read_discovery_header` reads 132 bytes and requires
  `DICM` at 128. Anything else is `DiscoveryReason::MissingPart10Preamble`,
  reported as "not DICOM (no DICM preamble)". **A PNG is skipped today, and
  the 132-byte read that rejects it already holds every magic number we
  need.**
- Discovery is header-only for DICOM: it parses up to the top-level pixel
  element and reads no samples (except the bounded FRACTIONAL SEG check). The
  raster path must keep that property.

### 1.2 Model

- `FileEntry` (`types.rs:130`) is DICOM-shaped: twelve DICOM identity
  strings (empty when missing), pixel description (rows, columns,
  bits_allocated, pixel_representation, samples_per_pixel, photometric,
  rescale, `transfer_syntax_uid`, `default_window`) and a large
  `SeriesMetadata`. Everything is `Default`, so a raster entry can leave the
  DICOM parts empty. **There is no field for the file's format.**
- `FileSummary` on the wire (`contracts.rs:336`) mirrors it; `object_kind`
  comes from the SOP Class and `support_state` from `classify_pixel_support`,
  which needs a known transfer syntax.

### 1.3 Pixels

- Dispatch: `pixels/syntax.rs` `Codec` enum chosen by
  `codec_for_syntax(transfer_syntax_uid)`; unknown UID is `422`.
- Raw tier: decoded little-endian samples plus `X-Frame-*` headers
  (`RawFrameMetadata`); layouts 1/8/16/32/64-bit, 1 or 3 samples, float on the
  native path only. Byte-budgeted LRU keyed by `(file_index, frame)`.
- Display tier: always PNG. Grayscale 8/16-bit integers take their samples
  from the raw cache and window them through the per-stored-value table in
  `render.rs`; colour ends in `render::encode_rgb8_display_png`, which embeds
  a source ICC profile as `iCCP` and is never windowed.
- Default window when the file has none: **1st to 99th percentile of the
  frame** (`window.rs:346`). For an 8-bit photo or PNG this would stretch
  contrast by default, which no image viewer does.
- Client: raw path (client-side window/level in a worker) only for
  `samplesPerPixel === 1` up to 20 Mpx, and only once the frame's
  `/value-mapping` has loaded; colour always uses the server PNG.
- No colour-management crate is linked; ICC is passed through to the browser.
- `pixeldata_frame.rs` `DecodedFrame{bytes, rows, columns, bits_allocated,
  samples_per_pixel, icc_profile}` is the closest existing "decoded frame"
  and is what a raster decoder should produce.

### 1.4 Everything else that assumes DICOM

| Place | Today with a raster | Needs |
|---|---|---|
| `server/tags.rs` `build_tag_tree` | 500 "failed to open DICOM" | raster metadata branch (section 8) |
| `value_mapping.rs` `FileValueMappings::read` | error, so the client raw path never draws | identity mapping for rasters |
| `semantic.rs` `semantic_context` | opens the header before dispatching on SOP class | return not-applicable before opening |
| `references` handler | `open_header` error | empty list |
| `wsi` handler | gated on SOP class first | nothing |
| `series.rs:174` series catalog | drops files with empty Study/Series UID | nothing for v1 (section 7) |
| `fileTree.ts` study view | one "Unknown Patient / Study / Unknown Series" per file | grouping rule (section 7) |
| `--filter` | substring on empty fields fails, so any DICOM filter excludes rasters | keep; add `format` and `path` fields |
| `annotations.rs` | keyed by index, matched by path | works once the file is loaded |
| startup summary, CLI help, error text | "DICOM" hard-coded | wording |
| VS Code custom editor | `*.dcm`, `*.dicom`, `*.ima` only | add raster selectors at `option` priority |
| Python wrapper | passes paths through | docs only |

---

## 2. Which formats, and how deep

### 2.1 What the linked decoders can do

| Crate (already in the lock) | Handles | Does not handle |
|---|---|---|
| `png 0.17` | 1/2/4/8/16-bit gray, gray+alpha, RGB, RGBA, palette with `tRNS`, Adam7, `iCCP`, `eXIf`, text chunks, APNG | nothing we need |
| `zune-jpeg 0.4` (via `image`) | 8-bit baseline and progressive, gray/RGB, CMYK/YCCK converted, ICC and EXIF segments, orientation via `image` | 12-bit, lossless (SOF3) and arithmetic-coded JPEG (my understanding of zune-jpeg 0.4; to verify with fixtures) |
| `tiff 0.9.1` | strips **and tiles** (`read_chunk`), multi-page (`seek_to_image`), **BigTIFF**, none/LZW/Deflate/PackBits/new-style JPEG (7), planar and chunky, u8-u64, i8-i64, f32/f64, WhiteIsZero, YCbCr (3 samples), CMYK | old-style JPEG (6), JPEG 2000 (33003/33005, common in SVS slides), LZMA, zstd, palette (`RGBPalette` falls through to "unsupported") |
| `image-webp 0.2` | lossy and lossless still WebP, alpha, ICC, EXIF, animation | nothing we need |
| `jpeg2k`/openjp2, `jxl-oxide` (DICOM codecs) | could decode standalone `.jp2`/`.j2k` and `.jxl` too | not requested |

`image` exposes only the first TIFF page, so multi-page and tiled reads call
the `tiff` crate directly. PNG, JPEG and WebP go through `image` decoders
(`icc_profile()`, `exif_metadata()`, `orientation()` are in 0.25.6).

### 2.2 Options

- **A. Minimum:** PNG and JPEG only. Covers the colleagues' reported case and
  most ML datasets, but TIFF is the usual container for 16-bit research
  images and microscopy, and it would be the first follow-up request.
- **B. Recommended:** PNG, JPEG and TIFF at the depths in 2.3, plus still
  WebP because it costs a match arm. Formats that need new dependencies or
  new semantics wait.
- **C. Broad:** B plus BMP, GIF, APNG/animated WebP as cine, JPEG 2000 and
  JPEG XL files, OME-TIFF and SVS pyramids. Each is cheap to decode but each
  brings semantics (animation timing, pyramids, physical sizes) that the
  viewer would then have to honour. Not worth it before the hub.

### 2.3 Recommended v1 depth

| Format | In v1 | Out of v1 (reported as unsupported with a stable reason) |
|---|---|---|
| PNG | all bit depths, gray/gray+alpha/RGB/RGBA/palette, Adam7, 16-bit | APNG animation (first frame only, noted in metadata) |
| JPEG | 8-bit baseline and progressive, gray/RGB; CMYK/YCCK converted approximately to sRGB (CMYK profile dropped, 6.2) | 12-bit, lossless and arithmetic JPEG (`raster.jpeg_unsupported_process`) |
| TIFF | strips and tiles; multi-page as frames (rule in 2.4); 1/8/16/32-bit unsigned and signed integer, 32/64-bit float gray; RGB/RGBA 8/16-bit; WhiteIsZero; BigTIFF; LZW/Deflate/PackBits/new JPEG | old JPEG, JPEG 2000, LZMA, zstd compression; palette, CMYK, Lab, YCbCr subsampling other than inside JPEG; pyramidal WSI-style files beyond the pixel cap |
| WebP | still, lossy and lossless, alpha | animation (first frame only) |

**Pixel cap.** A frame is decoded whole (there is no tiled viewport), so each
frame needs a ceiling or a 100k x 100k TIFF takes the server down. Proposal:
`rows * columns <= 268 Mpx` (16k x 16k) per frame; over that the file is
listed with `support_state = unsupported`, reason `raster.too_large`, and its
metadata still shows. For scale, a 4096 x 5120 mammogram is 21 Mpx. Use
`image`'s `Limits` and the `tiff` decoder's limits so a lying header cannot
force an allocation.

**Amended 2026-09-30: the cap stays at 268 Mpx, and two separate limits bound
memory.** The cap alone bounds neither in-flight decode memory nor the
browser, where an 8-bit frame near the cap becomes a ~1 GiB RGBA canvas at
Chrome's area limit.

- **Browser display budget.** Frames above a display budget (default
  **64 Mpx**) are displayed through a downsampled canvas (the server's display
  PNG at a reduced scale). The full-resolution raw samples are
  still served for readout and annotation coordinates, which stay in the
  stored grid. The budget is a client setting, so it can follow the browser.
- **Byte-based decode admission.** Each decode reserves its estimated
  decoded bytes (samples, intermediates and the output frame) against a
  per-process decode-memory budget before it takes a `DECODE_PERMITS` slot;
  requests wait in a **bounded queue** and get 503 with `Retry-After` when
  the queue is full. A frame whose estimate exceeds the whole budget is
  refused for that request with a clear error, not marked unsupported.
- **Not tied to the cache budget.** The earlier "the decoded frame must fit
  the raw cache budget" rule made `support_state` change with a host flag;
  `raster.too_large` now depends only on the 268 Mpx cap. A frame larger
  than the raw cache is decoded but not cached.

### 2.4 Multi-page TIFF rule

Pages are frames when they match page 0 in **width, height, samples per pixel,
sample format, bits per sample, photometric interpretation, extra-sample
(alpha) type and orientation** (tag 274), and are not flagged
reduced-resolution (`NewSubfileType` bit 0) or mask (bit 2). (Amended
2026-09-30: bits per sample, photometric, alpha type and orientation were
missing, so an 8-bit and a 16-bit page, or a BlackIsZero and a WhiteIsZero
page, could share one frame map with one set of per-file fields.) Other pages
are excluded from the frame map, listed in the metadata panel with the field
that differed, reported in the file's warnings, and not shown. A page whose
ICC profile differs from page 0's joins the frame map but is reported, and
page 0's profile is used. This handles ImageJ/tifffile stacks (all pages
equal, frames) and files with an embedded thumbnail page (skipped). Mixed-size
page sets (e.g. scanned multi-page documents) show page 0 plus a "N pages not
shown" note; a later version could present them as separate files.

The frame-to-page mapping (frame `k` is IFD `n`) must be kept per file,
because identity and annotations will need the IFD index, not our derived
frame index (section 10).

---

## 3. Detecting files by content

### 3.1 Options

- **A. Extension only.** Cheap and predictable, but misses renamed files and
  accepts junk with a `.png` name. dcmview never uses extensions for DICOM.
- **B. Content only (recommended).** Reuse the 132-byte read:
  1. `DICM` at offset 128 wins first. This matters for DICOM WSI
     "dual-personality" files, whose preamble *is* a TIFF header: they must
     stay DICOM.
  2. Otherwise match the first bytes: PNG `89 50 4E 47 0D 0A 1A 0A`; JPEG
     `FF D8 FF`; TIFF `49 49 2A 00` / `4D 4D 00 2A`; BigTIFF `49 49 2B 00` /
     `4D 4D 00 2B`; WebP `RIFF....WEBP`.
  3. Otherwise skipped as today ("not a DICOM or image file").
  Files shorter than 132 bytes currently fail the DICOM read at EOF; the
  raster sniff must run on the short buffer too (a valid tiny PNG is about 70
  bytes).
- **C. Content plus extension agreement.** Rejects `scan.png` that is really
  a JPEG. Real datasets are full of those, and the decoders do not care, so
  this adds friction for nothing. Show a mismatch as a metadata note instead.

### 3.2 Default: on or opt-in (needs you)

Today a folder with DICOM plus `.jpg` previews or `.png` exports shows only
the DICOM. With rasters on by default those files appear in the navigator
and the "loaded N files" count. Options:

- **On by default, `--formats` to narrow** (recommended). `--formats dicom`
  restores today's behaviour; `--formats dicom,png` etc. picks a subset. It
  suits the colleagues' mixed datasets without a flag, and section 7 keeps
  rasters out of the patient/study cards so clinical browsing stays clean.
- **Opt-in `--images`.** Zero change for current users, but the people who
  asked have to know about a flag, and the VS Code custom editor would have
  to pass it.
- **On, but only for paths named explicitly** (not found by a directory
  walk). Surprising when a folder of PNGs shows nothing.

Either way, a raster named explicitly on the command line is always loaded,
and `--filter` gains `format=` and `path=` fields. Existing DICOM filters
(`modality=MG`) keep excluding rasters because their fields are empty, which
is the right behaviour.

### 3.3 Discovery reasons and wording

Add `DiscoveryReason::ValidImage` (`valid_image`) and replace
`MissingPart10Preamble` for files that are neither DICOM nor a known raster
with `unrecognized_format`. That renames a stable wire code, so it needs your
sign-off under AGENTS.md; the alternative is to keep
`missing_part10_preamble` as the code and only change its text. Add
`raster_header_invalid` for a file with a raster signature whose header does
not parse. The startup line becomes "loaded N files (D DICOM, R images)".

---

## 4. Discovery cost per format

Discovery must stay header-only. What each format needs to read:

| Format | Header read | Also collected |
|---|---|---|
| PNG | chunks up to the first `IDAT` | `IHDR`, `sBIT`, `iCCP`, `eXIf`, text chunks, `acTL` (animated) |
| JPEG | markers up to the first SOF | APP1 EXIF (orientation, thumbnail offset), APP2 ICC, Adobe APP14 (CMYK) |
| TIFF | walk the whole IFD chain | per-page size, samples, compression, subfile type; ICC tag 34675, EXIF IFD, `ImageDescription` |
| WebP | `VP8 `/`VP8L`/`VP8X` header | ICC, EXIF, animation flag |

A TIFF with thousands of pages means thousands of small IFD reads at
discovery; that is still header-only and comparable to a DICOM with a large
per-frame functional group sequence. Keep the IFD offsets from this walk so a
frame request can seek straight to its page.

Discovery never hashes file content. If identity wants a content hash for
rasters (section 10), it has to be computed lazily or by the hub.

---

## 5. Model and pixel pipeline

### 5.1 How to represent a raster in `FileEntry`

- **A. Pseudo-DICOM.** Synthesize UIDs, a private "transfer syntax" and
  Modality `OT`, so everything downstream "just works". Least code, but it
  lies on the wire (tag panel, references, filters, the series catalog, and
  most importantly identity, which would get invented UIDs that change every
  run). Rejected.
- **B. Explicit format (recommended).** Add `format: FileFormat` (`Dicom`,
  `Png`, `Jpeg`, `Tiff`, `Webp`) and `raster: Option<Box<RasterMetadata>>` to
  `FileEntry`. DICOM identity strings stay empty, `transfer_syntax_uid` stays
  empty, `object_kind` gets a new `image` value. On the wire `FileSummary`
  gains `file_format` and an optional `raster` summary (section 11).
- **C. A separate `RasterEntry` type and registry.** Cleanest types, but
  every endpoint, cache key and frontend store keyed on one file index would
  split in two. Not worth it.

`RasterMetadata` holds: container format, colour type, bit depth and sample
format, photometric, total pages and the frame-to-IFD map (with the excluded
pages and why, 2.4), alpha present and its type (associated or
unassociated), orientation (1-8), ICC bytes (or presence plus an offset to
re-read), `sBIT` significant bits (informational, 5.3) and TIFF
`MaxSampleValue`, APNG/animated flag, and a compact list of warnings. These
per-file fields hold for every frame, which the 2.4 compatibility rule
guarantees.

### 5.2 Dispatch

Add `Codec::Raster(RasterFormat)`, chosen from `file.format` **before** the
transfer-syntax lookup, so `classify_pixel_support`, `load_raw_frame` and
`load_frame` all follow one table as today. A new `pixels/raster.rs` decodes
one frame into the existing `DecodedFrame` shape and normalises it.

**Decoder output is not stored sample semantics** (amended 2026-09-30,
reproduced against the locked crates). `image 0.25.6` decodes PNG with
`png::Transformations::EXPAND`, which scales low-bit gray by
`255/(2^n-1)`, so a 1-bit PNG `[0,1]` comes back as `[0,255]`. `tiff 0.9.1`
already inverts WhiteIsZero (`max - x` for unsigned, `1.0 - x` for float, no
change for signed), so inverting again for MONOCHROME1 would invert twice.
`pixels/raster.rs` therefore normalizes decoder output back to the stored
samples before anything else sees them: low-bit PNG values are mapped back to
the source bit depth (or png is called directly without the scaling), and
tiff's WhiteIsZero inversion is undone, so "raw" always means the samples as
stored in the file. Display inversion for MONOCHROME1 is then applied once,
by the shared windowing path, exactly as for DICOM.

| Source | Raw tier (samples served) | Photometric | Display |
|---|---|---|---|
| gray 1/2/4-bit PNG | one byte per sample, **values at source depth (0..2^n-1)**: `image`'s expansion to 0..255 is undone | MONOCHROME2 | windowed |
| gray 8/16-bit | as stored, 16-bit swapped to little endian | MONOCHROME2 | windowed |
| TIFF WhiteIsZero, unsigned or float | **as stored**: tiff's `max - x` / `1.0 - x` inversion is undone | MONOCHROME1 | windowed, then inverted once, as DICOM |
| TIFF WhiteIsZero, signed int | as stored (tiff does not invert signed samples) | MONOCHROME1 | windowed, then inverted once |
| TIFF signed int / float | as stored, `pixel_representation` 1 / float kind | MONOCHROME2 | windowed (float path already exists) |
| RGB 8-bit, palette (expanded) | interleaved RGB | RGB | `encode_rgb8_display_png` with ICC |
| CMYK/YCCK JPEG | converted approximately to sRGB, interleaved RGB | RGB | `encode_rgb8_display_png` **without** ICC: the CMYK profile no longer describes the pixels, so it is dropped and the conversion is noted in the file report |
| RGB 16-bit | interleaved RGB16 | RGB | reduced to 8-bit for the PNG (ICC kept) |
| gray+alpha, RGBA | see 5.4 | | |

Each row has a fixture that asserts raw values, readout and display output
(section 9), so a decoder upgrade that changes its output fails a test
rather than silently shifting readouts and annotations.

Grayscale rasters then get the shared windowing, the raw cache reuse and
client-side window/level for free, provided `/value-mapping` answers with an
identity mapping. Colour behaves exactly like DICOM colour.

One small leak to fix: the frontend reads planar configuration when
`rawColorNeedsPlanarConfiguration(transfer_syntax_uid)`; raster raw samples
are always interleaved, so that check must key on format, not on the empty
UID.

### 5.3 Default window (needs you, small)

The percentile default is right for DICOM without a window and wrong for an
8-bit photo. Options:

- **8-bit: identity (0-255); 16-bit: percentile, unless TIFF
  `MaxSampleValue` declares the range** (recommended). Matches every image
  viewer for 8-bit, and 16-bit containers often hold 10-14-bit data that
  would look black at identity.

  **Amended 2026-09-30: `sBIT` is not a value range.** The PNG spec
  has encoders scale original samples up to the full container depth, and
  `sBIT` only records the original precision, so a conformant 16-bit PNG
  with `sBIT=12` spans about 0..65535. Windowing it as `0..4095` would clip
  most of the image to white. So the window keeps the **stored-depth range**
  (percentile for 16-bit, as above); `sBIT` is informational, shown in the
  metadata panel and recorded in `RasterMetadata`, and never narrows the
  window or rescales samples. TIFF `MaxSampleValue` is a real range and
  still sets the window. An `sBIT=12` fixture freezes this.
- Percentile everywhere (today's DICOM rule). Consistent, but a JPEG
  photograph looks wrong on first open.
- Identity everywhere. 16-bit research images look black.

Label masks stored as 8-bit PNG with values 0/1 will look black at identity.
"Full dynamic" is one click away already; the metadata panel can flag
"value range 0-1" as a hint. It is also a reason for the palette-index option
in 5.5.

### 5.4 Alpha

- **A. Flatten over black for display, keep alpha in the raw tier as a fourth
  sample** (recommended). The viewport background is black, so display looks
  the same as compositing, and the readout can still show `A`. This widens
  the raw contract to `samples_per_pixel = 4` and a new photometric value
  (`RGBA`, and `MONOCHROME2` with 2 samples for gray+alpha), which
  `rawWindowing.ts` rejects today, so those frames go through the server PNG
  like colour.

  **Associated vs unassociated alpha** (amended 2026-09-30). Flattening
  over black equals compositing only for premultiplied (associated) colour.
  PNG and WebP alpha is unassociated; TIFF declares it in `ExtraSamples`
  (1 = associated, 2 = unassociated). Associated alpha is **un-premultiplied
  on decode**, so the raw tier always holds unassociated colour plus alpha,
  and display flattening multiplies by alpha once. Pages with different
  alpha types do not share a frame map (2.4).
- **B. Drop alpha entirely.** Simplest; the readout cannot show it, and alpha
  used as a mask is invisible.
- **C. Keep alpha in the display PNG.** The viewport would show a checkerboard
  or background through the image; it complicates overlay compositing for
  little gain.

### 5.5 Palette images (a follow-up worth naming)

Palette PNGs are a common label-mask format (PASCAL VOC style). Expanding them
to RGB loses the index. dcmview already supports DICOM PALETTE COLOR with a
"palette index" readout (`palette.rs`). Mapping palette PNG and TIFF onto that
path keeps the index for the readout and for future "fill mask from file"
imports. Recommend v1 expands to RGB and this follows soon after; flag it for
the annotation model and the adapters doc as a possible mask import source.

### 5.6 Decode cost and caching

- JPEG, PNG and WebP decode the whole image per request; they are one frame,
  and the raw cache keeps the result, so a second window costs nothing.
- Multi-page TIFF seeks to the page's IFD from the offsets kept at discovery.
- A tiled TIFF decodes every tile of a frame; the pixel cap bounds it.
- Decodes run under the existing `DECODE_PERMITS` semaphore off the executor,
  as the maintainer invariants require, after byte-based admission (2.3).
- **Hashing is not free** (amended 2026-09-30). A whole-file digest for
  identity reads every byte. For single-image PNG, JPEG and WebP the decode
  has just read the whole file, so hashing after it is mostly CPU; for
  multi-page TIFF (read a page at a time from the IFD offsets) it is extra
  I/O over the whole file. Whole-file digests therefore run as **background
  I/O with a visible pending state**, never in the frame-request path, and
  "finished before the user draws" is not assumed. Per-frame pixel digests
  are **separate** from the whole-file digest: each covers one frame's
  normalized samples (5.2) in a defined canonical byte layout (dimensions,
  sample format, samples little-endian, interleaved) and is lowercase hex
  like the other digests. How keys use them is the annotation model's call
  (section 10).
- Possible later optimisation: for an 8-bit RGB PNG needing no transform,
  serve the original bytes as the display PNG. Not v1.

---

## 6. EXIF orientation and colour profiles

### 6.1 Orientation (needs you; reaches the annotation model)

JPEG EXIF, TIFF tag 274 and WebP/PNG EXIF can declare one of eight
orientations. The ML ecosystem disagrees on what to do with it:
Pillow's `Image.open` and `torchvision.io.decode_jpeg` ignore it by default;
OpenCV's `imread` applies it by default; browsers apply it. Whatever dcmview
chooses, some user's coordinates will disagree with some library. So the
choice must be declared and recorded, not implied.

- **A. Stored pixel grid, orientation as the initial view transform**
  (recommended). Samples and every coordinate (readout, annotations,
  exports) are in the file's stored raster, the same grid numpy gets from
  Pillow without `exif_transpose`. The viewer opens the tab with the EXIF
  orientation applied through the existing per-tab orientation state
  (`viewTransform.ts` already does rotation 0/90/180/270 plus flipH/flipV,
  which covers all eight cases), and shows an "EXIF orientation" note that
  can be reset. The orientation value is exposed on the wire so adapters can
  produce "as displayed" coordinates on request. This matches how DICOM
  works (coordinates are in the pixel matrix, display may rotate) and keeps
  the pixel pipeline untouched.
- **B. Apply at decode.** The served raster is the displayed one and
  coordinates are in that grid. Matches OpenCV and browsers, but then
  annotations disagree with Pillow/torch loaders by default, and a file whose
  EXIF is later stripped silently changes coordinate frames.
- **C. Ignore it.** Simplest; phone and camera JPEGs open sideways. Not
  acceptable for the colleagues' use.

### 6.2 Colour profiles

- **A. Pass the ICC through in the display PNG** (recommended). This is
  exactly what DICOM colour does today (`encode_rgb8_display_png` with an
  `iCCP` chunk) and the browser converts. Sources: PNG `iCCP`, JPEG APP2,
  TIFF tag 34675, WebP `ICCP`. Pixel values in the raw tier and in
  annotations are never colour-converted. Validate profiles with the existing
  `icc.rs` `normalize_profile`, which today checks only length, padding and
  the `acsp` magic; the raster path also checks the header's data colour
  space (`RGB ` for RGB output, `GRAY` for gray) and drops a mismatched
  profile with a report.
- **CMYK/YCCK JPEG** (amended 2026-09-30): zune-jpeg's conversion to RGB
  is approximate and not profile-driven, so the APP2 CMYK profile no longer
  describes the pixels. The pixels are treated as sRGB, the CMYK profile is
  **dropped** (never copied onto the RGB output), and the file report notes
  "CMYK converted approximately to sRGB; embedded CMYK profile not used".
- **B. Convert to sRGB on the server** (lcms2 or moxcms). Needed only if the
  client ever windows colour itself or composes colour in a canvas that
  ignores ICC. Adds a C or new Rust dependency. Not now.
- **C. Ignore profiles.** Wide-gamut phone photos look dull. Not needed since
  A is free.

PNG `sRGB`/`gAMA`/`cHRM` chunks are dropped when the server re-encodes;
record them in metadata and ignore them for display (rare in medical data).
16-bit RGB loses its profile today on the JPEG 2000 path; the raster path
should keep it after reducing to 8-bit.

---

## 7. Where rasters sit in the clinical hierarchy (needs you)

Rasters have no patient, study or series. Options:

- **A. One "Images" group in the study view, sub-grouped by folder**
  (recommended). A top-level node after the patients, holding the folder
  tree of raster files (same shaping as the directory view). Patients stay
  clean, rasters are one click away, and the gallery can group them by
  folder. The directory view needs no change.
- **B. Synthesize patient/study/series from the path** (for example
  grandparent/parent folders). Some datasets are laid out
  `patient/study/image.png`, many are not; wrong guesses are worse than none.
- **C. Directory view only.** Hides rasters from the default study view,
  confusing for a folder of only PNGs.
- **D. Sidecar mapping.** A CSV or the hub's file list maps each raster to
  patient/study/series IDs. Useful for datasets converted from DICOM. Not v1,
  but the hub's file-list format should leave room for per-file identity
  columns (`seams.md` 6), and the annotation model's label targets should
  allow it.

**Stacks.** Rasters stay out of the series catalog in v1 (it already drops
files without UIDs), so each raster is its own tab and a multi-page TIFF
scrolls as a multiframe file. A folder of same-size slices (`slice_000.png`
...) is a common export of CT volumes; a later "folder stack" (same folder,
same format and size, natural filename order) could join the series catalog
with an id like `folder:<path>`. Recommend deferring it, and noting it for the
gallery doc.

Search: add the path and format to the navigator's searchable values and a
`format:` scope.

---

## 8. The tag panel for rasters

- **A. Reuse `TagNode`** (recommended). Emit a tree in the existing wire
  shape with namespaced tags: `File` (format, size, detected vs extension),
  `PNG:IHDR` / `JPEG:SOF0` / `TIFF:page 0` (dimensions, bit depth, colour
  type, compression, interlace, pages), `ICC` (profile description, colour
  space), `EXIF` (IFD0, EXIF, GPS, with numeric tag ids in the tag column),
  PNG text chunks, TIFF `ImageDescription` (often OME-XML or ImageJ
  metadata), XMP as text. The panel title becomes "Metadata" for rasters
  instead of "DICOM tags". The `vr` column carries the data type (`ASCII`,
  `SHORT`, `RATIONAL`...). No contract change beyond content.
- **B. A new metadata wire type** with sections and typed values. Cleaner,
  but a second panel layout for a secondary feature.

EXIF listing needs a parser: `kamadak-exif` (pure Rust, BSD-2) is the usual
choice and would be the one new dependency. `tags/select` should keep working
for rasters with the same namespaced paths, since the readout uses it.

PHI note: EXIF and text chunks can hold names, dates, GPS and device serial
numbers. Showing them is fine (it is the user's file); a hub should never
copy raster metadata into what it exports.

---

## 9. Test fixtures

- **Committed tiny fixtures from the existing generator** (recommended for
  unit and integration). `examples/generate_test_fixtures.rs` already uses
  `image` and must regenerate byte-identically (`core` profile). Add a
  `tests/fixtures/raster/` set, a few KB each: gray 8/16-bit PNG, 1-bit PNG,
  palette PNG with `tRNS`, RGBA PNG, 16-bit PNG with `sBIT=12` (scaled to
  full depth), PNG with `iCCP`, baseline and progressive JPEG, CMYK JPEG
  with a CMYK APP2 profile, JPEG with each EXIF orientation (APP1 bytes
  written by hand; `image` does not write EXIF), 16-bit and float TIFF,
  WhiteIsZero TIFF (unsigned, signed and float), a TIFF with associated
  alpha, 3-page TIFF plus a thumbnail page, a TIFF whose page 1 differs from
  page 0 in bit depth or photometric (excluded and reported), a tiled TIFF, a
  BigTIFF (the `tiff` encoder writes BigTIFF and multi-page; tiles may need
  hand-built bytes), lossless WebP, a DICOM dual-personality TIFF header, a
  truncated PNG, a PNG named `.jpg`. Each goes in `tests/fixtures/README.md`
  with its purpose. Every fixture asserts **raw sample values** (for
  example the 1-bit PNG serves `[0,1]`, the WhiteIsZero TIFF serves its
  stored values), not only that it decodes.
- **Windowing oracle.** Add raster cases to `tests/windowing-cases.json` so
  server and client agree on the 8-bit identity default, the 16-bit
  percentile default, `sBIT=12` leaving the window at stored depth, and
  MONOCHROME1 inverted once.
- **Test corpus.** `dcmview-test-corpus` is DICOM-only and its generator is
  the external `synth-dicom-gen`. A raster profile there is useful later for
  stress (large tiled TIFF, thousands of pages, very large PNG) and for a
  real-world variety set; it needs a new `artifact_kind` in the registry and
  schema. Recommend not in v1.

---

## 10. Identity requirements for the annotation model

These are requirements and observations, not decisions:

1. **No intrinsic ID.** Rasters have nothing like a SOP Instance UID. EXIF
   `ImageUniqueID` exists but is rare and unreliable.
2. **Discovery does not read content.** A whole-file hash for rasters means
   reading every byte, which DICOM discovery never does. A folder of 100k
   JPEGs at ~1 MB each is ~100 GB of reads. Identity must be computable
   lazily (on first annotation or export), in the background, or supplied by
   the hub (which can hash once at campaign setup and pass keys to spokes in
   the file list). Lazy is not free either: for multi-page TIFF the
   whole-file digest costs extra I/O beyond the frame being viewed, so it is
   background work with a pending state (5.6).
3. **Moves and duplicates.** Path identifies location; a content hash
   identifies bytes. Moved files keep their hash; duplicated files share one.
   Both are common in ML datasets (the same image in `train/` and `all/`).
4. **Metadata edits.** Stripping EXIF or re-saving changes a file hash but not
   pixels. A pixel hash survives that but needs a full decode. Worth deciding
   which one keys annotations and whether the other is recorded.
5. **Frames.** Frame identity for a multi-page TIFF should record the IFD
   index, not only our derived frame index, so a change to the page rule in
   2.4 cannot shift annotations. Single-image formats have one frame, index 0.
6. **Coordinates.** The owner confirmed option 6.1 A: coordinates are in
   the stored pixel grid and the orientation value (1-8) should be recorded
   with the file's identity or the annotation, so an "as displayed" export
   can be derived.
7. **Format recorded.** The detected format belongs in the identity record,
   because the same bytes named `.jpg` or `.png` must match.
8. **Converted datasets.** The same image may exist as DICOM and as PNG. No
   automatic link is possible; a user-supplied external ID column (hub file
   list, section 7 D) is the only realistic route.
9. **EMBED.** EMBED CSV matching is by path and will work for rasters once
   they load; the `anon_dicom_path` column name is then a misnomer, which is
   an adapters doc and EMBED-extension question.

---

## 11. Interfaces for downstream areas

Fixed: the owner confirmed every decision in section 12 on 2026-09-30; amended
the same day after the reviews (the display budget and the fixes listed at the
top).

**Wire and model (dcmview)**

- `FileSummary.file_format`: `"dicom" | "png" | "jpeg" | "tiff" | "webp"`.
- `FileSummary.raster: RasterSummary | null` with `color_type`,
  `bit_depth`, `sample_format` (`uint | int | float`), `has_alpha`,
  `orientation` (1-8, default 1), `has_icc`, `pages_total`, `frame_pages`
  (IFD index per frame), `excluded_pages` (IFD index plus the field that
  differed, 2.4), `animated`, `significant_bits` (from `sBIT`,
  informational only).
- Raw samples are **stored sample semantics** whatever the decoder returns
  (5.2): low-bit PNG at source depth, WhiteIsZero TIFF un-inverted, alpha
  unassociated. MONOCHROME1 display inversion happens once, in the shared
  windowing path.
- Limits: 268 Mpx per frame for `raster.too_large` (host-independent);
  a client display budget (default 64 Mpx) above which the viewer shows a
  downsampled canvas; byte-based decode admission with a bounded queue (503
  with `Retry-After` when full).
- Rasters have empty DICOM identity strings, empty `transfer_syntax_uid`,
  `object_kind = "image"`, and a `support_state` with `raster.*` reasons
  (`raster.too_large`, `raster.unsupported_compression`,
  `raster.unsupported_color`, `raster.jpeg_unsupported_process`).
- Raw tier: rasters use the existing layouts, plus `samples_per_pixel` 2 and
  4 with alpha; samples are always interleaved; photometric `MONOCHROME1`,
  `MONOCHROME2`, `RGB`, `RGBA`.
- `/value-mapping` returns an identity mapping for rasters; `/references` an
  empty list; semantic context "not applicable"; tags a namespaced `TagNode`
  tree.
- Discovery reasons: `valid_image`, `unrecognized_format` (or the old code,
  per 3.3), `raster_header_invalid`. CLI: `--formats`, filter fields `format`
  and `path`.

**The annotation model (`annotation-model.md`)**

- The requirements in section 10.
- Coordinates for rasters live in the stored pixel grid; the display
  orientation is a view transform recorded as metadata.
- Hierarchy label targets for rasters are folder, file and frame only, unless
  a sidecar or hub file list supplies patient/study/series.
- Possible mask sources: palette PNG indices and 1-bit or 8-bit mask PNGs
  (section 5.5), for when masks and imports are designed.

**Integration (`seams.md`)**

- The hub's file list should allow optional per-file columns: precomputed
  identity key, and patient/study/series or external ID (section 7 D).
- Raster support needs no new spoke seam; `--formats` should be passable by
  the hub.

**The gallery (`gallery-views.md`)**

- Rasters group under the "Images" node by folder in study view, and by
  folder in directory view.
- Thumbnail fast paths: the EXIF thumbnail in JPEG APP1 (usually 160 x 120),
  TIFF reduced-resolution pages, and the discovery-time IFD map. JPEG
  DCT-domain scaled decode is available in `jpeg-decoder` (already in the
  lock through `tiff`) if zune-jpeg does not offer it; worth measuring.
  Thumbnails are rendered in the **stored pixel grid**, never rotated or
  flipped on the server; the client applies the EXIF orientation with the
  same transform state the viewer uses (amended 2026-09-30). So a
  stored-grid coordinate maps to a thumbnail coordinate by one scale per
  axis, and orientations 5 to 8, which swap axes, are handled by the client
  transform. An EXIF thumbnail that is already oriented is used only if it
  can be mapped back to the stored grid; otherwise it is skipped.
- Thumbnail sources follow the same normalization (5.2) as the viewer.
- A "folder stack" of same-size slices is a possible later grouping.

**Output adapters (`output-adapters.md`)**

- Adapters must read `orientation` to offer "as displayed" coordinates, and
  should not assume a DICOM UID exists.
- Mask import from 1-bit or low-bit PNG sees values at source depth (0/1),
  not the decoder's 0/255 (5.2).

**The hub**

- Studio-side; not part of the public design. See `seams.md` for the dcmview
  side.

---

## 12. Decisions for the owner

**Status: all nine confirmed by the owner as recommended on 2026-09-30.** The
interfaces in section 11 are fixed on that basis.

1. **Format scope for v1:** PNG, JPEG (8-bit), TIFF (multi-page, 16-bit,
   float, tiled, BigTIFF under a pixel cap) and still WebP; animation,
   12-bit/lossless JPEG, pyramids and exotic TIFF compressions later.
   *Confirmed.*
2. **On by default** with `--formats dicom` to restore DICOM-only, or opt-in
   with a flag? *Confirmed: on by default.*
3. **Discovery reason code:** rename `missing_part10_preamble` to
   `unrecognized_format` (stable wire code, needs sign-off), or keep the code
   and change the text? *Confirmed: rename.*
4. **EXIF orientation:** stored grid with orientation as the initial view
   transform, or rotate at decode? *Confirmed: stored grid.*
5. **Default window:** 8-bit identity, 16-bit percentile or declared range.
   *Confirmed.* **Amended 2026-09-30**: `sBIT` is informational, not
   a declared range; the window keeps the stored-depth range. TIFF
   `MaxSampleValue` still declares a range.
6. **Alpha:** flatten for display, keep as a raw sample. *Confirmed.*
   **Amended 2026-09-30**: associated alpha is un-premultiplied on
   decode, so the raw tier always holds unassociated alpha.
7. **Clinical view placement:** one "Images" group sub-grouped by folder.
   *Confirmed; folder stacks and sidecar mappings later.*
8. **Tag panel:** reuse the tag tree with namespaced tags, and add
   `kamadak-exif` as a dependency. *Confirmed.*
9. **Pixel cap** of 268 Mpx per frame. *Confirmed, adjustable later.*
   **Amended 2026-09-30**: the cap stays at 268 Mpx and no longer
   depends on the raw cache budget; a separate browser display budget
   (default 64 Mpx, downsampled canvas above it) and byte-based decode
   admission with a bounded queue bound browser and decode memory.


---

## Re-baseline amendment (0.3.2, confirmed by the owner 2026-10-05)

This doc was written against dcmview 0.3.1 dev. Since then 0.3.1 added presentation-state graphic annotations drawn over the frame, and 0.3.2 added `--mask` display masking and in-memory redaction boxes edited with a Redact tool. The owner confirmed these resolutions on 2026-10-05 :

- **`--mask` covers rasters.** In a masked session, identifying raster metadata (EXIF, PNG text chunks, TIFF tags) is masked the same way DICOM identifiers are.
- Redaction boxes apply to raster frames as they do to DICOM frames.
