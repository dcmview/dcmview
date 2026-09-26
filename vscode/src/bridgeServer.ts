import * as crypto from 'crypto';
import * as fs from 'fs';
import * as http from 'http';
import * as path from 'path';
import * as vscode from 'vscode';
import { resolveBinaryPath } from './binary';
import {
  bridgeRegistryDirectory,
  lastBridgeRegistryPublishTime,
  publishBridgeRegistry,
  unpublishBridgeRegistry,
} from './bridgeRegistry';
import { formatError, getOutputChannel } from './output';
import { ExtensionSettings, readSettings } from './settings';
import {
  activeSessionCount,
  findSession,
  startSession,
  stopSession,
} from './viewerSessions';

export interface BridgeServer {
  readonly id: string;
  readonly server: http.Server;
  readonly url: string;
  readonly token: string;
  registryPath?: string;
}

interface BridgeLaunchRequest {
  program?: string;
  args?: string[];
  cwd?: string;
  wait?: boolean;
  binaryPath?: string;
}

interface BridgeLaunchResponse {
  sessionId: string;
  url: string;
  exitCode?: number;
}

let bridgeServer: BridgeServer | undefined;

let binaryResolutionWarningShown = false;

export async function ensureBridge(context: vscode.ExtensionContext): Promise<BridgeServer> {
  if (bridgeServer) {
    return bridgeServer;
  }

  const token = crypto.randomBytes(24).toString('hex');
  const server = http.createServer((request, response) => {
    void handleBridgeRequest(context, token, request, response);
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });

  const address = server.address();
  if (!address || typeof address === 'string') {
    server.close();
    throw new Error('Unable to start dcmview VS Code bridge.');
  }

  bridgeServer = {
    id: crypto.randomUUID(),
    server,
    token,
    url: `http://127.0.0.1:${address.port}`,
  };
  return bridgeServer;
}

export async function stopBridge(): Promise<void> {
  const bridge = bridgeServer;
  bridgeServer = undefined;
  if (!bridge) {
    return;
  }

  await unpublishBridgeRegistry(bridge);

  await new Promise<void>((resolve) => {
    bridge.server.close(() => resolve());
  });
}

export async function republishBridgeRegistryIfAvailable(): Promise<void> {
  if (!bridgeServer) {
    return;
  }
  const registryDir = bridgeRegistryDirectory();
  try {
    await publishBridgeRegistry(bridgeServer, registryDir);
  } catch (error) {
    getOutputChannel().appendLine(`dcmview bridge registry publish failed: ${formatError(error)}`);
  }
}

export function showBridgeStatus(): void {
  const lastBridgeRegistryPublishMs = lastBridgeRegistryPublishTime();
  const output = getOutputChannel();
  if (!bridgeServer) {
    output.appendLine('dcmview bridge status: not running');
    output.show(true);
    return;
  }
  output.appendLine('dcmview bridge status: running');
  output.appendLine(`  url: ${bridgeServer.url}`);
  output.appendLine(`  registryPath: ${bridgeServer.registryPath ?? '(not published)'}`);
  output.appendLine(
    `  lastPublish: ${
      lastBridgeRegistryPublishMs ? new Date(lastBridgeRegistryPublishMs).toISOString() : '(never)'
    }`,
  );
  output.appendLine(`  activeSessions: ${activeSessionCount()}`);
  output.show(true);
}

async function handleBridgeRequest(
  context: vscode.ExtensionContext,
  token: string,
  request: http.IncomingMessage,
  response: http.ServerResponse,
): Promise<void> {
  try {
    if (!isAuthorizedBridgeRequest(request, token)) {
      writeJson(response, 401, { error: 'unauthorized' });
      return;
    }

    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    if (request.method === 'POST' && url.pathname === '/launch') {
      const launchRequest = await readJsonBody<BridgeLaunchRequest>(request);
      const launchResponse = await launchFromBridge(context, launchRequest);
      writeJson(response, 200, launchResponse);
      return;
    }

    const stopMatch = /^\/sessions\/([^/]+)\/stop$/.exec(url.pathname);
    if (request.method === 'POST' && stopMatch) {
      const session = findSession(stopMatch[1]);
      if (!session) {
        writeJson(response, 404, { error: 'session not found' });
        return;
      }
      await stopSession(session);
      writeJson(response, 200, bridgeStopResponse());
      return;
    }

    const waitMatch = /^\/sessions\/([^/]+)\/wait$/.exec(url.pathname);
    if ((request.method === 'GET' || request.method === 'POST') && waitMatch) {
      const session = findSession(waitMatch[1]);
      if (!session) {
        writeJson(response, 404, { error: 'session not found' });
        return;
      }
      const exitCode = await session.exitCode;
      writeJson(response, 200, bridgeWaitResponse(exitCode));
      return;
    }

    writeJson(response, 404, { error: 'not found' });
  } catch (error) {
    if (error instanceof BridgeRequestError) {
      writeJson(response, error.statusCode, { error: error.message });
      return;
    }
    writeJson(response, 500, { error: formatError(error) });
  }
}

export function isAuthorizedBridgeRequest(
  request: Pick<http.IncomingMessage, 'headers'>,
  token: string,
): boolean {
  const auth = request.headers.authorization;
  if (auth === `Bearer ${token}`) {
    return true;
  }
  return request.headers['x-dcmview-token'] === token;
}

export function bridgeStopResponse(): { ok: true } {
  return { ok: true };
}

export function bridgeWaitResponse(exitCode: number): { exitCode: number } {
  return { exitCode };
}

async function launchFromBridge(
  context: vscode.ExtensionContext,
  request: BridgeLaunchRequest,
): Promise<BridgeLaunchResponse> {
  const settings = readSettings();
  const output = getOutputChannel();
  const binary = await resolveBridgeLaunchBinary(context, settings, request);
  const args = normalizeInterceptedArgs(request.args ?? []);
  const cwd = request.cwd && path.isAbsolute(request.cwd) ? request.cwd : firstWorkspacePath();
  const title = `dcmview: ${request.program ?? 'terminal'}`;
  const session = await startSession(context, binary, args, cwd, title, settings, output);
  const response: BridgeLaunchResponse = {
    sessionId: session.id,
    url: session.url,
  };
  if (request.wait) {
    response.exitCode = await session.exitCode;
  }
  return response;
}

class BridgeRequestError extends Error {
  constructor(
    readonly statusCode: number,
    message: string,
  ) {
    super(message);
  }
}

async function resolveBridgeLaunchBinary(
  context: vscode.ExtensionContext,
  settings: ExtensionSettings,
  request: BridgeLaunchRequest,
): Promise<string> {
  try {
    return await resolveBinaryPath(context.extensionUri.fsPath, settings.binaryPath);
  } catch (localError) {
    const clientBinary = request.binaryPath;
    if (clientBinary && (await clientBinaryPathIsTrusted(clientBinary))) {
      return clientBinary;
    }
    if (!binaryResolutionWarningShown) {
      binaryResolutionWarningShown = true;
      vscode.window.showWarningMessage(
        'dcmview bridge could not find a usable binary. Set dcmview.binaryPath or use a dcmview-py wrapper with a bundled binary.',
      );
    }
    const suffix = clientBinary ? `; rejected client binary ${clientBinary}` : '';
    throw new BridgeRequestError(
      422,
      `${formatError(localError)}${suffix}. Set dcmview.binaryPath or install a bundled dcmview binary.`,
    );
  }
}

export async function clientBinaryPathIsTrusted(filePath: string): Promise<boolean> {
  if (!path.isAbsolute(filePath)) {
    return false;
  }
  const baseName = path.basename(filePath).toLowerCase();
  if (baseName !== 'dcmview' && baseName !== 'dcmview.exe') {
    return false;
  }
  let stats: fs.Stats;
  try {
    stats = await fs.promises.stat(filePath);
  } catch {
    return false;
  }
  if (!stats.isFile()) {
    return false;
  }
  if (process.platform === 'win32' || typeof process.getuid !== 'function') {
    return true;
  }
  return stats.uid === process.getuid() && (stats.mode & 0o022) === 0;
}

function firstWorkspacePath(): string {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? process.cwd();
}

export function normalizeInterceptedArgs(args: readonly string[]): string[] {
  const normalized = [...args];
  if (!hasFlag(normalized, '--no-browser')) {
    normalized.unshift('--no-browser');
  }
  if (!hasFlag(normalized, '--startup-json')) {
    normalized.unshift('--startup-json');
  }
  if (!hasOptionValue(normalized, ['--host'])) {
    normalized.unshift('127.0.0.1');
    normalized.unshift('--host');
  }
  if (!hasOptionValue(normalized, ['--port', '-p'])) {
    normalized.unshift('0');
    normalized.unshift('--port');
  }
  return normalized;
}

function hasFlag(args: readonly string[], flag: string): boolean {
  return args.includes(flag);
}

function hasOptionValue(args: readonly string[], options: readonly string[]): boolean {
  for (const option of options) {
    if (args.includes(option)) {
      return true;
    }
    if (args.some((arg) => arg.startsWith(`${option}=`))) {
      return true;
    }
  }
  return false;
}

async function readJsonBody<T>(request: http.IncomingMessage): Promise<T> {
  const chunks: Buffer[] = [];
  let totalBytes = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    totalBytes += buffer.byteLength;
    if (totalBytes > 1024 * 1024) {
      throw new Error('Bridge request body is too large.');
    }
    chunks.push(buffer);
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8')) as T;
}

function writeJson(response: http.ServerResponse, statusCode: number, body: unknown): void {
  const payload = JSON.stringify(body);
  response.writeHead(statusCode, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(payload),
  });
  response.end(payload);
}
