import { execFile } from 'node:child_process';
import { cp, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import { REPO } from './test-helpers.js';

const SCRIPTS = path.join(REPO, 'scripts');
const PLATFORMS = [
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-arm64',
  'win32-x64',
];

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
  const dir = await mkdtemp(path.join(tmpdir(), 'firescale-release-'));
  temp.push(dir);
  return dir;
}
afterEach(async () => {
  for (const dir of temp.splice(0)) await rm(dir, { recursive: true, force: true, maxRetries: 5 });
});

/** A copy of the files of this repo that record the version. */
async function versionFiles(): Promise<string> {
  const root = await scratch();
  for (const file of ['Cargo.toml', 'Cargo.lock', 'package.json']) {
    await cp(path.join(REPO, file), path.join(root, file));
  }
  for (const [folder, manifest] of [
    ['crates', 'Cargo.toml'],
    ['packages', 'package.json'],
  ] as const) {
    for (const entry of await readdir(path.join(REPO, folder), { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const from = path.join(REPO, folder, entry.name, manifest);
      const text = await readFile(from, 'utf8').catch(() => undefined);
      if (text === undefined) continue;
      await mkdir(path.join(root, folder, entry.name), { recursive: true });
      await writeFile(path.join(root, folder, entry.name, manifest), text);
    }
  }
  return root;
}

describe('scripts/bump-version.mjs', () => {
  it('finds the same version everywhere in this repo', async () => {
    const result = await node('bump-version.mjs', ['--check']);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    const manifest = JSON.parse(await readFile(path.join(REPO, 'package.json'), 'utf8')) as {
      version: string;
    };
    expect(result.stdout).toBe(`${manifest.version}\n`);
  });

  it('sets the version everywhere it is recorded, and nothing else', async () => {
    const root = await versionFiles();
    const before = await readFile(path.join(root, 'Cargo.lock'), 'utf8');
    const result = await node('bump-version.mjs', ['9.8.7-rc.1', '--root', root]);
    expect(result.code).toBe(0);
    expect(result.stdout.split('\n').slice(1, -1)).toEqual([
      '  Cargo.toml',
      '  Cargo.lock',
      '  package.json',
      '  packages/agents/package.json',
      '  packages/cli/package.json',
      '  packages/generators/package.json',
      '  packages/sdk/package.json',
    ]);
    const check = await node('bump-version.mjs', [
      '--check',
      '--tag',
      'v9.8.7-rc.1',
      '--root',
      root,
    ]);
    expect(check).toEqual({ code: 0, stdout: '9.8.7-rc.1\n', stderr: '' });

    // Only the workspace's own crates change in the lockfile.
    const after = await readFile(path.join(root, 'Cargo.lock'), 'utf8');
    const changed = after.split('\n').filter((line, index) => line !== before.split('\n')[index]);
    expect(changed).toEqual(Array.from({ length: 4 }, () => 'version = "9.8.7-rc.1"'));
    expect(await readFile(path.join(root, 'Cargo.toml'), 'utf8')).toContain(
      '[workspace.package]\nversion = "9.8.7-rc.1"\n',
    );

    const again = await node('bump-version.mjs', ['9.8.7-rc.1', '--root', root]);
    expect(again.stdout).toBe('Already at 9.8.7-rc.1. Nothing changed.\n');
  });

  it('keeps CRLF line endings', async () => {
    const root = await versionFiles();
    const file = path.join(root, 'packages', 'sdk', 'package.json');
    await writeFile(file, (await readFile(file, 'utf8')).replace(/\r?\n/g, '\r\n'));
    expect((await node('bump-version.mjs', ['2.0.0', '--root', root])).code).toBe(0);
    const text = await readFile(file, 'utf8');
    expect(text).toContain('"version": "2.0.0",\r\n');
    expect(text.replace(/\r\n/g, '')).not.toContain('\n');
  });

  it('fails when one place disagrees, and when the tag does not match', async () => {
    const root = await versionFiles();
    await node('bump-version.mjs', ['1.2.3', '--root', root]);
    const wrongTag = await node('bump-version.mjs', ['--check', '--tag', 'v1.2.4', '--root', root]);
    expect(wrongTag.code).toBe(1);
    expect(wrongTag.stderr).toContain('the tag "v1.2.4" does not match the version 1.2.3');
    expect(wrongTag.stderr).toContain('Tag the commit v1.2.3');
    // A tag without the "v" is not a release tag.
    expect(
      (await node('bump-version.mjs', ['--check', '--tag', '1.2.3', '--root', root])).code,
    ).toBe(1);

    const file = path.join(root, 'packages', 'cli', 'package.json');
    await writeFile(file, (await readFile(file, 'utf8')).replace('"1.2.3"', '"1.2.2"'));
    const drift = await node('bump-version.mjs', ['--check', '--root', root]);
    expect(drift.code).toBe(1);
    expect(drift.stderr).toContain('the version is not the same everywhere');
    expect(drift.stderr).toContain('1.2.2  packages/cli/package.json');
  });

  it('rejects something that is not a version', async () => {
    const root = await versionFiles();
    for (const bad of ['v1.2.3', '1.2', 'latest', '1.2.3+build']) {
      const result = await node('bump-version.mjs', [bad, '--root', root]);
      expect(result.code).toBe(1);
      expect(result.stderr).toContain('is not a version');
    }
    expect((await node('bump-version.mjs', ['--root', root])).stderr).toContain('no version given');
    expect((await node('bump-version.mjs', ['--check', '--root', root])).code).toBe(0);
  });
});

describe('scripts/publish-npm.mjs', () => {
  let server: Server;
  let registry = '';
  /** Paths the fake registry answers with 200. Everything else is 404. */
  let published = new Set<string>();
  let broken = false;
  const requests: string[] = [];

  beforeAll(async () => {
    server = createServer((request, response) => {
      requests.push(request.url ?? '');
      response.statusCode = broken ? 503 : published.has(request.url ?? '') ? 200 : 404;
      response.end('{}');
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    registry = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => new Promise<void>((resolve) => void server.close(() => resolve())));
  afterEach(() => {
    published = new Set();
    broken = false;
    requests.length = 0;
  });

  /** The downloaded artifacts of a release: one folder per platform, three tarballs each. */
  async function artifacts(version: string, skip: string[] = []): Promise<string> {
    const dir = await scratch();
    for (const platform of PLATFORMS) {
      const folder = path.join(dir, `npm-${platform}`);
      await mkdir(folder, { recursive: true });
      for (const name of [
        `firescale-cli-${platform}-${version}.tgz`,
        `firescale-sdk-${version}.tgz`,
        `firescale-${version}.tgz`,
      ]) {
        // Each tarball has bytes of its own, so a mix-up shows in its integrity.
        if (!skip.includes(`${platform}/${name}`)) {
          await writeFile(path.join(folder, name), `${platform}/${name}`);
        }
      }
    }
    return dir;
  }

  const plan = (dir: string, version: string) =>
    node('publish-npm.mjs', ['--dir', dir, '--version', version, '--dry-run'], {
      FSCALE_NPM_REGISTRY: registry,
    });

  it('plans the six platform packages, then the sdk, then firescale', async () => {
    const result = await plan(await artifacts('1.2.3'), '1.2.3');
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    expect(result.stdout.split('\n').slice(0, -1)).toEqual([
      ...PLATFORMS.map(
        (platform) =>
          `publish  @firescale/cli-${platform}@1.2.3 from firescale-cli-${platform}-1.2.3.tgz (dry run)`,
      ),
      'publish  @firescale/sdk@1.2.3 from firescale-sdk-1.2.3.tgz (dry run)',
      'publish  firescale@1.2.3 from firescale-1.2.3.tgz (dry run)',
      'Dry run: 8 to publish, 0 already on the registry.',
    ]);
    // One lookup per package, for exactly that version.
    expect(requests).toEqual([
      ...PLATFORMS.map((platform) => `/@firescale%2fcli-${platform}/1.2.3`),
      '/@firescale%2fsdk/1.2.3',
      '/firescale/1.2.3',
    ]);
  });

  it('skips what is already on the registry, so a half-finished release can be completed', async () => {
    published = new Set([
      '/@firescale%2fcli-darwin-arm64/1.2.3',
      '/@firescale%2fcli-darwin-x64/1.2.3',
      '/@firescale%2fsdk/1.2.3',
    ]);
    const result = await plan(await artifacts('1.2.3'), '1.2.3');
    expect(result.code).toBe(0);
    const lines = result.stdout.split('\n');
    expect(lines.filter((line) => line.startsWith('skip'))).toEqual([
      'skip     @firescale/cli-darwin-arm64@1.2.3 (already on the registry)',
      'skip     @firescale/cli-darwin-x64@1.2.3 (already on the registry)',
      'skip     @firescale/sdk@1.2.3 (already on the registry)',
    ]);
    expect(lines.filter((line) => line.startsWith('publish'))).toHaveLength(5);
    expect(lines.at(-2)).toBe('Dry run: 5 to publish, 3 already on the registry.');

    // Everything published: nothing left to do, and still exit 0.
    published = new Set(requests);
    const done = await plan(await artifacts('1.2.3'), '1.2.3');
    expect(done.code).toBe(0);
    expect(done.stdout.split('\n').at(-2)).toBe(
      'Dry run: 0 to publish, 8 already on the registry.',
    );
  });

  it('stops before anything is published when a tarball is missing', async () => {
    const dir = await artifacts('1.2.3', ['win32-arm64/firescale-cli-win32-arm64-1.2.3.tgz']);
    const result = await plan(dir, '1.2.3');
    expect(result.code).toBe(1);
    expect(result.stdout).toBe('');
    expect(result.stderr).toContain('no tarball firescale-cli-win32-arm64-1.2.3.tgz');
    expect(requests).toEqual([]);

    // Tarballs of another version do not count.
    const other = await plan(await artifacts('1.2.2'), '1.2.3');
    expect(other.code).toBe(1);
    expect(other.stderr).toContain('no tarball');
  });

  it('stops when the registry cannot say whether a version exists', async () => {
    broken = true;
    const result = await plan(await artifacts('1.2.3'), '1.2.3');
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('could not find out whether @firescale/cli-darwin-arm64@1.2.3');
    expect(result.stdout).not.toContain('publish');
  }, 30_000);

  /** The stand-in npm (scripts/fake-npm.mjs), with a registry of its own in a JSON file. */
  async function fakeNpm(version = '11.21.3'): Promise<{
    env: Record<string, string>;
    calls: () => Promise<string[]>;
    tags: (name: string) => Promise<Record<string, string>>;
  }> {
    const dir = await scratch();
    const state = path.join(dir, 'registry.json');
    const log = path.join(dir, 'calls.log');
    const env = {
      FSCALE_NPM_COMMAND: path.join(SCRIPTS, 'fake-npm.mjs'),
      FSCALE_FAKE_NPM_STATE: state,
      FSCALE_FAKE_NPM_LOG: log,
      FSCALE_FAKE_NPM_VERSION: version,
      FSCALE_RELEASE_WAIT_MS: '0',
    };
    const calls = async () =>
      (await readFile(log, 'utf8').catch(() => '')).split('\n').filter(Boolean);
    const tags = async (name: string) => {
      const registry = JSON.parse(await readFile(state, 'utf8')) as {
        packages: Record<string, { tags: Record<string, string> }>;
      };
      return registry.packages[name]?.tags ?? {};
    };
    return { env, calls, tags };
  }

  const publish = (
    dir: string,
    version: string,
    env: Record<string, string>,
    extra: Record<string, string> = {},
  ) => node('publish-npm.mjs', ['--dir', dir, '--version', version], { ...env, ...extra });

  it('publishes the eight tarballs in order under next, never latest, the shared two from the linux-x64 build', async () => {
    const npm = await fakeNpm();
    const dir = await artifacts('1.2.3');
    const result = await publish(dir, '1.2.3', npm.env);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    expect(await npm.calls()).toEqual([
      ...PLATFORMS.map((p) => `publish firescale-cli-${p}-1.2.3.tgz --access public --tag next`),
      'publish firescale-sdk-1.2.3.tgz --access public --tag next',
      'publish firescale-1.2.3.tgz --access public --tag next',
    ]);
    expect(result.stdout).toContain('Done: 8 published, 0 already on the registry.');
    // latest is never touched by a publish: only promotion moves it (ADR 0092).
    for (const name of ['@firescale/cli-linux-x64', '@firescale/sdk', 'firescale']) {
      expect(await npm.tags(name)).toEqual({ next: '1.2.3' });
    }
    // The registry holds the bytes of the linux-x64 build's copy of the shared two.
    expect(result.stdout).toContain(
      'checked  firescale@1.2.3: the registry has the bytes of firescale-1.2.3.tgz, next is 1.2.3',
    );
    expect(result.stdout).toContain(
      'Checked: all 8 on the registry with the built bytes, under next.',
    );
  });

  it('stops at the first publish that fails, and a second run finishes the release', async () => {
    const npm = await fakeNpm();
    const dir = await artifacts('1.2.3');
    const first = await publish(dir, '1.2.3', npm.env, { FSCALE_FAKE_NPM_FAIL: 'firescale-sdk-' });
    expect(first.code).toBe(1);
    expect(first.stderr).toContain('npm publish failed for @firescale/sdk@1.2.3');
    expect(first.stderr).toContain('run the job again');
    // firescale itself was never attempted.
    expect(await npm.calls()).toHaveLength(7);

    // The registry now has the six that went through.
    const second = await publish(dir, '1.2.3', npm.env);
    expect(second.code).toBe(0);
    expect(second.stdout).toContain('Done: 2 published, 6 already on the registry.');
    expect((await npm.calls()).slice(7)).toEqual([
      'publish firescale-sdk-1.2.3.tgz --access public --tag next',
      'publish firescale-1.2.3.tgz --access public --tag next',
    ]);
  });

  it('publishes a pre-release under the dist-tag next', async () => {
    const npm = await fakeNpm();
    const result = await publish(await artifacts('2.0.0-rc.1'), '2.0.0-rc.1', npm.env);
    expect(result.code).toBe(0);
    const calls = await npm.calls();
    expect(calls).toHaveLength(8);
    expect(calls.every((call) => call.endsWith('--access public --tag next'))).toBe(true);
    expect(await npm.tags('firescale')).toEqual({ next: '2.0.0-rc.1' });
  });

  it('refuses an npm too old for trusted publishing, before anything is published', async () => {
    const npm = await fakeNpm('10.9.9');
    const result = await publish(await artifacts('1.2.3'), '1.2.3', npm.env);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('needs npm 11.5.1 or later, and this is 10.9.9');
    expect(await npm.calls()).toEqual([]);
  });

  it('fails when the registry does not hold the bytes of the tarball', async () => {
    const npm = await fakeNpm();
    const result = await publish(await artifacts('1.2.3'), '1.2.3', npm.env, {
      FSCALE_FAKE_NPM_CORRUPT: 'firescale-sdk-',
    });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain(
      '@firescale/sdk@1.2.3 on the registry is not the bytes of firescale-sdk-1.2.3.tgz',
    );
    expect(result.stderr).toContain('Do not promote 1.2.3');
  });

  it('fails when a publish exits 0 but the version never reaches the registry', async () => {
    const npm = await fakeNpm();
    const result = await publish(await artifacts('1.2.3'), '1.2.3', npm.env, {
      FSCALE_FAKE_NPM_SILENT: 'firescale-1.2.3.tgz',
    });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('firescale@1.2.3 is not on the registry');
  });

  it('fails when next is not the version after publishing', async () => {
    const npm = await fakeNpm();
    const dir = await artifacts('1.2.3');
    expect((await publish(dir, '1.2.3', npm.env)).code).toBe(0);
    // Someone pointed next elsewhere; a second run skips every publish and still checks.
    await node('fake-npm.mjs', ['seed', 'firescale', 'next', '1.2.4'], npm.env);
    const again = await publish(dir, '1.2.3', npm.env);
    expect(again.code).toBe(1);
    expect(again.stderr).toContain('next of firescale is 1.2.4, not 1.2.3');
  });

  it('refuses the real npm in a run that is not a tag push', async () => {
    for (const [event, refType] of [
      ['workflow_dispatch', 'branch'],
      ['push', 'branch'],
      ['schedule', 'branch'],
    ]) {
      const result = await node(
        'publish-npm.mjs',
        ['--dir', await artifacts('1.2.3'), '--version', '1.2.3'],
        {
          FSCALE_NPM_REGISTRY: registry,
          FSCALE_NPM_COMMAND: '',
          // Should the guard ever be missing, npm still reaches only the
          // stand-in registry, and only as a dry run: this test never publishes.
          npm_config_registry: registry,
          npm_config_dry_run: 'true',
          GITHUB_ACTIONS: 'true',
          GITHUB_EVENT_NAME: event ?? '',
          GITHUB_REF_TYPE: refType ?? '',
        },
      );
      expect(result.code, event).toBe(1);
      expect(result.stderr).toContain('refusing to run the real npm');
      expect(result.stdout).not.toContain('publish ');
    }
  });

  it('needs --dir and --version', async () => {
    const result = await node('publish-npm.mjs', ['--dry-run']);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('--dir and --version are both needed');
  });
});

describe('scripts/verify-published.mjs', () => {
  it('passes when the command prints the version, with or without a leading v', async () => {
    for (const version of ['1.2.3', 'v1.2.3']) {
      const result = await node('verify-published.mjs', [version, '--attempts', '1'], {
        FSCALE_NPX_COMMAND: 'node -e "console.log(\'1.2.3\')"',
      });
      expect(result.stderr).toBe('');
      expect(result.code).toBe(0);
      expect(result.stdout).toContain('firescale@1.2.3 is installable');
    }
  });

  it('tries again, then fails with the next step', async () => {
    const wrong = await node('verify-published.mjs', ['1.2.3', '--attempts', '2', '--wait', '0'], {
      FSCALE_NPX_COMMAND: 'node -e "console.log(\'1.2.2\')"',
    });
    expect(wrong.code).toBe(1);
    expect(wrong.stdout).toContain('attempt 1 of 2: not yet (it printed "1.2.2")');
    expect(wrong.stdout).toContain('attempt 2 of 2');
    expect(wrong.stderr).toContain('did not print 1.2.3 after 2 attempts');

    const failing = await node(
      'verify-published.mjs',
      ['1.2.3', '--attempts', '1', '--wait', '0'],
      {
        FSCALE_NPX_COMMAND: 'node -e "process.exit(7)"',
      },
    );
    expect(failing.code).toBe(1);
    expect(failing.stdout).toContain('not yet (exit 7');
  });

  it('with --tag, checks that the tag gives the version', async () => {
    const ok = await node('verify-published.mjs', ['1.2.3', '--tag', 'next', '--attempts', '1'], {
      FSCALE_NPX_COMMAND: 'node -e "console.log(\'1.2.3\')"',
    });
    expect(ok.stderr).toBe('');
    expect(ok.code).toBe(0);
    expect(ok.stdout).toContain('firescale@next is 1.2.3');

    const behind = await node(
      'verify-published.mjs',
      ['1.2.3', '--tag', 'latest', '--attempts', '1', '--wait', '0'],
      { FSCALE_NPX_COMMAND: 'node -e "console.log(\'1.2.2\')"' },
    );
    expect(behind.code).toBe(1);
    expect(behind.stderr).toContain('firescale@latest did not give 1.2.3');

    for (const bad of ['next; echo hi', 'NEXT', '']) {
      const result = await node('verify-published.mjs', ['1.2.3', '--tag', bad, '--attempts', '1']);
      expect(result.code, bad).toBe(1);
      expect(result.stderr).toContain('is not a dist-tag');
    }
  });

  it('rejects something that is not a version', async () => {
    const result = await node('verify-published.mjs', ['1.2.3 && echo hi', '--attempts', '1']);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('is not a version');
  });
});

describe('scripts/release-sums.mjs', () => {
  /** One artifact folder as a build job leaves it: three tarballs. */
  async function folder(root: string, name: string): Promise<string> {
    const dir = path.join(root, name);
    await mkdir(dir, { recursive: true });
    for (const file of [
      'firescale-1.2.3.tgz',
      'firescale-sdk-1.2.3.tgz',
      'firescale-cli-x-1.2.3.tgz',
    ]) {
      await writeFile(path.join(dir, file), `bytes of ${name}/${file}`);
    }
    return dir;
  }

  it('writes SHA256SUMS for every tarball, and a check of the same bytes passes', async () => {
    const root = await scratch();
    const dir = await folder(root, 'npm-linux-x64');
    const written = await node('release-sums.mjs', ['write', dir]);
    expect(written.stderr).toBe('');
    expect(written.code).toBe(0);
    const sums = await readFile(path.join(dir, 'SHA256SUMS'), 'utf8');
    const lines = sums.split('\n').filter(Boolean);
    expect(lines).toHaveLength(3);
    for (const line of lines) expect(line).toMatch(/^[0-9a-f]{64} {2}firescale[\w.-]*\.tgz$/);
    // The format `sha256sum -c` reads, sorted by file name.
    expect(lines.map((line) => line.slice(66))).toEqual([
      'firescale-1.2.3.tgz',
      'firescale-cli-x-1.2.3.tgz',
      'firescale-sdk-1.2.3.tgz',
    ]);
    const check = await node('release-sums.mjs', ['check', dir]);
    expect(check.stderr).toBe('');
    expect(check.code).toBe(0);
    expect(check.stdout).toContain('3 tarballs match SHA256SUMS');
  });

  it('checks every artifact folder below the given one', async () => {
    const root = await scratch();
    for (const name of ['npm-linux-x64', 'npm-win32-x64']) {
      await node('release-sums.mjs', ['write', await folder(root, name)]);
    }
    const check = await node('release-sums.mjs', ['check', root]);
    expect(check.code).toBe(0);
    expect(check.stdout).toContain('6 tarballs match SHA256SUMS');
  });

  it('fails on a changed byte, naming the file', async () => {
    const root = await scratch();
    const dir = await folder(root, 'npm-linux-x64');
    await node('release-sums.mjs', ['write', dir]);
    await writeFile(path.join(dir, 'firescale-sdk-1.2.3.tgz'), 'other bytes');
    const check = await node('release-sums.mjs', ['check', root]);
    expect(check.code).toBe(1);
    expect(check.stderr).toContain(
      'npm-linux-x64/firescale-sdk-1.2.3.tgz does not match SHA256SUMS',
    );
  });

  it('fails on a tarball that is not listed, a listed one that is gone, and a folder with no sums', async () => {
    const root = await scratch();
    const dir = await folder(root, 'npm-linux-x64');
    await node('release-sums.mjs', ['write', dir]);
    await writeFile(path.join(dir, 'extra-1.2.3.tgz'), 'x');
    const extra = await node('release-sums.mjs', ['check', root]);
    expect(extra.code).toBe(1);
    expect(extra.stderr).toContain('npm-linux-x64/extra-1.2.3.tgz is not in SHA256SUMS');

    await rm(path.join(dir, 'extra-1.2.3.tgz'));
    await rm(path.join(dir, 'firescale-1.2.3.tgz'));
    const gone = await node('release-sums.mjs', ['check', root]);
    expect(gone.code).toBe(1);
    expect(gone.stderr).toContain('npm-linux-x64/firescale-1.2.3.tgz is in SHA256SUMS but missing');

    const bare = await folder(root, 'npm-darwin-arm64');
    expect(bare).toBeTruthy();
    const none = await node('release-sums.mjs', ['check', path.join(root, 'npm-darwin-arm64')]);
    expect(none.code).toBe(1);
    expect(none.stderr).toContain('no SHA256SUMS');
  });

  it('fails on a name listed twice, and on an empty SHA256SUMS', async () => {
    const root = await scratch();
    const dir = await folder(root, 'npm-linux-x64');
    await node('release-sums.mjs', ['write', dir]);
    const sums = path.join(dir, 'SHA256SUMS');
    const first = (await readFile(sums, 'utf8')).split('\n')[0] ?? '';
    await writeFile(sums, `${await readFile(sums, 'utf8')}${'0'.repeat(64)}  ${first.slice(66)}\n`);
    const twice = await node('release-sums.mjs', ['check', root]);
    expect(twice.code).toBe(1);
    expect(twice.stderr).toContain(
      `npm-linux-x64/${first.slice(66)} is listed twice in SHA256SUMS`,
    );

    const empty = await scratch();
    await writeFile(path.join(empty, 'SHA256SUMS'), '');
    const none = await node('release-sums.mjs', ['check', empty]);
    expect(none.code).toBe(1);
    expect(none.stderr).toContain('SHA256SUMS lists no tarball');
  });

  it('refuses a folder with no tarballs, and a wrong command', async () => {
    const empty = await scratch();
    expect((await node('release-sums.mjs', ['write', empty])).stderr).toContain('no .tgz');
    expect((await node('release-sums.mjs', ['frob', empty])).code).toBe(1);
  });
});

describe('.github/workflows/release.yml', () => {
  const read = () => readFile(path.join(REPO, '.github', 'workflows', 'release.yml'), 'utf8');
  /** The text of one job: from its key to the next key at the same depth. */
  const job = (text: string, name: string): string => {
    const start = text.indexOf(`\n  ${name}:\n`);
    expect(start).toBeGreaterThan(-1);
    const rest = text.slice(start + 1);
    const next = /\n {2}[a-z][a-z-]*:\n/.exec(rest.slice(1));
    return next ? rest.slice(0, next.index + 1) : rest;
  };

  it('uses no token and no secret', async () => {
    const text = (await read()).replace(/^\s*#.*$/gm, '');
    expect(text).not.toMatch(/secrets\./);
    expect(text).not.toMatch(/NODE_AUTH_TOKEN|NPM_TOKEN|_authToken/);
  });

  it('gives only the staging publish job the OIDC permission, in its environment', async () => {
    const text = await read();
    expect(text.match(/id-token: write/g)).toHaveLength(1);
    expect(job(text, 'staging')).toContain('id-token: write');
    expect(job(text, 'staging')).toContain('environment: staging');
    // The default for every other job.
    expect(text).toMatch(/\npermissions:\n {2}contents: read\n/);
  });

  it('publishes only for a pushed tag, after the version check and the build', async () => {
    const text = await read();
    const publish = job(text, 'staging');
    expect(publish).toContain("if: github.event_name == 'push' && github.ref_type == 'tag'");
    expect(publish).toContain('runs-on: ubuntu-latest');
    expect(publish).toContain('node scripts/publish-npm.mjs --dir artifacts');
    expect(job(text, 'build')).toContain('needs: [version, audit]');
    expect(job(text, 'build')).not.toContain('npm publish');
    expect(job(text, 'version')).toContain('bump-version.mjs --check --tag "$TAG"');
    // A tag on a commit that is not on main is refused before any build.
    expect(job(text, 'version')).toContain(
      'git merge-base --is-ancestor "$GITHUB_SHA" origin/main',
    );
    for (const name of ['version', 'staging', 'staging-test']) {
      expect(job(text, name)).toMatch(/timeout-minutes: \d+/);
    }
    // A manual run has no job that publishes: the staging tests need staging, which is skipped.
    const verify = job(text, 'staging-test');
    expect(verify).toContain('needs: [version, staging]');
    expect(verify).toContain('os: [ubuntu-latest, macos-latest, windows-latest]');
    expect(text).toMatch(/\non:\n {2}workflow_dispatch:\n {2}push:\n {4}tags: \['v\*'\]\n/);
  });

  it('audits the dependencies before any build: pnpm audit and a pinned cargo-deny', async () => {
    const text = await read();
    const audit = job(text, 'audit');
    expect(audit).toContain('needs: version');
    expect(audit).toContain('run: pnpm audit --audit-level high');
    // cargo-deny is a downloaded binary: one exact version, its SHA-256 checked before it runs.
    expect(audit).toMatch(/cargo-deny-0\.22\.1-x86_64-unknown-linux-musl/);
    expect(audit).toContain('13f7e825e9cb811c2f8de63f29b0f45851dfb53004029f0f2501af524728364d');
    expect(audit).toMatch(/sha256sum (-c|--check)/);
    expect(audit).toContain('cargo-deny check');
    expect(audit).toMatch(/timeout-minutes: \d+/);
    // Nothing is built until the audit passes.
    expect(job(text, 'build')).toContain('needs: [version, audit]');
  });

  it('writes SHA256SUMS in each build, and checks them wherever a tarball is read', async () => {
    const text = await read();
    const build = job(text, 'build');
    const write = build.indexOf('node scripts/release-sums.mjs write dist-npm');
    expect(write).toBeGreaterThan(build.indexOf('scripts/pack-npm.mjs'));
    expect(write).toBeLessThan(build.indexOf('actions/upload-artifact'));
    expect(build).toContain('dist-npm/SHA256SUMS');
    // The check runs before the tarballs are used.
    const smoke = job(text, 'smoke');
    expect(smoke.indexOf('release-sums.mjs check')).toBeGreaterThan(-1);
    expect(smoke.indexOf('release-sums.mjs check')).toBeLessThan(smoke.indexOf('smoke-npm.mjs'));
    const publish = job(text, 'staging');
    expect(publish.indexOf('release-sums.mjs check artifacts')).toBeGreaterThan(-1);
    expect(publish.indexOf('release-sums.mjs check artifacts')).toBeLessThan(
      publish.indexOf('publish-npm.mjs'),
    );
  });

  it('smokes the tarballs on three OSes with npm, pnpm and yarn before anything is published', async () => {
    const text = await read();
    const smoke = job(text, 'smoke');
    expect(smoke).toContain('needs: [version, build]');
    for (const [os, key] of [
      ['ubuntu-latest', 'linux-x64'],
      ['macos-latest', 'darwin-arm64'],
      ['windows-latest', 'win32-x64'],
    ]) {
      expect(smoke).toMatch(new RegExp(`- \\{ os: ${os}, key: ${key} \\}`));
    }
    expect(smoke).toContain('name: npm-${{ matrix.key }}');
    for (const pm of ['npm', 'pnpm', 'yarn']) {
      expect(smoke).toContain(`node scripts/smoke-npm.mjs --pm ${pm} --tarballs dist-npm`);
    }
    expect(smoke).toMatch(/timeout-minutes: \d+/);
    expect(smoke).not.toContain('id-token');
    expect(job(text, 'staging')).toContain('needs: [version, build, smoke]');
  });

  it('tests what npm serves under next on three OSes, and runs the real repos against it', async () => {
    const text = await read();
    const tests = job(text, 'staging-test');
    // The version first (retried while the registry catches up), then the commands.
    const wait = tests.indexOf('node scripts/verify-published.mjs "$VERSION" --tag next');
    const run = tests.indexOf('node scripts/try-published.mjs next --expect "$VERSION"');
    expect(wait).toBeGreaterThan(-1);
    expect(run).toBeGreaterThan(wait);
    expect(tests).not.toContain('id-token');

    const real = job(text, 'real-repos');
    // After the staging tests: the many repo jobs start only when next installs.
    expect(real).toContain('needs: [version, staging, staging-test]');
    expect(real).toContain('uses: ./.github/workflows/real-repos.yml');
    expect(real).toContain('fscale: next');
    expect(real).toContain('version: ${{ needs.version.outputs.version }}');
    expect(real).toMatch(/permissions:\n {6}contents: read\n/);
  });

  it('pins every action to a commit', async () => {
    const uses = (await read()).match(/uses: .+/g) ?? [];
    expect(uses.length).toBeGreaterThan(8);
    for (const line of uses) {
      // A workflow of this repository is read from the same commit: nothing to pin.
      if (line.startsWith('uses: ./')) {
        expect(line).toMatch(/^uses: \.\/\.github\/workflows\/[\w-]+\.yml$/);
        continue;
      }
      expect(line).toMatch(/^uses: [\w./-]+@[0-9a-f]{40} # v\d+\.\d+\.\d+$/);
    }
    // Every action is still pinned: only the one local workflow is exempt.
    expect(uses.filter((line) => line.startsWith('uses: ./'))).toEqual([
      'uses: ./.github/workflows/real-repos.yml',
    ]);
  });

  it('restores no cache in any job', async () => {
    // What a release publishes is built from the tagged commit and the
    // lockfiles only. A cache is files from an earlier run, which a run on
    // another ref may have written. So no job may hold a line that names a
    // cache: an action (actions/cache, actions/cache/restore, rust-cache,
    // sccache), a `cache:` input of a setup action, a `cache-from:`, an env
    // variable. The one thing allowed is a key that turns a cache off.
    const text = await read();
    const jobs = text.slice(text.indexOf('\njobs:\n'));
    const names = [...jobs.matchAll(/^ {2}([a-z][\w-]*):\r?$/gm)].map((match) => match[1] ?? '');
    expect(names).toEqual([
      'version',
      'audit',
      'build',
      'smoke',
      'staging',
      'staging-test',
      'real-repos',
    ]);
    const turnedOff = /^\s*(- )?[\w-]*cache[\w-]*: (false|none)$/i;
    const found = names.flatMap((name) =>
      job(text, name)
        .split(/\r?\n/)
        // Comments may say "cache"; a comment restores nothing.
        .filter((line) => !/^\s*#/.test(line))
        .filter((line) => /cache/i.test(line) && !turnedOff.test(line))
        .map((line) => `${name}: ${line.trim()}`),
    );
    expect(found).toEqual([]);
  });

  it("turns off setup-node's own cache in every job", async () => {
    // setup-node can cache a package manager's store without a `cache:`
    // input (ADR 0041), and it saves in a post step. A release turns it off.
    const text = await read();
    for (const name of ['version', 'audit', 'build', 'smoke', 'staging', 'staging-test']) {
      const steps = job(text, name).split(/\r?\n(?= {6}- )/);
      const setups = steps.filter((step) => step.includes('uses: actions/setup-node@'));
      expect(setups, name).toHaveLength(1);
      for (const step of setups) expect(step, name).toMatch(/^ {10}package-manager-cache: false$/m);
    }
  });

  it('builds all six platforms', async () => {
    const build = job(await read(), 'build');
    for (const platform of PLATFORMS) expect(build).toContain(`key: ${platform}`);
  });
});
