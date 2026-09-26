import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as vscode from 'vscode';
import { formatError, getOutputChannel } from './output';
import { BRIDGE_REGISTRY_DIR_ENV } from './settings';

export const BRIDGE_REGISTRY_MAX_AGE_MS = 3 * 60 * 60 * 1000;

export const BRIDGE_REGISTRY_REFRESH_MS = 60 * 60 * 1000;

export const BRIDGE_REGISTRY_PRESENCE_CHECK_MS = 60 * 1000;

export interface WritableBridgeRegistry {
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

let bridgeRefreshTimer: NodeJS.Timeout | undefined;

let bridgePresenceTimer: NodeJS.Timeout | undefined;

let lastBridgeRegistryPublishMs: number | undefined;

export function startBridgeRegistryRefresh(bridge: WritableBridgeRegistry, registryDir: string): void {
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

export function stopBridgeRegistryRefresh(): void {
  if (bridgeRefreshTimer) {
    clearInterval(bridgeRefreshTimer);
    bridgeRefreshTimer = undefined;
  }
  if (bridgePresenceTimer) {
    clearInterval(bridgePresenceTimer);
    bridgePresenceTimer = undefined;
  }
}

export async function unpublishBridgeRegistry(bridge: WritableBridgeRegistry): Promise<void> {
  stopBridgeRegistryRefresh();
  if (bridge.registryPath) {
    await fs.promises.unlink(bridge.registryPath).catch(() => undefined);
  }
  lastBridgeRegistryPublishMs = undefined;
}

export function lastBridgeRegistryPublishTime(): number | undefined {
  return lastBridgeRegistryPublishMs;
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

export async function publishBridgeRegistry(
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

export function registryDirectoryIsTrusted(stats: Pick<fs.Stats, 'uid' | 'mode'>): boolean {
  if (process.platform === 'win32' || typeof process.getuid !== 'function') {
    return true;
  }
  return stats.uid === process.getuid() && (stats.mode & 0o022) === 0;
}

export function bridgeRegistryEntry(
  bridge: Pick<WritableBridgeRegistry, 'id' | 'url' | 'token'>,
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
