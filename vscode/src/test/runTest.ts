import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { runTests } from '@vscode/test-electron';

async function main(): Promise<void> {
  const extensionDevelopmentPath = path.resolve(__dirname, '..', '..');
  const extensionTestsPath = path.resolve(__dirname, 'suite', 'index');
  const version = process.env.DCMVIEW_VSCODE_TEST_VERSION?.trim() || '1.90.2';
  // Keep the test instance's bridge out of the developer's real registry, so
  // terminal launches elsewhere never route into it, and let tests read it.
  const registryDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dcmview-vscode-test-bridges-'));

  try {
    await runTests({
      extensionDevelopmentPath,
      extensionTestsPath,
      version,
      extensionTestsEnv: { DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR: registryDir },
    });
  } finally {
    fs.rmSync(registryDir, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
