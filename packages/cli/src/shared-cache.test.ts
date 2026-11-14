import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

// Faster generated CI and a shared cache (M22, ADR 0041).

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SHARED = '.firescale/shared';

/** The jobs of a workflow, by name: each job's lines, from its header to the next job. */
function jobs(workflow: string): Map<string, string> {
  const lines = workflow.split('\n');
  const start = lines.indexOf('jobs:');
  const found = new Map<string, string>();
  let name: string | undefined;
  let body: string[] = [];
  for (const line of lines.slice(start + 1)) {
    const header = /^ {2}([A-Za-z0-9_-]+):$/.exec(line);
    if (header || /^\S/.test(line)) {
      if (name) found.set(name, body.join('\n'));
      name = header?.[1];
      body = [];
      continue;
    }
    body.push(line);
  }
  if (name) found.set(name, body.join('\n'));
  return found;
}

async function initWorkflow(
  files: Record<string, string> = {},
  file = '.github/workflows/ci.yml',
): Promise<string> {
  const repo = await fixtures.create({ files, track: 'no-git' });
  expect((await runGit(repo.root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  const result = await runCli(['init', '--force', '--packs', 'core'], repo.root);
  expect(result.code, result.stdout + result.stderr).toBe(EXIT_OK);
  const text = await repo.read(file);
  if (text === null) throw new Error(`init wrote no ${file}`);
  return text;
}

describe('the generated workflow', () => {
  it('has a tamper guard of its own, started only by pull_request_target, that runs nothing of the pull request', async () => {
    const guard = await initWorkflow({}, '.github/workflows/tamper-guard.yml');
    // Only pull_request_target starts it: no run of another event holds a
    // skipped job of the same name, which would pass a required check.
    expect(guard).toMatch(/^on:\n {2}pull_request_target:\n\n/m);
    expect(guard).not.toMatch(/^ {2}(pull_request|push|workflow_dispatch):/m);
    const all = jobs(guard);
    expect([...all.keys()]).toEqual(['tamper-guard']);
    const body = all.get('tamper-guard') ?? '';
    expect(body).toMatch(/^ {4}cache-mode: none$/m);
    // Its token reads the code and the reviews, and writes nothing.
    expect(guard).toMatch(/^permissions:\n {2}contents: read\n {2}pull-requests: read$/m);
    expect(body).not.toContain('actions/cache');
    expect(body).not.toMatch(/^\s+ref: /m);
    expect(body).not.toMatch(/^\s+if: /m);
    const runs = [...body.matchAll(/- run: (.*)$/gm)].map((m) => m[1]);
    expect(runs).toEqual([
      'git fetch --no-tags origin "$HEAD_SHA"',
      'gh api --paginate "repos/$REPO/pulls/$PR/reviews" > "$RUNNER_TEMP/reviews.json"',
      expect.stringMatching(
        /^npx --yes firescale@\d+\.\d+\.\d+\S* tamper --pr-base --approvals "\$RUNNER_TEMP\/reviews\.json"$/,
      ),
    ]);
  });

  it('has a pull request job that can restore caches and never save one', async () => {
    const workflow = await initWorkflow({
      'package.json': '{"name":"demo","scripts":{"test":"node -e 0"}}\n',
      'package-lock.json': '{"lockfileVersion":3}\n',
      'svc/requirements.txt': 'requests\n',
      'svc/main.py': 'x = 1\n',
      'svc/pytest.ini': '',
    });
    const all = jobs(workflow);
    const pullRequest = [...all].filter(([, body]) => body.includes('pull_request'));
    expect(pullRequest.map(([name]) => name)).toEqual(['pull-request']);
    expect(workflow).not.toContain('pull_request_target');
    for (const [, body] of pullRequest) {
      // The token itself cannot save.
      expect(body).toMatch(/^ {4}cache-mode: read$/m);
      // No step that saves: only restore-only cache steps...
      const uses = [...body.matchAll(/uses: (\S+)@/g)].map((m) => m[1]);
      expect(uses).toContain('actions/cache/restore');
      expect(uses).not.toContain('actions/cache');
      expect(uses).not.toContain('actions/cache/save');
      // ...and no setup action that saves its own cache in a post step.
      expect(body).not.toMatch(/^\s+cache: /m);
      expect(body).toContain('package-manager-cache: false');
      // It reads the shared step records, and verifies only what changed.
      expect(body).toContain(`path: ${SHARED}`);
      expect(body).toContain(
        `npx firescale verify --affected --pr-base --jobs 2 --shared-cache ${SHARED}`,
      );
      // The whole history, for the merge base.
      expect(body).toContain('fetch-depth: 0');
    }
    // Saves happen only in the job for pushes, and only on the default branch.
    const push = all.get('default-branch') ?? '';
    expect(push).toContain("if: github.event_name == 'push'");
    const saves = push.split('- uses: ').filter((step) => step.startsWith('actions/cache/save@'));
    expect(saves.length).toBe(3);
    for (const save of saves) {
      expect(save).toContain('github.ref_name == github.event.repository.default_branch');
    }
    // The shared records are saved only when they hold more than was restored.
    expect(saves.find((save) => save.includes(`path: ${SHARED}`))).toContain(
      `!endsWith(steps.firescale-cache.outputs.cache-matched-key, hashFiles('${SHARED}/*.json'))`,
    );
    // A new push to a pull request cancels its run in progress; each push has a group of its own.
    expect(workflow).toContain(
      "concurrency:\n  group: ci-${{ github.event.pull_request.number || github.run_id }}\n  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n",
    );
    // Every action is pinned to a commit.
    for (const match of workflow.matchAll(/uses: (\S+)/g)) {
      expect(match[1]).toMatch(/^[\w-]+\/[\w/-]+@[0-9a-f]{40}$/);
    }
  });
});

/** GitHub Actions' variables for a run of `event` on `ref`, in the repo at `root`. */
async function github(
  root: string,
  event: 'push' | 'pull_request',
  ref: string,
  extra: Record<string, string | undefined> = {},
): Promise<Record<string, string | undefined>> {
  const sha = (await runGit(root, ['rev-parse', 'HEAD'])).stdout.trim();
  const outside = path.join(root, '..', `${path.basename(root)}-event.json`);
  await writeFile(outside, JSON.stringify({ repository: { default_branch: 'main' } }));
  return {
    GITHUB_ACTIONS: 'true',
    GITHUB_EVENT_NAME: event,
    GITHUB_REF: ref,
    GITHUB_SHA: sha,
    GITHUB_REPOSITORY: 'owner/repo',
    GITHUB_WORKFLOW_REF: `owner/repo/.github/workflows/ci.yml@${ref}`,
    GITHUB_EVENT_PATH: outside,
    GITHUB_WORKSPACE: root,
    ImageOS: 'ubuntu24',
    ImageVersion: '20261109.1',
    ...extra,
  };
}

const WORKFLOW = 'name: CI\non: [pull_request, push]\njobs: {}\n';

/** A committed repo on main whose one test step logs each run. */
async function sharedRepo() {
  const outside = await fixtures.create({ files: {}, track: 'no-git' });
  const log = path.join(outside.root, 'runs.log');
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: 'node test.js' },
      }),
      '.gitignore': '.firescale/\n',
      'test.js': `require('fs').appendFileSync(${JSON.stringify(log)}, 'test\\n'); console.log('ok');\n`,
      'src.txt': 'one\n',
      '.github/workflows/ci.yml': WORKFLOW,
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'initial');
  const runs = async (): Promise<number> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean).length;
  const records = async (): Promise<string[]> =>
    (await readdir(path.join(root, ...SHARED.split('/'))).catch(() => [] as string[])).sort();
  /** What a fresh runner has: no local step cache, only what the workflow restored. */
  const freshRunner = () =>
    rm(path.join(root, '.firescale', 'steps'), { recursive: true, force: true });
  return { root, runs, records, freshRunner };
}

interface StepJson {
  name: string;
  status: string;
  cache?: { sharedFrom?: string };
}

async function verifyJson(
  root: string,
  env: Record<string, string | undefined>,
): Promise<{ steps: StepJson[]; notes?: string[] }> {
  const result = await runCli(['verify', '--json', '--shared-cache', SHARED], root, env);
  expect(result.code, result.stdout + result.stderr).toBe(EXIT_OK);
  return JSON.parse(result.stdout) as { steps: StepJson[]; notes?: string[] };
}

const testStep = (report: { steps: StepJson[] }): StepJson | undefined =>
  report.steps.find((step) => step.name === 'test');

describe('the shared cache of CI', () => {
  it('keeps small records written on the default branch, and a pull request reuses them on an exact key', async () => {
    const { root, runs, records, freshRunner } = await sharedRepo();
    const push = await github(root, 'push', 'refs/heads/main');
    await verifyJson(root, push);
    expect(await runs()).toBe(1);
    const written = await records();
    expect(written).toHaveLength(1);
    const record = JSON.parse(
      await readFile(path.join(root, ...SHARED.split('/'), written[0] ?? ''), 'utf8'),
    ) as Record<string, unknown>;
    expect(Object.keys(record).sort()).toEqual([
      'durationMs',
      'format',
      'key',
      'outputDigest',
      'result',
      'time',
      'writer',
    ]);
    expect(record.result).toBe('passed');
    expect(record.writer).toEqual({
      workflow: 'owner/repo/.github/workflows/ci.yml@refs/heads/main',
      ref: 'refs/heads/main',
      commit: push.GITHUB_SHA,
    });
    expect(`${record.key as string}.json`).toBe(written[0]);
    expect(JSON.stringify(record).length).toBeLessThan(600);

    // A pull request on a fresh runner: served from the record, never run, nothing written.
    await freshRunner();
    const pull = await github(root, 'pull_request', 'refs/pull/7/merge');
    const hit = await verifyJson(root, pull);
    expect(await runs()).toBe(1);
    expect(testStep(hit)?.cache?.sharedFrom).toBe(push.GITHUB_SHA);
    expect(await records()).toEqual(written);
    const human = await runCli(['verify', '--shared-cache', SHARED], root, pull);
    expect(human.stdout).toMatch(
      /from the shared cache of CI \(kept by the default branch at [0-9a-f]{7}\)/,
    );
  });

  it('uses a record only when the whole key matches', async () => {
    const { root, runs, freshRunner } = await sharedRepo();
    await verifyJson(root, await github(root, 'push', 'refs/heads/main'));
    expect(await runs()).toBe(1);
    const pull = await github(root, 'pull_request', 'refs/pull/7/merge');
    // Another runner image.
    await freshRunner();
    await verifyJson(root, { ...pull, ImageVersion: '20261116.1' });
    expect(await runs()).toBe(2);
    // Another workflow file.
    await freshRunner();
    await writeFile(path.join(root, '.github', 'workflows', 'ci.yml'), 'name: CI 2\non: [push]\n');
    await verifyJson(root, pull);
    expect(await runs()).toBe(3);
    // Its exact bytes back: a `git checkout` may write CRLF on Windows,
    // which is another workflow file to the key.
    await writeFile(path.join(root, '.github', 'workflows', 'ci.yml'), WORKFLOW);
    // Another file the step reads.
    await freshRunner();
    await writeFile(path.join(root, 'src.txt'), 'two\n');
    await verifyJson(root, pull);
    expect(await runs()).toBe(4);
    // Everything as it was: a hit again.
    await freshRunner();
    await writeFile(path.join(root, 'src.txt'), 'one\n');
    await verifyJson(root, pull);
    expect(await runs()).toBe(4);
  });

  it('never uses a record that was not written from the default branch, and a pull request never writes', async () => {
    const { root, runs, records, freshRunner } = await sharedRepo();
    // A pull request run writes nothing.
    await verifyJson(root, await github(root, 'pull_request', 'refs/pull/7/merge'));
    expect(await records()).toEqual([]);
    // Nor does a push to another branch.
    await freshRunner();
    await verifyJson(root, await github(root, 'push', 'refs/heads/feature'));
    expect(await records()).toEqual([]);
    expect(await runs()).toBe(2);
    // The push to the default branch never takes a pass from this machine's
    // own cache, which the runs above filled: it runs the step itself.
    await verifyJson(root, await github(root, 'push', 'refs/heads/main'));
    expect(await runs()).toBe(3);
    const [name] = await records();
    const file = path.join(root, ...SHARED.split('/'), name ?? '');
    const record = JSON.parse(await readFile(file, 'utf8')) as {
      writer: { ref: string };
    };
    // The same record, said to come from another branch, is not used.
    record.writer.ref = 'refs/heads/feature';
    await writeFile(file, JSON.stringify(record));
    await freshRunner();
    const pull = await github(root, 'pull_request', 'refs/pull/7/merge');
    const report = await verifyJson(root, pull);
    expect(testStep(report)?.cache).toBeUndefined();
    expect(await runs()).toBe(4);
    await freshRunner();
    const log = await runCli(['verify', '--shared-cache', SHARED], root, {
      ...pull,
      FSCALE_LOG: 'cache',
    });
    expect(log.stdout + log.stderr).toContain(
      'shared record not used: it was written from refs/heads/feature, not from the default branch refs/heads/main',
    );
  });

  it('never deletes anything but records, and refuses the repo or a folder above it', async () => {
    const { root } = await sharedRepo();
    const push = await github(root, 'push', 'refs/heads/main');
    for (const dir of ['.', '..', '.firescale', '.firescale/steps']) {
      const result = await runCli(['verify', '--json', '--shared-cache', dir], root, push);
      expect(result.code, result.stderr).toBe(EXIT_OK);
      const report = JSON.parse(result.stdout) as { notes?: string[] };
      expect(report.notes?.join('\n')).toContain(`--shared-cache ${dir} was not used`);
    }
    expect((await runGit(root, ['status', '--porcelain'])).stdout).toBe('');
    expect(await readFile(path.join(root, 'src.txt'), 'utf8')).toBe('one\n');
    // In a folder of its own, files that are not records are left alone.
    const own = path.join(root, 'records');
    await mkdir(path.join(own, 'keep'), { recursive: true });
    await writeFile(path.join(own, 'notes.txt'), 'mine\n');
    await writeFile(path.join(own, 'keep', 'a.json'), '{}');
    await runCli(['verify', '--shared-cache', 'records'], root, push);
    expect(await readFile(path.join(own, 'notes.txt'), 'utf8')).toBe('mine\n');
    expect(await readFile(path.join(own, 'keep', 'a.json'), 'utf8')).toBe('{}');
  });

  it('is neither read nor written on a laptop', async () => {
    const { root, runs, records } = await sharedRepo();
    await verifyJson(root, await github(root, 'push', 'refs/heads/main'));
    await rm(path.join(root, '.firescale', 'steps'), { recursive: true, force: true });
    const laptop = {
      GITHUB_ACTIONS: undefined,
      GITHUB_EVENT_NAME: undefined,
      GITHUB_REF: undefined,
      GITHUB_EVENT_PATH: undefined,
    };
    const before = await records();
    const report = await verifyJson(root, laptop);
    expect(testStep(report)?.cache).toBeUndefined();
    expect(await runs()).toBe(2);
    expect(report.notes?.join('\n')).toContain(
      '--shared-cache is used only in GitHub Actions (GITHUB_ACTIONS=true)',
    );
    expect(await records()).toEqual(before);
  });
});

describe('a shallow clone', () => {
  it('runs every project with a note when the merge base is not in it, and compares once the history is there', async () => {
    const origin = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: {
            lint: false,
            typecheck: false,
            test: false,
            projects: {
              a: { lint: false, typecheck: false, test: 'node -e 0' },
              b: { lint: false, typecheck: false, test: 'node -e 0' },
            },
          },
        }),
        '.gitignore': '.firescale/\n',
        'a/package.json': '{"name":"a"}\n',
        'a/index.js': 'module.exports = 1;\n',
        'b/package.json': '{"name":"b"}\n',
        'b/index.js': 'module.exports = 2;\n',
      },
      track: 'no-git',
    });
    const source = origin.root;
    expect((await runGit(source, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
    await commitAll(source, 'base');
    await writeFile(path.join(source, 'b', 'index.js'), 'module.exports = 3;\n');
    await commitAll(source, 'main moves on');
    expect((await runGit(source, ['checkout', '--quiet', '-b', 'feature', 'HEAD~1'])).code).toBe(0);
    await writeFile(path.join(source, 'a', 'index.js'), 'module.exports = 4;\n');
    await commitAll(source, 'change a');
    await writeFile(path.join(source, 'a', 'more.js'), '1;\n');
    await commitAll(source, 'change a again');

    const outside = await fixtures.create({ files: {}, track: 'no-git' });
    const clone = path.join(outside.root, 'clone');
    await mkdir(clone);
    const url = pathToFileURL(source).href;
    // What actions/checkout does with its default fetch-depth of 1.
    for (const args of [
      ['init', '--quiet', '--initial-branch=feature'],
      ['remote', 'add', 'origin', url],
      ['fetch', '--quiet', '--depth=1', 'origin', 'feature', 'main'],
      ['checkout', '--quiet', '-b', 'feature', 'origin/feature'],
    ]) {
      const result = await runGit(clone, args);
      expect(result.code, `git ${args.join(' ')}: ${result.stderr}`).toBe(0);
    }
    expect((await runGit(clone, ['rev-parse', '--is-shallow-repository'])).stdout.trim()).toBe(
      'true',
    );
    const shallow = await runCli(
      ['verify', '--affected', '--base', 'origin/main', '--json'],
      clone,
    );
    expect(shallow.code, shallow.stderr).toBe(EXIT_OK);
    const report = JSON.parse(shallow.stdout) as {
      affected: { everything: boolean; reason?: string; projectsRun: number };
      base?: { compared: boolean; reason?: string };
      notes?: string[];
    };
    expect(report.affected.everything).toBe(true);
    expect(report.affected.reason).toContain('this clone is shallow');
    expect(report.affected.projectsRun).toBe(2);
    expect(report.base?.compared).toBe(false);
    expect(report.base?.reason).toContain('fetch-depth: 0');
    expect(report.notes?.join('\n')).toContain('this clone is shallow');
    // A name that does not resolve is a typo, not a shallow clone.
    const typo = await runCli(['verify', '--affected', '--base', 'origin/mian'], clone);
    expect(typo.code).toBe(EXIT_USAGE);
    expect(typo.stderr).toContain('--base "origin/mian" is not a commit');

    // With the whole history (fetch-depth: 0), only a is affected and there is a base.
    expect((await runGit(clone, ['fetch', '--quiet', '--unshallow', 'origin'])).code).toBe(0);
    const full = await runCli(['verify', '--affected', '--base', 'origin/main', '--json'], clone);
    expect(full.code, full.stderr).toBe(EXIT_OK);
    const fullReport = JSON.parse(full.stdout) as typeof report;
    expect(fullReport.affected.everything).toBe(false);
    expect(fullReport.affected.projectsRun).toBe(1);
  });
});
