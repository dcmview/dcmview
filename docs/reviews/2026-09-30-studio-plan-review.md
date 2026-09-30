# dcmview / dcmview-studio implementation-plan review

Reviewed 2026-09-30. Scope: technical correctness and implementability of the
supplied `dcmview-roadmap-review.zip`, not the value of its confirmed features.
References below are to the bundle's document names and section numbers.
Later confirmed decisions take precedence over earlier drafts. In particular,
the versioned file list, stable socket path, bearer-auth thumbnail fetching,
Studio name, all-vector default and earlier sidecar delivery are treated as
settled, not reopened as findings.

The public source was checked at
[`b78536f9545c86d465585730a8a054f79c0f06f8`](https://github.com/dcmview/dcmview/tree/b78536f9545c86d465585730a8a054f79c0f06f8).
No implementation changes were made. The private documentation, Studio and
test-corpus repositories were not inspected. Blocker means a contract or safety
issue that must be resolved before implementing the affected foundation or
enabling the affected feature; it does not mean all independent work must stop.

1. **Blocker — Collection-dependent keys lack a complete transition protocol.**
   **Documents:** `annotation-model.md` §§1.3–1.8; `integration-contract.md` §12.
   **Problem:** A file gets `sop:` alone and `b3:` in a colliding collection,
   contradicting the requirement that its key be computable from the file alone.
   A newly discovered same-UID, same-size file provisionally shares annotations
   before byte verification, while the proposed store rekey does not specify how
   queued edits, undo nodes and other references retain their original target.
   **Suggested fix:** Separate immutable session occurrence IDs from resolved
   content identity, retain the occurrence on pending edits, and merge only
   after verification; finalize inventory identities before emitting them, and
   make manifest-supplied identities authoritative in spokes. Define an atomic
   identity-resolution event that updates records, queues, history and selection.

2. **Blocker — The operation protocol does not cover all its writers.**
   **Documents:** `annotation-model.md` §§7.2–7.4; `annotation-tools-ux.md`
   §§7.2–7.3, 15; `integration-contract.md` §§7.2–7.4.
   **Problem:** `SetLabel` has neither a record ID nor a revision precondition,
   layer operations are unspecified, and per-file FIFOs cannot order a study
   label edited from two files or a gallery batch touching several files.
   Atomic `Batch` remains a downstream request even though history jumps and
   exclusive-mask edits depend on it.
   **Suggested fix:** Specify versioned targets, including absent/deleted
   records, for every operation; use one ordered client transaction queue or
   explicit multi-target dependencies. Commit validation, state changes, audit
   records and deduplication results together, return all affected revisions,
   and define delete/restore and conflicting-before-state behavior.

3. **Blocker — “Done” is not tied to a committed annotation revision.**
   **Documents:** `hub-product.md` §§4.1, 5.7–5.8, 9.1; `annotation-tools-ux.md`
   §§9–10; `integration-contract.md` §7.3.
   **Problem:** Progress requests can overtake pending annotation writes, so a
   reader can mark an empty durable snapshot done while their last shape is
   still queued; subsequent edits to a done item also have no specified
   automatic state transition. Because done-with-no-shapes means negative,
   these races change agreement results, and closing can permanently reject
   edits still displayed as retryable.
   **Suggested fix:** Put completion behind a per-item commit barrier and
   record the acknowledged revision/sequence in an idempotent progress event;
   require edits to reopen/invalidate completion atomically. Specify pause,
   close and failed-export transitions, including recovery/export of rejected
   pending client work.

4. **Blocker — The share-safe transformation misses identifiers outside typed fields.**
   **Documents:** `hub-product.md` §§11.2–11.5, 15.4; `annotation-model.md` §6.2;
   `output-adapters.md` §§4.2, 5.4.
   **Problem:** Renaming UID, path and username fields leaves possible original
   identifiers in text labels, notes, uploaded metadata, guidelines, template
   literals, reports, unknown extensions and audit before/after payloads.
   Preserving unknown fields and the full audit log is incompatible with an
   automatic claim that identifiers were pseudonymized everywhere.
   **Suggested fix:** Define a separate allowlisted share-safe schema and apply
   the mapping consistently to keys and references throughout the artifact;
   omit unclassified text, extensions, templates and log content by default,
   with an explicit reviewed inclusion path. Retain the pseudonymization
   disclaimer and document the deliberate linkability of retained digests.

5. **Major — Matching can ignore contradictory identity evidence.**
   **Documents:** `annotation-model.md` §§1.3, 1.6–1.7; `hub-product.md` §§12, 17.
   **Problem:** The first-success resolver can accept a UID or path match even
   when a supplied digest disagrees, while dimensions alone do not detect
   replacement pixels. With hashes optional for normal DICOM, “verify keys on
   decode” cannot detect a changed image retaining its SOP UID and dimensions.
   **Suggested fix:** Check all available strong evidence before accepting a
   match, reject ambiguities and digest conflicts, and distinguish identity
   matching from content verification. Define source immutability/fingerprint
   checks, report unverified files honestly, and require explicit reconciliation
   before changed images count in agreement.

6. **Major — Hierarchy keys are not sufficiently namespaced.**
   **Documents:** `annotation-model.md` §§4.3, 13; `hub-product.md` §§5.1–5.2,
   7.2, 11.2.
   **Problem:** Folder targets contain only a relative path although Studio
   supports several roots, so `root0/case1` and `root1/case1` can collapse;
   PatientID-only targets similarly merge different issuing authorities or
   missing identifiers. This affects assignment and labels, not just captions.
   **Suggested fix:** Include a stable root ID in folder targets, and an issuer
   or explicit dataset namespace in patient targets; define non-collapsing
   fallback identities for missing hierarchy IDs and carry them through every
   manifest and API. DICOM provides an
   [Issuer of Patient ID](https://dicom.nema.org/medical/dicom/final/cp800_ft.pdf)
   for this distinction.

7. **Major — Count-based catalog deltas cannot carry identity updates.**
   **Documents:** `gallery-views.md` §7.4 and decision 9;
   `annotation-model.md` §§1.7–1.8.
   **Problem:** `/api/files?since=<count>` assumes entries only append, but
   pending keys become resolved, aliases are assigned, and collisions rekey
   existing entries without increasing the count. Clients using those deltas
   can retain obsolete keys and duplicate selections indefinitely.
   **Suggested fix:** Use a catalog revision cursor with insert/update events
   and explicit reset semantics, or deliver key/alias changes through a
   separate versioned channel that every catalog consumer must apply.

8. **Major — Dynamic worklists cannot update a spoke's fixed file scope.**
   **Documents:** `hub-product.md` §§4.2, 5.6, 5.8, 15.2;
   `integration-contract.md` §5.3.
   **Problem:** New and reassigned items appear without restarting a spoke,
   but its file list is read only at startup; the recipient cannot render new
   files, while the previous reader can still request files removed from their
   worklist. Updating the worklist does not update the pixel-serving registry.
   **Suggested fix:** Version assignment scope and either support authenticated
   registry updates or perform a controlled refresh/restart before exposing the
   new worklist. Recheck current authorization on pixel, metadata and annotation
   access, including requests from stale tabs.

9. **Major — Restart recovery still has incompatible definitions.**
   **Documents:** `integration-contract.md` §7.3; `annotation-tools-ux.md`
   §§7.3, 15; `hub-product.md` §4.2.
   **Problem:** Integration and Studio retain automatic reload language, while
   the tools plan explicitly loses history and unsent edits on page reload;
   these cannot both describe recovery. Dirty-only replay also cannot restore
   acknowledged annotations lost when a standalone memory backend restarts.
   **Suggested fix:** Specify an in-page reconnect handshake that preserves the
   queue, verifies campaign/backend identity, resolves op outcomes, reloads the
   catalog and snapshots, and then reconciles pending changes. Define a separate
   memory-backend recovery/reset policy instead of claiming the durable-backend
   behavior applies to it.

10. **Major — The sidecar does not yet establish its durability guarantee.**
    **Documents:** `integration-contract.md` §6.4; `annotation-model.md` §7.3.
    **Problem:** Temp-file fsync followed by rename omits syncing the parent
    directory, and the sidecar contract has no exclusive writer lock or durable
    deduplication state for retries after restart. Consequently it can lose an
    acknowledged replacement or apply a previously committed operation again.
    **Suggested fix:** Lock the destination, persist revision and op-outcome
    metadata with the snapshot, publish atomically and sync the directory before
    acknowledging; specify equivalent platform guarantees and a transaction
    layout for external mask tiles. See the
    [fsync directory requirement](https://www.man7.org/linux/man-pages/man2/fsync.2.html).

11. **Major — The proposed label uniqueness constraint permits duplicates.**
    **Document:** `hub-product.md` §7.2.
    **Problem:** `UNIQUE(target_kind,target_id,frame_index,field,layer_id,author)`
    allows duplicate non-frame labels because their `frame_index` is NULL;
    SQLite treats NULLs as distinct in UNIQUE constraints. I reproduced two
    identical study-label targets being inserted successfully.
    **Suggested fix:** Use separate partial unique indexes for frame and
    non-frame labels, or a checked non-null canonical target key; declare the
    remaining identity columns NOT NULL. This follows
    [SQLite's documented NULL semantics](https://sqlite.org/nulls.html).

12. **Major — Studio child launches can be intercepted by VS Code.**
    **Documents:** `integration-contract.md` §§5.4, 8.4; `hub-product.md` §5.1.
    **Problem:** The proposed spawn environment omits `DCMVIEW_VSCODE_BYPASS=1`,
    but the existing application routes launches through VS Code before local
    startup, including when the current directory matches a registered workspace.
    Starting Studio from a VS Code terminal can therefore prevent the expected
    child socket/startup protocol.
    **Suggested fix:** Set the existing bypass on every supervised child and
    inventory subprocess, and handle `inventory` before viewer routing.
    Verified in
    [application dispatch](https://github.com/dcmview/dcmview/blob/b78536f/src/application.rs)
    and [bridge discovery](https://github.com/dcmview/dcmview/blob/b78536f/src/bridge/registry.rs).

13. **Major — Metadata blinding lacks an end-to-end response policy.**
    **Documents:** `hub-product.md` §§6.2, 15.2; `output-adapters.md` §§4, 6, 9.
    **Problem:** Redacting patient/path fields and denying tags does not cover
    native/EMBED exports, template previews, reference responses, series text,
    discovery errors or annotation FileRefs, all of which can expose the hidden
    values. Display configuration alone cannot establish the claimed blinding.
    **Suggested fix:** Define an endpoint-and-field allowlist for blinded spokes,
    including errors, downloads and export previews, and use opaque grouping
    identifiers where the viewer still needs hierarchy relationships. Reject
    incompatible export options server-side and test the complete route table.

14. **Major — Host/Origin checks leave a clickjacking route.**
    **Documents:** `integration-contract.md` §§2, 5.1, 9; `hub-product.md` §13.
    **Problem:** A malicious page can frame the tokenless Studio front door at
    `localhost` and trick the user into clicking its real controls; requests
    originating inside that frame have valid Host and Origin values and inherit
    the SSH user's identity. No framing restriction is specified.
    **Suggested fix:** Send an explicit CSP `frame-ancestors` policy on Studio
    pages and proxied viewers, allowing only the embeddings actually required;
    keep standalone VS Code embedding as a separately tested policy. See
    [OWASP's clickjacking guidance](https://cheatsheetseries.owasp.org/cheatsheets/Clickjacking_Defense_Cheat_Sheet.html).

15. **Major — Artifact integrity checks are not safe import rules.**
    **Documents:** `hub-product.md` §§11–12; `output-adapters.md` §§5.2, 9.4.
    **Problem:** A self-supplied manifest can validate hashes for malicious ZIP
    members, symlinks or paths just as easily as benign ones; extraction limits,
    duplicate-member rules and safe output filenames are unspecified.
    Template-generated names and bundled source paths present the same traversal
    and overwrite boundary on export.
    **Suggested fix:** Treat artifacts and generated names as untrusted: reject
    absolute/parent-traversing paths and links, contain all writes in a fresh
    directory, reject duplicate normalized names, and cap member counts and
    expanded bytes. Validate the complete artifact before importing any state or
    resolving external file references.

16. **Major — Template fuel is not a complete resource sandbox.**
    **Documents:** `output-adapters.md` §3.2; `hub-product.md` §§7, 11.
    **Problem:** The plan calls fuel-limited templates safe, but fuel bounds VM
    instructions rather than all allocations or work inside helpers; a single
    large expansion/materialization can still exhaust the shared hub process.
    Source size, output bytes, helper work and job concurrency have no limits.
    **Suggested fix:** Specify independent input/output/memory/time limits,
    bounded helpers, restricted template loading and concurrent-job limits;
    isolate execution where hard memory enforcement is required. MiniJinja
    documents fuel as an
    [instruction budget](https://docs.rs/minijinja/latest/minijinja/struct.Environment.html#method.set_fuel).

17. **Major — Greedy assignment can reject a feasible campaign.**
    **Document:** `hub-product.md` §§5.3–5.5.
    **Problem:** With reader caps `(A=3,B=2,C=1)` and three items requiring two
    readers, legal greedy tie choices `BC`, then `AB`, leave only A available
    for the last item, although `AB,AB,AC` is feasible. Weighted balance and
    optimal pair balance likewise are not guaranteed by the described heuristic.
    **Suggested fix:** Separate feasibility from balancing, using a capacity
    matching/flow step or deterministic repair/backtracking; state measured
    objectives rather than unsupported guarantees. Freeze reader/stratum order,
    weight arithmetic, tier rounding, seed encoding and RNG tie consumption
    alongside golden assignments.

18. **Major — Shape matching is not restricted to the same image.**
    **Document:** `hub-product.md` §9.3.
    **Problem:** Matching is specified per unit/pair/class, but a study or
    patient unit contains several unrelated pixel coordinate systems. Identical
    boxes on different mammographic views could therefore be counted as a match.
    **Suggested fix:** Partition matches by canonical file identity and compatible
    frame space first, then aggregate per unit; require an explicit registration
    before comparing geometry across files. Apply the restriction to points,
    lines and masks as well as area shapes.

19. **Major — Thresholding after Hungarian assignment undercounts matches.**
    **Document:** `hub-product.md` §9.3.
    **Problem:** Maximizing summed IoU and then discarding subthreshold edges
    does not maximize valid detections: the realizable matrix
    `[[5/14, 1/2], [1/2, 3/4]]` selects the diagonal and retains one match at
    0.5, while the off-diagonal has two valid matches. The proposed F1 changes
    from 1 to 0.5 solely because of this ordering.
    **Suggested fix:** Gate invalid edges first, maximize valid-match cardinality,
    and then optimize IoU with deterministic tie-breaking and unmatched dummy
    nodes. The distinction follows the
    [linear assignment objective](https://docs.scipy.org/doc/scipy/reference/generated/scipy.optimize.linear_sum_assignment.html).

20. **Major — Agreement observations and missing-data rules are incomplete.**
    **Documents:** `hub-product.md` §§9.1–9.4, 11.2; `annotation-model.md` §4.3.
    **Problem:** A unit can contain multiple file/frame labels and multiple
    objects with attributes, so the proposed “unit × reader × field” table
    does not identify a unique observation; missing required labels are also
    possible on done items. Standard balanced ICC(2,1) cannot simply be applied
    to arbitrary incomplete overlap, and all-negative F1 is `0/0`.
    **Suggested fix:** Define observation keys by label target or matched object,
    separate missing from negative, and specify eligible observations and
    undefined-result handling per statistic; use an explicit complete-panel
    subset or a documented incomplete-design estimator for ICC. Bootstrap at
    the independence level, potentially patient rather than assignment unit;
    see the [ICC design guidance](https://pmc.ncbi.nlm.nih.gov/articles/PMC4913118/).

21. **Major — The hub lacks geometry required by its measurements and adapters.**
    **Documents:** `hub-product.md` §§5.1, 7.2, 9.3; `annotation-model.md` §8.1;
    `output-adapters.md` §§4.1, 7.4–7.5.
    **Problem:** Inventory returns FileRefs without pixel spacing or patient
    geometry, yet the hub must calculate millimetre distances and supply source
    headers and geometry to SEG/NIfTI adapters while linking no DICOM reader.
    A file-wide spacing field would also miss per-frame spacing already
    supported by dcmview.
    **Suggested fix:** Add a versioned metadata/geometry provider through
    inventory or a dcmview service, including per-frame spacing and its source,
    or run DICOM-dependent exports inside a supervised dcmview process. Decide
    the provider before freezing ExportContext and agreement tables.

22. **Major — The proposed raw raster values disagree with the locked decoders.**
    **Document:** `image-formats.md` §§2.1, 5.2.
    **Problem:** `image 0.25.6` expands a one-bit PNG's stored `[0,1]` to
    `[0,255]`, while `tiff 0.9.1` already inverts unsigned WhiteIsZero samples;
    passing that TIFF output through MONOCHROME1 inverts it a second time.
    Both behaviors were reproduced using the repository's compiled dependencies.
    **Suggested fix:** Specify decoder-output normalization separately from
    stored sample semantics: deliberately preserve/recover source values, or
    declare a normalized raw representation with an explicit mapping. Add exact
    raw/readout/display cases for low-bit PNG and signed, unsigned and float
    WhiteIsZero TIFF, rather than relying only on visual output.

23. **Major — `sBIT` is not a low-valued sample range.**
    **Document:** `image-formats.md` §§5.2–5.3, 9.
    **Problem:** PNG `sBIT` records the original precision in the high bits of
    samples scaled to the container depth; it does not mean a 16-bit image with
    `sBIT=12` contains values only in `0..4095`. Using that as its declared
    window range can clip most of the image.
    **Suggested fix:** Retain the stored-depth range or explicitly recover the
    original significant-bit samples and describe that mapping; freeze an
    `sBIT=12` fixture with known 16-bit values. The
    [PNG specification](https://www.w3.org/TR/png-3/#11sBIT) defines this scaling.

24. **Major — CMYK profiles cannot be copied onto converted RGB pixels.**
    **Document:** `image-formats.md` §§5.2, 6.2.
    **Problem:** CMYK/YCCK JPEGs are converted to RGB, but the plan then passes
    their original ICC profile into the RGB PNG; a CMYK profile describes the
    wrong sample space. Existing `normalize_profile` checks basic header
    structure, not color-space compatibility.
    **Suggested fix:** Use a valid profile-aware conversion to a declared RGB
    space, or omit incompatible profiles with a report when using an approximate
    conversion; validate profile/sample-space agreement before encoding. PNG
    requires an [RGB profile for RGB images](https://www.w3.org/TR/png-3/#11iCCP).

25. **Major — The TIFF frame rule permits incompatible per-page presentation.**
    **Document:** `image-formats.md` §§2.4, 5.1–5.4, 6.1.
    **Problem:** Matching width, height, sample count and sample format does not
    guarantee equal bit depth, photometric interpretation, alpha association or
    orientation, yet these are represented once per file. Pages that satisfy the
    rule can consequently decode, display or map coordinates differently from
    page zero without a place to record the difference.
    **Suggested fix:** Either support the relevant metadata per frame or require
    compatible values when building the frame map and report excluded pages;
    normalize associated versus unassociated TIFF alpha explicitly.

26. **Major — Lazy hashing is neither free nor well-defined for partial decodes.**
    **Documents:** `annotation-model.md` §§1.6–1.7; `image-formats.md` §5.6;
    `hub-product.md` §17.
    **Problem:** Reading one TIFF page or one DICOM frame does not read the
    whole file, so the claim that a full file hash is already in the page cache
    and needs only a CPU pass is false for large multiframe files. A single
    file-level pixel digest also cannot be completed from whichever frame
    happened to be decoded first.
    **Suggested fix:** Budget whole-file verification as explicit background or
    setup I/O, with cancellation and a visible pending state; define a canonical
    digest encoding and distinguish per-frame digests from a completed file
    digest. Do not promise an unmeasured percentage of decode time.

27. **Major — Thumbnail pixels depend on which cache happens to be warm.**
    **Document:** `gallery-views.md` §§3.4–3.5.
    **Problem:** The display-cache fast path shrinks a fully presented PNG,
    which can contain shutters and overlays, while the raw/reduced/full decode
    paths explicitly omit them. The same thumbnail key can therefore display
    materially different content depending on prior viewer activity.
    **Suggested fix:** Make all sources obey one presentation policy, caching a
    suitable pre-presentation buffer or skipping incompatible cache hits. If
    reduced-resolution histogram variation is accepted, document and bound that
    separately from structural presentation differences.

28. **Major — A class label map is not a lossless instance export.**
    **Documents:** `annotation-model.md` §§3.2, 5; `output-adapters.md` §7.3.
    **Problem:** Two exclusive, non-overlapping mask objects of the same class
    become indistinguishable when PNG values equal class indices, despite the
    claim that exclusive layers export losslessly. Also, overlap precedence
    depends on layer order that the model deliberately does not persist.
    **Suggested fix:** Describe class-map export as lossy for instances, offer
    segment-index maps or per-instance PNGs with an ID sidecar, and include an
    explicit stable overlap-priority list in export options and reports.

29. **Major — COCO results cannot resolve their images without a mapping.**
    **Document:** `output-adapters.md` §7.2.
    **Problem:** Results arrays contain numeric `image_id` and `category_id`,
    but the proposed resolver expects `file_key` or `file_name`, neither of
    which is present in standard results. IDs cannot safely be interpreted as
    discovery indices or the current schema's category order.
    **Suggested fix:** Require the originating COCO dataset/export manifest or
    an explicit image/category mapping, including frame mappings; reject
    unknown or ambiguous IDs. The
    [COCO results specification](https://github.com/cocodataset/cocodataset.github.io/blob/master/dataset/format-results.htm)
    makes this dependency explicit through its ID references.

30. **Major — Export checks and reported output are not bound to one snapshot.**
    **Document:** `output-adapters.md` §§4.2, 9.1–9.3.
    **Problem:** Repeating the same request body at `/export/check` and
    `/export` does not repeat the same dataset if edits occur between requests;
    a lazy export over a live store can also observe mixed revisions.
    Thus the displayed loss report need not describe the downloaded artifact.
    **Suggested fix:** Produce a snapshot/revision token at check time and
    require export to use it or reject it as stale; generate the final report
    from the actual render, tied to an export ID or included in the archive.

31. **Major — Cache budgets do not bound process or browser memory.**
    **Documents:** `integration-contract.md` §5.6; `image-formats.md` §2.3;
    `annotation-tools-ux.md` §§7.5, 8.2; `hub-product.md` §17.
    **Problem:** Cache limits exclude concurrent decoder intermediates,
    encoded source buffers, the catalog, masks/history and browser images;
    even an accepted 268-Mpixel grayscale frame expands to about 1 GiB as RGBA
    display pixels. Per-spoke core-count semaphores also multiply concurrency across
    active readers, so `--cache-budget` is not the stated containment mechanism.
    **Suggested fix:** Define separate resident-cache, in-flight decode and
    browser-render limits, with byte-based admission, bounded queues and a
    host-wide spoke/concurrency policy. Keep unsupported-image decisions
    separate from whether one decoded frame fits in the LRU.

32. **Minor — The deterministic artifact promise includes variable timestamps.**
    **Documents:** `hub-product.md` §§11.1–11.3; `output-adapters.md` §4.1.
    **Problem:** Using each invocation's export time for ZIP members, manifest
    `created_at` and template metadata contradicts “same database state gives a
    byte-identical artifact.” Compression implementation and adapter-generated
    IDs can introduce further variation beyond a fixed compression level.
    **Suggested fix:** Define determinism over an immutable export specification
    containing a fixed timestamp, writer/adapter versions and generation seeds,
    or promise canonical payload equivalence instead of identical ZIP bytes.

33. **Minor — The proposed frame-weighted score is not spatiotemporal IoU.**
    **Document:** `hub-product.md` §9.3.
    **Problem:** Multiplying 2D IoU by frame-set Jaccard generally differs from
    intersection-over-union over pixel/frame pairs: areas of 2 with overlap 1,
    each on two frames with one common frame, yield `1/9` by the proposed
    product but `1/7` by actual IoU. Calling both IoU makes thresholds and
    downstream recomputation ambiguous.
    **Suggested fix:** Keep the confirmed product as an explicitly named custom
    similarity with its own thresholds, or use the true product-space union
    formula; record the choice in agreement metadata and golden cases.

34. **Minor — Polling prevents the specified idle lifecycle from working reliably.**
    **Documents:** `integration-contract.md` §5.4; `hub-product.md` §§4.2, 5.8.
    **Problem:** Worklist polling every 60 seconds is proxied activity, so a
    forgotten tab keeps a spoke alive indefinitely and races the 60-second
    “no request” window used for config restarts. Background traffic is not a
    reliable indication of human activity.
    **Suggested fix:** Track user activity separately from polling and use an
    explicit safe-to-restart handshake that checks pending writes; refresh
    configuration by version rather than hoping for a request-free interval.

35. **Minor — The thumbnail transform contract contradicts raster orientation.**
    **Documents:** `gallery-views.md` §§3.2, 9; `image-formats.md` §11.
    **Problem:** Gallery geometry is described as never rotated or flipped and
    mappable from annotations by axis scaling only, while raster thumbnails
    must honor EXIF orientation. Orientations 5–8 swap axes, so that mapping is
    insufficient for the promised future annotation overlays.
    **Suggested fix:** Define thumbnail space explicitly and return or derive
    the complete stored-to-thumbnail transform, including orientation and
    aspect ratio, with all eight EXIF cases tested.

36. **Minor — Background priority cannot guarantee zero viewer delay.**
    **Document:** `gallery-views.md` §§4.2, 9.
    **Problem:** Already running thumbnail decodes are non-preemptible; on a
    one-core host, `max(1, cores/2)` allows the sole permit to be occupied by a
    slow background decode when an interactive request arrives. Priority over
    queued jobs does not establish the promised “never delays” guarantee.
    **Suggested fix:** Specify a bounded latency target and measure it under
    load, reserve interactive capacity where possible, and constrain or disable
    costly background work on one-core hosts.

Verification evidence: the SQLite uniqueness probe inserted two study-label
rows with the same target/field/layer/author and NULL frame index. The matching
counterexample uses rectangles with x intervals `[4,11]`, `[8,14]` for reader A
and `[6,18]`, `[6,14]` for reader B, all with y interval `[0,1]`. The raster
probe decoded hand-built two-pixel files through the already compiled locked
`image 0.25.6` and `tiff 0.9.1` libraries and returned `[0,255]` for one-bit
PNG source samples `[0,1]`, and `[255,0]` for WhiteIsZero source samples
`[0,255]`. Relevant implementation is in `image`'s PNG decoder and
`png`'s `expand_gray_u8`, and `tiff`'s `decoder/image.rs` `expand_chunk`.

The corner-origin DICOM SCOORD convention itself is correct; it was checked
against [DICOM PS3.3 C.18.6](https://dicom.nema.org/medical/dicom/current/output/chtml/part03/sect_C.18.6.html).
The overall Rust/Svelte split, per-user ephemeral spokes, committed synchronous
annotation writes and local SQLite WAL architecture are reasonable. The
network-filesystem override remains an explicitly risky mode: exclusive SQLite
locking removes the shared-memory requirement, but does not establish reliable
remote locking or fsync behavior; it also needs a defined live/headless export
path because another connection can be locked out. See
[SQLite WAL](https://www.sqlite.org/wal.html) and
[SQLite's network-filesystem discussion](https://www.sqlite.org/useovernet.html).

I could not verify private corpus recipes/conformance infrastructure, private
documentation claims, or the uncommitted benchmark scripts, fixtures, machine
conditions and timings quoted in the plans. They should be supplied as
reproducible acceptance assets; their numbers are not treated here as verified
performance guarantees. No new implementation exists to exercise end-to-end,
and no application test suite was run for this documentation-only review.

Overall assessment: the product split is technically plausible, but the plan
is not yet implementable as one coherent contract. Resolve identity,
transactions/completion and privacy boundaries first; then freeze executable
examples for assignment, agreement and pixel normalization before independent
area implementation proceeds. These fixes preserve the confirmed feature set.
