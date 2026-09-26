# Changelog

## Unreleased

- `dcmview` no longer looks for bridges in the pre-0.2.5 registry locations
  under `$XDG_RUNTIME_DIR` and `/tmp`. Extension 0.2.5 and later already
  publish only to the per-user state directory.

## 0.2.12 - 2026-08-31

- Publish the target-specific extension packages to Open VSX through an
  independently gated release job so Cursor users can install
  `beatricebm.dcmview` from their extension marketplace.
- Describe VS Code and Cursor consistently across the Marketplace README,
  installation matrix, package metadata, and troubleshooting links.
- Include the validated geometry-aligned DICOM SEG overlay viewer delivered by
  the bundled `dcmview` binary while keeping Pixel Preview as the default and
  refusing ambiguous or incompatible source mappings.
- Add an attributed gallery captured from the real viewer and VS Code Explorer
  **Open with dcmview** workflow, with image URLs pinned to the `v0.2.12` tag.

## 0.2.9 - 2026-08-26

- Keep Explorer and Tags accessible in compact and narrow webviews through
  overlay drawers with keyboard dismissal and focus restoration.
- Use stable absolute GitHub documentation links in the Marketplace README and
  verify their final form inside packaged VSIX artifacts.
- Improve cine playback smoothness in constrained webviews by advancing only
  after frames are loaded and rendered.

## 0.2.5 - 2026-06-12

- Improve VS Code bridge reliability for Remote-SSH and notebook workflows by
  publishing the bridge in the per-user state directory, falling back from stale
  env endpoints to registry discovery, and validating optional client-supplied
  `dcmview` binaries from `dcmview-py`.
- Updated wrappers scan legacy registry locations for one release, so update the
  VS Code extension first when rolling this out across shared hosts.

## 0.2.2

- Add Windows 11 x64 release artifacts across GitHub Releases, PyPI wheels, and
  target-specific VSIX packages.
- Bundle and resolve `dcmview.exe` for Windows Python and VS Code installs.
- Add Windows CI and release validation coverage for the committed fixture
  smoke test.

## 0.2.1

- Publish target-specific VSIX packages for Linux x64, macOS x64, and macOS
  arm64.
- Rename the Marketplace extension identity to `beatricebm.dcmview`.
- Document supported Marketplace platforms and the `dcmview.binaryPath`
  fallback for unsupported systems.

## 0.2.0

- Add initial VSIX packaging with bundled Linux x64, macOS x64, and macOS arm64
  binaries.
- Add VS Code commands for opening files, folders, and workspaces in `dcmview`.
- Add integrated terminal interception for `dcmview` and `dcmview-py` commands.
