import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { baseCalls, commitAll, EXIT_OK, runCli, stepCalls } from './test-helpers.js';

// Blame-aware verify: reruns and the stop hook (M21, ADR 0039).

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const EXIT_BLOCK = 2;

interface Step {
  name: string;
  project?: string;
  status: string;
  label?: string;
  failedTests?: { id: string; label?: string }[];
}

interface Report {
  result: string;
  ok: boolean;
  steps: Step[];
}

function step(report: Report, name: string, project?: string): Step {
  const found = report.steps.find((s) => s.name === name && s.project === project);
  if (!found) throw new Error(`no step ${name} in ${project ?? 'the root'}`);
  return found;
}

/** A repo on a `feature` branch off `main`, with a test step given as a node script. */
async function repoWith(test: string, files: Record<string, string> = {}) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: 'node test.js' },
      }),
      '.gitignore': '.firescale/\n',
      'test.js': test,
      'state.cfg': 'ok\n',
      ...files,
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  const write = (file: string, content: string) => writeFile(path.join(root, file), content);
  return { root, write };
}

/** Fails while `state.cfg` says "fail". */
const STATE_TEST = [
  "const fs = require('fs');",
  "process.exit(fs.readFileSync('state.cfg', 'utf8').includes('fail') ? 1 : 0);",
  '',
].join('\n');

const hook = (root: string, env: Record<string, string> = {}) =>
  runCli(['hooks', 'run', '--agent', 'claude'], root, env, {
    input: JSON.stringify({ session_id: 'blame', stop_hook_active: false }),
  });

describe('reruns', () => {
  it('labels a new failure that passes when run again flaky, and does not block', async () => {
    const outside = await fixtures.create({ files: {}, track: 'no-git' });
    const marker = path.join(outside.root, 'ran-once');
    // Fails on its first run on the head only: the marker is outside the
    // repo, so the base run (after it) and the rerun see it.
    const flaky = [
      "const fs = require('fs');",
      `const marker = ${JSON.stringify(marker)};`,
      "if (!fs.existsSync(marker)) { fs.writeFileSync(marker, 'x'); console.log('FAIL first run'); process.exit(1); }",
      'process.exit(0);',
      '',
    ].join('\n');
    const { root, write } = await repoWith(flaky);
    await write('state.cfg', 'a change\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_OK);
    const json = JSON.parse(result.stdout) as Report;
    expect(step(json, 'test').status).toBe('failed');
    expect(step(json, 'test').label).toBe('flaky');
    expect(json.result).toBe('pre-existing');
  });

  it('writes nothing about a flaky result to disk', async () => {
    const outside = await fixtures.create({ files: {}, track: 'no-git' });
    const marker = path.join(outside.root, 'ran-once');
    const flaky = [
      "const fs = require('fs');",
      `const marker = ${JSON.stringify(marker)};`,
      "if (!fs.existsSync(marker)) { fs.writeFileSync(marker, 'x'); process.exit(1); }",
      'process.exit(0);',
      '',
    ].join('\n');
    const { root, write } = await repoWith(flaky);
    await write('state.cfg', 'a change\n');
    const result = await runCli(['verify'], root);
    expect(result.stdout).toContain('Flaky: test failed, then passed when run again');
    // Nothing under .firescale names it, and the second run's pass was not
    // stored as a pass of the step.
    const stored: string[] = [];
    const walk = async (dir: string): Promise<void> => {
      for (const entry of await readdir(dir, { withFileTypes: true }).catch(() => [])) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) await walk(full);
        else stored.push(await readFile(full, 'utf8'));
      }
    };
    await walk(path.join(root, '.firescale'));
    expect(stored.join('\n')).not.toMatch(/flaky/i);
    const steps = await readdir(path.join(root, '.firescale', 'steps')).catch(() => []);
    for (const entry of steps) {
      const text = await readFile(path.join(root, '.firescale', 'steps', entry), 'utf8');
      expect(text).not.toContain('"step": "test"');
    }
  });

  it('keeps a failure that fails again new', async () => {
    const { root, write } = await repoWith(STATE_TEST);
    await write('state.cfg', 'fail\n');
    const json = JSON.parse((await runCli(['verify', '--json'], root)).stdout) as Report;
    expect(step(json, 'test').label).toBe('new');
    expect(json.ok).toBe(false);
  });
});

describe('the stop hook', () => {
  it('lets an agent stop when every failure is pre-existing, and blocks on a new one', async () => {
    const { root, write } = await repoWith(STATE_TEST, { 'state.cfg': 'fail\n' });
    await write('other.cfg', 'a change the hook sees\n');
    const old = await hook(root);
    expect(old.code).toBe(EXIT_OK);
    expect(old.stderr).not.toContain('FAILED');
    // The test did run, and failed, as on the base.
    const json = JSON.parse((await runCli(['verify', '--json'], root)).stdout) as Report;
    expect(step(json, 'test').label).toBe('pre-existing');
    // A new failure: the test passed on the base.
    const fresh = await repoWith(STATE_TEST);
    await fresh.write('state.cfg', 'fail\n');
    const blocked = await hook(fresh.root);
    expect(blocked.code).toBe(EXIT_BLOCK);
    expect(blocked.stderr).toContain('verify: FAILED at test (new)');
  });

  it('runs only the projects the change touches', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    await commitAll(repo.root, 'initial');
    expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    const log = path.join(tools.root, 'calls.log');
    await repo.write('backend/audit_service/main.py', 'app = 1\n');
    const result = await hook(repo.root, shimEnv(tools.root, log));
    expect(result.code).toBe(EXIT_OK);
    expect(await stepCalls(log)).toEqual(['python -m pytest in audit_service']);
    // Nothing failed, so the base was never run.
    expect(await baseCalls(log)).toEqual([]);
  });

  it('runs on the base only the failing steps, never the passing ones', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    await commitAll(repo.root, 'initial');
    expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    const log = path.join(tools.root, 'calls.log');
    await repo.write('backend/audit_service/main.py', 'app = 1\n');
    await repo.write('frontend/src/main.ts', 'export const app = 2;\n');
    // python fails everywhere (also on the base); the frontend passes.
    const result = await hook(repo.root, shimEnv(tools.root, log, 'python'));
    expect(result.code).toBe(EXIT_OK);
    expect(await baseCalls(log)).toEqual(['python -m pytest in audit_service']);
  });
});
