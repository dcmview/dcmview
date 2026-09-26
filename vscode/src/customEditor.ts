import * as vscode from 'vscode';
import { resolveBinaryPath } from './binary';
import { formatError, getOutputChannel } from './output';
import { readSettings } from './settings';
import { startPathSessionInPanel } from './viewerSessions';

export const DICOM_CUSTOM_EDITOR_VIEW_TYPE = 'dcmview.dicomViewer';

class DicomCustomDocument implements vscode.CustomDocument {
  constructor(readonly uri: vscode.Uri) {}

  dispose(): void {
    // Session cleanup is tied to each custom editor webview panel.
  }
}

export class DicomCustomEditorProvider implements vscode.CustomReadonlyEditorProvider<DicomCustomDocument> {
  constructor(private readonly context: vscode.ExtensionContext) {}

  openCustomDocument(uri: vscode.Uri): DicomCustomDocument {
    if (uri.scheme !== 'file') {
      throw new Error('dcmview can only open file-system paths.');
    }
    return new DicomCustomDocument(uri);
  }

  async resolveCustomEditor(
    document: DicomCustomDocument,
    webviewPanel: vscode.WebviewPanel,
  ): Promise<void> {
    webviewPanel.webview.options = {
      enableScripts: true,
      localResourceRoots: [this.context.extensionUri],
    };
    const settings = readSettings();
    const output = getOutputChannel();
    const filePath = document.uri.fsPath;

    try {
      const binary = await resolveBinaryPath(this.context.extensionUri.fsPath, settings.binaryPath);
      await startPathSessionInPanel(binary, [filePath], settings, output, webviewPanel);
    } catch (error) {
      output.appendLine(formatError(error));
      throw error;
    }
  }
}
