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
