import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { coreVersion } from '@firescale/native';
import { describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE } from './cli.js';
import { runCli } from './test-helpers.js';

const execFileAsync = promisify(execFile);

async function readJson(url: URL): Promise<{ version: string }> {
  return JSON.parse(await readFile(url, 'utf8')) as { version: string };
}

describe('fscale --version', () => {
  it('prints the version reported by the Rust core through the binding', async () => {
    const result = await runCli(['--version']);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe(`${coreVersion()}\n`);
    expect(result.stdout).toMatch(/^\d+\.\d+\.\d+\n$/);
  });

  it('keeps the Rust core and the npm package on the same version', async () => {
    const pkg = await readJson(new URL('../package.json', import.meta.url));
    expect(coreVersion()).toBe(pkg.version);
  });

  it('works end to end from the built binary', async () => {
    const bin = fileURLToPath(new URL('../dist/bin.js', import.meta.url));
    const { stdout } = await execFileAsync(process.execPath, [bin, '--version']);
    expect(stdout.trim()).toBe(coreVersion());
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
