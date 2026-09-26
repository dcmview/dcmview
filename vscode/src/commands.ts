import * as vscode from 'vscode';
import { resolveBinaryPath } from './binary';
import { formatError, getOutputChannel } from './output';
import { readSettings } from './settings';
import { startPathSession } from './viewerSessions';

export async function openPathCommand(
  context: vscode.ExtensionContext,
  uri?: vscode.Uri,
  selectedUris?: vscode.Uri[],
): Promise<void> {
  const uris = selectedUris && selectedUris.length > 0 ? selectedUris : uri ? [uri] : [];
  if (uris.length > 0) {
    await openUris(context, uris);
    return;
  }

  const picked = await vscode.window.showOpenDialog({
    canSelectFiles: true,
    canSelectFolders: true,
    canSelectMany: true,
    openLabel: 'Open with dcmview',
  });
  if (picked && picked.length > 0) {
    await openUris(context, picked);
  }
}

export async function openWorkspaceSelectionCommand(context: vscode.ExtensionContext): Promise<void> {
  const workspaceFolders = vscode.workspace.workspaceFolders ?? [];
  if (workspaceFolders.length === 0) {
    vscode.window.showWarningMessage('Open a workspace folder before launching dcmview.');
    return;
  }

  if (workspaceFolders.length === 1) {
    await openUris(context, [workspaceFolders[0].uri]);
    return;
  }

  const picked = await vscode.window.showQuickPick(
    workspaceFolders.map((folder) => ({
      label: folder.name,
      description: folder.uri.fsPath,
      uri: folder.uri,
    })),
    { placeHolder: 'Select a workspace folder to open with dcmview' },
  );
  if (picked) {
    await openUris(context, [picked.uri]);
  }
}

async function openUris(context: vscode.ExtensionContext, uris: readonly vscode.Uri[]): Promise<void> {
  const filePaths = collectFileSystemPaths(uris);
  if (filePaths.length === 0) {
    vscode.window.showErrorMessage('dcmview can only open file-system paths.');
    return;
  }

  const settings = readSettings();
  const output = getOutputChannel();

  try {
    const binary = await resolveBinaryPath(context.extensionUri.fsPath, settings.binaryPath);
    await startPathSession(context, binary, filePaths, settings, output);
  } catch (error) {
    output.appendLine(formatError(error));
    vscode.window.showErrorMessage(formatError(error));
  }
}

export function collectFileSystemPaths(uris: readonly vscode.Uri[]): string[] {
  return uris.filter((uri) => uri.scheme === 'file').map((uri) => uri.fsPath);
}
