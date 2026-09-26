import * as fs from 'fs';
import * as path from 'path';

export async function resolveBinaryPath(extensionRoot: string, configuredBinary: string): Promise<string> {
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
