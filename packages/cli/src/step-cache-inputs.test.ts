// Additive inputs (ADR 0076): `verify.projects.<folder>.inputs`, and the same
// per step, name more things a step's key must hold: files git ignores, by a
// glob, and environment variables, by name. They can only add.
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { EXIT_OK, runCli, stepCalls } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const AUDIT = 'backend/audit_service';

/** The course project with `verify.projects` set, and stand-ins for its tools. */
async function courseProject(projects: Record<string, unknown>) {
  const repo = await fixtures.create({
    files: {
      ...courseProjectFiles(),
      '.gitignore': 'generated/\nbuild-out/\n',
      'firescale.config.json': JSON.stringify({ packs: ['core'], verify: { projects } }),
    },
  });
  const tools = await fixtures.create({ files: {}, track: 'no-git' });
  await writeToolShims(tools.root, { versions: true });
  const log = path.join(tools.root, 'calls.log');
  /** Runs verify and gives the steps that ran, as "<tool> ... in <folder>". */
  const verify = async (env: Record<string, string> = {}) => {
    await writeFile(log, '');
    const result = await runCli(['verify'], repo.root, {
      ...shimEnv(tools.root, log),
      FSCALE_LOG: 'cache',
      ...env,
    });
    expect(result.code, result.stdout + result.stderr).toBe(EXIT_OK);
    const calls = await stepCalls(log);
    return { result, calls, projects: projectsIn(calls) };
  };
  const put = async (file: string, text: string) => {
    await mkdir(path.dirname(path.join(repo.root, file)), { recursive: true });
    await writeFile(path.join(repo.root, file), text);
  };
  return { repo, verify, put };
}

function projectsIn(calls: string[]): string[] {
  return [...new Set(calls.map((call) => call.replace(/^.* in /, '')))].sort();
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

describe('additive inputs of a project', () => {
  it('runs the project again when an ignored file its glob names appears or changes, and no other project', async () => {
    const { verify, put } = await courseProject({
      [AUDIT]: { inputs: [{ ignored: 'generated/**' }] },
    });
    const first = await verify();
    expect(first.projects).toContain('audit_service');
    expect(first.projects.length).toBeGreaterThan(3);
    expect((await verify()).projects).toEqual([]);

    await put(`${AUDIT}/generated/schema.json`, '{"version":1}\n');
    expect((await verify()).projects).toEqual(['audit_service']);
    expect((await verify()).projects).toEqual([]);

    await put(`${AUDIT}/generated/schema.json`, '{"version":2}\n');
    expect((await verify()).projects).toEqual(['audit_service']);

    // A deeper file, and the old bytes again.
    await put(`${AUDIT}/generated/deep/more.json`, '{}\n');
    expect((await verify()).projects).toEqual(['audit_service']);
    expect((await verify()).projects).toEqual([]);

    // An ignored file the glob does not name, and one of another project.
    await put(`${AUDIT}/build-out/x.txt`, 'x\n');
    await put('backend/auth_service/generated/schema.json', '{}\n');
    expect((await verify()).projects).toEqual([]);
  });

  it('runs the project again when a variable it names changes, is set or is unset', async () => {
    const { verify } = await courseProject({
      [AUDIT]: { inputs: [{ env: 'AUDIT_API_URL' }] },
    });
    await verify({ AUDIT_API_URL: 'http://one' });
    expect((await verify({ AUDIT_API_URL: 'http://one' })).projects).toEqual([]);
    expect((await verify({ AUDIT_API_URL: 'http://two' })).projects).toEqual(['audit_service']);
    expect((await verify({ AUDIT_API_URL: 'http://two' })).projects).toEqual([]);
    expect((await verify()).projects).toEqual(['audit_service']);
    // Set to nothing is not the same as unset.
    expect((await verify({ AUDIT_API_URL: '' })).projects).toEqual(['audit_service']);
    // A variable nothing names changes nothing.
    expect((await verify({ AUDIT_API_URL: '', OTHER_URL: 'x' })).projects).toEqual([]);
  });

  it("keys only the step that names the input when it is the step's own", async () => {
    // The auth service has a lint step (ruff) and a test step (pytest).
    const { verify } = await courseProject({
      'backend/auth_service': { test: { inputs: [{ env: 'AUTH_SEED' }] } },
    });
    const auth = (calls: string[]) => calls.filter((call) => call.endsWith(' in auth_service'));
    const first = await verify({ AUTH_SEED: '1' });
    expect(auth(first.calls).length).toBeGreaterThan(1);
    const second = await verify({ AUTH_SEED: '2' });
    expect(second.projects).toEqual(['auth_service']);
    expect(auth(second.calls)).toHaveLength(1);
    expect(auth(second.calls)[0]).toContain('pytest');
  });

  it('runs a project again when its inputs are added to the config, and only that project', async () => {
    const { repo, verify } = await courseProject({});
    await verify();
    expect((await verify()).projects).toEqual([]);
    await repo.write(
      'firescale.config.json',
      JSON.stringify({
        packs: ['core'],
        verify: { projects: { [AUDIT]: { inputs: [{ env: 'AUDIT_API_URL' }] } } },
      }),
    );
    const after = await verify();
    expect(after.projects).toContain('audit_service');
    expect(after.projects).not.toContain('auth_service');
    expect(after.projects).not.toContain('ledger_service');
  });

  it('keeps no value of a named variable in any output, log or stored file', async () => {
    const { repo, verify } = await courseProject({
      [AUDIT]: { inputs: [{ env: 'AUDIT_TOKEN' }, { ignored: 'generated/**' }] },
    });
    const secret = 'sk-test-5f6a7b-never-shown';
    await mkdir(path.join(repo.root, AUDIT, 'generated'), { recursive: true });
    await writeFile(path.join(repo.root, AUDIT, 'generated', 'key.txt'), `${secret}-file\n`);
    for (const run of [1, 2]) {
      const { result } = await verify({ AUDIT_TOKEN: secret });
      expect(result.stdout, `run ${run}`).not.toContain(secret);
      expect(result.stderr, `run ${run}`).not.toContain(secret);
    }
    const stored = [
      ...(await walk(path.join(repo.root, '.firescale'))),
      ...(await walk(path.join(os.tmpdir(), 'firescale-runs'))),
    ];
    expect(stored.some((file) => file.includes(`${path.sep}steps${path.sep}`))).toBe(true);
    for (const file of stored) {
      const text = await readFile(file, 'utf8').catch((error: NodeJS.ErrnoException) => {
        if (error.code === 'ENOENT') return '';
        throw error;
      });
      expect(text, file).not.toContain(secret);
    }
  });

  it('does not keep the pass of a step that wrote a file its own glob names', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: {
            projects: {
              web: {
                test: {
                  command: `node -e "require('fs').mkdirSync('out',{recursive:true}); require('fs').appendFileSync('out/log.txt', 'ran\\n')"`,
                  inputs: [{ ignored: 'out/**' }],
                },
                lint: false,
                typecheck: false,
              },
            },
          },
        }),
        'web/package.json': JSON.stringify({ name: 'web', private: true }),
        'web/src/a.js': 'export const a = 1;\n',
        '.gitignore': 'out/\n.firescale/\n',
      },
    });
    for (const run of [1, 2]) {
      const result = await runCli(['verify'], repo.root, {});
      expect(result.code, `run ${run}: ${result.stdout}${result.stderr}`).toBe(EXIT_OK);
      expect(result.stdout).toContain(
        'Not cached: test in web passed, but the next run cannot reuse it: files it depends on changed while verify ran.',
      );
    }
    expect(await readFile(path.join(repo.root, 'web', 'out', 'log.txt'), 'utf8')).toBe(
      'ran\nran\n',
    );
  });
});

describe('the config reader and additive inputs', () => {
  const refused = async (projects: Record<string, unknown>): Promise<string> => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core'], verify: { projects } }),
        'web/package.json': '{"name":"web"}',
      },
    });
    const result = await runCli(['verify'], repo.root, {});
    expect(result.code).not.toBe(EXIT_OK);
    return result.stderr;
  };

  it('offers no way to remove an input', async () => {
    for (const entry of [
      { exclude: 'docs/**' },
      { not: 'docs/**' },
      { ignored: '!generated/**' },
      { runtime: 'node --version' },
      '!src/**',
      'src/**',
    ]) {
      const stderr = await refused({ web: { inputs: [entry] } });
      expect(stderr, JSON.stringify(entry)).toContain('verify.projects["web"].inputs');
      expect(stderr, JSON.stringify(entry)).toContain(
        'Inputs can only add to what a step\'s key holds: each entry is { "ignored": "<glob>" } or { "env": "NAME" }.',
      );
    }
  });

  it('refuses a glob that leaves the project folder, an empty one, and a name that is not a variable name', async () => {
    for (const entry of [
      { ignored: '../other/**' },
      { ignored: '/etc/**' },
      { ignored: 'a\\b' },
      { ignored: '' },
      { env: 'NOT A NAME' },
      { env: '' },
      { env: 'A=B' },
      { ignored: 'dist/**', env: 'BOTH' },
    ]) {
      const stderr = await refused({ web: { test: { inputs: [entry] } } });
      expect(stderr, JSON.stringify(entry)).toContain('verify.projects["web"].test.inputs');
    }
    expect(await refused({ web: { inputs: { ignored: 'dist/**' } } })).toContain(
      'verify.projects["web"].inputs must be a list',
    );
  });

  it('refuses inputs for a folder that verify runs no project in, where they would do nothing', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: { projects: { backend: { inputs: [{ env: 'API_URL' }] } } },
        }),
        'backend/api/pyproject.toml': '[project]\nname = "api"\n',
        'backend/api/main.py': 'app = None\n',
      },
    });
    for (const flags of [[], ['--dry-run']]) {
      const result = await runCli(['verify', ...flags], repo.root, {});
      expect(result.code, flags.join(' ')).not.toBe(EXIT_OK);
      expect(result.stderr, flags.join(' ')).toContain(
        'verify.projects["backend"] names inputs, but verify runs no project in that folder, so they would be part of no key.',
      );
    }
  });

  it('names inputs among the keys of a project step', async () => {
    expect(await refused({ web: { test: { cmd: 'x' } } })).toContain(
      'unknown key "cmd" in verify.projects["web"].test. Allowed keys: command, timeout, inputs, expected.',
    );
  });

  it('refuses inputs outside verify.projects, where no project is named', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: { test: { command: 'node -e 0', inputs: [{ env: 'X' }] } },
        }),
      },
    });
    const result = await runCli(['verify'], repo.root, {});
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stderr).toContain(
      'verify.test.inputs is not a setting: inputs are set per project, under verify.projects.<folder>',
    );
  });
});
