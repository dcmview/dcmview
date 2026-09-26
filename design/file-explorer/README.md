# File explorer visual exploration

This folder records the file navigator direction that was selected: the
refined card rail (`refined-card-rail-*.png`, Study and Directory
organization, full window and panel crops). The baseline and the rejected
directions' screenshots were removed; their working experiments remain on the
`codex/explorer-*` branches listed below.

## Evaluation frame

- Open `golden-jpeg-baseline-large-single-frame.dcm` for representative image
  scale and preserve the no-pixel and multiframe cases in the navigator.
- Compare Study organization (patient → study → series → image) with Directory
  organization derived directly from each file path.
- Review desktop, compact, and narrow layouts against the real fixture backend.
- Prefer recognition and containment over repeated labels; keep long filenames,
  keyboard semantics, filtering, and active-file state legible.

## Directions

| Direction | Branch | Primary idea |
| --- | --- | --- |
| Nested blocks | `codex/explorer-blocks` | Strong spatial containment for each medical hierarchy tier |
| Hierarchy rail | `codex/explorer-rail` | Dense tree with guide rails and consistent node markers |
| Drill-down browser | `codex/explorer-columns` | One level at a time with a persistent breadcrumb path |
| Card rail (selected, then refined) | `codex/explorer-compact` | Bounded patient cards with colored study/series rails |

## Reference patterns

- Apple column views: each column represents one hierarchy level and selection
  reveals the next. This supports frequent movement through deep structures.
- IBM Carbon tree view: branch/leaf affordances, consistent icons, and keyboard
  behavior make file-system hierarchies recognizable and accessible.
- Windows tree view: indentation, chevrons, folder icons, and document icons
  should work together rather than relying on indentation alone.
- VS Code compact folders: single-child directory chains can be visually
  compressed when density becomes a problem.

The Study/Directory segmented control is treated as an organization switch, not
as a separate tool. Both modes therefore share the same header, search position,
selection accent, density, and collapse behavior in each concept.
