// Duration hints (ADR 0078, addendum; build idea 14, first step): a project
// step may say how long it is expected to take,
// `verify.projects.<folder>.<step>.expected`. `--budget` uses it only when
// no time is kept for the step on this machine. A kept time always wins.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(async () => {
  await fixtures.cleanup();
});

const MINUTE = 60_000;
const NOT_CI = { CI: undefined, GITHUB_ACTIONS: undefined };
const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);
const KEY = 'verify.projects["backend/api"].test.expected';

function nodeScript(file: string): string {
  return `"${process.execPath}" ${file}`;
}

/** A step that appends its name to `ran.log` in the repo root, then passes. */
function logs(name: string): string {
  return `require('fs').appendFileSync('../../ran.log', '${name}\\n');\n`;
}

/** A repo with one project, backend/api, whose test says what `test` says. */
async function repoWith(test: Record<string, unknown>) {
  return fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: {
          lint: false,
          typecheck: false,
          test: false,
          projects: {
            'backend/api': {
              lint: nodeScript('lint.js'),
              test: { command: nodeScript('test.js'), ...test },
            },
          },
        },
      }),
      '.gitignore': 'ran.log\n',
      'backend/api/lint.js': logs('lint'),
      'backend/api/test.js': logs('test'),
    },
  });
}

async function ran(root: string): Promise<string[]> {
  return (await readFile(path.join(root, 'ran.log'), 'utf8').catch(() => ''))
    .split(/\r?\n/)
    .filter(Boolean);
}

/** Writes a kept time for backend/api's test, as an earlier run would have. */
async function keepTestTime(root: string, minutes: number): Promise<void> {
  await mkdir(path.join(root, '.firescale'), { recursive: true });
  await writeFile(
    path.join(root, '.firescale', 'step-times.json'),
    JSON.stringify({
      format: 1,
      steps: [
        {
          project: 'backend/api',
          step: 'test',
          command: nodeScript('test.js'),
          durationMs: minutes * MINUTE,
          finished: true,
          ranAt: 1,
        },
      ],
    }),
  );
}

interface NotRun {
  step: string;
  project?: string;
  reason: string;
  lastKnownMs?: number;
  lastKnownFinished?: boolean;
  expectedMs?: number;
}

interface Report {
  schemaVersion: number;
  result: string;
  notRun: NotRun[];
  steps: (NotRun & { name: string; status: string; detail: string })[];
}

async function report(json: string): Promise<Report> {
  const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as object;
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  check(data);
  expect(check.errors ?? []).toEqual([]);
  return data as Report;
}

describe("verify --budget uses a step's expected time when no time is kept for it", () => {
  const said = `it is expected to take 1h 40m (${KEY}) and has no kept time`;

  it('leaves the step out, names the setting, and never reads OK', async () => {
    const repo = await repoWith({ expected: '1h 40m' });
    const result = await runCli(['verify', '--budget', '30m', '--progress'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout.split('\n')[0]).toBe(
      'verify: PARTIAL - 2 of 3 steps ran, 1 not run (over the --budget of 30m)',
    );
    expect(result.stdout).toContain(`NOT RUN  over the budget of 30m: ${said}\n`);
    expect(result.stdout).toContain(
      `\nNot run: test in backend/api was not started, because ${said} and the budget is 30m (--budget). This run says nothing about it. To run it in backend/api: ${nodeScript('test.js')}. Once it passes in a run of fscale verify without --budget, its kept time is used in place of the expected one.\n`,
    );
    expect(result.stderr).toContain(
      `[verify] test in backend/api: NOT RUN: ${said}, and the budget is 30m (--budget)\n`,
    );
    expect(await ran(repo.root)).toEqual(['lint']);
  });

  it('says so in JSON: the expected time, and no last known time', async () => {
    const repo = await repoWith({ expected: 6000 });
    const result = await runCli(['verify', '--budget', '30m', '--json'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    const json = await report(result.stdout);
    expect(json.schemaVersion).toBe(9);
    expect(json.result).toBe('partial');
    expect(json.notRun).toEqual([
      {
        step: 'test',
        project: 'backend/api',
        reason: `over the budget of 30m: ${said}`,
        expectedMs: 100 * MINUTE,
      },
    ]);
    const test = json.steps.find((step) => step.name === 'test' && step.project === 'backend/api');
    expect([test?.status, test?.expectedMs, test?.lastKnownMs]).toEqual([
      'over-budget',
      100 * MINUTE,
      undefined,
    ]);
  });

  it('lists the step in a dry run', async () => {
    const repo = await repoWith({ expected: '1h 40m' });
    const dry = await runCli(['verify', '--dry-run', '--budget', '30m'], repo.root, NOT_CI);
    expect(dry.code).toBe(EXIT_OK);
    expect(dry.stdout).toContain(
      `\nWith --budget 30m, 1 step would not be run:\n  test in backend/api: ${said}\n`,
    );
  });

  it('runs the step when its expected time is within the budget', async () => {
    const repo = await repoWith({ expected: '20m' });
    const result = await runCli(['verify', '--budget', '30m'], repo.root, NOT_CI);
    expect(result.stdout.split('\n')[0]).toBe('verify: OK');
    expect(await ran(repo.root)).toEqual(['lint', 'test']);
  });

  it('does nothing without --budget: the step runs, and is not announced as slow', async () => {
    const repo = await repoWith({ expected: '1h 40m' });
    const result = await runCli(['verify', '--progress'], repo.root, NOT_CI);
    expect(result.stdout.split('\n')[0]).toBe('verify: OK');
    expect(result.stderr).not.toContain('slow step');
    expect(result.stderr).not.toContain('expected');
    expect(await ran(repo.root)).toEqual(['lint', 'test']);
  });
});

describe('a kept time always wins over the expected time', () => {
  it('runs a step whose kept time is within the budget, however long it is expected to take', async () => {
    const repo = await repoWith({ expected: '1h 40m' });
    await keepTestTime(repo.root, 10);
    const result = await runCli(['verify', '--budget', '30m'], repo.root, NOT_CI);
    expect(result.stdout.split('\n')[0]).toBe('verify: OK');
    expect(await ran(repo.root)).toEqual(['lint', 'test']);
  });

  it('leaves out a step whose kept time is over the budget, however short it is expected to be', async () => {
    const repo = await repoWith({ expected: '1m' });
    await keepTestTime(repo.root, 100);
    const result = await runCli(['verify', '--budget', '30m', '--json'], repo.root, NOT_CI);
    const json = await report(result.stdout);
    expect(json.notRun).toEqual([
      {
        step: 'test',
        project: 'backend/api',
        reason: 'over the budget of 30m: it took 1h 40m when it last ran',
        lastKnownMs: 100 * MINUTE,
        lastKnownFinished: true,
      },
    ]);
    expect(await ran(repo.root)).toEqual(['lint']);
  });

  it('after a pass without --budget, the kept time is used and the hint no longer is', async () => {
    const repo = await repoWith({ expected: '1h 40m' });
    expect((await runCli(['verify', '--no-cache'], repo.root, NOT_CI)).code).toBe(EXIT_OK);
    const result = await runCli(['verify', '--budget', '30m', '--no-cache'], repo.root, NOT_CI);
    expect(result.stdout.split('\n')[0]).toBe('verify: OK');
    expect(await ran(repo.root)).toEqual(['lint', 'test', 'lint', 'test']);
  });
});

describe('the setting is checked', () => {
  it.each([
    [false, 'must be a time such as "10m", "90s" or "1h 30m", or a number of seconds.'],
    [0, 'must be longer than zero.'],
    ['soon', 'must be a time such as "10m", "90s" or "1h 30m", or a number of seconds.'],
  ])('refuses %o', async (value, message) => {
    const repo = await repoWith({ expected: value });
    const result = await runCli(['verify', '--dry-run'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain(`${KEY} ${message}`);
  });

  it('is set per project, not for the whole repo', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: { test: { command: 'echo t', expected: '1h' } },
        }),
      },
    });
    const result = await runCli(['verify', '--dry-run'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain(
      'verify.test.expected is not a setting: an expected time is set per project, under verify.projects.<folder>.test.expected.',
    );
  });
});
