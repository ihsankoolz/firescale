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
        if (!skip.includes(`${platform}/${name}`)) await writeFile(path.join(folder, name), '');
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

  /** A stand-in for npm: logs each call, and fails a publish whose tarball name contains FSCALE_FAKE_NPM_FAIL. */
  async function fakeNpm(
    version = '11.19.1',
  ): Promise<{ script: string; calls: () => Promise<string[]> }> {
    const dir = await scratch();
    const script = path.join(dir, 'fake-npm.mjs');
    const log = path.join(dir, 'calls.log');
    await writeFile(
      script,
      [
        "import { appendFileSync } from 'node:fs';",
        "import path from 'node:path';",
        'const args = process.argv.slice(2);',
        `if (args[0] === '--version') { console.log(${JSON.stringify(version)}); process.exit(0); }`,
        'const shown = args.map((arg) => (arg.endsWith(".tgz") ? path.basename(arg) : arg)).join(" ");',
        `appendFileSync(${JSON.stringify(log)}, shown + "\\n");`,
        'const fail = process.env.FSCALE_FAKE_NPM_FAIL;',
        'process.exit(fail && shown.includes(fail) ? 1 : 0);',
        '',
      ].join('\n'),
    );
    const calls = async () =>
      (await readFile(log, 'utf8').catch(() => '')).split('\n').filter(Boolean);
    return { script, calls };
  }

  const publish = (
    dir: string,
    version: string,
    script: string,
    env: Record<string, string> = {},
  ) =>
    node('publish-npm.mjs', ['--dir', dir, '--version', version], {
      FSCALE_NPM_REGISTRY: registry,
      FSCALE_NPM_COMMAND: script,
      ...env,
    });

  it('publishes the eight tarballs in order, the shared two from the linux-x64 build', async () => {
    const npm = await fakeNpm();
    const dir = await artifacts('1.2.3');
    const result = await publish(dir, '1.2.3', npm.script);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    expect(await npm.calls()).toEqual([
      ...PLATFORMS.map((p) => `publish firescale-cli-${p}-1.2.3.tgz --access public`),
      'publish firescale-sdk-1.2.3.tgz --access public',
      'publish firescale-1.2.3.tgz --access public',
    ]);
    expect(result.stdout).toContain('Done: 8 published, 0 already on the registry.');
  });

  it('stops at the first publish that fails, and a second run finishes the release', async () => {
    const npm = await fakeNpm();
    const dir = await artifacts('1.2.3');
    const first = await publish(dir, '1.2.3', npm.script, {
      FSCALE_FAKE_NPM_FAIL: 'firescale-sdk-',
    });
    expect(first.code).toBe(1);
    expect(first.stderr).toContain('npm publish failed for @firescale/sdk@1.2.3');
    expect(first.stderr).toContain('run the job again');
    // firescale itself was never attempted.
    expect(await npm.calls()).toHaveLength(7);

    // The registry now has the six that went through.
    published = new Set(PLATFORMS.map((p) => `/@firescale%2fcli-${p}/1.2.3`));
    const second = await publish(dir, '1.2.3', npm.script);
    expect(second.code).toBe(0);
    expect(second.stdout).toContain('Done: 2 published, 6 already on the registry.');
    expect((await npm.calls()).slice(7)).toEqual([
      'publish firescale-sdk-1.2.3.tgz --access public',
      'publish firescale-1.2.3.tgz --access public',
    ]);
  });

  it('publishes a pre-release under the dist-tag next', async () => {
    const npm = await fakeNpm();
    const result = await publish(await artifacts('2.0.0-rc.1'), '2.0.0-rc.1', npm.script);
    expect(result.code).toBe(0);
    const calls = await npm.calls();
    expect(calls).toHaveLength(8);
    expect(calls.every((call) => call.endsWith('--access public --tag next'))).toBe(true);
  });

  it('refuses an npm too old for trusted publishing, before anything is published', async () => {
    const npm = await fakeNpm('10.9.9');
    const result = await publish(await artifacts('1.2.3'), '1.2.3', npm.script);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('needs npm 11.5.1 or later, and this is 10.9.9');
    expect(await npm.calls()).toEqual([]);
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

  it('rejects something that is not a version', async () => {
    const result = await node('verify-published.mjs', ['1.2.3 && echo hi', '--attempts', '1']);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('is not a version');
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

  it('gives only the publish job the OIDC permission', async () => {
    const text = await read();
    expect(text.match(/id-token: write/g)).toHaveLength(1);
    expect(job(text, 'publish')).toContain('id-token: write');
    // The default for every other job.
    expect(text).toMatch(/\npermissions:\n {2}contents: read\n/);
  });

  it('publishes only for a pushed tag, after the version check and the build', async () => {
    const text = await read();
    const publish = job(text, 'publish');
    expect(publish).toContain("if: github.event_name == 'push' && github.ref_type == 'tag'");
    expect(publish).toContain('needs: [version, build]');
    expect(publish).toContain('runs-on: ubuntu-latest');
    expect(publish).toContain('node scripts/publish-npm.mjs --dir artifacts');
    expect(job(text, 'build')).toContain('needs: version');
    expect(job(text, 'build')).not.toContain('npm publish');
    expect(job(text, 'version')).toContain('bump-version.mjs --check --tag "$TAG"');
    // A tag on a commit that is not on main is refused before any build.
    expect(job(text, 'version')).toContain(
      'git merge-base --is-ancestor "$GITHUB_SHA" origin/main',
    );
    for (const name of ['version', 'publish', 'verify']) {
      expect(job(text, name)).toMatch(/timeout-minutes: \d+/);
    }
    // A manual run has no job that publishes: verify needs publish, which is skipped.
    const verify = job(text, 'verify');
    expect(verify).toContain('needs: [version, publish]');
    expect(verify).toContain('os: [ubuntu-latest, macos-latest, windows-latest]');
    expect(text).toMatch(/\non:\n {2}workflow_dispatch:\n {2}push:\n {4}tags: \['v\*'\]\n/);
  });

  it('pins every action to a commit', async () => {
    const uses = (await read()).match(/uses: .+/g) ?? [];
    expect(uses.length).toBeGreaterThan(8);
    for (const line of uses) expect(line).toMatch(/^uses: [\w./-]+@[0-9a-f]{40} # v\d+\.\d+\.\d+$/);
  });

  it('builds all six platforms', async () => {
    const build = job(await read(), 'build');
    for (const platform of PLATFORMS) expect(build).toContain(`key: ${platform}`);
  });
});
