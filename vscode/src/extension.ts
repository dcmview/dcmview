import * as vscode from 'vscode';
import { stopBridgeRegistryRefresh } from './bridgeRegistry';
import { republishBridgeRegistryIfAvailable, showBridgeStatus, stopBridge } from './bridgeServer';
import { openPathCommand, openWorkspaceSelectionCommand } from './commands';
import { DICOM_CUSTOM_EDITOR_VIEW_TYPE, DicomCustomEditorProvider } from './customEditor';
import { initOutputChannel } from './output';
import { configureTerminalInterception } from './terminalInterception';
import { stopAllSessions } from './viewerSessions';

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(initOutputChannel());

  context.subscriptions.push(
    vscode.commands.registerCommand(
      'dcmview.openPath',
      (uri?: vscode.Uri, selectedUris?: vscode.Uri[]) => openPathCommand(context, uri, selectedUris),
    ),
    vscode.commands.registerCommand('dcmview.openWorkspaceSelection', () =>
      openWorkspaceSelectionCommand(context),
    ),
    vscode.commands.registerCommand('dcmview.stopAll', async () => {
      await stopAllSessions();
      vscode.window.showInformationMessage('Stopped all dcmview sessions.');
    }),
    vscode.commands.registerCommand('dcmview.showBridgeStatus', () => showBridgeStatus()),
    vscode.window.registerCustomEditorProvider(
      DICOM_CUSTOM_EDITOR_VIEW_TYPE,
      new DicomCustomEditorProvider(context),
    ),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('dcmview')) {
        void configureTerminalInterception(context);
      }
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      void configureTerminalInterception(context);
    }),
    vscode.window.onDidOpenTerminal(() => {
      void republishBridgeRegistryIfAvailable();
    }),
    { dispose: () => stopBridgeRegistryRefresh() },
    { dispose: () => void stopBridge() },
  );

  void configureTerminalInterception(context);
}

export async function deactivate(): Promise<void> {
  await stopBridge();
  await stopAllSessions();
}
