import * as assert from 'assert';
import * as childProcess from 'child_process';
import * as fs from 'fs';
import * as http from 'http';
import * as os from 'os';
import * as path from 'path';
import * as vscode from 'vscode';
import { posixShim } from '../../terminalInterception';
import { activeSessionCount } from '../../viewerSessions';

// These tests drive the real debug binary that `python scripts/check.py e2e`
// (or `cargo build --bin dcmview`) leaves in target/debug, which the extension
// also finds through its normal binary resolution. Every flag the extension
// and its shims pass therefore has to exist on the binary's CLI.
const repoRoot = path.resolve(__dirname, '../../../..');
const fixtureName = 'golden-uncompressed-u16-multiframe.dcm';
const fixturePath = path.join(repoRoot, 'tests', 'fixtures', fixtureName);
const debugBinary = path.join(repoRoot, 'target', 'debug', process.platform === 'win32' ? 'dcmview.exe' : 'dcmview');

function getJson(url: string): Promise<{ status: number; body: any }> {
  return new Promise((resolve, reject) => {
    http
      .get(url, (response) => {
        const chunks: Buffer[] = [];
        response.on('data', (chunk: Buffer) => chunks.push(chunk));
        response.on('end', () =>
          resolve({ status: response.statusCode ?? 0, body: JSON.parse(Buffer.concat(chunks).toString('utf8')) }),
        );
      })
      .on('error', reject);
  });
}

async function waitFor<T>(description: string, probe: () => T | undefined, timeoutMs = 20000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = probe();
    if (value !== undefined) {
      return value;
    }
    if (Date.now() > deadline) {
      throw new Error(`Timed out waiting for ${description}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

function findTab(matches: (input: unknown) => boolean): vscode.Tab | undefined {
  return vscode.window.tabGroups.all.flatMap((group) => group.tabs).find((tab) => matches(tab.input));
}

function publishedBridge(): Promise<{ bridgeUrl: string; token: string }> {
  const registryDir = process.env.DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR;
  assert.ok(registryDir, 'runTest must point DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR at a private directory');
  return waitFor('the extension to publish its bridge registry entry', () => {
    const entry = fs.existsSync(registryDir)
      ? fs.readdirSync(registryDir).find((name) => name.endsWith('.json'))
      : undefined;
    return entry ? JSON.parse(fs.readFileSync(path.join(registryDir, entry), 'utf8')) : undefined;
  });
}

async function scannedFiles(viewerUrl: string): Promise<any[]> {
  const deadline = Date.now() + 20000;
  for (;;) {
    const { status, body } = await getJson(`${viewerUrl}/api/files`);
    assert.strictEqual(status, 200);
    if (body.scan_complete) {
      return body.files;
    }
    if (Date.now() > deadline) {
      throw new Error('Timed out waiting for the viewer to finish scanning');
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

suite('dcmview viewer launch', () => {
  suiteSetup(async function () {
    this.timeout(30000);
    assert.ok(fs.existsSync(debugBinary), `build the debug binary first (cargo build --bin dcmview): ${debugBinary}`);
    const extension = vscode.extensions.getExtension('beatricebm.dcmview');
    assert.ok(extension, 'development extension should be available');
    await extension.activate();
  });

  teardown(async () => {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
  });

  test('a terminal shim launch opens the fixture in VS Code and ends when the panel closes', async function () {
    if (process.platform === 'win32') {
      this.skip();
    }
    this.timeout(60000);
    const { bridgeUrl, token } = await publishedBridge();
    const shimDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcmview-shim-'));
    const shim = path.join(shimDir, 'dcmview');
    fs.writeFileSync(shim, posixShim(debugBinary, 'dcmview'), { mode: 0o755 });

    // What typing `dcmview <file>` in a VS Code terminal runs: the shim calls
    // the binary's bridge client, which asks the extension's bridge to launch.
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      DCMVIEW_VSCODE_BRIDGE_URL: bridgeUrl,
      DCMVIEW_VSCODE_BRIDGE_TOKEN: token,
    };
    delete env.DCMVIEW_VSCODE_BYPASS;
    const client = childProcess.spawn(shim, [fixturePath], { cwd: path.dirname(fixturePath), env });
    const clientExit = new Promise<number | null>((resolve) => client.once('exit', (code) => resolve(code)));
    let stdout = '';
    client.stdout.on('data', (chunk: Buffer) => (stdout += chunk.toString('utf8')));
    let stderr = '';
    client.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString('utf8')));

    try {
      const viewerUrl = await waitFor('the shim to report a VS Code viewer URL', () =>
        /opened in VS Code at (http:\/\/127\.0\.0\.1:\d+)/.exec(stdout)?.[1],
      ).catch((error: Error) => {
        throw new Error(`${error.message}\nstdout: ${stdout}\nstderr: ${stderr}`);
      });

      const files = await scannedFiles(viewerUrl);
      assert.strictEqual(files.length, 1);
      assert.strictEqual(path.basename(files[0].path), fixtureName);
      assert.strictEqual(files[0].has_pixels, true);
      assert.ok(files[0].frame_count > 1, 'fixture is multi-frame');

      const panel = findTab(
        (input) => input instanceof vscode.TabInputWebview && input.viewType.endsWith('dcmview.viewer'),
      );
      assert.ok(panel, 'the bridge launch should open a dcmview webview panel');
      await vscode.window.tabGroups.close(panel);

      // The bridge client waits on the viewer, so it returns once the viewer
      // process has exited, and the viewer's port stops answering.
      assert.strictEqual(typeof (await clientExit), 'number');
      await assert.rejects(() => getJson(`${viewerUrl}/api/files`), 'viewer port should be closed');
    } finally {
      client.kill('SIGINT');
      fs.rmSync(shimDir, { recursive: true, force: true });
    }
  });

  test('the DICOM custom editor runs a viewer only while it is open', async function () {
    this.timeout(60000);
    // Non-recursive scanning adds --no-recursive, so the custom editor also
    // exercises the optional flag the extension builds.
    const config = vscode.workspace.getConfiguration('dcmview');
    await config.update('defaultRecursive', false, vscode.ConfigurationTarget.Global);
    try {
      assert.strictEqual(activeSessionCount(), 0);

      await vscode.commands.executeCommand('vscode.openWith', vscode.Uri.file(fixturePath), 'dcmview.dicomViewer');
      const editor = await waitFor('the custom editor tab', () =>
        findTab((input) => input instanceof vscode.TabInputCustom && input.viewType === 'dcmview.dicomViewer'),
      );
      await waitFor('the custom editor viewer session', () => (activeSessionCount() === 1 ? true : undefined));

      await vscode.window.tabGroups.close(editor);
      await waitFor('the custom editor viewer to stop', () => (activeSessionCount() === 0 ? true : undefined));
    } finally {
      await config.update('defaultRecursive', undefined, vscode.ConfigurationTarget.Global);
    }
  });
});
