import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import {
  ENFORCED_CONFIG,
  onlyEnvRule,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_ENV,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

function failingRepo() {
  return fixtures.create({
    files: {
      '.env': FAKE_ENV,
      'apps/web/.env.local': FAKE_ENV,
      '.gitignore': 'dist\n',
      'README.md': '# fixture\n',
      'firescale.config.json': ENFORCED_CONFIG,
    },
  });
}

describe('fscale fix', () => {
  it('fixes core/no-committed-env, and a second run changes nothing', async () => {
    const repo = await failingRepo();
    expect((await runCli(['check'], repo.root)).code).toBe(EXIT_VIOLATIONS);

    const first = await runCli(['fix'], repo.root);
    expect(first.code).toBe(EXIT_OK);
    expect(first.stderr).toBe('');
    expect(first.stdout).toMatchSnapshot();
    expect(await repo.trackedFiles()).toEqual(['.gitignore', 'README.md', 'firescale.config.json']);
    expect(await repo.read('.env')).toBe(FAKE_ENV);
    const gitignore = await repo.read('.gitignore');
    expect(gitignore).toBe(
      'dist\n# Env files hold secrets. Commit an example file instead.\n.env\n.env.*\n!.env.example\n',
    );
    expect((await runCli(['check'], repo.root)).code).toBe(EXIT_OK);

    const second = await runCli(['fix'], repo.root);
    expect(second.code).toBe(EXIT_OK);
    expect(second.stdout).toBe('Nothing to fix: no deterministic fix has anything left to do.\n');
    expect(await repo.read('.gitignore')).toBe(gitignore);
    expect(await repo.trackedFiles()).toEqual(['.gitignore', 'README.md', 'firescale.config.json']);
  });

  it('--dry-run prints a diff and changes nothing', async () => {
    const repo = await failingRepo();
    const trackedBefore = await repo.trackedFiles();

    const result = await runCli(['fix', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout).toContain(
      '--- a/.gitignore\n+++ b/.gitignore\n@@ -1,1 +1,5 @@\n dist\n+',
    );
    expect(result.stdout).not.toContain('\\');

    expect(await repo.trackedFiles()).toEqual(trackedBefore);
    expect(await repo.read('.gitignore')).toBe('dist\n');
    expect((await runCli(['check'], repo.root)).code).toBe(EXIT_VIOLATIONS);

    // The dry run and the real fix describe the same changes.
    const applied = await runCli(['fix'], repo.root);
    expect(applied.stdout).toContain('untracked .env');
    expect(applied.stdout).toContain('edited .gitignore');
  });

  it('shows a new file as a diff against /dev/null', async () => {
    const repo = await fixtures.create({ files: { '.env': FAKE_ENV } });
    const result = await runCli(['fix', '--dry-run'], repo.root);
    expect(result.stdout).toContain('would create .gitignore');
    expect(result.stdout).toContain('--- /dev/null\n+++ b/.gitignore\n@@ -0,0 +1,4 @@\n');
    expect(await repo.read('.gitignore')).toBeNull();
  });

  it('fixes rules that are only evaluated, and skips rules that are off', async () => {
    const evaluated = await fixtures.create({
      files: { '.env': FAKE_ENV, 'firescale.config.json': onlyEnvRule('evaluated') },
    });
    expect((await runCli(['fix'], evaluated.root)).stdout).toContain('untracked .env');

    const off = await fixtures.create({
      files: {
        '.env': FAKE_ENV,
        'firescale.config.json': onlyEnvRule('off'),
      },
    });
    expect((await runCli(['fix'], off.root)).stdout).toBe(
      'Nothing to fix: no deterministic fix has anything left to do.\n',
    );
    expect(await off.trackedFiles()).toContain('.env');
  });

  it('honours --rules and rejects extra arguments', async () => {
    const repo = await failingRepo();
    const noMatch = await runCli(['fix', '--rules', 'nope/*'], repo.root);
    expect(noMatch.code).toBe(EXIT_USAGE);
    expect(noMatch.stderr).toContain('matches no rule');
    const extra = await runCli(['fix', 'now'], repo.root);
    expect(extra.code).toBe(EXIT_USAGE);
    expect(extra.stderr).toContain('fix takes no arguments');
    expect(await repo.trackedFiles()).toContain('.env');
  });

  it('a fix invalidates the result cache', async () => {
    const repo = await failingRepo();
    await runCli(['check'], repo.root);
    await runCli(['fix'], repo.root);
    const after = await runCli(['check'], repo.root, { FSCALE_LOG: 'cache' });
    expect(after.stderr).toBe('cache: miss core/no-committed-env\n');
    expect(after.code).toBe(EXIT_OK);
  });
});
