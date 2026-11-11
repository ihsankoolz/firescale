import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { BIN, REPO } from './test-helpers.js';

const run = promisify(execFile);
const PLATFORMS = [
  'linux-x64',
  'linux-arm64',
  'darwin-x64',
  'darwin-arm64',
  'win32-x64',
  'win32-arm64',
];
const platform = `${process.platform}-${process.arch}`;

interface Manifest {
  version: string;
  platform: string;
  tarballs: { firescale: string; sdk: string; platform: string };
}
interface Pkg {
  name: string;
  version: string;
  private?: boolean;
  os?: string[];
  cpu?: string[];
  bin?: Record<string, string>;
  dependencies?: Record<string, string>;
  optionalDependencies?: Record<string, string>;
}

let work = '';
let manifest: Manifest;
const extracted: Record<string, string> = {};

async function exists(file: string): Promise<boolean> {
  return stat(file).then(
    () => true,
    () => false,
  );
}

async function extract(kind: string, tarball: string): Promise<string> {
  const dir = path.join(work, `x-${kind}`);
  await mkdir(dir, { recursive: true });
  // Relative paths with cwd: GNU tar on Windows reads `D:\\...` as a remote host.
  await run('tar', ['-xzf', path.join('out', tarball), '-C', `x-${kind}`], { cwd: work });
  return path.join(dir, 'package');
}

async function pkg(kind: string): Promise<Pkg> {
  return JSON.parse(
    await readFile(path.join(extracted[kind] ?? '', 'package.json'), 'utf8'),
  ) as Pkg;
}

describe('npm packaging', () => {
  beforeAll(async () => {
    work = await mkdtemp(path.join(tmpdir(), 'fscale-pack-'));
    const out = path.join(work, 'out');
    // Run twice: the second run must replace the first without failing.
    for (let i = 0; i < 2; i += 1) {
      await run(
        process.execPath,
        [path.join(REPO, 'scripts', 'pack-npm.mjs'), '--binary', BIN, '--out', out],
        {
          cwd: REPO,
        },
      );
    }
    manifest = JSON.parse(await readFile(path.join(out, 'manifest.json'), 'utf8')) as Manifest;
    for (const kind of ['firescale', 'sdk', 'platform'] as const) {
      extracted[kind] = await extract(kind, manifest.tarballs[kind]);
    }
  }, 60_000);

  afterAll(async () => {
    if (work) await rm(work, { recursive: true, force: true, maxRetries: 5 });
  });

  it('writes three tarballs and a manifest, and removes the staging area', async () => {
    expect(manifest.platform).toBe(platform);
    for (const file of Object.values(manifest.tarballs)) {
      expect(file).toMatch(/\.tgz$/);
      expect(await exists(path.join(work, 'out', file))).toBe(true);
    }
    expect(new Set(Object.values(manifest.tarballs)).size).toBe(3);
    expect(await exists(path.join(work, 'out', 'stage'))).toBe(false);
  });

  it('puts a working binary in the platform package', async () => {
    const p = await pkg('platform');
    const [os, cpu] = platform.split('-');
    expect(p.name).toBe(`@firescale/cli-${platform}`);
    expect(p.version).toBe(manifest.version);
    expect(p.os).toEqual([os]);
    expect(p.cpu).toEqual([cpu]);
    const binary = path.join(
      extracted['platform'] ?? '',
      'bin',
      process.platform === 'win32' ? 'fscale.exe' : 'fscale',
    );
    expect(await exists(binary)).toBe(true);
    const { stdout } = await run(binary, ['--version']);
    expect(stdout.trim()).toBe(manifest.version);
  });

  it('builds the firescale package with exact versions and all files', async () => {
    const p = await pkg('firescale');
    const text = await readFile(path.join(extracted['firescale'] ?? '', 'package.json'), 'utf8');
    expect(text).not.toContain('workspace:');
    expect(p.name).toBe('firescale');
    expect(p.version).toBe(manifest.version);
    expect(Object.keys(p.bin ?? {}).sort()).toEqual(['firescale', 'fscale']);
    expect(p.dependencies?.['@firescale/sdk']).toBe(manifest.version);
    expect(p.optionalDependencies).toEqual(
      Object.fromEntries(PLATFORMS.map((key) => [`@firescale/cli-${key}`, manifest.version])),
    );
    for (const file of [
      'bin/fscale.js',
      'dist/host.js',
      'dist/index.js',
      'schemas/check-output.schema.json',
      'schemas/verify-output.schema.json',
    ]) {
      expect(await exists(path.join(extracted['firescale'] ?? '', file)), file).toBe(true);
    }
  });

  it('builds a public sdk package', async () => {
    const p = await pkg('sdk');
    expect(p.name).toBe('@firescale/sdk');
    expect(p.private).toBeUndefined();
    expect(p.version).toBe(manifest.version);
    expect(await exists(path.join(extracted['sdk'] ?? '', 'dist', 'index.js'))).toBe(true);
  });
});
