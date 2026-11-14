import { mkdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_USAGE, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

// `fscale tamper` and the base of a pull request as GitHub gives it (M25,
// ADR 0045): a change cannot move what it is compared with by moving a ref
// in the clone or replacing an object, and a commit can be judged from
// git's objects without checking it out.

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const MATH = 'export const add = (a: number, b: number): number => a + b;\n';
const TESTS =
  'import { add } from "./math";\n\nit("adds", () => {\n  expect(add(1, 2)).toBe(3);\n});\n\nit("adds zero", () => {\n  expect(add(0, 0)).toBe(0);\n});\n';
const ONE_TEST =
  'import { add } from "./math";\n\nit("adds", () => {\n  expect(add(1, 2)).toBe(3);\n});\n';

const config = (tamper = 'evaluated') =>
  `${JSON.stringify({ packs: ['core'], verify: { lint: false, typecheck: false, test: false, tamper } })}\n`;

interface Report {
  base: string;
  policy: string;
  findings: { kind: string; file: string; line: number; onBase: boolean; what: string }[];
}

async function git(root: string, ...args: string[]): Promise<string> {
  const result = await runGit(root, [
    '-c',
    'user.name=fixture',
    '-c',
    'user.email=fixture@example.invalid',
    '-c',
    'commit.gpgsign=false',
    ...args,
  ]);
  if (result.code !== 0) throw new Error(`git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

/**
 * main: the base. A branch `change` deletes a test. With `merge`, a merge
 * commit of the branch into main is made (as GitHub's refs/pull/N/merge)
 * and checked out.
 */
async function prRepo(tamper = 'evaluated'): Promise<{ root: string; base: string; head: string }> {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': config(tamper),
      '.gitignore': '.firescale/\n',
      'src/math.ts': MATH,
      'src/math.test.ts': TESTS,
    },
    track: 'no-git',
  });
  const root = repo.root;
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await commitAll(root, 'base');
  const base = await git(root, 'rev-parse', 'HEAD');
  await git(root, 'checkout', '--quiet', '-b', 'change');
  await writeFile(path.join(root, 'src', 'math.test.ts'), ONE_TEST);
  await commitAll(root, 'delete a test');
  const head = await git(root, 'rev-parse', 'HEAD');
  return { root, base, head };
}

const parse = (stdout: string): Report => JSON.parse(stdout) as Report;

describe('fscale tamper --head', () => {
  it('judges a commit from git objects, whatever the working tree holds', async () => {
    const { root, base, head } = await prRepo();
    // The working tree puts the test back; the commit still deleted it.
    await writeFile(path.join(root, 'src', 'math.test.ts'), TESTS);
    const result = await runCli(['tamper', '--json', '--head', head, '--base', 'main'], root);
    expect(result.code, result.stderr).toBe(EXIT_OK);
    const report = parse(result.stdout);
    expect(report.base).toBe(base);
    expect(report.findings.map((f) => f.kind).sort()).toEqual([
      'assertion-removed',
      'test-deleted',
    ]);
    expect(report.findings.find((f) => f.kind === 'test-deleted')).toMatchObject({
      file: 'src/math.test.ts',
      line: 7,
      onBase: true,
      what: 'adds zero',
    });
    // Without --head the working tree is compared, and it is clean.
    const tree = parse((await runCli(['tamper', '--json', '--base', 'main'], root)).stdout);
    expect(tree.findings).toEqual([]);
  });

  it('defaults the base to the parent of the head commit, and blocks when enforced', async () => {
    const { root, head } = await prRepo('enforced');
    const result = await runCli(['tamper', '--head', head], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout.split('\n')[0]).toBe('tamper: FAILED - 2 findings');
    expect(result.stdout).toContain(
      'test deleted: src/math.test.ts:7 (on the base commit): adds zero',
    );
  });

  it('refuses a head that is not a commit', async () => {
    const { root } = await prRepo();
    const result = await runCli(['tamper', '--head', 'no-such-ref'], root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('--head "no-such-ref" is not a commit');
  });
});

describe('--pr-base: the base GitHub gives, not a ref in the clone', () => {
  /** A pull_request run: the merge commit checked out, as actions/checkout does. */
  async function pullRequestRun(): Promise<{
    root: string;
    base: string;
    head: string;
    env: Record<string, string>;
  }> {
    const { root, base, head } = await prRepo();
    await git(root, 'checkout', '--quiet', 'main');
    await git(root, 'merge', '--quiet', '--no-ff', '-m', 'Merge pull request', 'change');
    const merge = await git(root, 'rev-parse', 'HEAD');
    await git(root, 'checkout', '--quiet', '--detach', merge);
    const event = path.join(root, '..', `event-${path.basename(root)}.json`);
    await writeFile(
      event,
      JSON.stringify({ pull_request: { base: { sha: base }, head: { sha: head } } }),
    );
    return {
      root,
      base,
      head,
      env: {
        GITHUB_ACTIONS: 'true',
        GITHUB_EVENT_NAME: 'pull_request',
        GITHUB_EVENT_PATH: event,
        GITHUB_SHA: merge,
      },
    };
  }

  it('compares with the merge commit first parent, even after the refs in the clone moved', async () => {
    const { root, base, env } = await pullRequestRun();
    // Code from the change, run during install or tests, moves the base
    // branch onto the change and replaces the base commit.
    await git(root, 'update-ref', 'refs/heads/main', 'change');
    await git(root, 'replace', base, 'change');
    for (const command of [
      ['tamper', '--json', '--pr-base'],
      ['verify', '--json', '--pr-base'],
    ]) {
      const result = await runCli(command, root, env);
      expect(result.code, result.stderr).toBe(EXIT_OK);
      const report =
        command[0] === 'tamper'
          ? parse(result.stdout)
          : (JSON.parse(result.stdout) as { tamper: Report }).tamper;
      expect(report.base).toBe(base);
      expect(report.findings.map((f) => f.kind).sort()).toEqual([
        'assertion-removed',
        'test-deleted',
      ]);
    }
    // A --base resolved from the moved ref would have seen nothing.
    const moved = parse((await runCli(['tamper', '--json', '--base', 'main'], root, env)).stdout);
    expect(moved.findings).toEqual([]);
  });

  it('reads a pull_request_target run: the base branch checked out, the head as objects', async () => {
    const { root, base, head } = await prRepo();
    await git(root, 'checkout', '--quiet', 'main');
    const event = path.join(root, '..', `event-target-${path.basename(root)}.json`);
    await writeFile(
      event,
      JSON.stringify({ pull_request: { base: { sha: base }, head: { sha: head } } }),
    );
    const result = await runCli(['tamper', '--json', '--pr-base'], root, {
      GITHUB_ACTIONS: 'true',
      GITHUB_EVENT_NAME: 'pull_request_target',
      GITHUB_EVENT_PATH: event,
      GITHUB_SHA: base,
    });
    expect(result.code, result.stderr).toBe(EXIT_OK);
    expect(
      parse(result.stdout)
        .findings.map((f) => f.kind)
        .sort(),
    ).toEqual(['assertion-removed', 'test-deleted']);
    // Annotations, in GitHub Actions, without --json.
    const human = await runCli(['tamper', '--pr-base'], root, {
      GITHUB_ACTIONS: 'true',
      GITHUB_EVENT_NAME: 'pull_request_target',
      GITHUB_EVENT_PATH: event,
      GITHUB_SHA: base,
    });
    expect(human.stdout).toContain(
      '::warning file=src/math.test.ts,title=tamper guard%3A test deleted::adds zero (line 7 on the base commit)',
    );
  });

  it('judges what merging would change on the base tip, with the base tip policy, however old the branch point', async () => {
    const wf = (job: string) =>
      `on:\n  pull_request:\njobs:\n  build:\n    runs-on: ubuntu-latest\n${job}    steps:\n      - run: npm run build\n      - run: npm test\n`;
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': config('evaluated'),
        '.github/workflows/ci.yml': `on:\n  pull_request:\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: npm run build\n`,
      },
      track: 'no-git',
    });
    const root = repo.root;
    await git(root, 'init', '--quiet', '--initial-branch=main');
    await commitAll(root, 'old base');
    const old = await git(root, 'rev-parse', 'HEAD');
    // The base branch later adds tests to the job and enforces the guard.
    await writeFile(path.join(root, 'firescale.config.json'), config('enforced'));
    await writeFile(path.join(root, '.github', 'workflows', 'ci.yml'), wf(''));
    await commitAll(root, 'tests and enforced');
    const tip = await git(root, 'rev-parse', 'HEAD');
    // The pull request branches from the old commit and makes the job
    // unable to fail: on its own diff, only a build step is affected.
    await git(root, 'checkout', '--quiet', '-b', 'change', old);
    await writeFile(
      path.join(root, '.github', 'workflows', 'ci.yml'),
      `on:\n  pull_request:\njobs:\n  build:\n    runs-on: ubuntu-latest\n    continue-on-error: true\n    steps:\n      - run: npm run build\n`,
    );
    await commitAll(root, 'continue on error');
    const head = await git(root, 'rev-parse', 'HEAD');
    await git(root, 'checkout', '--quiet', 'main');
    const event = path.join(root, '..', `event-old-${path.basename(root)}.json`);
    await writeFile(
      event,
      JSON.stringify({ pull_request: { base: { sha: old }, head: { sha: head } } }),
    );
    const result = await runCli(['tamper', '--json', '--pr-base'], root, {
      GITHUB_ACTIONS: 'true',
      GITHUB_EVENT_NAME: 'pull_request_target',
      GITHUB_EVENT_PATH: event,
      GITHUB_SHA: tip,
    });
    expect(result.code, result.stderr).toBe(EXIT_VIOLATIONS);
    const report = parse(result.stdout);
    expect(report.base).toBe(tip);
    expect(report.policy).toBe('enforced');
    expect(report.findings.map((f) => f.what)).toContain(
      'npm test (on pull requests; cannot fail: continue-on-error: true)',
    );
  });

  it('fails instead of judging nothing when it cannot find the commits', async () => {
    const { root, env } = await pullRequestRun();
    const outside = await runCli(['tamper', '--pr-base'], root, { GITHUB_ACTIONS: undefined });
    expect(outside.code).toBe(EXIT_USAGE);
    expect(outside.stderr).toContain('this is not a GitHub Actions run');
    const push = await runCli(['verify', '--pr-base'], root, { ...env, GITHUB_EVENT_NAME: 'push' });
    expect(push.code).toBe(EXIT_USAGE);
    const missing = await runCli(['tamper', '--pr-base'], root, {
      ...env,
      GITHUB_SHA: '0123456789abcdef0123456789abcdef01234567',
    });
    expect(missing.code).toBe(EXIT_USAGE);
    expect(missing.stderr).toContain('fetch-depth: 0');
    const both = await runCli(['verify', '--pr-base', '--base', 'main'], root, env);
    expect(both.code).toBe(EXIT_USAGE);
  });
});

describe('files hidden from git', () => {
  it('compares a test file git is told not to look at', async () => {
    const { root } = await prRepo();
    await git(root, 'checkout', '--quiet', '-b', 'hide', 'main');
    for (const flag of ['--skip-worktree', '--assume-unchanged']) {
      await git(root, 'update-index', flag, 'src/math.test.ts');
      await rm(path.join(root, 'src', 'math.test.ts'));
      expect(await git(root, 'status', '--porcelain')).toBe('');
      const report = parse((await runCli(['tamper', '--json', '--base', 'main'], root)).stdout);
      expect([...new Set(report.findings.map((f) => f.kind))].sort(), flag).toEqual([
        'assertion-removed',
        'test-deleted',
      ]);
      await git(root, 'update-index', flag.replace('--', '--no-'), 'src/math.test.ts');
      await mkdir(path.join(root, 'src'), { recursive: true });
      await git(root, 'checkout', '--', 'src/math.test.ts');
    }
  });
});
