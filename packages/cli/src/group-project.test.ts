/**
 * The group-project pack through the real binary (M11, ADR 0023): `init`
 * builds a repo it passes, the generated files are pinned by snapshots, and
 * a CI workflow a person wrote is never rewritten.
 */
import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles } from './course-fixture.js';
import { EXIT_OK, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const run = promisify(execFile);

interface Diagnostic {
  ruleId: string;
  file: string;
  fixable: 'deterministic' | 'agent' | 'manual';
  message: string;
}

const CONFIG = '{ "packs": ["core", "group-project"] }\n';

async function diagnostics(root: string, env: Record<string, string> = {}): Promise<Diagnostic[]> {
  const result = await runCli(['check', '--json', '--no-cache'], root, env);
  return (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
}

const groupProject = (found: Diagnostic[]): Diagnostic[] =>
  found.filter((d) => d.ruleId.startsWith('group-project/'));

/**
 * An environment where git reads only a config of the test's own, so the
 * developer's or runner's `init.defaultBranch` cannot change the result.
 */
async function gitConfig(content: string): Promise<Record<string, string>> {
  const home = await fixtures.create({ files: { gitconfig: content }, track: 'no-git' });
  return { GIT_CONFIG_GLOBAL: path.join(home.root, 'gitconfig'), GIT_CONFIG_NOSYSTEM: '1' };
}

/** Puts a repo on `branch` (before or after its first commit). */
async function onBranch(root: string, branch: string): Promise<void> {
  await run('git', ['symbolic-ref', 'HEAD', `refs/heads/${branch}`], { cwd: root });
}

describe('fscale init --packs core,group-project', () => {
  it('builds a repo where the pack reports no violations, with the template and guide pinned', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const isolated = await gitConfig('');
    const result = await runCli(['init', '--packs', 'core,group-project'], repo.root, isolated);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    expect(await repo.read('firescale.config.json')).toBe(
      '{\n  "packs": ["core", "group-project"]\n}\n',
    );
    // A pull request run on GitHub must not change the answer for a new repo.
    expect(
      groupProject(await diagnostics(repo.root, { ...isolated, GITHUB_HEAD_REF: 'my-stuff' })),
    ).toEqual([]);
    expect(groupProject(await diagnostics(repo.root, isolated))).toEqual([]);

    const template = await repo.read('.github/pull_request_template.md');
    const guide = await repo.read('CONTRIBUTING.md');
    expect(template).toMatchSnapshot('pull_request_template.md');
    expect(guide).toMatchSnapshot('CONTRIBUTING.md');
    // One workflow, written once, that runs verify.
    expect(await repo.read('.github/workflows/ci.yml')).toContain('- run: npx firescale verify');
    expect(await repo.trackedFiles()).toEqual(
      expect.arrayContaining([
        '.github/pull_request_template.md',
        '.github/workflows/ci.yml',
        'CONTRIBUTING.md',
      ]),
    );

    // sync finds nothing to do.
    const check = await runCli(['sync', '--check'], repo.root, isolated);
    expect(check.code).toBe(EXIT_OK);
  });

  it('accepts the branch git gives a new repo, whatever init.defaultBranch says', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const trunk = await gitConfig('[init]\n\tdefaultBranch = trunk\n');
    const result = await runCli(['init', '--packs', 'core,group-project'], repo.root, trunk);
    expect(result.code).toBe(EXIT_OK);
    const { stdout } = await run('git', ['symbolic-ref', '--short', 'HEAD'], {
      cwd: repo.root,
      env: { ...process.env, ...trunk },
    });
    expect(stdout.trim()).toBe('trunk');
    expect(groupProject(await diagnostics(repo.root, trunk))).toEqual([]);
  });

  it('writes the commands of each project into the guide', async () => {
    const repo = await fixtures.create({
      files: { ...courseProjectFiles(), 'firescale.config.json': CONFIG },
    });
    const result = await runCli(['fix', '--rules', 'group-project/contributing'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(await repo.read('CONTRIBUTING.md')).toMatchSnapshot(
      'CONTRIBUTING.md of the course project',
    );
  });
});

describe('a CI workflow a person wrote', () => {
  const HAND_WRITTEN =
    '# Our CI. Do not touch.\r\nname: tests\r\non: [pull_request]\r\njobs:\r\n  test:\r\n    runs-on: ubuntu-latest\r\n    steps:\r\n      - run: npm ci\r\n      - run: npm test\r\n';

  it('is never rewritten by adopt, sync or fix: the missing verify step is left for an agent', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': CONFIG,
        '.github/workflows/tests.yml': HAND_WRITTEN,
        'README.md': '# App\n\n## Install\n\nnpm ci\n\n## Usage\n\nnpm test\n',
      },
    });
    const file = path.join(repo.root, '.github', 'workflows', 'tests.yml');
    for (const argv of [['adopt'], ['sync'], ['fix']]) {
      const result = await runCli(argv, repo.root);
      expect(result.code, argv.join(' ')).toBe(EXIT_OK);
      expect(await readFile(file, 'utf8'), argv.join(' ')).toBe(HAND_WRITTEN);
    }
    expect(await repo.read('.github/workflows/ci.yml')).toBeNull();
    const left = groupProject(await diagnostics(repo.root));
    expect(left.map((d) => `${d.ruleId} ${d.fixable} ${d.file}`)).toEqual([
      'group-project/ci-verify agent .github/workflows/tests.yml',
    ]);
  });
});

describe('group-project/branch-name', () => {
  it('reports a branch whose name does not match, and not one that does', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': CONFIG, 'README.md': '# App\n' },
    });
    await onBranch(repo.root, 'my-stuff');
    const bad = groupProject(await diagnostics(repo.root)).filter(
      (d) => d.ruleId === 'group-project/branch-name',
    );
    expect(bad).toHaveLength(1);
    expect(bad[0]?.fixable).toBe('manual');
    expect(bad[0]?.message).toContain('git branch -m feat/my-stuff');

    await onBranch(repo.root, 'feat/my-stuff');
    const good = groupProject(await diagnostics(repo.root)).filter(
      (d) => d.ruleId === 'group-project/branch-name',
    );
    expect(good).toEqual([]);
  });
});
