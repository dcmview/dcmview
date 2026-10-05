# Annotation tools and UX: scoping

Scoping analysis, not a spec. Nothing was changed in any repository.
Read against `dcmview/dcmview` at `b78536f` (0.3.1 dev) and the earlier
write-ups: `annotation-model.md` (section 12 confirmed by the owner on
2026-09-29), the integration seams (`seams.md`), `image-formats.md` and
`gallery-views.md` (those three not yet confirmed at the time). Date:
2026-09-29.

Already decided and not re-argued here: dcmview stays ephemeral and the hub
owns durable state; one neutral annotation model with EMBED as an adapter at
parity; standalone gets a URL token and an optional Unix socket. From the
annotation model, confirmed: corner-origin float coordinates in the stored
pixel grid; the geometry types in its 3.1; masks as per-object binary segments
in sparse 64 px tiles with an exclusive-layer flag; class plus attributes,
with hierarchy labels sharing one field system; UUIDv7 ids created by the
client; every edit is an op carrying its before state, undo is an inverse
forward op, and the branching tree is client-only.

Each section gives the options, what they cost against our code, and a
recommendation. Decisions that need the owner are collected in section 14; the
contract for other areas is section 15.

**Amended 2026-09-30 after two external reviews** (the resulting decisions
were all confirmed by the owner as recommended, plus the "fixes, no decision
needed"). The changes to this doc are listed below and made in place.

## Review amendments (2026-09-30)

- **0 (T7), 7.3, 14 item 8**: history survives a server restart **in hub mode only** (soft re-sync); standalone keeps the hard reload, so history does not survive a standalone restart; a memory-backend restart is reported as a reset.
- **1, 3.3, 8.2, 16**: browser display budget: frames above 64 Mpx display through a downsampled canvas, full-resolution pixels still served for readout; masks and the loupe follow the same budget.
- **4.3, 7.2, 7.3, 15**: op queue key is the file key, or the label target id for non-file label targets (layer ops use the layer id); one global ordered client queue replaces "one FIFO per file key"; `Batch` is atomic with one result, now confirmed by the model.
- **6.3, 6.4**: study, series, patient and folder labels queue under their target id, so edits from two files share one key.
- **7.3**: a standalone rekey is one atomic event the client applies to records, queue, history and selection.
- **9**: memory-backend restart shown as a reset; standalone restart still
  reloads.
- **10**: `done` is sent only after the item's queue drains and is acknowledged; a later edit moves the item back to `in_progress`; missing required labels still warn, not block.
- **13**: EMBED golden comparison order-insensitive until rows are sorted by
  path.
- **14**: decision 8 amended; decisions 6 and 7 note the queue key and atomic
  `Batch`.
- **15**: interfaces updated for all of the above, plus background polls marked `X-Dcmview-Background: 1` so they do not keep a spoke from going idle.

---

## 0. Summary

| # | Decision | Recommendation |
|---|---|---|
| T1 | How pointer handling leaves `ImageViewport.svelte` | A small in-house `ToolHost` plus one state machine per tool behind a common `Tool` interface. No drawing library. |
| T2 | v1 tool set and keys | Select, point, line, polyline, polygon (click or freehand), rect, ellipse; brush, eraser and fill-from-shape in the mask milestone. Keys in section 3.1. |
| T3 | Who moves a shape by its interior | The Select tool, or Ctrl/Cmd-drag in any tool. Drawing tools grab handles and outlines, so you can draw a point inside a rectangle. Changes today's rect tool, so needs sign-off. |
| T4 | Mouse and trackpad mapping | Mouse: left = active tool, middle-drag = pan, wheel = zoom (today), right-drag = zoom, right-click = context menu. Trackpad: a sticky per-gesture input profile, pinch and two-finger pan, click-click placement (3.5). |
| T5 | Undo scope | One tree per file (the file is the "document"), plus one tree for gallery bulk actions. Undoing jumps to the frame where the change happened. |
| T6 | Branch UX | Ctrl+Z / Ctrl+Shift+Z walk the current branch and redo follows the newest child; Ctrl+Alt+Z / Ctrl+Alt+Shift+Z step through every state in time order (Vim's `g-`/`g+`); a History panel shows the tree; a one-time hint appears the first time a branch forms. |
| T7 | History memory | 64 MB budget per page, pruning off-path branches first; the tree survives a server restart **in hub mode only** (soft re-sync, keyed by file key), not a standalone restart (hard reload) and not a page reload (amended 2026-09-30). |
| T8 | Classes and keys | A sticky active class; digits 1 to 9 pick it, and also reclassify the selected shape. |
| T9 | Default tools without a config | All vector tools on with the implicit `roi` class; export warns before writing an EMBED CSV that would skip shapes. |
| T10 | Where the annotation UI lives | A new "Annotations" tab in the right sidebar beside Tags, replacing today's floating ROI list. Moves existing UI, so needs sign-off. |
| T11 | Mask rendering and editing | Tile buffers edited in place on the main thread, drawn into lazily created 512 px chunk canvases, compressed in a worker. Measured costs in section 8. |
| T12 | Pleasantness features in scope for v1 | Sticky class, zero modal dialogs, visible save state, "next unfinished" and "done and next", prefetch of the next item, measurements on lines, the pixel-grid brush preview, "protect other segments". |

---

## 1. What exists today (checked)

- **One annotation tool.** `viewerTools.ts`: `ActiveTool = 'pan' | 'scroll' |
  'zoom' | 'window_level' | 'annotate_rect'`, keys `P S Z W R`. The toolbar
  is a `SegmentedControl` over `TOOL_ORDER`.
- **Pointer handling is inline in `ImageViewport.svelte` (1,798 lines).**
  `DragState` is a union of seven modes (`pan`, `wl`, `zoom_drag`,
  `scroll_drag`, `draw_roi`, `move_roi`, `resize_roi`); `onPointerDown`
  (line 1279) switches on `activeTool`, `onPointerMove` and `onPointerUp`
  switch on `dragState.mode`. Middle button pans with any tool; the right
  button is swallowed (`preventDefault`, no action) and the context menu is
  suppressed. The wheel zooms, or scrolls frames when the Scroll tool is
  active.
- **ROI editing.** `roiEditing.ts` does hit testing (handle tolerance
  `max(3, 8 / scale)` image px, then rectangle interiors, topmost first);
  pressing inside a ROI with the rect tool **moves** it, on a handle
  **resizes** it, elsewhere **draws** a new one. `canonicalRect` rounds to
  integer edges and rejects boxes under 2 px.
- **Drafts go through the store.** A move or resize calls
  `annotations.showDraft` on every pointer move, and `beginLiveEdit` stops
  save completions from overwriting the geometry under the pointer. A drag
  that crosses a file or frame change is cancelled and restored
  (`cancelMismatchedRoiDrag`).
- **Rendering.** `RoiOverlay.svelte` is an SVG in image-pixel coordinates
  inside the transformed image layer (`viewBox="0 0 columns rows"`,
  non-scaling strokes, handle radii divided by scale). `RoiLabels.svelte`
  draws `#n` labels unscaled in viewport pixels. The image canvas is sized to
  the full frame (`canvasEl.width = bitmap.width`), so a 4096 x 3328 mammogram
  is already a 54.5 MB RGBA canvas, and a frame near the 268 Mpx raster cap
  would be about 1 GiB, at Chrome's canvas area limit. **Browser display
  budget** (amended 2026-09-30): frames above a display budget (default **64
  Mpx**) display through a **downsampled canvas** sized to fit the budget,
  while full-resolution pixels are still served for readout and the intensity
  gate (image-formats owns the budget and the byte-based decode admission
  behind it). Annotation coordinates are unaffected: overlays stay in stored
  pixel space and the downsampled canvas is only drawn scaled. Below the
  budget, the canvas stays full resolution as today.
- **ROI list.** `RoiList.svelte` floats inside the viewport: count, save
  status (`saving…`, `unsaved`), per-ROI coordinates and frames, "Current" /
  "All" frame-scope buttons, Delete, and Retry/Revert on errors.
- **Keyboard.** One window-level dispatcher (`App.svelte`
  `handleWindowKeydown` over `keyboardShortcuts.ts` `shortcutFor`): Up/Down
  change file, Left/Right and `[`/`]` step frames, Space toggles cine,
  Delete/Backspace delete the selected ROI when the rect tool is active, tool
  letters switch tools. **Finding:** tool letters are matched without
  checking modifiers, so today Ctrl+Z selects the Zoom tool and Ctrl+R flips
  to the ROI tool before the browser reloads. Undo keys need that fixed
  first.
- **No undo of any kind.** The only recovery is "Revert" after a failed save.
- **SEG overlays disable ROI tools** (`overlay` short-circuits the rect
  tool), because the displayed frame belongs to another file.
- **Touch:** the viewport sets `touch-action: none` and uses pointer events,
  so single-finger drags act as the mouse; there is no pinch or two-finger
  handling.
- **VS Code:** the viewer runs in a cross-origin `<iframe>` inside the
  webview (`vscode/src/viewerSessions.ts`), so key events go to the viewer
  and VS Code's own shortcuts do not fire while it has focus (inferred from
  the iframe structure; worth one manual check for Ctrl+Z).

---

## 2. What makes annotation pleasant or tedious

The owner asked for an honest look. This section drives the choices in the
rest of the document.

### 2.1 Where tedium comes from

From how labelling tools are used in practice (CVAT, Label Studio, ITK-SNAP,
3D Slicer, OHIF, and in-house research tools), the pain is rarely the drawing
itself. It is:

1. **Actions per object.** Pick tool, pick class, draw, open a dialog, fill
   attributes, confirm. Every extra step is paid thousands of times. The
   biopsy-clip campaign in the roadmap is "find the clip, click it, next
   image": if that takes more than one click and one key, the tool is in the
   way.
2. **Mode errors.** Drawing when you meant to pan, moving the rectangle you
   wanted to draw inside, pressing a key that meant something else in this
   mode. Each one costs a correction and some trust.
3. **Fear of losing work.** Unclear save state, an undo that drops redo, a
   process that exits with everything in memory. People slow down when they
   are afraid.
4. **Waiting.** A spinner between images, even 500 ms, dominates a session of
   quick reads. Prefetching the next item matters more than any tool.
5. **Precision hunting.** Tiny handles at fit-to-screen zoom on a 4k
   mammogram, a vertex you cannot grab, a brush you cannot see the extent of.
6. **Not knowing what is left.** No progress, no "next unfinished", no
   marker for missing required fields.
7. **Segmentation by hand.** Painting a mass boundary pixel by pixel is
   genuinely slow. Good segmentation tools feel pleasant because of assists
   (fill from a contour, lasso fill, threshold-constrained brushes, hole
   filling, later model suggestions), not because the brush is nice.

### 2.2 What that means for the design

- **Target action counts** for the three canonical flows, used as acceptance
  checks for the implementation:

  | Flow | Target |
  |---|---|
  | Point on a clip, same class as last time, go next | 1 click + 1 key |
  | Rect of a new class with one required category attribute | 1 key (class) + 1 drag + 1 key (attribute) |
  | File-level category label, go next | 1 key + 1 key |
  | Same label on 40 thumbnails in the gallery | a range selection + 1 key |

- **No modal dialogs anywhere in the annotation flow.** Attribute prompts are
  non-blocking popovers (6.2); errors are inline or toasts.
- **One grammar for every tool** (3.2) so modifiers mean the same thing
  everywhere.
- **Undo that never loses a state** (section 7), and save state that is
  always visible (section 9).
- **Assists before new tools** in the mask milestone: fill from shape, lasso
  fill, "protect other segments", intensity-gated brush and hole filling
  (section 8) give more than a better brush.

### 2.3 What not to build

- Blend modes, layer groups, per-layer transforms, rulers and guides. The
  roadmap asks for "only the core features" of layers.
- Persistent per-user preferences in dcmview (brush size, panel layout). That
  would be a config file; standalone gets sensible defaults per session.
- Rotated rectangles and rotated ellipses in v1 tools (the model reserves
  `angle`; a rotated rectangle is a polygon).
- Hover previews of history states for masks (expensive, and the time-travel
  keys already give instant previews by doing).

---

## 3. Tool set and interaction design

### 3.1 Tools and keys

Existing keys keep their meaning. New letters are provisional and can be
rebound per campaign through the config (section 11).

| Tool | Key | Create | Edit | Snap default | Notes |
|---|---|---|---|---|---|
| Select | `V` | none | move by interior or outline, handles, Shift-click adds to selection | none | Only tool that moves a closed shape by its interior (T3) |
| Point | `D` | click | drag the point | pixel centres (model 2.1) | Drawn at a constant screen size; rapid clicking makes many points |
| Line | `L` | drag, or click then click | endpoint handles | none | Shows length, in mm when spacing is known (3.4) |
| Polyline | `Y` | click vertices; press-and-drag draws freehand; Enter or double-click finishes | vertex handles; Alt-click an edge inserts, Alt-click a vertex deletes | none | Freehand is simplified (3.3) |
| Polygon | `G` | as polyline; clicking the first vertex also closes | as polyline | none | Needs at least 3 vertices |
| Rect | `R` | drag | 8 handles, as today | pixel edges (keeps EMBED exact) | Today's tool, same key |
| Ellipse | `E` | drag a bounding box | 4 axis handles | none | `angle` stays 0 in v1 |
| Brush | `B` | paint into the active segment | paint more | pixel centres (pixel-in-circle rule) | Mask milestone (section 8) |
| Eraser | `X` toggles Brush and Eraser | erase from the active segment | | | Mask milestone |
| Pan, Zoom, W/L, Scroll | `P Z W S` | unchanged | | | Become `Tool`s behind the same interface |

Keys that already exist: Up/Down change file, Left/Right and `[`/`]` step
frames, Space toggles cine, Delete/Backspace delete. New global keys: Ctrl/Cmd+Z
undo, Ctrl/Cmd+Shift+Z and Ctrl+Y redo, Ctrl/Cmd+Alt+Z and
Ctrl/Cmd+Alt+Shift+Z time travel (7.4), `1`–`9` active class (6.1), Esc
cancels the gesture in progress and then clears the selection, Enter finishes
a polyline or polygon, `H` toggles the History panel, `?` shows a key
cheat-sheet, `N` goes to the next unfinished item and Ctrl/Cmd+Enter marks
done and goes next (section 10). Alt+arrows nudge the selection by one pixel
(Shift+Alt by ten), because plain arrows already navigate.

**Brush size and `[`/`]`.** `[`/`]` for brush size is the convention in
painting software (Photoshop, GIMP, Krita), but dcmview uses them for frame
steps. Options: (a) `[`/`]` resize the brush while the Brush or Eraser is
active and step frames otherwise, (b) use `-`/`=` for size everywhere, (c)
Alt+wheel only. Recommend (a), plus a size slider in the tool options, because
users arriving from any painting tool will press `[` without thinking, and
Left/Right still step frames in every tool. This is a mode-dependent key, so
it is listed for the owner (decision 5).

### 3.2 One grammar for all tools

| Input | Meaning in every tool |
|---|---|
| Shift while drawing | Constrain: square, circle, 45° line segments, horizontal/vertical edges |
| Alt while drawing | From the centre (rect, ellipse); temporarily toggle snapping (point, line, polyline, polygon) |
| Shift-click | Add or remove from the selection |
| Esc | Cancel the gesture in progress; if none, clear the selection |
| Enter | Finish the open shape; with a pre-label selected, accept it (future ML) |
| Delete / Backspace | Delete the selection; while drawing a polyline or polygon, remove the last vertex instead |
| Middle-drag | Pan (today; mouse) |
| Wheel | Mouse: zoom, or frames in the Scroll tool (today). Trackpad: two-finger scroll pans, pinch zooms (today). See 3.5 |
| Alt+wheel | Step frames in any tool, with either device (new; 3.5) |
| Right-drag | Zoom (new, mouse only; the right button does nothing today) |
| Right-click on a shape | Context menu: class, layer, frame scope, fill mask inside/outside, duplicate to frame, delete (two-finger click on a trackpad) |
| Press and release without moving | Start click-click placement for rect, ellipse and line (3.5) |

Right-drag zoom versus a context menu is disambiguated by movement: a press
and release under 4 screen pixels of travel is a click. OHIF maps the right
button to zoom as well, so radiology users will not be surprised.

"Ctrl/Cmd" means **Cmd on macOS and Ctrl elsewhere**, never Ctrl on macOS:
there Ctrl+click is a right-click, so Ctrl-drag would open a context menu.

### 3.3 Tool details that decide how it feels

- **Drawing tools grab outlines and handles, not interiors** (T3). Today the
  rect tool moves a rectangle when you press inside it. That makes it
  impossible to draw a point or a smaller rectangle inside a larger shape,
  which is exactly how nested findings (a clip inside a mass, calcifications
  inside a region) are labelled. Recommendation: every drawing tool hit-tests
  (1) handles of the selected shape, (2) outlines of any shape within the
  tolerance; a press anywhere else draws. Moving by the interior belongs to
  the Select tool; in any other tool, **Ctrl/Cmd-drag on an interior moves the
  shape** (similar to Figma's Ctrl/Cmd "direct select"), so no tool switch is
  needed. This changes today's rect behaviour, so it needs the owner's
  sign-off (decision 3). The alternative that keeps parity is to let the rect
  tool keep interior-move and accept that nested drawing needs a tool switch.
- **Hit tolerance** stays screen-based, 8 screen px as today, turned into
  image px by the zoom. Priority: selected shape's handles, then the selected
  shape's outline, then the topmost other outline, then (Select tool only)
  interiors, smallest area first so a small shape inside a large one wins.
  Linear scan is fine for hundreds of shapes per frame; a grid index waits
  until model layers bring thousands.
- **Handles on dense polygons.** A freehand outline can have hundreds of
  vertices. Show vertex handles only within ~40 screen px of the pointer or
  when zoomed past the point where vertices are 6 screen px apart.
- **Freehand simplification.** Ramer–Douglas–Peucker with a tolerance of
  about 0.75 screen px converted into image px at the current zoom, so a
  stroke drawn zoomed out is coarser than one drawn zoomed in, as the user
  expects. Keep the raw points until pointer up.
- **Minimum sizes.** Keep today's 2 px minimum for rectangles (a click with
  the rect tool must not create a sliver); the same for ellipses and lines;
  a polygon needs a non-zero area.
- **Clamping.** Tools clamp to `[0, columns] × [0, rows]` while drawing
  (today's `pointFromPointer` does this), so the store never sees rejected
  geometry.
- **Drafts are not in the store.** A gesture renders from a separate draft
  layer and becomes one op at pointer up. That replaces `showDraft` and the
  `beginLiveEdit` guard: a sync refresh can never fight the pointer, and a
  drag is naturally one undo step.
- **Frame scope when creating.** Single-frame files: `all` (model 2.3).
  Multi-frame: the current frame, as today. The inspector edits the scope
  (current, all, a range), and **`F` adds the current frame to the selected
  shape's frames, Shift+F removes it**. While a shape
  is selected and you scroll frames, it stays selected and shows as a dashed
  ghost on frames outside its scope, so extending a DBT finding over slices
  12 to 18 is "select, scroll, F, scroll, F".
  (Fill-from-shape then moves to the context menu and Ctrl+F; section 8.4.)
- **Copying across frames and files.** Ctrl/Cmd+C and V copy the selection;
  paste onto another frame keeps coordinates (a single-frame shape pasted into
  a multi-frame file gets the current frame). Pasting onto a file with other
  dimensions is refused if the shape would fall outside.
- **Points for clips.** Rapid clicking must never be swallowed by double-click
  detection; the point tool ignores `dblclick` entirely.
- **Precision.** A loupe (a 4x magnifier circle following the pointer while a
  drawing gesture is in progress, toggled with `M`) helps at fit-to-screen
  zoom on large images. Cheap to do because the source canvas is already at
  full resolution (for frames above the 64 Mpx display budget, section 1, the
  loupe reads the full-resolution region instead). Recommend as a v1
  nice-to-have, not a blocker.
- **Pen and touch.** Pens behave as a mouse (pointer events already cover
  them); pressure is ignored in v1 and could later scale brush size. Pinch
  zoom and two-finger pan on touchscreens are added to the `ToolHost` so
  tablets work, but touchscreens are not a v1 target. Trackpads are, and get
  their own section (3.5).

### 3.4 Measurements

A line shows its length at its midpoint, and a closed shape can show area in
the inspector. In mm when the file has `PixelSpacing`; for projection images
with only `ImagerPixelSpacing`, the label says "mm at detector" so nobody
mistakes it for anatomical size; in pixels otherwise. Measurements are derived
for display only and never stored (model 2.1). Useful for "clip within X mm of
the lesion" checks, and nearly free once the line tool exists.

### 3.5 Mice and trackpads (the owner, 2026-09-30: needs its own handling)

Confirming decision 4, the owner asked for specific handling for touchpads and
tool UX built around telling mice and touchpads apart. Right-drag and
middle-drag are mouse gestures; a trackpad user cannot comfortably do either,
and drags on a trackpad are the tiring, imprecise part.

**What happens today (checked).** `ImageViewport.svelte` `onWheel` classifies
every wheel event on its own: Ctrl or Meta (which is how browsers report a
trackpad pinch) zooms; a pixel-mode event with any horizontal delta or a
vertical delta under 50 px (`TRACKPAD_WHEEL_DELTA_THRESHOLD`) pans; anything
else zooms. Three weaknesses: the verdict can flip mid-gesture (a fast
two-finger swipe produces deltas over 50 px and jumps to zooming, and
high-resolution or free-spinning mouse wheels send small pixel deltas and
pan); in the Scroll tool every event steps one frame, so trackpad momentum
runs through dozens of frames; and nothing else in the UI knows which device
is in use.

**What the browser can tell us.** Pointer events report `pointerType` `pen`
and `touch` reliably, but a trackpad and a mouse both report `mouse`. The
only evidence is in wheel events: `deltaMode` (line or page means a mouse
wheel), delta size and regularity (mouse notches repeat one step such as 100
or 120; trackpads send varied, often fractional deltas at 60 to 120 Hz), any
horizontal component, and `ctrlKey` without the key held (a pinch). Safari
also sends `gesturestart`/`gesturechange` for pinches. Browsers do not expose
the momentum phase. An Apple Magic Mouse behaves like a trackpad and should
be treated as one.

**Recommendation: a sticky input profile, classified per gesture.**

- A **gesture** is a run of wheel events with gaps under about 150 ms. It is
  classified once, from its first few events, and keeps that verdict to the
  end, so a swipe never changes meaning halfway.
- The **session profile** (`mouse` or `trackpad`) follows gestures with
  hysteresis: it switches after two confident gestures of the other kind, so
  one ambiguous event does not flip it. A laptop user who plugs in a mouse
  switches within a couple of scrolls.
- **Override:** Auto (default), Mouse or Trackpad in the viewer's settings
  menu, and `tools.input_profile` in the config. Held in memory for the page,
  like other view state.
- The `ToolHost` owns this (a small `inputProfile.ts`, pure and unit-tested);
  tools read the profile from the context and never inspect wheel events.

| Action | Mouse profile | Trackpad profile |
|---|---|---|
| Pan | Middle-drag; Pan tool | Two-finger scroll; Pan tool |
| Zoom | Wheel; right-drag; Zoom tool | Pinch; Zoom tool |
| Step frames | Arrows, `[`/`]`, Alt+wheel, wheel in the Scroll tool | Arrows, `[`/`]`, Alt+two-finger scroll, two-finger scroll in the Scroll tool |
| Context menu | Right-click | Two-finger click or tap |
| Draw a rect, ellipse or line | Drag, or click-click | Click-click (drag also works) |

**Frame stepping from trackpad scrolls.** Accumulate the delta and step one
frame per ~30 px, not one per event, and stop stepping when the deltas decay
smoothly after the fingers lift (a momentum tail), since the browser gives no
phase. With a mouse, one notch stays one frame, as today. Alt is used instead
of Shift because Chrome and Firefox on Windows and Linux turn Shift+wheel into
horizontal scrolling. This replaces the Alt+wheel brush-size idea from my
first draft; brush size stays on `[`/`]` and a slider.

**Tool UX that works without dragging.** These help mouse users too, so they
are not profile-dependent, but they are what makes trackpad annotation
practical:

- **Click-click placement.** A press and release without movement with the
  rect, ellipse or line tool fixes the first corner or end; the shape follows
  the pointer and a second click places it (Esc cancels). Today a click with
  the rect tool is ignored as a sliver, so this adds behaviour without
  changing any existing drag. Polylines and polygons are already click-based;
  their freehand mode needs a drag and is optional.
- **Handles grabbed by click-click too.** Clicking a handle picks it up,
  moving the pointer moves it, clicking drops it. Drag works as before.
- **Masks on a trackpad.** Painting needs a held drag and is the weak spot.
  The trackpad path for segmentation is polygon fill and lasso fill
  (section 8.4), which are click-based or short drags. A **latched brush**
  (click to start painting, click to stop) is available as an option in the
  trackpad profile, off by default, because an unintended latch paints
  wherever the pointer goes; undo makes that recoverable.
- **Hit tolerance** rises from 8 to 10 screen px in the trackpad profile,
  since trackpad pointing is less precise at the moment of clicking.
- **Tap-to-click** produces a press and release with no movement, which the
  point tool and click-click placement handle naturally.

**Showing the difference.** The `?` cheat-sheet and tool tooltips show the
current profile's gestures. The first time a session is classified as
trackpad, one hint appears: "Trackpad: pinch to zoom, two fingers to pan,
Alt+scroll for frames."

**Testing.** Record wheel-event traces as fixtures (macOS trackpad in Chrome,
Safari and Firefox; Magic Mouse; Windows precision touchpad; Linux libinput;
notched and free-spinning Logitech wheels; a Windows mouse in Firefox, which
uses line mode) and unit-test the classifier and the frame accumulator
against them. The per-event heuristic today has no such tests.

---

## 4. Splitting pointer handling out of `ImageViewport.svelte`

### 4.1 Options

- **A. Keep adding cases** to the `DragState` union and the three handlers.
  Six new tools, a brush and a history system would push the file past
  3,000 lines, and every tool's bugs would share one set of closures. Rejected.
- **B. In-house `ToolHost` plus one state machine per tool** (recommended).
  The viewport keeps rendering, transforms and the readout; it forwards
  pointer, wheel and key events to a host that owns capture, universal
  gestures and the active tool.
- **C. A drawing library.** Candidates: Konva or Fabric (their own canvas
  scene graph, conflicting with the existing SVG-in-image-space overlay and
  CSS transforms), Paper.js (vector-only, no masks), OpenLayers draw
  interactions (a map framework), Annotorious (built around OpenSeadragon or
  plain `<img>`, not our viewport), Cornerstone3D tools (brings its own
  rendering, viewport and state model; adopting it means replacing the
  viewer). Each would either fight the existing transform pipeline or replace
  it. The geometry set is small and fixed by the annotation model, so the
  library would mainly save hit-testing code we can write and test in a few
  hundred lines.

### 4.2 The interface (sketch)

```ts
interface ToolContext {
  file: FileSummary; fileKey: string; frame: number;
  toImage(client: Point): Point | null;      // clientToImagePoint, clamped
  toScreen(image: Point): Point;             // imageToViewportPoint
  scale: number;                             // view zoom, for screen-based tolerances
  snap(p: Point, mode: SnapMode): Point;
  hitTest(p: Point, opts: HitOptions): Hit | null;
  records: AnnotationView;                   // read-only view of committed records on this frame
  draft: DraftLayer;                         // what the tool is drawing right now
  commit(op: Op, label: string): void;       // one op, one undo node
  activeClass: ClassDef; activeLayer: Layer; config: ToolsConfig;
  rawFrame(): RawFrame | null;               // for the intensity-gated brush
}

interface Tool {
  id: ToolId;
  cursor(ctx: ToolContext, hover: Hit | null): string;
  pointerDown(e: ToolPointer, ctx: ToolContext): "capture" | "ignore";
  pointerMove(e: ToolPointer, ctx: ToolContext): void;   // e.coalesced for brushes
  pointerUp(e: ToolPointer, ctx: ToolContext): void;
  key(e: KeyboardEvent, ctx: ToolContext): boolean;      // true = handled
  cancel(ctx: ToolContext): void;                        // Esc, file or frame change, pointercancel
}
```

- The host handles, before any tool: the input profile (3.5), middle-drag
  pan, right-drag zoom, wheel, pinch, the file-or-frame-changed cancel (today's
  `cancelMismatchedRoiDrag`), pointer capture and `getCoalescedEvents()`.
- Tool state is a small explicit state machine per tool (`idle`, `drawing`,
  `dragging-handle`, …), unit-testable without Svelte.
- Keys: the global dispatcher asks the active tool first when focus is in the
  viewport or on the body, then falls back to the global table. The table
  gains a modifier check so Ctrl+Z is no longer "Zoom tool".
- Rendering components: `AnnotationOverlay.svelte` (vector SVG in image
  space, generalised from `RoiOverlay`), `MaskOverlay.svelte` (section 8),
  `AnnotationLabels.svelte` (unscaled, from `RoiLabels`), `DraftOverlay`.
- SVG stays the vector renderer. It handles hundreds of shapes per frame with
  non-scaling strokes; if model layers later bring thousands of polygons per
  frame, a canvas renderer can sit behind the same component.

### 4.3 Module layout

```
lib/annotation/
  types.ts            generated from the model crate (ts-rs), as api-types.ts is today
  store.svelte.ts     op-based client store: records by id, indexed by file key and frame;
                      optimistic apply; one global ordered op queue, state tracked per
                      queue key (file key, label target id or layer id), retry and backoff
  history.ts          undo tree (pure TS)
  geometry/*.ts       per type: bounds, hitTest, handles, move, resize, rasterize
  tools/*.ts          one state machine per tool
  masks/*.ts          tile buffers, stamping, fills, chunk renderer; masks.worker.ts for compression
  panel/*.svelte      Annotations tab: list, inspector, labels, layers, history
lib/viewport/ToolHost.svelte.ts
```

`ImageViewport.svelte` loses roughly lines 1279 to 1500 and the drag state,
and gains one `ToolHost` instance. The existing `ImageViewport.test.ts`
(985 lines) keeps passing unchanged through the first step (section 13).

---

## 5. Where the annotation UI lives

### 5.1 Options

- **A. Keep the floating list** in the viewport and grow it. It covers the
  image, and the inspector, labels, layers and history do not fit there.
- **B. A new "Annotations" tab in the right sidebar**, beside Tags
  (recommended). The sidebar is already resizable and collapsible
  (`tag-panel-shell`), and in compact layouts it is already a drawer. Sections,
  each collapsible: **Objects** (list for this frame, filter by class and
  layer), **Inspector** (class, attributes, frame scope, layer, author and
  time), **Labels** (hierarchy labels, 6.3), **Layers** (section 7), **History**
  (section 7.4), and a header with save state and progress.
- **C. A second left sidebar.** The left side is the navigator, which
  annotators also need.

Recommend B, with the Tags tab one click away. The floating list moves, so
this needs sign-off (decision 10). A compact **class bar** (colour chips with
their digit) sits in the viewer toolbar so the most used control never
requires the sidebar.

### 5.2 On-canvas feedback

- Labels next to shapes show the class name (or short code) instead of `#n`,
  and a warning dot when a required attribute is missing.
- Colour comes from the class (schema `color`), with a colour-blind-safe
  default palette when a class has none. The implicit `roi` class keeps
  today's `--roi` colour, so existing users see no change.
- Shapes outside the current frame's scope are hidden, except the selected
  one, which shows as a ghost (3.3).

---

## 6. Classes, attributes and hierarchy labels

### 6.1 Active class and keys

- The **active class is sticky**: it stays chosen across shapes, frames and
  files, because campaigns are usually "many of the same thing".
- `1`–`9` choose the class in schema order, shown as keycaps in the class
  bar. When a shape is selected (including the one you just drew), the digit
  also changes that shape's class. That supports both habits, "class then
  draw" and "draw then class", and the change is visible and undoable. The
  cost is a possible surprise when someone presses a digit meaning "next
  shape" with a shape still selected; recommend it anyway because a freshly
  drawn shape is selected and reclassifying it is the common case
  (decision 9).
- **Class restricts geometry** (model 4.2). Choosing a class whose allowed
  geometry excludes the current tool switches to its first allowed tool;
  choosing a tool greys out classes that cannot use it. With the implicit
  `roi` class, everything is allowed.
- The schema may give classes and options explicit `hotkey`s; campaign
  authors override the defaults there (section 11).

### 6.2 Attributes

- The inspector shows the selected shape's attributes with widgets by type:
  category as a row of buttons with keycaps when there are up to 7 options, a
  dropdown beyond that; multi-category as toggle chips; boolean as a switch;
  number as an input with stepper (and a slider when min/max are set and the
  range is small); text as an input that commits on Enter or blur, so typing
  is one op and one undo step, not one per keystroke.
- **Attribute prompt** (config `prompt_attributes`, default `"required"`):
  after drawing a shape whose class has required attributes, a small popover
  appears beside it with the first missing field. Its options take the digit
  keys while it is open; Esc or clicking elsewhere dismisses it, and the
  shape keeps a missing-field marker. The popover never blocks drawing the
  next shape.
- `required` is advisory (model 4.2): it drives markers, the file's
  "incomplete" state and the done check (section 10), never a save blocker.

### 6.3 Hierarchy labels in the viewer

- The Labels section lists every field whose `applies_to` includes a level the
  open file belongs to, grouped Patient, Study, Series, File, Frame (Frame
  only for multi-frame files, Folder in directory mode and for rasters).
  Rasters show only File, Frame and Folder (model 4.3).
- Values set at a higher level show on every file under it with a "set on
  study" note; the model has no inheritance, so a series value is not copied
  to its files. Whether exports expand ancestors onto files is an adapter
  option (the adapters doc).
- Each change is one `SetLabel` op (label id and `base_rev`, model 7.2),
  applied immediately. A label on a `file` or `frame` target queues under the
  file key; a `patient`, `study`, `series` or `folder` label queues under its
  canonical target id (model 4.3), so the same study label edited from file A
  and from file B goes through one queue key (amended 2026-09-30).
- **Label-only campaigns** (for example "is there a clip: yes/no", no shapes):
  when the schema has no classes, digits go to the options of the first
  file-level category field, so "1, next, 2, next" works with no mouse.

### 6.4 Bulk labels from the gallery

The gallery doc proposed the gallery as the surface for bulk hierarchy labels
(select tiles, press a key). The rule for which target a key press writes:

- A field has one or more `applies_to` levels; a tile represents a level
  (file, stack, series). The action writes at the field level **nearest the
  tile's own level**: exactly the tile's level when allowed; otherwise the
  nearest allowed ancestor (a study-level field on a file tile writes the
  file's study); otherwise each descendant file (a file-level field on a
  series tile writes every file in it).
- Targets are de-duplicated, so selecting four files of one study and
  pressing a study-level key writes one label.
- A stack tile that is exactly one series behaves as a series tile; any other
  stack writes to each of its files. That answers the gallery doc's question
  "is a stack a label target" without adding a stack level to the model.
- One key press is one `Batch` op (model 4.3 and the gallery doc), one undo
  step in the gallery's history (7.2), with a toast "Image quality: good on 12
  series, Undo". The `Batch` is atomic with one result (confirmed in model
  7.2, 2026-09-30): either every target gets the label or none does, and the
  toast reports one outcome.
- Tiles show label chips from the viewer's own layers, which is the batch
  summary the gallery doc asked the annotation model for.

---

## 7. Layers and the branching undo tree

### 7.1 Layers UI

- The Layers section lists layers with: an **active** radio (where new
  annotations go), **visible** (eye; Alt-click solos), **locked**, colour
  mode, opacity (applies to masks and fills), name and count. Read-only
  layers (`review`, `model`, `import` when configured read-only) show a fixed
  lock.
- **Colour by class** (default) or **by layer** (useful when an admin compares
  two annotators in a review spoke).
- Moving shapes between layers: context menu or the inspector's layer field,
  as an update op (model 5).
- Standalone starts with one layer, "Annotations", and imports go into it for
  EMBED parity (model 5). In a hub annotator spoke, the user's own layer is
  the only writable one and is active; model layers (future) are read-only
  with **Accept** (Enter) copying the selected pre-label into the user's layer
  with `derived_from`, and **Accept all on this image**.
- Visible, locked, order and opacity are view state (model 5): in memory per
  page, sent nowhere.

### 7.2 Undo scope

Options:

- **One global tree.** Simple, but Ctrl+Z after moving to the next image would
  silently undo something on an image you are no longer looking at. Tools
  that do this usually navigate back to the change, which is disorienting in
  a campaign that moves forward all the time.
- **One tree per file** (recommended). The file is the "document", as in any
  editor with tabs. Every op made while that file is open, including a
  study-level label set from it, goes into its tree. Gallery bulk actions go
  into one **gallery tree**.
- **One tree per file and frame.** Too fine: DBT work moves between frames
  constantly, and frame scope edits span frames.

Within a file's tree, **undoing a change on another frame jumps to that frame
first** and says so in a toast ("Undid move on frame 14"), so no change is
ever invisible. Selection is not an undo step, but undo and redo select the
records they touched.

Cross-tree interference (a study label set in file A, then changed from file
B) is caught by the staleness check in 7.3: undo in A stops with "changed
elsewhere" instead of overwriting B's change. Both edits carry the label's
id and `base_rev` and share one queue key (the study's target id), so they
also reach the server in order (amended 2026-09-30).

### 7.3 Data structure

```ts
type HistoryNode = {
  id: number;                 // local
  parent: number | null;
  children: number[];         // in creation order
  lastChild: number | null;   // most recently visited, what Redo follows
  op: Op;                     // the forward op as first applied (with before and after)
  label: string;              // "Draw rect (mass)", "Brush stroke", "Set density = 40"
  frame: number | null;       // where to jump on undo or redo
  at: number;                 // wall-clock time, for time travel and the panel
  bytes: number;              // for the budget
  stale: boolean;
};
```

- **Undo** applies `inverse(node.op)` as a new forward op and moves to the
  parent. **Redo** applies the child's `op` again and moves to it. The
  inverse is computed from the stored before state (model 7.2), so it never
  reads the server.
- **Every application gets a fresh `op_id`.** The server de-duplicates by
  `op_id` (`seams.md` 9), so re-sending a redone op with its original id
  would be answered "already applied" and do nothing. The tree stores the
  op's content; each send wraps it in a new envelope. This is easy to get
  wrong and belongs in the op-store tests.
- **`base_rev` comes from the client's current view of each record at send
  time**, not from the stored op. Before sending, the client also checks that
  each record's current value equals the op's before state (for masks, per
  tile). A mismatch means something outside this tree changed the record (a
  second tab, an admin, a pre-label refresh, another file's tree): the node is
  marked stale and the step is refused with a message. The server's
  `base_rev` check then only has to catch changes the client has not seen
  yet.
- **Jumps** (clicking a node in the panel, or time travel) walk up to the
  lowest common ancestor and down to the target. The client sends the **net
  change** per record (before at the current state, after at the target) as
  one `Batch`, so a jump is one round trip and applies all or nothing
  (`Batch` atomicity with one result is confirmed in model 7.2, 2026-09-30).
- **Coalescing:** one gesture is one node (drafts commit at pointer up);
  Alt+arrow nudges within 600 ms merge into one node; a text attribute is one
  node when it commits.
- **Pending saves:** the tree moves optimistically; ops go through **one
  global ordered client queue** (amended 2026-09-30), so an undo
  never overtakes the op it undoes, even when the two touch different queue
  keys (a file op and a study label). Dirty, pending and retry state is
  tracked per queue key: the file key, the label target id for non-file
  label targets, or the layer id for layer ops (model 7.2). A `Batch` goes
  through the queue as one unit. If an op is finally rejected (422 or 409
  after refetch), its node and the subtree built on it are marked stale and
  the user is told.
- **Restarts** (amended 2026-09-30). The tree is keyed by **file
  key**. **In hub mode** a spoke or server restart is a soft re-sync: the
  page keeps its stores, refetches the catalog, remaps tabs, queued ops and
  history by file key, checks campaign and backend identity and resolves
  pending ops, so history survives. **In standalone** the frontend keeps
  today's hard reload on a new server instance (a restart mints a new token
  and usually a new origin), so history and unsent ops do **not** survive a
  standalone restart. A memory-backend restart is reported to the user as a
  **reset**, not recovered. Neither mode survives a page reload: that would
  need browser storage of annotation content, which is PHI-adjacent
  persistent state. Recommend not in v1.
- **Rekeying** (standalone only, model 1.7): when a file's key changes
  mid-session, the client applies the rekey event in one step to its
  records, queue entries, this tree and the selection, so undo keeps working
  on the file under its new key.

### 7.4 Presenting branches

The data structure is the easy part; the risk is that branches exist but
nobody can reach them. Three layers, cheapest first:

1. **Ctrl/Cmd+Z and Ctrl/Cmd+Shift+Z** behave like every editor. Redo at a
   fork follows the most recently visited child. So the owner's case, "undo,
   make a small change, want the old redo back", is one redo away as soon as
   they undo the small change.
2. **Time travel, Ctrl/Cmd+Alt+Z and Ctrl/Cmd+Alt+Shift+Z**, step through
   every state the file has been in, in the order they were visited,
   regardless of branches (Vim's `g-`/`g+`). Nobody needs to understand the
   tree: pressing it repeatedly walks back through time until the lost state
   reappears. Implementation: a visit log of node ids; each step jumps as in
   7.3.
3. **The History panel** (`H`): a compact vertical list, newest at the
   bottom, with forks drawn as indented side branches (like a git graph),
   each row showing the label, the frame and a relative time. Click jumps;
   the current node is highlighted; stale nodes are greyed with a tooltip.

Plus a **one-time hint**, the first time in a session that a new edit creates
a branch: "Your earlier redo steps are kept. Ctrl+Alt+Z to get back to them."
That teaches the feature at the exact moment it matters.

Honest assessment: layers 1 and 2 deliver most of the value; the panel is
what makes branches visible to people who want to see them, and costs the
most UI work. Recommend all three, with the panel allowed to land after the
tools if time is short.

### 7.5 Feasibility with masks, and the memory budget

Feasible. Branching adds nothing per node over linear undo; masks only make
nodes larger, and tile deltas keep them small. Measured with a Node script
(bit-packed 64 px tiles, raw deflate), on a 4096 x 3328 image:

| Stroke | Tiles touched | Stamping, whole stroke | Undo node, deflated before + after | Uncompressed before + after |
|---|---|---|---|---|
| 15 px radius, 1,000 px long | 30 | 12–18 ms over 268 stamps | 2.4 KB | 30 KB |
| 40 px radius, 2,000 px long | 86 | 11 ms over 201 stamps | 5.6 KB | 88 KB |
| 150 px radius, 3,000 px long | 314 | 13–17 ms over 81 stamps | 12.2 KB | 322 KB |
| Fill a 1,500 x 1,500 px region | 576 | n/a | ~4.6 KB (a full tile deflates to 8 bytes) | 590 KB |

Stamping spread over a stroke's pointer events is well under 1 ms per event,
so painting stays at display rate on the main thread. Compressing takes 10 to
40 ms per stroke and runs in a worker after pointer up.

Budget recommendation: **64 MB of history per page**, configurable
(`tools.history.budget_mb`). Nodes hold uncompressed tiles until the worker
returns compressed ones. At about 10 KB per compressed stroke that is several
thousand strokes. When over budget, prune in this order: leaves of off-path
branches (oldest first), then the oldest nodes above the root of the current
path, never the newest 50 nodes on the current path. A pruned tree shows
"older history trimmed" at its top.

Two correctness points for masks, both already in the model: an op on an
exclusive layer carries the tiles it cleared from other segments, so undo
restores them; and the staleness check compares tiles, so undoing a stroke
after another tab painted over the same tiles is refused rather than
corrupting either.

---

## 8. Brush, eraser and masks

### 8.1 Editing representation

- The model stores a segment as bit-packed 64 x 64 tiles per frame. While a
  segment is being edited, the client keeps the tiles it touches unpacked
  (`Uint8Array(4096)`, 4 KB each) and packs them at pointer up. A lesion is a
  few dozen tiles, so this is tens of KB.
- **Pixel rule:** a pixel is painted when its centre (x + 0.5, y + 0.5) lies
  inside the brush footprint or the shape being filled. This matches the
  model's coordinate convention and makes fill-from-rect exactly the EMBED
  box's pixels.
- **Stamping:** coalesced pointer events, one stamp every quarter radius
  along the path, filled as row spans directly into tiles (what the
  benchmark measured).
- **Brush shapes:** round (default) and square (for one-pixel work). Sizes in
  image pixels, shown in the cursor and, when spacing is known, in mm.
  Defaults and presets come from the config.

### 8.2 Rendering

Options:

- **One canvas at image resolution per viewport.** Simple, but another 54.5 MB
  for a mammogram on top of the image canvas, even when the mask is small.
- **Lazily created 512 px chunk canvases** (recommended): a chunk exists only
  where some visible segment has pixels; an edited tile repaints its 64 px
  region with `putImageData`. Memory follows content. Chunks sit in the same
  transformed image layer as the SVG, so zoom, pan, flips and rotation need
  no extra code. On frames above the 64 Mpx display budget (section 1), chunks
  are drawn at the same downsampling as the image canvas, so a
  large mask cannot rebuild a full-resolution canvas; editing still uses
  full-resolution tiles.
- **WebGL with an R8 label texture and a palette shader.** Most efficient and
  makes outline mode and opacity changes free, but adds a GL path to a viewer
  that has none. Keep as the upgrade path if chunk canvases prove slow.

Display: fill at the layer's opacity (default 40%) in the class colour, the
selected segment brighter. An outline-only mode (toggle `O`) is worth adding
soon after, since radiologists often want to see the pixels under a mask.
When zoomed past about 4x, the brush cursor shows the actual pixels it would
paint (a pixel-grid preview), which removes guesswork at boundaries.

### 8.3 Brush behaviour

- Paints into the **active segment**: the selected mask annotation, or, if
  none is selected, a new segment of the active class created by the first
  stroke. `Shift+N` starts a new segment of the active class explicitly (two
  touching masses must be two segments).
- **Eraser** removes pixels from the active segment only (safe default);
  a toggle extends it to every segment in the layer.
- **Exclusive layers** (model 3.2): painting takes the pixels from other
  segments. A **"protect other segments"** toggle paints only unowned pixels,
  which is how boundaries between adjacent structures get drawn quickly
  (ITK-SNAP's "paint over: clear label" mode).
- **Intensity-gated brush:** optionally paint only pixels whose stored or
  modality value is inside a range (defaulting to the current window). The raw
  frame is already on the client for the W/L path; for display-only frames
  (server-windowed colour, some compressed paths) the gate is unavailable and
  the control says so. Cheap, and a real accelerator for calcifications and
  high-contrast structures.
- Multi-frame: the brush paints the current frame. "Copy this frame's segment
  to the next/previous frame" (Ctrl+Shift+Right/Left) is the v1 helper;
  interpolation between frames is later.

### 8.4 Fill from shape

- Context menu or **Ctrl/Cmd+F** on a selected closed shape (polygon, rect,
  ellipse): fill **inside** into the active segment, or Shift for **outside**
  (everything in the image not inside the shape).
- Default: the shape is **consumed**, meaning it is deleted in the same `Batch`
  so the result is one undo step and the canvas is not cluttered with
  scaffolding. Alt keeps the shape. This is a small product call (decision 11).
- **Lasso fill:** in the Brush tool, Shift-drag draws a freehand outline that
  fills on release (Alt-Shift erases). For outlining masses this is usually
  faster than painting.
- **Fill holes** on the active segment (flood fill from the image border)
  as a one-click action.
- Rasterisation uses the pixel-centre rule; polygons use even-odd filling.
- Outside-fill of a whole 4k image touches every tile (about 3,400), each
  deflating to 8 bytes: roughly 30 KB of op payload, acceptable.

### 8.5 Not in the mask milestone

Fractional masks (`depth: 8`) are displayed, not edited ("threshold into
binary" can come with ML work); contour tracing of masks is adapter work
(the adapters doc); 3D brushes, interpolation, region growing and model-assisted
segmentation are later.

---

## 9. Save state and trust

- The annotation tab header and the status bar show one of **Saved**,
  **Saving…**, **Not saved, retrying** (with the reason), or **Error** (with
  Retry). "Saved" means acknowledged by the store, which in hub mode is the
  hub (`seams.md` 9).
- `503 hub_unavailable` is retried with backoff; a `beforeunload` warning is
  shown while anything is unsaved (`seams.md` 9).
- **Restarts** (amended 2026-09-30): in hub mode a server restart is a
  soft re-sync and the header just shows Saving… until pending ops resolve.
  In standalone the page reloads on a new server instance, as today, and
  anything unsaved is lost; with the memory backend the restart is shown as
  a reset ("the server restarted; annotations since the last export are
  gone"), never as a silent empty store.
- A `409` conflict refetches the record, marks affected history nodes stale
  and shows a toast; there is nothing to merge because users never share a
  layer.
- **Standalone risk.** Today, annotations live only as long as the process,
  and the richer tools make much more work possible to lose. Minimum for v1:
  the Export button carries a dot whenever there are changes since the last
  export, and closing the tab with such changes shows the browser's
  "leave page?" prompt. This strengthens the case for the opt-in `file:`
  sidecar backend (`seams.md` 8, first planned for "later"); from the UX side
  I would bring it forward to ship with the new tools.
- `read_only: true` (`seams.md` 7) disables editing tools visibly, with a
  tooltip saying why.

---

## 10. Campaign flow inside the viewer

The hub owns worklists and what "done" means (`seams.md` 10); the keys and
feel belong here.

- **`N`: next unfinished item** in worklist order (`list_position`, `seams.md`
  6), and **Ctrl/Cmd+Enter: mark done and go next**. Without a worklist, `N`
  is unbound and Up/Down work as today.
- The done action warns (does not block) when required fields are missing,
  listing them, with "Mark done anyway".
- **Done waits for the queue** (amended 2026-09-30). Marking done
  sends `done` only after every op for that item has drained from the queue
  and been acknowledged; meanwhile the item shows "Finishing…" and "done and
  next" moves on at once (the send completes in the background, and the
  progress line counts the item when the hub confirms). The progress event
  records the committed revision. If an op for the item is finally rejected,
  `done` is not sent and the item is flagged in the worklist. **Any later edit
  to a done item moves it back to `in_progress`** atomically with the op (the
  hub does this; the worklist and progress line update, with a toast
  "Reopened: edited after done"). When an admin pauses or closes the
  campaign, the client first drains or exports pending work, and shows what
  could not be saved rather than retrying forever.
- **Prefetch the next worklist item** through the existing prefetch policy,
  so "next" shows a frame immediately. This matters more to how the tool
  feels than anything else in this document.
- **Carry the view to the next image** (zoom, pan, W/L) as a toggle, off by
  default because images differ in size and laterality; useful for series of
  similar images.
- A progress line in the annotation header, "37 of 120 done".
- Time per item can be derived from op timestamps; no client
  instrumentation is needed.

---

## 11. The `tools` section of `--annotation-config`

`seams.md` 7 fixes the envelope; the `tools` section is defined here.

```json
"tools": {
  "enabled": ["select", "point", "line", "polyline", "polygon", "rect", "ellipse", "brush"],
  "default": "rect",
  "snap": { "point": "pixel_centers", "rect": "pixel_edges" },
  "frame_scope_default": "current",
  "prompt_attributes": "required",
  "brush": { "sizes": [4, 8, 16, 32, 64], "default_size": 16, "max_size": 512,
             "shapes": ["round", "square"], "intensity_gate": true },
  "fill_consumes_shape": true,
  "measurements": true,
  "input_profile": "auto",
  "trackpad": { "latched_brush": false },
  "history": { "budget_mb": 64 },
  "keymap": { "point": "D", "polygon": "G", "time_travel_back": "Ctrl+Alt+Z" }
}
```

- Every key is optional; unknown keys are rejected at startup with a message
  (consistent with the envelope's validation).
- Navigation tools (pan, zoom, W/L, scroll) are always available.
- `snap` values: `none`, `pixel_centers`, `pixel_edges`.
- **No config file** (decision 12): all vector tools are enabled with the
  implicit `roi` class, the rect tool is the default so the first thing
  existing users see is unchanged, and the brush appears when the mask
  milestone ships. Before an EMBED CSV export that would skip shapes, the
  export shows the adapter's report ("3 ellipses and 1 polygon cannot be
  written to EMBED CSV") and offers the JSON export instead. The alternative
  is rect-only without a config, which keeps today exactly but hides the new
  tools from every standalone user.

---

## 12. Accessibility and environments

- Every action has a key and a visible control; tooltips show keys; `?` opens a
  cheat-sheet generated from the active keymap, so rebinding in a campaign
  keeps it truthful.
- Focus: tool keys act when focus is on the page body or the viewport, never
  in inputs (today's `isEditableTarget` rule stays).
- The default class palette is chosen to be distinguishable for common colour
  vision deficiencies, and selected, draft and incomplete shapes differ by
  line style as well as colour (today's `RoiOverlay` already does this).
- **Keyboard layouts:** tool letters are matched on `event.key` as today. On
  non-QWERTY layouts, `event.code` would keep positions but break mnemonics;
  keep `key`, since the keymap is configurable.
- **VS Code:** keys reach the viewer inside the iframe; one manual check that
  Ctrl+Z and Ctrl+Shift+Z are not intercepted by the webview host is on the
  test list.
- **macOS:** Cmd replaces Ctrl throughout; Cmd+Alt+Z is not a system shortcut.

---

## 13. Suggested build order (for later, not now)

1. **Extract the `ToolHost`** and port pan, zoom, W/L, scroll and the rect
   tool with no behaviour change; fix the modifier check in
   `shortcutFor`. Existing tests pass. Then the input profile with recorded
   wheel traces (3.5), and click-click placement.
2. **Op-based client store and history (vector only).** Until the backend op
   API exists (model build order step 4), the store can translate its records
   into today's EMBED `PUT`, so frontend work does not wait for the backend.
   Undo, redo, time travel.
3. **Vector tools**, class bar, inspector, the Annotations tab (after decision
   10), measurements.
4. **Hierarchy labels** in the viewer, then gallery bulk labels once the
   gallery lands.
5. **Layers UI** and review colouring.
6. **History panel.**
7. **Mask milestone:** tile buffers and chunk rendering, brush and eraser,
   fill from shape, lasso fill, protect-other-segments, intensity gate, fill
   holes, mask undo in the worker.

Tests: the pure modules (geometry, tools as state machines, history, tile
stamping and rasterisation) under vitest; interaction tests with synthetic
pointer events, as `ImageViewport.test.ts` does today; a timing test that
fails if stamping a long stroke exceeds a budget; golden tests that EMBED
export is byte-identical after drawing rects with the new tool (rows compared
order-insensitively until EMBED rows are sorted by path, model 9.2).

---

## 14. Decisions (confirmed by the owner, 2026-09-30)

All 13 confirmed as recommended. Decision 4 came with a condition, now section
3.5: specific handling for trackpads, and tool UX built around telling mice
and trackpads apart. Decision 13: yes to the v1 safety markers and yes to
bringing the `file:` sidecar forward.


1. **Architecture:** in-house `ToolHost` plus one state machine per tool, no
   drawing library. **Confirmed.**
2. **Tool set and keys** in 3.1 (V, D, L, Y, G, R, E, B, X; brush and eraser in
   the mask milestone), all rebindable per campaign. **Confirmed.**
3. **Interior dragging:** drawing tools grab only handles and outlines, so
   shapes can be drawn inside shapes; the Select tool, or Ctrl/Cmd-drag in any
   tool, moves by the interior. Changes today's rect tool. **Confirmed** (sign-off
   for the rect behaviour change).
4. **Right button:** right-drag zooms, right-click on a shape opens a context
   menu (today it does nothing). **Confirmed**, with trackpad handling added
   (section 3.5).
5. **`[` and `]`** resize the brush while Brush or Eraser is active, and step
   frames otherwise (Left/Right always step frames). **Confirmed.**
6. **Undo scope:** one tree per file plus one for gallery bulk actions; undo
   jumps to the frame of the change. **Confirmed.** **Amended 2026-09-30
   (no new decision)**: undo trees stay per file, but ops are sent
   through one global ordered queue with per-key state (file key, label
   target id, layer id), so a study label set from two files' trees is
   ordered on one key (7.2, 7.3).
7. **Branches:** redo follows the most recent branch, time-travel keys walk
   every past state, a History panel shows the tree, and a one-time hint
   appears when the first branch forms. **Confirmed**; the panel may follow
   the tools. **Amended 2026-09-30**: jumps rely on `Batch` being atomic with
   one result, now confirmed in model 7.2.
8. **History lifetime and budget:** 64 MB per page, off-path branches pruned
   first; survives server restarts, not page reloads. **Confirmed.**
   **Amended 2026-09-30**: survives server restarts **in hub mode
   only** (soft re-sync). Standalone keeps the hard reload on a new server
   instance, so history does not survive a standalone restart; a
   memory-backend restart is reported as a reset. Budget unchanged.
9. **Digits:** `1`–`9` set the sticky active class and also reclassify the
   selected shape. **Confirmed.**
10. **Annotation UI placement:** an Annotations tab in the right sidebar beside
    Tags, replacing the floating ROI list, with a class bar in the toolbar.
    Moves existing UI. **Confirmed** (sign-off for moving the ROI list).
11. **Fill from shape consumes the shape** (Alt keeps it). **Confirmed.**
12. **No-config default:** all vector tools on with the implicit `roi` class,
    rect as the default tool, and a warning before an EMBED export that skips
    shapes. **Confirmed.**
13. **Standalone safety:** unexported-changes marker and leave-page prompt in
    v1; and I'd bring the `file:` sidecar (`seams.md` 8) forward to ship
    with the tools. **Confirmed, both.**

Also from the review, not a change to the items above: "done" waits for the
item's queue to drain and a later edit reopens it (section 10); frames
above the 64 Mpx browser display budget show a downsampled canvas (sections 1
and 8.2).

---

## 15. Interfaces for downstream areas

What this doc fixes; section 14 is confirmed.

**For the annotation model (`annotation-model.md`), requests**
- ~~Confirm a `Batch` op is applied **atomically** (all or nothing, one result),
  because history jumps and fill-and-consume rely on it.~~ **Confirmed
  2026-09-30** (model 7.2, 7.4): atomic, one envelope, one result carrying
  every affected revision.
- Relied on from the model (2026-09-30): `SetLabel` carries the label id and
  `base_rev`; the inverse of a delete is `RestoreAnnotation` under the same id;
  standalone rekeys arrive as one event the client applies to records, queue,
  history and selection.
- The client re-sends a redone op's content under a **fresh `op_id`**; op ids
  identify applications, not changes. Worth one sentence in the model doc.
- Confirm clients may compare a record's current value with an op's before
  state to detect staleness (they need the full before value for updates, and
  per-tile before bodies for masks, which 7.2 already provides).
- `Patch` for a mask segment's class, attributes and layer is an ordinary
  `UpdateAnnotation`; tile edits are `MaskTiles`. No other op types are needed
  by the tools.

**For integration (`seams.md`)**
- Ops are sent from **one global ordered client queue** (amended
  2026-09-30, replacing "one FIFO queue per file key"); each op has a queue
  key (file key, label target id for non-file label targets, layer id for
  layer ops) that tracks its dirty and retry state; a `Batch` is one unit in
  the queue. The client never has two ops in flight out of order, and
  retries resend the same `op_id` (UUIDv7).
- History is keyed by file key and survives a server-instance change **in
  hub mode only**, where the page soft re-syncs instead of reloading
  (`seams.md` 9). Standalone keeps the hard reload; a memory-backend
  restart is shown as a reset.
- Background requests (worklist and catalog polls) carry
  `X-Dcmview-Background: 1` so they do not count as activity for idle
  timers.
- `GET /api/annotation-config` supplies the `tools` section in section 11.
- A mask-heavy snapshot can reach a few hundred KB per file; the snapshot
  endpoint should be per file key (it is, in `seams.md` 10).

**For the gallery (`gallery-views.md`)**
- Bulk label keys follow the field's hotkeys (or digits for the primary
  file-level field in label-only campaigns); target resolution is the
  "nearest allowed level" rule in 6.4; a stack tile that equals one series is
  a series.
- One key press is one atomic `Batch` (one result) and one step in the
  gallery's history tree; the gallery shows the undo toast.
- Tiles show label chips and shape counts from the viewer's own layers.

**For output adapters (`output-adapters.md`)**
- The export UI shows `AdapterInfo.capabilities` before export and the
  report after, and warns before an EMBED export that would skip shapes.
- Measurements are display only; adapters derive physical values themselves.

**For the hub**
- The viewer provides "next unfinished" (`N`) and "done and next"
  (Ctrl/Cmd+Enter) keys and a progress line; the states and the
  `/spoke/v1/progress` payload are the hub's (`seams.md` 10).
- `done` is sent only after the item's queue drains and every op is
  acknowledged, and carries the committed revision; a later op moves a done
  item back to `in_progress`; pause and close let the client drain or export
  pending work first (section 10).
- Done warns but does not block on missing required labels.
- Review spokes: read-only `review` layers, colour-by-layer mode, solo per
  layer.
- The `tools` config fields in section 11, including the keymap, are what a
  hub writes for a spoke.

**For image formats (`image-formats.md`)**
- The intensity-gated brush uses the raw frame when there is one; rasters
  with a raw tier (8/16-bit gray, RGB) get it, others show the gate as
  unavailable.
- Masks on rasters use the stored pixel grid like every other annotation; the
  EXIF orientation only changes the view.
- Frames above the browser display budget (default 64 Mpx) display
  through a downsampled canvas; the viewer still needs full-resolution pixels
  for readout and the intensity gate. Image-formats owns the budget value and
  byte-based decode admission.

---

## 16. Risks

- **Scope.** This is the largest frontend change since the viewer was built.
  The build order keeps each step shippable; the mask milestone is the one
  most likely to slip.
- **Mode-dependent keys** (`[`/`]`, digits with a selection, Enter accepting
  pre-labels) trade a little predictability for speed. Each one is visible in
  the UI and undoable.
- **History correctness** across tabs and hub refreshes depends on the
  staleness check; it needs thorough tests, including masks on exclusive
  layers.
- **Performance of SVG** with very many shapes (model layers later) and of
  chunk canvases on very large frames; both have a named upgrade path, and
  the 64 Mpx display budget bounds browser canvas memory for the
  largest rasters.
- **Standalone data loss** until a persistence option exists (section 9).


---

## Re-baseline amendment (0.3.2, confirmed by the owner 2026-10-05)

This doc was written against dcmview 0.3.1 dev. Since then 0.3.1 added presentation-state graphic annotations drawn over the frame, and 0.3.2 added `--mask` display masking and in-memory redaction boxes edited with a Redact tool. The owner confirmed these resolutions on 2026-10-05 :

- **The `ToolHost` extraction ports the Redact tool and the presentation-state
  overlay drawing** into `ToolHost` along with pan, zoom, W/L, scroll and
  rect.
- **Redaction boxes keep their own client store** (today a second `AnnotationStore`), outside the op client store, the global queue and the history tree.
