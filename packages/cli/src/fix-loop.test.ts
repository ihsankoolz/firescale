/**
 * The fix loop (ADR 0018) through the real binary: ordering by `after`,
 * convergence, the errors when fixes do not settle, and the multi-pass dry run.
 * The sample rules are custom TypeScript rules, so this also covers `after`
 * in the SDK and the bridge.
 */
import { cp, mkdir, readdir, symlink } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, FAKE_ENV, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const REPO_ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const LOOP_PACK = fileURLToPath(new URL('../test-fixtures/loop-pack/', import.meta.url));
const NOTHING = 'Nothing to fix: no deterministic fix has anything left to do.\n';

/** A fixture repo with the loop-pack fixtures at tools/loop and one of its packs turned on. */
async function loopRepo(
  pack: string,
  rules: Record<string, string> = {},
  files: Record<string, string> = {},
): Promise<FixtureRepo> {
  const repo = await fixtures.create({
    files: {
      'README.md': '# fixture\n',
      'firescale.config.json': JSON.stringify({ packs: [`./tools/loop/${pack}.ts`], rules }),
      ...files,
    },
  });
  await cp(LOOP_PACK, path.join(repo.root, 'tools', 'loop'), { recursive: true });
  const modules = path.join(repo.root, 'node_modules');
  await mkdir(path.join(modules, '@firescale'), { recursive: true });
  // 'junction' works on Windows without admin rights; other platforms ignore the type.
  await symlink(
    path.join(REPO_ROOT, 'packages', 'sdk'),
    path.join(modules, '@firescale', 'sdk'),
    'junction',
  );
  return repo;
}

/** Every file in the repo root, so a test can prove that nothing was created. */
const rootFiles = async (repo: FixtureRepo): Promise<string[]> =>
  (await readdir(repo.root)).filter((name) => name !== '.git' && name !== '.firescale').sort();

describe('fix order', () => {
  it('fixes a rule after the rule it depends on, in one run', async () => {
    const repo = await loopRepo('order');
    const result = await runCli(['fix'], repo.root);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    // z-base sorts last by id, and still runs first, because a-derived says `after`.
    expect(result.stdout).toBe(
      [
        'order/z-base',
        '  created base.txt',
        '',
        'order/a-derived',
        '  created derived.txt',
        '',
        'Fixed: 2 changes in 2 rules. Run `fscale check` to see what is left.',
        '',
      ].join('\n'),
    );
    expect(await repo.read('base.txt')).toBe('base\n');
    expect(await repo.read('derived.txt')).toBe('derived from: base\n');

    const again = await runCli(['fix'], repo.root);
    expect(again.stdout).toBe(NOTHING);
    expect(again.code).toBe(EXIT_OK);
  });

  it('converges in a later pass when the dependency is not declared', async () => {
    const repo = await loopRepo('unordered');
    const result = await runCli(['fix'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(await repo.read('derived.txt')).toBe('derived from: base\n');
    expect((await runCli(['fix'], repo.root)).stdout).toBe(NOTHING);
  });

  it('leaves the order alone when --rules selects only the dependent rule', async () => {
    const repo = await loopRepo('order');
    const result = await runCli(['fix', '--rules', 'order/a-derived'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    // Its dependency did not run, so there is nothing it can do yet.
    expect(result.stdout).toBe(NOTHING);
    expect(await repo.read('base.txt')).toBeNull();
  });

  it('rejects a dependency cycle before anything is written', async () => {
    const repo = await loopRepo('cycle');
    const before = await rootFiles(repo);
    for (const argv of [['fix'], ['fix', '--dry-run'], ['fix', '--rules', 'cycle/one']]) {
      const result = await runCli(argv, repo.root);
      expect(result.code).toBe(EXIT_USAGE);
      expect(result.stdout).toBe('');
      expect(result.stderr).toBe(
        'fscale: These rules must each run after another of them, so there is no order to fix them in: "cycle/one" -> "cycle/two" -> "cycle/one". Remove one of the "after" entries to break the cycle.\n',
      );
    }
    expect(await rootFiles(repo)).toEqual(before);
  });

  it('a cycle is broken by turning one of its rules off', async () => {
    const repo = await loopRepo('cycle', { 'cycle/two': 'off' });
    const result = await runCli(['fix'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(await repo.read('one.txt')).toBe('1\n');
    expect(await repo.read('two.txt')).toBeNull();
  });

  it('rejects an `after` id that no rule has, before anything is written', async () => {
    const repo = await loopRepo('unknown');
    const before = await rootFiles(repo);
    const result = await runCli(['fix'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      'fscale: Rule "unknown/typo" must run after "core/gitignore-basic", but no rule has that id. Fix the id in the rule\'s "after" list.\n',
    );
    expect(await rootFiles(repo)).toEqual(before);
  });
});

describe('fixes that do not settle', () => {
  it('stops when two fixes undo each other, and names both', async () => {
    const repo = await loopRepo('fight');
    const result = await runCli(['fix'], repo.root);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatchSnapshot();
    expect(result.stderr).toContain('Rules that write style.txt: "fight/spaces", "fight/tabs"');
    expect(result.stderr).toContain('The fix stopped after these changes had already been made:');
  });

  it('stops at the pass limit when a fix never settles', async () => {
    const repo = await loopRepo('restless', { 'restless/lazy': 'off' });
    const result = await runCli(['fix'], repo.root);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stderr).toContain(
      'The fixes did not settle after 10 passes. Still changing files in the last pass: "restless/counter" (files: count.txt).',
    );
    expect(await repo.read('count.txt')).toBe('line\n'.repeat(10));
  });

  it('stops when a fix does nothing about the violations it claims', async () => {
    const repo = await loopRepo('restless', { 'restless/counter': 'off' });
    const result = await runCli(['fix'], repo.root);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stderr).toBe(
      'fscale: The fix of "restless/lazy" changed nothing, but the rule still reports violations it says its fix can repair. This is a bug in the rule: its fix does not do what its check asks for. Run `fscale check --rules restless/lazy` to see the violations, and fix them by hand.\n',
    );
  });

  it('in a dry run, reports the same failure and changes nothing', async () => {
    const repo = await loopRepo('fight');
    const before = await rootFiles(repo);
    const result = await runCli(['fix', '--dry-run'], repo.root);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stderr).toContain('The fixes do not settle');
    expect(result.stderr).toContain('This was a dry run, so nothing was changed.');
    expect(await rootFiles(repo)).toEqual(before);
  });
});

describe('fix --dry-run across dependent fixes', () => {
  it('prints what `fscale fix` then does, and changes nothing', async () => {
    const repo = await loopRepo('order');
    const before = await rootFiles(repo);
    const tracked = await repo.trackedFiles();

    const dry = await runCli(['fix', '--dry-run'], repo.root);
    expect(dry.code).toBe(EXIT_OK);
    expect(dry.stderr).toBe('');
    expect(dry.stdout).toMatchSnapshot();
    // The second fix saw the file the first one would create.
    expect(dry.stdout).toContain('+++ b/derived.txt\n@@ -0,0 +1,1 @@\n+derived from: base\n');
    expect(await rootFiles(repo)).toEqual(before);
    expect(await repo.trackedFiles()).toEqual(tracked);

    const real = await runCli(['fix'], repo.root);
    expect(real.code).toBe(EXIT_OK);
    // The same rules, files and order: only the wording and the diffs differ.
    const summary = (text: string): string[] =>
      text.split('\n').filter((line) => /^\S+\/\S+$|^ {2}\S/.test(line));
    expect(summary(dry.stdout).map((line) => line.replace('would create', 'created'))).toEqual(
      summary(real.stdout),
    );
    // And the contents on disk are the ones the diff showed.
    for (const [file, content] of [
      ['base.txt', 'base\n'],
      ['derived.txt', 'derived from: base\n'],
    ] as const) {
      expect(dry.stdout).toContain(`--- /dev/null\n+++ b/${file}\n@@ -0,0 +1,1 @@\n+${content}`);
      expect(await repo.read(file)).toBe(content);
    }
  });

  it('shows one rule editing the file another rule creates, for built-in rules', async () => {
    // No .gitignore: core/gitignore-basics creates it, then core/no-committed-env adds to it.
    const repo = await fixtures.create({ files: { '.env': FAKE_ENV, 'package.json': '{}\n' } });
    const dry = await runCli(['fix', '--dry-run'], repo.root);
    expect(dry.code).toBe(EXIT_OK);
    expect(dry.stdout).toMatchSnapshot();
    expect(dry.stdout).toContain('core/gitignore-basics\n  would create .gitignore\n');
    expect(dry.stdout).toContain(
      'core/no-committed-env\n  would untrack .env (the file stays on disk)\n  would edit .gitignore\n',
    );
    expect(await repo.read('.gitignore')).toBeNull();
    expect(await repo.trackedFiles()).toContain('.env');

    await runCli(['fix'], repo.root);
    const written = (await repo.read('.gitignore')) ?? '';
    // Every line the two diffs added is in the file, in the same order.
    const added = dry.stdout
      .split('\n')
      .filter((line) => line.startsWith('+') && !line.startsWith('+++'))
      .map((line) => line.slice(1));
    expect(written.split('\n').filter((line) => line !== '')).toEqual(
      added.filter((line) => line !== ''),
    );
  });
});

describe('every built-in fix is idempotent through the loop', () => {
  it('a second `fscale fix` changes nothing', async () => {
    // Triggers all three built-in fixes: gitignore-basics, env-example, no-committed-env.
    const repo = await fixtures.create({
      files: {
        '.env': FAKE_ENV,
        'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
        // Built from parts, so this test file is not itself seen as reading env vars.
        'src/index.js': `console.log(${['API_URL', 'PORT'].map((name) => `process.${'env'}.${name}`).join(', ')});\n`,
      },
    });
    const first = await runCli(['fix'], repo.root);
    expect(first.code).toBe(EXIT_OK);
    for (const rule of ['core/gitignore-basics', 'core/env-example', 'core/no-committed-env']) {
      expect(first.stdout).toContain(`${rule}\n`);
    }
    const snapshot = async (): Promise<Record<string, string | null>> => ({
      gitignore: await repo.read('.gitignore'),
      example: await repo.read('.env.example'),
      tracked: (await repo.trackedFiles()).join(','),
    });
    const after = await snapshot();
    expect(after.example).toContain('API_URL');

    const second = await runCli(['fix'], repo.root);
    expect(second.code).toBe(EXIT_OK);
    expect(second.stdout).toBe(NOTHING);
    expect(await snapshot()).toEqual(after);

    const dry = await runCli(['fix', '--dry-run'], repo.root);
    expect(dry.stdout).toBe(NOTHING);
  });
});
