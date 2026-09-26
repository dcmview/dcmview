# DICOM compatibility runner

`run.py` checks a real `dcmview` binary against a stored `synth-dicom-gen`
smoke corpus. It is a local validation tool and is not run by any CI workflow.
It measures viewer behavior for research inspection; it does not grade DICOM
conformance or clinical suitability.

```bash
python scripts/compatibility/run.py \
  --corpus-root /path/to/container \
  --binary target/debug/dcmview \
  --output /path/to/empty-output-dir
```

`--corpus-root` is a local producer container: `smoke.tar.gz` plus
`artifact-index.json`. Before the viewer starts, the runner checks the archive
against the index digest, extracts it (regular files only, no paths outside
the corpus), and checks every payload against the SHA-256 and size in
`corpus/manifest.json`. It then launches the binary on every payload and runs
these checks per file over HTTP:

| Check | Oracle |
|---|---|
| `metadata` | Manifest UIDs, SOP class, transfer syntax, geometry, and declared tag values |
| `raw_frame_hashes` | SHA-256 of every decoded raw frame equals the manifest frame hash (lossless) |
| `raw_lossy_error` | JPEG Baseline samples stay within the manifest's max-error/RMSE tolerance |
| `display_exact` | Every navigated display frame equals the 8-bit output computed from the recipe samples |
| `display_frames`, `raw_headers`, `cache` | PNG geometry, raw metadata headers, `X-Cache` MISS then HIT |
| `unsupported_transfer_syntax`, `metadata_only`, `error_recovery` | 422/404 JSON errors, and the server keeps serving |
| `references`, `series`, segmentation / parametric map / RT dose, `wsi_*`, modality tags | Declared manifest expectations, where the entry declares them |

`display_exact` computes the expected frame independently of dcmview: stored
value, Rescale Slope/Intercept, the DICOM LINEAR window, then MONOCHROME1
inversion. A declared window is checked through the default request; otherwise
the check uses `mode=full_dynamic` (the window spanning the frame's rescaled
minimum and maximum), because the default fallback is a percentile heuristic.
Pixel Padding samples are excluded from that range and not asserted. RGB frames
are compared sample for sample. Cases whose display depends on data the
manifest does not carry (Modality/VOI LUT and palette tables, YBR color, overlays
and shutters, non-identity presentation LUTs, lossy JPEG) are reported as not
computable with the reason, rather than passed.

The runner writes `report.json` (per-file checks and a per-check tally) plus
the viewer's stdout/stderr logs, prints a summary, and exits 1 if any check
failed or 2 if the corpus or viewer could not be prepared.

## Local use

The corpus is handled locally only. Assemble a container from the local
`synth-dicom-gen` corpus: pack its `corpus/` directory (with `manifest.json`)
as `smoke.tar.gz`, and write `artifact-index.json` with `archive_sha256` and
`archive_size_bytes`. Then run:

```bash
DCMVIEW_COMPAT_CORPUS_ROOT=/path/to/container python scripts/check.py compatibility-artifact
```

For changes to codecs or the display pipeline, run it on the base commit and
again on the change. Every check that passed on the base must still pass.

The runner's own unit tests (`test_run.py`) run in the `python-unit` check
layer.
