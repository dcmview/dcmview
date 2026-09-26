import * as vscode from 'vscode';

export const BRIDGE_BYPASS_ENV = 'DCMVIEW_VSCODE_BYPASS';

export const BRIDGE_REGISTRY_DIR_ENV = 'DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR';

export const BRIDGE_TOKEN_ENV = 'DCMVIEW_VSCODE_BRIDGE_TOKEN';

export const BRIDGE_URL_ENV = 'DCMVIEW_VSCODE_BRIDGE_URL';

export interface ExtensionSettings {
  binaryPath: string;
  defaultRecursive: boolean;
  extraArgs: string[];
  startupTimeoutSeconds: number;
  terminalInterceptionEnabled: boolean;
}

export function readSettings(): ExtensionSettings {
  const config = vscode.workspace.getConfiguration('dcmview');
  return {
    binaryPath: config.get('binaryPath', '').trim(),
    defaultRecursive: config.get('defaultRecursive', true),
    extraArgs: config.get('extraArgs', []),
    startupTimeoutSeconds: config.get('startupTimeoutSeconds', 20),
    terminalInterceptionEnabled: config.get('terminalInterception.enabled', true),
  };
}
