import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { REPO } from './test-helpers.js';

// The staging half of the release pipeline (ADR 0092, steps 5 and 6): the
// stand-in npm, the commands run against what npm serves, and the gate on the
// real repos.

const SCRIPTS = path.join(REPO, 'scripts');

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
  const dir = await mkdtemp(path.join(tmpdir(), 'firescale-staging-'));
  temp.push(dir);
  return dir;
}
afterEach(async () => {
  for (const dir of temp.splice(0)) await rm(dir, { recursive: true, force: true, maxRetries: 5 });
});

describe('scripts/fake-npm.mjs', () => {
  async function registry(): Promise<Record<string, string>> {
    const dir = await scratch();
    return { FSCALE_FAKE_NPM_STATE: path.join(dir, 'registry.json') };
  }

  it('publishes, shows and moves dist-tags in its own registry file', async () => {
    const env = await registry();
    const dir = await scratch();
    const tarball = path.join(dir, 'firescale-cli-linux-x64-1.2.3.tgz');
    await writeFile(tarball, 'bytes');
    expect(
      (await node('fake-npm.mjs', ['publish', tarball, '--access', 'public', '--tag', 'next'], env))
        .code,
    ).toBe(0);
    const tags = await node(
      'fake-npm.mjs',
      ['view', '@firescale/cli-linux-x64', 'dist-tags', '--json'],
      env,
    );
    expect(JSON.parse(tags.stdout)).toEqual({ next: '1.2.3' });
    const integrity = await node(
      'fake-npm.mjs',
      ['view', '@firescale/cli-linux-x64@1.2.3', 'dist.integrity', '--json'],
      env,
    );
    // npm's own form: sha512 of the bytes, in base64.
    expect(JSON.parse(integrity.stdout)).toBe(
      'sha512-' +
        (await import('node:crypto')).createHash('sha512').update('bytes').digest('base64'),
    );
    expect(
      (
        await node(
          'fake-npm.mjs',
          ['dist-tag', 'add', '@firescale/cli-linux-x64@1.2.3', 'latest'],
          env,
        )
      ).code,
    ).toBe(0);
    const after = await node(
      'fake-npm.mjs',
      ['view', '@firescale/cli-linux-x64', 'dist-tags', '--json'],
      env,
    );
    expect(JSON.parse(after.stdout)).toEqual({ next: '1.2.3', latest: '1.2.3' });
  });

  it('answers as npm does for a version that is not there, a second publish, and a tag on a missing version', async () => {
    const env = await registry();
    const dir = await scratch();
    const tarball = path.join(dir, 'firescale-1.2.3.tgz');
    await writeFile(tarball, 'x');
    const missing = await node(
      'fake-npm.mjs',
      ['view', 'firescale@1.2.3', 'dist.integrity', '--json'],
      env,
    );
    expect(missing.code).toBe(1);
    expect(missing.stderr).toContain('E404');
    await node('fake-npm.mjs', ['publish', tarball, '--tag', 'next'], env);
    const again = await node('fake-npm.mjs', ['publish', tarball, '--tag', 'next'], env);
    expect(again.code).toBe(1);
    expect(again.stderr).toContain('E403');
    const tag = await node('fake-npm.mjs', ['dist-tag', 'add', 'firescale@9.9.9', 'latest'], env);
    expect(tag.code).toBe(1);
    expect(tag.stderr).toContain('E404');
  });

  it('fails a call as npm does when OIDC is refused, and can pass one that changes nothing', async () => {
    const env = await registry();
    await node('fake-npm.mjs', ['seed', 'firescale', 'latest', '1.0.0'], env);
    const refused = await node('fake-npm.mjs', ['dist-tag', 'add', 'firescale@1.0.0', 'next'], {
      ...env,
      FSCALE_FAKE_NPM_FAIL: 'dist-tag',
    });
    expect(refused.code).toBe(1);
    expect(refused.stderr).toContain('ENEEDAUTH');
    const silent = await node('fake-npm.mjs', ['dist-tag', 'add', 'firescale@1.0.0', 'next'], {
      ...env,
      FSCALE_FAKE_NPM_SILENT: 'dist-tag',
    });
    expect(silent.code).toBe(0);
    const tags = await node('fake-npm.mjs', ['view', 'firescale', 'dist-tags', '--json'], env);
    expect(JSON.parse(tags.stdout)).toEqual({ latest: '1.0.0' });
  });
});

describe('scripts/try-published.mjs', () => {
  /** A stand-in for `npx --yes firescale@<spec>`: logs each command, its folder and its npm cache. */
  async function fakeFscale(): Promise<{ command: string; log: string }> {
    const dir = await scratch();
    const script = path.join(dir, 'fake-fscale.mjs');
    const log = path.join(dir, 'log.txt');
    await writeFile(
      script,
      [
        "import { appendFileSync, readdirSync } from 'node:fs';",
        'const [command] = process.argv.slice(2);',
        'const entries = readdirSync(process.cwd()).length;',
        'const cache = process.env.npm_config_cache ?? "";',
        `appendFileSync(${JSON.stringify(log)}, JSON.stringify({ command, entries, cache, cwd: process.cwd() }) + "\\n");`,
        "if (command === 'init') appendFileSync('firescale.config.json', '{}');",
        'console.log(`fake ${command}`);',
        'const code = process.env[`FSCALE_FAKE_EXIT_${command.toUpperCase()}`];',
        'process.exit(code ? Number(code) : 0);',
        '',
      ].join('\n'),
    );
    return { command: `node "${script}"`, log };
  }
  const entries = async (log: string) =>
    (await readFile(log, 'utf8'))
      .split('\n')
      .filter(Boolean)
      .map(
        (line) =>
          JSON.parse(line) as { command: string; entries: number; cache: string; cwd: string },
      );

  it('runs init, check, verify and doctor in a new empty folder with a new npm cache', async () => {
    const fake = await fakeFscale();
    const result = await node('try-published.mjs', ['next'], { FSCALE_NPX_COMMAND: fake.command });
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    const runs = await entries(fake.log);
    expect(runs.map((run) => run.command)).toEqual(['init', 'check', 'verify', 'doctor']);
    expect(runs[0]?.entries).toBe(0);
    // One folder and one cache for the whole try, both new, both outside the repo.
    expect(new Set(runs.map((run) => run.cwd)).size).toBe(1);
    expect(new Set(runs.map((run) => run.cache)).size).toBe(1);
    expect(runs[0]?.cache).not.toBe('');
    expect(path.resolve(runs[0]?.cwd ?? '').startsWith(path.resolve(REPO))).toBe(false);
    expect(result.stdout).toContain('PASS  firescale@next doctor');
    expect(result.stdout).toContain('firescale@next: init, check, verify and doctor all exit 0.');
  });

  it('fails at the first command that does not exit 0, naming it', async () => {
    const fake = await fakeFscale();
    const result = await node('try-published.mjs', ['1.2.3'], {
      FSCALE_NPX_COMMAND: fake.command,
      FSCALE_FAKE_EXIT_VERIFY: '1',
    });
    expect(result.code).toBe(1);
    expect(result.stdout).toContain('FAIL  firescale@1.2.3 verify (exit 1)');
    expect(result.stdout).toContain('fake verify');
    expect(result.stderr).toContain('`firescale@1.2.3 verify` exited 1');
    expect((await entries(fake.log)).map((run) => run.command)).toEqual([
      'init',
      'check',
      'verify',
    ]);
  });

  it('accepts a dist-tag or a version, nothing else', async () => {
    for (const bad of ['next; echo hi', '1.2', '', 'Latest']) {
      const result = await node('try-published.mjs', [bad]);
      expect(result.code, bad).toBe(1);
      expect(result.stderr).toContain('is not a dist-tag or a version');
    }
  });
});

describe('scripts/real-repos-gate.mjs', () => {
  async function summary(lines: string[]): Promise<string> {
    const file = path.join(await scratch(), 'summary.txt');
    await writeFile(file, `${lines.join('\n')}\n`);
    return file;
  }

  it('passes when every fscale command exited with a code fscale uses', async () => {
    const file = await summary([
      '1-doctor: exit 1 after 2 s',
      '1-check: exit 0 after 1 s',
      '1-verify-budget: exit 2 after 3 s',
      '2-own-test: exit 101 after 30 s',
      '3-verify-again: exit 130 after 9 s',
    ]);
    const result = await node('real-repos-gate.mjs', ['--summary', file]);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(0);
    expect(result.stdout).toContain('4 fscale commands, none crashed');
  });

  it("fails on a crash in fscale's own commands, not in the repo's", async () => {
    const file = await summary([
      '1-doctor: exit 0 after 2 s',
      '1-check: exit 101 after 1 s',
      '2-own-test: exit 1 after 30 s',
      '3-verify-again: exit 127 after 0 s',
    ]);
    const result = await node('real-repos-gate.mjs', ['--summary', file]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('1-check exited 101');
    expect(result.stderr).toContain('3-verify-again exited 127');
    expect(result.stderr).not.toContain('2-own-test');
  });

  it('fails when the installed fscale is not the version, or when fscale never ran', async () => {
    const file = await summary(['1-check: exit 0 after 1 s']);
    const wrong = await node('real-repos-gate.mjs', [
      '--summary',
      file,
      '--version',
      '1.2.3',
      '--printed',
      '1.2.2',
    ]);
    expect(wrong.code).toBe(1);
    expect(wrong.stderr).toContain('the installed fscale printed "1.2.2", not 1.2.3');
    const right = await node('real-repos-gate.mjs', [
      '--summary',
      file,
      '--version',
      '1.2.3',
      '--printed',
      '1.2.3',
    ]);
    expect(right.code).toBe(0);

    const none = await node('real-repos-gate.mjs', [
      '--summary',
      path.join(await scratch(), 'none.txt'),
    ]);
    expect(none.code).toBe(1);
    expect(none.stderr).toContain('fscale never ran');
  });
});

describe('.github/workflows/real-repos.yml', () => {
  const read = () => readFile(path.join(REPO, '.github', 'workflows', 'real-repos.yml'), 'utf8');

  it('can be called with fscale from npm instead of a build', async () => {
    const text = await read();
    expect(text).toMatch(/\n {2}workflow_call:\n {4}inputs:\n/);
    for (const input of ['fscale', 'version', 'only']) {
      expect(text).toMatch(new RegExp(`\\n {6}${input}:\\n(?: {8}.+\\n)*? {8}type: string\\n`));
    }
    // No build when fscale comes from npm.
    expect(text).toMatch(/\n {2}build:\n(?: {4}.+\n)*? {4}if: \$\{\{ !inputs\.fscale \}\}\n/);
    expect(text).toContain('npm install --prefix "$RUNNER_TEMP/fscale-npm"');
  });

  it('gates every repo on fscale not crashing, and on the version from npm', async () => {
    const text = await read();
    expect(text).toContain('scripts/real-repos-gate.mjs --summary out/summary.txt');
    expect(text).toContain('--version "$VERSION"');
    // The gate is not allowed to fail quietly.
    const gate = text.slice(text.indexOf("- name: 'Gate:"));
    expect(gate.slice(0, gate.indexOf('\n      - '))).not.toContain('continue-on-error');
  });
});
