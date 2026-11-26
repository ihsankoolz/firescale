import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { REPO } from './test-helpers.js';

// The production half of the release pipeline (ADR 0092, steps 8 to 10):
// record latest, move it, move it back. Always against the stand-in npm.

const SCRIPTS = path.join(REPO, 'scripts');
const PLATFORMS = [
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-arm64',
  'win32-x64',
];
const PACKAGES = [...PLATFORMS.map((p) => `@firescale/cli-${p}`), '@firescale/sdk', 'firescale'];

interface Result {
  code: number;
  stdout: string;
  stderr: string;
}

function node(script: string, args: string[], env: Record<string, string> = {}): Promise<Result> {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [path.join(SCRIPTS, script), ...args],
      { encoding: 'utf8', env: { ...process.env, ...env } },
      (error, stdout, stderr) => {
        resolve({ code: error && typeof error.code === 'number' ? error.code : 0, stdout, stderr });
      },
    );
  });
}

const temp: string[] = [];
async function scratch(): Promise<string> {
  const dir = await mkdtemp(path.join(tmpdir(), 'firescale-promotion-'));
  temp.push(dir);
  return dir;
}
afterEach(async () => {
  for (const dir of temp.splice(0)) await rm(dir, { recursive: true, force: true, maxRetries: 5 });
});

/**
 * A release in the stand-in registry: every package has `latest` at `old`, and
 * `version` published under next from tarballs in `dir`.
 */
async function staged(version = '1.3.0', old = '1.2.0') {
  const root = await scratch();
  const state = path.join(root, 'registry.json');
  const log = path.join(root, 'calls.log');
  const env: Record<string, string> = {
    FSCALE_NPM_COMMAND: path.join(SCRIPTS, 'fake-npm.mjs'),
    FSCALE_FAKE_NPM_STATE: state,
    FSCALE_FAKE_NPM_LOG: log,
    FSCALE_RELEASE_WAIT_MS: '0',
  };
  for (const name of PACKAGES) await node('fake-npm.mjs', ['seed', name, 'latest', old], env);
  const dir = path.join(root, 'artifacts');
  for (const platform of PLATFORMS) {
    const folder = path.join(dir, `npm-${platform}`);
    await mkdir(folder, { recursive: true });
    for (const name of [
      `firescale-cli-${platform}-${version}.tgz`,
      `firescale-sdk-${version}.tgz`,
      `firescale-${version}.tgz`,
    ]) {
      await writeFile(path.join(folder, name), `${platform}/${name}`);
    }
  }
  const published = await node('publish-npm.mjs', ['--dir', dir, '--version', version], env);
  expect(published.stderr).toBe('');
  await writeFile(log, '');
  const record = path.join(root, 'promotion-record.json');
  const tags = async (name: string) =>
    (
      JSON.parse(await readFile(state, 'utf8')) as {
        packages: Record<string, { tags: Record<string, string> }>;
      }
    ).packages[name]?.tags ?? {};
  const calls = async () => (await readFile(log, 'utf8')).split('\n').filter(Boolean);
  const run = (command: string, args: string[], extra: Record<string, string> = {}) =>
    node('promote-npm.mjs', [command, ...args], { ...env, ...extra });
  return { env, dir, record, tags, calls, run, version, old };
}

describe('scripts/promote-npm.mjs record', () => {
  it('writes the latest of all eight packages, and moves nothing', async () => {
    const r = await staged();
    const result = await r.run('record', ['--version', '1.3.0', '--out', r.record]);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    const record = JSON.parse(await readFile(r.record, 'utf8')) as {
      version: string;
      latest: Record<string, string>;
    };
    expect(record.version).toBe('1.3.0');
    expect(record.latest).toEqual(Object.fromEntries(PACKAGES.map((name) => [name, '1.2.0'])));
    expect(await r.calls()).toEqual([]);
    expect(result.stdout).toContain('recorded latest of 8 packages: firescale is 1.2.0');
  });

  it('refuses a pre-release: it never becomes latest', async () => {
    const r = await staged('1.3.0-rc.1');
    const result = await r.run('record', ['--version', '1.3.0-rc.1', '--out', r.record]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('1.3.0-rc.1 is a pre-release: it stays under next');
  });

  it('refuses when a latest is already the candidate, so a record never holds it', async () => {
    const r = await staged();
    await node(
      'fake-npm.mjs',
      ['dist-tag', 'add', '@firescale/cli-linux-x64@1.3.0', 'latest'],
      r.env,
    );
    const result = await r.run('record', ['--version', '1.3.0', '--out', r.record]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('latest of @firescale/cli-linux-x64 is already 1.3.0');
  });

  it('refuses to move latest down', async () => {
    const r = await staged('1.3.0', '1.10.0');
    const result = await r.run('record', ['--version', '1.3.0', '--out', r.record]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('1.3.0 is lower than the latest of firescale, 1.10.0');
  });
});

describe('scripts/promote-npm.mjs promote', () => {
  async function recorded() {
    const r = await staged();
    expect((await r.run('record', ['--version', '1.3.0', '--out', r.record])).code).toBe(0);
    return r;
  }
  const promote = (r: Awaited<ReturnType<typeof staged>>, extra: Record<string, string> = {}) =>
    r.run('promote', ['--version', '1.3.0', '--record', r.record, '--dir', r.dir], extra);

  it('moves latest on all eight, firescale last, and reads each back', async () => {
    const r = await recorded();
    const result = await promote(r);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    expect(await r.calls()).toEqual(PACKAGES.map((name) => `dist-tag add ${name}@1.3.0 latest`));
    for (const name of PACKAGES) expect((await r.tags(name)).latest).toBe('1.3.0');
    expect(result.stdout).toContain('latest   firescale is 1.3.0 (was 1.2.0)');
    expect(result.stdout).toContain('Promoted: latest is 1.3.0 on all 8 packages.');
  });

  it('a re-run moves only what is not on the candidate yet', async () => {
    const r = await recorded();
    const first = await promote(r, { FSCALE_FAKE_NPM_FAIL: 'dist-tag add @firescale/sdk@' });
    expect(first.code).toBe(1);
    const again = await promote(r);
    expect(again.code).toBe(0);
    expect((await r.calls()).slice(7)).toEqual([
      'dist-tag add @firescale/sdk@1.3.0 latest',
      'dist-tag add firescale@1.3.0 latest',
    ]);
    expect(again.stdout).toContain('already  @firescale/cli-linux-x64 is 1.3.0');
  });

  it('fails loudly when npm refuses a dist-tag, naming the package and the setting', async () => {
    const r = await recorded();
    const result = await promote(r, { FSCALE_FAKE_NPM_FAIL: 'dist-tag add firescale@' });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('npm dist-tag add firescale@1.3.0 latest failed');
    expect(result.stderr).toContain('ENEEDAUTH');
    expect(result.stderr).toContain('Allow npm dist-tag');
    expect((await r.tags('firescale')).latest).toBe('1.2.0');
  });

  it('fails when npm exits 0 but latest did not move', async () => {
    const r = await recorded();
    const result = await promote(r, { FSCALE_FAKE_NPM_SILENT: 'dist-tag add @firescale/sdk@' });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain(
      'npm reported success, but latest of @firescale/sdk is 1.2.0, not 1.3.0',
    );
    // Nothing after it was moved.
    expect((await r.tags('firescale')).latest).toBe('1.2.0');
  });

  it('stops before moving anything when latest moved since the record', async () => {
    const r = await recorded();
    await node('fake-npm.mjs', ['seed', 'firescale', 'latest', '1.2.1'], r.env);
    const result = await promote(r);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('latest of firescale is 1.2.1; the record says 1.2.0');
    expect(await r.calls()).toEqual([]);
  });

  it('stops before moving anything when the registry does not hold the built bytes', async () => {
    const r = await recorded();
    await writeFile(path.join(r.dir, 'npm-linux-x64', 'firescale-1.3.0.tgz'), 'other bytes');
    const result = await promote(r);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain(
      'firescale@1.3.0 on the registry is not the bytes of firescale-1.3.0.tgz',
    );
    expect(await r.calls()).toEqual([]);
  });

  it('refuses a pre-release, a record of another version, and an npm too old for dist-tags', async () => {
    const r = await recorded();
    const pre = await r.run('promote', [
      '--version',
      '1.3.0-rc.1',
      '--record',
      r.record,
      '--dir',
      r.dir,
    ]);
    expect(pre.code).toBe(1);
    expect(pre.stderr).toContain('is a pre-release');
    const other = await r.run('promote', [
      '--version',
      '1.3.1',
      '--record',
      r.record,
      '--dir',
      r.dir,
    ]);
    expect(other.code).toBe(1);
    expect(other.stderr).toContain('the record is for 1.3.0, not 1.3.1');
    const old = await promote(r, { FSCALE_FAKE_NPM_VERSION: '11.20.0' });
    expect(old.code).toBe(1);
    expect(old.stderr).toContain('needs npm 11.21.0 or later, and this is 11.20.0');
    expect(await r.calls()).toEqual([]);
  });

  it('refuses the real npm in a run that is not a tag push', async () => {
    const r = await recorded();
    const result = await r.run(
      'promote',
      ['--version', '1.3.0', '--record', r.record, '--dir', r.dir],
      {
        FSCALE_NPM_COMMAND: '',
        // Reads go to a registry that is not there; npm itself only as a dry run.
        FSCALE_NPM_REGISTRY: 'http://127.0.0.1:9',
        npm_config_registry: 'http://127.0.0.1:9',
        npm_config_dry_run: 'true',
        GITHUB_ACTIONS: 'true',
        GITHUB_EVENT_NAME: 'workflow_dispatch',
        GITHUB_REF_TYPE: 'branch',
      },
    );
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('refusing to run the real npm');
  });
});

describe('scripts/promote-npm.mjs rollback', () => {
  async function promoted() {
    const r = await staged();
    await r.run('record', ['--version', '1.3.0', '--out', r.record]);
    await r.run('promote', ['--version', '1.3.0', '--record', r.record, '--dir', r.dir]);
    await writeFile(path.join(path.dirname(r.record), 'calls.log'), '');
    return r;
  }

  it('moves latest back on all eight, firescale first, and reads each back; never unpublishes', async () => {
    const r = await promoted();
    const result = await r.run('rollback', ['--record', r.record]);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    expect(await r.calls()).toEqual(
      [...PACKAGES].reverse().map((name) => `dist-tag add ${name}@1.2.0 latest`),
    );
    for (const name of PACKAGES) expect((await r.tags(name)).latest).toBe('1.2.0');
    // The candidate stays on the registry, under next and by its number.
    expect((await r.tags('firescale')).next).toBe('1.3.0');
    expect(result.stdout).toContain(
      'Rolled back: latest is the recorded version on all 8 packages (8 moved).',
    );
  });

  it('undoes a promotion that stopped halfway, and a second run has nothing to do', async () => {
    const r = await staged();
    await r.run('record', ['--version', '1.3.0', '--out', r.record]);
    await r.run('promote', ['--version', '1.3.0', '--record', r.record, '--dir', r.dir], {
      FSCALE_FAKE_NPM_FAIL: 'dist-tag add @firescale/sdk@',
    });
    await writeFile(path.join(path.dirname(r.record), 'calls.log'), '');
    const result = await r.run('rollback', ['--record', r.record]);
    expect(result.code).toBe(0);
    expect(await r.calls()).toHaveLength(6);
    expect(result.stdout).toContain('already  firescale is 1.2.0');
    const again = await r.run('rollback', ['--record', r.record]);
    expect(again.code).toBe(0);
    expect(again.stdout).toContain('(0 moved)');
  });

  it('tries every package, then fails naming the ones it could not move back', async () => {
    const r = await promoted();
    const result = await r.run('rollback', ['--record', r.record], {
      FSCALE_FAKE_NPM_FAIL: 'dist-tag add @firescale/sdk@',
    });
    expect(result.code).toBe(1);
    expect(await r.calls()).toHaveLength(8);
    expect(result.stderr).toContain('latest of @firescale/sdk is still 1.3.0');
    expect(result.stderr).toContain('npm dist-tag add @firescale/sdk@1.2.0 latest');
    expect((await r.tags('firescale')).latest).toBe('1.2.0');
  });

  it('fails when npm exits 0 but latest did not move back', async () => {
    const r = await promoted();
    const result = await r.run('rollback', ['--record', r.record], {
      FSCALE_FAKE_NPM_SILENT: 'dist-tag add firescale@',
    });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('latest of firescale is still 1.3.0');
  });

  it('needs a record', async () => {
    const r = await staged();
    const result = await r.run('rollback', [
      '--record',
      path.join(path.dirname(r.record), 'none.json'),
    ]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('no promotion record');
  });
});

describe('scripts/release-issue.mjs', () => {
  it('says what failed, what rollback did, to ship a patch, and suggests npm deprecate', async () => {
    const out = await scratch();
    const result = await node('release-issue.mjs', [
      '--version',
      '1.3.0',
      '--run',
      'https://github.com/o/r/actions/runs/1',
      '--promote',
      'success',
      '--smoke',
      'failure',
      '--rollback',
      'success',
      '--out',
      path.join(out, 'issue.md'),
    ]);
    expect(result.code).toBe(0);
    expect(result.stdout.trim()).toBe('Release 1.3.0 rolled back: the production smoke failed');
    const body = await readFile(path.join(out, 'issue.md'), 'utf8');
    expect(body).toContain('https://github.com/o/r/actions/runs/1');
    expect(body).toContain('`latest` is back on the recorded version on all eight packages');
    expect(body).toContain('Ship a patch');
    expect(body).toContain('npm deprecate firescale@1.3.0');
    expect(body).toContain('Nothing was unpublished');
  });

  it('says loudly when the rollback itself failed, and marks a rehearsal', async () => {
    const out = await scratch();
    const result = await node('release-issue.mjs', [
      '--version',
      '1.3.0',
      '--run',
      'https://github.com/o/r/actions/runs/2',
      '--promote',
      'failure',
      '--smoke',
      'skipped',
      '--rollback',
      'failure',
      '--rehearsal',
      '--out',
      path.join(out, 'issue.md'),
    ]);
    expect(result.code).toBe(0);
    expect(result.stdout.trim()).toBe(
      '[rehearsal] Release 1.3.0 rolled back: the promotion failed',
    );
    const body = await readFile(path.join(out, 'issue.md'), 'utf8');
    expect(body).toContain('**The rollback failed.**');
    expect(body).toContain('node scripts/promote-npm.mjs rollback');
    expect(body).toContain('A rehearsal');
  });
});

describe('scripts/rehearsal-seed.mjs', () => {
  /** A registry that answers the dist-tags of every package with `latest`. */
  async function registry(latest: string): Promise<{ url: string; close: () => Promise<void> }> {
    const { createServer } = await import('node:http');
    const server = createServer((request, response) => {
      response.setHeader('content-type', 'application/json');
      response.end(
        (request.url ?? '').startsWith('/-/package/')
          ? JSON.stringify({ latest, next: latest })
          : '{}',
      );
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    const port = typeof address === 'object' && address ? address.port : 0;
    return {
      url: `http://127.0.0.1:${port}`,
      close: () => new Promise<void>((resolve) => void server.close(() => resolve())),
    };
  }

  it("copies npm's latest of the eight packages into the stand-in, and only latest", async () => {
    const npm = await registry('1.2.0');
    try {
      const state = path.join(await scratch(), 'registry.json');
      const result = await node('rehearsal-seed.mjs', ['--version', '1.3.0'], {
        FSCALE_NPM_REGISTRY: npm.url,
        FSCALE_FAKE_NPM_STATE: state,
      });
      expect(result.stderr).toBe('');
      expect(result.code).toBe(0);
      const seeded = JSON.parse(await readFile(state, 'utf8')) as {
        packages: Record<string, { tags: Record<string, string> }>;
      };
      expect(Object.keys(seeded.packages)).toEqual(PACKAGES);
      for (const name of PACKAGES) expect(seeded.packages[name]?.tags).toEqual({ latest: '1.2.0' });
    } finally {
      await npm.close();
    }
  });

  it('refuses a version that is already latest on npm: rehearse the next one', async () => {
    const npm = await registry('1.3.0');
    try {
      const result = await node('rehearsal-seed.mjs', ['--version', '1.3.0'], {
        FSCALE_NPM_REGISTRY: npm.url,
        FSCALE_FAKE_NPM_STATE: path.join(await scratch(), 'registry.json'),
      });
      expect(result.code).toBe(1);
      expect(result.stderr).toContain('1.3.0 is already latest of');
    } finally {
      await npm.close();
    }
  });
});
