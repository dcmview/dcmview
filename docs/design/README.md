# Design: the confirmed plan, not current behavior

These documents are the confirmed design for upcoming dcmview releases. They
describe what will be built, not what exists. `docs/architecture.md` remains
the normative description of what exists today.

Decisions marked confirmed were agreed by the owner and are not re-argued in
implementing PRs. An implementing PR cites the doc and section it implements
(for example "annotation-model 7.2" or "seams 6").

## Files

- `annotation-model.md`: the neutral annotation model, file identity and file
  keys, coordinates, geometry and masks, classes and labels, layers, the
  operation model, and EMBED parity.
- `annotation-tools-ux.md`: the annotation tools, pointer and trackpad
  handling, the branching undo tree, masks and the brush, save state, and the
  `tools` config section.
- `gallery-views.md`: the gallery, thumbnails and their endpoint, decode
  scheduling, grouping, and the catalog revision cursor.
- `image-formats.md`: PNG, JPEG, TIFF and WebP support, detection by content,
  the raster pixel pipeline, orientation and colour profiles.
- `output-adapters.md`: export and import adapters, table specs and
  templates, the export view, and the export API.
- `seams.md`: the consolidated list of seams dcmview implements so it can run
  standalone or as a spoke under a hub, with the dcmview-side definition of
  each: the access token, startup JSON, Unix socket mode, `--file-list`,
  `--annotation-config`, `--annotation-backend`, the `/spoke/v1` routes,
  `dcmview inventory`, and the `protocol` and `key_rules` versions.

Section numbers in these files are stable, because other docs and PRs cite
them.

## dcmview-studio

dcmview-studio is the multi-user companion to dcmview. It has its own design
docs, which are not public; this folder refers to it only through the seams
dcmview implements (`seams.md`). Where a section here concerned only the
studio, its heading is kept and its body says so.

## Reading notes

- "Standalone" means dcmview started by a person, as today. "Hub mode" means
  dcmview started as a spoke by a hub.
- The five area docs were written as scoping analyses with options and a
  recommendation per decision. The recommendations were then confirmed; each
  doc's decisions section records the confirmation dates, and later
  amendments are marked in place with their date.
- Cross-references of the form "model 7.2" or "model doc 7.2" point at
  `annotation-model.md`; "`seams.md` 6" points at section 6 of `seams.md`.
