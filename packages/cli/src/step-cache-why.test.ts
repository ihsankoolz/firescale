// Why a step ran again (ADR 0075): beside each kept pass, a small manifest
// of digests, one per part of the step's key. On a miss, the parts that
// differ from the step's last pass are named under FSCALE_LOG=cache.
import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { EXIT_OK, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

/** A node one-liner that appends `word` to `steps.log`, which git ignores. */
function logs(word: string): string {
  return `node -e "require('fs').appendFileSync('steps.log', '${word}\\n')"`;
}

async function loggingRepo() {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: logs('lint'), test: logs('test') },
      }),
      'src/a.js': 'export const a = 1;\n',
      '.gitignore': 'steps.log\n.env\n.env.local\n.firescale/\n',
    },
  });
  const verify = (env: Record<string, string> = {}, ...flags: string[]) =>
    runCli(['verify', ...flags], repo.root, { FSCALE_LOG: 'cache', TZ: 'UTC', ...env });
  return { repo, verify };
}

/** The lines of the cache log that say why a step ran again. */
function why(stderr: string): string[] {
  return stderr
    .split(/\r?\n/)
    .filter((line) => line.startsWith('cache: why '))
    .map((line) => line.replace(/\(\d{4}-\d\d-\d\d \d\d:\d\d UTC\)/, '(WHEN)'));
}

describe('why a step ran again', () => {
  it('says there is no kept pass on the first run, and nothing on a hit', async () => {
    const { verify } = await loggingRepo();
    const first = await verify();
    expect(first.code).toBe(EXIT_OK);
    expect(why(first.stderr)).toEqual([
      'cache: why lint ran: no pass of it is kept to compare with',
      'cache: why test ran: no pass of it is kept to compare with',
    ]);
    const second = await verify();
    expect(second.stdout.split('\n')[0]).toBe(
      'verify: OK (2 of 3 steps passed from the cache, not run now)',
    );
    expect(why(second.stderr)).toEqual([]);
  });

  it('names a changed file group, a changed env file and a changed variable, each alone', async () => {
    const { repo, verify } = await loggingRepo();
    expect((await verify()).code).toBe(EXIT_OK);

    await repo.write('src/a.js', 'export const a = 2;\n');
    const files = await verify();
    expect(why(files.stderr)).toEqual([
      'cache: why lint ran again: these inputs differ from its last pass (WHEN): files that belong to no project',
      'cache: why test ran again: these inputs differ from its last pass (WHEN): files that belong to no project',
    ]);

    await writeFile(path.join(repo.root, '.env'), 'API_KEY=sk-test-never-shown-4d5e\n');
    const env = await verify();
    expect(why(env.stderr)).toEqual([
      'cache: why lint ran again: these inputs differ from its last pass (WHEN): the env files (.env, .env.local)',
      'cache: why test ran again: these inputs differ from its last pass (WHEN): the env files (.env, .env.local)',
    ]);

    const zone = await verify({ TZ: 'Asia/Singapore' });
    expect(why(zone.stderr)).toEqual([
      'cache: why lint ran again: these inputs differ from its last pass (WHEN): the variable TZ',
      'cache: why test ran again: these inputs differ from its last pass (WHEN): the variable TZ',
    ]);
    // Nothing of this is in the normal output of a quick step.
    for (const result of [files, env, zone]) {
      expect(result.stdout).not.toContain('Ran again');
      expect(result.stdout).not.toContain('differ');
    }
  });

  it('names the command when the command changed, and two parts when two changed', async () => {
    const { repo, verify } = await loggingRepo();
    expect((await verify()).code).toBe(EXIT_OK);
    await repo.write(
      'firescale.config.json',
      JSON.stringify({ packs: ['core'], verify: { lint: logs('lint'), test: logs('test2') } }),
    );
    const result = await verify();
    const lines = why(result.stderr);
    // The config file is a file of the repo root's own steps.
    expect(lines[0]).toBe(
      'cache: why lint ran again: these inputs differ from its last pass (WHEN): files that belong to no project',
    );
    expect(lines[1]).toBe(
      'cache: why test ran again: these inputs differ from its last pass (WHEN): the command; files that belong to no project',
    );
  });

  it('compares with the last pass, also after a run that failed', async () => {
    const { repo, verify } = await loggingRepo();
    expect((await verify()).code).toBe(EXIT_OK);
    await repo.write(
      'firescale.config.json',
      JSON.stringify({
        packs: ['core'],
        verify: { lint: logs('lint'), test: 'node -e "process.exit(1)"' },
      }),
    );
    const failed = await verify();
    expect(failed.code).not.toBe(EXIT_OK);
    const again = await verify();
    expect(why(again.stderr)).toEqual([
      'cache: why test ran again: these inputs differ from its last pass (WHEN): the command; files that belong to no project',
    ]);
  });

  it('names the project folder and the shared folder whose files changed', async () => {
    const repo = await fixtures.create({
      files: { ...courseProjectFiles(), '.gitignore': 'generated/\n.env\n.env.local\n' },
    });
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    const env = {
      ...shimEnv(tools.root, path.join(tools.root, 'calls.log')),
      FSCALE_LOG: 'cache',
    };
    expect((await runCli(['verify'], repo.root, env)).code).toBe(EXIT_OK);

    await repo.write('backend/shared/db.py', 'URL = "changed"\n');
    await repo.write('backend/auth_service/main.py', 'from shared.db import URL\n\napp = 1\n');
    const result = await runCli(['verify'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    const lines = why(result.stderr);
    expect(lines).toContain(
      'cache: why test in backend/auth_service ran again: these inputs differ from its last pass (WHEN): files in backend/auth_service; files in backend/shared',
    );
    expect(lines).toContain(
      'cache: why test in backend/ledger_service ran again: these inputs differ from its last pass (WHEN): files in backend/shared',
    );
    // A service that does not use the shared folder was a hit.
    expect(lines.filter((line) => line.includes('audit_service'))).toEqual([]);
  });

  it('keeps digests only: no path of a file, no value, in the manifest beside the passes', async () => {
    const { repo, verify } = await loggingRepo();
    const secret = 'sk-test-7a8b9c-never-shown';
    await writeFile(path.join(repo.root, '.env'), `API_KEY=${secret}\n`);
    expect((await verify({ NODE_OPTIONS: '--max-old-space-size=4096' })).code).toBe(EXIT_OK);
    const dir = path.join(repo.root, '.firescale', 'steps');
    const names = (await readdir(dir)).filter((name) => name.startsWith('last-'));
    expect(names).toHaveLength(2);
    for (const name of names) {
      expect(name).toMatch(/^last-[0-9a-f]{16}\.json$/);
      const text = await readFile(path.join(dir, name), 'utf8');
      expect(text).not.toContain(secret);
      expect(text).not.toContain('max-old-space-size');
      expect(text).not.toContain('a.js');
      const manifest = JSON.parse(text) as { parts: Record<string, string>; ranAt: number };
      expect(Object.keys(manifest.parts)).toContain('files that belong to no project');
      expect(Object.keys(manifest.parts)).toContain('the env files (.env, .env.local)');
      expect(Object.keys(manifest.parts)).toContain('the variable NODE_OPTIONS');
      for (const digest of Object.values(manifest.parts)) {
        expect(digest).toMatch(/^[0-9a-f]{16}$/);
      }
      expect(text.length).toBeLessThan(4000);
    }
  });

  it('says nothing with --no-cache, which never looks a pass up', async () => {
    const { verify } = await loggingRepo();
    expect((await verify()).code).toBe(EXIT_OK);
    const result = await verify({}, '--no-cache');
    expect(why(result.stderr)).toEqual([]);
  });
});
