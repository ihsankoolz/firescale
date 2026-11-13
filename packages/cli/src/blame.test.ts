import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import {
  baseCalls,
  commitAll,
  EXIT_OK,
  EXIT_VIOLATIONS,
  runCli,
  stepCalls,
} from './test-helpers.js';

// Blame-aware verify: the base run (M19, ADR 0037).

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);

interface Step {
  name: string;
  project?: string;
  status: string;
  label?: string;
  base?: { status: string; source: string; note?: string };
}

interface Report {
  schemaVersion: number;
  ok: boolean;
  result: string;
  steps: Step[];
  base?: { compared: boolean; reason?: string; ran: number; sameInputs: number; fromCache: number };
  notes?: string[];
}

async function report(json: string): Promise<Report> {
  const schema: object = JSON.parse(await readFile(SCHEMA, 'utf8'));
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  check(data);
  expect(check.errors ?? []).toEqual([]);
  return data as Report;
}

function step(report: Report, name: string, project?: string): Step {
  const found = report.steps.find((s) => s.name === name && s.project === project);
  if (!found) throw new Error(`no step ${name} in ${project ?? 'the root'}`);
  return found;
}

/**
 * A step that passes or fails by what `file` holds: "fail" fails it. It
 * appends `head <name>` or `base <name>` to the log, by whether it runs in
 * the base worktree.
 */
function stepScript(name: string, file: string, log: string): string {
  return [
    "const fs = require('fs');",
    "const where = process.cwd().includes('firescale-base-') ? 'base' : 'head';",
    `fs.appendFileSync(${JSON.stringify(log)}, where + ' ${name}\\n');`,
    `process.exit(fs.readFileSync(${JSON.stringify(file)}, 'utf8').includes('fail') ? 1 : 0);`,
    '',
  ].join('\n');
}

/**
 * A repo on `main`, committed, with lint and test steps that fail when
 * `lint.txt` or `test.txt` says "fail". The log of runs sits outside it.
 */
async function blameRepo(
  lint: 'ok' | 'fail',
  test: 'ok' | 'fail',
  verify: Record<string, unknown> = {},
) {
  const outside = await fixtures.create({ files: {}, track: 'no-git' });
  const log = path.join(outside.root, 'runs.log');
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: 'node lint.js', typecheck: false, test: 'node test.js', ...verify },
      }),
      '.gitignore': '.firescale/\n',
      'lint.js': stepScript('lint', 'lint.txt', log),
      'test.js': stepScript('test', 'test.txt', log),
      'lint.txt': `${lint}\n`,
      'test.txt': `${test}\n`,
      'package-lock.json': '{"lockfileVersion":3}\n',
    },
    track: 'no-git',
  });
  const root = repo.root;
  const init = await runGit(root, ['init', '--quiet', '--initial-branch=main']);
  expect(init.code).toBe(0);
  await commitAll(root, 'base');
  // The change is made on a branch; the base is its merge base with main.
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  const runs = async (): Promise<string[]> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean);
  const write = (file: string, content: string) => writeFile(path.join(root, file), content);
  return { root, runs, write };
}

describe('blame-aware verify', () => {
  it('passes, labelled pre-existing, when the test already fails on the base commit', async () => {
    const { root, write } = await blameRepo('ok', 'fail');
    await write('lint.txt', 'ok, changed\n');
    const result = await runCli(['verify'], root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout.split('\n')[0]).toMatch(
      /^verify: OK - no new failures; pre-existing \(fails on the base commit [0-9a-f]{7} too\): test$/,
    );
    expect(result.stdout).toMatch(/ {2}test +FAILED +\[pre-existing\] node test\.js \(exit 1\)/);
    expect(result.stdout).toMatch(/Pre-existing: test fails on the base commit [0-9a-f]{7} too/);
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(json.schemaVersion).toBe(6);
    expect(json.result).toBe('pre-existing');
    expect(json.ok).toBe(true);
    expect(step(json, 'test').label).toBe('pre-existing');
    expect(step(json, 'test').base?.status).toBe('failed');
  });

  it('runs no base step for a step that passed on the head, so a repaired one has no label (ADR 0043)', async () => {
    const { root, runs, write } = await blameRepo('ok', 'fail');
    await write('lint.txt', 'fail\n');
    await write('test.txt', 'ok\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'lint').label).toBe('new');
    expect(step(json, 'test').status).toBe('passed');
    expect(step(json, 'test').label).toBeUndefined();
    expect(step(json, 'test').base).toBeUndefined();
    expect(json.base).toMatchObject({ ran: 1 });
    expect((await runs()).filter((run) => run.startsWith('base '))).toEqual(['base lint']);
  });

  it('blocks a failure introduced on the head, committed or not, and with verify.labelFixed labels a repaired one fixed', async () => {
    const { root, write } = await blameRepo('ok', 'fail', { labelFixed: true });
    // Uncommitted: lint breaks, test is repaired.
    await write('lint.txt', 'fail\n');
    await write('test.txt', 'ok\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(json.result).toBe('failed');
    expect(step(json, 'lint').label).toBe('new');
    expect(step(json, 'test').label).toBe('fixed');
    const human = await runCli(['verify'], root);
    // The repaired test passed in the run above, so its pass comes from the cache.
    expect(human.stdout.split('\n')[0]).toBe(
      'verify: FAILED at lint (new) (1 of 3 steps passed from the cache, not run now)',
    );
    expect(human.stdout).toMatch(
      /New: lint passes on the base commit [0-9a-f]{7}, so this change broke it\./,
    );
    expect(human.stdout).toMatch(
      /Fixed by this change \(it fails on the base commit [0-9a-f]{7}\): test/,
    );

    // Committed on the branch: the base is still the merge base with main.
    await commitAll(root, 'break lint');
    const committed = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(committed.result).toBe('failed');
    expect(step(committed, 'lint').label).toBe('new');
    expect(step(committed, 'test').label).toBe('fixed');
  });

  it('does not let a pre-existing lint failure hide a new test failure', async () => {
    const { root, write } = await blameRepo('fail', 'ok');
    await write('test.txt', 'fail\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'lint').status).toBe('failed');
    expect(step(json, 'lint').label).toBe('pre-existing');
    expect(step(json, 'test').status).toBe('failed');
    expect(step(json, 'test').label).toBe('new');
    const human = await runCli(['verify'], root);
    expect(human.stdout.split('\n')[0]).toMatch(
      /^verify: FAILED at test \(new\); pre-existing \(fails on the base commit [0-9a-f]{7} too, does not block\): lint$/,
    );
  });

  it('makes no base run the second time with the same base, and runs it again after a lockfile change', async () => {
    const { root, runs, write } = await blameRepo('ok', 'fail');
    // An install command for the base worktree, so a lockfile change can be judged.
    const config = JSON.parse(await readFile(path.join(root, 'firescale.config.json'), 'utf8'));
    config.verify.baseInstall = 'node -e 0';
    await write('firescale.config.json', JSON.stringify(config));
    await commitAll(root, 'base install');
    await write('lint.txt', 'ok, changed\n');
    await runCli(['verify'], root);
    // lint passes on the head, so only test runs on the base (ADR 0043).
    const first = (await runs()).filter((run) => run.startsWith('base '));
    expect(first).toEqual(['base test']);
    const second = await report((await runCli(['verify', '--json'], root)).stdout);
    expect((await runs()).filter((run) => run.startsWith('base '))).toHaveLength(1);
    expect(second.base).toMatchObject({ ran: 0, fromCache: 1 });
    expect(step(second, 'test').label).toBe('pre-existing');
    expect(step(second, 'test').base?.source).toBe('cache');
    await write('package-lock.json', '{"lockfileVersion":3,"packages":{}}\n');
    const third = await report((await runCli(['verify', '--json'], root)).stdout);
    expect((await runs()).filter((run) => run.startsWith('base '))).toHaveLength(2);
    expect(step(third, 'test').base?.source).toBe('ran');
  });

  it("does not judge the base with the head's dependencies after a lockfile change", async () => {
    const { root, runs, write } = await blameRepo('ok', 'fail');
    await write('package-lock.json', '{"lockfileVersion":3,"packages":{}}\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test').label).toBeUndefined();
    expect(step(json, 'test').base?.status).toBe('setup-needed');
    expect(json.notes?.join('\n')).toContain(
      "package-lock.json changed, so the head's installed dependencies do not stand for the base's; set verify.baseInstall",
    );
    expect((await runs()).filter((run) => run.startsWith('base '))).toEqual([]);
  });

  it('never touches the working tree or the index', async () => {
    const { root, write } = await blameRepo('ok', 'fail');
    await write('lint.txt', 'fail\n');
    await write('untracked.txt', 'new file\n');
    await runGit(root, ['add', 'lint.txt']);
    await write('lint.txt', 'fail, and an unstaged change\n');
    const state = async () => {
      const files: string[] = [];
      for (const file of ['lint.txt', 'test.txt', 'untracked.txt', 'lint.js', 'test.js']) {
        files.push(await readFile(path.join(root, file), 'utf8'));
      }
      const index = createHash('sha256')
        .update(await readFile(path.join(root, '.git', 'index')))
        .digest('hex');
      const status = (await runGit(root, ['status', '--porcelain'])).stdout;
      const worktrees = (await runGit(root, ['worktree', 'list'])).stdout.split('\n').length;
      return { files, index, status, worktrees };
    };
    const before = await state();
    const result = await runCli(['verify'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(await state()).toEqual(before);
  });

  it('still blocks a failing test on the default branch with a clean tree, where there is no base', async () => {
    const { root } = await blameRepo('ok', 'fail');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(json.result).toBe('failed');
    expect(step(json, 'test').label).toBeUndefined();
    expect(json.base?.compared).toBe(false);
    expect(json.base?.reason).toContain('nothing to compare');
    const human = await runCli(['verify'], root);
    expect(human.stdout).toContain(
      'No base commit to compare with (the working tree is the base commit',
    );
  });

  it('falls back to blocking when the base run runs out of its budget, and says so', async () => {
    const { root, write } = await blameRepo('ok', 'fail');
    const config = JSON.parse(await readFile(path.join(root, 'firescale.config.json'), 'utf8'));
    config.verify.test = 'node slow-on-base.js';
    config.verify.baseBudget = '2s';
    await write('firescale.config.json', JSON.stringify(config));
    await write(
      'slow-on-base.js',
      "if (process.cwd().includes('firescale-base-')) setTimeout(() => process.exit(1), 20000); else process.exit(1);\n",
    );
    await commitAll(root, 'slow on the base');
    expect((await runGit(root, ['branch', '--force', 'main', 'HEAD'])).code).toBe(0);
    await write('lint.txt', 'ok, changed\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test').label).toBeUndefined();
    expect(step(json, 'test').base?.status).toBe('over-budget');
    expect(json.notes?.join('\n')).toContain('ran out of its time budget (2s, verify.baseBudget)');
  }, 30_000);

  it('never labels a step that needs setup on the head new', async () => {
    const { root, write } = await blameRepo('ok', 'ok');
    const config = JSON.parse(await readFile(path.join(root, 'firescale.config.json'), 'utf8'));
    config.verify.test = 'no-such-program-firescale-test';
    await write('firescale.config.json', JSON.stringify(config));
    await commitAll(root, 'a missing program');
    await write('lint.txt', 'ok, changed\n');
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(json, 'test').status).toBe('setup-needed');
    expect(step(json, 'test').label).toBeUndefined();
    expect(json.result).toBe('setup-needed');
  });
});

describe('blame-aware verify needs evidence before it lets a failure pass', () => {
  it('does not hide a new failure in a step that also failed on the base another way', async () => {
    const { root, write } = await blameRepo('ok', 'ok');
    // test.js prints which checks fail: on the base "mul" is broken.
    const verbose = [
      "const fs = require('fs');",
      "const broken = fs.readFileSync('test.txt', 'utf8').split(',').map((s) => s.trim()).filter(Boolean);",
      "for (const name of broken) console.log('FAIL ' + name);",
      'process.exit(broken.length > 0 ? 1 : 0);',
      '',
    ].join('\n');
    await write('test.js', verbose);
    await write('test.txt', 'mul\n');
    await commitAll(root, 'mul is broken on the base');
    // The base of the branch is still the first commit; move main along.
    expect((await runGit(root, ['branch', '--force', 'main', 'HEAD'])).code).toBe(0);
    await write('test.txt', 'mul, add\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test').label).toBeUndefined();
    expect(step(json, 'test').base?.note).toContain('not in the same way');
    // The same failure as on the base does not block.
    await write('test.txt', 'mul\n');
    await write('lint.txt', 'ok, changed\n');
    const same = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(same, 'test').label).toBe('pre-existing');
  });

  it('never makes a commit on the default branch its own base', async () => {
    const { root, write } = await blameRepo('ok', 'ok');
    expect((await runGit(root, ['checkout', '--quiet', 'main'])).code).toBe(0);
    await write('test.txt', 'fail\n');
    await commitAll(root, 'a broken commit on main');
    await write('lint.txt', 'ok, changed\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(json.base?.compared).toBe(false);
    expect(json.base?.reason).toContain('default branch main with no upstream');
    expect(step(json, 'test').label).toBeUndefined();
  });

  it('judges a project that is new on the head without failing the run', async () => {
    const { root, write } = await blameRepo('ok', 'ok');
    const config = JSON.parse(await readFile(path.join(root, 'firescale.config.json'), 'utf8'));
    config.verify.projects = {
      fresh: { lint: false, typecheck: false, test: 'node -e "process.exit(1)"' },
    };
    await write('firescale.config.json', JSON.stringify(config));
    const { mkdir } = await import('node:fs/promises');
    await mkdir(path.join(root, 'fresh'), { recursive: true });
    await write('fresh/package.json', '{"name":"fresh"}\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test', 'fresh').status).toBe('failed');
    expect(step(json, 'test', 'fresh').label).toBeUndefined();
    expect(step(json, 'test', 'fresh').base?.note).toContain('does not exist on the base commit');
  });

  it("leaves the index untouched even when a file's time changed", async () => {
    const { root, write } = await blameRepo('ok', 'fail');
    const index = async () =>
      createHash('sha256')
        .update(await readFile(path.join(root, '.git', 'index')))
        .digest('hex');
    // Same content, new time: `git status` would refresh the index.
    await write('test.js', await readFile(path.join(root, 'test.js'), 'utf8'));
    await write('lint.txt', 'ok, changed\n');
    const before = await index();
    await runCli(['verify'], root);
    expect(await index()).toBe(before);
  });
});

describe("the base worktree's dependencies", () => {
  it("links the head's node_modules in, and links nothing when verify.baseInstall is set", async () => {
    const { root, write } = await blameRepo('ok', 'fail');
    await write('.gitignore', '.firescale/\nnode_modules/\n');
    const probe = [
      "const fs = require('fs');",
      "if (process.cwd().includes('firescale-base-')) {",
      "  fs.appendFileSync(process.argv[2], 'base ' + (fs.existsSync('node_modules/marker.txt') ? 'linked' : 'none') + '\\n');",
      '}',
      'process.exit(1);',
      '',
    ].join('\n');
    await write('probe.js', probe);
    const config = JSON.parse(await readFile(path.join(root, 'firescale.config.json'), 'utf8'));
    const log = path.join(root, '..', `${path.basename(root)}-probe.log`);
    config.verify.test = `node probe.js ${JSON.stringify(log)}`;
    await write('firescale.config.json', JSON.stringify(config));
    await commitAll(root, 'probe');
    expect((await runGit(root, ['branch', '--force', 'main', 'HEAD'])).code).toBe(0);
    const { mkdir } = await import('node:fs/promises');
    await mkdir(path.join(root, 'node_modules'), { recursive: true });
    await write('node_modules/marker.txt', 'installed\n');
    await write('lint.txt', 'ok, changed\n');
    await runCli(['verify'], root);
    config.verify.baseInstall = 'node -e 0';
    await write('firescale.config.json', JSON.stringify(config));
    await runCli(['verify'], root);
    expect((await readFile(log, 'utf8')).split(/\r?\n/).filter(Boolean)).toEqual([
      'base linked',
      'base none',
    ]);
    // The head's node_modules is untouched.
    expect(await readFile(path.join(root, 'node_modules', 'marker.txt'), 'utf8')).toBe(
      'installed\n',
    );
  });
});

describe('blame-aware verify on the course project', () => {
  it('runs on the base only the projects whose inputs differ', async () => {
    const repo: FixtureRepo = await fixtures.create({ files: courseProjectFiles() });
    await commitAll(repo.root, 'initial');
    expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    const log = path.join(tools.root, 'calls.log');
    // Every Python test fails, on the head and on the base.
    const env = shimEnv(tools.root, log, 'python');
    await repo.write('backend/audit_service/main.py', 'app = 1\n');
    const result = await runCli(['verify', '--json'], repo.root, env);
    const json = await report(result.stdout);
    expect(result.code).toBe(EXIT_OK);
    expect(json.result).toBe('pre-existing');
    expect(await baseCalls(log)).toEqual(['python -m pytest in audit_service']);
    for (const service of [
      'auth_service',
      'ledger_service',
      'market_service',
      'realtime_service',
    ]) {
      const test = step(json, 'test', `backend/${service}`);
      expect(test.label).toBe('pre-existing');
      expect(test.base?.source).toBe('same-inputs');
    }
    expect(step(json, 'test', 'backend/audit_service').base?.source).toBe('ran');
    expect((await stepCalls(log)).length).toBeGreaterThan(0);
  });
});

// Per-test labels (M20, ADR 0038).

/** A stand-in for pytest: tests test_a, test_b and test_c in each tests/test_*.py, failing those failing.txt names. */
const FAKE_PYTEST = [
  "const fs = require('fs');",
  "const files = fs.readdirSync('tests').filter((f) => f.startsWith('test_') && f.endsWith('.py')).sort();",
  "const failing = fs.readFileSync('failing.txt', 'utf8').split(',').map((s) => s.trim()).filter(Boolean);",
  // Split PYTEST_ADDOPTS as pytest does (shlex, POSIX): a word in single
  // quotes stays one word; a word that is not an option is a path, and
  // pytest stops with exit 4 when it does not exist.
  "const words = [...(process.env.PYTEST_ADDOPTS || '').matchAll(/'([^']*)'|(\\S+)/g)].map((m) => m[1] ?? m[2]);",
  "for (let i = 0; i < words.length; i += 1) { if (words[i] === '-o') { i += 1; continue; } if (!words[i].startsWith('-')) { console.log('ERROR: file or directory not found: ' + words[i]); process.exit(4); } }",
  "const report = /^--junitxml=(.+)$/.exec(words.find((w) => w.startsWith('--junitxml=')) || '');",
  'let xml = \'<?xml version="1.0"?><testsuites><testsuite name="pytest">\';',
  'for (const file of files) {',
  "  for (const name of ['test_a', 'test_b', 'test_c']) {",
  '    const bad = failing.includes(name);',
  "    xml += '<testcase classname=\"tests.' + file.replace('.py', '') + '\" name=\"' + name + '\">' + (bad ? '<failure message=\"assert\">assert False</failure>' : '') + '</testcase>';",
  "    if (bad) console.log('FAILED tests/' + file + '::' + name + ' - assert False');",
  '  }',
  '}',
  "xml += '</testsuite></testsuites>';",
  'if (report) fs.writeFileSync(report[1], xml);',
  'process.exit(failing.length > 0 ? 1 : 0);',
  '',
].join('\n');

/** A stand-in for Vitest's default reporter, failing the tests failing.txt names. */
const FAKE_VITEST = [
  "const fs = require('fs');",
  "const failing = fs.readFileSync('failing.txt', 'utf8').split(',').map((s) => s.trim()).filter(Boolean);",
  "const names = ['adds', 'multiplies', 'divides'];",
  "if (failing.length > 0) console.log('⎯⎯⎯⎯⎯⎯⎯ Failed Tests ' + failing.length + ' ⎯⎯⎯⎯⎯⎯⎯\\n');",
  "for (const name of failing) console.log(' FAIL  src/math.test.ts > math > ' + name + '\\nAssertionError: expected 1 to be 2');",
  "console.log('      Tests  ' + failing.length + ' failed | ' + (names.length - failing.length) + ' passed (' + names.length + ')');",
  'process.exit(failing.length > 0 ? 1 : 0);',
  '',
].join('\n');

async function runnerRepo(script: string, file: string, failing: string) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: `node ${file}` },
      }),
      '.gitignore': '.firescale/\n',
      [file]: script,
      'failing.txt': `${failing}\n`,
      'tests/test_old.py': 'def test_a():\n    assert True\n',
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  const write = (name: string, content: string) => writeFile(path.join(root, name), content);
  return { root, write };
}

interface FailedTest {
  id: string;
  label?: string;
}

function failedTests(report: Report): FailedTest[] {
  return (step(report, 'test') as Step & { failedTests?: FailedTest[] }).failedTests ?? [];
}

describe('per-test labels', () => {
  it('labels only the new failing test new, and names it, for pytest', async () => {
    const { root, write } = await runnerRepo(FAKE_PYTEST, 'pytest.js', 'test_a');
    await write('failing.txt', 'test_a, test_b\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test').label).toBe('new');
    expect(failedTests(json)).toEqual([
      { id: 'tests/test_old.py::test_a', label: 'pre-existing' },
      { id: 'tests/test_old.py::test_b', label: 'new' },
    ]);
    const human = await runCli(['verify'], root);
    expect(human.stdout).toContain('New failing tests:\n  tests/test_old.py::test_b\n');
    expect(human.stdout).not.toContain('  tests/test_old.py::test_a\n');
    expect(human.stdout).toContain('(1 other already failed there)');
  });

  it('labels only the new failing test new, and names it, for Vitest', async () => {
    const { root, write } = await runnerRepo(FAKE_VITEST, 'vitest.js', 'adds');
    await write('failing.txt', 'adds, divides\n');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test').label).toBe('new');
    expect(failedTests(json)).toEqual([
      { id: 'src/math.test.ts > math > adds', label: 'pre-existing' },
      { id: 'src/math.test.ts > math > divides', label: 'new' },
    ]);
    expect((await runCli(['verify'], root)).stdout).toContain(
      'New failing tests:\n  src/math.test.ts > math > divides\n',
    );
    // Only the old one failing: pre-existing.
    await write('failing.txt', 'adds\n');
    await write('tests/note.txt', 'a change\n');
    const old = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(old, 'test').label).toBe('pre-existing');
    expect(old.result).toBe('pre-existing');
  });

  it('falls back to step labels, with a note, for a runner without per-test results', async () => {
    const plain = [
      "const fs = require('fs');",
      "const failing = fs.readFileSync('failing.txt', 'utf8').trim();",
      "if (failing) console.log('broken: ' + failing);",
      'process.exit(failing ? 1 : 0);',
      '',
    ].join('\n');
    const { root, write } = await runnerRepo(plain, 'check.js', 'one');
    await write('tests/note.txt', 'a change\n');
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(json, 'test').label).toBe('pre-existing');
    expect(failedTests(json)).toEqual([]);
    expect(json.notes?.join('\n')).toContain(
      'test: its runner gives no per-test results, so it is compared as a whole step',
    );
  });

  it('never turns the old failures of a renamed test file into new ones', async () => {
    const { root } = await runnerRepo(FAKE_PYTEST, 'pytest.js', 'test_a');
    expect((await runGit(root, ['mv', 'tests/test_old.py', 'tests/test_new.py'])).code).toBe(0);
    const result = await runCli(['verify', '--json'], root);
    const json = await report(result.stdout);
    // The same test, by git's rename: never new. Its output names the new
    // file, so it is not proven the same failure either: it blocks, saying so.
    expect(failedTests(json)).toEqual([{ id: 'tests/test_new.py::test_a', label: 'pre-existing' }]);
    expect(step(json, 'test').label).toBeUndefined();
    expect(step(json, 'test').base?.note).toContain('the same tests fail on the base commit');
    expect(result.code).toBe(EXIT_VIOLATIONS);
  });

  it('keeps per-test labels when the temp folder has a space in its name', async () => {
    const { root, write } = await runnerRepo(FAKE_PYTEST, 'pytest.js', 'test_a');
    const spaced = await fixtures.create({ files: {}, track: 'no-git' });
    const temp = path.join(spaced.root, 'with space');
    const { mkdir } = await import('node:fs/promises');
    await mkdir(temp, { recursive: true });
    await write('failing.txt', 'test_a, test_b\n');
    const env = { TMPDIR: temp, TEMP: temp, TMP: temp };
    const result = await runCli(['verify', '--json'], root, env);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    expect(step(json, 'test').label).toBe('new');
    expect(failedTests(json)).toEqual([
      { id: 'tests/test_old.py::test_a', label: 'pre-existing' },
      { id: 'tests/test_old.py::test_b', label: 'new' },
    ]);
  });

  it('does not trust a base on which no test passed', async () => {
    const { root, write } = await runnerRepo(FAKE_PYTEST, 'pytest.js', 'test_a, test_b, test_c');
    await write('failing.txt', 'test_a\n');
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(json, 'test').label).toBeUndefined();
    expect(step(json, 'test').base?.note).toContain('no test passed on the base commit');
  });
});

// Small fixes after M21 (ADR 0040).

/**
 * A repo with two Node projects, `a` and `b`, each with its own lockfile and
 * a test that fails on the base. `baseInstall` is committed on the base.
 */
async function twoProjectRepo(
  baseInstall?: string,
  change: string = 'a/package-lock.json',
  extra: Record<string, string> = {},
) {
  const outside = await fixtures.create({ files: {}, track: 'no-git' });
  const log = path.join(outside.root, 'runs.log');
  const project = (name: string) => ({
    [`${name}/package.json`]: JSON.stringify({ name, version: '1.0.0', private: true }),
    [`${name}/package-lock.json`]: '{"lockfileVersion":3}\n',
    [`${name}/test.js`]: stepScript(`${name} test`, 'test.txt', log),
    [`${name}/test.txt`]: 'fail\n',
  });
  const step = { lint: false, typecheck: false, test: 'node test.js' };
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: {
          lint: false,
          typecheck: false,
          test: false,
          projects: { a: step, b: step },
          ...(baseInstall ? { baseInstall } : {}),
        },
      }),
      '.gitignore': '.firescale/\n',
      ...project('a'),
      ...project('b'),
      ...extra,
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  const runs = async (): Promise<string[]> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean);
  const write = (file: string, content: string) => writeFile(path.join(root, file), content);
  // Both projects change, and one dependency list. Both still fail as on the base.
  await write(change, `${(await readFile(path.join(root, change), 'utf8')).trim()} \n`);
  await write('a/test.txt', 'fail, changed\n');
  await write('b/test.txt', 'fail, changed\n');
  return { root, runs };
}

describe("a project's changed lockfile", () => {
  it('leaves only that project without a base, with a note, and judges the others', async () => {
    const { root, runs } = await twoProjectRepo();
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const json = await report(result.stdout);
    const a = step(json, 'test', 'a');
    expect(a.status).toBe('failed');
    expect(a.label).toBeUndefined();
    expect(a.base?.status).toBe('setup-needed');
    expect(json.notes?.join('\n')).toContain(
      "test in a: a/package-lock.json changed, so the head's installed dependencies do not stand for the base's; set verify.baseInstall to a command that installs them in the base worktree, so this failure blocks as it would without a base.",
    );
    const b = step(json, 'test', 'b');
    expect(b.label).toBe('pre-existing');
    expect(b.base?.source).toBe('ran');
    // Only b ran on the base; a's linked dependencies would have been wrong there.
    expect((await runs()).filter((run) => run.startsWith('base '))).toEqual(['base b test']);
    const human = await runCli(['verify'], root);
    expect(human.stdout.split('\n')[0]).toMatch(/^verify: FAILED at test in a/);
  });

  it("does the same for one project's requirements list", async () => {
    const { root, runs } = await twoProjectRepo(undefined, 'b/requirements.txt', {
      'b/requirements.txt': 'requests==2.0.0\n',
    });
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(json, 'test', 'b').label).toBeUndefined();
    expect(step(json, 'test', 'b').base?.status).toBe('setup-needed');
    expect(json.notes?.join('\n')).toContain('test in b: b/requirements.txt changed');
    expect(step(json, 'test', 'a').label).toBe('pre-existing');
    expect((await runs()).filter((run) => run.startsWith('base '))).toEqual(['base a test']);
  });

  it('counts a changed root lockfile for every project below it', async () => {
    const { root, runs } = await twoProjectRepo(undefined, 'package-lock.json', {
      'package-lock.json': '{"lockfileVersion":3}\n',
    });
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    for (const project of ['a', 'b']) {
      expect(step(json, 'test', project).label).toBeUndefined();
      expect(step(json, 'test', project).base?.status).toBe('setup-needed');
    }
    expect(json.notes?.join('\n')).toContain('test in a: package-lock.json changed');
    expect((await runs()).filter((run) => run.startsWith('base '))).toEqual([]);
  });

  it('runs verify.baseInstall and judges that project when it is set', async () => {
    const { root, runs } = await twoProjectRepo('node -e 0');
    const result = await runCli(['verify', '--json'], root);
    expect(result.code).toBe(EXIT_OK);
    const json = await report(result.stdout);
    expect(json.result).toBe('pre-existing');
    expect(step(json, 'test', 'a').label).toBe('pre-existing');
    expect(step(json, 'test', 'a').base?.source).toBe('ran');
    expect(step(json, 'test', 'b').label).toBe('pre-existing');
    expect((await runs()).filter((run) => run.startsWith('base ')).sort()).toEqual([
      'base a test',
      'base b test',
    ]);
  });

  it('counts a changed requirements list in the kept base result', async () => {
    const { root, runs, write } = await blameRepo('ok', 'fail');
    const config = JSON.parse(await readFile(path.join(root, 'firescale.config.json'), 'utf8'));
    config.verify.baseInstall = 'node -e 0';
    await write('firescale.config.json', JSON.stringify(config));
    await write('requirements.txt', 'requests==2.0.0\n');
    await commitAll(root, 'base install and a requirements list');
    expect((await runGit(root, ['branch', '--force', 'main', 'HEAD'])).code).toBe(0);
    await write('lint.txt', 'ok, changed\n');
    await runCli(['verify'], root);
    // Only the failing test runs on the base (ADR 0043).
    const before = (await runs()).filter((run) => run.startsWith('base ')).length;
    expect(before).toBe(1);
    // Same base, other requirements: the kept result is not used.
    await write('requirements.txt', 'requests==2.1.0\n');
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(json, 'test').base?.source).toBe('ran');
    expect((await runs()).filter((run) => run.startsWith('base ')).length).toBe(2);
  });
});

describe('fixed needs a real failure on the base (with verify.labelFixed, ADR 0043)', () => {
  /**
   * test.js passes when test.txt says ok and the ignored build output
   * `build/out.txt` exists: a worktree, which has no ignored files, fails
   * it whatever the code.
   */
  const NEEDS_BUILD = (log: string) =>
    [
      "const fs = require('fs');",
      "const where = process.cwd().includes('firescale-base-') ? 'base' : 'head';",
      `fs.appendFileSync(${JSON.stringify(log)}, where + ' test\\n');`,
      "const built = fs.existsSync('build/out.txt');",
      "if (!built) console.log('Error: build/out.txt is missing; run the build first');",
      "process.exit(built && !fs.readFileSync('test.txt', 'utf8').includes('fail') ? 0 : 1);",
      '',
    ].join('\n');

  it('gives no label, and a note, when the step fails in any worktree', async () => {
    const { root, runs, write } = await blameRepo('ok', 'ok', { labelFixed: true });
    const outside = path.join(root, '..', `${path.basename(root)}-needs-build.log`);
    await write('test.js', NEEDS_BUILD(outside));
    await write('.gitignore', '.firescale/\nbuild/\n');
    await commitAll(root, 'the test needs a build output');
    expect((await runGit(root, ['branch', '--force', 'main', 'HEAD'])).code).toBe(0);
    const { mkdir } = await import('node:fs/promises');
    await mkdir(path.join(root, 'build'), { recursive: true });
    await write('build/out.txt', 'built\n');
    // lint breaks, so the base worktree is needed; test changes but passes.
    await write('lint.txt', 'fail\n');
    await write('test.txt', 'ok, changed\n');
    const log = async () =>
      (await readFile(outside, 'utf8')).split(/\r?\n/).filter((line) => line === 'base test');
    const human = await runCli(['verify'], root);
    expect(human.code).toBe(EXIT_VIOLATIONS);
    expect(human.stdout).not.toContain('Fixed by this change');
    expect(human.stdout).toMatch(
      /test: it fails on the base commit [0-9a-f]{7}, but it fails in a worktree of the head too, so that failure comes from the worktree, not from the code, so it is not labelled fixed\./,
    );
    // The base run and the run in a worktree of the head each ran the test once.
    expect(await log()).toHaveLength(2);
    // A second run keeps no unpaired failure: the test gets no base result,
    // and nothing runs in a worktree.
    const second = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(second, 'test').label).toBeUndefined();
    expect(step(second, 'test').base).toBeUndefined();
    expect(await log()).toHaveLength(2);
    expect((await runs()).filter((run) => run.startsWith('base '))).toEqual(['base lint']);
    // Without the cache both run again, and the JSON says why.
    const json = await report((await runCli(['verify', '--json', '--no-cache'], root)).stdout);
    const test = step(json, 'test');
    expect(test.status).toBe('passed');
    expect(test.base?.status).toBe('failed');
    expect(test.label).toBeUndefined();
    expect(test.base?.note).toMatch(
      /^it fails on the base commit [0-9a-f]{7}, but it fails in a worktree of the head too, so that failure comes from the worktree, not from the code, so it is not labelled fixed$/,
    );
    expect(await log()).toHaveLength(4);
  });

  it('labels the step fixed when it passes in a worktree of the head, and keeps that pairing', async () => {
    const { root, runs, write } = await blameRepo('ok', 'fail', { labelFixed: true });
    await write('lint.txt', 'fail\n');
    await write('test.txt', 'ok\n');
    const json = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(json, 'test').label).toBe('fixed');
    expect(step(json, 'test').base?.note).toBeUndefined();
    // On the base, then in a worktree of the head.
    const base = async () => (await runs()).filter((run) => run === 'base test');
    expect(await base()).toHaveLength(2);
    const again = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(again, 'test').label).toBe('fixed');
    expect(step(again, 'test').base?.source).toBe('cache');
    expect(await base()).toHaveLength(2);
  });

  it('does not pair a kept base failure with a later run of the head', async () => {
    const { root, write } = await blameRepo('ok', 'ok', { labelFixed: true });
    // The test also needs an installed dependency, which is missing at first.
    const needsDep = [
      "const fs = require('fs');",
      "const ok = fs.existsSync('node_modules/dep/ok') && !fs.readFileSync('test.txt', 'utf8').includes('fail');",
      'process.exit(ok ? 0 : 1);',
      '',
    ].join('\n');
    await write('test.js', needsDep);
    await write('.gitignore', '.firescale/\nnode_modules/\n');
    await commitAll(root, 'the test needs a dependency');
    expect((await runGit(root, ['branch', '--force', 'main', 'HEAD'])).code).toBe(0);
    const { mkdir } = await import('node:fs/promises');
    await mkdir(path.join(root, 'node_modules', 'dep'), { recursive: true });
    await write('lint.txt', 'fail\n');
    await write('test.txt', 'ok, changed\n');
    const first = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(first, 'test').label).toBe('pre-existing');
    // The install is finished; the lockfile does not change.
    await write('node_modules/dep/ok', '');
    const second = await report((await runCli(['verify', '--json'], root)).stdout);
    expect(step(second, 'test').status).toBe('passed');
    expect(step(second, 'test').label).toBeUndefined();
    // The kept failure was not used: no worktree was needed, so no base result.
    expect(step(second, 'test').base).toBeUndefined();
    // When the base does run again, it passes now: the old failure was the install's.
    const fresh = await report((await runCli(['verify', '--json', '--no-cache'], root)).stdout);
    expect(step(fresh, 'test').label).toBeUndefined();
    expect(step(fresh, 'test').base).toMatchObject({ status: 'passed', source: 'ran' });
  });
});
