// A test command that exits 0 while its own per-test results hold failures
// or errors is FAILED, and is never cached (ADR 0072). That is what a script
// ending in `|| exit 0` does, and a pytest hook that sets the exit status to 0.
// The stand-ins print what the real runners print for a failing run.
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(() => fixtures.cleanup());

/** What a stand-in prints, and the JUnit report it writes when pytest is asked for one. */
interface Answer {
  out?: string;
  err?: string;
  junit?: string;
  /** The exit code on the head (0 when left out), and on the base commit (0 when left out). */
  code?: number;
  baseCode?: number;
  /** Wait this long before answering. */
  delayMs?: number;
}

/**
 * Writes a stand-in for `tool` into `dir`. It exits 0 unless the answer says
 * otherwise. Each call that
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
    'if (answer.delayMs) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, answer.delayMs);',
    'if (answer.out) process.stdout.write(answer.out);',
    'if (answer.err) process.stderr.write(answer.err);',
    "process.exit((process.env.FSCALE_BASE_RUN === '1' ? answer.baseCode : answer.code) ?? 0);",
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
  label?: string;
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

/**
 * A repo whose only step is the runner's command, and a folder of stand-ins
 * beside it. With `answer`, the stand-in gives that instead; with `branch`,
 * the repo is on a `feature` branch off `main`, so there is a base commit.
 */
async function repoWith(
  runner: Runner,
  failing: string[],
  options: { answer?: Answer; branch?: boolean; command?: string } = {},
) {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: options.command ?? runner.command },
      }),
      '.gitignore': '.firescale/\n',
      'notes.txt': 'one\n',
    },
    ...(options.branch ? { track: 'no-git' as const } : {}),
  });
  if (options.branch) {
    expect((await runGit(repo.root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
    await commitAll(repo.root, 'base');
    expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    await writeFile(path.join(repo.root, 'notes.txt'), 'two\n');
  }
  const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
  await writeTool(tools, runner.tool, options.answer ?? runner.answer(failing));
  const current = process.env['PATH'] ?? process.env['Path'] ?? '';
  const env = {
    PATH: `${tools}${path.delimiter}${current}`,
    CI: undefined,
    GITHUB_ACTIONS: undefined,
    FSCALE_LOG: 'cache',
  };
  const verify = (...flags: string[]) => runCli(['verify', ...flags], repo.root, env);
  const answer = (next: Answer) => writeFile(path.join(tools, 'answer.json'), JSON.stringify(next));
  return { repo, tools, verify, answer };
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
  it('does not count a coverage threshold or unhandled errors beside an exit code of 0', async () => {
    const [, vitestRunner] = RUNNERS;
    if (!vitestRunner) throw new Error('no Vitest runner');
    const out =
      '⎯⎯⎯⎯ Unhandled Errors ⎯⎯⎯⎯\nVitest caught 1 unhandled error during the test run.\nError: boom\n ❯ src/a.ts:1:1\n' +
      'ERROR: Coverage for lines (50%) does not meet global threshold (80%)\n' +
      ' Test Files  1 passed (1)\n      Tests  3 passed (3)\n';
    const { verify } = await repoWith(vitestRunner, [], { answer: { out } });
    const result = await verify();
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout.split('\n')[0]).toBe('verify: OK');
  });
});

describe('an exit code of 0 beside failing tests, and the base commit', () => {
  const [pytestRunner] = RUNNERS;
  if (!pytestRunner) throw new Error('no pytest runner');

  // The base's command hides the same failing tests behind an exit code of
  // 0. Before ADR 0072 the base passed, so the head's failure was new and
  // blocked. Reading the base as failing would make it pre-existing: a pass
  // that is easier to get. The base is judged by its exit code, as before.
  it('keeps a failure new when the base commit hid the same failing tests', async () => {
    const answer = { ...pytestRunner.answer(['charges', 'refunds']), code: 1, baseCode: 0 };
    const { verify } = await repoWith(pytestRunner, [], { answer, branch: true });
    const result = await verify('--json');
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = JSON.parse(result.stdout) as Report;
    expect(report.result).toBe('failed');
    expect(testStep(result.stdout)).toMatchObject({ status: 'failed', label: 'new' });
  });

  // `pytest -x` stops at its first failure, so its results may hide a later
  // new one: they are never evidence for pre-existing (ADR 0039). That holds
  // when the head's command also hides its exit code.
  it('never takes a run that stops at its first failure as evidence, whatever it exits with', async () => {
    const answer = { ...pytestRunner.answer(['charges']), code: 0, baseCode: 1 };
    const { verify } = await repoWith(pytestRunner, [], {
      answer,
      branch: true,
      command: 'pytest -x',
    });
    const result = await verify('--json');
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const step = testStep(result.stdout);
    expect(step).toMatchObject({ status: 'failed', exitCode: 0 });
    // It blocks: the base cannot judge it, so it has no label, or it is new.
    expect(step.label).not.toBe('pre-existing');
    expect(step.label).not.toBe('flaky');
    expect((JSON.parse(result.stdout) as Report).result).toBe('failed');
  });
});

describe('an exit code of 0 beside failing tests, and the kept step time', () => {
  // A pass replaces the kept time of a step, also with a shorter one; a
  // failure only ever raises it (ADR 0057). This step is a failure.
  it('does not replace the time of the last pass with its own shorter one', async () => {
    const [, vitestRunner] = RUNNERS;
    if (!vitestRunner) throw new Error('no Vitest runner');
    const slow = { ...vitestRunner.answer([]), delayMs: 1500 };
    const { repo, verify, answer } = await repoWith(vitestRunner, [], { answer: slow });
    expect((await verify()).code).toBe(EXIT_OK);
    const kept = async (): Promise<number> => {
      const file = JSON.parse(
        await readFile(path.join(repo.root, '.firescale', 'step-times.json'), 'utf8'),
      ) as { steps: { step: string; durationMs: number }[] };
      return file.steps.find((one) => one.step === 'test')?.durationMs ?? 0;
    };
    expect(await kept()).toBeGreaterThanOrEqual(1500);

    await answer(vitestRunner.answer(['charges']));
    expect((await verify('--no-cache')).code).toBe(EXIT_VIOLATIONS);
    expect(await kept()).toBeGreaterThanOrEqual(1500);
  });
});
