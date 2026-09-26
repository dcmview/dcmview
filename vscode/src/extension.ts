import * as childProcess from 'child_process';
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as http from 'http';
import * as os from 'os';
import * as path from 'path';
import * as vscode from 'vscode';

const STARTUP_PREFIX = 'dcmview: server running at ';
const STARTUP_EVENT_TYPE = 'server_started';
export const DICOM_CUSTOM_EDITOR_VIEW_TYPE = 'dcmview.dicomViewer';
const BRIDGE_BYPASS_ENV = 'DCMVIEW_VSCODE_BYPASS';
const BRIDGE_REGISTRY_DIR_ENV = 'DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR';
const BRIDGE_TOKEN_ENV = 'DCMVIEW_VSCODE_BRIDGE_TOKEN';
const BRIDGE_URL_ENV = 'DCMVIEW_VSCODE_BRIDGE_URL';
export const BRIDGE_REGISTRY_MAX_AGE_MS = 3 * 60 * 60 * 1000;
export const BRIDGE_REGISTRY_REFRESH_MS = 60 * 60 * 1000;
export const BRIDGE_REGISTRY_PRESENCE_CHECK_MS = 60 * 1000;

interface StartupEvent {
  type: string;
  url: string;
  host: string;
  port: number;
}

interface ExtensionSettings {
  binaryPath: string;
  defaultRecursive: boolean;
  extraArgs: string[];
  startupTimeoutSeconds: number;
  terminalInterceptionEnabled: boolean;
}

interface RunningSession {
  readonly id: string;
  readonly panel: vscode.WebviewPanel;
  readonly process: childProcess.ChildProcessWithoutNullStreams;
  readonly output: vscode.OutputChannel;
  readonly name: string;
  readonly url: string;
  readonly exitCode: Promise<number>;
  stopped: boolean;
}

class DicomCustomDocument implements vscode.CustomDocument {
  constructor(readonly uri: vscode.Uri) {}

  dispose(): void {
    // Session cleanup is tied to each custom editor webview panel.
  }
}

class DicomCustomEditorProvider implements vscode.CustomReadonlyEditorProvider<DicomCustomDocument> {
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

interface BridgeServer {
  readonly id: string;
  readonly server: http.Server;
  readonly url: string;
  readonly token: string;
  registryPath?: string;
}

interface WritableBridgeRegistry {
  readonly id: string;
  readonly url: string;
  readonly token: string;
  registryPath?: string;
}

interface BridgeRegistryEntry {
  version: 1;
  instanceId: string;
  bridgeUrl: string;
  token: string;
  workspaceRoots: string[];
  createdAtMs: number;
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

const sessions = new Set<RunningSession>();
const sessionsById = new Map<string, RunningSession>();
let outputChannel: vscode.OutputChannel | undefined;
let bridgeServer: BridgeServer | undefined;
let bridgeRefreshTimer: NodeJS.Timeout | undefined;
let bridgePresenceTimer: NodeJS.Timeout | undefined;
let lastBridgeRegistryPublishMs: number | undefined;
let binaryResolutionWarningShown = false;

export function activate(context: vscode.ExtensionContext): void {
  outputChannel = vscode.window.createOutputChannel('dcmview');
  context.subscriptions.push(outputChannel);

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

async function openPathCommand(
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

async function openWorkspaceSelectionCommand(context: vscode.ExtensionContext): Promise<void> {
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

function readSettings(): ExtensionSettings {
  const config = vscode.workspace.getConfiguration('dcmview');
  return {
    binaryPath: config.get('binaryPath', '').trim(),
    defaultRecursive: config.get('defaultRecursive', true),
    extraArgs: config.get('extraArgs', []),
    startupTimeoutSeconds: config.get('startupTimeoutSeconds', 20),
    terminalInterceptionEnabled: config.get('terminalInterception.enabled', true),
  };
}

async function resolveBinaryPath(extensionRoot: string, configuredBinary: string): Promise<string> {
  const candidates = binaryCandidates(extensionRoot, configuredBinary);
  for (const candidate of candidates) {
    if (candidate.kind === 'path' && (await isExecutableFile(candidate.value))) {
      return candidate.value;
    }
    if (candidate.kind === 'path-name' && (await findOnPath(candidate.value))) {
      return candidate.value;
    }
  }

  throw new Error(
    'Unable to find dcmview. Set dcmview.binaryPath, run cargo build, install a VSIX with a bundled binary for this platform, or install dcmview on PATH.',
  );
}

export function binaryCandidates(
  extensionRoot: string,
  configuredBinary: string,
  platform: NodeJS.Platform = process.platform,
  arch: string = process.arch,
): Array<{ kind: 'path' | 'path-name'; value: string }> {
  const executable = platform === 'win32' ? 'dcmview.exe' : 'dcmview';
  const candidates: Array<{ kind: 'path' | 'path-name'; value: string }> = [];
  if (configuredBinary.length > 0) {
    candidates.push({ kind: 'path', value: configuredBinary });
  }
  candidates.push(
    { kind: 'path', value: path.resolve(extensionRoot, '..', 'target', 'debug', executable) },
    {
      kind: 'path',
      value: path.resolve(
        extensionRoot,
        'resources',
        'bin',
        `${platform}-${arch}`,
        executable,
      ),
    },
    { kind: 'path-name', value: executable },
  );
  return candidates;
}

async function isExecutableFile(filePath: string): Promise<boolean> {
  try {
    const stats = await fs.promises.stat(filePath);
    return stats.isFile();
  } catch {
    return false;
  }
}

async function findOnPath(executable: string): Promise<boolean> {
  const pathValue = process.env.PATH ?? '';
  const pathExt = process.platform === 'win32' ? (process.env.PATHEXT ?? '.EXE').split(';') : [''];
  for (const dir of pathValue.split(path.delimiter)) {
    for (const ext of pathExt) {
      const candidate = path.join(dir, executable.endsWith(ext.toLowerCase()) ? executable : `${executable}${ext}`);
      if (await isExecutableFile(candidate)) {
        return true;
      }
    }
  }
  return false;
}

async function startSession(
  context: vscode.ExtensionContext,
  binary: string,
  args: readonly string[],
  cwd: string,
  title: string,
  settings: ExtensionSettings,
  output: vscode.OutputChannel,
): Promise<RunningSession> {
  const panel = vscode.window.createWebviewPanel('dcmview.viewer', title, vscode.ViewColumn.Beside, {
    enableScripts: true,
    localResourceRoots: [context.extensionUri],
  });
  try {
    return await startSessionInPanel(binary, args, cwd, title, settings, output, panel);
  } catch (error) {
    panel.dispose();
    throw error;
  }
}

async function startSessionInPanel(
  binary: string,
  args: readonly string[],
  cwd: string,
  title: string,
  settings: ExtensionSettings,
  output: vscode.OutputChannel,
  panel: vscode.WebviewPanel,
): Promise<RunningSession> {
  output.appendLine(`Launching: ${binary} ${args.map(shellQuote).join(' ')}`);

  const child = childProcess.spawn(binary, args, {
    cwd,
    env: { ...process.env, [BRIDGE_BYPASS_ENV]: '1' },
  });
  let session: RunningSession | undefined;
  let panelDisposed = false;
  const panelDisposeListener = panel.onDidDispose(() => {
    if (session) {
      void stopSession(session, false);
      return;
    }
    panelDisposed = true;
    child.kill('SIGINT');
  });

  let serverUrl: string;
  try {
    serverUrl = await waitForStartupOrTerminate(
      child,
      settings.startupTimeoutSeconds * 1000,
      output,
    );
  } catch (error) {
    panelDisposeListener.dispose();
    throw error;
  }

  try {
    if (panelDisposed) {
      throw new Error('dcmview panel closed before startup completed.');
    }
    const externalUri = await vscode.env.asExternalUri(vscode.Uri.parse(serverUrl));
    if (panelDisposed) {
      throw new Error('dcmview panel closed before startup completed.');
    }
    panel.webview.html = webviewHtml(panel.webview, externalUri);
  } catch (error) {
    panelDisposeListener.dispose();
    child.kill('SIGINT');
    throw error;
  }

  let resolveExitCode: (exitCode: number) => void;
  const exitCode = new Promise<number>((resolve) => {
    resolveExitCode = resolve;
  });
  session = {
    id: crypto.randomUUID(),
    panel,
    process: child,
    output,
    name: title,
    url: serverUrl,
    exitCode,
    stopped: false,
  };

  child.once('exit', (code, signal) => {
    output.appendLine(`dcmview exited (${title}): code=${code ?? 'null'} signal=${signal ?? 'null'}`);
    panelDisposeListener.dispose();
    sessions.delete(session);
    sessionsById.delete(session.id);
    resolveExitCode(Number(code ?? 0));
    if (!session.stopped) {
      panel.dispose();
    }
  });

  sessions.add(session);
  sessionsById.set(session.id, session);
  return session;
}

async function startPathSession(
  context: vscode.ExtensionContext,
  binary: string,
  filePaths: readonly string[],
  settings: ExtensionSettings,
  output: vscode.OutputChannel,
): Promise<RunningSession> {
  return startSession(
    context,
    binary,
    buildDcmviewArgs(filePaths, settings),
    commonWorkingDirectory(filePaths),
    sessionTitle(filePaths),
    settings,
    output,
  );
}

async function startPathSessionInPanel(
  binary: string,
  filePaths: readonly string[],
  settings: ExtensionSettings,
  output: vscode.OutputChannel,
  panel: vscode.WebviewPanel,
): Promise<RunningSession> {
  return startSessionInPanel(
    binary,
    buildDcmviewArgs(filePaths, settings),
    commonWorkingDirectory(filePaths),
    sessionTitle(filePaths),
    settings,
    output,
    panel,
  );
}

function sessionTitle(filePaths: readonly string[]): string {
  return `dcmview: ${path.basename(filePaths[0])}${filePaths.length > 1 ? ` +${filePaths.length - 1}` : ''}`;
}

function buildDcmviewArgs(filePaths: readonly string[], settings: ExtensionSettings): string[] {
  const args = ['--no-browser', '--port', '0', '--host', '127.0.0.1', '--startup-json'];
  if (!settings.defaultRecursive) {
    args.push('--no-recursive');
  }
  args.push(...settings.extraArgs, ...filePaths);
  return args;
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

function commonWorkingDirectory(filePaths: readonly string[]): string {
  const first = filePaths[0];
  try {
    const stats = fs.statSync(first);
    return stats.isDirectory() ? first : path.dirname(first);
  } catch {
    return path.dirname(first);
  }
}

export async function waitForStartupOrTerminate(
  child: Pick<childProcess.ChildProcessWithoutNullStreams, 'stdout' | 'stderr' | 'once' | 'kill'>,
  timeoutMs: number,
  output: Pick<vscode.OutputChannel, 'append' | 'appendLine'>,
): Promise<string> {
  try {
    return await waitForStartup(child, timeoutMs, output);
  } catch (error) {
    child.kill('SIGINT');
    throw error;
  }
}

export function waitForStartup(
  child: Pick<childProcess.ChildProcessWithoutNullStreams, 'stdout' | 'stderr' | 'once'>,
  timeoutMs: number,
  output: Pick<vscode.OutputChannel, 'append' | 'appendLine'>,
): Promise<string> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let stdoutBuffer = '';
    const recentLines: string[] = [];
    const timer = setTimeout(() => {
      fail(new Error(`Timed out waiting for dcmview startup after ${timeoutMs / 1000}s.`));
    }, timeoutMs);

    const fail = (error: Error) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      reject(error);
    };

    child.stdout.on('data', (chunk: Buffer) => {
      stdoutBuffer += chunk.toString('utf8');
      const lines = stdoutBuffer.split(/\r?\n/);
      stdoutBuffer = lines.pop() ?? '';
      for (const line of lines) {
        output.appendLine(line);
        recentLines.push(line);
        recentLines.splice(0, Math.max(0, recentLines.length - 20));
        const parsed = parseStartupLine(line);
        if (parsed && !settled) {
          settled = true;
          clearTimeout(timer);
          resolve(parsed);
        }
      }
    });

    child.stderr.on('data', (chunk: Buffer) => {
      output.append(chunk.toString('utf8'));
    });

    child.once('error', (error) => {
      fail(error);
    });

    child.once('exit', (code, signal) => {
      if (!settled) {
        fail(
          new Error(
            `dcmview exited before startup (code=${code ?? 'null'}, signal=${signal ?? 'null'}).\n${recentLines.join('\n')}`,
          ),
        );
      }
    });
  });
}

export function parseStartupLine(line: string): string | undefined {
  const trimmed = line.trim();
  if (trimmed.startsWith('{')) {
    try {
      const event = JSON.parse(trimmed) as Partial<StartupEvent>;
      if (event.type === STARTUP_EVENT_TYPE && typeof event.url === 'string') {
        return event.url;
      }
    } catch {
      return undefined;
    }
  }

  if (trimmed.startsWith(STARTUP_PREFIX)) {
    return trimmed.slice(STARTUP_PREFIX.length);
  }
  return undefined;
}

function webviewHtml(webview: vscode.Webview, externalUri: vscode.Uri): string {
  const iframeSrc = escapeHtml(externalUri.toString());
  const frameOrigin = escapeHtml(`${externalUri.scheme}://${externalUri.authority}`);
  const csp = [
    "default-src 'none'",
    `frame-src ${frameOrigin}`,
    `img-src ${webview.cspSource} https: data:`,
    "style-src 'unsafe-inline'",
  ].join('; ');

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="${csp}">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>dcmview</title>
  <style>
    html, body, iframe {
      width: 100%;
      height: 100%;
      margin: 0;
      padding: 0;
      border: 0;
      background: #1a1a1a;
      overflow: hidden;
    }
  </style>
</head>
<body>
  <iframe src="${iframeSrc}" title="dcmview"></iframe>
</body>
</html>`;
}

async function stopAllSessions(): Promise<void> {
  await Promise.all(Array.from(sessions).map((session) => stopSession(session)));
}

async function stopSession(session: RunningSession, disposePanel = true): Promise<void> {
  if (session.stopped) {
    return;
  }
  session.stopped = true;
  sessions.delete(session);
  await terminateProcess(session.process, session.output, session.name);
  if (disposePanel) {
    session.panel.dispose();
  }
}

async function terminateProcess(
  child: childProcess.ChildProcessWithoutNullStreams,
  output: vscode.OutputChannel,
  name: string,
): Promise<void> {
  if (child.exitCode !== null || child.killed) {
    return;
  }

  output.appendLine(`Stopping ${name}`);
  child.kill('SIGINT');

  await new Promise<void>((resolve) => {
    const timer = setTimeout(() => {
      if (child.exitCode === null && !child.killed) {
        child.kill('SIGTERM');
      }
      resolve();
    }, 2500);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

function getOutputChannel(): vscode.OutputChannel {
  if (!outputChannel) {
    outputChannel = vscode.window.createOutputChannel('dcmview');
  }
  return outputChannel;
}

async function configureTerminalInterception(context: vscode.ExtensionContext): Promise<void> {
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

async function ensureBridge(context: vscode.ExtensionContext): Promise<BridgeServer> {
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

async function stopBridge(): Promise<void> {
  const bridge = bridgeServer;
  bridgeServer = undefined;
  if (!bridge) {
    return;
  }

  stopBridgeRegistryRefresh();
  if (bridge.registryPath) {
    await fs.promises.unlink(bridge.registryPath).catch(() => undefined);
  }
  lastBridgeRegistryPublishMs = undefined;

  await new Promise<void>((resolve) => {
    bridge.server.close(() => resolve());
  });
}

function startBridgeRegistryRefresh(bridge: BridgeServer, registryDir: string): void {
  stopBridgeRegistryRefresh();
  bridgeRefreshTimer = setInterval(() => {
    void publishBridgeRegistry(bridge, registryDir).catch((error) => {
      getOutputChannel().appendLine(`dcmview bridge registry refresh failed: ${formatError(error)}`);
    });
  }, BRIDGE_REGISTRY_REFRESH_MS);
  bridgeRefreshTimer.unref?.();
  bridgePresenceTimer = setInterval(() => {
    void ensureBridgeRegistryPresent(bridge, registryDir).catch((error) => {
      getOutputChannel().appendLine(`dcmview bridge registry presence check failed: ${formatError(error)}`);
    });
  }, BRIDGE_REGISTRY_PRESENCE_CHECK_MS);
  bridgePresenceTimer.unref?.();
}

function stopBridgeRegistryRefresh(): void {
  if (bridgeRefreshTimer) {
    clearInterval(bridgeRefreshTimer);
    bridgeRefreshTimer = undefined;
  }
  if (bridgePresenceTimer) {
    clearInterval(bridgePresenceTimer);
    bridgePresenceTimer = undefined;
  }
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
      const session = sessionsById.get(stopMatch[1]);
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
      const session = sessionsById.get(waitMatch[1]);
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

async function ensureShimDirectory(context: vscode.ExtensionContext, binary: string): Promise<string> {
  const shimDir = path.join(context.globalStorageUri.fsPath, 'terminal-shims');
  await fs.promises.mkdir(shimDir, { recursive: true });
  await Promise.all([
    writeShim(shimDir, 'dcmview', binary, 'dcmview'),
    writeShim(shimDir, 'dcmview-py', binary, 'dcmview-py'),
  ]);
  return shimDir;
}

export function bridgeRegistryDirectory(env: NodeJS.ProcessEnv = process.env): string {
  const configured = env[BRIDGE_REGISTRY_DIR_ENV];
  if (configured && configured.trim().length > 0) {
    return configured;
  }

  const stateHome = env.XDG_STATE_HOME;
  if (stateHome && path.isAbsolute(stateHome)) {
    return path.join(stateHome, 'dcmview', 'vscode-bridges');
  }

  const home = env.HOME;
  if (home && path.isAbsolute(home)) {
    return path.join(home, '.local', 'state', 'dcmview', 'vscode-bridges');
  }

  const userProfile = env.USERPROFILE;
  if (userProfile && path.isAbsolute(userProfile)) {
    return path.join(userProfile, '.local', 'state', 'dcmview', 'vscode-bridges');
  }

  if (env !== process.env) {
    return path.join('.', '.local', 'state', 'dcmview', 'vscode-bridges');
  }

  const fallbackHome = os.homedir();
  const homePath = fallbackHome && path.isAbsolute(fallbackHome) ? fallbackHome : '.';
  return path.join(homePath, '.local', 'state', 'dcmview', 'vscode-bridges');
}

export function isExpiredRegistryEntry(createdAtMs: number, nowMs: number): boolean {
  return (
    createdAtMs <= 0 ||
    createdAtMs > nowMs + BRIDGE_REGISTRY_MAX_AGE_MS ||
    nowMs - createdAtMs > BRIDGE_REGISTRY_MAX_AGE_MS
  );
}

export function orderBridgeRegistryEndpoints(
  cwd: string,
  entries: BridgeRegistryEntry[],
  requireWorkspace: boolean,
  nowMs = Date.now(),
): string[][] {
  const normalizedCwd = path.resolve(cwd);
  const candidates = entries
    .filter((entry) => !isExpiredRegistryEntry(entry.createdAtMs, nowMs))
    .map((entry) => ({
      score: workspaceMatchScore(normalizedCwd, entry.workspaceRoots),
      createdAtMs: entry.createdAtMs,
      endpoint: [entry.bridgeUrl, entry.token],
    }))
    .filter((candidate) => !requireWorkspace || candidate.score > 0)
    .sort((left, right) => right.score - left.score || right.createdAtMs - left.createdAtMs);

  const seen = new Set<string>();
  const endpoints: string[][] = [];
  for (const candidate of candidates) {
    const key = `${candidate.endpoint[0]}\0${candidate.endpoint[1]}`;
    if (!seen.has(key)) {
      seen.add(key);
      endpoints.push(candidate.endpoint);
    }
  }
  return endpoints;
}

function workspaceMatchScore(cwd: string, workspaceRoots: readonly string[]): number {
  let best = 0;
  for (const root of workspaceRoots) {
    const normalizedRoot = path.resolve(root);
    const relative = path.relative(normalizedRoot, cwd);
    if (relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative))) {
      best = Math.max(best, normalizedRoot.length);
    }
  }
  return best;
}

export async function writeBridgeRegistry(
  bridge: WritableBridgeRegistry,
  registryDir: string,
  roots: string[] = workspaceRoots(),
  createdAtMs = Date.now(),
): Promise<string> {
  await fs.promises.mkdir(registryDir, { recursive: true, mode: 0o700 });
  if (process.platform !== 'win32') {
    await fs.promises.chmod(registryDir, 0o700);
  }
  const stats = await fs.promises.stat(registryDir);
  if (!registryDirectoryIsTrusted(stats)) {
    throw new Error(`Refusing to publish dcmview bridge registry in untrusted directory: ${registryDir}`);
  }

  const registryPath = path.join(registryDir, `${bridge.id}.json`);
  const previousRegistryPath = bridge.registryPath;
  const tempPath = `${registryPath}.${process.pid}.tmp`;
  const payload = JSON.stringify(bridgeRegistryEntry(bridge, roots, createdAtMs), null, 2);
  await fs.promises.writeFile(tempPath, payload, { encoding: 'utf8', mode: 0o600 });
  if (process.platform !== 'win32') {
    await fs.promises.chmod(tempPath, 0o600);
  }
  await fs.promises.rename(tempPath, registryPath);
  bridge.registryPath = registryPath;
  if (previousRegistryPath && previousRegistryPath !== registryPath) {
    await fs.promises.unlink(previousRegistryPath).catch(() => undefined);
  }
  return registryPath;
}

async function publishBridgeRegistry(
  bridge: WritableBridgeRegistry,
  registryDir: string,
  roots: string[] = workspaceRoots(),
  createdAtMs = Date.now(),
): Promise<string> {
  const registryPath = await writeBridgeRegistry(bridge, registryDir, roots, createdAtMs);
  lastBridgeRegistryPublishMs = createdAtMs;
  return registryPath;
}

export async function ensureBridgeRegistryPresent(
  bridge: WritableBridgeRegistry,
  registryDir: string,
): Promise<string | undefined> {
  if (bridge.registryPath) {
    try {
      await fs.promises.stat(bridge.registryPath);
      return undefined;
    } catch {
      return publishBridgeRegistry(bridge, registryDir);
    }
  }
  return publishBridgeRegistry(bridge, registryDir);
}

async function republishBridgeRegistryIfAvailable(): Promise<void> {
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

function showBridgeStatus(): void {
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
  output.appendLine(`  activeSessions: ${sessions.size}`);
  output.show(true);
}

export function registryDirectoryIsTrusted(stats: Pick<fs.Stats, 'uid' | 'mode'>): boolean {
  if (process.platform === 'win32' || typeof process.getuid !== 'function') {
    return true;
  }
  return stats.uid === process.getuid() && (stats.mode & 0o022) === 0;
}

export function bridgeRegistryEntry(
  bridge: Pick<BridgeServer, 'id' | 'url' | 'token'>,
  workspaceRoots: string[],
  createdAtMs = Date.now(),
): BridgeRegistryEntry {
  return {
    version: 1,
    instanceId: bridge.id,
    bridgeUrl: bridge.url,
    token: bridge.token,
    workspaceRoots,
    createdAtMs,
  };
}

function workspaceRoots(): string[] {
  return (vscode.workspace.workspaceFolders ?? [])
    .map((folder) => folder.uri)
    .filter((uri) => uri.scheme === 'file')
    .map((uri) => uri.fsPath);
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

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_./:=+-]+$/.test(value)) {
    return value;
  }
  return JSON.stringify(value);
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}
