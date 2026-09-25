# Contributing to dcmview

`dcmview` is a temporary DICOM inspection tool for research and development
workflows. It is not for clinical diagnosis or clinical decision-making.

## Privacy And Safety

DICOM files, screenshots, logs, paths, metadata, and annotation CSVs can contain
PHI, patient identifiers, study identifiers, institution names, hostnames, or
other sensitive research data. Do not post that material in public issues,
pull requests, discussions, or logs unless it is fully de-identified and
approved for public sharing.

Use synthetic fixtures, redacted logs, or minimal reproduction steps whenever
possible. Report suspected security vulnerabilities privately; see
[SECURITY.md](SECURITY.md).

## Development Setup

Prerequisites:

- Rust 1.88+
- Node.js 20.19+ and npm for frontend and VS Code builds
- Python 3.9+ for wrappers and repository checks

Install frontend dependencies:

```bash
npm --prefix frontend ci
```

Build the Rust binary:

```bash
cargo build
```

During backend-only development, you may skip the frontend rebuild only when
`frontend/dist/index.html` already exists:

```bash
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo check --locked
```

See the [development reference](docs/development.md) for source builds and the
[architecture and test model](docs/architecture.md) for normative module
ownership, contracts, lifecycle invariants, and test seams.

## Tests And Checks

Use the canonical profile that matches the highest boundary changed:

```bash
# Fast development feedback; add --install when dependencies need npm ci
python scripts/check.py quick

# Deterministic frontend, Rust, Python-unit, and VS Code-compile coverage
python scripts/check.py core

# Core plus real binary, HTTP smoke, Python, and VS Code Electron integration
python scripts/check.py e2e

# Independent opt-in remote-fixture coverage
python scripts/check.py external
```

`quick` does not run Rust tests or VS Code tests. `core` adds deterministic
fixture regeneration checks, the default-feature, non-ignored locked Rust
suite, and VS Code compilation. `e2e` adds the real-process layers. `external`
is separate because it may download or cache remote DICOM fixtures. See the
[profile reference](docs/development.md#canonical-check-profiles) for the exact
contents.

Use targeted commands while iterating, then run at least `core` when changing
shared HTTP contracts, server behavior, pixel decoding, generated types, or
fixtures. Use `e2e` for process startup, Python wrapper, bridge, smoke, or VS
Code integration changes.

For `debug-api` changes, also run:

```bash
DCMVIEW_SKIP_FRONTEND_BUILD=1 cargo check --features debug-api --locked
```

## Fixture Policy

Committed DICOM fixtures live in `tests/fixtures/` and are intentionally small,
synthetic, and generated from repository code:

```bash
cargo run --example generate_test_fixtures
```

Do not commit real clinical data, institutional data, screenshots of real data,
or unapproved downloaded DICOM files. Integration tests should exercise the real
DICOM layer with generated fixtures instead of mocking decoding, metadata, or
transport behavior.

Remote fixture coverage is feature-gated because it can download or cache data:

```bash
python scripts/check.py external
```

## Documentation Expectations

Update documentation when a change affects install paths, CLI flags, Python
wrapper behavior, VS Code settings, environment variables, HTTP API behavior,
annotation CSV semantics, release steps, or troubleshooting guidance.

Useful entry points:

- [Documentation index](docs/index.md)
- [Configuration reference](docs/configuration.md)
- [Troubleshooting guide](docs/troubleshooting.md)
- [Python reference](docs/python.md)
- [Internal API reference](docs/api.md)
- [Architecture and test model](docs/architecture.md)
- [Release process](docs/releasing.md)

## Pull Requests

Keep pull requests focused on one coherent change. Include:

- The behavior or documentation changed.
- The tests or checks run.
- Any known limitations or follow-up work.
- Confirmation that no PHI, sensitive DICOM data, sensitive screenshots, or
  unredacted logs are included.

Use the existing Rust, Svelte, Python, and VS Code patterns in the repository.
Avoid unrelated refactors when fixing a bug or adding a narrowly scoped feature.
