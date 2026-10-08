# dcmview Development Reference

This page collects the source-build and local-development details that are too
long for the README. `dcmview` is a Rust binary with an embedded Svelte frontend
and a Python wrapper that resolves or bundles the same binary.

## Prerequisites

- Rust 1.88+
- Node.js 20.19+ and npm at build time
- CMake and a C++ toolchain at Rust build time for the vendored, statically
  linked CharLS JPEG-LS decoder
- Python 3.9+ for wrappers and the canonical check runner
- `ssh` on `PATH` only when testing SSH forwarding helpers

The Rust baseline matches `Cargo.toml` and CI. The Node baseline matches CI and
the frontend and VS Code development toolchains.

## Canonical Check Profiles

Use `scripts/check.py`; local validation and CI share this check runner:

```bash
# Normal development loop; add --install to run frontend npm ci first
python scripts/check.py quick

# Full deterministic core checks
python scripts/check.py core

# Core plus real-process and VS Code Electron integration
python scripts/check.py e2e

# Opt-in remote fixtures that may use network or cache state
python scripts/check.py external
```

| Profile | Runs |
|---|---|
| `quick` | Version parity, generated frontend contract checks, Svelte/TypeScript checks, Vitest, frontend build, Rust format and strict all-target Clippy, and Python unit tests. It does not run Rust tests or VS Code tests. |
| `core` | Frontend checks/build, Rust format and Clippy, deterministic fixture regeneration that must leave the current fixture tree unchanged, the default-feature, non-ignored locked Rust suite, Python unit tests, and VS Code compilation. |
| `e2e` | `core`, then a real debug-binary build, Python wrapper binary integration, debug-binary HTTP smoke, and VS Code Electron integration. |
| `external` | Only the feature-gated ignored remote-fixture integration tests after building frontend assets. It is separate from `e2e`. |
| `timing` | Startup and discovery timing of this checkout against the released baseline. Opt-in and local; see [Startup And Discovery Timing](#startup-and-discovery-timing). |

Pass `--install` when npm dependencies should be installed from their lockfiles.
Without it, the profiles reuse existing `node_modules`. CI runs focused
component profiles in separate jobs for clearer failure attribution.

## Source Builds

Build everything:

```bash
cargo build
cargo build --release
```

`build.rs` runs the frontend build before embedding assets with `rust-embed`.
It runs `npm ci` only when `frontend/package-lock.json` changes since the last
successful install stamp.

Skip the frontend rebuild only when `frontend/dist/index.html` already exists:

```bash
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo build
```

The build script also honors `DCMVIEW_NODE_PATH` and `DCMVIEW_NPM_PATH` when
they point to absolute executable paths. See the
[configuration reference](configuration.md#build-and-development-environment-variables)
for build-only environment variables.

## Frontend Development

Install dependencies:

```bash
npm --prefix frontend ci
```

Run a backend that the Vite dev server can proxy to:

```bash
dcmview --no-browser --host 127.0.0.1 --port 8888 tests/fixtures
```

Then start the standalone frontend dev server:

```bash
npm --prefix frontend run dev
```

The Vite dev server proxies `/api` to `http://127.0.0.1:8888`, so start a
backend on that host and port before using the standalone frontend.

Frontend checks:

```bash
npm --prefix frontend run check:contracts
npm --prefix frontend run typecheck
npm --prefix frontend run test
npm --prefix frontend run build
```

## Rust Development

Common backend checks:

```bash
cargo fmt --all
cargo fmt --all -- --check
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo check --locked
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo clippy --workspace --all-targets --locked -- -D warnings
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo test --workspace --locked
```

The repository is a Cargo workspace whose root is also the `dcmview` package.
Without `--workspace`, `cargo test` and `cargo clippy` cover only that
package and skip the member crates under `crates/`. `cargo test -p
dcmview-protocol` or `cargo test -p dcmview-annotation` runs one member and
needs no frontend build. After changing a type of the annotation model, run
`cargo run -p dcmview-annotation --example generate_annotation_model` to
rewrite `frontend/src/generated/annotation-types.ts` and the JSON Schema
under `crates/dcmview-annotation/schema/`; the crate's tests fail while
either is stale.

Prefer `python scripts/check.py quick` or `core` for handoff. The individual
commands remain useful for targeted iteration.

Use the `debug-api` feature only when debugging the local viewer API from
another browser origin:

```bash
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo run --features debug-api -- ./study_dir
```

The feature enables permissive CORS and prints a build warning. Do not use it
for ordinary local or remote inspection workflows.

## Python Wrapper Development

Run Python wrapper tests:

```bash
python -m unittest discover -s python/tests
```

The wrapper resolves a bundled binary, `DCMVIEW_BINARY`, or a `dcmview` binary
on `PATH`. See the [Python reference](python.md) for wrapper behavior and the
[configuration reference](configuration.md#binary-resolution) for resolution
order.

## Test Fixtures

Committed fixtures live under `tests/fixtures/` and are generated by:

```bash
cargo run --example generate_test_fixtures
```

Integration tests use real DICOM fixtures and cover discovery, display-frame
decoding, raw-frame transport, cache headers, tag serialization, and
annotations. Do not mock the DICOM layer for integration coverage.

Two upstream fixture cases are behind the `remote-fixtures` feature and ignored
by default because they may download or cache files through
`dicom-test-files`: one loader/API metadata case and one JPEG 2000
display/cache case. Committed JPEG 2000 fixtures remain in the default suite.
Run the canonical external profile:

```bash
python scripts/check.py external
```

## Architecture Summary

The [architecture and test model](architecture.md) is normative. In brief:

- `main.rs` parses the CLI; `application.rs` chooses bridge or local execution.
- `startup/` assembles the local viewer, binds before discovery, and owns
  discovery cancellation and joins through server exit.
- `api/contracts.rs` owns HTTP wire declarations and generates the checked-in
  TypeScript contract used by `frontend/src/api.ts`.
- `crates/dcmview-protocol` owns the launch and startup contract (the
  `--startup-json` line); `api/contracts.rs` re-exports it.
- `crates/dcmview-annotation` owns the neutral annotation model, its
  validation and its operations. The root package takes file keys from it
  (`src/keys/`) and nothing else.
- `server/` separates runtime, lifecycle, catalog, API, tags, and embedded web
  assets. `pixels/` separates service, codecs, caches, windowing, and rendering.
- `App.svelte` composes `FileNavigator`, `OpenImageTabs`, the viewer controls,
  viewport, frame slider, tag panel, and status bar.

## Startup And Discovery Timing

File identity must not slow startup or discovery
(`docs/design/annotation-model.md` 1.7). `scripts/startup_timing.py` is the
check. It is opt-in and local: no profile that CI runs calls it, because a
shared runner's timing is not a gate.

```bash
# This checkout against the released baseline, v0.4.0
python scripts/startup_timing.py run

# Exit 1 when a gated metric is past the threshold
python scripts/startup_timing.py run --enforce

# Two binaries you already have; one profile; more runs
python scripts/startup_timing.py run --baseline A --candidate B --profile cohort --runs 15

# Report against another commit than the merge base, or against none
python scripts/startup_timing.py run --base-ref REF
python scripts/startup_timing.py run --no-base

# The same through the check runner
python scripts/check.py timing
```

**Binaries.** The candidate is a release build of the working tree as it is
on disk, copied to `target/timing/candidate/`, or `--candidate PATH`, or
`--candidate-ref REF`. The baseline is a release build of the tag `v0.4.0`,
or `--baseline PATH`, or `--baseline-ref REF`. A third binary is timed
beside them and only reported: the commit this checkout left the main branch
at (`git merge-base HEAD origin/main`, else `main`), or `--base-ref REF`, or
`--base PATH`; `--no-base` leaves it out, and so does a checkout that is on
the main branch. A ref is built from `git archive` into
`target/timing/refs/<commit>-rustc-<version>-<compiler commit>/`, so no
branch is switched and no worktree is made. It is kept by commit and by
compiler, so it is built once per toolchain and a baseline built by another
compiler is never compared with a candidate built by this one. Every
build reuses this checkout's built `frontend/dist` with
`DCMVIEW_SKIP_FRONTEND_BUILD=1` (the embedded page plays no part in what is
timed) and compiles into one shared `target/timing/build/`, so the
dependencies are built once and the checkout's own `target/release` is not
used. Nothing under `target/` is committed. `DCMVIEW_TIMING_DIR` moves the
whole directory. The binaries and the shared build directory take about
0.7 GB. Run the script from the checkout it times and let it build there:
pointing another checkout's `CARGO_TARGET_DIR` at this `target/` mixes two
source trees in one build directory.

**Inputs.** Synthetic folders written on first use under
`target/timing/inputs/` (about 0.6 GB), with no real data and no download:

| Profile | Files |
|---|---|
| `small` | 16 DICOM files in one folder |
| `study` | 2,000 DICOM files, one study of 8 series |
| `tree` | 20,000 DICOM files, 100 patients in nested folders |
| `cohort` | 100,000 DICOM files, 500 patients in nested folders |
| `duplicated` | 5,000 DICOM files and a byte-identical copy of the whole tree |
| `collision` | 5,000 DICOM files and, for each, a second file with the same SOP Instance UID and another length |
| `no-uid` | 10,000 DICOM files without a SOP Instance UID |
| `masked` | the folder of `tree`, served with `--mask` |
| `images` | 5,000 PNG files in 20 folders |

`duplicated`, `collision` and `no-uid` are the folders where file keys do
more than record a UID: every file is an alias, every UID is contested, or
no file has a key until it is hashed. `masked` is the session that builds
every key from a masked UID.

Each DICOM file is a small CT image with a header of ordinary size;
discovery reads a file only up to its pixel data, so the header is what is
timed. A profile is compared only when both binaries list the same number
of files from it: `v0.4.0` reads no raster images, so `images` is skipped
against it and is useful between two builds that do.

**A binary that cannot serve.** A profile is skipped in that one case: the
baseline answers, lists none of the profile's files, and the candidate
serves it. Nothing else is a skip:

| What happened | Printed | Exit status |
|---|---|---|
| The candidate cannot serve a profile (it exits, never answers, or lists none of the files) | `FAILED`, with the process's exit status and the last lines of its standard error | 1, with or without `--enforce` |
| The baseline cannot serve a profile for any reason but listing none of its files | `NOT COMPARED`, with the same evidence | 2 |
| A process dies during the timed runs | its exit status and standard error; the profile's timed runs are made once more, and a second death is one of the two rows above | as above |
| No local port can be assigned (`Can't assign requested address`: other work holds the machine's ports) | that, in plain words; nothing is compared | 2 |

Status 2 says the comparison could not be made, not that the candidate is
slower: run it again. A merge base that cannot serve is left out of the
report and changes no status.

**What is timed**, from just before the process is spawned with
`--no-browser --no-token --startup-json --port 0`:

| Metric | Ends at | Gated |
|---|---|---|
| first response | the first `200` from `/api/health`, requested as soon as the `server_started` line names the port | yes |
| first file | the first health response that counts a file, polled every 0.2 ms on one open connection | yes |
| scan complete | the `scan_complete` line | yes |
| catalog listing | one `GET /api/files` after the scan, with its size in bytes | no, reported |
| resident memory | the process's resident set (`ps`) when the scan is complete, before the listing, as the median of the runs; not on Windows | no, reported |

"First file" is a short interval measured by asking, so its resolution is
the polling interval plus one round trip, about a third of a millisecond.
Each poll opened a connection and slept a millisecond before; that was a
tenth of the 10 ms it measures on `study`, and the metric swung by more
than 15% between repeats of one binary. The polling ends with the first
file, so it does not weigh on the scan.

Each binary gets one discarded warm-up run per profile, then the binaries
alternate for 9 timed runs each, so drift in the machine's load falls on
all of them.

**The merge base.** The gate compares with the release, and a branch
carries everything merged since the release as well as its own change. The
second table of each profile gives the same metrics against the merge
base, which is the change alone. It is reported and never gated: it says
whether a result against the release is this change or earlier work.

**The gate.** The best (fastest) run of each binary is compared, because
other work on the machine only ever adds time: the fastest run repeats to
within a few percent where the median does not. For each profile, first
response, first file and scan complete each pass when

```text
candidate best of 9 <= v0.4.0 best of 9 x 1.05 + 3 ms
```

This is the gate the owner set on 2026-10-08 for the rule that file
identity must not slow startup or discovery. Changing the percentage, the
floor, the number of runs, the baseline or the gated metrics is the owner's
decision, not a fix for a failing run.

The 3 ms floor covers the metrics that take a few milliseconds, where
scheduling jitter is a large fraction. Both numbers are
`THRESHOLD_PERCENT` and `THRESHOLD_FLOOR_MS` at the top of the script. The
percentage is set by what the instrument can tell apart: the same binary
timed against itself differs by up to about 3% between two best-of-9 sets
on a machine that is doing other work. A failure that two further runs do
not repeat is noise; one that repeats is a regression to fix, not a
threshold to raise. A metric that passes but is more than 2% slower in
repeated runs is reported with the change, since it is a cost even when it
is under the gate.

## Cache Budgets

Backend frame cache budgets default to 256 MiB for display PNGs, 384 MiB for
raw sample frames, 64 MiB for overlays and 64 MiB for thumbnail JPEGs (768 MiB
total). `--cache-budget` sets one total that is split in those proportions.
Thumbnails only fill their own cache, preserving the viewer's working set.
The frontend also keeps active frame blobs, raw buffers, and rendered bitmaps
in memory for responsiveness. Frontend retention follows
the selected logical stack rather than an individual source file, and releases
entries through byte-budgeted LRU eviction or stack disposal. Cache budget
changes should therefore consider total browser plus server memory pressure.
