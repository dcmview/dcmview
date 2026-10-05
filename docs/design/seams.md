# Seams between dcmview and a hub

This is the consolidated list of every seam dcmview implements and publishes
so that it can run standalone or as a "spoke" under a hub (dcmview-studio),
followed by the dcmview-side definition of each one. It describes what dcmview
does at each seam. How the hub is built and what it does on its side of a seam
is not part of the public design.

Confirmed by the owner on 2026-09-30 (decisions in section 15), amended the
same day after two external reviews, and extended on 2026-10-01 (section
15, items 12 and 13). Like the rest of this folder it is the confirmed plan,
not current behavior.

Terms: **standalone** is dcmview started by a person, as today. **Hub mode**
is dcmview started by a hub as a child process (a **spoke**), scoped to one
user's files, config and annotation backend. dcmview stays ephemeral in both;
in hub mode the hub owns durable state.

---

## 1. Consolidated seam list

One list of every seam. The "Defined in" column names the doc and section that
owns the seam's content. Every row is covered by the `protocol` integer and
the bump rule in section 12. Status: *confirmed* (in a confirmed doc),
*amended* (confirmed, then changed by the reviews), *new* (added by the
reviews and confirmed), *open* (requested, not yet settled).

| Seam | Defined in | Status |
|---|---|---|
| Bearer token, `--no-token`, `DCMVIEW_TOKEN` | section 2 | confirmed |
| Startup JSON (`url`, `base_url`, `token`, `socket`, `protocol`) | section 3 | confirmed |
| `key_rules` version in startup JSON, inventory summary and `hello`; refusal on mismatch | section 12 | new |
| `--unix-socket PATH` | section 4 | confirmed |
| `--cache-budget BYTES` | section 5 | confirmed |
| Hidden `--exit-with-parent` with a kept-open stdin pipe | section 5 | amended |
| `DCMVIEW_VSCODE_BYPASS=1` on every child; `inventory` dispatched before VS Code routing | section 5 | new |
| `X-Dcmview-Background: 1` on background requests | section 5 | new |
| Safe-to-restart handshake (spoke reports pending ops) | sections 5, 10 | new |
| `--file-list PATH` and `FileSummary.list_position` | section 6 | confirmed |
| File-list column form `#dcmview-file-list v1` (`path`, `key`, `external_id`) | section 6 | confirmed |
| `key` mandatory in hub-written lists; spoke refuses a mismatching file | section 6 | new |
| Per-file patient/study/series columns in the file list | image-formats 11 | open (v1 has `key` and `external_id` only) |
| `--formats` passable by the hub | image-formats 11 | confirmed |
| Live versioned scope update over the spoke channel; every route checks scope | sections 6, 10 | new |
| `--annotation-config` envelope and allowed top-level keys (`version`, `author`, `read_only`, `tools`, `label_schema`, `layers`, `worklist`, `display`, `exports`, `imports`) | section 7 | amended |
| `tools` section | annotation-tools-ux 11 | confirmed |
| `label_schema`, `layers`; `"ordered": true` on category fields | annotation-model 4.2, 5; section 7 | confirmed |
| `worklist` section (`enabled`, `source`, `mode`) and the `/api/worklist` routes | section 7 | confirmed |
| `display` section (`blind_metadata`, `captions`); endpoint and field allowlist under blinding | section 7 | amended |
| `exports` (spec/template list) | output-adapters 13 | confirmed |
| `exports.allowed` and `imports`, enforced in the spoke | section 7; output-adapters 5.4 | new |
| `--export-template PATH`, `--annotations-format` | output-adapters 13 | confirmed |
| `--annotation-backend` (`memory`, `hub+unix://`, `file:`) | section 8 | confirmed |
| `file:` sidecar: lock, directory fsync, persisted outcomes, group commit | section 8 | amended |
| `/spoke/v1/hello`, `annotations`, `ops`, `progress`, `worklist` | section 10 | confirmed |
| Op targets, queue key, atomic `Batch`, UUIDv7, affected revisions in results | section 9; annotation-model 7 | amended |
| Soft re-sync (hub mode), hard reload (standalone), memory reset | section 9 | amended |
| Progress events (`opened`, `done`, `skipped`, `reopened`, `accept`, `return`); `done` after drain with committed revision; later op reopens | section 10 | amended |
| Review snapshots (per-reader `review` layers, read-only); review spokes allow progress events | section 10 | amended |
| `dcmview inventory` subcommand (`FileRef` JSONL) | section 11 | confirmed |
| Per-frame `spacing` in `FileRef` (row, col mm, source tag) | section 11; annotation-model 1.3 | new |
| `dcmview export` subprocess for SEG/NIfTI | section 11; output-adapters 7.4 | new |
| Catalog revision cursor (`?since=<revision>`, `reset`), `X-File-Key` on first frame response | gallery-views 7.4; annotation-model 1.8 | new |
| Shared crates `dcmview-annotation`, `dcmview-adapters`, `dcmview-protocol` by release tag | section 13 | confirmed |

**Public and hidden seams** (confirmed 2026-10-01). Seams that are useful
standalone (`--file-list`, `--cache-budget`, `--annotation-config`,
`--annotation-backend file:`, `inventory`, `--formats`) ship public and
documented. Hub-only surfaces (`hub+unix://`, worklist and review modes,
`/spoke/v1`, the `display` section) ship hidden, are marked experimental in
`AGENTS.md`, and stay outside its sign-off rule until dcmview-studio 0.1 is
released, so the protocol can still change once a hub exercises it.

---

## 2. Access token

**Where the token travels.** The launch URL is
`http://127.0.0.1:PORT/#token=<t>`. Fragments are never sent to the server, so
they are not logged, forwarded or put in `Referer`.

- `index.html` and hashed `assets/*` are served **without** auth. They contain
  only the build, no PHI, so the page loads in any frame or tab.
- On load the frontend reads `#token=`, stores it in `sessionStorage` (per
  tab, survives the reload that `X-Server-Instance` triggers), and removes it
  from the address bar with `history.replaceState`.
- `send()` in `api.ts` adds `Authorization: Bearer <t>` to every API request.
  **Everything under `/api` returns 401 without it**
  (`ApiErrorCode::Unauthorized`). No endpoint is exempt, including
  `/api/health`.
- No cookie. A query-string token (`?token=`) is not accepted.
- Cost: every API load must be a `fetch`. Export ROIs becomes fetch then a
  blob download, and the gallery follows the same rule (section 14).

**The 401 page.** When the API says 401 and no token is stored, the viewer
shows "This viewer needs its access link. Copy the full URL, including
`#token=…`, from the terminal where dcmview is running." When a stored token
is rejected (the process restarted on the same port), it says the viewer
restarted and to reopen from the new link.

**Generation and handling.**

- 32 bytes from the OS RNG, base64url, 43 characters. Constant-time
  comparison.
- **Never on argv.** `/proc/<pid>/cmdline` is world-readable on Linux. A fixed
  token comes from the `DCMVIEW_TOKEN` environment variable. A later
  `--token-file PATH` is fine.
- One token per process, valid for its lifetime. No expiry, no rotation.
- Stdout carries the token (startup line and JSON). Notebook caveat: the
  Python wrapper echoes stdout into the cell, so a saved notebook contains a
  token that stops working when the viewer stops.

**Defaults.** The token is **on by default for every listener**, TCP and Unix
socket alike. `--no-token` is the explicit opt-out, with a stderr warning like
the non-loopback one.

**Effect on each entry point.**

| Entry point | Change |
|---|---|
| CLI, browser opener | `open::that` gets the fragment URL. Nothing else. |
| Printed `ssh -L` hint | Adds the "then open" line with `localhost` and the token. |
| Python `view()` | None required: `handle.url` includes the token. Optionally add `handle.token`. |
| VS Code extension | None required for old extensions. New extensions use `base_url` + `token` **by default**: pass `base_url` through `asExternalUri` and append `#token=` afterwards. `url` stays the fallback for older binaries. |
| VS Code bridge (terminal shims, Python in a VS Code terminal) | None: the extension launches the process and reads `url`. `vscode_session_started.url` carries the token for the handle. |
| Reverse proxy (jupyter-server-proxy) | Works: the fragment stays in the browser, the `Authorization` header is forwarded by default. Proxies that already authenticate can run dcmview with `--no-token`. |
| Scripts using the HTTP API | **Behaviour change**: must send `Authorization: Bearer $TOKEN` (read from startup JSON or set via `DCMVIEW_TOKEN`). `--no-token` restores today's behaviour. Owner sign-off given (section 15, item 3). |
| `--host 0.0.0.0` | Still warns (plain HTTP), but no longer "unauthenticated". |

---

## 3. Startup output and startup JSON

Human output:

```text
dcmview: (on a remote server? run on your local machine: ssh -L 43127:localhost:43127 user@host)
dcmview: then open http://localhost:43127/#token=Xy…
dcmview: server running at http://127.0.0.1:43127/#token=Xy…
```

`--startup-json`:

```json
{"type":"server_started","url":"http://127.0.0.1:43127/#token=Xy…",
 "base_url":"http://127.0.0.1:43127","token":"Xy…",
 "host":"127.0.0.1","port":43127,"protocol":1,"key_rules":1}
```

| Field | Meaning |
|---|---|
| `url` | The full launch URL **including the token**. The Python wrapper, the VS Code extension and the Rust bridge client all use `url` verbatim, so older ones keep working against a newer binary. `null` in socket mode. |
| `base_url` | The URL without the fragment. |
| `token` | The access token, absent with `--no-token`. |
| `host`, `port` | As today (TCP mode). |
| `socket` | The socket path (socket mode only). |
| `protocol` | Integer covering every seam in section 1 (section 12). |
| `key_rules` | Integer version of the file-key rules (section 12). |

New fields are additive; the pinned startup-contract test gains them. Socket
mode prints:

```json
{"type":"server_started","socket":"/…/scan.sock","url":null,"token":"…","protocol":1,"key_rules":1}
```

---

## 4. Unix socket mode

- `--unix-socket PATH`. Mutually exclusive with `--host`/`--port`. No implicit
  default path in v1.
- Linux and macOS only. On Windows the flag errors with a clear message.
- **Replaces TCP**: the process listens on the socket and nowhere else.

**Permissions.**

- dcmview binds inside a directory that is mode 0700 and owned by the process
  uid, and creates the socket with a 0077 umask. The directory is the real
  boundary.
- If the parent directory is not owned by the process uid or is group or
  other writable, dcmview **refuses to start**.
- **Peer-uid check**: on accept, dcmview reads the peer credentials and drops
  connections whose uid is not its own. With `ssh -L`, sshd connects as the
  logged-in user, so this passes for the owner of the process.

**Stale sockets and cleanup.** At startup, if `PATH` exists: a socket that
refuses connections is stale and is unlinked; a live one is "already in use";
a non-socket is an error (dcmview never unlinks a regular file). The socket is
unlinked on graceful shutdown and in a `Drop` guard.

**Token.** Kept on in socket mode. Filesystem permissions stop other accounts
on the server; they do not stop a page in the user's browser from using a
forwarded port. A hub that starts a spoke supplies the token through
`DCMVIEW_TOKEN` and sends the header itself.

**Output and forwarding.**

```text
dcmview: listening on /run/user/1000/dcmview/scan.sock
dcmview: on your local machine run: ssh -L 8080:/run/user/1000/dcmview/scan.sock user@host
dcmview: then open http://localhost:8080/#token=Xy…
```

`url` is null in the startup JSON because the local port is the user's choice.
The Python wrapper and VS Code do not offer socket mode in v1. Forwarding to a
socket needs OpenSSH 6.7+ on both ends and sshd's
`AllowStreamLocalForwarding`; PuTTY cannot forward to a Unix socket.

---

## 5. Flags and headers for a parent process

These exist so that a parent (a hub, or the Python wrapper) can run dcmview as
a well-behaved child. The public ones are useful standalone too.

- **`--cache-budget BYTES`** (public). Scales the display, raw and overlay
  caches proportionally (default total about 700 MiB per process). A thumbnail
  cache takes its share from the same budget.
- **`--exit-with-parent`** (hidden). dcmview treats EOF on stdin as a stop
  signal. It only works if the parent passes a **pipe it keeps open** for the
  child's lifetime and never writes to; a closed or `/dev/null` stdin is an
  immediate EOF and an immediate exit.
- **`DCMVIEW_VSCODE_BYPASS=1`** (existing). A parent sets it on every child so
  a spoke never routes into VS Code. dcmview dispatches the `inventory` and
  `export` subcommands before any VS Code routing.
- **`X-Dcmview-Background: 1`** (request header). The viewer sends it on
  background requests: worklist and catalog polls, and thumbnail prefetch
  beyond the visible screen. A parent can then tell user activity from
  background traffic; dcmview's own behavior does not change with it.
- **Safe-to-restart handshake.** Asked over the spoke channel (section 10)
  whether it is safe to restart, a spoke answers yes only when no op is
  pending or in flight for any open page. The page reports its queue state to
  the spoke for this.
- **`--timeout`** (existing) stays the backstop for an idle spoke.
- **Readiness.** A parent can treat a spoke as ready once stdout shows
  `server_started` (the socket then exists) and `scan_complete`.

Environment: `DCMVIEW_TOKEN` (the token clients must present to this process)
and `DCMVIEW_HUB_TOKEN` (the token this process presents on the spoke channel,
section 8). Neither is ever accepted on argv.

---

## 6. `--file-list PATH`

**Plain form.** Newline-delimited UTF-8 paths, one per line, blank lines
ignored, no comments or quoting. Relative paths resolve against the **current
working directory**, the same rule as positional paths. Entries may be files
or directories; the list is unioned with positional `PATH`s; `--filter` and
`--no-recursive` still apply. Non-UTF-8 paths cannot be expressed in v1.
Missing or unreadable entries are skipped with the usual skip reasons, not
fatal.

**List order is exposed.** Each `FileSummary` gains an optional
`list_position` (the line number), so a worklist or the gallery can present
the list's order. Discovery order stays as it is.

**Column form.** A file whose first line is exactly `#dcmview-file-list v1`
continues with a tab-separated header naming columns. `path` is required;
`key` and `external_id` are optional in general. v1 ignores unknown optional
columns. Per-file patient, study and series columns are not in v1 (open in
section 1).

**Keys in hub-written lists.** A hub always writes the column form, and `key`
is **mandatory** there. In hub mode the listed key is authoritative. The spoke
still computes its own key; if it differs from the listed key under the same
key-rule version (section 12), the spoke **refuses to serve that file** and
reports it (a skip reason and a hub-visible event). It never
silently rekeys. In standalone, keys are session-scoped (annotation-model
1.3).

**Live scope.** In hub mode the file list at startup is the initial scope
only. A new, versioned file list can arrive over the spoke channel (section
10); the spoke acknowledges the version it now enforces, and **every pixel,
metadata and annotation route checks the current scope**, so a stale tab loses
access to a file that left the scope and gains the new ones without a restart.

**Invariant.** dcmview serves only the files in its registered set. There is
no path-based file access in the API, and references resolve within the set.
This is kept and tested.

---

## 7. `--annotation-config PATH`

One JSON document, validated at startup: a bad file exits non-zero with a
message before binding, like a bad `--annotations` CSV. It is exposed to the
frontend at `GET /api/annotation-config`. Absent file means today's behaviour.

```json
{
  "version": 1,
  "author": "alice",
  "read_only": false,
  "tools": { },
  "label_schema": { },
  "layers": [ ],
  "worklist": { "enabled": true },
  "display": { },
  "exports": { },
  "imports": false
}
```

**Allowed top-level keys.** The envelope **rejects unknown top-level keys**.
The allowed set for `version: 1` is `version`, `author`, `read_only`, `tools`,
`label_schema`, `layers`, `worklist`, `display`, `exports`, `imports`. Adding
a key follows the bump rule in section 12.

| Key | Content | Defined in |
|---|---|---|
| `author` | Display only in hub mode, where authorship is stamped on the hub side of the spoke channel. Standalone it defaults to the Unix username. | annotation-model 6.1 |
| `read_only` | `true` disables all editing tools in the viewer. | annotation-tools-ux 9 |
| `tools` | Tool enablement, defaults, keymap. | annotation-tools-ux 11 |
| `label_schema`, `layers` | The label schema and the initial layers. A `category` field may carry an optional `"ordered": true`, meaning its option order is a scale. | annotation-model 4.2, 5; this section |
| `worklist` | `{ "enabled": bool, "source": "hub", "mode": "annotate" \| "review" \| "adjudicate" }`. With a worklist the spoke serves its own routes (for example `GET /api/worklist` and `POST /api/worklist/progress`, declared in `contracts.rs` like every endpoint), forwarding to `/spoke/v1/worklist` and `/spoke/v1/progress` (section 10). | this section |
| `display` | `{ "blind_metadata": bool, "captions": "default" \| "neutral" }`. With `blind_metadata` on, the spoke serves only an allowlist of endpoints and fields (errors, downloads, previews, `/api/series` descriptions and FileRefs included), uses opaque grouping ids, and refuses tag and semantic-context endpoints. | this section |
| `exports` | `{ "allowed": bool, "specs": [...] }`. `specs` is the spec and template list; `allowed` says whether this spoke's user may export. | output-adapters 5.4 |
| `imports` | Boolean: whether this spoke's user may import. | output-adapters 5.4 |

`exports.allowed` and `imports` default to `false` in hub mode and are ignored
in standalone, where the user owns the process. With `display.blind_metadata`
on, the spoke refuses identifier-bearing exports and all imports regardless.
The spoke enforces these on every export, check and import route.

---

## 8. `--annotation-backend SPEC`

| Spec | Meaning |
|---|---|
| `memory` | Today's in-memory store (default). |
| `hub+unix://PATH` | Pass-through to a hub over a Unix socket at `PATH`, presenting the bearer token from `DCMVIEW_HUB_TOKEN`. The spoke keeps no annotation cache. |
| `file:PATH` | Opt-in write-through sidecar for standalone persistence. Ships with the annotation tools. |

Inside dcmview this is one annotation store trait (annotation-model 7.4): the
in-memory store is one implementation, the hub client another, the sidecar a
third. The EMBED-shaped endpoints stay as a view over whichever store is
active. `--annotations CSV` together with a hub backend is rejected.

**The `file:` sidecar.**

- Opt-in only. Without it nothing is written, so "ephemeral by default" still
  holds; this is a user-chosen output like Export, never a change to DICOM.
- Content: the neutral model's serialization (annotation-model 6.4), keyed by
  stable file key, so it reloads regardless of discovery order.
- **Write-through**: an op is acknowledged only after the file is durable
  (write to a temp file in the same directory, fsync the file, rename, then
  **fsync the parent directory**).
- **Group commit**: ops that arrive while a write is in progress are batched
  into the next snapshot, and each op's acknowledgement still waits for the
  durable write that contains it.
- **Persisted op outcomes**: the applied `op_id`s and their results are
  written with the snapshot, so a retried op after a restart returns its
  original result instead of applying twice.
- **Exclusive writer lock**: an exclusive advisory lock (`flock` on a
  `PATH.lock` file) for the process lifetime; a second dcmview pointed at the
  same sidecar refuses to start with a message naming the holder's pid.
- If `PATH` exists at startup it is loaded and edits continue it; a malformed
  file stops startup with a message rather than being overwritten.

---

## 9. Operations, queues and restarts

The browser-to-dcmview contract is identical for every backend, so the
frontend has one code path. Payloads are in annotation-model 7; this section
fixes the transport rules dcmview and its frontend follow.

- Writes are **operations**, each with a client-generated `op_id`
  (**UUIDv7**). Applying the same `op_id` twice returns the original result,
  so every retry is safe.
- Each op names a **versioned target**, never a file index: the annotation id
  and stable file key; the label id (plus its `LabelTarget`) and `base_rev`
  for `SetLabel`; the layer id and its revision for layer ops.
- **Queue key**: the file key, or the canonical label target id for labels on
  a non-file target (study, series, patient, folder), or the layer id for
  layer ops. The client sends ops through one global ordered queue and tracks
  dirty, pending and retry state per queue key. Retries resend the same
  `op_id`; a redone change goes out under a fresh `op_id`.
- **`Batch` is atomic**: it applies entirely or not at all, returns **one**
  result, and passes through the queue as one unit.
- A result returns **every affected revision**, so the client updates all its
  `base_rev`s at once. A `base_rev` mismatch is answered `409` with the
  current state.
- **"Saved" means acknowledged by the store**: the hub in hub mode, the
  durable file with `file:`, memory otherwise.

**Status mapping, spoke to browser.** Backend unreachable or timed out gives
`503` with `code: hub_unavailable`, always retryable and never partially
applied; `409` conflict; `422` invalid op; `403` read-only or not in scope.
The spoke allows 5 s per backend call. On `503` the browser keeps the edit
dirty, retries with backoff, shows "not saved", and warns on tab close.

**Restarts.**

- **Hub mode: soft re-sync.** On a new `X-Server-Instance` the page keeps its
  stores, refetches the catalog (the `?since=<revision>` cursor, gallery-views
  7.4), remaps open tabs, queued ops and history by file key, verifies through
  `/spoke/v1/hello` that the campaign and backend identity are unchanged, and
  then resolves pending ops by resending them under their original `op_id`s.
- **Standalone: hard reload**, as today. A restarted standalone process has a
  new token and usually a new port, so history and unsent ops do not survive.
  A `file:` sidecar keeps the annotations themselves.
- **Memory backend: reset.** Acknowledged annotations die with the process.
  The page reports a reset (backend identity changed, previous work not on the
  server), never a recovery.

---

## 10. The spoke channel: `/spoke/v1`

With `--annotation-backend hub+unix://PATH` the spoke is a client of a small
versioned HTTP API on that socket. It authenticates with its bearer token and
never states who it is; identity, authorship and timestamps are assigned on
the hub side. The spoke's `author` field is advisory.

| Method | Path | Purpose (spoke side) |
|---|---|---|
| GET | `/spoke/v1/hello` | Protocol and key-rule version check, campaign and backend identity (for soft re-sync), user display info |
| GET | `/spoke/v1/annotations?file_key=` | Current records for the files the spoke serves. In review mode the response carries several readers' records as per-reader read-only `review` layers, which the spoke passes through |
| POST | `/spoke/v1/ops` | Ops for one queue key, or one atomic `Batch`; results `{op_id, status, revs: [{target, rev}], seq}`, one result for a `Batch` |
| POST | `/spoke/v1/progress` | Worklist progress events |
| GET | `/spoke/v1/worklist` | The worklist for this spoke's user |

**Messages to the spoke.** Two messages travel the other way: the **scope
update** (a versioned file list, section 6), which the spoke acknowledges with
the version it now enforces; and the **safe-to-restart query** (section 5).
The messages and their versioning are the contract; the carrier (a long-poll
the spoke holds open, or a call to a route on the spoke's socket) is not fixed
here.

**Progress events.** `opened`, `done`, `skipped` (with a reason) and
`reopened` in annotate mode; `accept` and `return` in review mode. The spoke
sends `done` only after the item's op queues are drained and acknowledged, and
the event records the committed revision. A later op on a done item reopens
it; the viewer shows the item as in progress again. When ops start being
refused because work was paused or closed, the client first drains or exports
pending work and shows what could not be saved.

**Review mode.** With `read_only: true` and `worklist.mode` set to `review`,
the viewer disables editing and shows the `review` layers read-only. Progress
events (`accept`, `return`) are still sent.

---

## 11. `dcmview inventory` and `dcmview export`

**`dcmview inventory`** (public):

```text
dcmview inventory [PATH…] [--file-list PATH] [--filter EXPR] [--no-recursive]
                  [--hash none|collisions|rasters|all] [--root DIR…] --jsonl
```

It runs the same discovery and file-key code as the viewer and writes JSON
Lines: one `FileRef` line per file
(annotation-model 1.3), skip lines for skipped entries, and a summary line
that carries the `key_rules` version. Each `FileRef` carries `spacing` per
frame (row and column in mm, plus the name of the source tag), so a consumer
can compute millimetre measures without reading DICOM. `--hash` chooses which
files get a whole-file digest (annotation-model 1.7). `--root` names the roots
that `FileRef.path` and folder targets are relative to (annotation-model 1.3,
4.3). The subcommand is dispatched before any VS Code routing.

**`dcmview export`** (for adapters that need DICOM headers, such as SEG and
NIfTI): a subprocess that takes the export view's input (native JSONL for the
selection plus a file list with keys) and writes into a fresh output
directory. It reads the source headers and per-frame spacing itself. See
output-adapters 7.4.

---

## 12. Versioning: `protocol` and `key_rules`

**`protocol`** is one integer covering every seam in section 1, reported in
`server_started` and in `/spoke/v1/hello`. A parent checks it before trusting
a child, at every start.

**Bump rule.** Any change that an older peer would misread bumps `protocol`: a
new **required** field, route or config key; a new allowed top-level config
key (the envelope rejects unknown keys, section 7); a removed or renamed one;
or a changed meaning. A new **optional** field that older peers may ignore
does not bump it. Each seam's owning doc states which of its changes are
required.

**`key_rules`** is a separate integer for the file-key rules
(annotation-model 1.3, 1.7), reported in `server_started`, in the `dcmview
inventory` summary line and in `/spoke/v1/hello`. Any change to how a key is
derived bumps it. A spoke or inventory run whose `key_rules` differs from the
one its peer recorded is **refused loudly**, because a key-rule change would
orphan stored records. Keys are authoritative from the file list in hub mode
(section 6) and session-scoped in standalone; persisted records resolve
through `FileRef` evidence, not through the key alone.

---

## 13. Shared crates

`dcmview/dcmview` becomes a Cargo workspace. The root package stays the
`dcmview` binary and library, so the Python and VS Code build paths do not
move.

- `crates/dcmview-annotation`: the neutral model, validation, ops
  (annotation-model). No dependency on dicom-rs, axum, tokio or the pixel
  pipeline.
- `crates/dcmview-adapters`: EMBED CSV and the other adapters
  (output-adapters).
- `crates/dcmview-protocol`: startup events, the `--annotation-config`
  envelope, the `/spoke/v1` wire types, the `protocol` and `key_rules`
  constants. Its types feed `ts-rs` like `contracts.rs` does today.

Other repositories consume them as git dependencies pinned to dcmview release
tags. Publication on crates.io can follow if outside adapter authors need it.

---

## 14. Rules for all dcmview work

- Every `/api` request needs `Authorization: Bearer <token>` unless the
  process runs with `--no-token`. `index.html` and `assets/*` are public.
- The frontend loads every API resource with `fetch` through `send()` in
  `api.ts`. No `<img src="api/…">`, no `<a href="api/…">`, no `EventSource`
  without the header. Downloads are fetch-then-blob. Thumbnails are fetched
  into blobs.
- New endpoints are declared in `contracts.rs` as today and inherit auth; no
  endpoint is exempt.
- Responses stay streamable and cacheable per file key, not per index.
- Background requests carry `X-Dcmview-Background: 1` (section 5).
- Every catalog consumer applies updates by file key, not by index.

**Tests the seams need** (dcmview side): the 401 matrix over `endpoints::ALL`;
fragment handling in the frontend; socket stale-file and permission cases and
peer-uid rejection; `--file-list` parsing in both forms; config validation
errors and unknown-key rejection; `--exit-with-parent` with a kept-open pipe
and immediate exit on a closed stdin; a spoke under `VSCODE_*` variables with
the bypass set never opens in VS Code; a file that left the scope returns 403
on every route after the scope update; a listed key that differs from the
spoke's is refused; the sidecar lock refuses a second writer; soft re-sync
keeps history and queue across a spoke restart; a memory-backend restart shows
a reset; a `Batch` is all-or-nothing with one result; a mixed-version test
(old startup-line parser, new line) for the Python wrapper and the VS Code
extension.

---

## 15. Decisions

Confirmed by the owner on 2026-09-30 unless a later date is given. Item 3
counts as the `AGENTS.md` sign-off for narrowing unauthenticated API access.

1. **Token transport:** fragment token plus bearer header, page shell public,
   no cookie. **Confirmed.**
2. **Startup JSON:** `url` includes the token, with new `base_url`, `token`,
   `socket`, `protocol` fields. **Confirmed.** **Amended 2026-09-30**: new VS
   Code extensions use `base_url` + `token` by default; a `key_rules` field
   joins `protocol`.
3. **Defaults:** token on for every listener, `--no-token` opt-out,
   `DCMVIEW_TOKEN` to fix one; scripts calling the API send the header.
   **Confirmed (owner sign-off).**
4. **Token in socket mode:** kept on. **Confirmed.**
5. **Spokes:** dcmview runs as a spoke with one user's file list, config and
   backend. **Confirmed.** **Amended 2026-09-30**: the startup file list is
   the initial scope only, with live versioned scope updates;
   `X-Dcmview-Background: 1` marks background requests; the safe-to-restart
   handshake; `DCMVIEW_VSCODE_BYPASS=1` on every child.
6. **Sync:** synchronous write-through, no spoke cache, idempotent ops
   addressed by stable file key. **Confirmed.** **Amended 2026-09-30**: ops
   name versioned targets; queue key is the file key, label target id or
   layer id; `Batch` is atomic with one result; `op_id` is UUIDv7; results
   return every affected revision. Keys come from the file list in hub mode
   (mandatory `key` column, refusal on mismatch, `key_rules` version).
   Restart: soft re-sync in hub mode, hard reload in standalone,
   memory-backend restart reported as a reset. `done` waits for the item's
   queue to drain; a later op reopens it.
7. **Crate sharing:** dcmview as a workspace, tagged git dependencies.
   **Confirmed.** **Amended 2026-09-30**: the `protocol` bump rule and this
   consolidated seam list.
8. **Standalone sidecar:** opt-in `file:` backend shipped together with the
   new annotation tools; in-memory stays the default. **Confirmed.**
   **Amended 2026-09-30**: exclusive writer lock, directory fsync after
   rename, persisted op outcomes, group commit with durable acks.
9. **Orphaned children:** hidden `--exit-with-parent`. **Amended
   2026-09-30**: only with a stdin pipe the parent keeps open.
10. **Blinding and exports at the boundary** (added 2026-09-30): the config
    envelope gains `imports` and `exports.allowed` (allowed top-level keys now
    include `exports`, `display`, `imports`); with `blind_metadata` on, the
    spoke refuses identifier-bearing exports and all imports.
11. **`inventory` spacing and `dcmview export`** (added 2026-09-30): `FileRef`
    carries per-frame spacing; exports that need DICOM headers run in a
    `dcmview export` subprocess.
12. **Hub-only seams are unpromised until the studio ships** (confirmed
    2026-10-01): see the note under the table in section 1.
13. **Two small seam calls** (confirmed 2026-10-01): file-list v1 ignores
    unknown optional columns, and patient, study and series columns stay out
    of v1; no endpoint is exempt from the token, including `/api/health`.
