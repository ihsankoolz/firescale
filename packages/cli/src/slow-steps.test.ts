// Slow steps (ADR 0057): verify keeps how long each step took, warns before
// a step that was slow the last time, keeps a time for a step that was
// stopped or timed out, and `--budget` leaves out steps slower than a time.
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import {
  BIN,
  commitAll,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  HOST,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
const started: ChildProcess[] = [];
afterEach(async () => {
  for (const child of started.splice(0)) child.kill('SIGKILL');
  await fixtures.cleanup();
});

const WINDOWS = process.platform === 'win32';
const EXIT_STOPPED = 130;

const MINUTE = 60_000;

/** A command line that runs a Node script file in the step's folder. */
function nodeScript(file: string): string {
  return `"${process.execPath}" ${file}`;
}

/** A repo whose verify runs only what `verify` says. */
async function repoWith(verify: Record<string, unknown>, files: Record<string, string> = {}) {
  return fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({ packs: ['core'], verify }),
      ...files,
    },
  });
}

interface TimeRecord {
  project?: string;
  step: string;
  command: string;
  durationMs: number;
  finished: boolean;
  ranAt: number;
}

interface TimesFile {
  format: number;
  steps: TimeRecord[];
}

function timesPath(root: string): string {
  return path.join(root, '.firescale', 'step-times.json');
}

async function readTimes(root: string): Promise<TimesFile> {
  return JSON.parse(await readFile(timesPath(root), 'utf8')) as TimesFile;
}

/** Rewrites the kept time of one step, as if it had taken that long. */
async function setTime(
  root: string,
  step: string,
  change: Partial<TimeRecord>,
  project?: string,
): Promise<void> {
  const file = await readTimes(root);
  const record = file.steps.find((r) => r.step === step && r.project === project);
  if (!record) throw new Error(`no kept time for ${step}`);
  Object.assign(record, change);
  await writeFile(timesPath(root), JSON.stringify(file, null, 2));
}

/** The progress lines of a run, without the prefix. */
function progressLines(stderr: string): string[] {
  return stderr
    .split(/\r?\n/)
    .filter((line) => line.startsWith('[verify] '))
    .map((line) => line.slice('[verify] '.length));
}

// Verify gives its commands CI=1, and this repo's own verify runs these
// tests: a test of a laptop run removes the variables that mean CI.
const NOT_CI = { CI: undefined, GITHUB_ACTIONS: undefined };

describe('the time each step took is kept, and a slow step is announced', () => {
  it('warns before a step that took more than 5 minutes last time, also after its files changed', async () => {
    const repo = await repoWith(
      { lint: 'echo lint-ran', typecheck: false, test: nodeScript('test.js') },
      { 'test.js': "console.log('one');\n" },
    );
    const first = await runCli(['verify', '--progress'], repo.root, NOT_CI);
    expect(first.code).toBe(EXIT_OK);
    // Nothing is known yet, so nothing is announced.
    expect(first.stderr).not.toContain('slow step');
    const kept = await readTimes(repo.root);
    expect(kept.format).toBe(1);
    expect(kept.steps.map((r) => [r.step, r.command, r.finished])).toEqual([
      ['lint', 'echo lint-ran', true],
      ['test', nodeScript('test.js'), true],
    ]);
    expect(kept.steps.every((r) => r.project === undefined && r.durationMs >= 0)).toBe(true);

    // As on the course project: one step takes 1h 40m, another 2 minutes.
    await setTime(repo.root, 'test', { durationMs: 100 * MINUTE });
    await setTime(repo.root, 'lint', { durationMs: 2 * MINUTE });
    // The files change, so the step cache's key changes and the steps run.
    await writeFile(path.join(repo.root, 'test.js'), "console.log('two');\n");
    const second = await runCli(['verify', '--progress'], repo.root, NOT_CI);
    expect(second.code).toBe(EXIT_OK);
    const lines = progressLines(second.stderr);
    const warning =
      'test: slow step: it took 1h 40m when it last ran. To leave out steps this slow, run fscale verify --budget <time>';
    expect(lines).toContain(warning);
    // Said once, before the step starts.
    expect(lines.filter((line) => line.includes('slow step'))).toEqual([warning]);
    expect(lines.indexOf(warning)).toBe(
      lines.indexOf(`test: started: ${nodeScript('test.js')}`) - 1,
    );
    // The step ran and passed, so its real time replaces the old one.
    const after = await readTimes(repo.root);
    expect(after.steps.find((r) => r.step === 'test')?.durationMs).toBeLessThan(MINUTE);
    // Progress only: the summary on stdout says nothing about it.
    expect(second.stdout).not.toContain('slow step');
  });

  it('says so when the kept times are there but cannot be read, and runs the step', async () => {
    const repo = await repoWith(
      { lint: false, typecheck: false, test: nodeScript('test.js') },
      { 'test.js': "console.log('one');\n" },
    );
    // A folder where the file should be: it is there, and is not a file.
    await mkdir(timesPath(repo.root), { recursive: true });
    const result = await runCli(['verify', '--budget', '30m', '--progress'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    const lines = progressLines(result.stderr);
    const said = lines.findIndex((line) => line.includes('could not read'));
    expect(lines[said]).toMatch(
      /^test: could not read \.firescale\/step-times\.json \(.+\), so how long this step took the last time is not known: it runs$/,
    );
    expect(lines[said + 1]).toBe(`test: started: ${nodeScript('test.js')}`);
    // What is there is left alone, and nothing is left beside it.
    expect(await readdir(timesPath(repo.root))).toEqual([]);
    const beside = await readdir(path.join(repo.root, '.firescale'));
    expect(beside.filter((name) => name.endsWith('.tmp') || name.endsWith('.lock'))).toEqual([]);
  });

  it('says in the summary and in --json, with no progress shown, that --budget could not read the kept times', async () => {
    const repo = await repoWith(
      { lint: false, typecheck: false, test: nodeScript('test.js') },
      { 'test.js': "console.log('one');\n" },
    );
    await mkdir(timesPath(repo.root), { recursive: true });
    const warning =
      /^could not read \.firescale\/step-times\.json \(.+\), so --budget 30m left out no step for its kept time: a step whose time could not be read ran\. Delete \.firescale\/step-times\.json if it stays unreadable\.$/;
    // Piped, as an agent or a script runs it: no progress line is shown.
    const plain = await runCli(['verify', '--budget', '30m', '--no-cache'], repo.root, NOT_CI);
    expect(plain.code).toBe(EXIT_OK);
    expect(plain.stderr).toBe('');
    const said = plain.stdout.split('\n').filter((line) => line.startsWith('warning: '));
    expect(said).toHaveLength(1);
    expect(said[0]?.slice('warning: '.length)).toMatch(warning);
    const json = await runCli(
      ['verify', '--budget', '30m', '--no-cache', '--json'],
      repo.root,
      NOT_CI,
    );
    const report = JSON.parse(json.stdout) as { warnings?: string[] };
    expect(report.warnings).toHaveLength(1);
    expect(report.warnings?.[0]).toMatch(warning);
    // Without --budget nothing depends on the kept times: no warning.
    const full = await runCli(['verify', '--no-cache'], repo.root, NOT_CI);
    expect(full.stdout).not.toContain('could not read');
  });

  it('keeps the time of a project step under its folder, and warns for it by name', async () => {
    const repo = await repoWith(
      {
        lint: false,
        typecheck: false,
        test: false,
        projects: { 'backend/api': { test: nodeScript('test.js') } },
      },
      { 'backend/api/test.js': "console.log('one');\n" },
    );
    expect((await runCli(['verify'], repo.root, NOT_CI)).code).toBe(EXIT_OK);
    const kept = await readTimes(repo.root);
    expect(kept.steps.map((r) => [r.project, r.step])).toEqual([['backend/api', 'test']]);
    await setTime(repo.root, 'test', { durationMs: 6 * MINUTE }, 'backend/api');
    await writeFile(path.join(repo.root, 'backend', 'api', 'test.js'), "console.log('two');\n");
    const second = await runCli(['verify', '--progress'], repo.root, NOT_CI);
    expect(progressLines(second.stderr)).toContain(
      'test in backend/api: slow step: it took 6m 00s when it last ran. To leave out steps this slow, run fscale verify --budget <time>',
    );
  });
});

/** Waits until `check` is true, or fails after `ms`. */
async function until(check: () => boolean | Promise<boolean>, ms = 30_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`gave up after ${ms} ms`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

/** Starts `fscale verify --progress` and collects what it prints. */
function startVerify(cwd: string, env: Record<string, string> = {}) {
  // Not a CI run, and no virtual environment of the person running the tests.
  const kept = Object.entries(process.env).filter(
    ([name]) => !['VIRTUAL_ENV', 'CI', 'GITHUB_ACTIONS'].includes(name.toUpperCase()),
  );
  const childEnv: NodeJS.ProcessEnv = {
    ...Object.fromEntries(kept),
    FSCALE_NODE_HOST: HOST,
    ...env,
  };
  const child = spawn(BIN, ['verify', '--progress'], {
    cwd,
    env: childEnv,
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
  started.push(child);
  let stderr = '';
  child.stdout.resume();
  child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
  const exited = new Promise<number | null>((resolve) =>
    child.on('close', (code) => resolve(code)),
  );
  return { child, exited, stderr: () => stderr };
}

/**
 * A test step that writes `ready` and then runs for a minute, or ends at
 * once when `quick.flag` is there.
 */
const HANGS = {
  '.gitignore': 'ready\nquick.flag\n',
  'test.js': [
    "const fs = require('fs');",
    "console.log('working');",
    "fs.writeFileSync('ready', '');",
    "if (!fs.existsSync('quick.flag')) setTimeout(() => {}, 60000);",
    '',
  ].join('\n'),
};

describe('a step that did not finish leaves how long it ran', () => {
  it.skipIf(WINDOWS)(
    'after Ctrl+C, and the next run says it ran at least that long and did not finish',
    async () => {
      const repo = await repoWith(
        { lint: false, typecheck: false, test: nodeScript('test.js') },
        HANGS,
      );
      const run = startVerify(repo.root);
      await until(() => existsSync(path.join(repo.root, 'ready')));
      await new Promise((resolve) => setTimeout(resolve, 300));
      run.child.kill('SIGINT');
      expect(await run.exited).toBe(EXIT_STOPPED);
      const kept = (await readTimes(repo.root)).steps;
      expect(kept.map((r) => [r.step, r.finished])).toEqual([['test', false]]);
      expect(kept[0]?.durationMs).toBeGreaterThanOrEqual(300);

      // As on the course project: stopped after 1h 33m.
      await setTime(repo.root, 'test', { durationMs: 93 * MINUTE });
      await writeFile(path.join(repo.root, 'quick.flag'), '');
      const again = await runCli(['verify', '--progress'], repo.root, NOT_CI);
      expect(again.code).toBe(EXIT_OK);
      expect(progressLines(again.stderr)).toContain(
        'test: slow step: it ran at least 1h 33m when it last ran and did not finish. To leave out steps this slow, run fscale verify --budget <time>',
      );
      // It finished this time: the record says how long it really takes.
      const after = (await readTimes(repo.root)).steps;
      expect(after.map((r) => [r.step, r.finished])).toEqual([['test', true]]);
      expect(after[0]?.durationMs).toBeLessThan(MINUTE);
    },
  );

  it('when verify itself is killed outright, from what it wrote while the step ran', async () => {
    const repo = await repoWith(
      { lint: false, typecheck: false, test: nodeScript('test.js') },
      HANGS,
    );
    // A heartbeat every 200 ms stands for the one a minute of a real run.
    const run = startVerify(repo.root, { FSCALE_HEARTBEAT_MS: '200' });
    await until(() => existsSync(path.join(repo.root, 'ready')));
    await until(() => run.stderr().includes('test: still running after'));
    await until(() => existsSync(timesPath(repo.root)), 5_000);
    // SIGKILL, or TerminateProcess on Windows: no handler of fscale runs.
    run.child.kill('SIGKILL');
    await run.exited;
    const kept = (await readTimes(repo.root)).steps;
    expect(kept.map((r) => [r.step, r.finished])).toEqual([['test', false]]);
    expect(kept[0]?.durationMs).toBeGreaterThanOrEqual(200);
  });

  it.each([
    { limit: 'its own time limit', verify: { test: { command: '', timeout: '2s' } } },
    { limit: "its project's time limit", verify: { projects: { '.': { timeout: '2s' } } } },
    { limit: 'the idle limit', verify: { idleTimeout: '2s', test: '' } },
  ])('after it hit $limit', async ({ verify }) => {
    const command = nodeScript('test.js');
    const settings: Record<string, unknown> = { lint: false, typecheck: false, ...verify };
    if ('projects' in verify) {
      settings['test'] = false;
      settings['projects'] = { '.': { timeout: '2s', test: command } };
    } else if (typeof settings['test'] === 'string') {
      settings['test'] = command;
    } else {
      settings['test'] = { command, timeout: '2s' };
    }
    const repo = await repoWith(settings, HANGS);
    const result = await runCli(['verify'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain('TIMED OUT');
    const kept = (await readTimes(repo.root)).steps;
    expect(kept.map((r) => [r.step, r.finished])).toEqual([['test', false]]);
    expect(kept[0]?.durationMs).toBeGreaterThanOrEqual(2000);
    expect(kept[0]?.durationMs).toBeLessThan(MINUTE);
  });

  it('keeps the longer time when a step that is known to be slow is stopped early', async () => {
    const repo = await repoWith(
      { lint: false, typecheck: false, test: { command: nodeScript('test.js'), timeout: '2s' } },
      HANGS,
    );
    await writeFile(path.join(repo.root, 'quick.flag'), '');
    expect((await runCli(['verify'], repo.root, NOT_CI)).code).toBe(EXIT_OK);
    await setTime(repo.root, 'test', { durationMs: 100 * MINUTE });
    await rm(path.join(repo.root, 'quick.flag'));
    // Timed out after 2 s: that says nothing new about a step of 1h 40m.
    // (--no-cache: the pass above is in the step cache.)
    const stopped = await runCli(['verify', '--no-cache'], repo.root, NOT_CI);
    expect(stopped.code).toBe(EXIT_VIOLATIONS);
    expect(stopped.stdout).toContain('TIMED OUT');
    const kept = (await readTimes(repo.root)).steps;
    expect(kept.map((r) => [r.step, r.finished, r.durationMs])).toEqual([
      ['test', true, 100 * MINUTE],
    ]);
  });
});

const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);

interface BudgetStep {
  name: string;
  project?: string;
  status: string;
  detail: string;
  label?: string;
  base?: unknown;
  cache?: unknown;
  lastKnownMs?: number;
  lastKnownFinished?: boolean;
}

interface BudgetReport {
  schemaVersion: number;
  ok: boolean;
  result: string;
  stepsTotal: number;
  stepsRun: number;
  budgetMs?: number;
  steps: BudgetStep[];
  notRun: {
    step: string;
    project?: string;
    reason: string;
    lastKnownMs: number;
    lastKnownFinished: boolean;
  }[];
}

/** Parses verify's JSON and checks it against the shipped schema. */
async function budgetReport(json: string): Promise<BudgetReport> {
  const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as object;
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  check(data);
  expect(check.errors ?? []).toEqual([]);
  return data as BudgetReport;
}

/** Writes the kept times of a repo, as an earlier run would have. */
async function seedTimes(root: string, steps: Omit<TimeRecord, 'ranAt'>[]): Promise<void> {
  await mkdir(path.join(root, '.firescale'), { recursive: true });
  const file: TimesFile = { format: 1, steps: steps.map((step) => ({ ...step, ranAt: 1 })) };
  await writeFile(timesPath(root), JSON.stringify(file));
}

/** A step that appends its name to `ran.log` in the repo root, then passes. */
function logs(name: string, log = 'ran.log'): string {
  return `require('fs').appendFileSync(${JSON.stringify(log)}, '${name}\\n');\n`;
}

async function ran(root: string): Promise<string[]> {
  return (await readFile(path.join(root, 'ran.log'), 'utf8').catch(() => ''))
    .split(/\r?\n/)
    .filter(Boolean);
}

/** The first line of the summary. */
function headline(stdout: string): string {
  return stdout.split(/\r?\n/)[0] ?? '';
}

/** How the Not run paragraph ends: what changes a kept time, and what forgets it. */
const KEPT_TIME_HINT =
  'Its kept time comes down only when it passes in a run of fscale verify without --budget; running the command by hand changes nothing. To forget every kept time, delete .firescale/step-times.json.';

describe('verify --budget leaves out the steps that are known to take longer', () => {
  const verify = { lint: nodeScript('lint.js'), typecheck: false, test: nodeScript('test.js') };
  const files = {
    '.gitignore': 'ran.log\n',
    'lint.js': logs('lint'),
    'test.js': logs('test'),
  };
  const slowTest = (finished: boolean, minutes: number) => [
    { step: 'lint', command: nodeScript('lint.js'), durationMs: 2 * MINUTE, finished: true },
    { step: 'test', command: nodeScript('test.js'), durationMs: minutes * MINUTE, finished },
  ];

  it('does not start a step over the budget, says NOT RUN with its last time, and never reads OK', async () => {
    const repo = await repoWith(verify, files);
    await seedTimes(repo.root, slowTest(true, 100));
    const result = await runCli(['verify', '--budget', '30m', '--progress'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    expect(headline(result.stdout)).toBe(
      'verify: PARTIAL - 2 of 3 steps ran, 1 not run (over the --budget of 30m)',
    );
    expect(result.stdout).toContain(
      '  test       NOT RUN  over the budget of 30m: it took 1h 40m when it last ran\n',
    );
    expect(result.stdout).toContain(
      `\nNot run: test was not started, because it took 1h 40m when it last ran and the budget is 30m (--budget). This run says nothing about it. To run it: ${nodeScript('test.js')}. ${KEPT_TIME_HINT}\n`,
    );
    expect(result.stdout).not.toMatch(/verify: OK/);
    // Lint is under the budget and ran; the test command was never started.
    expect(await ran(repo.root)).toEqual(['lint']);
    const lines = progressLines(result.stderr);
    expect(lines).toContain(
      'test: NOT RUN: it took 1h 40m when it last ran, and the budget is 30m (--budget)',
    );
    expect(lines.some((line) => line.startsWith('test: started'))).toBe(false);
    // Its last known time is kept as it was.
    const kept = (await readTimes(repo.root)).steps.find((r) => r.step === 'test');
    expect([kept?.durationMs, kept?.finished]).toEqual([100 * MINUTE, true]);

    // Nothing about it went into a cache: without the budget it runs.
    const whole = await runCli(['verify'], repo.root, NOT_CI);
    expect(whole.code).toBe(EXIT_OK);
    expect(headline(whole.stdout)).toBe(
      'verify: OK (1 of 3 steps passed from the cache, not run now)',
    );
    expect(await ran(repo.root)).toEqual(['lint', 'test']);
  });

  const BUDGET_RUN = ['verify', '--budget', '30m', '--no-cache'];

  it('brings a step back once the file of kept times is deleted, as the summary says', async () => {
    const repo = await repoWith(verify, files);
    await seedTimes(repo.root, slowTest(true, 100));
    const left = await runCli(BUDGET_RUN, repo.root, NOT_CI);
    expect(left.stdout).toContain(KEPT_TIME_HINT);
    expect(await ran(repo.root)).toEqual(['lint']);
    // Deleting the file forgets every kept time: the step runs again, and
    // its pass sets a new, short time.
    await rm(timesPath(repo.root));
    const back = await runCli(BUDGET_RUN, repo.root, NOT_CI);
    expect(headline(back.stdout)).toBe('verify: OK');
    expect(await ran(repo.root)).toEqual(['lint', 'lint', 'test']);
    const kept = (await readTimes(repo.root)).steps.find((r) => r.step === 'test');
    expect(kept?.durationMs).toBeLessThan(MINUTE);
  });

  it('brings a step back once it passes in a run without --budget', async () => {
    const repo = await repoWith(verify, files);
    await seedTimes(repo.root, slowTest(true, 100));
    expect((await runCli(['verify', '--no-cache'], repo.root, NOT_CI)).code).toBe(EXIT_OK);
    const back = await runCli(BUDGET_RUN, repo.root, NOT_CI);
    expect(headline(back.stdout)).toBe('verify: OK');
    expect(await ran(repo.root)).toEqual(['lint', 'test', 'lint', 'test']);
  });

  it('says the same in JSON: the status, the reason, the last known time and the count', async () => {
    const repo = await repoWith(verify, files);
    await seedTimes(repo.root, slowTest(false, 93));
    const result = await runCli(['verify', '--budget=1h', '--json'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    const report = await budgetReport(result.stdout);
    expect(report.schemaVersion).toBe(7);
    expect([report.result, report.ok]).toEqual(['partial', true]);
    expect([report.stepsTotal, report.stepsRun]).toEqual([3, 2]);
    expect(report.budgetMs).toBe(60 * MINUTE);
    const reason =
      'over the budget of 1h: it ran at least 1h 33m when it last ran and did not finish';
    expect(report.notRun).toEqual([
      { step: 'test', reason, lastKnownMs: 93 * MINUTE, lastKnownFinished: false },
    ]);
    const test = report.steps.find((step) => step.name === 'test');
    expect(test?.status).toBe('over-budget');
    expect(test?.detail).toBe(reason);
    expect([test?.lastKnownMs, test?.lastKnownFinished]).toEqual([93 * MINUTE, false]);
    expect(test?.cache).toBeUndefined();
    expect(await ran(repo.root)).toEqual(['lint']);
    // A run without --budget has an empty list and no budget.
    const whole = await budgetReport(
      (await runCli(['verify', '--json'], repo.root, NOT_CI)).stdout,
    );
    expect([whole.result, whole.notRun, whole.budgetMs]).toEqual(['ok', [], undefined]);
  });

  it('announces a slow step that is within the budget without telling you to use --budget', async () => {
    const repo = await repoWith(verify, files);
    await seedTimes(repo.root, slowTest(true, 100));
    const result = await runCli(['verify', '--budget', '3h', '--progress'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    const slow = progressLines(result.stderr).filter((line) => line.includes('slow step'));
    expect(slow).toEqual(['test: slow step: it took 1h 40m when it last ran']);
  });

  it('runs a step with no known time, and a step whose time is within the budget', async () => {
    const repo = await repoWith(verify, files);
    // Nothing is known about lint; the test took exactly the budget.
    await seedTimes(repo.root, slowTest(true, 30).slice(1));
    const result = await runCli(['verify', '--budget', '30m'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    expect(headline(result.stdout)).toBe('verify: OK');
    expect(await ran(repo.root)).toEqual(['lint', 'test']);
  });

  it('takes a pass from the cache instead of leaving the step out', async () => {
    const repo = await repoWith(verify, files);
    expect((await runCli(['verify'], repo.root, NOT_CI)).code).toBe(EXIT_OK);
    await setTime(repo.root, 'test', { durationMs: 100 * MINUTE });
    const result = await runCli(['verify', '--budget', '30m'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    expect(headline(result.stdout)).toBe(
      'verify: OK (2 of 3 steps passed from the cache, not run now)',
    );
  });

  it('leaves out a step of a project, and names its folder', async () => {
    const repo = await repoWith(
      {
        lint: false,
        typecheck: false,
        test: false,
        projects: { 'backend/api': { lint: nodeScript('lint.js'), test: nodeScript('test.js') } },
      },
      {
        'backend/api/lint.js': logs('lint', '../../ran.log'),
        'backend/api/test.js': logs('test', '../../ran.log'),
        '.gitignore': 'ran.log\n',
      },
    );
    await seedTimes(repo.root, [
      {
        project: 'backend/api',
        step: 'test',
        command: nodeScript('test.js'),
        durationMs: 100 * MINUTE,
        finished: true,
      },
    ]);
    const result = await runCli(['verify', '--budget', '45m'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_OK);
    expect(headline(result.stdout)).toBe(
      'verify: PARTIAL - 2 of 3 steps ran, 1 not run (over the --budget of 45m)',
    );
    expect(result.stdout).toContain(
      '    test       NOT RUN  over the budget of 45m: it took 1h 40m when it last ran\n',
    );
    expect(result.stdout).toContain(
      `\nNot run: test in backend/api was not started, because it took 1h 40m when it last ran and the budget is 45m (--budget). This run says nothing about it. To run it in backend/api: ${nodeScript('test.js')}. ${KEPT_TIME_HINT}\n`,
    );
    expect(await ran(repo.root)).toEqual(['lint']);
  });

  it('names the step that was not run beside a failure, and still fails', async () => {
    const repo = await repoWith(verify, { ...files, 'test.js': 'process.exit(1);\n' });
    await seedTimes(repo.root, [
      { step: 'lint', command: nodeScript('lint.js'), durationMs: 100 * MINUTE, finished: true },
    ]);
    const result = await runCli(['verify', '--budget', '30m'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(headline(result.stdout)).toBe(
      'verify: FAILED at test; 1 step not run (over the --budget of 30m)',
    );
    expect(result.stdout).toContain(
      '  lint       NOT RUN  over the budget of 30m: it took 1h 40m when it last ran\n',
    );
  });

  it('fails when verify.failOnSkipped is on: a run that left a step out is not a pass there', async () => {
    const repo = await repoWith({ ...verify, failOnSkipped: true }, files);
    await seedTimes(repo.root, slowTest(true, 100));
    const result = await runCli(['verify', '--budget', '30m', '--json'], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await budgetReport(result.stdout);
    expect([report.result, report.ok]).toEqual(['failed', false]);
    // (--no-cache: the run above kept lint's pass.)
    const human = await runCli(['verify', '--budget', '30m', '--no-cache'], repo.root, NOT_CI);
    expect(headline(human.stdout)).toBe(
      'verify: FAILED - 1 of 3 steps not run (over the --budget of 30m), and verify.failOnSkipped is on',
    );
  });

  it('never labels a step that was not run, and never lets the run read OK, with a base to compare with', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core'], verify }),
        '.gitignore': '.firescale/\nran.log\n',
        // Lint fails on the base and on the head; the test fails on the base.
        'lint.js': 'process.exit(1);\n',
        'test.js': 'process.exit(1);\n',
        'note.txt': 'base\n',
      },
      track: 'no-git',
    });
    expect((await runGit(repo.root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
    await commitAll(repo.root, 'base');
    expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    await writeFile(path.join(repo.root, 'note.txt'), 'changed\n');
    await seedTimes(repo.root, slowTest(true, 100).slice(1));
    const result = await runCli(['verify', '--budget', '30m', '--json'], repo.root, NOT_CI);
    const report = await budgetReport(result.stdout);
    const lint = report.steps.find((step) => step.name === 'lint');
    const test = report.steps.find((step) => step.name === 'test');
    // The pre-existing lint failure does not block, but the run is partial.
    expect(lint?.label).toBe('pre-existing');
    expect([test?.status, test?.label, test?.base]).toEqual(['over-budget', undefined, undefined]);
    expect([report.result, report.ok]).toEqual(['partial', true]);
    const human = await runCli(['verify', '--budget', '30m'], repo.root, NOT_CI);
    expect(headline(human.stdout)).toMatch(
      /^verify: PARTIAL - 2 of 3 steps ran, 1 not run \(over the --budget of 30m\); pre-existing \(fails on the base commit [0-9a-f]{7} too\): lint$/,
    );
  });

  it.each([
    ['soon', 'got "soon"'],
    ['0', 'got "0"'],
    ['0m', 'got "0m"'],
    ['1.5h', 'got "1.5h"'],
  ])('rejects the budget %s and says what a time looks like', async (value, got) => {
    const repo = await repoWith(verify, files);
    const result = await runCli(['verify', `--budget=${value}`], repo.root, NOT_CI);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain(
      `--budget needs a time longer than zero, such as "30m", "90s" or "1h 30m", or a number of seconds; ${got}.`,
    );
    expect(await ran(repo.root)).toEqual([]);
  });

  it('accepts a plain number of seconds, as the time limits in the config do', async () => {
    const repo = await repoWith(verify, files);
    await seedTimes(repo.root, slowTest(true, 100));
    const result = await runCli(['verify', '--budget', '3600'], repo.root, NOT_CI);
    expect(headline(result.stdout)).toBe(
      'verify: PARTIAL - 2 of 3 steps ran, 1 not run (over the --budget of 1h)',
    );
  });
});

describe('--budget is refused where a partial run must not pass for a whole one', () => {
  const verify = { lint: false, typecheck: false, test: nodeScript('test.js') };
  const files = { '.gitignore': 'ran.log\n', 'test.js': logs('test') };
  const NEXT_STEP =
    'Run fscale verify without --budget there; to bound a slow step in CI, set its time limit in the firescale config (verify.<step>.timeout).';

  it.each([
    [{ CI: 'true', GITHUB_ACTIONS: undefined }, 'CI is set in the environment'],
    [{ CI: '1', GITHUB_ACTIONS: undefined }, 'CI is set in the environment'],
    [{ CI: undefined, GITHUB_ACTIONS: 'true' }, 'GITHUB_ACTIONS is true in the environment'],
  ])('in CI (%o): says why, and what to do, and runs nothing', async (env, why) => {
    const repo = await repoWith(verify, files);
    for (const extra of [[], ['--json'], ['--dry-run']]) {
      const result = await runCli(['verify', '--budget', '30m', ...extra], repo.root, env);
      expect(result.code).toBe(EXIT_USAGE);
      expect(result.stdout).toBe('');
      expect(result.stderr).toContain(
        `fscale: --budget cannot be used in CI (${why}): it leaves slow steps out, so the run would pass without them. ${NEXT_STEP}\n`,
      );
    }
    expect(await ran(repo.root)).toEqual([]);
    // Without the flag, CI runs every step as before.
    const whole = await runCli(['verify'], repo.root, env);
    expect(whole.code).toBe(EXIT_OK);
    expect(await ran(repo.root)).toEqual(['test']);
  });

  // `CI=false` is how some people turn CI behaviour off on their own
  // machine. It still counts as CI (as for verify's commands), so the
  // message says how to get out of it there.
  it.each(['false', '0'])(
    'with CI=%s: also says that the value counts as CI, and to unset it on your own machine',
    async (value) => {
      const repo = await repoWith(verify, files);
      const result = await runCli(['verify', '--budget', '30m'], repo.root, {
        CI: value,
        GITHUB_ACTIONS: undefined,
      });
      expect(result.code).toBe(EXIT_USAGE);
      expect(result.stderr).toContain(
        `fscale: --budget cannot be used in CI (CI is set in the environment): it leaves slow steps out, so the run would pass without them. ${NEXT_STEP} CI is "${value}" here, and any value that is not empty counts as CI: on your own machine, unset CI.\n`,
      );
      expect(await ran(repo.root)).toEqual([]);
    },
  );

  it('is not taken for CI by an empty CI variable', async () => {
    const repo = await repoWith(verify, files);
    const result = await runCli(['verify', '--budget', '30m'], repo.root, {
      CI: '',
      GITHUB_ACTIONS: undefined,
    });
    expect(result.code).toBe(EXIT_OK);
    expect(await ran(repo.root)).toEqual(['test']);
  });

  it('in the stop hook: says why, and what to do, and runs nothing', async () => {
    const repo = await repoWith(verify, files);
    const result = await runCli(
      ['hooks', 'run', '--agent', 'claude', '--budget', '30m'],
      repo.root,
      NOT_CI,
    );
    // A stop hook never exits with 2 for its own errors: Claude Code and
    // Codex read that as "do not stop", and a broken hook must not trap an
    // agent (as for every other error of `hooks run`).
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toBe('');
    expect(result.stderr).toContain(
      'fscale: --budget cannot be used in the stop hook: it leaves slow steps out, so an agent could stop without them having run. Remove --budget from the hook command (fscale hooks run --agent <name>); a person can still run fscale verify --budget <time> by hand.\n',
    );
    expect(await ran(repo.root)).toEqual([]);
  });
});
