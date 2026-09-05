# dcmview Documentation

Start here when you need more detail than the README provides. `dcmview` is a
temporary DICOM inspection tool for research and development workflows, not
clinical diagnosis. Do not publish DICOM files, screenshots, logs, local paths,
or metadata unless they are fully de-identified and approved for public use.

<!-- dcmview-marketing:start -->
## Viewer gallery

### Cine playback and semantic context

![Chest CT cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/chest-ct-cine.gif)

![DICOM SEG semantic overlay in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/mr-seg-cine.gif)

### Modality coverage

![Chest radiograph in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/radiograph.png)

![Mammography study in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/mammography.gif)

![PET cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/pet-cine.gif)

![Ultrasound cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/ultrasound-cine.gif)

![RT Dose semantic context in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/rt-dose-context.png)

![DICOM whole-slide microscopy context in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/wsi-context.png)

[Source imagery attribution](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/media/marketing/ATTRIBUTION.md)
<!-- dcmview-marketing:end -->

## User Guides

- [Troubleshooting](troubleshooting.md): fixes for install, startup, discovery,
  decode, tunnel, VS Code, and annotation CSV problems.
- [Configuration reference](configuration.md): CLI flags, Python wrapper
  parameters, VS Code settings, environment variables, and binary resolution.
- [Annotation reference](annotations.md): EMBED-style ROI CSV columns,
  coordinate format, frame scoping, validation, and export behavior.
- [README](../README.md): install matrix, quick start, remote workflow, viewer
  features, annotations, and issue-reporting guidance.

## Python

- [Python reference](python.md): `dcmview-py` install notes, `view()`
  parameters, blocking and non-blocking usage, context managers, VS Code bridge
  behavior, and examples for notebooks and scripts.

## VS Code And Cursor

- [Editor extension README](../vscode/README.md): VS Code Marketplace and Cursor
  installation, supported platforms, settings, terminal interception, and
  bundled binary behavior.
- [VS Code extension local testing](vscode-extension-local-testing.md):
  development workflow for testing the extension from this repository.

## API And Debugging

- [Internal HTTP API](api.md): viewer-internal endpoints for debugging, smoke
  tests, progressive scan polling, cache headers, raw-frame metadata, and error
  semantics.
- [Bridge protocol contract](contracts/bridge-protocol.json): JSON schema for
  the VS Code bridge launch protocol.
- [Bridge registry contract](contracts/vscode-bridge-registry.json): JSON
  schema for VS Code bridge registry entries.

## Development And Release

- [Contributing](../CONTRIBUTING.md): contributor setup, test expectations,
  fixture policy, documentation expectations, pull request guidance, and
  no-PHI reporting rules.
- [Architecture and test model](architecture.md): normative module ownership,
  runtime and HTTP contract flow, lifecycle invariants, test seams, canonical
  check profiles, and non-blocking extension points.
- [Changelog](../CHANGELOG.md): user-visible CLI, Python, VS Code, API/debug,
  documentation, and packaging changes.
- [Development reference](development.md): source builds, frontend proxy
  workflow, canonical check profiles, fixture policy, and cache budgets.
- [Release process](releasing.md): local release checks, GitHub Release assets,
  PyPI wheels, VS Code Marketplace publishing, and Homebrew tap prerequisites.
- [Release checklist](release-checklist.md): repeatable release-note,
  publication, CI qualification, tagging, documentation synchronization,
  incident handling, and next-version steps for every tagged release.
- [v0.2.10 frontend QA](v0.2.10-frontend-qa.md): active browser matrix,
  release-blocker fixes, automated gate, and remaining responsive manual check.
- [npm audit notes](npm-audit-2026-06-09.md): recorded frontend and VS Code npm
  audit state.

## Internal Planning Notes

These documents are project planning and review notes. They are useful for
maintainers, but they are not user-facing setup guides.

- [Public-facing project review](public-facing-project-review.md)
- [Structural review remediation plan](structural-review-remediation-plan.md)
- [Large-directory scalability plan](large-directory-scalability-plan.md)
- [VS Code extension feasibility](vscode-extension-feasibility.md)
- [VS Code DICOM auto-open feasibility](vscode-dicom-auto-open-feasibility.md)
- [VS Code bridge review findings](vscode-bridge-review-findings.md)
- [VS Code bridge remote reliability plan](vscode-bridge-remote-reliability-plan.md)
- [JupyterLab extension feasibility](jupyterlab-extension-feasibility.md)
