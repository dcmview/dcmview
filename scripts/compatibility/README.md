# DICOM compatibility campaign

This directory owns DICOM View's manifest-driven compatibility automation. It
measures viewer behavior; it does not grade DICOM conformance or clinical
suitability.

Freeze the prepared corpus scope before running any probes:

```bash
python scripts/compatibility/scope.py \
  --suite-root /path/to/dicom-test-suite \
  --output /path/outside/both/repos/canonical-worklist.json
```

The scope freezer verifies every manifest-selected file hash, records each
manifest hash, and deduplicates copies only when both the DICOM file hash and a
normalized expected-contract hash match. The output is created read-only and
will not be replaced by non-identical content. It separately records execution
safety (`safe`, `timeout`, `crash`, `flaky`) and compatibility outcomes (`full_support`,
`metadata_only`, `known_gap`, `failure`, `unavailable`).

When a smaller corrected corpus is a strict subset of an already frozen
canonical corpus, merge it without weakening the canonical inventory:

```bash
python scripts/compatibility/merge.py \
  --base /outside/canonical-worklist.json \
  --overlay /outside/corrected-subset-worklist.json \
  --output /outside/corrected-canonical-worklist.json
```

The merger re-verifies both worklist content hashes, every selected DICOM hash,
and every expected-contract hash. An overlay row may replace a base row only
when case ID, selected relative path, SOP Instance UID, DICOM identity, image
layout, and UIDs remain unchanged; the DICOM file and expected contract must
change together. The merged worklist records both source worklist hashes and
the exact replacement count.

Run a bounded HTTP campaign against a previously built binary:

```bash
python scripts/compatibility/run.py \
  --suite-root /path/to/dicom-test-suite \
  --worklist /outside/canonical-worklist.json \
  --binary target/debug/dcmview \
  --root smoke \
  --output /outside/smoke-run-1
```

The runner launches a loopback, port-zero, no-browser process with structured
startup output and VS Code bridge bypass. It waits for `scan_complete`, maps
files by normalized path plus SOP Instance UID, and probes file information,
tags, display and raw frames, structured errors, and cache MISS/HIT behavior.
It also captures the completed `/api/series` catalog once per shard and records
each file's logical stack, virtual position, ordered source frames, geometry
warnings, concatenation identity, and WSI level/companion classification.
Manifest-declared series capabilities are evaluated from that server-owned
evidence rather than by reimplementing DICOM ordering in the harness. For
reference-bearing objects, the runner compares the typed reference endpoint to
the manifest identity closure and requires every declared SOP/path/frame target
to resolve locally before marking reference resolution as probed. Physical
pixel aspect is compared exactly from `/api/files`; that observation is labeled
as API metadata evidence and does not claim a real-browser layout check.

For lossless transfer syntaxes, every manifest frame hash is compared with the
decoded raw endpoint, not only the first frame. The first and last display
frames are also decoded far enough to validate PNG dimensions, while declared
small visual patterns are checked from decoded PNG pixels where an explicit
validator exists.

Presentation evidence follows the same exact-contract rule. The prepared
2-by-2 diagonal overlay cases use full-dynamic display mode and mark
`read_overlay_plane` as probed only when the declared overlay pixels are white
and the two non-overlay pixels retain their exact expected grayscale values.
This isolates overlay evidence from VOI/windowing behavior. The prepared
rectangular shutter opening covers the full image, so
the report records its bounds and a passing display non-regression check but
keeps `apply_display_shutter` explicitly unprobed: there are no outside-opening
pixels with which to prove replacement. When a manifest supplies an exact ICC
profile size and SHA-256, the runner decompresses the PNG `iCCP` chunk and
compares those bytes. That proves profile preservation only;
`apply_icc_profile` remains unprobed because neither a numeric color transform
nor frame-to-optical-path profile selection is measured.

It writes separate process logs, a companion-schema detail report, a timing-free
normalized report for reproducibility comparison, and a SHA-256 artifact index.
The normalized form removes transient registry indices and omits body hashes and
sizes only for index-bearing series and reference JSON responses. Stable
path/SOP identity and pixel payload hashes remain available for comparison.

## Stored current smoke artifact

The current viewer path consumes the exact producer container directly. It does
not build or invoke `synth-dicom-gen`, and it does not require a
`dicom-test-suite` checkout. The container contains sibling
`smoke.tar.gz` and `artifact-index.json`; the deterministic tar has exactly
`corpus/manifest.json` plus the manifest-declared DICOM payloads:

```bash
python scripts/compatibility/run.py \
  --corpus-root /outside/producer-container \
  --binary target/debug/dcmview \
  --output /outside/smoke-run-1 \
  --expected-generator-revision "$DCMVIEW_CORPUS_GENERATOR_REVISION" \
  --expected-generator-artifact-sha256 "$DCMVIEW_CORPUS_GENERATOR_ARTIFACT_SHA256" \
  --expected-generator-artifact-size-bytes "$DCMVIEW_CORPUS_GENERATOR_ARTIFACT_SIZE_BYTES" \
  --expected-target "$DCMVIEW_CORPUS_GENERATOR_TARGET" \
  --expected-toolchain "$DCMVIEW_CORPUS_GENERATOR_TOOLCHAIN" \
  --expected-runtime-identities-sha256 "$DCMVIEW_CORPUS_RUNTIME_IDENTITIES_SHA256" \
  --expected-definition-manifest-sha256 "$DCMVIEW_CORPUS_DEFINITION_MANIFEST_SHA256" \
  --expected-generator-version 0.3.0 \
  --expected-manifest-sha256 "$DCMVIEW_CORPUS_MANIFEST_SHA256" \
  --expected-manifest-size-bytes "$DCMVIEW_CORPUS_MANIFEST_SIZE_BYTES" \
  --expected-corpus-definition-sha256 "$DCMVIEW_CORPUS_DEFINITION_SHA256" \
  --expected-profile smoke --expected-seed "$DCMVIEW_CORPUS_SEED" \
  --expected-binding-id "$DCMVIEW_CORPUS_BINDING_ID" \
  --expected-archive-sha256 "$DCMVIEW_CORPUS_ARCHIVE_SHA256" \
  --expected-archive-size-bytes "$DCMVIEW_CORPUS_ARCHIVE_SIZE_BYTES"
```

The consumer verifies the archive digest and size, the index binding ID, the
generator revision/artifact SHA-256 and size/target/toolchain/features,
runtime identities, both definition digests, generated manifest digest/size,
profile/seed, and every payload hash/size. It rejects symlinks, hard links,
special entries, path traversal, duplicate members, undeclared outer files,
and TOCTOU changes. The archive is extracted into a private closed tree with
no-follow reads; only after the tree is closed do its DICOM paths enter the
viewer worklist. All immutable pins are required for the CI consumer.

After those checks, the same existing compatibility runner starts the supplied
`dcmview` binary with the verified DICOM paths and performs the normal metadata,
display, raw-frame, cache, error-recovery, and assertion-backed HTTP probes.
The companion viewer report is checked against the viewer-owned
`scripts/compatibility/viewer-report.schema.json`; no generator-owned schema is
needed at consumption time.
Viewer failures remain viewer-owned outcomes; a successful artifact check does
not claim that every case renders.

Viewer CI exposes this as `python scripts/check.py compatibility-artifact`.
It downloads the GitHub artifact API URL addressed only by the configured
repository, workflow run ID, and numeric artifact ID, verifies the downloaded
ZIP digest (the GitHub `sha256:` prefix is accepted and normalized), and safely extracts the producer container with
`extract_github_artifact.py`. The repository variables are the corresponding
`DCMVIEW_CORPUS_ARTIFACT_*` locator/digest values plus the complete
`DCMVIEW_CORPUS_*` index-pin set used above. A partially configured variable
set fails closed; an entirely unconfigured lane is skipped. No mutable name or
`latest` lookup is accepted, and ordinary viewer jobs still do not check out or
build the generator.

## Robustness profiles

Negative, stress, and fuzz qualification are deliberately separate from the
valid-corpus campaign. Each command is bounded, writes a machine-readable
report with bounded output evidence, and launches only the supplied viewer
binary. The valid-corpus runner additionally writes process logs and a SHA-256
artifact index.

Run the 15 isolated negative cases with a known-good recovery object:

```bash
python scripts/compatibility/negative_runner.py \
  --worklist /outside/negative-worklist.json \
  --binary target/debug/dcmview \
  --healthy-file tests/fixtures/valid-uncompressed.dcm \
  --output /outside/negative-run-1
```

Every case must terminate within its deadline, match its declared failure
layer, avoid a crash, and leave a fresh viewer able to serve the healthy file.

Record the suite stress baseline independently:

```bash
python scripts/compatibility/stress_runner.py \
  --worklist /outside/stress-worklist.json \
  --binary target/debug/dcmview \
  --output /outside/stress-run-1
```

The stress runner records discovery, concurrency, frame latency, cache behavior,
and bounded output. It reports observations rather than inventing performance
thresholds that are absent from the pinned suite.

The suite fuzz profile contains qualification evidence but no reusable payload
corpus. Run deterministic, payload-disciplined mutations from an explicit seed:

```bash
python scripts/compatibility/fuzz_runner.py \
  --worklist /outside/fuzz-worklist.json \
  --binary target/debug/dcmview \
  --healthy-file tests/fixtures/valid-uncompressed.dcm \
  --seed-file tests/fixtures/valid-uncompressed.dcm \
  --output /outside/fuzz-run-1
```

Candidate count, mutation count, input bytes, target operations, wall time,
process output, response size, and retained failing artifacts are all capped.
Generated payloads are not retained when every candidate is rejected cleanly.

These workflows remain local and opt-in. They are not wired into CI, scheduled
jobs, or the release process, and the pinned `dicom-test-suite` checkout is
always treated as read-only input.
