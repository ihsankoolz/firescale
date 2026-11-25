// Old tests, new code (ADR 0089): when the tamper guard finds a changed
// expected value or a weakened assertion in a test file the base commit has,
// the test step runs once more in a worktree of the head with the base's
// version of that file. A test of it that fails there is evidence on the
// finding. Report only: it never adds, lowers or removes a finding, and
// never makes verify fail. The stand-in `vitest` reads the test file it is
// given and fails when it still expects the base's value.
import { chmod, mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, runCli } from './test-helpers.js';

/** The stop hook's exit code when it sends Claude Code back. */
const EXIT_BLOCK_HOOK = 2;

const fixtures = fixtureSet();
afterEach(() => fixtures.cleanup());

/** How the stand-in answers: `honest` reads the file; the others ignore it. */
type Mode = 'honest' | 'always-pass' | 'cannot-collect' | 'exit-zero' | 'other-file';

async function writeVitest(dir: string, mode: Mode): Promise<void> {
  await mkdir(dir, { recursive: true });
  const script = [
    "const fs = require('node:fs');",
    "const path = require('node:path');",
    'const args = process.argv.slice(2);',
    "if (args[0] === '--version') { console.log('vitest 1.0.0'); process.exit(0); }",
    "fs.appendFileSync(path.join(__dirname, 'calls.log'), process.cwd() + '\\n');",
    `const mode = ${JSON.stringify(mode)};`,
    "if (mode === 'cannot-collect') { console.error('SyntaxError: the module has no export named total'); process.exit(1); }",
    "const text = fs.readFileSync(path.join(process.cwd(), 'src', 'cart.test.ts'), 'utf8');",
    "const fails = mode === 'honest' && text.includes('toBe(10)');",
    "const exitZero = mode === 'exit-zero' && text.includes('toBe(10)');",
    'if (fails || exitZero) {',
    "  process.stdout.write('⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯\\n\\n FAIL  src/cart.test.ts > cart > adds the items\\nAssertionError: expected 11 to be 10\\n Test Files  1 failed (1)\\n      Tests  1 failed | 1 passed (2)\\n');",
    '  process.exit(exitZero ? 0 : 1);',
    '}',
    // Vitest names each file it ran: ` ✓ src/cart.test.ts (2 tests) 3ms`.
    "const ran = mode === 'other-file' ? 'src/other.test.ts' : 'src/cart.test.ts';",
    "process.stdout.write(' ✓ ' + ran + ' (2 tests) 3ms\\n Test Files  1 passed (1)\\n      Tests  2 passed (2)\\n');",
    'process.exit(0);',
    '',
  ].join('\n');
  await writeFile(path.join(dir, 'tool.cjs'), script);
  await writeFile(path.join(dir, 'calls.log'), '');
  if (process.platform === 'win32') {
    await writeFile(path.join(dir, 'vitest.cmd'), `@"${process.execPath}" "%~dp0tool.cjs" %*\r\n`);
  } else {
    const file = path.join(dir, 'vitest');
    await writeFile(
      file,
      `#!/bin/sh\nexec "${process.execPath}" "$(dirname "$0")/tool.cjs" "$@"\n`,
    );
    await chmod(file, 0o755);
  }
}

const cartTest = (expected: number, extra = ''): string =>
  `import { describe, expect, it } from 'vitest';\nimport { total } from './cart';\n\ndescribe('cart', () => {\n  it('adds the items', () => {\n    expect(total([4, 6])).toBe(${String(expected)});\n  });\n${extra}});\n`;

/**
 * A repo on `feature` off `main`. The base's test expects 10; the head
 * changes the code to give 11 and the test to expect 11: `expected-changed`.
 */
async function repoWith(
  mode: Mode,
  options: {
    lint?: string;
    head?: Record<string, string>;
    verify?: Record<string, unknown>;
    base?: Record<string, string>;
  } = {},
) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: {
          lint: options.lint ?? false,
          typecheck: false,
          test: 'vitest run',
          ...options.verify,
        },
      }),
      '.gitignore': '.firescale/\n',
      'src/cart.ts':
        'export const total = (items: number[]): number => items.reduce((a, b) => a + b, 0);\n',
      'src/cart.test.ts': cartTest(10),
      ...options.base,
      // Fails on the head's code only; with a file `slow-base-lint`, takes 3 s on
      // the base commit.
      'lint.cjs':
        "const fs = require('node:fs');\nif (process.env.FSCALE_BASE_RUN === '1' && fs.existsSync('slow-base-lint')) { const end = Date.now() + 3000; while (Date.now() < end) {} }\nprocess.exit(fs.readFileSync('src/cart.ts', 'utf8').includes('b, 1)') ? 1 : 0);\n",
    },
    track: 'no-git',
  });
  expect((await runGit(repo.root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(repo.root, 'base');
  expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  const head = options.head ?? {
    'src/cart.ts':
      'export const total = (items: number[]): number => items.reduce((a, b) => a + b, 1);\n',
    'src/cart.test.ts': cartTest(11),
  };
  for (const [file, text] of Object.entries(head)) {
    await writeFile(path.join(repo.root, file), text);
  }
  const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
  await writeVitest(tools, mode);
  const current = process.env['PATH'] ?? process.env['Path'] ?? '';
  const env = {
    PATH: `${tools}${path.delimiter}${current}`,
    CI: undefined,
    GITHUB_ACTIONS: undefined,
  };
  const calls = async () =>
    (await readFile(path.join(tools, 'calls.log'), 'utf8'))
      .split('\n')
      .filter((line) => line !== '');
  return { repo, env, calls };
}

interface Finding {
  kind: string;
  file: string;
  evidence?: string[];
}
interface OldTests {
  step: string;
  status: string;
  files: string[];
  detail: string;
}
interface Report {
  ok: boolean;
  tamper?: { findings: Finding[]; oldTests?: OldTests[] };
}

describe('old tests, new code (ADR 0089)', () => {
  it('adds the failing test of the base’s version to the finding, and does not fail verify', async () => {
    const { repo, env, calls } = await repoWith('honest');
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    const report = JSON.parse(result.stdout) as Report;
    expect(report.ok).toBe(true);
    const finding = report.tamper?.findings.find((f) => f.kind === 'expected-changed');
    expect(finding?.file).toBe('src/cart.test.ts');
    expect(finding?.evidence).toEqual([
      "the base's version fails on the head's code: src/cart.test.ts > cart > adds the items: AssertionError: expected 11 to be 10",
    ]);
    expect(report.tamper?.oldTests).toEqual([
      {
        step: 'test',
        status: 'failed',
        files: ['src/cart.test.ts'],
        detail:
          "1 test of the base's versions of these files fails on the head's code: the change needed different tests to pass",
      },
    ]);
    // Once on the head, once in the worktree of the head with the base's file.
    expect(await calls()).toHaveLength(2);
    const human = await runCli(['verify', '--no-cache'], repo.root, env);
    expect(human.stdout).toContain(
      "    evidence: the base's version fails on the head's code: src/cart.test.ts > cart > adds the items: AssertionError: expected 11 to be 10",
    );
    expect(human.stdout).toContain(
      '  Old tests, new code (test; src/cart.test.ts; report only): failed: 1 test',
    );
  });

  it('never lowers a finding when the base’s version passes', async () => {
    const { repo, env } = await repoWith('always-pass');
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    const report = JSON.parse(result.stdout) as Report;
    const finding = report.tamper?.findings.find((f) => f.kind === 'expected-changed');
    expect(finding).toBeDefined();
    expect(finding?.evidence).toBeUndefined();
    // Never shown as reassurance (ADR 0090): the JSON says it ran and
    // found no failure; the normal output says nothing of it.
    expect(report.tamper?.oldTests?.map((run) => run.status)).toEqual(['ran-no-failure']);
    const human = await runCli(['verify', '--no-cache'], repo.root, env);
    expect(human.stdout).not.toContain('Old tests, new code');
    expect(human.stdout).not.toContain('no test of these files failed');
  });

  it('says "failed", not "passed", when the step exits 0 while the base’s test fails (review)', async () => {
    const { repo, env } = await repoWith('exit-zero');
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    const report = JSON.parse(result.stdout) as Report;
    expect(report.tamper?.oldTests?.map((run) => run.status)).toEqual(['failed']);
    expect(
      report.tamper?.findings.find((f) => f.kind === 'expected-changed')?.evidence,
    ).toHaveLength(1);
  });

  it('never runs in a pull_request_target run (ADR 0045)', async () => {
    const { repo, env, calls } = await repoWith('honest');
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, {
      ...env,
      GITHUB_EVENT_NAME: 'pull_request_target',
    });
    const report = JSON.parse(result.stdout) as Report;
    expect(report.tamper?.findings.find((f) => f.kind === 'expected-changed')).toBeDefined();
    expect(report.tamper?.oldTests).toBeUndefined();
    // Only the head's own run: nothing ran in a worktree.
    for (const cwd of await calls()) {
      expect(await realpath(cwd)).toBe(await realpath(repo.root));
    }
  });

  it('says "not judged" when the base’s tests cannot be collected on the head’s code', async () => {
    const { repo, env } = await repoWith('cannot-collect');
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    const report = JSON.parse(result.stdout) as Report;
    const run = report.tamper?.oldTests?.[0];
    expect(run?.status).toBe('not-judged');
    expect(run?.detail).toContain('could not be read');
    expect(report.tamper?.findings.find((f) => f.kind === 'expected-changed')).toBeDefined();
  });

  it('says "not judged" when the run collected none of the base’s tests (ADR 0090)', async () => {
    const { repo, env } = await repoWith('other-file');
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    const report = JSON.parse(result.stdout) as Report;
    const run = report.tamper?.oldTests?.[0];
    expect(run?.status).toBe('not-judged');
    expect(run?.detail).toContain('collected none of the base');
  });

  it('shares verify.baseBudget with the base run, and says "not judged" when it has run out (ADR 0090)', async () => {
    const { repo, env } = await repoWith('honest', {
      lint: 'node lint.cjs',
      verify: { baseBudget: '2s' },
      // Lint takes 3 s on the base commit: the base run uses up the budget.
      base: { 'slow-base-lint': '' },
    });
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    const report = JSON.parse(result.stdout) as Report;
    const run = report.tamper?.oldTests?.[0];
    expect(run?.status).toBe('not-judged');
    expect(run?.detail).toContain('verify.baseBudget');
  });

  it('runs nothing when the guard has no finding it applies to', async () => {
    const { repo, env } = await repoWith('honest', {
      head: {
        'src/cart.test.ts': cartTest(
          10,
          "  it('also adds', () => {\n    expect(total([])).toBe(0);\n  });\n",
        ),
      },
    });
    const result = await runCli(['verify', '--json', '--no-cache'], repo.root, env);
    const report = JSON.parse(result.stdout) as Report;
    expect(report.tamper?.oldTests).toBeUndefined();
  });

  /** Installs Claude Code's stop hook with this limit, and runs it once. */
  async function stopHook(root: string, env: Record<string, string | undefined>, seconds: number) {
    await mkdir(path.join(root, '.claude'), { recursive: true });
    await runCli(
      ['hooks', 'install', '--agent', 'claude', '--timeout', String(seconds)],
      root,
      env,
    );
    expect(await readFile(path.join(root, '.claude', 'settings.json'), 'utf8')).toContain(
      `"timeout": ${String(seconds)}`,
    );
    return runCli(['hooks', 'run', '--agent', 'claude'], root, env, {
      input: JSON.stringify({ session_id: 's', stop_hook_active: false }),
    });
  }

  it('in the stop hook, says "not judged" when no time is kept for the run, however long the limit (ADR 0090)', async () => {
    const { repo, env, calls } = await repoWith('honest', { lint: 'node lint.cjs' });
    const hook = await stopHook(repo.root, env, 3600);
    expect(hook.code).toBe(EXIT_BLOCK_HOOK);
    expect(hook.stderr).toContain(
      '  Old tests, new code (test; src/cart.test.ts; report only): not judged: no time is kept yet for this run',
    );
    // Nothing started in a worktree.
    expect(await calls()).toHaveLength(1);
  });

  it('in the stop hook, runs once the worktree’s and the step’s times are kept, and keeps the worktree’s time (ADR 0090)', async () => {
    const { repo, env } = await repoWith('honest', { lint: 'node lint.cjs' });
    // A verify keeps the step's time, and the worktree's making and removal.
    await runCli(['verify', '--no-cache'], repo.root, env);
    const kept = await readFile(path.join(repo.root, '.firescale', 'step-times.json'), 'utf8');
    expect(kept).toContain('old tests: worktree');
    const hook = await stopHook(repo.root, env, 3600);
    expect(hook.code).toBe(EXIT_BLOCK_HOOK);
    expect(hook.stderr).toContain(
      "    evidence: the base's version fails on the head's code: src/cart.test.ts > cart > adds the items",
    );
  });

  it('in the stop hook, says "not judged" instead of going past the hook’s time limit', async () => {
    // Lint fails on the head's code only, so the hook blocks and its
    // message shows the guard.
    const { repo, env, calls } = await repoWith('honest', { lint: 'node lint.cjs' });
    await mkdir(path.join(repo.root, '.claude'), { recursive: true });
    const install = await runCli(
      ['hooks', 'install', '--agent', 'claude', '--timeout', '30'],
      repo.root,
      env,
    );
    // It may exit 1 here (npx cannot start an unpublished version); the
    // file is written either way.
    expect(install.stdout).toContain("Claude Code's stop hook may run for 30s");
    expect(await readFile(path.join(repo.root, '.claude', 'settings.json'), 'utf8')).toContain(
      '"timeout": 30',
    );
    const hook = await runCli(['hooks', 'run', '--agent', 'claude'], repo.root, env, {
      input: JSON.stringify({ session_id: 's', stop_hook_active: false }),
    });
    expect(hook.code).toBe(EXIT_BLOCK_HOOK);
    expect(hook.stderr).toContain(
      "  Old tests, new code (test; src/cart.test.ts; report only): not judged: no time was left for it before the stop hook's limit.",
    );
    // Only the head's own run: nothing started past the limit.
    expect(await calls()).toHaveLength(1);
  });
});
