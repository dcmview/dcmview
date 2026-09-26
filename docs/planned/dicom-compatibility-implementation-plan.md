# DICOM Compatibility Implementation Plan

## Objective

Establish a policy-driven, reproducible compatibility program using
`dicom-test-suite` as the primary deterministic corpus, then bring `dcmview`
into conformance with the agreed support boundaries.

The finished system must distinguish pixel correctness, presentation
correctness, navigation, semantic context, robustness, and explicitly
unsupported behavior. It must never equate "file opened" with full DICOM
support.

The implementation sequence is:

```text
Pinned corpus -> support policy -> harness v2 -> baseline campaign
  -> viewer remediation and features -> full compatibility campaign
```

CI scheduling and release-process integration are outside the scope of this
plan.

## Governing product scope

`dcmview` is a developer and research inspection tool, not a clinical
diagnostic workstation.

### Support classifications

Every case must be evaluated independently across:

1. Discovery and safe handling.
2. Metadata fidelity.
3. Raw pixel decoding.
4. Display and presentation fidelity.
5. Frame, series, geometry, and reference navigation.
6. Semantic-context capabilities.
7. Robustness, recovery, timing, and resource behavior.

User-facing outcomes are:

| Classification | Promise |
|---|---|
| Pixel-faithful interactive | Correct pixels, declared presentation behavior, and frame navigation |
| Pixel preview | Useful inspection of the decoded pixel array without object-specific interpretation |
| Metadata/reference navigation | Tags, identities, and relationships are available without a renderer |
| Controlled unsupported | Stable, explicit rejection or skip; no crash, hang, or misleading output |
| Out of scope | Deliberately excluded from the local-file viewer |

Semantic context is an optional capability layered on pixel preview; it does
not promote a derived object to pixel-faithful clinical support.

### Agreed family policy

- Classic CR, CT, MR, DX, MG, NM, PET, US, XA/XRF, SC, and ordinary
  multiframe objects target pixel-faithful generic image inspection.
- Enhanced CT/MR/PET target pixel-faithful frame display and basic geometry.
  Advanced enhanced dimensions and per-frame semantics are separate
  capabilities.
- VL endoscopic, microscopic, and photographic objects target generic color
  viewing.
- SEG, Parametric Map, and RT Dose default to pixel preview and offer an
  opt-in semantic-context mode.
- WSI supports positioned individual-tile preview, not Total Pixel Matrix
  reconstruction.
- RWVM, PR, registration, SR/KOS, waveforms, RT
  planning/structure/radiation objects, PDF, and STL default to
  metadata/reference navigation unless a dedicated capability is implemented.
- DICOMDIR is recognized and skipped with a stable unsupported reason.
  Ordinary instances discovered beside it remain loadable.
- DIMSE, DICOMweb, TLS, digital signatures, secure media, and clinical
  interoperability validation remain out of scope.
- Unsupported transfer syntaxes and video must produce controlled unsupported
  behavior.

## Repository and source rules

- Implement changes in the `dcm-view` repository.
- Initially pin `dicom-test-suite` commit
  `28d45eb7ced872c5ce92f920e694acccd49e6478`.
- Treat the sibling `dicom-test-suite` repository as a read-only upstream
  dependency. Report upstream defects separately rather than silently
  modifying it.
- Generate corpora into temporary or explicitly selected artifact locations;
  do not commit generated DICOM trees.
- Use the fresh manifest and executable registry as authority, not historical
  generated directories or stale summary files.
- Update `docs/architecture.md` whenever the harness, contracts, or lifecycle
  structure changes.
- Preserve unrelated worktree changes.

Every completed logical unit must receive a granular commit following the
repository's `type(scope): subject` policy. Stage selectively and run
`git log --oneline -3` after every commit. Do not batch independent harness,
backend, frontend, and documentation changes.

## Phase 1: Freeze corpus and compatibility contracts

### Deliverables

- Record the pinned suite commit, required feature set, tool inventory,
  manifest digest, and generation command.
- Generate and validate these profiles separately:
  - `all`: currently 152 logical cases and 169 physical files.
  - `legacy`: 1 file.
  - `negative`: 15 files.
  - `stress`: currently 139 physical files.
  - `fuzz`: qualification evidence only; no retained payload corpus.
- Reject a corpus when:
  - Manifest or schema validation fails.
  - A declared file hash differs.
  - Built-in suite validation reports a failure.
  - The suite commit or manifest digest differs from the recorded campaign
    input without an explicit update.
- Never infer case membership by globbing files; use manifest paths.

### Acceptance criteria

- A clean environment can reproduce all profile manifests.
- Physical-file and logical-case counts are derived from manifests, not source
  constants.
- Profile boundaries remain distinct.
- A suite upgrade requires an explicit reviewed pin update.

## Phase 2: Introduce a machine-readable support policy

Add a versioned support policy under `scripts/compatibility`, accompanied by
schema validation and human-readable documentation.

### Policy requirements

Rules may match:

- Object family and SOP Class.
- Transfer syntax.
- Pixel representation and photometric layout.
- Expected suite capability or semantic.
- Profile.
- Known narrow-domain constraints.

Each rule must declare:

- Intended classification.
- Required assertions.
- Optional semantic-context assertions.
- Expected unsupported status and error behavior.
- Rationale and explicit exclusions.
- Precedence when multiple rules match.

The policy must distinguish:

- Required and verified.
- Preview-only.
- Metadata-only.
- Controlled unsupported.
- Out of scope.
- Temporarily unverified.

### Transfer-syntax baseline

Target qualification for:

- Implicit and Explicit VR Little Endian.
- Explicit VR Big Endian in the legacy profile.
- Deflated Explicit VR Little Endian.
- RLE Lossless.
- JPEG Baseline.
- JPEG Lossless `.57` and `.70`.
- JPEG 2000 Lossless.
- JPEG-LS Lossless.
- JPEG XL Lossless.
- Deflated Image Frame within its declared one-bit monochrome domain.

Keep controlled unsupported or unverified until independently exercised:

- JPEG Extended 12-bit `.51`.
- JPEG 2000 lossy `.91`.
- JPEG-LS near-lossless `.81`.
- JPEG XL recompression and lossy `.111` and `.112`.
- HTJ2K `.201` and `.203`.
- Video transfer syntaxes.

### Acceptance criteria

- Every manifest case resolves to exactly one policy outcome.
- No `known_gap` catch-all remains.
- Expected unsupported behavior passes only when the actual error is controlled
  and policy-compliant.
- No capability is claimed simply because its name appears in a whitelist.

## Phase 3: Refactor corpus scope and worklist generation

Replace the hardcoded older inventory in `scripts/compatibility/scope.py`.

### Requirements

- Accept explicit corpus roots and profiles.
- Preserve suite-relative manifest identity while allowing generated corpora
  outside the suite checkout.
- Verify every referenced file hash and contract hash.
- Deduplicate only exact manifest identities.
- Produce an immutable campaign worklist.
- Handle multi-file logical cases correctly.
- Maintain separate worklist models for:
  - Valid files.
  - Legacy files.
  - Negative mutations.
  - Stress scenarios.
  - Payload-free fuzz qualification.
- Do not require SOP Instance UID for negative inputs.
- Do not treat cases from unselected profiles as unexpectedly unavailable.

### Tests

Cover:

- Current fresh manifests.
- Multi-file cases.
- Relocated corpus roots.
- Hash mismatches.
- Duplicate identities.
- Missing optional identifiers.
- Profile isolation.
- Suite pin or digest mismatch.
- Historical worklist migration or an explicit incompatibility error.

## Phase 4: Replace capability whitelisting with assertion-backed evidence

Create an assertion registry in which each claimed capability maps to concrete
checks.

### Valid-file assertions

- File discovered and mapped to the expected path and SOP identity.
- Expected object kind and support classification.
- Exact relevant metadata values, including character sets, private creators,
  sequences, time zones, and long or multivalued fields.
- Raw response headers match manifest image organization.
- Every frame of every claimed lossless path matches its canonical decoded
  hash.
- Lossy outputs satisfy the suite's numeric maximum-error and RMSE thresholds.
- Display output has exact dimensions and normalized pixels where an oracle
  exists.
- First, last, middle, and deterministic random frame access work.
- Cache `MISS` and `HIT` behavior is correct.
- References include SOP, frame, and segment-number relationships.
- Series ordering, geometry warnings, and concatenations match expected
  evidence.
- A failed decode does not prevent a subsequent healthy request.

### Presentation assertions

Add exact checks for:

- Rescale and Modality LUT.
- VOI LUT and explicit window parameters.
- DICOM LINEAR boundary behavior.
- MONOCHROME1 inversion.
- Pixel Padding Value and Range treatment.
- RGB, YBR, palette, and planar behavior.
- Overlay and shutter behavior.
- ICC preservation or declared color-management behavior.
- Server-rendered PNG versus frontend raw-windowing parity.

Do not treat screenshots alone as pixel-correctness evidence. Compare
normalized pixel buffers; capture screenshots as diagnostic artifacts.

### Claim discipline

Remove or rename unsupported claims such as "reconstruct WSI pyramid" when the
implementation only groups logical stacks. Each report statement must
correspond to an assertion that actually ran.

## Phase 5: Redesign reports and classifications

Emit:

1. A report conforming to the suite's `viewer-report.schema.json` contract.
2. A richer `dcmview` evidence report.

The detailed report must include:

- Suite and viewer commits.
- Binary hash and build features.
- Manifest and policy digests.
- Per-assertion results.
- Policy outcome versus actual outcome.
- Raw and display hashes and lossy metrics.
- Error codes and response bodies.
- Timing and resource measurements.
- Artifacts such as PNGs, logs, and failure screenshots.
- Summaries by object family, transfer syntax, support classification, and
  compatibility dimension.

Use explicit statuses such as:

- `passed`
- `failed`
- `expected_unsupported`
- `unexpected_unsupported`
- `timeout`
- `crash`
- `unavailable`
- `not_applicable`

A controlled unsupported result is not a failure when required by policy. A
successful but incorrect rendering is always a failure.

## Phase 6: Run and preserve a current baseline campaign

Build the current `dcmview` source, verify its version and binary hash, and run
the valid `all` and `legacy` profiles through the new harness before
implementing compatibility fixes.

### Baseline objectives

- Separate viewer defects from harness defects.
- Produce a complete policy-classified inventory.
- Confirm suspected high-priority issues:
  - RGB JPEG silently rendered as grayscale.
  - Pixel padding ignored.
  - Windowing boundary differences.
  - Browser raw and server presentation divergence.
  - `support_state` disagreement with endpoint behavior.
  - Enhanced functional-group overclaims.
  - WSI reconstruction overclaims.
  - Documentation and runtime contract inconsistencies.

Preserve full machine-readable output in the selected campaign artifact
location. Commit a dated summary only if it is intended to become durable
project evidence.

Do not use the older 165-file campaign as acceptance evidence; it remains
triage history only.

## Phase 7: Correct silent and contract-breaking behavior

Fix correctness issues before expanding semantic features.

### Priority order

1. Eliminate successful incorrect color rendering. RGB JPEG must render
   correctly or return controlled unsupported.
2. Align `support_state`, documented support, and actual endpoint routing.
3. Implement correct Pixel Padding Value and Range handling.
4. Correct declared DICOM windowing behavior.
5. Make frontend raw rendering preserve or explicitly disable incompatible
   LUT, overlay, and shutter semantics.
6. Resolve raw-response inconsistencies such as one-bit packed versus expanded
   representation.
7. Qualify encapsulated frame access, BOT/EOT behavior, odd padding, and random
   access.
8. Reconcile documentation with the supported JPEG, JPEG-LS, JPEG XL, JPEG
   2000, RLE, and Deflated Image Frame layouts.

Each correction must add a focused regression test and receive its own coherent
commit.

## Phase 8: Implement semantic-context previews

The default for SEG, Parametric Map, and RT Dose remains **Pixel Preview**. Add
an explicit **Semantic Context** toggle.

The UI must always show which mode is active and which transformations or
mappings have been applied.

### SEG

Pixel Preview:

- Display the stored or decoded segment frame without implying source
  alignment.

Semantic Context:

- Show segment number, label, description, coded property, algorithm type and
  name, and recommended color.
- Resolve referenced source instances, frames, and segment relationships.
- Overlay on the source image only when geometry and reference mapping are
  validated.
- If a source is missing or mapping is ambiguous, show context metadata and a
  reason the overlay is unavailable. Never guess.

Tests:

- Binary, fractional, label-map, multiframe, and WSI-referencing SEG.
- Segment-number reference closure.
- Missing and ambiguous source objects.
- Valid and invalid overlay eligibility.
- Toggle preserves exact underlying raw pixels.

### Parametric Map and RWVM

Pixel Preview:

- Show stored integer or floating-point values and frames.

Semantic Context:

- Resolve embedded or referenced Real World Value Mapping.
- Display mapped values, units, quantity codes, derivation, and source
  references.
- Identify whether the shown value is stored or mapped.
- Do not infer a mapping when none is present.

Tests:

- Float32 and float64 maps.
- RWVM ranges, units, quantity codes, and references.
- Missing or incompatible RWVM.
- Stored-versus-mapped value inspection.

### RT Dose

Pixel Preview:

- Display the stored dose pixel array.

Semantic Context:

- Apply Dose Grid Scaling.
- Display Dose Units, Dose Type, Dose Summation Type, grid geometry, and
  references.
- Permit source-image overlay only when Frame of Reference and geometry
  compatibility are validated.
- Do not imply prescription correctness or clinical acceptability.

Tests:

- Exact stored and scaled values.
- Grid orientation and offsets.
- Units, type, and summation metadata.
- Compatible and incompatible source geometry.
- Missing references and malformed scaling.

## Phase 9: Implement positioned WSI tile preview

Do not implement a whole-slide rendering or tile-stitching engine.

### Required behavior

For the selected frame, show:

- The decoded tile.
- Its rectangle within the Total Pixel Matrix.
- Pyramid level.
- Tile row and column position.
- Optical path.
- Focal plane.
- Sparse or full tiling status.
- Companion-image and source relationships where available.

Add a lightweight schematic minimap showing the current tile rectangle within
the matrix. It must be metadata-driven and must not decode or compose
neighboring tiles.

### Explicit exclusions

- No full-slide mosaic.
- No adjacent-tile stitching.
- No pan-across-slide renderer.
- No resampling across pyramid levels.
- No assumption that frame order equals spatial order.

### Tests

- Tiled-full and tiled-sparse WSI.
- Multiple optical paths.
- Multiple focal planes where represented.
- Pyramid companion roles.
- WSI-referencing SEG.
- Incorrect or missing slide-position metadata.
- Large frame counts without unbounded catalog or UI growth.

## Phase 10: Negative, stress, and fuzz runners

### Negative runner

Run each malformed case in isolation with bounded time and output.

Accept only outcomes permitted by the suite case and product policy:

- Safe discovery skip with a stable reason.
- Controlled parse or decode error.
- A deliberately tolerated partial inspection state.
- Server remains alive where a malformed object reached a running server.

Always fail on:

- Crash.
- Hang.
- Resource runaway.
- Silent incorrect success.
- Corruption of subsequent healthy requests.

Missing required DICOM attributes do not automatically need rejection;
`dcmview` is an inspector, not a validator. The expected behavior must be
policy-defined per case.

### Stress runner

Measure:

- Startup and discovery time.
- First-frame and random-frame latency.
- Peak RSS where supported.
- Cache use and eviction.
- Concurrent same-frame and different-frame requests.
- Recovery after errors.
- Shutdown and cancellation.

Initially record baselines rather than inventing arbitrary hard limits.
Establish reviewed thresholds after the first stable runs.

### Fuzzing

The suite's fuzz profile has no reusable payload corpus. Add a
`dcmview`-specific target adapter or promote important minimized failures into
deterministic negative fixtures. Enforce bounded input size, operations,
duration, and retained artifacts.

## Phase 11: Real-browser compatibility tests

Use the real Svelte frontend with a real `dcmview` backend.

Keep the automated browser set representative rather than duplicating all API
cases.

Cover:

- Pixel-preview versus semantic-context toggles.
- SEG overlays and unavailable-overlay explanations.
- Stored versus mapped Parametric Map values.
- Stored versus scaled RT Dose values.
- WSI tile minimap and spatial labels.
- Server-PNG versus browser-raw parity.
- Frame and cine navigation.
- Window/level, zoom, pan, orientation, and file switching.
- Metadata-only and unsupported states.
- Errors followed by successful continued interaction.

Prefer existing frontend test infrastructure for pure logic. Add browser
automation only for behavior that requires the actual DOM, canvas, worker, or
network boundary. Capture screenshots and logs on failure.

## Phase 12: DICOMDIR and other explicit boundaries

Implement deterministic DICOMDIR behavior:

- Recognize the Media Storage Directory SOP Class.
- Skip it with a stable `unsupported_media_directory`-style reason.
- Continue recursive discovery of ordinary DICOM files.
- Do not parse the file-set hierarchy.
- Do not advertise DICOM media support.

Add equivalent stable policy behavior for out-of-scope protocols, security
objects, video, and unsupported transfer syntaxes where they can appear in
local input.

## Final definition of done

The implementation is complete when:

- The suite input is pinned, reproducible, validated, and manifest-driven.
- Valid, legacy, negative, and stress profiles have separate workflows.
- Every support claim is backed by a concrete assertion.
- Every suite case resolves through the versioned support policy.
- All claimed lossless raw paths match exact per-frame hashes.
- Lossy cases use numeric suite thresholds.
- Metadata, references, geometry, presentation, and UI behavior are tested at
  the level claimed.
- SEG, Parametric Map, and RT Dose provide default pixel preview plus opt-in
  semantic context.
- WSI provides metadata-correct positioned tile preview without slide
  reconstruction.
- DICOMDIR and other excluded features have stable controlled behavior.
- Expected unsupported cases pass policy; unexpected unsupported cases fail.
- No tested input can crash, hang, or poison the viewer for subsequent
  requests.
- Current documentation matches runtime behavior.
- Full machine-readable reports are preserved and summaries are understandable
  without inspecting raw logs.
- All changes are recorded in descriptive, granular commits and the final
  worktree is clean.

## Orchestration guidance

Independent workstreams may proceed in parallel only after the support policy
and report contracts are stable. Suitable parallel streams are:

- Corpus, worklist, and reporting infrastructure.
- Pixel and presentation oracles.
- Negative and stress runners.
- SEG, Parametric Map, and RT Dose backend semantics.
- Semantic-context frontend.
- WSI positioning and UI.
- Browser automation.

Assign a single writer at a time to shared files such as `run.py`, `App.svelte`,
API contracts, generated TypeScript contracts, and `docs/architecture.md`.
Integrate and verify each logical unit before beginning dependent work.
