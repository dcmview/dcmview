# Testing review — 2026-09-05

Assessment of commit `117a5909b131a090c1fefd321ecab6385ee1dee4`.
This is a review and proposed plan, not a change to the normative check profiles.

## Overall assessment

The repository has a sound foundation: real DICOM fixtures, independently
exercised HTTP contracts, deterministic lifecycle seams, fast frontend behavior
tests, and separate installed-artifact checks. Most sampled tests protect real
failure modes. There is no evidence that wholesale removal of small tests would
produce meaningful savings.

The main inefficiency is selection and setup. CI runs unrelated jobs on every
PR, Rust profiles build frontend assets and regenerate fixtures, and ordinary
extension logic runs inside Electron. Meanwhile, compatibility-tool unit tests
and real browser acceptance are outside the automated CI gate.

Priorities are: select relevant domains; make existing coverage more honest;
automate a few important missing integration paths; then optimize measured
costs. A hardcoded value is useful when it is an independent expected result
for a meaningful behavior. It is weak when it merely repeats a constant,
implementation detail, or setup performed by the test itself.

## Evidence and verification

Inspected the local runner, both GitHub workflows, test entry points, build
integration, architecture documentation, representative Rust/TypeScript/Python
tests, and compatibility tooling. Test-value judgments below are specific
examples, not a claim that every assertion received an exhaustive audit.

| Suite executed locally | Result | Reported execution time |
|---|---|---|
| Frontend Vitest | 100 passed, 18 files | 598 ms total Vitest duration |
| Python default discovery | 66 passed | 53 ms |
| Compatibility-tool unit discovery | 68 passed | 94 ms |
| Frontend contract generator tests | 18 passed | 1.278 s |
| Rust library | 149 passed, 6 ignored | 20 ms |
| Rust binary unit tests | 36 passed | 30 ms |
| Rust integration | 96 passed, 4 ignored | 2.31 s |

These are one local sample with installed dependencies, not CI benchmarks.
Rust initially compiled dependencies and encountered sandbox restrictions on
loopback listeners. A rerun with loopback access passed: 281 tests, 10 ignored,
with a 0.30 s incremental build. The ignored cases are eight prepared-corpus
tests and two remote-fixture placeholders. The full `core`/`e2e` profiles,
fixture regeneration, Electron, and external corpus campaigns were not run.

The [September 1 CI run](https://github.com/dcmview/dcmview/actions/runs/33461420110)
finished in 3m59s. Job durations included wheel packaging 3m50s, Windows Rust
2m13s, Linux Rust 1m24s, macOS Rust 1m01s, Python integration 1m00s,
frontend 29s, and the two Electron jobs 26s and 36s. The sum of job elapsed
times was about 13 runner-minutes, not an estimate of billing. That run tested
`43c2cad`, not the current local commit; use it as an illustrative historical
sample, not certification of the current checkout or a runtime baseline.

An [August 29 failure](https://github.com/dcmview/dcmview/actions/runs/33227397976)
included a Windows JPEG comparison differing by three luminance levels. The
current multiframe test allows a tolerance of three. This illustrates why
lossy-codec tolerances need explicit justification; it does not establish a
flake rate or justify discarding pixel assertions.

## What is already valuable

- **Pixel oracles:** raw sample equality, bit packing, endian conversion,
  signed values, DICOM half-unit window boundaries, LUT order, and rescale
  precedence. Small exact arrays make these errors easy to detect and diagnose.
- **Behavior under concurrency:** stale request suppression, revisioned
  annotation persistence, cache eviction/resource cleanup, and discovery
  cancellation. These protect data and resource ownership, not trivia.
- **HTTP boundaries:** shared JSON errors, extractor rejection, PNG decoding,
  raw metadata, and cache MISS/HIT transitions. Unit tests alone cannot prove
  router/extractor/header behavior.
- **Real fixture and process paths:** loading DICOM through the loader and
  server catches integration errors that manually constructed entries cannot.
  Installed wheels and extracted archives establish different invariants from
  in-process Axum tests; keep these boundaries represented.
- **Generator mutation checks:** changing Serde naming or endpoint declarations
  and observing generated output tests meaningful generator behavior. Do not
  remove these merely because they assert strings.
- **Compatibility evidence:** immutable inputs, raw hashes, bounded processes,
  and explicit distinctions between unprobed and supported behavior are strong.
  Preserve that honesty when adding CI coverage.

## Remove, consolidate, relocate, or strengthen

| Location | Assessment | Recommended action |
|---|---|---|
| `tests/integration/pixels_jpeg_decode.rs:69`, `jpeg_lossless_routes_through_server_decode` | Only rejects one combination: successful raw JPEG pass-through. A 404, 422, empty response, or unrelated failure can pass. | Replace with an explicit decode-error status/code/envelope assertion and successful recovery request. Keep existing valid lossless fixture coverage. |
| `python/tests/test_release_archive.py:32` | The test writes the `.sha256` sidecar itself, then validates it. Production could stop creating sidecars and this test would still pass. | Exercise the packaging entry point and inspect the sidecar it creates. Preserve archive-member and independent digest checks. Remove test-authored sidecar setup. |
| `python/tests/test_check_profiles.py` | Exact call ordering and the human-readable external label duplicate runner implementation. Layer inclusion and remote isolation are useful. | Remove label equality and incidental ordering requirements. Test required/forbidden layers, dependency ordering, deduplication, and selector behavior. Keep semantic flags such as `--locked` and remote isolation. |
| `frontend/src/lib/semanticPresentation.test.ts` | Literal mode labels have little defect-detection value; supported-object gating and ambiguous overlay rejection are substantive. | Remove or consolidate trivial label-only cases. Retain unavailable/missing mapping and selection behavior. Expect negligible runtime savings. |
| `src/bridge/protocol.rs:61` | Asserting that static fixture fields contain `POST` and `/launch` does not prove that the client uses them. | Drop these literal fixture checks once live request contract coverage owns method/path verification. Keep serialization naming, optional-field omission, and shared response parsing. |
| `tests/integration/server_minimal.rs:289` | The name claims a valid image, but checks only status/media type/cache headers. Similar window/cache behavior exists elsewhere. | Fold into the stronger window/cache tests, or actually decode and compare pixels. Preserve the independent cache-slot scenario. |
| `tests/integration/api_contract.rs:519` | Exact object-key equality repeats DTO inventories and rejects additive fields. | Keep exactness only where a closed shape is intentional. Otherwise assert required fields/types and stable error codes, backed by generator/serialization conformance. Do not derive every oracle from production constants. |
| `tests/integration/golden_fixtures.rs:127` | JPEG expectation calls production `resolve_window`; a bug in window selection can affect both sides. | Keep this as codec integration evidence, but use an independently specified window/output for tests claiming window-selection correctness. |
| `tests/integration/golden_fixtures.rs:151` | Decodes an 8.5-million-pixel image, but checks dimensions and never opens a browser despite the viewer-geometry name. | Rename as a large-image decode smoke; move to a scale/browser lane if timing shows material cost. Retain small non-square geometry cases in core. |
| `vscode/src/test/suite/extension.test.ts:154` | Shim substring checks prove token presence, not shell quoting, argument preservation, or fallback execution. | Replace with bounded executable shim tests using a recording helper and paths/arguments containing spaces. Run each shell on its actual platform. |
| Same extension suite overall | Most of its 20 cases exercise parsing, policies, or filesystem helpers. | Extract helpers from the `vscode` import boundary and run them under Node. Keep command activation, custom editor, and actual session lifecycle tests in Electron. |

Do not bulk-delete all repeated cache checks: cache-key semantics at unit level,
HTTP headers at API level, and installed binary behavior are distinct. Remove
duplication within a boundary after documenting which remaining test owns each
invariant. Parameterization improves maintenance but does not itself save much
execution time.

## Coverage gaps that matter more than adding test counts

1. **Compatibility-tool tests are orphaned from ordinary checks.** Default
   Python discovery only searches `python/tests`; the 68 tests under
   `scripts/compatibility` are absent from `quick`, `core`, `e2e`, and CI.
   Add an explicit `compatibility-unit` lane. These tests need no external corpus.
   Likewise, the complete `marketing` validation profile is not invoked by CI,
   even though its Python unit tests are included through default discovery.
2. **There is no automated real-viewer acceptance gate in the checked-in CI.**
   Vitest tests helpers and fetch wrappers; the documented browser acceptance
   is not invoked by a test profile. Add a small real Svelte/browser suite against
   committed fixtures: visible frame change, windowing, file-switch race,
   transformed/frame-scoped ROI edit and export, metadata-only state, and recovery.
   Assert canvas/output/network behavior instead of full-page pixel snapshots.
3. **Electron coverage is shallow at the actual product boundary.** Public
   commands are registered, but tests do not launch the real viewer, open a
   DICOM editor session, and stop it through the extension. Add one such journey
   after moving pure helpers out of Electron. Current CI tests minimum VS Code
   and non-blocking Insiders, not current stable; add stable coverage and move
   Insiders to a scheduled advisory lane.
4. **Contract completeness is partly conventional.** The every-endpoint loop
   explicitly skips SEG overlay, relying on a separate test. Use a coverage
   registry that requires every operation to have a scenario, including special
   setup, and fails when a newly added operation has none.
5. **Cross-renderer consistency deserves one shared independent oracle.** Rust
   and browser windowing are separately tested. Feed both a small independently
   authored matrix of signed/rescaled/windowed samples, including threshold and
   constant-frame edges. Supplement with real-browser wiring tests.
6. **Performance evidence is outside normal gates.** Reuse existing stress
   timing/RSS tooling for a bounded scheduled baseline. Measure startup, first
   decoded frame, sequential/random frame access, and memory plateau. Report
   hardware, build type, input hashes, repetitions, and distributions; avoid
   tight wall-clock assertions on shared PR runners.

## Make selection modular locally and remotely

Keep `core`, `e2e`, and `external` as explicit broad checks. Add a shared suite
manifest and a dependency-aware planner used by both `check.py` and CI. Proposed
interfaces: `check.py affected --base <ref> --explain` and
`check.py suite <name>`. These commands do not exist today.

Start with domain selection, not function-level impact inference. The full
frontend suite takes under a second here; filtering individual frontend tests
is primarily a local convenience. Rust remains one package with one aggregate
integration target, so a name filter reduces execution but does not eliminate
compilation of the target. Split integration targets by domain only when
measured compile/link or operational isolation benefits justify it; do not
create many crates just to obtain narrower commands.

| Changed domain | Required focused work |
|---|---|
| Frontend presentation/helpers | Frontend checks; relevant browser journey; real asset build/serve smoke for shipped UI changes |
| Frontend raw rendering | Above plus shared pixel/window oracle |
| Pixel codecs/windowing | Rust pixel units and pixel HTTP integration; relevant codec/fixture tests; cross-renderer checks for shared semantics |
| Loader/catalog/series/references | Relevant Rust units and API scenarios; discovery/startup checks where affected; consumer navigation tests for changed wire behavior |
| API contracts, generator, shared types | Contract generation and runtime conformance; frontend consumers; affected Rust and wrapper/extension consumers |
| Server/startup/lifecycle/bridge | Rust lifecycle/API tests; real binary smoke; wrapper or extension integration according to the affected boundary |
| Python wrapper | Wrapper units and real binary integration; wheel verification when installation/resolution changes |
| VS Code extension | Node units, compile, relevant Electron integration; Rust bridge tests for protocol changes |
| Packaging/build/dependency files | Relevant artifact checks and platform matrix; compiler/feature checks as applicable |
| Fixture generator or fixtures | Dedicated regeneration/drift check and all consuming suites |
| Compatibility tooling/policy/schema | Compatibility units; bounded corpus smoke when runtime or support claims change |
| Marketing and prose | Marketing validation or docs checks; README still affects distribution contents |
| Workflow/planner changes, unknown paths | Validate selection and conservatively run broad coverage |

Important selector rules:

- Use PR merge-base comparison, not only the last commit. Include staged,
  unstaged, and untracked changes locally, and both old/new rename paths.
- Expand dependencies transitively. `docs/contracts/*.json` are executable
  cross-language fixtures, so a blanket `docs/**` exclusion would be wrong.
- Handle deleted files, empty diffs, missing base refs, and planner failures
  explicitly. Unknown inputs should widen coverage and explain why.
- List selected suites and reasons before execution; fail if a requested
  suite/filter unexpectedly discovers no tests.
- Test the planner with representative diffs and consumers. Keep full coverage
  on main and a scheduled/manual lane as protection against selection mistakes.
- Vitest supports `related` and `--changed`; use these for local helper iteration
  with a whole-suite fallback for shared configuration and non-imported assets.
  See the [Vitest CLI documentation](https://main.vitest.dev/guide/cli).

## Infrastructure changes, in priority order

**First: reduce unrelated jobs and expose missing suites.** Extend the existing
packaging-only change detector to domain outputs. Keep the workflow triggered
and gate individual jobs. Provide an always-running final required check that
validates the selected jobs actually succeeded; reject unexpectedly skipped
selected jobs. Workflow-level path skips can leave required checks pending,
whereas skipped conditional jobs report success, making explicit aggregation
important. See [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)
and [job conditions](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-jobs-with-conditions).
Actual branch-protection settings were not inspected.

**Second: separate work from prerequisites.** `rust_lint()` currently builds
frontend assets before even running formatting. Make formatting independent.
Separate fixture regeneration from `rust_test()`, retaining drift checks on
generator/fixture/codec dependency changes and broad qualification runs. Generate
into a temporary directory and compare outputs so checks do not overwrite local
fixtures. Preserve cross-platform determinism checks in broad runs.

Build frontend assets once per relevant CI run and share a same-commit artifact
with Rust jobs, using the existing skip-build mechanism. Keep at least one
genuine release embedding smoke. Cache dependencies using lockfile/toolchain/
platform keys; do not reuse stale test results or an arbitrary old binary as
proof of the current source. Benchmark artifact transfer against rebuilding
before adding more job dependencies. The current runner only deduplicates
build/install work within one invocation.

**Third: improve remote execution economics.** Add PR concurrency cancellation
and explicit job timeouts; neither appears in current CI. Preserve cross-platform
codec/process coverage for relevant changes, main, and releases. Do not run the
entire platform matrix for unrelated prose or helper edits. Insiders and large
corpus/stress campaigns fit scheduled/manual jobs. Keep tiny Python minimum/
latest checks when that domain changes; their test execution is already cheap.

**Fourth: connect qualification to release.** The release workflow builds and
smokes artifacts but does not itself require full source-suite success for the
tagged SHA. Require successful qualification of that exact SHA, via a reusable
workflow or verified CI result, before publication. Test actual bundled wheels
and VSIX launch behavior, not just development source compilation. Preserve
platform archive smoke tests because they test delivered artifacts.

**Fifth: make cost and failures observable.** Add per-step timing and structured
test reports, preserve failure logs and browser traces, and report discovered/
executed/skipped test counts. Distinguish dependency installation, compilation,
fixture generation, and test execution. Use narrowly bounded retries only to
diagnose flakes, recording the original failure; do not silently turn retries
into success. Run targeted mutation experiments on windowing, cache keys,
annotation revisions, and the selector to establish that tests detect the bugs
they claim to cover. Avoid a blanket coverage-percentage target.

## Suggested implementation sequence

1. Add compatibility-unit/marketing CI selection and a shared, explained domain
   planner with conservative fallbacks; preserve broad qualification.
2. Add concurrency/timeouts and measure setup costs; isolate fixture drift and
   formatting, then share frontend artifacts if measurements support it.
3. Replace weak JPEG and archive assertions; remove incidental label/order
   checks, consolidate overlapping HTTP cases without losing distinct behavior.
4. Extract extension Node tests and add a real viewer/extension journey plus
   a small browser acceptance suite.
5. Add scheduled external/performance qualification and an exact-SHA release
   gate with retained reports.

Success means irrelevant domains are skipped with a reviewable explanation,
meaningful boundary regressions still fail, and missing/skipped coverage is
visible. Decreasing the number of tests is not itself a success criterion.
