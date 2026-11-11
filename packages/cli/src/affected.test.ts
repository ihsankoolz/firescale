import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import {
  ENFORCED_CONFIG,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_ENV,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

/** Commits everything that is staged, with a throwaway identity. */
async function commit(repo: FixtureRepo, message: string): Promise<void> {
  const result = await runGit(repo.root, [
    '-c',
    'user.name=fixture',
    '-c',
    'user.email=fixture@example.invalid',
    '-c',
    'commit.gpgsign=false',
    'commit',
    '--quiet',
    '--no-verify',
    '-m',
    message,
  ]);
  expect(result.stderr).toBe('');
  expect(result.code).toBe(0);
}

async function files(repo: FixtureRepo, argv: string[]): Promise<string[]> {
  const result = await runCli(['check', '--json', ...argv], repo.root);
  return (JSON.parse(result.stdout) as CheckReport).diagnostics.map((d) => d.file);
}

/** A workspace where app depends on lib, and other stands alone. Each has a committed env file. */
async function workspace(): Promise<FixtureRepo> {
  const pkg = (name: string, deps: Record<string, string> = {}) =>
    `${JSON.stringify({ name, version: '1.0.0', dependencies: deps }, null, 2)}\n`;
  const repo = await fixtures.create({
    files: {
      'package.json': pkg('root'),
      'pnpm-workspace.yaml': "packages:\n  - 'packages/*'\n",
      'firescale.config.json': ENFORCED_CONFIG,
      'packages/lib/package.json': pkg('lib'),
      'packages/lib/.env': FAKE_ENV,
      'packages/lib/index.js': 'export const a = 1;\n',
      'packages/app/package.json': pkg('app', { lib: 'workspace:*' }),
      'packages/app/.env': FAKE_ENV,
      'packages/other/package.json': pkg('other'),
      'packages/other/.env': FAKE_ENV,
    },
  });
  await commit(repo, 'initial');
  return repo;
}

describe('fscale check --affected', () => {
  it('reports nothing when the working tree matches the last commit', async () => {
    const repo = await workspace();
    expect((await runCli(['check'], repo.root)).code).toBe(EXIT_VIOLATIONS);
    const affected = await runCli(['check', '--affected'], repo.root);
    expect(affected.code).toBe(EXIT_OK);
    expect(affected.stdout).toBe('OK: 1 rule checked, no violations.\n');
  });

  it('reports a newly tracked file, and nothing else', async () => {
    const repo = await workspace();
    await repo.write('tools/.env.local', FAKE_ENV);
    await repo.track(['tools/.env.local']);
    expect(await files(repo, ['--affected'])).toEqual(['tools/.env.local']);
    expect((await runCli(['check', '--affected'], repo.root)).code).toBe(EXIT_VIOLATIONS);
  });

  it('includes projects that depend on a changed project', async () => {
    const repo = await workspace();
    await repo.write('packages/lib/index.js', 'export const a = 2;\n');
    // lib changed, app depends on lib, other does not.
    expect(await files(repo, ['--affected'])).toEqual(['packages/app/.env', 'packages/lib/.env']);
    expect(await files(repo, [])).toEqual([
      'packages/app/.env',
      'packages/lib/.env',
      'packages/other/.env',
    ]);
  });

  it('counts untracked new files as changes', async () => {
    const repo = await workspace();
    await repo.write('packages/other/new-file.js', 'export {};\n');
    expect(await files(repo, ['--affected'])).toEqual(['packages/other/.env']);
  });

  it('--base also counts commits since the merge base', async () => {
    const repo = await workspace();
    await repo.write('packages/other/index.js', 'export {};\n');
    await repo.track(['packages/other/index.js']);
    await commit(repo, 'second');
    expect(await files(repo, ['--affected'])).toEqual([]);
    expect(await files(repo, ['--affected', '--base', 'HEAD~1'])).toEqual(['packages/other/.env']);
  });

  it('works when the firescale root is a folder inside a larger git repo', async () => {
    const repo = await fixtures.create({
      files: {
        'README.md': '# outer\n',
        'service/firescale.config.json': ENFORCED_CONFIG,
        'service/.env': FAKE_ENV,
      },
    });
    await commit(repo, 'initial');
    const service = path.join(repo.root, 'service');
    expect((await runCli(['check', '--affected', '--cwd', service])).code).toBe(EXIT_OK);
    await repo.write('service/sub/.env.local', FAKE_ENV);
    await repo.track(['service/sub/.env.local']);
    const result = await runCli(['check', '--json', '--affected', '--cwd', service]);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.diagnostics.map((d) => d.file)).toEqual(['sub/.env.local']);
  });

  it('keeps the summary and exit code consistent with the narrowed report', async () => {
    const repo = await workspace();
    await repo.write('packages/other/new-file.js', 'export {};\n');
    const result = await runCli(['check', '--json', '--affected'], repo.root);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.summary).toEqual({ rules: 1, violations: 1, enforcedViolations: 1 });
    expect(report.rules[0]?.violations).toBe(1);
    expect(report.ok).toBe(false);
    expect(result.code).toBe(EXIT_VIOLATIONS);
  });

  it('treats every file as changed when there is no commit yet', async () => {
    const repo = await fixtures.create({
      files: { '.env': FAKE_ENV, 'firescale.config.json': ENFORCED_CONFIG },
    });
    const result = await runCli(['check', '--affected'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stderr).toContain('no commit to compare with');
  });

  it('rejects --base without --affected, and a base that does not exist', async () => {
    const repo = await workspace();
    const alone = await runCli(['check', '--base', 'HEAD'], repo.root);
    expect(alone.code).toBe(EXIT_USAGE);
    expect(alone.stderr).toContain('--base only works together with --affected');
    const missing = await runCli(['check', '--affected', '--base', 'no-such-ref'], repo.root);
    expect(missing.code).toBe(EXIT_USAGE);
    expect(missing.stderr).toContain('--base "no-such-ref" is not a commit');
  });
});
