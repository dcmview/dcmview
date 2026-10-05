# Changelog

All notable user-visible changes to `dcmview` are tracked here. The VS Code
extension also keeps Marketplace-focused notes in
[`vscode/CHANGELOG.md`](vscode/CHANGELOG.md); extension changes that affect the
overall product should be summarized in both places.

`dcmview` is a research and development inspection tool, not a clinical
diagnostic viewer.

## Unreleased

## 0.4.0 - 2026-10-08

### Breaking changes

- Every `/api` request now requires `Authorization: Bearer <token>` by default,
  including health and downloads, over TCP and Unix sockets. HTTP scripts must
  read `token` and `base_url` from `--startup-json` (or extract the token from
  the launch URL fragment and remove the fragment before adding API paths).
  Missing or invalid credentials return `401 unauthorized` with
  `WWW-Authenticate: Bearer`, for unknown API routes too. Startup `url` now
  carries `#token=...`; JSON adds `base_url`, `token`, and `protocol`.
  `DCMVIEW_TOKEN` fixes the session token; `--no-token` explicitly disables
  authentication with a warning for use behind an authenticating proxy.
  Public binds still warn about unencrypted HTTP.
- The token is accepted in the `Authorization` header only: a query parameter
  or cookie does not authenticate. The viewer page and its hashed assets stay
  public and hold no file data. `DCMVIEW_TOKEN` must be non-empty and use only
  `A-Z a-z 0-9 - . _ ~`; an invalid value, or the variable set together with
  `--no-token`, stops startup. The token is never read from the command line.
- Opening the viewer from the terminal, Python `view()` and the VS Code
  extension need no change: each uses the launch URL, which carries the
  token. A bookmarked or hand-typed `http://127.0.0.1:PORT/` without the
  fragment now shows a page asking for the access link.

### Added

- `--cache-budget BYTES` sets the combined display, raw and overlay frame cache
  budget, with proportional shares, binary suffixes such as `256MiB`, and a
  `16MiB` minimum. The default remains 704 MiB; this is not a process memory cap.
- Python non-blocking handles expose read-only `token` and `base_url` startup
  fields for authenticated API calls, while `url` remains the launch URL.
  Older binaries, and viewers routed into VS Code through the bridge, leave
  the new properties as `None`.
- VS Code reads `base_url` and `token` separately so the token survives port
  forwarding, with `url` as the fallback for older binaries. Bridge launch
  URLs retain the token fragment, extension output omits startup credentials,
  and socket-only startup events report an unsupported-launch error.
- `--unix-socket PATH` serves the viewer through a private Unix domain socket
  on Linux and macOS, for shared-server inspection with SSH forwarding.
  The parent directory must be owned by the current effective user and not
  group- or other-writable; a missing parent is created with mode `0700`.
  The socket is mode `0600`, checks peer user IDs, and is removed on shutdown.
  An adjacent mode `0600` lock file prevents a busy or hung viewer's socket
  from being replaced and is also removed on shutdown.
  Socket mode conflicts with `--host` and `--port`, always bypasses VS Code
  routing, and never opens a browser automatically. It prints the
  `ssh -L 8080:<socket> user@host` command and the link to open, and its
  `--startup-json` line reports `socket` with `url: null` and no `base_url`.
- The viewer reads the access token from its launch link
  (`http://127.0.0.1:PORT/#token=…`), removes it from the address bar, keeps
  it for the tab across reloads, and sends it with every API request. Opened
  without the token, or after dcmview restarted on the same port, it says to
  open the link printed in the terminal instead of showing a load error;
  pasting that link into the same tab loads the viewer.
- Export ROIs reports a failed export in the viewer instead of leaving the
  browser on an error page or doing nothing.
- A right-button drag zooms in every tool, up to zoom in and down to zoom
  out, about the point it began on. A right click that does not move still
  does nothing.
- Alt+wheel steps frames in every tool, with a mouse or a trackpad.
- The ROI and Redact tools place a rectangle with two clicks as well as a
  drag: click one corner, move, click the opposite one. Esc cancels, as does
  changing the tool, file or frame.

### Changed

- The viewer tells a mouse wheel from a trackpad one gesture at a time
  instead of one event at a time, and remembers the device for the session.
  A fast two-finger swipe no longer jumps from panning to zooming halfway,
  and a wheel that reports small steps zooms instead of panning once the
  viewer has seen it is a wheel. After changing device, the first two
  gestures may still act as the previous one.
- In the Scroll tool a trackpad scroll steps one frame per 30 px of travel
  instead of one per wheel event, and stops when the fingers lift instead of
  running on through the momentum. A mouse wheel still steps one frame per
  notch.
- In the trackpad profile a rectangle's handles are grabbed from 10 screen
  pixels away instead of 8.

### Changed

- The printed viewer URL (`dcmview: server running at ...`) now ends in
  `/#token=...`, and the SSH forwarding hint is followed by a
  `then open http://localhost:PORT/#token=...` line to copy on the local
  machine.
- The warning for a non-loopback bind now says that plain HTTP does not
  encrypt the access token or DICOM data; it says the endpoints are
  unauthenticated only under `--no-token`.
- A failed browser launch no longer prints the opener's error text, which
  could contain the launch URL; it says to open the launch URL manually.
- The repository is a Cargo workspace. The root package is still the
  `dcmview` binary and library, so `cargo build --release --locked` and the
  wheel and VSIX builds are unchanged; `cargo test` and `cargo clippy` need
  `--workspace` to cover the new `crates/dcmview-protocol` member, which owns
  the launch and startup contract.
- Tagged releases publish only after the CLI, the Python package and the VS
  Code extension pass checks over real SSH and VS Code Remote-SSH against
  the built Linux wheel and VSIX.

### Fixed

- A trackpad pinch in the Scroll tool zooms instead of stepping frames.
- A ROI moved or resized while a second pointer pressed its middle button is
  saved where it was dropped; it used to stay drawn there unsaved.

- A redaction box change can no longer leave a frame cached without the new
  box. A frame requested at the same instant as the change could be rendered
  with the earlier boxes and then served from the cache until the next
  change. This affected 0.3.2.
- Tool, frame, cine and file shortcuts no longer fire while Ctrl, Cmd or Alt
  is held, so Ctrl+Z no longer selects the Zoom tool and Ctrl+R no longer
  selects the ROI tool before the browser or VS Code handles the combination.
  `[` and `]` still step frames when typed with Option or AltGr, and Delete
  and Backspace still remove the selected ROI with a modifier held.

### Documentation

- The README mouse summary described a right-drag zoom and wheel frame
  scrolling that do not exist. It now says what the viewer does: middle-drag
  pans, the wheel zooms about the pointer, and the wheel steps frames in the
  Scroll tool.
- The annotation CSV reference now states the checks that run when a CSV
  loads (non-negative integer coordinates, frame indices, matching counts;
  box bounds are checked when an edit is saved, not on load) and shows the
  empty `ROI_frames` value unquoted, as export writes it.
- `docs/design/` holds the confirmed design for upcoming releases. It is
  design, not current behavior.

### Known limitations

- The token does not encrypt anything: over plain HTTP on a non-loopback
  bind, the token and the DICOM data are readable on the network. Keep the
  loopback default and forward with SSH.
- When dcmview opens the browser itself, the operating system's opener
  receives the launch URL, token included, as a process argument that other
  local users can read on some systems. On a shared machine use
  `--no-browser` and open the printed link, or use `--unix-socket`.
- A token fixed with `DCMVIEW_TOKEN` is not rate-limited; use a long random
  value.
- The `--unix-socket` directory needs a filesystem that supports file locks.
  Some network filesystems do not; use a local directory such as one under
  `$XDG_RUNTIME_DIR`. A forced kill can leave a stale socket, which the next
  launch on that path replaces. Unix socket forwarding needs OpenSSH 6.7 or
  newer on both ends.
- Python `view()` and the VS Code extension do not offer socket mode, and
  `--unix-socket` is not available on Windows.
- `DCMVIEW_TOKEN` set only in a launching terminal or notebook is not passed
  through the VS Code bridge: a viewer the extension manages inherits the
  extension host's environment and otherwise generates its own token.

## 0.3.2 - 2026-10-02

### Added

- `--mask` starts a session that replaces patient identifiers in everything
  the viewer displays, for a shared or recorded screen. It is a display aid,
  not de-identification: files are never modified, nothing is persisted, and
  the mode is fixed for the process. Patients show as numbered pseudonyms and
  files as `File N`; dates and date-times move by one random offset per
  patient within a year; ages above 89 years show `089Y`; person names,
  private elements, and the attributes of the PS3.15 Basic Application Level
  Confidentiality Profile show `[masked]`; and instance UIDs become hashed
  `2.25.` UIDs that agree in the tag tree and every response, so references
  still resolve. Study and Series Description and patient sex, age, size,
  and weight are kept. Presentation state text is not drawn, and slide label
  and overview images are not shown.
- Python `view(mask=True)` starts the same masked session.
- A Redact tool (`X`) draws redaction boxes over burned-in pixel text. A box
  covers every frame of its file unless limited to the current frame, and
  **Apply to series** copies a file's boxes to the same-sized files of its
  series. Boxes work with or without `--mask` and are applied by the server
  in the display frame, the raw frame, and the presentation layer, so a
  redacted region is never sent to the browser.
- In a masked session, files that declare Burned In Annotation show a
  "Burned-in text" badge, since masking does not change pixels.
- `GET` and `PUT /api/file/{index}/redactions` read and replace one file's
  redaction boxes in the `EmbedRoiAnnotations` shape, and
  `PUT /api/file/{index}/redactions/series` copies them across the series.
- `/api/health` and `/api/files` report `masked`. File summaries add
  `display_name` and `burned_in_annotation`, and a presentation state's
  skipped counts add `masked_text`. Content a masked session withholds
  answers `403` with the error code `masked`.

### Fixed

- A page that had finished loading kept showing the previous session after
  the viewer was restarted on the same port. It now checks the server when
  the page is looked at again and reloads onto the new session.

### Known limitations

- Masking does not reach pixels, free text inside kept values such as
  descriptions, or paths: the Directory view shows real folder and file names
  under a "Not masked" note, and the exported ROI CSV keeps real paths. The
  terminal and VS Code's own Explorer are outside the viewer page.
- Redaction boxes are not saved or loaded; they are lost when the viewer
  exits and are not part of the ROI export.

## 0.3.1 - 2026-10-01

### Added

- Graphic and text annotations of Grayscale and Color Softcopy Presentation
  States are drawn on the images they reference: points, polylines,
  interpolated curves, circles, ellipses, and text in image pixel units,
  filled or outlined, in the layer's recommended display color when it
  declares one. An Annotations bar above the viewport offers the states that
  annotate the open image, one at a time and off until chosen. Shapes follow
  zoom, pan, flips, and rotation; text and point marks stay upright.
- Previous/next controls in the Annotations bar, and `,` / `.`, step through
  the shown state's annotation items. The current item is highlighted, the
  rest are dimmed, and stepping opens the image and frame the item
  references. An item applies to the images of its own Referenced Image
  Sequence or, without one, to every image the state's Referenced Series
  Sequence lists; a reference without frame numbers covers every frame.
- `GET /api/file/{index}/frame/{frame}/graphic-annotations?state=` returns
  the objects one presentation state draws on one image frame as JSON in
  image pixel coordinates. A frame the state does not annotate returns empty
  lists.
- `semantic-context` for a presentation state returns
  `kind: "presentation_state"` with its layers, annotation items, annotated
  local frames, and counts of objects that are not drawn. Semantic Context
  for the state can open it, or a single item, on an annotated image.

### Changed

- References of a presentation state also report images named only by an
  annotation item, not just those in its Referenced Series Sequence.

### Known limitations

- Only a presentation state's annotations are applied. Its window, shutter,
  LUTs, displayed area, and rotation or flip are not; objects in DISPLAY
  units are counted and not drawn, and MATRIX units, line and fill styles,
  and text reading direction are not interpreted. Display PNGs fetched from
  the API do not carry the graphics.

## 0.3.0 - 2026-09-29

### Added

- A visible image-position scrubber supports mouse and keyboard seeking at
  desktop, compact, and narrow widths; seeking pauses playback.
- Display responses add `X-Frame-Window-Applied`: `linear`, `real_world`, or
  `voi_lut` for grayscale presentation, omitted for color. The viewer trusts
  only `real_world` as confirmation that a requested unit window was applied.
- Every API response includes `X-Server-Instance`, including errors. A viewer
  detects a replacement server on its next response and reloads the catalog
  before using data from the new session.
- `GET /api/file/{index}/frame/{frame}/value-mapping` returns JSON describing
  stored, Modality, and real-world conversions. Separate RWVM instances can
  supply mappings for all or selected referenced frames; embedded mappings
  remain preferred.
- `GET /api/file/{index}/frame/{frame}/dose-overlay` and
  `GET /api/file/{index}/frame/{frame}/parametric-map-overlay` return the
  source-aligned colorwash as transparent PNGs. Oblique planes are sampled
  through patient coordinates with trilinear interpolation.
- SEG overlays use the segment's recommended CIELab or grayscale display
  color when provided, with palette colors as a fallback.
- WSI context groups navigable companion images by label, overview, thumbnail,
  volume, and other roles. RT Dose context shows grid dimensions, spacing,
  offsets, patient geometry, and referenced objects.
- `GET /api/file/{index}/frame/{frame}/presentation-layer` returns a frame's
  display shutter and overlay graphics as a transparent RGBA PNG;
  `FileSummary.presentation_layer` says which files have one. The value
  mapping gains `voi_lut`, and display frames take `preview=true` for drag
  previews that are not cached.
- Display frames take `unit` with `wc`/`ww` to window a frame's real-world
  values. Cine in the unit of a LUT mapping (a Parametric Map or RWVM LUT,
  non-monotonic ones included, or any mapping behind a Modality LUT) now
  matches the still image exactly; it previously approximated the window on
  stored values or fell back to each frame's default window.
- `--filter` (and Python's `filters=`) accepts DICOM keywords such as
  `PatientID=` or `StudyInstanceUID=` as well as the snake_case names, in any
  case; an unknown field's error lists both spellings.
- `GET /api/file/{index}/frame/{frame}/raw/pixel?row=&column=` returns one
  pixel's stored samples as a 1x1 raw frame. The pixel readout uses it for
  frames too large to fetch whole, instead of downloading tens of megabytes to
  read one value.
- With `--startup-json`, a completed scan that found files prints
  `{"type":"scan_complete","file_count":N}`.
- Display frames report the window they were rendered with in
  `X-Frame-Window-Center` and `X-Frame-Window-Width` (grayscale frames with a
  linear window; not color or VOI LUT frames, nor a window applied in a
  real-world `unit`). When a `unit` window cannot apply to a frame (its
  mapping has another unit, or its samples are not integers), the frame
  reports the default window it was shown with, and the viewer shows that
  window instead of the unit window.
- The status bar says when the server can no longer be reached, with a Retry;
  a failed first load has a Retry too.
- `RUST_LOG` now controls logging (for example `RUST_LOG=dcmview=debug` lists
  every skipped file and why); server errors are logged to stderr with their
  request.
- RLE Lossless frames labelled YBR_FULL_422, and JPEG Lossless and JPEG XL
  Lossless frames labelled YBR_FULL, now display in color. They were
  previously reported unsupported; a JPEG Lossless YBR_FULL frame requested
  anyway showed its Y, Cb, Cr components as red, green, and blue.
- JPEG XL Lossless frames labelled YBR_RCT now display in color; they were
  reported unsupported although PS3.5 lists that layout for the syntax.
- Circular, polygonal, and bitmap display shutters, alone or combined with a
  rectangular one, now mask display frames. Previously only a rectangular
  shutter was applied, and only when Shutter Presentation Value was present;
  a shutter without it now defaults to black.
- Display shutters now also mask color frames from every codec, filled with
  the Shutter Presentation Color CIELab Value when present, and enhanced
  multi-frame objects apply each frame's Frame Display Shutter Sequence.
- A pixel readout follows the cursor over the image: row, column, and frame,
  the stored sample (color components or palette index for color images),
  the Modality value (HU for CT), and the preferred real-world value with its
  unit and the mapping it came from, including RT Dose Grid Scaling in Gy.
  Files whose raw samples the server cannot serve keep the coordinates and
  say the value is unavailable.
- Window/level on a frame with a Real World Value Mapping or Dose Grid
  Scaling now works in that unit: the HUD shows center and width in it, a
  legend shows the window's range, and a dragged window is kept in mapped
  units across frames and files. Each frame, including those cine plays and
  prefetches, converts the window through its own mapping. LUT mappings are
  windowed exactly on the client-side raw path and, for supported integer
  samples, through the display endpoint during cine.
- With a dose or map overlay shown, the pixel readout also reports the
  overlaid value under the cursor ("dose 16.2 Gy"), from the new
  `dose-overlay/values` and `parametric-map-overlay/values` endpoints, which
  send a value overlay's resampled values as little-endian `f32`s.
- RT Dose and Parametric Map colorwash overlays can be drawn on the images
  they cover. An Overlay bar above the viewport turns one on and sets its
  opacity, a color bar shows Gy or the map's unit, and a slice outside the
  volume shows a note instead of a layer. "Show dose on source image" and
  "Show map on source image" in the volume's Semantic Context open a covered
  image with the overlay on.

### Changed

- Every HTTP response carries `X-Content-Type-Options: nosniff`, so browsers
  never reinterpret raw samples, JSON, or CSV that quote file contents.
- The viewer requests its assets and the API with page-relative URLs, so a
  reverse proxy can serve it under a path prefix. The server's routes and the
  documented `/api` paths are unchanged.
- Play with the W/L tool selected uses server-rendered frames with the current
  window; pausing returns to interactive client-side windowing when supported.
- Float samples, fractional Modality values, and real-world windows use the
  exact continuous window formula, including widths below one. Integer LINEAR
  windows retain their minimum width of one and existing pixels.
- The catalog's `raw_windowing_compatible` is always `true`, and
  `raw_windowing_reason` is always `null`: the browser now reproduces the
  declared LUTs, shutters, and overlays. Raw sample availability and browser
  size limits still determine which render path is used.
- `python -m dcmview_py` and the `dcmview`/`dcmview-py` console scripts forward
  arguments, help, version, and exit status to the binary, preserving VS Code
  routing.
- ROI outlines, resize handles, and labels retain their screen size when
  zooming; labels also remain upright through flips and rotation.
- Built on dicom-rs 0.10; JPEG XL frames now decode with jxl-oxide 0.12.
  Frames, raw samples, tags and file metadata are unchanged.
- Dragging the window on frames too large for the browser (over 20 Mpx) now
  updates the image during the drag from server-rendered previews, which are
  not cached; such frames no longer download their raw samples first.
- Window/level now updates live while dragging on files with a Modality LUT,
  a VOI LUT, overlay planes or a display shutter, and on one-bit, 32-bit and
  float frames: the browser windows them and draws the server's shutter and
  overlay graphics on top. They used to change only when the drag ended.
- Grayscale display frames are windowed from decoded samples shared with the
  raw-frame cache, so showing one frame with several windows (cine after a
  window change, a server-side window drag) decodes it once; five windows on a
  512x512 JPEG 2000 frame already fetched raw take 4 ms instead of 127 ms.
- `/api/files` `discovery` lists only skipped and filtered paths (up to the
  256 most recent). Accepted files are already in `files`, and with them in
  the list a large scan pushed every skip reason out of it. The `scanned`,
  `skipped` and `filtered` counters are unchanged.
- `--timeout` (and Python's `timeout=`) counts idle time from the end of the
  scan. It used to start at the first discovered file, so a long scan with no
  viewer open could end the process before the scan finished.
- Discovery is much faster: 3000 small files scan in about 0.05 s instead of
  3.2 s, and more threads now help rather than hurt. Each file is opened and
  its header parsed once, so files with large headers (multi-megabyte private
  sequences) scan in about 60% of the time they took.
- Large grayscale frames render with a quarter of the memory (an 8192x8192
  16-bit frame peaks at about 340 MB instead of 1.4 GB) and faster; automatic
  windows no longer sort every sample, on the server or in the browser.
- Identical concurrent frame requests share one decode, a request whose client
  went away still caches its frame, and concurrent decodes are bounded.
- Encapsulated frames are read by seeking with the offset table, deflated
  frames are streamed, and `/tags` no longer reads pixel data; frame caches are
  bounded by bytes only.
- JSON, CSV, scripts and styles are gzip-compressed for clients that accept
  it; content-hashed assets are cacheable.
- The viewer keeps frames when switching tabs, prefetches a new tab's
  neighbourhood first and the whole stack after 1.5 s or on cine, paces held
  arrow keys, and uses its window/level worker again (it had silently fallen
  back to the main thread).
- `frame_count` is bounded by the frames a file can hold, so a corrupt or
  truncated header no longer exhausts memory during discovery.
- Display frames of layouts the catalog marks unsupported return `422
  unsupported_pixel_layout` instead of `500`; a file deleted after discovery
  returns `404` naming it; error messages name the file, frame and cause.
- The startup summary breaks skipped files down by reason.
- The VS Code bridge client probes a registry entry before launching, so an
  entry whose port another service reused no longer blocks launches.
- Python: `view(..., block=False)` raises `CalledProcessError` when the viewer
  finds no files, non-blocking viewers stop when the interpreter exits, an
  interrupted blocking `view()` stops its viewer, and `CalledProcessError`
  carries the viewer's last output.
- Release binaries are built with LTO and stripped (about 8.7 MB instead of
  17 MB).
- VS Code routing now follows one rule for `dcmview`, `dcmview-py`, and
  `dcmview_py.view()`: open in VS Code from a VS Code terminal, or when the
  working directory is inside an open workspace folder. Otherwise the local
  viewer starts. Python previously routed to any open VS Code window, so a
  `view()` call from outside every workspace folder and outside a VS Code
  terminal now launches locally.
- The Python wrapper routes through the `dcmview` binary rather than its own
  bridge client. Non-blocking VS Code launches return a `ShutdownHandle`; the
  separate `BridgeShutdownHandle` type is gone.
- The viewer now ships its own fonts, Inter and JetBrains Mono, inside the
  binary, so text and tag-table columns look the same on every platform and
  offline. Previously it used each operating system's fonts. Scripts the
  fonts do not cover, such as CJK patient names, still come from the system.
- The viewer has a light theme and follows the operating system's light or
  dark setting; the image area stays dark in both. Inside VS Code it follows
  the editor's colour theme instead, including live theme switches.
- The viewer is restyled in the Bea · dcmview design system: neutral chrome
  with ink-outlined controls and line icons, one context strip for overlays
  and references, tabs that show the stack position, an explorer that shows
  the DICOM hierarchy by nesting with tier icons instead of tier labels, and
  opaque overlays on the image. ROIs are drawn as outlines only, and
  warnings, errors and "No pixels" or "Unsupported" files carry an icon and
  a word as well as a colour. The explorer now opens at 276px and the tag
  panel at 420px.

### Fixed

- The Python wrapper reads startup events only from the binary's stdout. It
  used to merge stderr, where discovery warnings quote file names, so a
  crafted file name could set `handle.url` to another address. stderr is
  still echoed and included in `CalledProcessError` output.
- FRACTIONAL SEGs that declare a maximum above 1 but store only 0/1 across
  the complete object now draw as binary masks, with a Semantic Context
  warning. Genuine fractional opacity and ordinary pixel previews are unchanged.
- Cine resumes after returning to a cached tab. Closing a tab discards its
  zoom, pan, and orientation, so reopening it starts fitted.
- Presets replace a preceding live W/L drag, and a manual drag leaves Full
  Dynamic and preset mode. The HUD, mapped legend, and unit fallback resolve
  the same applied window, including MONOCHROME1 legend direction.
- Scrolling retains the previous image until the next is ready. Images,
  colorwash, presentation layers, mappings, and labels switch together;
  prefetched mappings and overlays no longer blink or race during navigation.
- A presentation-layer failure leaves the image visible with a note. Moving
  between float32 and float64 files no longer combines samples with another
  file's mapping or throws an exception.
- Retry reloads a replacement server's catalog; on the same server it retries
  failed resources and removes the failed-frame placeholder. References
  refresh as discovery progresses and when it completes.
- Tag copy, sequence expansion, and value expansion have separate native
  buttons. Enter and Space activate the focused control without starting cine.
- MONOCHROME1 browser pixels match server inversion, and pixel padding remains
  black instead of turning white.
- Stop-signal listeners are registered before the viewer URL is printed, so
  an immediate Ctrl+C can shut the viewer down cleanly.
- Encapsulated-frame reads reject a fragment length larger than the remaining
  file before allocating its buffer, retaining the existing truncated-input
  error.
- A corrupt JPEG fragment is reported as a JPEG decode failure with the
  decoder's reason, not as an unsupported transfer syntax.
- A transient raw-frame failure no longer turns off client-side window/level
  for the rest of the session.
- `dcmview ... | head` no longer panics on the closed pipe.
- Server-rendered frames show the window they were actually drawn with. Files
  without Window Center/Width showed "W: 1 · C: 0", Full Dynamic showed the
  DICOM window instead of min/max, and a window/level drag on a file the
  browser cannot window started from that placeholder. Frames with no linear
  window (color, VOI LUT) no longer show a W/C value. Files with a real-world
  unit (RT Dose, Real World Value Mapping) label their legend from the same
  window instead of downloading the whole raw frame.
- Display frames larger than the viewer's frame caches (above roughly 32
  megapixels, such as an 8192x8192 image) are shown uncached instead of
  failing with a cache budget error.
- The tag panel's Value column no longer starts past the panel's right edge;
  the keyword column truncates first, so values stay visible at any panel
  width.
- A VS Code viewer that took more than 5 seconds to start was treated as a dead
  bridge: its registry entry was deleted and a second, local viewer opened.
  Slow launches now wait up to 120 seconds, and a launch VS Code did not
  confirm exits instead of opening a second viewer.

### Removed

- The Python wrapper no longer retries binaries older than v0.2.0 without
  startup/bridge flags. Use a current bundled or explicitly selected binary.
- Removed the `--tunnel`, `--tunnel-host`, and `--tunnel-port` options, the
  matching `dcmview-py` `view()` keyword arguments, and the `tunnelled` and
  `tunnel_host` fields of `/api/files`. The helper ran `ssh -L` on the machine
  serving the viewer, which forwarded to the SSH host's loopback rather than
  exposing the viewer to the user's machine, and its readiness probe connected
  to the viewer's own listener, so it reported success even when no forward
  existed. Use the printed `ssh -L <port>:localhost:<port> user@host` command
  from your local machine instead.
- The binary no longer scans the pre-0.2.5 VS Code bridge registry locations
  (`$XDG_RUNTIME_DIR/dcmview/vscode-bridges` and
  `/tmp/dcmview-vscode-bridges-$USER`). Only extension builds older than 0.2.5
  publish there; update the extension if terminal or notebook launches stop
  opening in VS Code.
- Removed `viewer.build_git_sha` from `/api/health`. Embedding the commit made
  every commit, checkout, or pull force a full crate rebuild, and a clone
  without `packed-refs` rebuilt on every Cargo invocation; `viewer.version`
  identifies released builds.
- Removed `source_path` from each frame in `/api/series` stacks. `file_index`
  identifies the file and `/api/files` carries its path; the repeated path
  was most of the response (half of it at 3000 files).

## 0.2.12 - 2026-08-31

### Geometry-Aligned DICOM SEG Overlays

- Added opt-in composition of DICOM Segmentation masks over locally resolved
  source images. The viewer preserves Pixel Preview as the default and exposes
  the composed view only when one source frame is validated for the selected
  SEG frame.
- Resolved sources from explicit per-frame derivation references or declared
  source instances plus patient geometry. Classic source series split across
  multiple single-frame objects are supported, and compatible source and SEG
  grids may use different matrix dimensions.
- Resampled binary and fractional masks through patient coordinates with
  nearest-neighbor semantics and returned transparent, source-sized PNGs from
  the new internal
  `/api/file/:index/frame/:frame/segmentation-overlay` endpoint.
- Kept missing, ambiguous, non-coplanar, non-overlapping, and otherwise
  incompatible mappings unavailable with explicit evidence instead of
  presenting an unvalidated overlay.

### Editor Distribution

- Added independently gated Open VSX publication of the same target-specific
  extension packages attached to GitHub Releases, making the extension
  available to Cursor under `beatricebm.dcmview` without coupling that channel
  to the VS Code Marketplace deployment.
- Updated the editor documentation, package description, and installation
  matrix for both VS Code and Cursor while retaining the existing binary
  resolution, Remote-SSH, notebook, and bridge behavior.

### Documentation And Release Reproducibility

- Added an attributed viewer gallery covering SEG, CT, radiography,
  mammography, PET, ultrasound, RT Dose, WSI, and the real VS Code Explorer
  workflow to the GitHub/PyPI, repository documentation, and editor README
  surfaces.
- Added a pinned, checksum-verified public-source inventory and deterministic
  browser, GIF, and VS Code capture workflow. The committed media lock records
  release inputs, source and output hashes, dimensions, tool versions, capture
  time, and human-reviewed modification summaries.
- Added a maintainer release checklist that treats published tags as immutable,
  qualifies the exact candidate commit through CI, verifies published
  artifacts and channels, synchronizes stable documentation through a pull
  request, and starts the next development version separately.

### Compatibility And Known Limitations

- This release adds no CLI options or breaking package-manifest changes. The
  viewer HTTP API remains an internal debugging and automation surface rather
  than a stable external integration contract.
- Semantic composition is deliberately conservative. Recommended Display
  CIELab colors are not yet interpreted, and ROI editing, interactive
  window/level, and cine remain disabled while a SEG overlay is composed.
- `dcmview` remains a research and development inspection tool, not a clinical
  diagnostic viewer. Public sample imagery is sourced through the NCI Imaging
  Data Commons and is credited in
  [`media/marketing/ATTRIBUTION.md`](media/marketing/ATTRIBUTION.md).

## 0.2.11 - 2026-08-28

### Semantic And Whole-Slide Context

- Added an explicit semantic-context view for DICOM Segmentation, Parametric
  Map, and RT Dose objects. The viewer now summarizes declared segments,
  real-world value mappings, dose scaling and geometry, and resolved source
  references while keeping the stored-pixel preview distinct from interpreted
  metadata.
- Added selected-frame positioning context for Whole Slide Microscopy images,
  including tile coordinates, Total Pixel Matrix location, pyramid level,
  optical path, focal plane, companion objects, and a compact minimap.
- Kept these features deliberately conservative: incompatible or ambiguous SEG
  geometry disables overlay eligibility, and WSI support positions the selected
  tile without claiming to stitch or reconstruct the full slide.

### Display And Decoding Fidelity

- Corrected DICOM LINEAR window boundaries and made server-rendered PNGs and
  browser-side raw rendering use the same presentation rules.
- Applied pixel-padding ranges and presentation processing consistently after
  native and compressed decoding, including JPEG, JPEG-LS, JPEG 2000, JPEG XL,
  RLE, and deflated paths.
- Preserved decoded JPEG color channels, normalized one-bit raw samples, and
  rejected lossy transfer syntaxes when their decoding path could not meet the
  declared fidelity contract.
- Improved encapsulated multiframe extraction for empty Basic Offset Tables,
  Extended Offset Tables, and fragment-spanning frames, while preserving
  lossless JPEG 2000 raw sample layouts.
- Decoded multi-valued Specific Character Set declarations and ISO 2022
  extension sequences more reliably in tags and discovery metadata.

### Viewer Reliability And API Changes

- Added generated API contracts for semantic context, WSI frame context, and
  raw-windowing safety. The viewer now falls back to server presentation when
  client-side windowing cannot safely reproduce the DICOM pipeline.
- Separated display-cache tiers, deduplicated concurrent raw-frame requests,
  and limited the loading indicator to frame work so metadata requests no
  longer obscure an already rendered image.
- Preserved the flexible pixel viewport when semantic or WSI context is shown,
  and limited semantic controls to SEG, Parametric Map, and RT Dose objects so
  unrelated images retain the standard inspection layout.
- Preserved referenced-frame identity across implicit multiframe targets and
  completed resolved target identities for more reliable in-viewer navigation.
- Recognized DICOMDIR deterministically as a metadata-only skipped object, and
  rejected malformed discovery metadata without aborting the wider scan.

### Compatibility Qualification

- Added a versioned support policy, pinned corpus inputs, assertion-backed
  evidence, and reproducible valid, legacy, negative, stress, and bounded fuzz
  profiles for compatibility testing.
- The resolved campaign completed 169 valid objects, one legacy object, 15
  negative cases, 139 stress files, and 24,465 deterministic fuzz operations
  without crashes, timeouts, unacceptable outcomes, or failed required
  assertions. Results describe research-inspection behavior, not clinical
  validation or a DICOM conformance certificate.
- Documented the frozen inputs, results, browser acceptance matrix, and
  intentional support boundaries in
  `docs/dicom-compatibility-campaign-2026-08-28.md` (removed; see git history).

## 0.2.10 - 2026-08-28

### Major Feature Additions

- Added logical series catalogs and virtual frame stacks so related single-frame,
  multiframe, and concatenated DICOM objects can be reviewed as one ordered
  sequence. The viewer now uses geometry-aware ordering, retains logical frame
  identity across source files, and navigates files in explorer order when no
  logical stack applies.
- Added typed DICOM reference extraction, local target resolution, API exposure,
  and in-viewer navigation. References such as a segmentation source can now
  open the resolved local object at the referenced frame.
- Expanded display and raw decoding across RLE Lossless, JPEG-LS Lossless
  grayscale, JPEG XL Lossless RGB, binary deflated image frames, extended native
  numeric formats, and native color layouts. Multifragments and planar RLE color
  are normalized before rendering.
- Added native presentation processing for Modality and VOI LUT sequences,
  embedded overlay planes, rectangular display shutters, stored-bit fields, and
  eight-bit DICOM LUTs. Unambiguous ICC profiles are preserved in generated PNGs
  for native and RLE color images.
- Added a bounded compatibility campaign runner with frozen corpus scope, typed
  expectations, evidence probes, and corrected-corpus overlay merging to make
  codec and presentation support reproducible and auditable.

### API And Observability Changes

- Added explicit prepared-object and transfer-syntax support classifications,
  structured discovery ledger entries, file support observability, and viewer
  build identity to the HTTP API.
- Added stable machine-readable error codes while retaining the shared JSON
  error envelope, plus selective metadata pagination for large tag trees.
- Exposed effective pixel aspect ratio and logical-series/reference data needed
  by the viewer. Images with non-square pixels now render using their physical
  geometry.
- Bounded discovery-ledger responses so scans with many rejected or unsupported
  objects cannot create unbounded API payloads.

### Fixes And Reliability

- Prevented active-file cleanup from recursively updating ROI selection during
  logical stack source changes, and preserved later ROI selection and editing.
- Restored frame slider, keyboard, scroll, and cine navigation for multiframe
  files that use per-file fallback instead of a catalog-backed logical stack.
- Kept files without complete Study and Series Instance UIDs independent rather
  than merging unrelated objects into one logical navigation sequence.
- Retained logical-stack frames across source changes and paced cine playback
  across the normalized sequence, including fallback frame advancement.
- Fell back cleanly to display rendering when an image layout does not support
  raw client-side windowing, avoiding broken placeholders for color images.
- Avoided retaining pixel payloads during discovery, reducing scan-time memory
  pressure, and fixed multifragment JPEG raw decoding.
- Corrected native stored-bit interpretation, eight-bit LUT rendering, planar
  RLE color display, deflated image-object recognition, and registry-dependent
  compatibility evidence.

### Build, Packaging, And Test Changes

- Statically linked the CharLS codec and stabilized its vendored CMake linkage;
  normalized the manylinux CMake library path for wheel builds.
- Updated release-tooling dependencies to address advisories and pinned fixture
  generator/build identity so generated evidence remains deterministic.
- Kept the external test profile limited to feature-gated remote fixtures while
  expanding committed integration coverage for logical series, discovery,
  native pixels, API contracts, tags, and supported codecs.
- Recorded the release frontend QA matrix in
  `docs/v0.2.10-frontend-qa.md` (removed; see git history), including the
  automated core gate, active browser checks, fixes found during review, and the
  remaining responsive-layout manual check.

## 0.2.9 - 2026-08-26

### Viewer Reliability

- Matched annotation CSV paths through normalized absolute keys, including
  relative paths, parent components, and symlink aliases, while moving CSV
  ingestion behind server startup and keeping unmatched large datasets cheap.
- Made Study and Directory explorer presentation deterministic without changing
  progressive file indices or adding CLI sorting controls.
- Replaced interval-driven cine playback with render-paced Loop and Sweep
  scheduling, shared in-flight frame work, decoded-frame prefetching, and
  bounded active-stack retention.
- Kept Explorer and Tags available in narrow browser and VS Code webview layouts
  through accessible overlay drawers with Escape/backdrop dismissal and focus
  restoration.
- Repaired Marketplace documentation links and added packaged-VSIX README
  verification so repository-relative links cannot recur in release artifacts.

## 0.2.7 - 2026-07-28

### CLI

- Expanded Rust CLI help with clearer option descriptions, value names, and
  examples for single-file viewing, recursive directory scans, remote
  `--no-browser` use with SSH forwarding, annotation CSV loading, and filters.
- Expanded `python -m dcmview_py --help` with matching option descriptions and
  examples.
- Hid the integration-only `--startup-json` flag from normal user-facing help.

### Python

- Added a Python wrapper reference covering `view()` parameters, blocking and
  non-blocking usage, context-manager behavior, handle lifecycle, exceptions,
  binary resolution, VS Code bridge behavior, and bypass options.
- Expanded the public `dcmview_py.view()` docstring so `help(dcmview_py.view)`
  is useful in scripts, notebooks, and interactive Python sessions.

### VS Code

- Linked the VS Code README to the shared troubleshooting, configuration,
  Python, and documentation index pages.
- Documented VS Code settings, binary resolution, bridge environment variables,
  and bridge bypass/debug behavior in the shared configuration reference.
- See [`vscode/CHANGELOG.md`](vscode/CHANGELOG.md) for Marketplace-specific
  extension release notes.

### API And Debugging

- Added a dedicated internal HTTP API reference for debugging and test
  automation, including progressive scan fields, polling guidance, cache
  headers, raw-frame metadata headers, annotation behavior, and error semantics.
- Clarified that the viewer HTTP API is internal to the local viewer and should
  not be treated as a stable external integration contract.
- Documented the debugging-only `debug-api` Cargo feature and its permissive
  CORS behavior.

### Documentation And Packaging

- Added troubleshooting, configuration, Python wrapper, annotation, development,
  and documentation index references.
- Tightened the README into a shorter public landing page while preserving
  install guidance, quick start workflows, remote usage, Python and VS Code
  pointers, safety notes, troubleshooting links, and issue-reporting guidance.
- Added contributor guidance for setup, tests, fixture policy, documentation
  expectations, pull requests, and no-PHI reporting rules.
- Clarified that Homebrew distribution is planned but not yet configured; public
  install guidance continues to point to PyPI, VS Code Marketplace, GitHub
  Releases, and source builds.
