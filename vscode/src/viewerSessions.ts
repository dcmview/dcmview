import * as childProcess from 'child_process';
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';
import { BRIDGE_BYPASS_ENV, ExtensionSettings } from './settings';

const STARTUP_PREFIX = 'dcmview: server running at ';

const STARTUP_EVENT_TYPE = 'server_started';

interface StartupEvent {
  type: string;
  url?: string | null;
  base_url?: string | null;
  token?: string | null;
}

export interface RunningSession {
  readonly id: string;
  readonly panel: vscode.WebviewPanel;
  readonly process: childProcess.ChildProcessWithoutNullStreams;
  readonly output: vscode.OutputChannel;
  readonly name: string;
  readonly url: string;
  readonly exitCode: Promise<number>;
  stopped: boolean;
}

const sessions = new Set<RunningSession>();

const sessionsById = new Map<string, RunningSession>();

export function findSession(id: string): RunningSession | undefined {
  return sessionsById.get(id);
}

export function activeSessionCount(): number {
  return sessions.size;
}

export async function startSession(
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

  let startup: StartupEvent;
  try {
    startup = await waitForStartupEvent(
      child,
      settings.startupTimeoutSeconds * 1000,
      output,
    );
  } catch (error) {
    panelDisposeListener.dispose();
    child.kill('SIGINT');
    throw error;
  }

  try {
    if (panelDisposed) {
      throw new Error('dcmview panel closed before startup completed.');
    }
    const externalUri = await vscode.env.asExternalUri(vscode.Uri.parse(startupBaseUrl(startup)));
    if (panelDisposed) {
      throw new Error('dcmview panel closed before startup completed.');
    }
    const theme = viewerTheme(vscode.window.activeColorTheme.kind);
    const viewerUri = forwardedViewerUrl(startup, externalUri, theme);
    panel.webview.html = webviewHtml(panel.webview, viewerUri);
    const themeListener = vscode.window.onDidChangeActiveColorTheme((colorTheme) => {
      void panel.webview.postMessage({ type: THEME_MESSAGE_TYPE, theme: viewerTheme(colorTheme.kind) });
    });
    panel.onDidDispose(() => themeListener.dispose());
  } catch (error) {
    panelDisposeListener.dispose();
    child.kill('SIGINT');
    if (panelDisposed) {
      throw error;
    }
    // URI and forwarding errors can quote their input, including a legacy
    // URL's token: keep the cause, drop the credential.
    const cause = (error instanceof Error ? error.message : String(error)).replace(
      /#token=[^\s"'<>)]*/g,
      '#token=…',
    );
    throw new Error(`Could not load the dcmview viewer URL in VS Code: ${cause}`);
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
    url: startupViewerUrl(startup),
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

export async function startPathSession(
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
    sessionTitle(filePaths, settings),
    settings,
    output,
  );
}

export async function startPathSessionInPanel(
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
    sessionTitle(filePaths, settings),
    settings,
    output,
    panel,
  );
}

/** A masked session's tab does not name the file: file names often identify the patient. */
function sessionTitle(filePaths: readonly string[], settings: Pick<ExtensionSettings, 'extraArgs'>): string {
  if (settings.extraArgs.includes('--mask')) {
    return 'dcmview: masked session';
  }
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
  return waitForStartupEvent(child, timeoutMs, output).then((event) => startupViewerUrl(event));
}

function waitForStartupEvent(
  child: Pick<childProcess.ChildProcessWithoutNullStreams, 'stdout' | 'stderr' | 'once'>,
  timeoutMs: number,
  output: Pick<vscode.OutputChannel, 'append' | 'appendLine'>,
): Promise<StartupEvent> {
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
        const safeLine = startupLogLine(line);
        output.appendLine(safeLine);
        recentLines.push(safeLine);
        recentLines.splice(0, Math.max(0, recentLines.length - 20));
        if (!settled) {
          try {
            const parsed = parseStartupEvent(line);
            if (parsed) {
              startupViewerUrl(parsed); // Reject socket-only or invalid URLs immediately.
              settled = true;
              clearTimeout(timer);
              resolve(parsed);
            }
          } catch {
            fail(new Error('dcmview did not report a usable HTTP viewer URL; Unix socket launches are not supported in VS Code.'));
          }
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
  const event = parseStartupEvent(line);
  return event ? startupViewerUrl(event) : undefined;
}

function parseStartupEvent(line: string): StartupEvent | undefined {
  const trimmed = line.trim();
  if (trimmed.startsWith('{')) {
    try {
      const event = JSON.parse(trimmed) as StartupEvent | null;
      if (event?.type === STARTUP_EVENT_TYPE) {
        return event;
      }
    } catch {
      return undefined;
    }
  }

  if (trimmed.startsWith(STARTUP_PREFIX)) {
    return { type: STARTUP_EVENT_TYPE, url: trimmed.slice(STARTUP_PREFIX.length) };
  }
  return undefined;
}

/** Only the bare origin of new startup events is sent through port forwarding. */
export function startupBaseUrl(event: StartupEvent): string {
  if (typeof event.base_url === 'string') {
    return event.base_url;
  }
  if (typeof event.url === 'string') {
    return event.url;
  }
  throw new Error('dcmview did not report a usable HTTP viewer URL; Unix socket launches are not supported in VS Code.');
}

/**
 * The URL the webview loads: the forwarded origin, then the theme query, then
 * the token fragment. The token is attached last, so it does not depend on
 * port forwarding keeping fragments.
 */
export function forwardedViewerUrl(
  event: StartupEvent,
  externalUri: vscode.Uri,
  theme: 'light' | 'dark',
): URL {
  return new URL(startupViewerUrl(event, viewerUrl(externalUri, theme).href));
}

/** Reattach credentials after forwarding; older binaries retain their URL fallback. */
export function startupViewerUrl(event: StartupEvent, externalUrl = startupBaseUrl(event)): string {
  try {
    const url = new URL(externalUrl);
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      throw new Error();
    }
    if (typeof event.base_url === 'string' && typeof event.token === 'string') {
      url.hash = `token=${encodeURIComponent(event.token)}`;
      return url.href;
    }
    return externalUrl;
  } catch {
    throw new Error('dcmview did not report a usable HTTP viewer URL.');
  }
}

function startupLogLine(line: string): string {
  const event = parseStartupEvent(line);
  if (event) {
    try {
      return `${STARTUP_PREFIX}${new URL(startupBaseUrl(event)).origin}`;
    } catch {
      return 'dcmview: startup event has no usable HTTP viewer URL';
    }
  }
  // Also covers the socket-mode SSH hint printed after its startup event.
  return line.replace(/#token=[^\s]*/g, '');
}

/** Message the viewer page accepts from its parent frame; see frontend/src/lib/app/theme.ts. */
const THEME_MESSAGE_TYPE = 'dcmview-theme';

/** The viewer follows the editor's colour theme; high-contrast themes map to their base. */
function viewerTheme(kind: vscode.ColorThemeKind): 'light' | 'dark' {
  return kind === vscode.ColorThemeKind.Light || kind === vscode.ColorThemeKind.HighContrastLight
    ? 'light'
    : 'dark';
}

export function viewerUrl(externalUri: vscode.Uri, theme: 'light' | 'dark'): URL {
  const url = new URL(externalUri.toString(true));
  url.searchParams.set('theme', theme);
  return url;
}

function webviewHtml(webview: vscode.Webview, externalUrl: URL): string {
  const iframeSrc = escapeHtml(externalUrl.href);
  const origin = externalUrl.origin;
  const frameOrigin = escapeHtml(origin);
  const nonce = crypto.randomBytes(16).toString('base64');
  const csp = [
    "default-src 'none'",
    `frame-src ${frameOrigin}`,
    `img-src ${webview.cspSource} https: data:`,
    "style-src 'unsafe-inline'",
    `script-src 'nonce-${nonce}'`,
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
      background: var(--vscode-editor-background, #1a1a1a);
      overflow: hidden;
    }
  </style>
</head>
<body>
  <iframe src="${iframeSrc}" title="dcmview"></iframe>
  <script nonce="${nonce}">
    const frame = document.querySelector('iframe');
    window.addEventListener('message', (event) => {
      if (event.data && event.data.type === ${JSON.stringify(THEME_MESSAGE_TYPE)}) {
        frame.contentWindow.postMessage(event.data, ${JSON.stringify(origin)});
      }
    });
  </script>
</body>
</html>`;
}

export async function stopAllSessions(): Promise<void> {
  await Promise.all(Array.from(sessions).map((session) => stopSession(session)));
}

export async function stopSession(session: RunningSession, disposePanel = true): Promise<void> {
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

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_./:=+-]+$/.test(value)) {
    return value;
  }
  return JSON.stringify(value);
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}
