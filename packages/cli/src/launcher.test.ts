import { execFile, spawnSync } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import {
  BIN,
  ENFORCED_CONFIG,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_ENV,
  onlyEnvRules,
  REPO,
} from './test-helpers.js';

const LAUNCHER = path.join(REPO, 'packages', 'cli', 'bin', 'fscale.js');
const CUSTOM_PACK = fileURLToPath(new URL('../test-fixtures/custom-pack/', import.meta.url));

const fixtures = fixtureSet();
afterEach(async () => {
  await fixtures.cleanup();
});

interface Result {
  code: number;
  stdout: string;
  stderr: string;
}

/** Runs the JS launcher. FSCALE_NODE_HOST is removed so the launcher has to set it. */
function launch(
  argv: string[],
  cwd: string = REPO,
  env: Record<string, string> = { FSCALE_BINARY: BIN },
): Promise<Result> {
  const base = { ...process.env };
  delete base['FSCALE_NODE_HOST'];
  delete base['FSCALE_BINARY'];
  return new Promise((resolve, reject) => {
    execFile(
      process.execPath,
      [LAUNCHER, ...argv],
      { cwd, encoding: 'utf8', windowsHide: true, env: { ...base, ...env } },
      (error, stdout, stderr) => {
        if (error && typeof error.code !== 'number') {
          reject(new Error(`Could not run the launcher: ${error.message}`, { cause: error }));
          return;
        }
        resolve({ code: error ? (error.code as number) : 0, stdout, stderr });
      },
    );
  });
}

/** Makes `@firescale/sdk` and `firescale` resolvable from the fixture. */
async function linkPackages(repo: FixtureRepo): Promise<void> {
  const modules = path.join(repo.root, 'node_modules');
  await mkdir(path.join(modules, '@firescale'), { recursive: true });
  // 'junction' works on Windows without admin rights; other platforms ignore the type.
  await symlink(
    path.join(REPO, 'packages', 'sdk'),
    path.join(modules, '@firescale', 'sdk'),
    'junction',
  );
  await symlink(path.join(REPO, 'packages', 'cli'), path.join(modules, 'firescale'), 'junction');
}

describe('npm launcher', () => {
  it('prints the version and exits 0', async () => {
    const pkg = JSON.parse(
      await readFile(path.join(REPO, 'packages', 'cli', 'package.json'), 'utf8'),
    ) as { version: string };
    const result = await launch(['--version']);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe(`${pkg.version}\n`);
  });

  it('passes exit codes and output through', async () => {
    const repo = await fixtures.create({
      files: { '.env': FAKE_ENV, 'firescale.config.json': ENFORCED_CONFIG },
    });
    const failing = await launch(['check'], repo.root);
    expect(failing.code).toBe(EXIT_VIOLATIONS);
    expect(failing.stdout + failing.stderr).toContain('.env');

    const unknown = await launch(['definitely-not-a-command']);
    expect(unknown.code).toBe(EXIT_USAGE);
    expect(unknown.stderr.length).toBeGreaterThan(0);
  });

  it('exits 2 and names FSCALE_BINARY when the override does not exist', async () => {
    const result = await launch(['--version'], REPO, {
      FSCALE_BINARY: path.join(REPO, 'no-such-folder', 'fscale'),
    });
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('FSCALE_BINARY');
  });

  it('sets FSCALE_NODE_HOST so a custom pack works without the test setting it', async () => {
    const repo = await fixtures.create({
      files: {
        'README.md': '# fixture\n',
        'TODO.md': '- item\n',
        'firescale.config.json': `${JSON.stringify(
          {
            packs: ['core', './tools/rules/pack.ts'],
            rules: onlyEnvRules('evaluated', { 'sample/no-todo-file': 'enforced' }),
          },
          null,
          2,
        )}\n`,
      },
    });
    await cp(CUSTOM_PACK, path.join(repo.root, 'tools', 'rules'), { recursive: true });
    await linkPackages(repo);
    const result = await launch(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain('sample/no-todo-file');
  });
});

describe('launcher and signals', () => {
  // Signals do not exist on Windows.
  it.skipIf(process.platform === 'win32')(
    'exits with 128 + the signal number when the binary is killed by a signal',
    async () => {
      const dir = await mkdtemp(path.join(tmpdir(), 'fscale-signal-'));
      try {
        // A stand-in binary that kills itself with SIGPIPE, which Node ignores if re-raised.
        const fake = path.join(dir, 'fake-fscale');
        await writeFile(fake, '#!/bin/sh\nkill -PIPE $$\n', { mode: 0o755 });
        const result = spawnSync(process.execPath, [LAUNCHER, '--version'], {
          encoding: 'utf8',
          env: { ...process.env, FSCALE_BINARY: fake },
        });
        expect(result.status).toBe(128 + 13);
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    },
  );
});
