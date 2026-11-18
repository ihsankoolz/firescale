// The step cache and env files (ADR 0074): the bytes of `.env` and
// `.env.local` at the repo root and in the unit's folder are part of a
// step's key, though git ignores them. Only a hash of them is used: no value
// is printed, logged or stored.
import { readdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { EXIT_OK, runCli, stepCalls } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

/** A node one-liner that appends `word` to `steps.log`, which git ignores. */
function logs(word: string): string {
  return `node -e "require('fs').appendFileSync('steps.log', '${word}\\n')"`;
}

/** A repo whose three steps only log that they ran, and which ignores its env files. */
async function loggingRepo() {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: logs('lint'), typecheck: logs('typecheck'), test: logs('test') },
      }),
      'src/a.js': 'export const a = 1;\n',
      '.gitignore': 'steps.log\n.env\n.env.local\n.firescale/\n',
    },
  });
  const log = path.join(repo.root, 'steps.log');
  const ran = async (): Promise<number> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean).length;
  const verify = (...flags: string[]) =>
    runCli(['verify', ...flags], repo.root, { FSCALE_LOG: 'cache' });
  return { repo, ran, verify };
}

/** Every file below `dir`. */
async function walk(dir: string): Promise<string[]> {
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
  const files: string[] = [];
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(full)));
    else if (entry.isFile()) files.push(full);
  }
  return files;
}

describe('the step cache and env files', () => {
  for (const name of ['.env', '.env.local']) {
    it(`runs the steps again when the ignored ${name} at the repo root appears, changes or goes`, async () => {
      const { repo, ran, verify } = await loggingRepo();
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(3);
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(3);

      // It appears.
      await writeFile(path.join(repo.root, name), 'FEATURE_FLAG=off\n');
      const appeared = await verify();
      expect(appeared.code).toBe(EXIT_OK);
      expect(await ran()).toBe(6);
      // Unchanged: from the cache.
      const same = await verify();
      expect(same.stdout.split('\n')[0]).toBe(
        'verify: OK (3 of 4 steps passed from the cache, not run now)',
      );
      expect(await ran()).toBe(6);

      // One byte changes.
      await writeFile(path.join(repo.root, name), 'FEATURE_FLAG=on\n');
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(9);
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(9);

      // The old bytes again: the pass kept for them is found.
      await writeFile(path.join(repo.root, name), 'FEATURE_FLAG=off\n');
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(9);

      // It goes: the first run's passes, made with no such file, are found.
      await rm(path.join(repo.root, name));
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(9);
    });
  }

  it("keys a project on its own folder's env files and the root's, not on another project's", async () => {
    const repo = await fixtures.create({
      files: { ...courseProjectFiles(), '.gitignore': 'generated/\n.env\n.env.local\n' },
    });
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    const log = path.join(tools.root, 'calls.log');
    const env = shimEnv(tools.root, log);
    const projects = async (): Promise<string[]> => {
      const calls = await stepCalls(log);
      await writeFile(log, '');
      return [...new Set(calls.map((call) => call.replace(/^.* in /, '')))].sort();
    };
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);
    const all = await projects();
    expect(all).toContain('audit_service');
    expect(all).toContain('frontend');
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);
    expect(await projects()).toEqual([]);

    // One service's own file: only that service runs again.
    await repo.write('backend/audit_service/.env', 'DATABASE_URL=postgres://one\n');
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);
    expect(await projects()).toEqual(['audit_service']);
    await repo.write('backend/audit_service/.env.local', 'DATABASE_URL=postgres://two\n');
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);
    expect(await projects()).toEqual(['audit_service']);
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);
    expect(await projects()).toEqual([]);

    // The root's file: every project runs again.
    await repo.write('.env', 'DATABASE_URL=postgres://three\n');
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);
    expect(await projects()).toEqual(all);
  });

  it('keeps no value of an env file in any output, log or stored file', async () => {
    const { repo, ran, verify } = await loggingRepo();
    const secret = 'sk-test-1a2b3c4d5e6f-never-shown';
    await writeFile(path.join(repo.root, '.env'), `API_KEY=${secret}\n`);
    await writeFile(path.join(repo.root, '.env.local'), `OTHER_KEY=${secret}-local\n`);
    const first = await verify('--progress');
    const second = await verify('--json');
    const third = await verify('--no-cache');
    expect(await ran()).toBe(6);
    for (const result of [first, second, third]) {
      expect(result.code).toBe(EXIT_OK);
      expect(result.stdout).not.toContain(secret);
      expect(result.stderr).not.toContain(secret);
    }
    // The cache log names the steps, so it was on.
    expect(first.stderr).toContain('cache: stored test');
    const stored = [
      ...(await walk(path.join(repo.root, '.firescale'))),
      ...(await walk(path.join(os.tmpdir(), 'firescale-runs'))),
    ];
    expect(stored.some((file) => file.includes(`${path.sep}steps${path.sep}`))).toBe(true);
    for (const file of stored) {
      // Other fscale runs share the temp folder: a record can be gone by now.
      const text = await readFile(file, 'utf8').catch((error: NodeJS.ErrnoException) => {
        if (error.code === 'ENOENT') return '';
        throw error;
      });
      expect(text, file).not.toContain(secret);
    }
    expect(await readFile(path.join(repo.root, '.env'), 'utf8')).toBe(`API_KEY=${secret}\n`);
  });

  // Making a symlink to a file needs a privilege on Windows.
  it.skipIf(process.platform === 'win32')(
    'keys an env file that is a link on where it points and on the bytes there, and still caches',
    async () => {
      const { repo, ran, verify } = await loggingRepo();
      const outside = await fixtures.create({ files: { 'real.env': 'A=1\n' }, track: 'no-git' });
      await symlink(path.join(outside.root, 'real.env'), path.join(repo.root, '.env'));
      const first = await verify();
      expect(first.code).toBe(EXIT_OK);
      expect(first.stdout).not.toContain('Not cached');
      expect(await ran()).toBe(3);
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(3);
      // The bytes it points to change.
      await writeFile(path.join(outside.root, 'real.env'), 'A=2\n');
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(6);
      // It points somewhere else, at the same bytes.
      await writeFile(path.join(outside.root, 'other.env'), 'A=2\n');
      await rm(path.join(repo.root, '.env'));
      await symlink(path.join(outside.root, 'other.env'), path.join(repo.root, '.env'));
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(9);
      expect((await verify()).code).toBe(EXIT_OK);
      expect(await ran()).toBe(9);
    },
  );

  // A junction on Windows, which needs no privilege; a symbolic link elsewhere.
  it('still caches when .env is a link to a folder, a virtual environment of that name', async () => {
    const { repo, ran, verify } = await loggingRepo();
    const outside = await fixtures.create({
      files: { 'pyvenv.cfg': 'home = x\n' },
      track: 'no-git',
    });
    await symlink(outside.root, path.join(repo.root, '.env'), 'junction');
    const first = await verify();
    expect(first.code).toBe(EXIT_OK);
    expect(first.stdout).not.toContain('Not cached');
    expect((await verify()).code).toBe(EXIT_OK);
    expect(await ran()).toBe(3);
  });

  it('does not keep the pass of a step that wrote an env file while it ran', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: {
            test: `node -e "require('fs').appendFileSync('.env', 'RUN=1\\n'); require('fs').appendFileSync('steps.log', 'test\\n')"`,
          },
        }),
        'src/a.js': 'export const a = 1;\n',
        '.gitignore': 'steps.log\n.env\n.env.local\n.firescale/\n',
      },
    });
    for (const run of [1, 2]) {
      const result = await runCli(['verify'], repo.root, {});
      expect(result.code, `run ${run}`).toBe(EXIT_OK);
      expect(result.stdout).toContain(
        'Not cached: test passed, but the next run cannot reuse it: files it depends on changed while verify ran.',
      );
    }
    expect(await readFile(path.join(repo.root, 'steps.log'), 'utf8')).toBe('test\ntest\n');
  });
});
