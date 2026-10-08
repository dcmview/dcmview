# Changelog

## Unreleased

## 0.4.0 - 2026-10-08

- The bundled viewer now requires an access token on every API request. The
  extension passes it to the viewer panel, so opening files and folders works
  as before. Scripts that call the viewer's HTTP API directly must send
  `Authorization: Bearer <token>`; see the main changelog.
- Preserve the access token across VS Code port forwarding by forwarding
  `base_url` and attaching `token` afterwards, so Remote-SSH sessions keep
  working. Older binaries still use `url`. Bridge clients receive the
  token-bearing launch URL, startup credentials are omitted from extension
  output, and socket-only startup events fail clearly.
- Viewer shortcuts no longer fire while Ctrl, Cmd or Alt is held, so Ctrl+Z
  and Ctrl+R reach VS Code instead of selecting the Zoom or ROI tool.
- Export ROIs reports a failed export in the viewer.
- A redaction box change can no longer leave a frame cached without the new
  box.
- `--unix-socket` is not supported in `dcmview.extraArgs`: the extension
  needs an HTTP viewer URL.
- `DCMVIEW_TOKEN` set only in a terminal or notebook is not passed through
  the bridge. A viewer the extension manages inherits the extension host's
  environment and otherwise generates its own token.

## 0.3.2 - 2026-10-02

- Adding `--mask` to `dcmview.extraArgs` starts masked sessions that replace
  patient identifiers in everything the viewer displays, for a shared or
  recorded screen: numbered pseudonyms, shifted dates, hashed UIDs, and
  `[masked]` identifiers. This is a display aid, not de-identification.
- A masked viewer panel is titled "dcmview: masked session" instead of the
  file name. VS Code's Explorer, the tab of a file opened in the DICOM custom
  editor, and the viewer's Directory view still show real folder and file
  names.
- A Redact tool (`X`) draws redaction boxes over burned-in pixel text, with
  or without masking. Boxes last for the session and are not saved.

## 0.3.1 - 2026-10-01

- Graphic and text annotations of Grayscale and Color Softcopy Presentation
  States are drawn on the images they reference. An Annotations bar offers
  the states that annotate the open image, one at a time and off until
  chosen; shapes follow zoom, pan, flips, and rotation.
- Previous/next controls, and `,` / `.`, step through the shown state's
  annotation items, highlighting the current one and opening the image and
  frame it references.
- Only the annotations are applied: the state's window, shutter, displayed
  area, and rotation or flip are not, and objects in DISPLAY units are
  counted but not drawn.

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
