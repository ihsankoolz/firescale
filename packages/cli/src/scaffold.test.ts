/**
 * `fscale init`, `fscale adopt` and `fscale sync` through the real binary
 * (SPEC §15, ADR 0019).
 *
 * The centre of this file is the promise that these commands never destroy a
 * person's work: the "existing work" tests run them over repos whose README,
 * CI workflow, .gitignore and AGENTS.md have unusual content, and compare
 * every file byte for byte before and after.
 */
import { execFile } from 'node:child_process';
import { mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const run = promisify(execFile);

interface Diagnostic {
  ruleId: string;
  file: string;
  fixable: 'deterministic' | 'agent' | 'manual';
}

/** Every file under `root` except git's own and firescale's cache, as bytes, keyed by forward-slash path. */
async function snapshot(root: string): Promise<Map<string, Buffer>> {
  const files = new Map<string, Buffer>();
  const walk = async (dir: string, prefix: string): Promise<void> => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (prefix === '' && (entry.name === '.git' || entry.name === '.firescale')) continue;
      const relative = prefix === '' ? entry.name : `${prefix}/${entry.name}`;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full, relative);
      else files.set(relative, await readFile(full));
    }
  };
  await walk(root, '');
  return files;
}

/** The git index (mode, blob and path of every entry), or '' outside a git repo. */
async function index(root: string): Promise<string> {
  try {
    return (await run('git', ['ls-files', '--stage'], { cwd: root })).stdout;
  } catch {
    return '';
  }
}

/** Splits text into lines that keep their line ending, so CRLF and LF stay distinct. */
const linesOf = (content: Buffer): string[] =>
  content.toString('utf8').match(/[^\n]*\n|[^\n]+$/g) ?? [];

/**
 * True when every line of `before` is found in `after`, ending included, in
 * the same order: nothing was removed, changed or reordered. One allowance:
 * a last line that had no line ending may have been given one, because
 * appending to a file requires it.
 */
function everyLineSurvives(before: Buffer, after: Buffer): boolean {
  const got = linesOf(after);
  let at = 0;
  for (const line of linesOf(before)) {
    const accepted = line.endsWith('\n') ? [line] : [line, `${line}\n`, `${line}\r\n`];
    const found = got.findIndex(
      (candidate, position) => position >= at && accepted.includes(candidate),
    );
    if (found === -1) return false;
    at = found + 1;
  }
  return true;
}

async function diagnostics(root: string): Promise<Diagnostic[]> {
  const result = await runCli(['check', '--json', '--no-cache'], root);
  return (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
}

async function exists(file: string): Promise<boolean> {
  return stat(file).then(
    () => true,
    () => false,
  );
}

/** An empty folder that is not a git repo. */
const emptyFolder = (): Promise<FixtureRepo> => fixtures.create({ track: 'no-git' });

describe('fscale init', () => {
  it('builds a repo with the core pack in an empty folder and leaves no deterministic violation', async () => {
    const repo = await emptyFolder();
    const result = await runCli(['init', '--packs', 'core'], repo.root);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();

    expect(await exists(path.join(repo.root, '.git'))).toBe(true);
    expect(await repo.read('firescale.config.json')).toBe('{\n  "packs": ["core"]\n}\n');
    expect([...(await snapshot(repo.root)).keys()].sort()).toEqual([
      '.github/workflows/ci.yml',
      '.gitignore',
      'README.md',
      'firescale.config.json',
    ]);
    expect((await diagnostics(repo.root)).filter((d) => d.fixable === 'deterministic')).toEqual([]);
  });

  it('turns on core and agent-ready by default and prints the violations left for a person', async () => {
    const repo = await emptyFolder();
    const result = await runCli(['init'], repo.root);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    expect(await repo.read('firescale.config.json')).toBe(
      '{\n  "packs": ["core", "agent-ready"]\n}\n',
    );

    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.fixable === 'deterministic')).toEqual([]);
    // The agent-ready pack has no fixes yet, so something is left, and init says so.
    expect(left.length).toBeGreaterThan(0);
    expect(result.stdout).toContain(
      `Left for you: ${String(left.length)} violation${left.length === 1 ? '' : 's'} that a fix cannot repair.`,
    );
    for (const rule of new Set(left.map((d) => d.ruleId))) {
      expect(result.stdout).toContain(`  ${rule}\n`);
      expect(result.stdout).toContain(`How to fix: fscale explain ${rule}`);
    }
    expect(result.stdout).toMatchSnapshot();
  });

  it('is repeatable: a second init keeps the config and changes nothing', async () => {
    const repo = await emptyFolder();
    await runCli(['init', '--packs', 'core'], repo.root);
    const before = await snapshot(repo.root);
    const again = await runCli(['init'], repo.root);
    expect(again.code).toBe(EXIT_OK);
    expect(again.stdout).toContain(
      'This folder already has a firescale config. It was kept as it is.',
    );
    expect(again.stdout).toContain('Nothing to change');
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('refuses a folder that is not empty and not a firescale repo, and points at adopt', async () => {
    const repo = await fixtures.create({
      files: { 'notes.txt': 'mine\n', 'src/main.py': 'print("hi")\n' },
      track: 'no-git',
    });
    const before = await snapshot(repo.root);
    const result = await runCli(['init'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatchInlineSnapshot(`
      "fscale: This folder is not empty (2 entries: notes.txt, src) and is not a firescale repo, so \`fscale init\` will not touch it.

      For a project that already exists, use adopt. It only adds what is missing and never overwrites a file:
        fscale adopt --dry-run   preview the changes
        fscale adopt             apply them

      To run init here anyway, pass --force. It adds the same files as adopt, with init's packs.
      "
    `);
    // Nothing happened: no git repo, no config, the files as they were.
    expect(await snapshot(repo.root)).toEqual(before);
    expect(await exists(path.join(repo.root, '.git'))).toBe(false);
  });

  it('also refuses a git repo that only holds a README', async () => {
    const repo = await fixtures.create({ files: { 'README.md': '# mine\n' } });
    const before = await snapshot(repo.root);
    const result = await runCli(['init'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('fscale adopt --dry-run');
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('runs in a folder that is not empty with --force, and overwrites nothing', async () => {
    const repo = await fixtures.create({
      files: { 'notes.txt': 'mine\n', 'README.md': 'My own words.' },
      track: 'no-git',
    });
    const before = await snapshot(repo.root);
    const result = await runCli(['init', '--force', '--packs', 'core'], repo.root);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    const after = await snapshot(repo.root);
    for (const [file, content] of before) expect(after.get(file)).toEqual(content);
    expect(after.has('firescale.config.json')).toBe(true);
  });

  it('refuses to start a second repo inside a firescale repo', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{}\n' } });
    const inner = path.join(repo.root, 'packages', 'new');
    await mkdir(inner, { recursive: true });
    const result = await runCli(['init'], inner);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('is inside the firescale repo at');
    expect(result.stderr).toContain('fscale sync');
    expect(await readdir(inner)).toEqual([]);
  });

  it('rejects an unknown pack before anything is written', async () => {
    const repo = await emptyFolder();
    const result = await runCli(['init', '--packs', 'core,nope'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('unknown rule pack "nope"');
    expect(result.stderr).toContain('Available packs: core, agent-ready');
    expect(await readdir(repo.root)).toEqual([]);
  });

  it('rejects --packs when a config already exists, and options that belong elsewhere', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{}\n' } });
    const packs = await runCli(['init', '--packs', 'core'], repo.root);
    expect(packs.code).toBe(EXIT_USAGE);
    expect(packs.stderr).toContain('already has a firescale config');

    const empty = await emptyFolder();
    for (const [argv, hint] of [
      [['init', '--dry-run'], 'fscale adopt --dry-run'],
      [['init', '--check'], 'fscale sync --check'],
      [['init', 'my-app'], 'use --cwd <dir>'],
      [['adopt', '--force'], 'fscale init --force'],
      [['adopt', '--packs', 'core'], 'fscale init --packs'],
      [['sync', '--dry-run'], 'fscale sync --check'],
      [['sync', '--rules', 'core/*'], 'fscale fix --rules'],
    ] as const) {
      const result = await runCli([...argv], empty.root);
      expect(result.code, argv.join(' ')).toBe(EXIT_USAGE);
      expect(result.stderr, argv.join(' ')).toContain(hint);
    }
    expect(await readdir(empty.root)).toEqual([]);
  });
});

// Content chosen to break a careless fix: CRLF, no final newline, a BOM,
// non-ASCII text, trailing spaces, tabs, and lines that look like firescale's own.
const ODD_README =
  '\uFEFF# Projet «Étoile» ☄\r\n\r\nPas de section d’installation ici.   \r\n\tindented with a tab\r\n<!-- firescale:begin -->not a real block<!-- firescale:end -->\r\nlast line, no newline';
const ODD_WORKFLOW =
  '# hand-written, push only: firescale must leave it alone\nname:   "weird" \non:\n    push:\n        branches: [ "trunk" ]\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo "pull_request"   # not a trigger\n';
const ODD_GITIGNORE = '# ours\r\n*.log\r\n!keep.log\r\n/dist/   \r\n\r\n\r\n# end, no newline';
const ODD_AGENTS =
  '# Agents\n\nNever touch `legacy/`.\n\n<!-- firescale:managed -->\nhand-written text that only looks managed\n<!-- /firescale:managed -->\n\n\tTabs\tand trailing spaces   \n';

const EXISTING_WORK: Record<string, string> = {
  'README.md': ODD_README,
  '.github/workflows/build.yml': ODD_WORKFLOW,
  '.gitignore': ODD_GITIGNORE,
  'AGENTS.md': ODD_AGENTS,
  'package.json': '{\n\t"name": "odd",\n\t"scripts": { "test": "node --test" }\n}',
  'src/index.js': 'console.log("hi");\r\n',
};

describe('existing work survives', () => {
  for (const [name, track, config] of [
    ['a repo with everything committed', 'all', undefined],
    ['a repo where nothing is tracked yet', 'none', undefined],
    ['a plain folder that is not a git repo', 'no-git', undefined],
    ['a repo with core and agent-ready on', 'all', '{ "packs": ["core", "agent-ready"] }'],
  ] as const) {
    it(`adopt leaves every existing line in place in ${name}`, async () => {
      const files = config ? { ...EXISTING_WORK, 'firescale.config.json': config } : EXISTING_WORK;
      const repo = await fixtures.create({ files, track });
      const before = await snapshot(repo.root);
      const tracked = await repo.trackedFiles();

      const result = await runCli(['adopt'], repo.root);
      expect(result.stderr).toBe('');
      expect(result.code).toBe(EXIT_OK);

      const after = await snapshot(repo.root);
      // No file is gone, and every line a person wrote is still there, in order.
      for (const [file, content] of before) {
        const now = after.get(file);
        expect(now, `${file} was deleted`).toBeDefined();
        expect(everyLineSurvives(content, now ?? Buffer.alloc(0)), `${file} lost a line`).toBe(
          true,
        );
      }
      // The README, the workflow, AGENTS.md and the source are byte for byte the same.
      for (const file of [
        'README.md',
        '.github/workflows/build.yml',
        'AGENTS.md',
        'package.json',
        'src/index.js',
      ]) {
        expect(after.get(file), file).toEqual(before.get(file));
      }
      // .gitignore only grew, after the person's last line, and kept its CRLF line endings.
      // Outside git there is nothing to ignore for, so it is not touched at all.
      const gitignore = (after.get('.gitignore') ?? Buffer.alloc(0)).toString('utf8');
      expect(gitignore.startsWith(ODD_GITIGNORE)).toBe(true);
      if (track === 'no-git') expect(gitignore).toBe(ODD_GITIGNORE);
      else expect(gitignore).toContain('\r\nnode_modules/\r\n');
      expect(gitignore.replaceAll('\r\n', '')).not.toContain('\n');
      // No second README or workflow was put beside the person's own.
      expect([...after.keys()].filter((file) => /readme/i.test(file))).toEqual(['README.md']);
      expect([...after.keys()].filter((file) => file.startsWith('.github/'))).toEqual([
        '.github/workflows/build.yml',
      ]);
      // Nothing a person had tracked was untracked.
      const trackedNow = await repo.trackedFiles();
      for (const file of tracked) expect(trackedNow).toContain(file);

      // What adopt could not do is listed, not papered over.
      expect(result.stdout).toContain('Left for you: ');
      expect(result.stdout).toContain('core/readme-setup');
      expect((await diagnostics(repo.root)).filter((d) => d.fixable === 'deterministic')).toEqual(
        [],
      );
    });
  }

  it('adopt does not replace files whose names differ only in case or extension', async () => {
    const files = {
      'ReadMe.MD': ODD_README,
      '.github/workflows/CI.YAML': ODD_WORKFLOW,
      '.github/workflows/deploy.yaml': ODD_WORKFLOW,
      'docs/README.md': 'nested\n',
    };
    const repo = await fixtures.create({ files, track: 'none' });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const after = await snapshot(repo.root);
    for (const [file, content] of before) expect(after.get(file), file).toEqual(content);
    expect([...after.keys()].sort()).toEqual(
      [...before.keys(), '.gitignore', 'firescale.config.json'].sort(),
    );
  });

  it('adopt keeps a committed env file on disk when it untracks it', async () => {
    const repo = await fixtures.create({
      files: { ...EXISTING_WORK, '.env': 'EXAMPLE=1\n' },
    });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('untracked .env (the file stays on disk)');
    const after = await snapshot(repo.root);
    expect(after.get('.env')).toEqual(before.get('.env'));
    for (const [file, content] of before) {
      expect(everyLineSurvives(content, after.get(file) ?? Buffer.alloc(0)), file).toBe(true);
    }
  });

  it('sync and init --force are as careful as adopt', async () => {
    for (const argv of [['sync'], ['init', '--force']]) {
      const files =
        argv[0] === 'sync' ? { ...EXISTING_WORK, 'firescale.config.json': '{}' } : EXISTING_WORK;
      const repo = await fixtures.create({ files });
      const before = await snapshot(repo.root);
      const result = await runCli(argv, repo.root);
      expect(result.code, argv.join(' ')).toBe(EXIT_OK);
      const after = await snapshot(repo.root);
      for (const [file, content] of before) {
        expect(
          everyLineSurvives(content, after.get(file) ?? Buffer.alloc(0)),
          `${argv.join(' ')}: ${file}`,
        ).toBe(true);
        if (file !== '.gitignore') expect(after.get(file), file).toEqual(content);
      }
    }
  });

  it('the line check itself notices a removed, changed or reordered line', () => {
    const text = (value: string): Buffer => Buffer.from(value, 'utf8');
    expect(everyLineSurvives(text('a\nb\n'), text('a\nb\nc\n'))).toBe(true);
    expect(everyLineSurvives(text('a\nb'), text('x\na\ny\nb\n'))).toBe(true);
    expect(everyLineSurvives(text('a\nb\n'), text('a\n'))).toBe(false);
    expect(everyLineSurvives(text('a\nb\n'), text('b\na\n'))).toBe(false);
    expect(everyLineSurvives(text('a \nb\n'), text('a\nb\n'))).toBe(false);
    expect(everyLineSurvives(text('a\r\nb\r\n'), text('a\nb\n'))).toBe(false);
    // A last line without a line ending may gain one, and nothing else.
    expect(everyLineSurvives(text('a\nb'), text('a\nb\r\nc\r\n'))).toBe(true);
    expect(everyLineSurvives(text('a\nb'), text('a\nbc\n'))).toBe(false);
  });
});

describe('fscale adopt', () => {
  const bare = (): Promise<FixtureRepo> =>
    fixtures.create({
      files: { 'package.json': '{ "name": "bare", "scripts": { "test": "node --test" } }\n' },
    });

  it('--dry-run prints a diff and changes nothing; adopt then leaves no deterministic violation', async () => {
    const repo = await bare();
    const before = await snapshot(repo.root);
    const indexBefore = await index(repo.root);

    const preview = await runCli(['adopt', '--dry-run'], repo.root);
    expect(preview.stderr).toBe('');
    expect(preview.code).toBe(EXIT_OK);
    expect(preview.stdout).toMatchSnapshot();
    expect(preview.stdout).toContain('--- /dev/null\n+++ b/firescale.config.json\n');
    expect(preview.stdout).toContain('+++ b/.github/workflows/ci.yml\n');
    expect(preview.stdout).toContain('Nothing was changed. Run `fscale adopt` to apply.');
    expect(await snapshot(repo.root)).toEqual(before);
    expect(await index(repo.root)).toBe(indexBefore);
    expect(await exists(path.join(repo.root, '.firescale'))).toBe(false);

    const real = await runCli(['adopt'], repo.root);
    expect(real.stderr).toBe('');
    expect(real.code).toBe(EXIT_OK);
    expect(real.stdout).toMatchSnapshot();
    expect((await diagnostics(repo.root)).filter((d) => d.fixable === 'deterministic')).toEqual([]);

    // The preview showed exactly the files the real run then wrote.
    const after = await snapshot(repo.root);
    const created = [...after.keys()].filter((file) => !before.has(file)).sort();
    const previewed = [...preview.stdout.matchAll(/^ {2}would create (\S+)/gm)]
      .map((match) => match[1])
      .sort();
    expect(previewed).toEqual(created);
    for (const file of created) {
      const lines = (after.get(file) ?? Buffer.alloc(0)).toString('utf8').trimEnd().split('\n');
      for (const line of lines) expect(preview.stdout, file).toContain(`+${line}\n`);
    }
  });

  it('writes a config with the core pack only, and a second adopt changes nothing', async () => {
    const repo = await bare();
    await runCli(['adopt'], repo.root);
    expect(await repo.read('firescale.config.json')).toBe('{\n  "packs": ["core"]\n}\n');
    const before = await snapshot(repo.root);
    const again = await runCli(['adopt'], repo.root);
    expect(again.code).toBe(EXIT_OK);
    expect(again.stdout).toContain(
      'Nothing to change: every deterministic fix is already in place.',
    );
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('uses a config that is already there, and never rewrites it', async () => {
    const valid = '{ "packs": ["core"], "rules": { "core/ci-exists": "off" } }';
    const repo = await fixtures.create({
      files: { 'firescale.config.json': valid, 'package.json': '{ "name": "x" }\n' },
    });
    const result = await runCli(['adopt'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(await repo.read('firescale.config.json')).toBe(valid);
    // The rule that is off wrote nothing.
    expect(await exists(path.join(repo.root, '.github'))).toBe(false);
    expect(result.stdout).not.toContain('created firescale.config.json');
  });

  it('stops before writing anything when the config is broken', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': '{ "packs": ["nope"] }' },
    });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('unknown rule pack "nope"');
    expect(await snapshot(repo.root)).toEqual(before);
  });
});

describe('fscale sync', () => {
  it('--check exits 1 after a generated file is deleted, and 0 after sync', async () => {
    const repo = await emptyFolder();
    await runCli(['init', '--packs', 'core'], repo.root);
    const clean = await runCli(['sync', '--check'], repo.root);
    expect(clean.code).toBe(EXIT_OK);
    expect(clean.stdout).toContain('In sync: `fscale sync` would change nothing.');

    for (const file of ['.github/workflows/ci.yml', 'README.md', '.gitignore']) {
      const generated = await repo.read(file);
      await rm(path.join(repo.root, ...file.split('/')));
      const before = await snapshot(repo.root);
      const indexBefore = await index(repo.root);

      const drift = await runCli(['sync', '--check'], repo.root);
      expect(drift.code, file).toBe(EXIT_VIOLATIONS);
      expect(drift.stdout, file).toContain(`would create ${file}`);
      expect(drift.stdout, file).toContain('Out of sync: `fscale sync` would make 1 change.');
      // --check changed nothing.
      expect(await snapshot(repo.root)).toEqual(before);
      expect(await index(repo.root)).toBe(indexBefore);

      const repair = await runCli(['sync'], repo.root);
      expect(repair.code, file).toBe(EXIT_OK);
      expect(repair.stdout, file).toContain(`created ${file}`);
      expect(await repo.read(file), file).toBe(generated);
      expect((await runCli(['sync', '--check'], repo.root)).code, file).toBe(EXIT_OK);
    }
  });

  it('also repairs a generated file that was committed and then deleted', async () => {
    const repo = await emptyFolder();
    await runCli(['init', '--packs', 'core'], repo.root);
    const git = (...args: string[]): Promise<unknown> =>
      run('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.com', ...args], {
        cwd: repo.root,
      });
    await git('add', '-A');
    await git('commit', '--quiet', '-m', 'init');
    await rm(path.join(repo.root, '.github'), { recursive: true });
    expect((await runCli(['sync', '--check'], repo.root)).code).toBe(EXIT_VIOLATIONS);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect((await runCli(['sync', '--check'], repo.root)).code).toBe(EXIT_OK);
    // Back to what was committed: git sees no difference.
    expect((await run('git', ['status', '--porcelain'], { cwd: repo.root })).stdout).toBe('');
  });

  it('needs a config, and says how to get one', async () => {
    const repo = await fixtures.create({ files: { 'README.md': '# x\n' } });
    const before = await snapshot(repo.root);
    for (const argv of [['sync'], ['sync', '--check']]) {
      const result = await runCli(argv, repo.root);
      expect(result.code).toBe(EXIT_USAGE);
      expect(result.stderr).toContain('No firescale config found');
      expect(result.stderr).toContain('fscale adopt');
      expect(result.stderr).toContain('fscale init');
    }
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('reports drift in a file a person edited back, without touching the rest', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': '{}',
        'package.json': '{ "name": "x" }\n',
        '.gitignore': 'node_modules/\n',
        'README.md': ODD_README,
      },
    });
    await runCli(['sync'], repo.root);
    await writeFile(path.join(repo.root, '.gitignore'), '# mine\n*.tmp\n');
    const drift = await runCli(['sync', '--check'], repo.root);
    expect(drift.code).toBe(EXIT_VIOLATIONS);
    expect(drift.stdout).toContain('would edit .gitignore');
    await runCli(['sync'], repo.root);
    expect(await repo.read('.gitignore')).toBe(
      '# mine\n*.tmp\n# Generated files and dependencies\nnode_modules/\n',
    );
    expect(await repo.read('README.md')).toBe(ODD_README);
  });
});
