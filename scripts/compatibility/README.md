# DICOM compatibility campaign

This directory owns dcmview's manifest-driven compatibility automation. It
measures viewer behavior; it does not grade DICOM conformance or clinical
suitability. The valid-corpus path consumes one explicitly pinned producer
container. It does not freeze a sibling checkout, merge a corrected worklist,
or invoke a generator from the viewer repository.

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

Record a bounded stress baseline independently from a caller-supplied worklist:

```bash
python scripts/compatibility/stress_runner.py \
  --worklist /outside/stress-worklist.json \
  --binary target/debug/dcmview \
  --output /outside/stress-run-1
```

The stress runner records discovery, concurrency, frame latency, cache behavior,
and bounded output. It reports observations rather than inventing performance
thresholds that are absent from the supplied worklist.

The fuzz profile contains qualification evidence but no reusable payload
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
jobs, or the release process. Their worklists are caller-supplied inputs and
are validated by the viewer-owned generic worklist parser; they do not require
a generator checkout.
