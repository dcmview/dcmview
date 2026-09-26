import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';
import { resolveBinaryPath } from './binary';
import { bridgeRegistryDirectory, publishBridgeRegistry, startBridgeRegistryRefresh } from './bridgeRegistry';
import { ensureBridge, stopBridge } from './bridgeServer';
import { formatError, getOutputChannel } from './output';
import {
  BRIDGE_BYPASS_ENV,
  BRIDGE_REGISTRY_DIR_ENV,
  BRIDGE_TOKEN_ENV,
  BRIDGE_URL_ENV,
  readSettings,
} from './settings';

export async function configureTerminalInterception(context: vscode.ExtensionContext): Promise<void> {
  const settings = readSettings();
  const env = context.environmentVariableCollection;
  env.persistent = false;
  env.clear();

  if (!settings.terminalInterceptionEnabled) {
    await stopBridge();
    return;
  }

  const output = getOutputChannel();
  try {
    const bridge = await ensureBridge(context);
    const registryDir = bridgeRegistryDirectory();
    await publishBridgeRegistry(bridge, registryDir);
    startBridgeRegistryRefresh(bridge, registryDir);

    env.description = 'Routes dcmview terminal commands into the VS Code dcmview viewer.';
    env.replace(BRIDGE_REGISTRY_DIR_ENV, registryDir);
    env.replace(BRIDGE_URL_ENV, bridge.url);
    env.replace(BRIDGE_TOKEN_ENV, bridge.token);

    try {
      const binary = await resolveBinaryPath(context.extensionUri.fsPath, settings.binaryPath);
      const shimDir = await ensureShimDirectory(context, binary);
      env.prepend('PATH', `${shimDir}${path.delimiter}`);
      output.appendLine(`dcmview terminal interception enabled at ${bridge.url}`);
    } catch (error) {
      output.appendLine(
        `dcmview bridge enabled at ${bridge.url}; terminal shims disabled: ${formatError(error)}`,
      );
    }
  } catch (error) {
    env.clear();
    output.appendLine(`dcmview terminal interception disabled: ${formatError(error)}`);
  }
}

async function ensureShimDirectory(context: vscode.ExtensionContext, binary: string): Promise<string> {
  const shimDir = path.join(context.globalStorageUri.fsPath, 'terminal-shims');
  await fs.promises.mkdir(shimDir, { recursive: true });
  await Promise.all([
    writeShim(shimDir, 'dcmview', binary, 'dcmview'),
    writeShim(shimDir, 'dcmview-py', binary, 'dcmview-py'),
  ]);
  return shimDir;
}

async function writeShim(
  shimDir: string,
  name: string,
  binary: string,
  program: string,
): Promise<void> {
  if (process.platform === 'win32') {
    const filePath = path.join(shimDir, `${name}.cmd`);
    await fs.promises.writeFile(filePath, windowsShim(binary, program), 'utf8');
    return;
  }

  const filePath = path.join(shimDir, name);
  await fs.promises.writeFile(filePath, posixShim(binary, program), { encoding: 'utf8', mode: 0o755 });
  await fs.promises.chmod(filePath, 0o755);
}

export function posixShim(binary: string, program: string): string {
  return `#!/bin/sh
if [ "\${${BRIDGE_BYPASS_ENV}:-}" = "1" ]; then
  exec ${shellSingleQuote(binary)} "$@"
fi
exec ${shellSingleQuote(binary)} --vscode-bridge-client ${shellSingleQuote(program)} "$@"
`;
}

export function windowsShim(binary: string, program: string): string {
  return `@echo off\r
if "%${BRIDGE_BYPASS_ENV}%"=="1" (\r
  "${binary}" %*\r
  exit /b %ERRORLEVEL%\r
)\r
"${binary}" --vscode-bridge-client ${program} %*\r
exit /b %ERRORLEVEL%\r
`;
}

function shellSingleQuote(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`;
}
