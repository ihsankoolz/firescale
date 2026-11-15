// Slow steps (ADR 0057): verify keeps how long each step took, warns before
// a step that was slow the last time, keeps a time for a step that was
// stopped or timed out, and `--budget` leaves out steps slower than a time.
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(async () => {
  await fixtures.cleanup();
});

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
