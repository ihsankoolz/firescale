import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import {
  commitAll,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  NODE_STUB,
  runCli,
  stepCalls,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);

interface Step {
  name: string;
  project?: string;
  status: string;
  command?: string;
  exitCode?: number;
  durationMs: number;
  detail: string;
  outputTail?: string[];
}

interface VerifyReport {
  version: string;
  ok: boolean;
  result: string;
  runner: string;
  stepsTotal: number;
  stepsRun: number;
  failOnSkipped: boolean;
  failedStep?: string;
  failedProject?: string;
  rerun?: string;
  steps: Step[];
  skipped: { step: string; project?: string; reason: string }[];
  warnings?: string[];
}

async function validate(json: string): Promise<VerifyReport> {
  const schema: object = JSON.parse(await readFile(SCHEMA, 'utf8'));
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  const ok = check(data);
  expect(check.errors ?? []).toEqual([]);
  expect(ok).toBe(true);
  return data as VerifyReport;
}

/** Removes what differs between runs and machines, so the rest can be snapshotted. */
function stable(report: VerifyReport): VerifyReport {
  return {
    ...report,
    version: '<version>',
    steps: report.steps.map((step) => ({ ...step, durationMs: 0 })),
  };
}

const FAILING_TEST = [
  "console.log('running 2 tests');",
  "console.error('\\u001b[31mFAIL\\u001b[0m adds two numbers: expected 4, got 5');",
  'process.exit(1);',
  '',
].join('\n');

/** A repo whose verify commands are plain `node` scripts, so output is the same everywhere. */
function repoFiles(test: string, extra: Record<string, string> = {}): Record<string, string> {
  return {
    'firescale.config.json': JSON.stringify({
      verify: { lint: 'node lint.js', typecheck: false, test: 'node test.js' },
    }),
    'lint.js': "console.log('lint clean');\n",
    'test.js': test,
    ...extra,
  };
}

describe('fscale verify', () => {
  it('exits 1 with a compact summary when a test fails', async () => {
    const repo = await fixtures.create({ files: repoFiles(FAILING_TEST) });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stderr).toBe('');
    expect(result.stdout).toMatchSnapshot();
  });

  it('prints schema-valid JSON for a failing repo', async () => {
    const repo = await fixtures.create({ files: repoFiles(FAILING_TEST) });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate(result.stdout);
    expect(report.failedStep).toBe('test');
    expect(report.rerun).toBe('node test.js');
    expect(stable(report)).toMatchSnapshot();
  });

  it('exits 0 when every step passes', async () => {
    const repo = await fixtures.create({ files: repoFiles("console.log('2 passed');\n") });
    const human = await runCli(['verify'], repo.root);
    expect(human.code).toBe(EXIT_OK);
    expect(human.stdout).toMatchSnapshot();
    const json = await runCli(['verify', '--json'], repo.root);
    expect(json.code).toBe(EXIT_OK);
    const report = await validate(json.stdout);
    expect(report.ok).toBe(true);
    expect(report.failedStep).toBeUndefined();
  });

  it('fails at check, and runs nothing else, when an enforced rule fails', async () => {
    const repo = await fixtures.create({
      files: {
        ...repoFiles("console.log('2 passed');\n"),
        'firescale.config.json': JSON.stringify({
          rules: { 'core/no-committed-env': 'enforced' },
          verify: { lint: 'node lint.js', typecheck: false, test: 'node test.js' },
        }),
        '.env': 'EXAMPLE=1\n',
      },
    });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate(result.stdout);
    expect(report.failedStep).toBe('check');
    expect(report.rerun).toBe('fscale check');
    expect(report.steps.map((step) => [step.name, step.status])).toEqual([
      ['check', 'failed'],
      ['lint', 'not-run'],
      // Turned off in the config: "off", which is not a skipped step (ADR 0020).
      ['typecheck', 'off'],
      ['test', 'not-run'],
    ]);
    expect(report.steps[0]?.outputTail?.join('\n')).toContain('core/no-committed-env');
  });

  it('runs a command with quoted arguments as written, on every platform', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          verify: { lint: false, typecheck: false, test: 'node -e "process.exit(3)"' },
        }),
      },
    });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate(result.stdout);
    expect(report.steps[3]?.exitCode).toBe(3);
    expect(report.rerun).toBe('node -e "process.exit(3)"');
  });

  it('shows only enforced violations for a failed check, and caps the list', async () => {
    const envFiles = Object.fromEntries(
      Array.from({ length: 40 }, (_, i) => [
        `env/.env.${String(i).padStart(2, '0')}`,
        'EXAMPLE=1\n',
      ]),
    );
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          rules: { 'core/no-committed-env': 'enforced' },
        }),
        ...envFiles,
      },
    });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const tail = (await validate(result.stdout)).steps[0]?.outputTail ?? [];
    expect(tail).toHaveLength(30);
    expect(tail[0]).toMatch(/^core\/no-committed-env {2}env\/\.env\.00 {2}/);
    // Report-only rules (the other seven core rules) are not part of a failure.
    expect(tail.slice(0, 29).every((line) => line.startsWith('core/no-committed-env'))).toBe(true);
    expect(tail[29]).toBe('... and 11 more. Run `fscale check` to see them all.');
  });

  it('finds the test command from package.json scripts', async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
        'test.js': FAILING_TEST,
      },
    });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate(result.stdout);
    expect(report.runner).toBe('scripts');
    expect(report.failedStep).toBe('test');
    expect(report.rerun).toBe('npm run test');
    expect(report.steps.map((step) => [step.name, step.status])).toEqual([
      ['check', 'passed'],
      ['lint', 'skipped'],
      ['typecheck', 'skipped'],
      ['test', 'failed'],
    ]);
    const tail = report.steps[3]?.outputTail?.join('\n') ?? '';
    expect(tail).toContain('FAIL adds two numbers: expected 4, got 5');
    expect(tail).not.toContain('\u001b');
  });

  it('passes with every step skipped in a repo with nothing to run', async () => {
    const repo = await fixtures.create({ files: { 'notes.txt': 'hello\n' } });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const report = await validate(result.stdout);
    expect(report.runner).toBe('none');
    // Exit code 0, but not a clean pass: the result says so.
    expect(report.ok).toBe(true);
    expect(report.result).toBe('partial');
    expect([report.stepsRun, report.stepsTotal]).toEqual([1, 4]);
    expect(report.skipped.map((skip) => skip.step)).toEqual(['lint', 'typecheck', 'test']);
    expect(report.steps.slice(1).every((step) => step.status === 'skipped')).toBe(true);
    expect(report.warnings).toEqual([
      'steps were skipped because no command was found: lint, typecheck, test. Set verify.lint in your firescale config to a command, or to false if this repo has none.',
    ]);
  });

  it('prints a visible warning for a step with no command, and none for a step turned off', async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
        'firescale.config.json': JSON.stringify({ verify: { typecheck: false } }),
        'test.js': "console.log('ok');\n",
      },
    });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout).toContain(
      '\nwarning: 1 step was skipped because no command was found: lint.',
    );
    expect(result.stdout).not.toContain('typecheck.');

    const off = await fixtures.create({ files: repoFiles("console.log('2 passed');\n") });
    const quiet = await runCli(['verify', '--json'], off.root);
    expect((await validate(quiet.stdout)).warnings).toBeUndefined();
  });

  it('rejects bad usage and a bad verify config', async () => {
    const repo = await fixtures.create({ files: repoFiles('') });
    const extra = await runCli(['verify', 'now'], repo.root);
    expect(extra.code).toBe(EXIT_USAGE);
    expect(extra.stderr).toContain('verify takes no arguments');
    // --base works without --affected since blame-aware verify (ADR 0037);
    // a ref git cannot compare with is a usage error.
    await commitAll(repo.root, 'initial');
    const base = await runCli(['verify', '--base', 'no-such-ref'], repo.root);
    expect(base.code).toBe(EXIT_USAGE);
    expect(base.stderr).toContain('--base "no-such-ref" is not a commit git can compare with HEAD');

    const bad = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ verify: { tests: 'x' } }) },
    });
    const result = await runCli(['verify'], bad.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      'fscale: firescale.config.json: unknown verify step "tests". Allowed steps: lint, typecheck, test; other keys: timeout, idleTimeout, ci, failOnSkipped, stopWhenServiceDown, stopAfterErrors, baseBudget, baseInstall, labelFixed, tamper, projects.\n',
    );
  });

  it('never prints a plain OK when a step was skipped', async () => {
    // The false green from a real repo: nothing to run, and "verify: OK".
    const empty = await fixtures.create({ files: { 'notes.txt': 'hello\n' } });
    const nothing = await runCli(['verify'], empty.root);
    expect(nothing.code).toBe(EXIT_OK);
    expect(nothing.stdout).toMatchSnapshot();
    expect(nothing.stdout.startsWith('verify: PARTIAL - 1 of 4 steps ran, 3 skipped\n')).toBe(true);
    expect(nothing.stdout).not.toMatch(/^verify: OK/m);

    // One step skipped among steps that pass.
    const some = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
        'firescale.config.json': JSON.stringify({ verify: { typecheck: false } }),
        'test.js': "console.log('ok');\n",
      },
    });
    const partial = await runCli(['verify'], some.root);
    expect(partial.code).toBe(EXIT_OK);
    expect(partial.stdout.startsWith('verify: PARTIAL - 2 of 3 steps ran, 1 skipped\n')).toBe(true);

    // With every step given a command or turned off, the same repo is a clean pass.
    const all = await fixtures.create({ files: repoFiles("console.log('2 passed');\n") });
    const clean = await runCli(['verify'], all.root);
    expect(clean.stdout.startsWith('verify: OK\n')).toBe(true);
    expect(clean.stdout).not.toContain('skipped');
  });

  it('fails on a skipped step when verify.failOnSkipped is on', async () => {
    const files = {
      'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
      'test.js': "console.log('ok');\n",
    };
    const repo = await fixtures.create({
      files: {
        ...files,
        'firescale.config.json': JSON.stringify({ verify: { failOnSkipped: true } }),
      },
    });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toMatchSnapshot();
    const report = await validate((await runCli(['verify', '--json'], repo.root)).stdout);
    expect(report.ok).toBe(false);
    expect(report.result).toBe('failed');
    expect(report.failOnSkipped).toBe(true);
    expect(report.failedStep).toBeUndefined();

    // Not a finding: the same setting with nothing skipped passes.
    const complete = await fixtures.create({
      files: {
        ...files,
        'firescale.config.json': JSON.stringify({
          verify: { failOnSkipped: true, lint: false, typecheck: false },
        }),
      },
    });
    const ok = await runCli(['verify'], complete.root);
    expect(ok.code).toBe(EXIT_OK);
    expect(ok.stdout.startsWith('verify: OK\n')).toBe(true);

    const bad = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ verify: { failOnSkipped: 'yes' } }) },
    });
    const wrong = await runCli(['verify'], bad.root);
    expect(wrong.code).toBe(EXIT_USAGE);
    expect(wrong.stderr).toContain('verify.failOnSkipped must be true or false');
  });
});

describe('lint and typecheck on a package that is not installed', () => {
  /** What ESLint 9 prints when its flat config imports a package that is not installed. */
  const ESLINT_MISSING = [
    "const path = require('node:path');",
    "const config = path.join(process.cwd(), 'eslint.config.js');",
    "console.error('\\nOops! Something went wrong! :(\\n\\nESLint: 9.9.0\\n');",
    "console.error(`Error [ERR_MODULE_NOT_FOUND]: Cannot find package '@eslint/js' imported from ${config}`);",
    "console.error('    at packageResolve (node:internal/modules/esm/resolve:854:9)');",
    'process.exit(2);',
    '',
  ].join('\n');

  function lintRepo(extra: Record<string, string> = {}): Record<string, string> {
    return {
      'firescale.config.json': JSON.stringify({
        verify: { lint: 'node lint.cjs', typecheck: false, test: false },
      }),
      'package.json': JSON.stringify({
        name: 'web',
        private: true,
        devDependencies: { '@eslint/js': '9.9.0', eslint: '9.9.0' },
      }),
      // The lockfile names the package: it was installed before, so npm ci
      // can install it again without rewriting the lockfile.
      'package-lock.json': JSON.stringify({
        lockfileVersion: 3,
        packages: { 'node_modules/@eslint/js': { version: '9.9.0' } },
      }),
      'eslint.config.js':
        "import js from '@eslint/js';\nexport default [js.configs.recommended];\n",
      'lint.cjs': ESLINT_MISSING,
      ...extra,
    };
  }

  it('Lint that fails on a missing package is setup needed, not failed', async () => {
    const repo = await fixtures.create({ files: lintRepo() });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout.split('\n')[0]).toBe('verify: SETUP NEEDED for lint');
    expect(result.stdout).toContain("the package '@eslint/js' is not installed");
    expect(result.stdout).toContain('Do not change code for it.');
    // npm ci, not npm install: the lockfile stays as it is.
    expect(result.stdout).toContain('Set it up: npm ci\nThen run again: node lint.cjs\n');
    expect(result.stdout).not.toContain('Fix the failure');

    const json = await validate((await runCli(['verify', '--json'], repo.root)).stdout);
    expect(json.result).toBe('setup-needed');
    const lint = json.steps.find((step) => step.name === 'lint');
    expect(lint?.status).toBe('setup-needed');
    expect((lint as { fix?: string } | undefined)?.fix).toBe('npm ci');
  });

  it('names npm install when the lockfile does not name the package yet (ADR 0043)', async () => {
    // Just added to package.json: npm ci would refuse the lockfile.
    const repo = await fixtures.create({
      files: lintRepo({ 'package-lock.json': '{"lockfileVersion":3,"packages":{}}\n' }),
    });
    const result = await runCli(['verify'], repo.root);
    expect(result.stdout.split('\n')[0]).toBe('verify: SETUP NEEDED for lint');
    expect(result.stdout).toContain('Set it up: npm install\nThen run again: node lint.cjs\n');
  });

  it('is a normal failure when the package is installed', async () => {
    const repo = await fixtures.create({
      files: lintRepo({ 'node_modules/@eslint/js/package.json': '{"name":"@eslint/js"}' }),
    });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout.split('\n')[0]).toBe('verify: FAILED at lint');
    expect(result.stdout).toContain('Fix the failure, then run again: node lint.cjs');
  });
});

describe('fscale verify in a repo with projects in subfolders', () => {
  /** The course project, with stand-ins for python, npx and npm on the PATH. */
  async function courseProject(config?: object) {
    const files = courseProjectFiles();
    if (config) files['firescale.config.json'] = JSON.stringify(config);
    const repo = await fixtures.create({ files });
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root);
    const log = path.join(tools.root, 'calls.log');
    const calls = (): Promise<string[]> => stepCalls(log);
    return { repo, calls, env: (fail = '') => shimEnv(tools.root, log, fail) };
  }

  it("runs the backend's and the frontend's steps, each in its own folder, and names every skipped step", async () => {
    const { repo, calls, env } = await courseProject();
    const result = await runCli(['verify'], repo.root, env());
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout.startsWith('verify: PARTIAL - 10 of 19 steps ran, 9 skipped\n')).toBe(
      true,
    );
    // What really ran, and where.
    expect(await calls()).toEqual([
      'python -m pytest in audit_service',
      'python -m ruff check . in auth_service',
      'python -m pytest in auth_service',
      'python -m pytest in ledger_service',
      'python -m pytest in market_service',
      'python -m pytest in realtime_service',
      'npx --no oxlint in frontend',
      'npx --no tsc --noEmit in frontend',
      'npm run test:run in frontend',
    ]);

    const json = await runCli(['verify', '--json'], repo.root, env());
    const report = await validate(json.stdout);
    expect(report.runner).toBe('projects');
    expect(report.result).toBe('partial');
    expect([report.stepsRun, report.stepsTotal]).toEqual([10, 19]);
    // Every step that did not run is named, in the list and in the summary.
    const skipped = report.steps.filter((step) => step.status === 'skipped');
    expect(skipped).toHaveLength(9);
    expect(report.skipped.map((skip) => `${skip.step} in ${skip.project ?? '.'}`)).toEqual(
      skipped.map((step) => `${step.name} in ${step.project ?? '.'}`),
    );
    for (const skip of report.skipped) {
      expect(result.stdout).toContain(`${skip.step} in ${skip.project ?? ''}`);
    }
    expect(stable(report)).toMatchSnapshot();
  });

  it('names the project when one of its steps fails, and stops there', async () => {
    const { repo, calls, env } = await courseProject();
    const result = await runCli(['verify', '--json'], repo.root, env('npx'));
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate(result.stdout);
    expect(report.result).toBe('failed');
    expect([report.failedStep, report.failedProject]).toEqual(['lint', 'frontend']);
    expect(report.rerun).toBe('npx --no oxlint');
    expect((await calls()).at(-1)).toBe('npx --no oxlint in frontend');
    const human = await runCli(['verify'], repo.root, env('npx'));
    expect(human.stdout.startsWith('verify: FAILED at lint in frontend\n')).toBe(true);
    expect(human.stdout).toContain('Fix the failure, then run again in frontend: npx --no oxlint');
  });

  it('takes commands and switches for single projects from the config', async () => {
    const { repo, calls, env } = await courseProject({
      verify: {
        typecheck: false,
        projects: {
          'backend/audit_service': false,
          'backend/auth_service': { lint: false },
          'backend/ledger_service': { lint: 'python -m ruff check .' },
          'backend/market_service': { lint: false },
          'backend/realtime_service': { lint: false },
          frontend: { test: 'npx --no playwright test' },
          'backend/shared': { lint: 'python -m ruff check .' },
        },
      },
    });
    const result = await runCli(['verify'], repo.root, env());
    expect(result.code).toBe(EXIT_OK);
    // Nothing is skipped any more, so this is a clean pass.
    expect(result.stdout.startsWith('verify: OK\n')).toBe(true);
    expect(result.stdout).toMatchSnapshot();
    expect(await calls()).toEqual([
      'python -m pytest in auth_service',
      'python -m ruff check . in ledger_service',
      'python -m pytest in ledger_service',
      'python -m pytest in market_service',
      'python -m pytest in realtime_service',
      'python -m ruff check . in shared',
      'npx --no oxlint in frontend',
      'npx --no playwright test in frontend',
    ]);
  });

  it('rejects a project folder that does not exist, and a malformed projects section', async () => {
    const missing = await courseProject({ verify: { projects: { 'backend/nope': false } } });
    const result = await runCli(['verify'], missing.repo.root, missing.env());
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('verify.projects names "backend/nope"');
    expect(await missing.calls()).toEqual([]);

    for (const [projects, message] of [
      [{ '../other': false }, 'must be a folder inside the repo'],
      [{ '/abs': false }, 'must be a folder inside the repo'],
      [{ frontend: false, 'frontend/': false }, 'lists the folder "frontend" twice'],
      [{ frontend: true }, 'must be false, to leave the project out, or an object'],
      [{ frontend: { tests: 'x' } }, 'unknown verify step "tests"'],
      [{ frontend: { test: 5 } }, 'verify.projects["frontend"].test must be a command'],
    ] as const) {
      const bad = await courseProject({ verify: { projects } });
      const wrong = await runCli(['verify'], bad.repo.root, bad.env());
      expect(wrong.code).toBe(EXIT_USAGE);
      expect(wrong.stderr).toContain(message);
    }
  });

  it('does not say OK when a project is missing from the plan', async () => {
    // Found by review: a second kind of project at the root was dropped, and
    // a project whose package.json cannot be parsed vanished.
    const twoKinds = await fixtures.create({
      files: {
        'package.json': JSON.stringify({
          scripts: { lint: 'node -e ""', typecheck: 'node -e ""', test: 'node -e ""' },
        }),
        'pyproject.toml': '[project]\nname = "x"\n',
      },
    });
    const first = await runCli(['verify'], twoKinds.root);
    expect(first.code).toBe(EXIT_OK);
    expect(first.stdout).toMatchSnapshot();
    expect(first.stdout.startsWith('verify: PARTIAL - 4 of 7 steps ran, 3 skipped\n')).toBe(true);
    expect(first.stdout).toContain('in the repo root (python):');

    const broken = await fixtures.create({
      files: {
        'a/package.json': JSON.stringify({
          scripts: { lint: 'node -e ""', typecheck: 'node -e ""', test: 'node -e ""' },
        }),
        'b/package.json': '{"name":"b","scripts":{"test":"exit 1"},}',
      },
    });
    const second = await runCli(['verify', '--json'], broken.root);
    const report = await validate(second.stdout);
    expect(report.result).toBe('partial');
    expect(report.skipped.map((skip) => `${skip.step} in ${skip.project ?? ''}`)).toEqual([
      'lint in b',
      'typecheck in b',
      'test in b',
    ]);
  });

  it('names a misspelt verify option instead of calling it a step', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ verify: { failOnSkiped: true } }) },
    });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      'fscale: firescale.config.json: unknown key "failOnSkiped" in "verify". Did you mean "failOnSkipped"?\n',
    );
  });

  it('starts no Node process', async () => {
    const { repo, env } = await courseProject();
    const result = await runCli(['verify'], repo.root, { ...env(), FSCALE_NODE: NODE_STUB });
    expect(result.code).toBe(EXIT_OK);
    expect(result.stderr).toBe('');
  });
});
