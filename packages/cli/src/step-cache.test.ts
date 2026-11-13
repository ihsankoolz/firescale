import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { EXIT_OK, EXIT_VIOLATIONS, runCli, stepCalls } from './test-helpers.js';

// The local step cache (M18, ADR 0036).

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);

interface Step {
  name: string;
  project?: string;
  status: string;
  durationMs: number;
  detail: string;
  cache?: { durationMs: number; ranAt: string };
}

interface Report {
  result: string;
  steps: Step[];
  cache?: { read: boolean; hits: number; savedMs: number };
}

async function validate(json: string): Promise<Report> {
  const schema: object = JSON.parse(await readFile(SCHEMA, 'utf8'));
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  check(data);
  expect(check.errors ?? []).toEqual([]);
  return data as Report;
}

/** A node one-liner that appends `word` to `steps.log`, which git ignores. */
function logs(word: string): string {
  return `node -e "require('fs').appendFileSync('steps.log', '${word}\\n')"`;
}

/** A repo whose three steps only log that they ran. */
async function loggingRepo(extra: Record<string, string> = {}) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: logs('lint'), typecheck: logs('typecheck'), test: logs('test') },
      }),
      'src/a.js': 'export const a = 1;\n',
      '.gitignore': 'steps.log\n',
      ...extra,
    },
  });
  const log = path.join(repo.root, 'steps.log');
  const ran = async (): Promise<string[]> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean);
  return { repo, ran };
}

/** The course project with stand-ins for python, npx and npm that answer --version. */
async function courseProject() {
  const repo = await fixtures.create({ files: courseProjectFiles() });
  const tools = await fixtures.create({ files: {}, track: 'no-git' });
  await writeToolShims(tools.root, { versions: true });
  const log = path.join(tools.root, 'calls.log');
  const reset = () => writeFile(log, '');
  const calls = (): Promise<string[]> => stepCalls(log);
  return {
    repo,
    calls,
    reset,
    env: (extra: Record<string, string> = {}) => ({ ...shimEnv(tools.root, log), ...extra }),
  };
}

/** The projects whose steps ran, from the stand-ins' log. */
function projectsIn(calls: string[]): string[] {
  return [...new Set(calls.map((call) => call.replace(/^.* in /, '')))].sort();
}

describe('the step cache', () => {
  it('runs no step again on a second run with no change, and says so', async () => {
    const { repo, ran } = await loggingRepo();
    const first = await runCli(['verify'], repo.root, {});
    expect(first.code).toBe(EXIT_OK);
    expect(await ran()).toEqual(['lint', 'typecheck', 'test']);
    expect(first.stdout.split('\n')[0]).toBe('verify: OK');

    const second = await runCli(['verify'], repo.root, {});
    expect(second.code).toBe(EXIT_OK);
    expect(await ran()).toEqual(['lint', 'typecheck', 'test']);
    const lines = second.stdout.split('\n');
    expect(lines[0]).toBe('verify: OK (3 of 4 steps passed from the cache, not run now)');
    for (const step of ['lint', 'typecheck', 'test']) {
      expect(second.stdout).toMatch(
        new RegExp(
          `  ${step} +cached +from the cache: it passed in \\S+ when it last ran \\(\\d{4}-\\d\\d-\\d\\d \\d\\d:\\d\\d UTC\\)`,
        ),
      );
    }
    // The check is not a cached step: it runs every time.
    expect(second.stdout).toMatch(/ {2}check +passed /);
    expect(second.stdout).toContain(
      'From the cache: 3 steps passed before on the same inputs, so they were not run again',
    );
    expect(second.stdout).toContain('Run fscale verify --no-cache to run every step.');

    const json = await validate((await runCli(['verify', '--json'], repo.root, {})).stdout);
    expect(json.cache).toMatchObject({ read: true, hits: 3 });
    for (const step of json.steps.filter((step) => step.name !== 'check')) {
      expect(step.status).toBe('passed');
      expect(step.durationMs).toBe(0);
      expect(step.cache?.ranAt).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);
    }
    expect(await ran()).toHaveLength(3);
  });

  it('--no-cache runs every step and still stores the passes', async () => {
    const { repo, ran } = await loggingRepo();
    await runCli(['verify', '--no-cache'], repo.root, {});
    const again = await runCli(['verify', '--no-cache', '--json'], repo.root, {});
    expect((await validate(again.stdout)).cache).toEqual({ read: false, hits: 0, savedMs: 0 });
    expect(await ran()).toHaveLength(6);
    const third = await runCli(['verify'], repo.root, {});
    expect(third.stdout.split('\n')[0]).toContain('3 of 4 steps passed from the cache');
    expect(await ran()).toHaveLength(6);
  });

  it('runs again after a change to a tracked, an untracked or a deleted file, and not for an ignored one', async () => {
    const { repo, ran } = await loggingRepo({ '.gitignore': 'steps.log\nout/\n' });
    const run = () => runCli(['verify'], repo.root, {});
    await run();
    await repo.write('out/build.js', 'ignored\n');
    await run();
    expect(await ran()).toHaveLength(3);
    await repo.write('src/a.js', 'export const a = 2;\n');
    await run();
    expect(await ran()).toHaveLength(6);
    await repo.write('src/new.js', 'export const b = 1;\n');
    await run();
    expect(await ran()).toHaveLength(9);
    // The same bytes again: a hit.
    await run();
    expect(await ran()).toHaveLength(9);
    // CRLF is other bytes: a miss, never a wrong hit.
    await repo.write('src/new.js', 'export const b = 1;\r\n');
    await run();
    expect(await ran()).toHaveLength(12);
  });

  it('never caches a failed step', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: { lint: false, typecheck: false, test: 'node -e "process.exit(1)"' },
        }),
      },
    });
    for (let run = 0; run < 2; run += 1) {
      const result = await runCli(['verify'], repo.root, { FSCALE_LOG: 'cache' });
      expect(result.code).toBe(EXIT_VIOLATIONS);
      expect(result.stdout.split('\n')[0]).toBe('verify: FAILED at test');
      expect(result.stderr).toContain('cache: miss test; not stored, because it did not pass');
    }
    const dir = path.join(repo.root, '.firescale', 'steps');
    expect(await readdir(dir).catch(() => [])).toEqual([]);
  });

  it('never caches a step that needs a service the repo does not start', async () => {
    const { repo, ran } = await loggingRepo({
      '.env.example': 'DATABASE_URL=postgresql://app:secret@localhost:5432/app\n',
    });
    for (let run = 0; run < 2; run += 1) {
      const result = await runCli(['verify'], repo.root, { FSCALE_LOG: 'cache' });
      expect(result.stderr).toContain(
        'cache: not cached test: it may need PostgreSQL on localhost:5432 (from .env.example (DATABASE_URL)), a service the repo does not start',
      );
    }
    expect(await ran()).toHaveLength(6);
  });

  it('says on every run, without FSCALE_LOG, why a passed step is not cached', async () => {
    const { repo } = await loggingRepo({
      '.env.example': 'DATABASE_URL=postgresql://app:secret@localhost:5432/app\n',
    });
    for (let run = 0; run < 2; run += 1) {
      const result = await runCli(['verify'], repo.root, {});
      expect(result.code).toBe(EXIT_OK);
      expect(result.stdout).toContain(
        'Not cached: test passed, but the next run cannot reuse it: it may need PostgreSQL on localhost:5432 (from .env.example (DATABASE_URL)), a service the repo does not start.',
      );
      expect(result.stdout).not.toContain('secret');
      const json = await validate((await runCli(['verify', '--json'], repo.root, {})).stdout);
      const test = json.steps.find((step) => step.name === 'test') as Step & { notCached?: string };
      expect(test.notCached).toMatch(/^it may need PostgreSQL on localhost:5432/);
    }
  });

  it('says nothing about the cache for a step that is cached', async () => {
    const { repo } = await loggingRepo();
    const first = await runCli(['verify'], repo.root, {});
    expect(first.stdout).not.toContain('Not cached');
    const json = await validate((await runCli(['verify', '--json'], repo.root, {})).stdout);
    for (const step of json.steps) expect(step).not.toHaveProperty('notCached');
  });

  it('caches a Node step started as npm.cmd beside the shell script named npm that Node installs', async () => {
    // On Windows, Node's folder holds `npm` (a shell script Windows cannot
    // run) beside `npm.cmd`. The step runs through cmd, which starts
    // npm.cmd; the version must be read the same way (ADR 0043).
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: { lint: false, typecheck: false, test: 'npm test' },
        }),
        'package.json': JSON.stringify({ name: 'web', scripts: { test: 'node -e ""' } }),
        'package-lock.json': '{"lockfileVersion":3}\n',
      },
    });
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    if (process.platform === 'win32') {
      await writeFile(path.join(tools.root, 'npm'), '#!/bin/sh\necho "this is not for Windows"\n');
    }
    const log = path.join(tools.root, 'calls.log');
    const env = { ...shimEnv(tools.root, log), FSCALE_LOG: 'cache' };
    const first = await runCli(['verify'], repo.root, env);
    expect(first.code).toBe(EXIT_OK);
    expect(first.stderr).not.toContain('could not be read');
    expect(first.stdout).not.toContain('Not cached');
    const second = await runCli(['verify'], repo.root, env);
    expect(second.stdout.split('\n')[0]).toBe(
      'verify: OK (1 of 2 steps passed from the cache, not run now)',
    );
    expect(second.stderr).toContain('cache: hit test');
    expect(await stepCalls(log)).toEqual(['npm test in ' + path.basename(repo.root)]);
  });

  it('says once when a change to the config means no kept pass applies (ADR 0043)', async () => {
    const { repo, ran } = await loggingRepo();
    await runCli(['verify'], repo.root, {});
    const same = await runCli(['verify'], repo.root, {});
    expect(same.stdout).not.toContain('changed since the last run');
    const config = JSON.parse(
      await readFile(path.join(repo.root, 'firescale.config.json'), 'utf8'),
    );
    config.verify.failOnSkipped = false;
    await repo.write('firescale.config.json', JSON.stringify(config));
    const changed = await runCli(['verify'], repo.root, {});
    const note =
      "firescale.config.json changed since the last run, and it is part of every step's cache key, so no pass kept before applies: every step runs again this time.";
    expect(changed.stdout.split(note)).toHaveLength(2);
    expect(await ran()).toHaveLength(6);
    const after = await runCli(['verify'], repo.root, {});
    expect(after.stdout).not.toContain(note);
    expect(await ran()).toHaveLength(6);
  });

  it('caches a step whose service the repo starts with Compose', async () => {
    const { repo, ran } = await loggingRepo({
      '.env.example': 'DATABASE_URL=postgresql://app:secret@localhost:5432/app\n',
      'compose.yaml': 'services:\n  db:\n    image: postgres:16\n    ports:\n      - "5432:5432"\n',
    });
    await runCli(['verify'], repo.root, {});
    await runCli(['verify'], repo.root, {});
    expect(await ran()).toHaveLength(3);
  });

  it('keys on allowlisted variables only, and writes no value of the environment', async () => {
    const { repo, ran } = await loggingRepo();
    const secret = 'sk-live-0123456789-do-not-store';
    await runCli(['verify'], repo.root, { MY_TOKEN: secret, NODE_ENV: 'test' });
    expect(await ran()).toHaveLength(3);
    // A variable outside the allowlist changes nothing.
    await runCli(['verify'], repo.root, { MY_TOKEN: 'another', NODE_ENV: 'test' });
    expect(await ran()).toHaveLength(3);
    // An allowlisted one runs every step again.
    await runCli(['verify'], repo.root, { MY_TOKEN: secret, NODE_ENV: 'production' });
    expect(await ran()).toHaveLength(6);
    const dir = path.join(repo.root, '.firescale', 'steps');
    const entries = await readdir(dir);
    expect(entries.length).toBeGreaterThan(0);
    for (const entry of entries) {
      const text = await readFile(path.join(dir, entry), 'utf8');
      expect(text).not.toContain(secret);
      expect(text).not.toContain('production');
    }
  });

  it('does not store a step that changes a file it depends on', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: {
            lint: false,
            typecheck: false,
            test: `node -e "require('fs').appendFileSync('src/out.js', '1')"`,
          },
        }),
        'src/out.js': '',
      },
    });
    const result = await runCli(['verify'], repo.root, { FSCALE_LOG: 'cache' });
    expect(result.code).toBe(EXIT_OK);
    expect(result.stderr).toContain(
      'cache: not stored test: files it depends on changed while verify ran',
    );
  });
});

describe('the step cache on the course project', () => {
  it('runs again only the projects a change reaches', async () => {
    const { repo, calls, reset, env } = await courseProject();
    const run = (extra: Record<string, string> = {}) => runCli(['verify'], repo.root, env(extra));
    const first = await run();
    expect(first.code).toBe(EXIT_OK);
    const everything = projectsIn(await calls());
    expect(everything).toEqual([
      'audit_service',
      'auth_service',
      'frontend',
      'ledger_service',
      'market_service',
      'realtime_service',
    ]);

    await reset();
    const second = await run();
    expect(await calls()).toEqual([]);
    expect(second.stdout.split('\n')[0]).toMatch(
      /passed from the cache, not run now|passed from the cache, \d+ skipped/,
    );

    // A change in one service runs that service only.
    await reset();
    await repo.write('backend/audit_service/main.py', 'app = 1\n');
    await run();
    expect(projectsIn(await calls())).toEqual(['audit_service']);

    // A change in the shared folder runs the services that import it.
    await reset();
    await repo.write('backend/shared/db.py', 'URL = "x"\n');
    await run();
    expect(projectsIn(await calls())).toEqual(['auth_service', 'ledger_service', 'market_service']);

    // The frontend's lockfile runs the frontend.
    await reset();
    await repo.write('frontend/package-lock.json', '{"lockfileVersion":3}\n');
    await run();
    expect(projectsIn(await calls())).toEqual(['frontend']);

    // A new version of a tool runs every step that uses it.
    await reset();
    await run({ SHIM_VERSION: '2.0.0' });
    expect(projectsIn(await calls())).toEqual(everything);

    // An allowlisted variable runs everything; another variable nothing.
    await reset();
    await run({ SHIM_VERSION: '2.0.0', SOME_OTHER_SETTING: 'x' });
    expect(await calls()).toEqual([]);
    await run({ SHIM_VERSION: '2.0.0', PYTHONPATH: 'somewhere' });
    expect(projectsIn(await calls())).toEqual(everything);

    // A screenshot or a README no project owns is in no project's key
    // (ADR 0034, ADR 0043): nothing runs.
    await reset();
    await repo.write('image.png', 'not really a png\n');
    await repo.write('README.md', '# Course project\n\nMore words.\n');
    await run({ SHIM_VERSION: '2.0.0', PYTHONPATH: 'somewhere' });
    expect(await calls()).toEqual([]);
    // A file directly in the repo root, where settings live, runs everything.
    await reset();
    await repo.write('ruff.toml', 'line-length = 90\n');
    await run({ SHIM_VERSION: '2.0.0', PYTHONPATH: 'somewhere' });
    expect(projectsIn(await calls())).toEqual(everything);
    // About ten verify runs of a six-project repo: 18.6 s on its own on a
    // Windows runner, so the default 30 s left no room for a busy one.
  }, 90_000);
});

describe('the step cache keys a step on the files as they are when it starts', () => {
  it('runs a step again when an earlier step changed its files', async () => {
    const rewrite = `node -e "const fs = require('fs'); if (fs.existsSync('c/trigger.txt')) fs.writeFileSync('b/a.txt', 'broken')"`;
    const check = `node -e "process.exit(require('fs').readFileSync('a.txt', 'utf8').includes('broken') ? 1 : 0)"`;
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: {
            lint: rewrite,
            typecheck: false,
            test: false,
            projects: {
              b: { lint: false, typecheck: false, test: check },
              c: { lint: false, typecheck: false, test: false },
            },
          },
        }),
        'b/package.json': '{"name":"b"}\n',
        'b/a.txt': 'ok\n',
        'c/package.json': '{"name":"c"}\n',
        'c/index.js': 'export {};\n',
      },
    });
    expect((await runCli(['verify'], repo.root)).code).toBe(EXIT_OK);
    const warm = await runCli(['verify'], repo.root);
    expect(warm.stdout.split('\n')[0]).toContain('passed from the cache');
    // The root's lint runs again (a file changed in c) and rewrites b/a.txt:
    // b's test must run on the new content, not come from the cache.
    await repo.write('c/trigger.txt', 'go\n');
    const after = await runCli(['verify'], repo.root, { FSCALE_LOG: 'cache' });
    expect(after.stdout.split('\n')[0]).toContain('FAILED at test in b');
    expect(after.stderr).toContain('cache: miss test in b; not stored, because it did not pass');
    expect(after.code).toBe(EXIT_VIOLATIONS);
  });
});
