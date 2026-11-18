// A test command that exits 0 while its own per-test results hold failures
// or errors is FAILED, and is never cached (ADR 0072). That is what a script
// ending in `|| exit 0` does, and a pytest hook that sets the exit status to 0.
// The stand-ins print what the real runners print for a failing run.
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(() => fixtures.cleanup());

/** What a stand-in prints, and the JUnit report it writes when pytest is asked for one. */
interface Answer {
  out?: string;
  err?: string;
  junit?: string;
}

/**
 * Writes a stand-in for `tool` into `dir`. It always exits 0. Each call that
 * is not a version probe is appended to `calls.log` there.
 */
async function writeTool(dir: string, tool: string, answer: Answer): Promise<void> {
  await mkdir(dir, { recursive: true });
  const script = [
    "const fs = require('node:fs');",
    "const path = require('node:path');",
    'const args = process.argv.slice(2);',
    `if (args[0] === '--version') { console.log('${tool} 1.0.0'); process.exit(0); }`,
    `fs.appendFileSync(path.join(__dirname, 'calls.log'), ['${tool}', ...args].join(' ') + '\\n');`,
    "const answer = JSON.parse(fs.readFileSync(path.join(__dirname, 'answer.json'), 'utf8'));",
    "const words = [...(process.env.PYTEST_ADDOPTS || '').matchAll(/'([^']*)'|(\\S+)/g)].map((m) => m[1] ?? m[2]);",
    "const report = /^--junitxml=(.+)$/.exec(words.find((w) => w.startsWith('--junitxml=')) || '');",
    'if (report && answer.junit) fs.writeFileSync(report[1], answer.junit);',
    'if (answer.out) process.stdout.write(answer.out);',
    'if (answer.err) process.stderr.write(answer.err);',
    'process.exit(0);',
    '',
  ].join('\n');
  await writeFile(path.join(dir, 'tool.cjs'), script);
  await writeFile(path.join(dir, 'answer.json'), JSON.stringify(answer));
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

async function calls(dir: string): Promise<string[]> {
  return (await readFile(path.join(dir, 'calls.log'), 'utf8')).split('\n').filter(Boolean);
}

interface Step {
  name: string;
  status: string;
  detail: string;
  exitCode?: number;
  cache?: object;
  failedTests?: { id: string }[];
}

interface Report {
  ok: boolean;
  result: string;
  steps: Step[];
}

const testcase = (name: string, inner: string): string =>
  `<testcase classname="tests.test_pay" name="${name}" file="tests/test_pay.py">${inner}</testcase>`;

const junit = (cases: string[]): string =>
  `<?xml version="1.0"?><testsuites><testsuite name="pytest">${cases.join('')}</testsuite></testsuites>`;

const vitest = (failing: string[]): string =>
  (failing.length > 0 ? `⎯⎯⎯⎯⎯⎯⎯ Failed Tests ${failing.length} ⎯⎯⎯⎯⎯⎯⎯\n\n` : '') +
  failing
    .map((name) => ` FAIL  src/pay.test.ts > pay > ${name}\nAssertionError: expected 1 to be 2\n`)
    .join('') +
  ` Test Files  1 ${failing.length > 0 ? 'failed' : 'passed'} (1)\n` +
  `      Tests  ${failing.length > 0 ? `${failing.length} failed | ` : ''}${3 - failing.length} passed (3)\n`;

const jest = (failing: string[]): string =>
  `${failing.length > 0 ? 'FAIL' : 'PASS'} src/pay.test.js\n` +
  failing
    .map(
      (name) =>
        `  ● pay › ${name}\n\n    expect(received).toBe(expected) // Object.is equality\n\n    Expected: 4\n    Received: 3\n\n      at Object.toBe (src/pay.test.js:4:17)\n\n`,
    )
    .join('') +
  `Tests:       ${failing.length > 0 ? `${failing.length} failed, ` : ''}${3 - failing.length} passed, 3 total\n`;

const cargo = (failing: string[]): string =>
  `\nrunning 3 tests\n` +
  ['charges', 'refunds', 'rounds']
    .map((name) => `test tests::${name} ... ${failing.includes(name) ? 'FAILED' : 'ok'}\n`)
    .join('') +
  (failing.length > 0
    ? `\nfailures:\n\n` +
      failing
        .map(
          (name) =>
            `---- tests::${name} stdout ----\n\nthread 'tests::${name}' panicked at src/lib.rs:10:5:\nassertion failed: 1 == 2\n\n`,
        )
        .join('') +
      `\nfailures:\n${failing.map((name) => `    tests::${name}\n`).join('')}\n`
    : '\n') +
  `test result: ${failing.length > 0 ? 'FAILED' : 'ok'}. ${3 - failing.length} passed; ${failing.length} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n`;

const CARGO_ERR = '     Running unittests src/lib.rs (target/debug/deps/pay-1a2b3c4d5e6f7a8b)\n';

interface Runner {
  /** The runner's name, as the test titles say it. */
  name: string;
  tool: string;
  command: string;
  /** What the stand-in answers when the tests named fail (and one more errors, for pytest). */
  answer: (failing: string[]) => Answer;
  /** The identities fscale gives the failing tests `charges` and `refunds`. */
  ids: string[];
}

const RUNNERS: Runner[] = [
  {
    name: 'pytest',
    tool: 'pytest',
    command: 'pytest',
    answer: (failing) => ({
      out: '============================= test session starts ==============================\n',
      junit: junit(
        ['charges', 'refunds', 'rounds'].map((name, index) =>
          testcase(
            `test_${name}`,
            !failing.includes(name)
              ? ''
              : index === 0
                ? '<failure message="assert 3 == 4">trace</failure>'
                : '<error message="fixture db not found">trace</error>',
          ),
        ),
      ),
    }),
    ids: ['tests/test_pay.py::test_charges', 'tests/test_pay.py::test_refunds'],
  },
  {
    name: 'Vitest',
    tool: 'vitest',
    command: 'vitest run',
    answer: (failing) => ({ out: vitest(failing) }),
    ids: ['src/pay.test.ts > pay > charges', 'src/pay.test.ts > pay > refunds'],
  },
  {
    name: 'Jest',
    tool: 'jest',
    command: 'jest',
    answer: (failing) => ({ out: jest(failing) }),
    ids: ['src/pay.test.js › pay › charges', 'src/pay.test.js › pay › refunds'],
  },
  {
    name: 'cargo test',
    tool: 'cargo',
    command: 'cargo test',
    answer: (failing) => ({ out: cargo(failing), err: CARGO_ERR }),
    ids: ['pay::tests::charges', 'pay::tests::refunds'],
  },
];

/** A repo whose only step is `command`, and a folder of stand-ins beside it. */
async function repoWith(runner: Runner, failing: string[]) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: runner.command },
      }),
      '.gitignore': '.firescale/\n',
    },
  });
  const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
  await writeTool(tools, runner.tool, runner.answer(failing));
  const current = process.env['PATH'] ?? process.env['Path'] ?? '';
  const env = {
    PATH: `${tools}${path.delimiter}${current}`,
    CI: undefined,
    GITHUB_ACTIONS: undefined,
    FSCALE_LOG: 'cache',
  };
  const verify = (...flags: string[]) => runCli(['verify', ...flags], repo.root, env);
  return { repo, tools, verify };
}

const testStep = (stdout: string): Step => {
  const report = JSON.parse(stdout) as Report;
  const step = report.steps.find((one) => one.name === 'test');
  if (!step) throw new Error(`no test step in ${stdout}`);
  return step;
};

describe('a test command that exits 0 while its own results hold failing tests', () => {
  for (const runner of RUNNERS) {
    it(`is FAILED for ${runner.name}, names the tests, and is never cached`, async () => {
      const { tools, verify } = await repoWith(runner, ['charges', 'refunds']);

      const first = await verify();
      expect(first.code).toBe(EXIT_VIOLATIONS);
      expect(first.stdout.split('\n')[0]).toBe('verify: FAILED at test');
      expect(first.stdout).toContain(
        `${runner.command} (exit 0): the command reported success but 2 tests failed`,
      );
      for (const id of runner.ids) expect(first.stdout).toContain(id);
      // Nothing was kept for the cache.
      expect(first.stderr).not.toContain('cache: stored');

      const second = await verify('--json');
      expect(second.code).toBe(EXIT_VIOLATIONS);
      const report = JSON.parse(second.stdout) as Report;
      expect(report.ok).toBe(false);
      expect(report.result).toBe('failed');
      const step = testStep(second.stdout);
      expect(step).toMatchObject({
        status: 'failed',
        exitCode: 0,
        detail: `${runner.command} (exit 0): the command reported success but 2 tests failed`,
      });
      expect(step.cache).toBeUndefined();
      expect((step.failedTests ?? []).map((test) => test.id).sort()).toEqual(runner.ids);
      // The command ran both times: the first run's result was not reused.
      expect(await calls(tools)).toHaveLength(2);
    });

    it(`still passes and caches ${runner.name} when it exits 0 with no failing test`, async () => {
      const { tools, verify } = await repoWith(runner, []);

      const first = await verify();
      expect(first.code).toBe(EXIT_OK);
      expect(first.stdout.split('\n')[0]).toBe('verify: OK');
      expect(first.stdout).not.toContain('reported success');

      const second = await verify('--json');
      expect(second.code).toBe(EXIT_OK);
      const step = testStep(second.stdout);
      expect(step.status).toBe('passed');
      expect(step.cache).toBeDefined();
      expect(await calls(tools)).toHaveLength(1);
    });
  }

  it('says "1 test failed" for one', async () => {
    const [, vitestRunner] = RUNNERS;
    if (!vitestRunner) throw new Error('no Vitest runner');
    const { verify } = await repoWith(vitestRunner, ['rounds']);
    const result = await verify();
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      'vitest run (exit 0): the command reported success but 1 test failed',
    );
    expect(result.stdout).not.toContain('1 tests failed');
  });
});
