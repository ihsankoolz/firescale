import { readFile } from 'node:fs/promises';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);

interface Step {
  name: string;
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
  runner: string;
  failedStep?: string;
  rerun?: string;
  steps: Step[];
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
      ['typecheck', 'skipped'],
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
    const base = await runCli(['verify', '--base', 'main'], repo.root);
    expect(base.code).toBe(EXIT_USAGE);
    expect(base.stderr).toContain('--base only works together with --affected');

    const bad = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ verify: { tests: 'x' } }) },
    });
    const result = await runCli(['verify'], bad.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      'fscale: firescale.config.json: unknown verify step "tests". Allowed steps: lint, typecheck, test.\n',
    );
  });
});
