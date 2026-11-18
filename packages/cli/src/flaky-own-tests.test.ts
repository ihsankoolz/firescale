// A flaky pass never excuses the change's own tests (ADR 0073): a failing
// test in a test file the change added or edited stays `new`, with a note,
// and is not run again. A failing test in a file the change did not touch
// is still run again and labelled `flaky` when it then passes.
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(() => fixtures.cleanup());

interface Answer {
  out?: string;
  err?: string;
  junit?: string;
}

/**
 * Writes a stand-in for `tool` into `dir`. Its first run on the head fails
 * with `failing`; every later run, and every run on the base commit, passes
 * with `passing`. Each run is logged to `calls.log` as `head` or `base`.
 */
async function writeFlakyTool(
  dir: string,
  tool: string,
  failing: Answer,
  passing: Answer,
): Promise<void> {
  await mkdir(dir, { recursive: true });
  const script = [
    "const fs = require('node:fs');",
    "const path = require('node:path');",
    'const args = process.argv.slice(2);',
    `if (args[0] === '--version') { console.log('${tool} 1.0.0'); process.exit(0); }`,
    "const base = process.env.FSCALE_BASE_RUN === '1';",
    "fs.appendFileSync(path.join(__dirname, 'calls.log'), (base ? 'base' : 'head') + '\\n');",
    "const marker = path.join(__dirname, 'ran-once');",
    'const first = !base && !fs.existsSync(marker);',
    "if (first) fs.writeFileSync(marker, 'x');",
    "const answer = JSON.parse(fs.readFileSync(path.join(__dirname, 'answers.json'), 'utf8'))[first ? 'failing' : 'passing'];",
    "const words = [...(process.env.PYTEST_ADDOPTS || '').matchAll(/'([^']*)'|(\\S+)/g)].map((m) => m[1] ?? m[2]);",
    "const report = /^--junitxml=(.+)$/.exec(words.find((w) => w.startsWith('--junitxml=')) || '');",
    'if (report && answer.junit) fs.writeFileSync(report[1], answer.junit);',
    'if (answer.out) process.stdout.write(answer.out);',
    'if (answer.err) process.stderr.write(answer.err);',
    'process.exit(first ? 1 : 0);',
    '',
  ].join('\n');
  await writeFile(path.join(dir, 'tool.cjs'), script);
  await writeFile(path.join(dir, 'answers.json'), JSON.stringify({ failing, passing }));
  await writeFile(path.join(dir, 'calls.log'), '');
  if (process.platform === 'win32') {
    await writeFile(path.join(dir, `${tool}.cmd`), `@"${process.execPath}" "%~dp0tool.cjs" %*\r\n`);
  } else {
    const file = path.join(dir, tool);
    await writeFile(
      file,
      `#!/bin/sh\nexec "${process.execPath}" "$(dirname "$0")/tool.cjs" "$@"\n`,
    );
    await chmod(file, 0o755);
  }
}

interface Step {
  name: string;
  status: string;
  label?: string;
  failedTests?: { id: string; label?: string }[];
}

interface Report {
  ok: boolean;
  result: string;
  steps: Step[];
  notes?: string[];
}

const junit = (failure: string): string =>
  `<?xml version="1.0"?><testsuites><testsuite name="pytest">` +
  `<testcase classname="tests.test_pay" name="test_charges" file="tests/test_pay.py">${failure}</testcase>` +
  `<testcase classname="tests.test_pay" name="test_refunds" file="tests/test_pay.py"></testcase>` +
  `</testsuite></testsuites>`;

const RUST_LIB = [
  'pub fn charge() -> u32 {',
  '    1',
  '}',
  '',
  '#[cfg(test)]',
  'mod tests {',
  '    #[test]',
  '    fn charges() {',
  '        assert_eq!(super::charge(), 1);',
  '    }',
  '}',
  '',
].join('\n');

const RUST_OTHER = [
  'pub fn round(value: u32) -> u32 {',
  '    value',
  '}',
  '',
  '#[cfg(test)]',
  'mod tests {',
  '    #[test]',
  '    fn rounds() {',
  '        assert_eq!(super::round(2), 2);',
  '    }',
  '}',
  '',
].join('\n');

interface Runner {
  name: string;
  tool: string;
  command: string;
  failing: Answer;
  passing: Answer;
  /** The identity fscale gives the failing test. */
  id: string;
  /** The file that holds the failing test, and the repo's other files. */
  testFile: string;
  files: Record<string, string>;
  /** A file of the repo that does not hold the failing test. */
  otherFile: string;
}

const RUNNERS: Runner[] = [
  {
    name: 'pytest',
    tool: 'pytest',
    command: 'pytest',
    failing: {
      out: '===== test session starts =====\nFAILED tests/test_pay.py::test_charges - assert 3 == 4\n',
      junit: junit('<failure message="assert 3 == 4">trace</failure>'),
    },
    passing: { out: '===== test session starts =====\n2 passed\n', junit: junit('') },
    id: 'tests/test_pay.py::test_charges',
    testFile: 'tests/test_pay.py',
    files: {
      'tests/test_pay.py':
        'def test_charges():\n    assert 3 == 3\n\n\ndef test_refunds():\n    assert True\n',
      'pay.py': 'def charge():\n    return 3\n',
    },
    otherFile: 'pay.py',
  },
  {
    name: 'Vitest',
    tool: 'vitest',
    command: 'vitest run',
    failing: {
      out:
        '⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯\n\n FAIL  src/pay.test.ts > pay > charges\nAssertionError: expected 3 to be 4\n' +
        ' Test Files  1 failed (1)\n      Tests  1 failed | 1 passed (2)\n',
    },
    passing: { out: ' Test Files  1 passed (1)\n      Tests  2 passed (2)\n' },
    id: 'src/pay.test.ts > pay > charges',
    testFile: 'src/pay.test.ts',
    files: {
      'src/pay.test.ts':
        "import { expect, it } from 'vitest';\nit('charges', () => expect(3).toBe(3));\n",
      'src/pay.ts': 'export const charge = () => 3;\n',
    },
    otherFile: 'src/pay.ts',
  },
  {
    name: 'Jest',
    tool: 'jest',
    command: 'jest',
    failing: {
      out:
        'FAIL src/pay.test.js\n  ● pay › charges\n\n    expect(received).toBe(expected) // Object.is equality\n\n    Expected: 4\n    Received: 3\n\n      at Object.toBe (src/pay.test.js:4:17)\n\n' +
        'Tests:       1 failed, 1 passed, 2 total\n',
    },
    passing: { out: 'PASS src/pay.test.js\nTests:       2 passed, 2 total\n' },
    id: 'src/pay.test.js › pay › charges',
    testFile: 'src/pay.test.js',
    files: {
      'src/pay.test.js': "test('charges', () => expect(3).toBe(3));\n",
      'src/pay.js': 'module.exports.charge = () => 3;\n',
    },
    otherFile: 'src/pay.js',
  },
  {
    name: 'cargo test',
    tool: 'cargo',
    command: 'cargo test',
    failing: {
      out:
        '\nrunning 2 tests\ntest tests::charges ... FAILED\ntest money::tests::rounds ... ok\n\nfailures:\n\n' +
        "---- tests::charges stdout ----\n\nthread 'tests::charges' panicked at src/lib.rs:9:9:\nassertion failed: 1 == 2\n\n" +
        '\nfailures:\n    tests::charges\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n',
      err: '     Running unittests src/lib.rs (target/debug/deps/pay-1a2b3c4d5e6f7a8b)\n',
    },
    passing: {
      out: '\nrunning 2 tests\ntest tests::charges ... ok\ntest money::tests::rounds ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n',
      err: '     Running unittests src/lib.rs (target/debug/deps/pay-1a2b3c4d5e6f7a8b)\n',
    },
    id: 'pay::tests::charges',
    testFile: 'src/lib.rs',
    files: { 'src/lib.rs': RUST_LIB, 'src/money.rs': RUST_OTHER },
    // Another Rust file with a test of its own: only the file that holds
    // the failing test counts, not any Rust file.
    otherFile: 'src/money.rs',
  },
];

/** A repo on a `feature` branch off `main`, whose only step is the runner's command. */
async function repoWith(runner: Runner, files: Record<string, string> = runner.files) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: runner.command },
      }),
      '.gitignore': '.firescale/\n',
      ...files,
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  const tools = path.join(root, '..', `${path.basename(root)}-tools`);
  await writeFlakyTool(tools, runner.tool, runner.failing, runner.passing);
  const current = process.env['PATH'] ?? process.env['Path'] ?? '';
  const env = {
    PATH: `${tools}${path.delimiter}${current}`,
    CI: undefined,
    GITHUB_ACTIONS: undefined,
  };
  const verify = (...flags: string[]) => runCli(['verify', ...flags], root, env);
  const append = async (file: string, text: string) => {
    const full = path.join(root, ...file.split('/'));
    const before = await readFile(full, 'utf8').catch(() => '');
    await mkdir(path.dirname(full), { recursive: true });
    await writeFile(full, before + text);
  };
  const calls = async () =>
    (await readFile(path.join(tools, 'calls.log'), 'utf8')).split('\n').filter(Boolean);
  return { root, verify, append, calls };
}

const testStep = (stdout: string): { report: Report; step: Step } => {
  const report = JSON.parse(stdout) as Report;
  const step = report.steps.find((one) => one.name === 'test');
  if (!step) throw new Error(`no test step in ${stdout}`);
  return { report, step };
};

const comment = (runner: Runner): string =>
  runner.testFile.endsWith('.py') ? '# edited\n' : '// edited\n';

describe('a failing test that passes when run again', () => {
  for (const runner of RUNNERS) {
    it(`stays new for ${runner.name} when the change edited its file, with a note`, async () => {
      const { verify, append, calls } = await repoWith(runner);
      await append(runner.testFile, comment(runner));

      const result = await verify('--json');
      expect(result.code).toBe(EXIT_VIOLATIONS);
      const { report, step } = testStep(result.stdout);
      expect(step.status).toBe('failed');
      expect(step.label).toBe('new');
      expect(step.failedTests).toEqual([{ id: runner.id, label: 'new' }]);
      expect(report.ok).toBe(false);
      expect(report.result).toBe('failed');
      expect(report.notes?.join('\n')).toContain(
        `test: not run again, and it stays new: 1 failing test is in a file this change added or edited (${runner.testFile}: ${runner.id}). A failing test of the change itself is never labelled flaky (ADR 0073).`,
      );
      // Once on the head, once on the base commit; no second run on the head.
      expect(await calls()).toEqual(['head', 'base']);
    });

    it(`is still flaky for ${runner.name} when the change did not touch its file`, async () => {
      const { verify, append, calls } = await repoWith(runner);
      await append(runner.otherFile, comment(runner));

      const result = await verify('--json');
      expect(result.code).toBe(EXIT_OK);
      const { report, step } = testStep(result.stdout);
      expect(step.status).toBe('failed');
      expect(step.label).toBe('flaky');
      expect(step.failedTests).toEqual([{ id: runner.id, label: 'flaky' }]);
      expect(report.result).toBe('pre-existing');
      expect(report.notes?.join('\n') ?? '').not.toContain('never labelled flaky');
      expect(await calls()).toEqual(['head', 'base', 'head']);
    });
  }

  it('stays new when the change added the test file', async () => {
    const [, vitestRunner] = RUNNERS;
    if (!vitestRunner) throw new Error('no Vitest runner');
    // The base commit has no test file; the change adds it, uncommitted.
    const { verify, append, calls } = await repoWith(vitestRunner, {
      'src/pay.ts': 'export const charge = () => 3;\n',
    });
    await append(vitestRunner.testFile, vitestRunner.files[vitestRunner.testFile] ?? '');

    const result = await verify();
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout.split('\n')[0]).toBe('verify: FAILED at test (new)');
    expect(result.stdout).toContain(
      'test: not run again, and it stays new: 1 failing test is in a file this change added or edited (src/pay.test.ts: src/pay.test.ts > pay > charges).',
    );
    expect(result.stdout).not.toContain('Flaky:');
    expect(await calls()).toEqual(['head', 'base']);
  });
});
