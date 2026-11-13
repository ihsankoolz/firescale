// The fixes from running 0.1.1 on the course project (ADR 0022): each Python
// service runs with its own virtual environment, one project's failure does
// not stop the others, and an environment that is not set up is reported as
// "setup needed", apart from real failures.
import { copyFile, mkdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { EXIT_OK, EXIT_VIOLATIONS, PYTHON_STUB, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const WINDOWS = process.platform === 'win32';
/** The interpreter inside a venv, as fscale writes it in a command on this platform. */
const VENV_PYTHON = WINDOWS ? '.venv\\Scripts\\python.exe' : '.venv/bin/python';
/** The same interpreter as path segments, for writing the stand-in. */
const VENV_PARTS = WINDOWS ? ['Scripts', 'python.exe'] : ['bin', 'python'];
const EXIT_BLOCK = 2;

interface Step {
  name: string;
  project?: string;
  status: string;
  command?: string;
  fix?: string;
  detail: string;
}

interface VerifyReport {
  schemaVersion: number;
  ok: boolean;
  result: string;
  failedStep?: string;
  failedProject?: string;
  steps: Step[];
  notes?: string[];
}

interface Facts {
  projects: {
    root: string;
    commands: Record<string, string>;
    interpreter?: { source: string; command: string; detail: string };
  }[];
}

async function validate<T>(schemaFile: string, json: string): Promise<T> {
  const schema: object = JSON.parse(
    await readFile(new URL(`../schemas/${schemaFile}`, import.meta.url), 'utf8'),
  );
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  check(data);
  expect(check.errors ?? []).toEqual([]);
  return data as T;
}

/** Copies the stand-in interpreter into `<env folder>/bin/python` or `Scripts\python.exe`. */
async function makeVenv(envFolder: string): Promise<void> {
  const target = path.join(envFolder, ...VENV_PARTS);
  await mkdir(path.dirname(target), { recursive: true });
  await copyFile(PYTHON_STUB, target);
}

/**
 * The course project, with stand-ins for `python`, `npx` and `npm` on the PATH and a
 * `.venv` in the folders of `withVenv`.
 */
async function courseProject(withVenv: string[] = []) {
  const repo = await fixtures.create({ files: courseProjectFiles() });
  for (const service of withVenv) {
    await makeVenv(path.join(repo.root, 'backend', service, '.venv'));
  }
  const tools = await fixtures.create({ files: {}, track: 'no-git' });
  await writeToolShims(tools.root);
  const log = path.join(tools.root, 'calls.log');
  const calls = async (): Promise<string[]> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean);
  const env = (extra: Record<string, string> = {}, fail = '') => ({
    ...shimEnv(tools.root, log, fail),
    FSCALE_STUB_LOG: log,
    ...extra,
  });
  return { repo, tools, calls, env };
}

describe('the Python interpreter of each project', () => {
  it("runs a service's steps with the interpreter in its own .venv, and says which one", async () => {
    const { repo, calls, env } = await courseProject(['audit_service']);
    const facts = await runCli(['facts', '--json'], repo.root, env());
    const parsed = await validate<Facts>('facts-output.schema.json', facts.stdout);
    const audit = parsed.projects.find((p) => p.root === 'backend/audit_service');
    expect(audit?.interpreter?.source).toBe('venv');
    expect(audit?.commands['test']).toBe(`${VENV_PYTHON} -m pytest`);
    expect(audit?.commands['install']).toBe(
      `${VENV_PYTHON} -m pip install -r requirements-dev.txt`,
    );
    // Not a finding: a service with no venv keeps today's command, and says so.
    const auth = parsed.projects.find((p) => p.root === 'backend/auth_service');
    expect(auth?.interpreter?.source).toBe('path');
    expect(auth?.interpreter?.detail).toMatch(/^no virtual environment found/);
    expect(auth?.commands['test']).toBe('python -m pytest');

    const human = await runCli(['facts'], repo.root, env());
    expect(human.stdout).toContain(
      `    interpreter: ${VENV_PYTHON} (the virtual environment in the project's .venv folder)\n`,
    );
    expect(human.stdout).toContain('    interpreter: python (no virtual environment found');

    const result = await runCli(['verify'], repo.root, env());
    expect(result.code).toBe(EXIT_OK);
    const ran = await calls();
    // The venv's interpreter really ran, in the service's folder, on every OS.
    expect(ran).toContain('venv-python -m pytest in audit_service');
    expect(ran).toContain('python -m pytest in auth_service');
    expect(ran.filter((line) => line.startsWith('venv-python'))).toEqual([
      'venv-python -m pytest in audit_service',
    ]);
    expect(result.stdout).toContain(
      'note: no virtual environment was found for backend/auth_service, backend/ledger_service, backend/market_service, backend/realtime_service, so their Python commands use python from the PATH.',
    );
  });

  it('uses an active VIRTUAL_ENV for a service that has no .venv of its own', async () => {
    const { repo, tools, calls, env } = await courseProject(['audit_service']);
    // A space in the path: the command must quote it for sh and for cmd.
    const active = path.join(tools.root, 'active env');
    await makeVenv(active);
    const result = await runCli(['verify'], repo.root, env({ VIRTUAL_ENV: active }));
    expect(result.code).toBe(EXIT_OK);
    const ran = await calls();
    // Every service ran a venv interpreter: its own, or the active one.
    for (const service of ['audit_service', 'auth_service', 'ledger_service']) {
      expect(ran).toContain(`venv-python -m pytest in ${service}`);
    }
    expect(ran.some((line) => line.startsWith('python '))).toBe(false);
    expect(result.stdout).not.toContain('no virtual environment was found');
    const facts = await validate<Facts>(
      'facts-output.schema.json',
      (await runCli(['facts', '--json'], repo.root, env({ VIRTUAL_ENV: active }))).stdout,
    );
    const sources = facts.projects
      .filter((p) => p.interpreter)
      .map((p) => `${p.root} ${p.interpreter?.source ?? ''}`);
    expect(sources).toEqual([
      'backend/audit_service venv',
      'backend/auth_service active',
      'backend/ledger_service active',
      'backend/market_service active',
      'backend/realtime_service active',
    ]);
  });
});

describe('projects run independently', () => {
  it("reports every project's result when the backend fails, and names every failure", async () => {
    const { repo, calls, env } = await courseProject();
    const result = await runCli(['verify', '--json'], repo.root, env({}, 'python'));
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate<VerifyReport>('verify-output.schema.json', result.stdout);
    expect(report.schemaVersion).toBe(5);
    expect(report.result).toBe('failed');
    expect([report.failedStep, report.failedProject]).toEqual(['test', 'backend/audit_service']);
    // The frontend still ran, after five failing services.
    const frontend = report.steps.filter((step) => step.project === 'frontend');
    expect(frontend.map((step) => `${step.name} ${step.status}`)).toEqual([
      'lint passed',
      'typecheck passed',
      'test passed',
    ]);
    // Only auth_service, whose lint failed first, has a step that did not run.
    expect(
      report.steps
        .filter((step) => step.status === 'not-run')
        .map((step) => `${step.name} in ${step.project ?? '.'}`),
    ).toEqual(['test in backend/auth_service']);
    expect(await calls()).toContain('npx --no oxlint in frontend');

    const human = await runCli(['verify'], repo.root, env({}, 'python'));
    expect(human.stdout.split('\n')[0]).toBe(
      'verify: FAILED at test in backend/audit_service, lint in backend/auth_service, test in backend/ledger_service, test in backend/market_service, test in backend/realtime_service',
    );
    // Within a project, the first failure still stops the rest.
    expect(human.stdout).toMatch(
      /in backend\/auth_service:\n {4}lint {7}FAILED .*\n.*\n {4}test {7}not run/,
    );
  });
});

describe('setup problems are not test failures', () => {
  const missing = { FSCALE_STUB_FAIL_IN: 'audit_service' };

  it('reports a missing module as setup needed, with the command that fixes it', async () => {
    const { repo, calls, env } = await courseProject(['audit_service']);
    const extra = {
      ...missing,
      FSCALE_STUB_MESSAGE: "E   ModuleNotFoundError: No module named 'jwt'",
    };
    const result = await runCli(['verify'], repo.root, env(extra));
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout.split('\n')[0]).toBe(
      'verify: SETUP NEEDED for test in backend/audit_service',
    );
    expect(result.stdout).toContain("the Python module 'jwt' is not installed");
    expect(result.stdout).toContain('Do not change code for it.');
    expect(result.stdout).toContain(
      `Set it up in backend/audit_service: ${VENV_PYTHON} -m pip install -r requirements-dev.txt`,
    );
    // The other services still ran.
    expect(await calls()).toContain('python -m pytest in realtime_service');

    const json = await runCli(['verify', '--json'], repo.root, env(extra));
    const report = await validate<VerifyReport>('verify-output.schema.json', json.stdout);
    expect(report.result).toBe('setup-needed');
    const test = report.steps.find(
      (step) => step.project === 'backend/audit_service' && step.name === 'test',
    );
    expect(test?.status).toBe('setup-needed');
    expect(test?.fix).toBe(`${VENV_PYTHON} -m pip install -r requirements-dev.txt`);

    // The stop hook tells the agent not to edit code for it.
    const hook = await runCli(['hooks', 'run', '--agent', 'claude'], repo.root, env(extra), {
      input: '{"session_id":"setup","stop_hook_active":false}',
    });
    expect(hook.code).toBe(EXIT_BLOCK);
    expect(hook.stderr).toContain('SETUP NEEDED');
    expect(hook.stderr).toContain('Do not edit code, tests or config to get past it.');
  });

  it('keeps a real test failure a normal failure', async () => {
    const { repo, env } = await courseProject(['audit_service']);
    const extra = { ...missing, FSCALE_STUB_MESSAGE: 'FAILED test_login - assert 401 == 200' };
    const result = await runCli(['verify', '--json'], repo.root, env(extra));
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate<VerifyReport>('verify-output.schema.json', result.stdout);
    expect(report.result).toBe('failed');
    expect(report.steps.some((step) => step.status === 'setup-needed')).toBe(false);
    const hook = await runCli(['hooks', 'run', '--agent', 'claude'], repo.root, env(extra), {
      input: '{"session_id":"real","stop_hook_active":false}',
    });
    expect(hook.code).toBe(EXIT_BLOCK);
    expect(hook.stderr).not.toContain('SETUP NEEDED');
  });
});
