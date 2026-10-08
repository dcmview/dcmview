// Drives real VS Code with the real Remote-SSH extension against the
// remote-ssh container, for python/tests/vscode_remote_ssh_integration.py.
//
//   node tests/remote/vscode_remote_ssh.mjs connect    connect once, so the
//                                                     VS Code Server installs
//   node tests/remote/vscode_remote_ssh.mjs scenarios  run the user flows and
//                                                     print one RESULT line each
//
// Configuration comes from DCMVIEW_VSCODE_REMOTE_CONFIG, a JSON file the
// Python suite writes: { workdir, sshConfig, hostAlias, remoteFolder,
// fixtureName, pythonEntry, artifactsDir, vscodeVersion }.

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const requireFromExtension = createRequire(path.join(repoRoot, 'vscode', 'package.json'));
const { _electron: electron } = requireFromExtension('playwright-core');
const {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
} = requireFromExtension('@vscode/test-electron');

const REMOTE_EXTENSION_ID = 'ms-vscode-remote.remote-ssh';
const CONNECT_TIMEOUT_MS = 240_000;
const PAINT_TIMEOUT_MS = 60_000;
const VIEWER_FRAME_URL = /^http:\/\/(localhost|127\.0\.0\.1):\d+\//;

const config = JSON.parse(fs.readFileSync(process.env.DCMVIEW_VSCODE_REMOTE_CONFIG, 'utf8'));
const userDataDir = path.join(config.workdir, 'vscode-user-data');
const extensionsDir = path.join(config.workdir, 'vscode-extensions');

async function prepareVSCode() {
  const executable = await downloadAndUnzipVSCode({
    version: config.vscodeVersion,
    cachePath: path.join(repoRoot, 'vscode', '.vscode-test'),
  });
  if (!fs.existsSync(path.join(extensionsDir, '.remote-ssh-installed'))) {
    const [cli, ...cliArgs] = resolveCliArgsFromVSCodeExecutablePath(executable);
    const install = spawnSync(
      cli,
      [...cliArgs, '--extensions-dir', extensionsDir, '--user-data-dir', userDataDir, '--install-extension', REMOTE_EXTENSION_ID],
      { stdio: 'inherit' },
    );
    if (install.status !== 0) {
      throw new Error(`installing ${REMOTE_EXTENSION_ID} failed with ${install.status}`);
    }
    fs.writeFileSync(path.join(extensionsDir, '.remote-ssh-installed'), '');
  }
  const settingsDir = path.join(userDataDir, 'User');
  fs.mkdirSync(settingsDir, { recursive: true });
  fs.writeFileSync(
    path.join(settingsDir, 'settings.json'),
    JSON.stringify(
      {
        'remote.SSH.configFile': config.sshConfig,
        'remote.SSH.remotePlatform': { [config.hostAlias]: 'linux' },
        'remote.SSH.showLoginTerminal': false,
        'remote.SSH.connectTimeout': 120,
        'remote.autoForwardPorts': false,
        'security.workspace.trust.enabled': false,
        'workbench.startupEditor': 'none',
        'workbench.editorAssociations': { '*.dcm': 'dcmview.dicomViewer' },
        'extensions.autoUpdate': false,
        'extensions.autoCheckUpdates': false,
        'update.mode': 'none',
        'telemetry.telemetryLevel': 'off',
        'terminal.integrated.enablePersistentSessions': false,
      },
      null,
      2,
    ),
  );
  return executable;
}

async function launch(executable) {
  const app = await electron.launch({
    executablePath: executable,
    args: [
      '--no-sandbox',
      '--disable-gpu-sandbox',
      '--disable-workspace-trust',
      '--skip-welcome',
      '--skip-release-notes',
      '--disable-telemetry',
      `--user-data-dir=${userDataDir}`,
      `--extensions-dir=${extensionsDir}`,
      `--folder-uri=vscode-remote://ssh-remote+${config.hostAlias}${config.remoteFolder}`,
    ],
    env: { ...process.env, DONT_PROMPT_WSL_INSTALL: '1' },
    timeout: 60_000,
  });
  const page = await app.firstWindow();
  return { app, page };
}

async function waitForRemoteWorkspace(page) {
  // The explorer lists the remote folder only once the remote extension host
  // is up, so the fixture's row is the readiness signal.
  await page
    .locator('.explorer-folders-view .monaco-list-row', { hasText: config.fixtureName })
    .first()
    .waitFor({ timeout: CONNECT_TIMEOUT_MS });
}

async function runCommand(page, title) {
  await page.keyboard.press('F1');
  const input = page.locator('.quick-input-widget input');
  await input.waitFor();
  await input.fill(`>${title}`);
  await page.locator('.quick-input-list .monaco-list-row', { hasText: title }).first().waitFor();
  await page.keyboard.press('Enter');
}

function viewerFrames(page) {
  return page.frames().filter((frame) => VIEWER_FRAME_URL.test(frame.url()));
}

async function framePaints(frame) {
  return frame
    .evaluate(() =>
      [...document.querySelectorAll('canvas')].some((canvas) => {
        if (!canvas.width || !canvas.height) return false;
        const pixels = canvas.getContext('2d')?.getImageData(0, 0, canvas.width, canvas.height).data;
        return !!pixels && pixels.some((value, index) => index % 4 !== 3 && value > 0);
      }),
    )
    .catch(() => false);
}

/**
 * Wait for a viewer frame not in `known` that paints. The page drops the
 * `#token=` fragment from its URL once read, and every frame request needs
 * the token, so a painted frame is the proof that it arrived.
 */
async function waitForNewPaintedViewer(page, known) {
  const deadline = Date.now() + PAINT_TIMEOUT_MS;
  let seen = [];
  while (Date.now() < deadline) {
    seen = viewerFrames(page).filter((frame) => !known.has(frame));
    for (const frame of seen) {
      if (await framePaints(frame)) {
        known.add(frame);
        return { painted: true, url: new URL(frame.url()).origin };
      }
    }
    await page.waitForTimeout(500);
  }
  return { painted: false, frames: seen.map((frame) => new URL(frame.url()).origin) };
}

/**
 * Open an integrated terminal on the remote. The command palette also lists
 * "Create New Terminal (Local)", which a prefix match can pick, so this uses
 * the default keybinding of workbench.action.terminal.new instead.
 */
async function newRemoteTerminal(page) {
  await page.keyboard.press('Control+Shift+Backquote');
  await page.waitForTimeout(3_000);
}

async function typeInTerminal(page, line) {
  // Creating a terminal focuses it; typing goes to its xterm textarea.
  await page.locator('.xterm-helper-textarea').last().waitFor({ state: 'attached' });
  await page.keyboard.type(line);
  await page.keyboard.press('Enter');
}

/** What a failed flow left on screen: terminal text and editor tab titles. */
async function diagnostics(page) {
  const terminal = await page
    .locator('.xterm-rows')
    .last()
    .innerText({ timeout: 2_000 })
    .catch(() => '');
  const tabs = await page
    .locator('.tabs-container .tab .label-name')
    .allInnerTexts()
    .catch(() => []);
  return { terminal: terminal.split('\n').filter((row) => row.trim()).slice(-15), tabs };
}

function report(name, ok, detail = {}) {
  console.log(`RESULT ${JSON.stringify({ name, ok, ...detail })}`);
}

async function screenshot(page, name) {
  if (!config.artifactsDir) return;
  fs.mkdirSync(config.artifactsDir, { recursive: true });
  await page.screenshot({ path: path.join(config.artifactsDir, `${name}.png`) }).catch(() => {});
}

async function connect(executable) {
  const { app, page } = await launch(executable);
  try {
    await waitForRemoteWorkspace(page);
    report('connect', true);
  } catch (error) {
    await screenshot(page, 'connect');
    report('connect', false, { error: String(error) });
  } finally {
    await app.close();
  }
}

async function scenarios(executable) {
  const { app, page } = await launch(executable);
  const known = new Set();
  try {
    await waitForRemoteWorkspace(page);
    // Terminal shims are added when the extension activates, after startup.
    await page.waitForTimeout(5_000);

    const steps = [
      [
        'custom-editor',
        async () => {
          await page
            .locator('.explorer-folders-view .monaco-list-row', { hasText: config.fixtureName })
            .first()
            .dblclick();
        },
      ],
      [
        'terminal-dcmview',
        async () => {
          await newRemoteTerminal(page);
          await typeInTerminal(page, `dcmview ${config.fixtureName}`);
        },
      ],
      [
        'terminal-python',
        async () => {
          await newRemoteTerminal(page);
          await typeInTerminal(page, `${config.pythonEntry} ${config.fixtureName}`);
        },
      ],
    ];
    for (const [name, open] of steps) {
      try {
        await open();
        const result = await waitForNewPaintedViewer(page, known);
        if (!result.painted) {
          await screenshot(page, name);
          Object.assign(result, await diagnostics(page));
        }
        report(name, result.painted, result);
      } catch (error) {
        await screenshot(page, name);
        report(name, false, { error: String(error) });
      }
    }

    await runCommand(page, 'View: Close All Editors');
    report('closed-all-editors', true);
    // Let the extension stop its sessions before the window goes away; the
    // Python suite then checks the remote has no dcmview left.
    await page.waitForTimeout(5_000);
  } catch (error) {
    await screenshot(page, 'scenarios');
    report('scenarios', false, { error: String(error) });
  } finally {
    await app.close();
  }
}

const phase = process.argv[2];
const executable = await prepareVSCode();
if (phase === 'connect') {
  await connect(executable);
} else if (phase === 'scenarios') {
  await scenarios(executable);
} else {
  throw new Error(`unknown phase ${phase}; use connect or scenarios`);
}
