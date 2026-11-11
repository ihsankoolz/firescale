import { existsSync } from 'node:fs';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { ENFORCED_CONFIG, EXIT_VIOLATIONS, FAKE_ENV, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const RULE = 'core/no-committed-env';
const LOG = { FSCALE_LOG: 'cache' };

function repo() {
  return fixtures.create({
    files: {
      '.env': FAKE_ENV,
      'README.md': '# fixture\n',
      'firescale.config.json': ENFORCED_CONFIG,
    },
  });
}

describe('fscale check result cache', () => {
  it('serves a second run with no file changes from the cache', async () => {
    const fixture = await repo();
    const first = await runCli(['check', '--json'], fixture.root, LOG);
    expect(first.code).toBe(EXIT_VIOLATIONS);
    expect(first.stderr).toBe(`cache: miss ${RULE}\n`);

    const second = await runCli(['check', '--json'], fixture.root, LOG);
    expect(second.stderr).toBe(`cache: hit ${RULE}\n`);
    expect(second.code).toBe(EXIT_VIOLATIONS);
    expect(second.stdout).toBe(first.stdout);
  });

  it('runs the rule again when one of its input files changes', async () => {
    const fixture = await repo();
    await runCli(['check'], fixture.root);
    await fixture.write('README.md', '# changed, but not an input\n');
    expect((await runCli(['check'], fixture.root, LOG)).stderr).toBe(`cache: hit ${RULE}\n`);

    await fixture.write('.gitignore', 'dist\n');
    expect((await runCli(['check'], fixture.root, LOG)).stderr).toBe(`cache: miss ${RULE}\n`);
  });

  it('notices a change in the git index even when no file content changed', async () => {
    const fixture = await fixtures.create({
      files: { '.env': FAKE_ENV, 'firescale.config.json': ENFORCED_CONFIG },
      track: ['firescale.config.json'],
    });
    const before = await runCli(['check'], fixture.root, LOG);
    expect(before.stdout).toBe('OK: 1 rule checked, no violations.\n');
    await fixture.track(['.env']);
    const after = await runCli(['check'], fixture.root, LOG);
    expect(after.stderr).toBe(`cache: miss ${RULE}\n`);
    expect(after.code).toBe(EXIT_VIOLATIONS);
  });

  it('notices a newly tracked env file whose name is in upper case', async () => {
    const fixture = await fixtures.create({
      files: { 'sub/.ENV.LOCAL': FAKE_ENV, 'firescale.config.json': ENFORCED_CONFIG },
      track: ['firescale.config.json'],
    });
    expect((await runCli(['check'], fixture.root, LOG)).stdout).toBe(
      'OK: 1 rule checked, no violations.\n',
    );
    await fixture.track(['sub/.ENV.LOCAL']);
    const after = await runCli(['check'], fixture.root, LOG);
    expect(after.stderr).toBe(`cache: miss ${RULE}\n`);
    expect(after.code).toBe(EXIT_VIOLATIONS);
    expect(after.stdout).toContain('sub/.ENV.LOCAL');
  });

  it('keeps the cache in .firescale/cache and out of git', async () => {
    const fixture = await repo();
    await runCli(['check'], fixture.root);
    expect(existsSync(path.join(fixture.root, '.firescale', 'cache'))).toBe(true);
    expect(await fixture.read('.firescale/.gitignore')).toContain('\n*\n');
  });

  it('--no-cache neither reads nor writes the cache', async () => {
    const fixture = await repo();
    const result = await runCli(['check', '--no-cache'], fixture.root, LOG);
    expect(result.stderr).toBe(`cache: skip ${RULE}\n`);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(existsSync(path.join(fixture.root, '.firescale'))).toBe(false);
  });

  it('prints nothing about the cache unless asked', async () => {
    const fixture = await repo();
    expect((await runCli(['check'], fixture.root)).stderr).toBe('');
  });
});
