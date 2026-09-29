# Changelog

## Unreleased

## 0.3.0 - 2026-09-29

- Terminal commands and Python calls route into VS Code when they carry its
  bridge environment or run inside a registered workspace folder; otherwise
  they open locally. Python and terminal shims share the binary's routing rule,
  including notebook kernels that only see the bridge registry.
- Slow viewer launches wait up to 120 seconds. An unconfirmed launch exits
  instead of opening a second local viewer, and stale bridge entries are
  probed before launch so a reused port does not block opening an image.
- The Bea · dcmview restyle adds outlined controls, line icons, clearer
  hierarchy and status indicators, and bundled Inter and JetBrains Mono fonts
  for consistent offline rendering.
- The cursor readout reports stored, Modality, and real-world values. RT Dose
  and Parametric Map colorwash overlays provide opacity controls, units, and
  overlaid values; W/L can operate in mapped units. SEG recommended colors,
  WSI companion navigation, and expanded RT Dose context are also included.
- FRACTIONAL SEGs whose maximum exceeds 1 but whose complete stored samples
  are all 0 or 1 draw as binary masks, with a Semantic Context warning.
- Viewer assets and API requests use page-relative URLs for forwarded paths;
  responses include `X-Content-Type-Options: nosniff`.
- A visible image-position scrubber supports keyboard and mouse seeking at
  narrow webview widths too. W/L cine uses the current window and returns to
  interactive windowing when paused; cached tabs resume playback correctly.
- Frame images, colorwash, presentation layers, mappings, and labels now switch
  together without black flashes. Layer errors keep the image visible with a
  note, and float32/float64 file transitions retain the correct mapping.
- Presets replace preceding W/L drags, manual drags leave Full Dynamic, and
  sub-unit non-integer windows retain their contrast. The applied window drives
  both the HUD and legend; ROI outlines, handles, and labels keep their screen
  size through zoom and orientation changes.
- Retry recovers failed resources or reloads a replaced server's catalog.
  References refresh during discovery, closing a tab discards its view state,
  and tag buttons support Enter/Space without starting cine.
- The bundled API adds `X-Frame-Window-Applied` for grayscale display frames and
  `X-Server-Instance` on every API response, allowing the viewer to identify the
  applied presentation and detect a replacement server before using its data.
- MONOCHROME1 padding stays black, stop signals work immediately after the URL
  appears, and corrupt encapsulated fragment lengths fail before allocation.
- Removed `--tunnel`, `--tunnel-host`, and `--tunnel-port` and the corresponding
  Python arguments. Forward the remote loopback port with the printed `ssh -L`
  command from your local machine, or use the existing Remote-SSH workflow.
- The viewer follows the editor's colour theme: light themes open it light,
  dark and high-contrast themes dark, and switching themes updates open
  viewers without reloading them.
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
