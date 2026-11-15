// Slow steps (ADR 0057): verify keeps how long each step took, warns before
// a step that was slow the last time, keeps a time for a step that was
// stopped or timed out, and `--budget` leaves out steps slower than a time.
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { BIN, EXIT_OK, EXIT_VIOLATIONS, HOST, runCli } from './test-helpers.js';

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
    const { rm } = await import('node:fs/promises');
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
