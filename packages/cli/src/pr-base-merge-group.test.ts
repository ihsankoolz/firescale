import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, runCli } from './test-helpers.js';

// `verify --pr-base` in a merge queue run: the base comes from the
// merge_group event's payload, never from a ref in the clone (ADR 0080).

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const pkg = (name: string) =>
  `${JSON.stringify({ name, version: '1.0.0', scripts: { test: 'node -e 0' } }, null, 2)}\n`;

async function git(root: string, ...argv: string[]): Promise<string> {
  const result = await runGit(root, [
    '-c',
    'user.name=fixture',
    '-c',
    'user.email=fixture@example.invalid',
    '-c',
    'commit.gpgsign=false',
    ...argv,
  ]);
  expect(result.code, result.stderr).toBe(0);
  return result.stdout.trim();
}

interface Queue {
  repo: FixtureRepo;
  /** The commit the group is built on. */
  base: string;
  /** The commit of the merge group, which is checked out. */
  head: string;
  /** A commit before the base. */
  older: string;
}

/** Two services. The merge group's commit changes only `a`. */
async function queue(): Promise<Queue> {
  const repo = await fixtures.create({
    files: {
      'a/package.json': pkg('a'),
      'a/index.js': 'export const a = 1;\n',
      'c/package.json': pkg('c'),
      'c/index.js': 'export const c = 1;\n',
    },
  });
  await git(repo.root, 'commit', '--quiet', '--no-verify', '-m', 'older');
  const older = await git(repo.root, 'rev-parse', 'HEAD');
  await repo.write('c/index.js', 'export const c = 2;\n');
  await git(repo.root, 'add', '-A');
  await git(repo.root, 'commit', '--quiet', '--no-verify', '-m', 'base');
  const base = await git(repo.root, 'rev-parse', 'HEAD');
  await repo.write('a/index.js', 'export const a = 2;\n');
  await git(repo.root, 'add', '-A');
  await git(repo.root, 'commit', '--quiet', '--no-verify', '-m', 'merge group');
  const head = await git(repo.root, 'rev-parse', 'HEAD');
  return { repo, base, head, older };
}

/** The environment of a GitHub Actions run for a `merge_group` event. */
async function event(
  root: string,
  group: Record<string, unknown>,
  sha: string,
): Promise<Record<string, string | undefined>> {
  const file = path.join(root, '..', `${path.basename(root)}-merge-group.json`);
  await writeFile(file, JSON.stringify({ action: 'checks_requested', merge_group: group }));
  return {
    GITHUB_ACTIONS: 'true',
    GITHUB_EVENT_NAME: 'merge_group',
    GITHUB_EVENT_PATH: file,
    GITHUB_SHA: sha,
    GITHUB_REF: 'refs/heads/gh-readonly-queue/main/pr-7-0000',
  };
}

interface Plan {
  base: string | null;
  units: { project?: string; affected: boolean }[];
}

const PLAN = ['verify', '--affected', '--pr-base', '--dry-run', '--json'];

describe('verify --pr-base in a merge queue run', () => {
  it('takes the base from the merge_group event', async () => {
    const { repo, base, head } = await queue();
    const env = await event(
      repo.root,
      {
        base_sha: base,
        head_sha: head,
        base_ref: 'refs/heads/main',
        head_ref: 'refs/heads/gh-readonly-queue/main/pr-7-0000',
      },
      head,
    );
    const result = await runCli(PLAN, repo.root, env);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    const plan = JSON.parse(result.stdout) as Plan;
    expect(plan.base).toBe(base);
    expect(plan.units.map((unit) => [unit.project, unit.affected])).toEqual([
      ['a', true],
      ['c', false],
    ]);
  });

  it('never reads a ref: the branches the event names may be missing or moved', async () => {
    const { repo, base, head, older } = await queue();
    // A branch with the base branch's name, at another commit, and the
    // event's own refs name branches the clone does not have.
    await git(repo.root, 'branch', '-f', 'release', older);
    const env = await event(
      repo.root,
      {
        base_sha: base,
        head_sha: head,
        base_ref: 'refs/heads/release',
        head_ref: 'refs/heads/gh-readonly-queue/release/pr-7-0000',
      },
      head,
    );
    const result = await runCli(PLAN, repo.root, env);
    expect(result.code, result.stderr).toBe(EXIT_OK);
    expect((JSON.parse(result.stdout) as Plan).base).toBe(base);
  });

  it('refuses an event that names no commits, or a base that is no ancestor of the group', async () => {
    const { repo, base, head } = await queue();
    const none = await runCli(PLAN, repo.root, await event(repo.root, { head_sha: head }, head));
    expect(none.code).not.toBe(EXIT_OK);
    expect(none.stderr).toContain('merge_group.base_sha');

    const short = await runCli(
      PLAN,
      repo.root,
      await event(repo.root, { base_sha: 'main', head_sha: head }, head),
    );
    expect(short.code).not.toBe(EXIT_OK);
    expect(short.stderr).toContain('merge_group.base_sha');

    // The base and the head swapped: the "base" is not part of the group.
    const swapped = await runCli(
      PLAN,
      repo.root,
      await event(repo.root, { base_sha: head, head_sha: base }, base),
    );
    expect(swapped.code).not.toBe(EXIT_OK);
  });

  it('refuses when the checkout or GITHUB_SHA is not the merge group commit', async () => {
    const { repo, base, head, older } = await queue();
    const group = { base_sha: base, head_sha: head };
    const other = await runCli(PLAN, repo.root, await event(repo.root, group, older));
    expect(other.code).not.toBe(EXIT_OK);
    expect(other.stderr).toContain('GITHUB_SHA');

    await git(repo.root, 'checkout', '--quiet', '--detach', base);
    const moved = await runCli(PLAN, repo.root, await event(repo.root, group, head));
    expect(moved.code).not.toBe(EXIT_OK);
    expect(moved.stderr).toContain('the checkout is at');
  });

  it('is still refused by tamper, which needs the pull request', async () => {
    const { repo, base, head } = await queue();
    const env = await event(repo.root, { base_sha: base, head_sha: head }, head);
    const result = await runCli(['tamper', '--pr-base'], repo.root, env);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stderr).toContain('merge_group');
  });
});
