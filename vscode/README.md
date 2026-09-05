![dcmview](https://raw.githubusercontent.com/dcmview/dcmview/main/dcmview-wordmark-darkmode-opaque-background.png)

# dcmview

Open local DICOM files and folders in `dcmview` directly from VS Code or Cursor.

`dcmview` is a fast, temporary DICOM inspection tool for research and
development workflows. The extension starts a local loopback `dcmview` server
from the selected file or folder and displays the viewer in an editor webview.

`dcmview` is intended for developer and research inspection on secure networks,
not clinical diagnosis. Avoid public-facing server binds; the extension launches
the bundled server on loopback and displays it in an editor webview.

Do not include PHI or sensitive DICOM content in public issue reports. Report
security issues privately to the maintainers before public disclosure.

For common install, binary resolution, VS Code interception, startup, tunnel,
and annotation CSV problems, see the main
[troubleshooting guide](https://github.com/dcmview/dcmview/blob/main/docs/troubleshooting.md). For settings, binary
resolution order, and bridge environment variables, see the
[configuration reference](https://github.com/dcmview/dcmview/blob/main/docs/configuration.md). For Python scripts and
notebooks that may route through the VS Code bridge, see the
[Python reference](https://github.com/dcmview/dcmview/blob/main/docs/python.md). The main
[documentation index](https://github.com/dcmview/dcmview/blob/main/docs/index.md) links the user, configuration,
troubleshooting, API/debugging, development, and release references.

<!-- dcmview-marketing:start -->
## In VS Code

![Open DICOM data with dcmview from VS Code Explorer](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/vscode/media/marketing/vscode-workflow.gif)

![DICOM cine playback in dcmview](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/vscode/media/marketing/chest-ct-cine.gif)

[Source imagery attribution](https://raw.githubusercontent.com/dcmview/dcmview/v0.2.12/vscode/media/marketing/ATTRIBUTION.md)
<!-- dcmview-marketing:end -->

## Supported Platforms

Marketplace builds currently bundle `dcmview` binaries for:

- Linux x64
- macOS x64
- macOS arm64
- Windows x64

On unsupported platforms, or when you need to test a locally built binary, set
`dcmview.binaryPath` to an absolute path to a compatible `dcmview` executable.

## Installation

- In VS Code, install `beatricebm.dcmview` from the VS Code Marketplace.
- In Cursor, install `beatricebm.dcmview` from Cursor's extension panel. Cursor
  obtains the extension from Open VSX after its marketplace security review.
- For local testing, download the target-specific VSIX for your platform from a
  tagged GitHub Release and use `Extensions: Install from VSIX...`.

## Usage

Use the Explorer context menu command `Open with dcmview` on DICOM files or
folders. The extension launches `dcmview --no-browser --port 0`, waits for the
local server URL, and opens the viewer beside your current editor.

For files named `*.dcm`, `*.dicom`, or `*.ima`, use the editor's
`Reopen With...` command and choose `dcmview` to open the file in a readonly
dcmview editor tab.
Set `dcmview` as the default editor for those patterns if you want double-clicks
to open matching DICOM files directly in dcmview. Extensionless DICOM files and
folders should still use the Explorer context menu command.

The command `dcmview: Open Workspace with dcmview` opens a selected workspace
folder. The command `dcmview: Stop All dcmview Sessions` terminates extension
managed viewer sessions.

When `dcmview.terminalInterception.enabled` is true, new integrated terminals
route `dcmview`, `dcmview-py`, and `python -m dcmview_py` invocations into editor
webview panels. Set `DCMVIEW_VSCODE_BYPASS=1` in a terminal to bypass that
integration for a single shell session.

## Settings

- `dcmview.binaryPath`: absolute path to a `dcmview` binary override.
- `dcmview.defaultRecursive`: recursively scan selected folders by default.
- `dcmview.extraArgs`: additional command-line arguments passed to `dcmview`.
- `dcmview.startupTimeoutSeconds`: seconds to wait for startup.
- `dcmview.terminalInterception.enabled`: route integrated terminal launches
  into editor webviews.

See the main [configuration reference](https://github.com/dcmview/dcmview/blob/main/docs/configuration.md) for binary
resolution order, environment variables, and how these settings map to
`dcmview` CLI arguments.
