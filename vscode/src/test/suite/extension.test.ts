import * as assert from 'assert';
import { EventEmitter } from 'events';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as vscode from 'vscode';
import { binaryCandidates } from '../../binary';
import {
  BRIDGE_REGISTRY_MAX_AGE_MS,
  BRIDGE_REGISTRY_PRESENCE_CHECK_MS,
  BRIDGE_REGISTRY_REFRESH_MS,
  bridgeRegistryDirectory,
  bridgeRegistryEntry,
  ensureBridgeRegistryPresent,
  registryDirectoryIsTrusted,
  writeBridgeRegistry,
} from '../../bridgeRegistry';
import {
  bridgeStopResponse,
  bridgeWaitResponse,
  clientBinaryPathIsTrusted,
  isAuthorizedBridgeRequest,
  normalizeInterceptedArgs,
} from '../../bridgeServer';
import { collectFileSystemPaths } from '../../commands';
import { posixShim, windowsShim } from '../../terminalInterception';
import { parseStartupLine, waitForStartup, waitForStartupOrTerminate } from '../../viewerSessions';

class FakeChild extends EventEmitter {
  readonly stdout = new EventEmitter();
  readonly stderr = new EventEmitter();
  readonly killedSignals: string[] = [];

  kill(signal?: NodeJS.Signals | number): boolean {
    this.killedSignals.push(String(signal ?? 'SIGTERM'));
    return true;
  }
}

const output = {
  append: () => undefined,
  appendLine: () => undefined,
};
const bridgeContract = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, '../../../../docs/contracts/bridge-protocol.json'), 'utf8'),
);
const registryContract = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, '../../../../docs/contracts/vscode-bridge-registry.json'), 'utf8'),
);
const extensionManifest = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, '../../../package.json'), 'utf8'),
);

suite('dcmview extension', () => {
  test('parses structured startup events', () => {
    const url = parseStartupLine(
      '{"type":"server_started","url":"http://127.0.0.1:51234","host":"127.0.0.1","port":51234}',
    );

    assert.strictEqual(url, 'http://127.0.0.1:51234');
  });

  test('parses legacy startup lines', () => {
    const url = parseStartupLine('dcmview: server running at http://127.0.0.1:51234');

    assert.strictEqual(url, 'http://127.0.0.1:51234');
  });

  test('ignores unrelated startup output', () => {
    assert.strictEqual(parseStartupLine('dcmview: loaded 2 DICOM file(s)'), undefined);
    assert.strictEqual(parseStartupLine('{"type":"other","url":"http://127.0.0.1:1"}'), undefined);
  });

  test('waits for structured startup events before legacy fallback', async () => {
    const child = new FakeChild();
    const startup = waitForStartup(child as never, 1000, output);

    child.stdout.emit(
      'data',
      Buffer.from(
        '{"type":"server_started","url":"http://127.0.0.1:51234","host":"127.0.0.1","port":51234}\n' +
          'dcmview: server running at http://127.0.0.1:9999\n',
      ),
    );

    assert.strictEqual(await startup, 'http://127.0.0.1:51234');
  });

  test('terminates child process when startup wait fails', async () => {
    const child = new FakeChild();

    await assert.rejects(
      () => waitForStartupOrTerminate(child as never, 1, output),
      /Timed out waiting for dcmview startup/,
    );
    assert.deepStrictEqual(child.killedSignals, ['SIGINT']);
  });

  test('collects only filesystem uris', () => {
    const paths = collectFileSystemPaths([
      vscode.Uri.file('/tmp/a.dcm'),
      vscode.Uri.parse('untitled:Scratch'),
      vscode.Uri.file('/tmp/study'),
    ]);

    assert.deepStrictEqual(paths, ['/tmp/a.dcm', '/tmp/study']);
  });

  test('orders binary candidates for local development', () => {
    const candidates = binaryCandidates('/repo/vscode', '/custom/dcmview');

    assert.deepStrictEqual(candidates[0], { kind: 'path', value: '/custom/dcmview' });
    assert.ok(candidates.some((candidate) => candidate.value.includes('target/debug/dcmview')));
    assert.strictEqual(candidates[candidates.length - 1].kind, 'path-name');
  });

  test('generates Windows x64 binary candidates', () => {
    const candidates = binaryCandidates('C:\\repo\\vscode', '', 'win32', 'x64');

    assert.ok(candidates.some((candidate) => candidate.value.includes('target/debug/dcmview.exe')));
    assert.ok(
      candidates.some((candidate) =>
        candidate.value.includes(path.join('resources', 'bin', 'win32-x64', 'dcmview.exe')),
      ),
    );
    assert.deepStrictEqual(candidates[candidates.length - 1], {
      kind: 'path-name',
      value: 'dcmview.exe',
    });
  });

  test('normalizes terminal-intercepted args without clobbering explicit host and port', () => {
    assert.deepStrictEqual(normalizeInterceptedArgs(['/tmp/scan.dcm']), [
      '--port',
      '0',
      '--host',
      '127.0.0.1',
      '--startup-json',
      '--no-browser',
      '/tmp/scan.dcm',
    ]);

    assert.deepStrictEqual(
      normalizeInterceptedArgs(['--host', 'localhost', '-p', '8888', '--no-browser', '/tmp/scan.dcm']),
      ['--startup-json', '--host', 'localhost', '-p', '8888', '--no-browser', '/tmp/scan.dcm'],
    );
  });

  test('generates shims that route through the bridge client with bypass fallback', () => {
    const posix = posixShim('/repo/target/debug/dcmview', 'dcmview-py');
    assert.ok(posix.includes('DCMVIEW_VSCODE_BYPASS'));
    assert.ok(posix.includes('--vscode-bridge-client'));
    assert.ok(posix.includes("'dcmview-py'"));

    const windows = windowsShim('C:\\repo\\target\\debug\\dcmview.exe', 'dcmview');
    assert.ok(windows.includes('DCMVIEW_VSCODE_BYPASS'));
    assert.ok(windows.includes('--vscode-bridge-client dcmview'));
  });

  test('matches shared bridge auth and response fixture', () => {
    const token = bridgeContract.auth.bearerToken;

    assert.strictEqual(
      isAuthorizedBridgeRequest({ headers: { authorization: `Bearer ${token}` } }, token),
      true,
    );
    assert.strictEqual(
      isAuthorizedBridgeRequest({ headers: { 'x-dcmview-token': token } }, token),
      true,
    );
    assert.strictEqual(isAuthorizedBridgeRequest({ headers: { authorization: 'Bearer wrong' } }, token), false);
    assert.deepStrictEqual(bridgeStopResponse(), bridgeContract.stop.response);
    assert.deepStrictEqual(bridgeWaitResponse(bridgeContract.wait.response.exitCode), bridgeContract.wait.response);
  });

  test('matches shared bridge registry contract', () => {
    assert.strictEqual(BRIDGE_REGISTRY_MAX_AGE_MS, registryContract.ttlMs);
    assert.strictEqual(BRIDGE_REGISTRY_REFRESH_MS, registryContract.refreshMs);
    assert.strictEqual(BRIDGE_REGISTRY_PRESENCE_CHECK_MS, registryContract.presenceCheckMs);

    for (const testCase of registryContract.registryDirs) {
      assert.strictEqual(bridgeRegistryDirectory(testCase.env), testCase.expected);
    }
  });

  test('serializes bridge registry entries for out-of-band discovery', () => {
    const entry = bridgeRegistryEntry(
      { id: 'instance-1', url: 'http://127.0.0.1:4567', token: 'secret' },
      ['/workspace'],
      12345,
    );

    assert.deepStrictEqual(entry, {
      version: 1,
      instanceId: 'instance-1',
      bridgeUrl: 'http://127.0.0.1:4567',
      token: 'secret',
      workspaceRoots: ['/workspace'],
      createdAtMs: 12345,
    });
  });

  test('refresh rewrites the same bridge registry entry with a new timestamp', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dcmview-registry-'));
    const registryDir = path.join(root, 'bridges');
    const bridge = { id: 'instance-1', url: 'http://127.0.0.1:4567', token: 'secret' };
    try {
      const firstPath = await writeBridgeRegistry(bridge, registryDir, ['/workspace'], 1000);
      let entry = JSON.parse(fs.readFileSync(firstPath, 'utf8'));
      assert.strictEqual(entry.instanceId, 'instance-1');
      assert.strictEqual(entry.createdAtMs, 1000);

      const secondPath = await writeBridgeRegistry(bridge, registryDir, ['/workspace'], 2000);
      entry = JSON.parse(fs.readFileSync(secondPath, 'utf8'));
      assert.strictEqual(secondPath, firstPath);
      assert.strictEqual(entry.instanceId, 'instance-1');
      assert.strictEqual(entry.createdAtMs, 2000);

      fs.unlinkSync(secondPath);
      const thirdPath = await writeBridgeRegistry(bridge, registryDir, ['/workspace'], 3000);
      entry = JSON.parse(fs.readFileSync(thirdPath, 'utf8'));
      assert.strictEqual(thirdPath, firstPath);
      assert.strictEqual(entry.createdAtMs, 3000);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  test('presence check republishes missing bridge registry file', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dcmview-registry-'));
    const registryDir = path.join(root, 'bridges');
    const bridge = { id: 'instance-1', url: 'http://127.0.0.1:4567', token: 'secret' };
    try {
      const firstPath = await writeBridgeRegistry(bridge, registryDir, ['/workspace'], 1000);
      fs.unlinkSync(firstPath);
      const secondPath = await ensureBridgeRegistryPresent(bridge, registryDir);
      assert.strictEqual(secondPath, firstPath);
      const entry = JSON.parse(fs.readFileSync(firstPath, 'utf8'));
      assert.strictEqual(entry.instanceId, 'instance-1');
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  test('validates client supplied bridge binaries', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dcmview-client-bin-'));
    try {
      const binary = path.join(root, process.platform === 'win32' ? 'dcmview.exe' : 'dcmview');
      const wrongName = path.join(root, 'viewer');
      fs.writeFileSync(binary, '');
      fs.writeFileSync(wrongName, '');
      if (process.platform !== 'win32') {
        fs.chmodSync(binary, 0o700);
        fs.chmodSync(wrongName, 0o700);
      }

      assert.strictEqual(await clientBinaryPathIsTrusted(binary), true);
      assert.strictEqual(await clientBinaryPathIsTrusted('relative/dcmview'), false);
      assert.strictEqual(await clientBinaryPathIsTrusted(wrongName), false);

      if (process.platform !== 'win32') {
        fs.chmodSync(binary, 0o722);
        assert.strictEqual(await clientBinaryPathIsTrusted(binary), false);
      }
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

  test('rejects untrusted unix registry directory metadata', () => {
    if (process.platform === 'win32' || typeof process.getuid !== 'function') {
      assert.strictEqual(registryDirectoryIsTrusted({ uid: 9999, mode: 0o777 } as fs.Stats), true);
      return;
    }
    const uid = process.getuid();
    assert.strictEqual(registryDirectoryIsTrusted({ uid, mode: 0o700 } as fs.Stats), true);
    assert.strictEqual(registryDirectoryIsTrusted({ uid: uid + 1, mode: 0o700 } as fs.Stats), false);
    assert.strictEqual(registryDirectoryIsTrusted({ uid, mode: 0o722 } as fs.Stats), false);
  });

  test('activation registers every contributed command', async () => {
    const extension = vscode.extensions.getExtension('beatricebm.dcmview');
    assert.ok(extension, 'development extension should be available');
    await extension.activate();

    const registered = new Set(await vscode.commands.getCommands(true));
    const contributed = extensionManifest.contributes.commands.map(
      (contribution: { command: string }) => contribution.command,
    );

    assert.ok(contributed.length > 0);
    assert.deepStrictEqual(
      contributed.filter((command: string) => !registered.has(command)),
      [],
    );
  });
});
