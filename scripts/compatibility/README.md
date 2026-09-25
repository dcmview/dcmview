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
  --output /outside/smoke-run-1
```

The consumer accepts only artifact descriptor schema `2.0.0`. It checks that
`artifact-index.json`, the archive, the generated manifest, and every payload
agree on their hashes and sizes, and that the index describes a closed
generator release descriptor. It rejects symlinks, hard links, special
entries, path traversal, duplicate members, undeclared outer files, and TOCTOU
changes. The archive is extracted into a private closed tree with no-follow
reads; only after the tree is closed do its DICOM paths enter the viewer
worklist. Trust in *which* container is being tested comes from the committed
ZIP digest described below, not from per-field pins.

After those checks, the same existing compatibility runner starts the supplied
`dcmview` binary with the verified DICOM paths and performs the normal metadata,
display, raw-frame, cache, error-recovery, and assertion-backed HTTP probes.
The companion viewer report is checked against the viewer-owned
`scripts/compatibility/viewer-report.schema.json`; no generator-owned schema is
needed at consumption time.
Viewer failures remain viewer-owned outcomes; a successful artifact check does
not claim that every case renders.

### Manual CI workflow

`.github/workflows/compatibility.yml` runs
`python scripts/check.py compatibility-artifact` and is dispatched manually
only; it never runs on push or pull request. It reads
`scripts/compatibility/corpus-artifact.json`:

| Field | Meaning |
|---|---|
| `repository` | Producer repository (`owner/name`) |
| `run_id` | Producer workflow run that uploaded the artifact (provenance record) |
| `artifact_id` | Numeric Actions artifact ID to download |
| `zip_sha256` | SHA-256 of the downloaded artifact ZIP (`sha256:` prefix allowed) |

The workflow downloads the artifact by ID with the
`DCMVIEW_COMPAT_CORPUS_TOKEN` secret, checks the ZIP against `zip_sha256`,
and extracts it with `extract_github_artifact.py`. It fails with a clear error
while no artifact is pinned.

To adopt a new producer artifact, pick a successful default-branch run of the
producer's publish workflow, read the artifact's `id` and `digest` from
`gh api repos/<repository>/actions/runs/<run_id>/artifacts`, commit them to the
pin file on a branch, and dispatch the workflow on that branch. The pin change
is reviewed like any other commit. The viewer repository never checks out or
builds the generator.

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
are validated by the viewer-owned generic worklist parser. The supported 0.2
shape selects exactly one of `negative`, `stress`, or `fuzz`; JSON size and
entry counts are bounded, payload paths must be confined regular files, and
each declared payload SHA-256 and size is checked before the viewer launches;
per-payload and aggregate staging budgets are bounded as well.
Negative and stress payloads are copied from no-follow verified descriptors
into a private temporary tree owned by the runner for its lifetime, then
cleaned up; the fuzz profile remains payload-free.
Malformed worklists fail with a concise runner error and do not require a
generator checkout.
