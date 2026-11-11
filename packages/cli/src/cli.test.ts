import { readdir, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

async function readVersion(file: URL): Promise<string> {
  const pkg = JSON.parse(await readFile(file, 'utf8')) as { version: string };
  return pkg.version;
}

describe('fscale --version', () => {
  it('prints exactly the version in packages/cli/package.json', async () => {
    const version = await readVersion(new URL('../package.json', import.meta.url));
    const result = await runCli(['--version']);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe(`${version}\n`);
    expect(result.stdout).toMatch(/^\d+\.\d+\.\d+\n$/);
  });

  it('keeps every package.json and the Cargo workspace on the same version', async () => {
    const expected = await readVersion(new URL('../package.json', import.meta.url));
    const manifests = [new URL('../../../package.json', import.meta.url)];
    const packagesDir = new URL('../../', import.meta.url);
    for (const entry of await readdir(packagesDir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const manifest = new URL(`${entry.name}/package.json`, packagesDir);
      try {
        await readFile(manifest, 'utf8');
        manifests.push(manifest);
      } catch {
        // A folder without a package.json is not a package.
      }
    }
    expect(manifests.length).toBeGreaterThan(1);
    for (const manifest of manifests) {
      expect(await readVersion(manifest), fileURLToPath(manifest)).toBe(expected);
    }

    const cargo = await readFile(new URL('../../../Cargo.toml', import.meta.url), 'utf8');
    const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargo)?.[1];
    const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(section ?? '')?.[1];
    expect(cargoVersion).toBe(expected);
  });
});

describe('fscale usage', () => {
  it('prints help with no arguments', async () => {
    const result = await runCli([]);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
  });

  it('rejects unknown options with a next step', async () => {
    const result = await runCli(['--nope']);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('fscale --help');
  });

  it('rejects unknown commands with a next step', async () => {
    const result = await runCli(['frobnicate']);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('unknown command "frobnicate"');
    expect(result.stderr).toContain('fscale --help');
  });
});
