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
  validation and its operations. The root package does not depend on it.
- `server/` separates runtime, lifecycle, catalog, API, tags, and embedded web
  assets. `pixels/` separates service, codecs, caches, windowing, and rendering.
- `App.svelte` composes `FileNavigator`, `OpenImageTabs`, the viewer controls,
  viewport, frame slider, tag panel, and status bar.

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
