// A verify run with a base commit that is stopped (Ctrl+C) after a step has
// already failed (ADR 0059): the failure has no `new` or `pre-existing`
// label and the tamper guard has not run, and the summary and `--json` say
// so, with the command that gets them.
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { BIN, commitAll, HOST } from './test-helpers.js';

const fixtures = fixtureSet();
const started: ChildProcess[] = [];
afterEach(async () => {
  for (const child of started.splice(0)) child.kill('SIGKILL');
  await fixtures.cleanup();
});

const WINDOWS = process.platform === 'win32';
const EXIT_STOPPED = 130;
const CTRL_BREAK = path.join(path.dirname(BIN), `fscale-ctrl-break${WINDOWS ? '.exe' : ''}`);
const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);
// The wait for the hanging step is shorter than the test's limit, so a slow
// machine fails with the wait's own message and fscale's output, not with
// Vitest's bare timeout. The limit covers the wait and the stop after it.
const READY_WAIT = 30_000;
const TEST_LIMIT = 60_000;

/**
 * A repo on a `feature` branch whose lint fails at once and whose test
 * writes `ready` and then hangs. One file differs from the base commit.
 */
async function stoppedRepo() {
  const outside = await fixtures.create({ files: {}, track: 'no-git' });
  const ready = path.join(outside.root, 'ready');
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: 'node lint.js', typecheck: false, test: 'node test.js' },
      }),
      '.gitignore': '.firescale/\n',
      'lint.js': "console.log('lint-broke');\nprocess.exit(1);\n",
      'test.js': `require('fs').writeFileSync(${JSON.stringify(ready)}, '');\nsetTimeout(() => {}, 120000);\n`,
      'notes.txt': 'base\n',
      'package-lock.json': '{"lockfileVersion":3}\n',
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  await writeFile(path.join(root, 'notes.txt'), 'changed\n');
  return { root, ready, scratch: outside.root };
}

function childEnv(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, FSCALE_NODE_HOST: HOST };
  delete env['VIRTUAL_ENV'];
  return env;
}

/**
 * Runs `fscale verify`, interrupts it once `ready` exists (SIGINT; on
 * Windows Ctrl+Break through the helper), and gives its exit code and stdout.
 */
async function interrupted(
  repo: { root: string; ready: string; scratch: string },
  args: string[],
): Promise<{ code: number | null; stdout: string }> {
  await rm(repo.ready, { force: true });
  if (WINDOWS) {
    const output = path.join(repo.scratch, 'verify.out');
    const helper = spawn(CTRL_BREAK, ['group', repo.ready, output, BIN, 'verify', ...args], {
      cwd: repo.root,
      env: childEnv(),
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    started.push(helper);
    let helperErr = '';
    helper.stderr.on('data', (chunk: Buffer) => (helperErr += chunk.toString()));
    const code = await new Promise<number | null>((resolve) => helper.on('close', resolve));
    // What fscale wrote to stderr goes into the message, so a failure seen
    // only on Windows can be read from the CI log.
    const fscaleErr = await readFile(`${output}.err`, 'utf8').catch(() => '(no stderr file)');
    expect(helperErr, `the helper failed; fscale's stderr:\n${fscaleErr}`).toBe('');
    const stdout = (await readFile(output, 'utf8').catch(() => '')).replace(/\r\n/g, '\n');
    expect(code, `fscale's stdout:\n${stdout}\nfscale's stderr:\n${fscaleErr}`).toBe(EXIT_STOPPED);
    return { code, stdout };
  }
  const child = spawn(BIN, ['verify', ...args], {
    cwd: repo.root,
    env: childEnv(),
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  started.push(child);
  let stdout = '';
  child.stdout.on('data', (chunk: Buffer) => (stdout += chunk.toString()));
  let stderr = '';
  child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
  const exited = new Promise<number | null>((resolve) => child.on('close', resolve));
  const deadline = Date.now() + READY_WAIT;
  while (!existsSync(repo.ready)) {
    if (Date.now() > deadline) {
      throw new Error(
        `the test step did not start within ${READY_WAIT} ms.\nstdout:\n${stdout}\nstderr:\n${stderr}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  child.kill('SIGINT');
  return { code: await exited, stdout };
}

const NOT_COMPARED =
  /\nNot compared with the base commit [0-9a-f]{7} \(main\): the run was stopped first, so no failure is labelled `new` or `pre-existing`, and the tamper guard did not run\. Run fscale verify again and let it finish to get both\.\n/;

describe('a verify run with a base that is stopped after a step failed', () => {
  it(
    'says that failures were not labelled and the tamper guard did not run',
    async () => {
      const repo = await stoppedRepo();
      const { code, stdout } = await interrupted(repo, []);
      expect(code).toBe(EXIT_STOPPED);
      expect(stdout.split('\n')[0]).toBe(
        'verify: STOPPED - interrupted after 2 of 3 steps ran; FAILED at lint',
      );
      expect(stdout).toMatch(/ {2}lint +FAILED +node lint\.js \(exit 1\)/);
      expect(stdout).toMatch(/ {2}test +STOPPED /);
      expect(stdout).toMatch(NOT_COMPARED);
      // No base run was started after the interrupt.
      expect(existsSync(path.join(repo.root, '.firescale', 'base'))).toBe(false);
    },
    TEST_LIMIT,
  );

  it(
    'carries the same fact in --json',
    async () => {
      const repo = await stoppedRepo();
      const { code, stdout } = await interrupted(repo, ['--json']);
      expect(code).toBe(EXIT_STOPPED);
      const schema: object = JSON.parse(await readFile(SCHEMA, 'utf8'));
      const check = new Ajv({ strict: true }).compile(schema);
      const report = JSON.parse(stdout) as {
        result: string;
        steps: { name: string; status: string; label?: string }[];
        base?: { compared: boolean; reference?: string; commit?: string; reason?: string };
        tamper?: unknown;
      };
      check(report);
      expect(check.errors ?? []).toEqual([]);
      expect(report.result).toBe('stopped');
      const lint = report.steps.find((step) => step.name === 'lint');
      expect(lint?.status).toBe('failed');
      expect(lint?.label).toBeUndefined();
      expect(report.tamper).toBeUndefined();
      expect(report.base?.compared).toBe(false);
      expect(report.base?.reference).toBe('main');
      expect(report.base?.commit).toMatch(/^[0-9a-f]{40}$/);
      expect(report.base?.reason).toBe(
        'the run was stopped before failures were compared with the base commit and before the tamper guard ran',
      );
    },
    TEST_LIMIT,
  );
});
