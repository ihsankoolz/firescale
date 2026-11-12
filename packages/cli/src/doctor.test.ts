// `fscale doctor`: is this machine ready for the repo? (SPEC §17, ADR 0026)
// The tools doctor asks for their versions are stand-ins on a PATH the test
// controls, so the results are the same on every machine.
import { chmod, copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, EXIT_VIOLATIONS, PYTHON_STUB, REPO, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const WINDOWS = process.platform === 'win32';
const SCHEMA = new URL('../schemas/doctor-output.schema.json', import.meta.url);

interface Check {
  section: string;
  project?: string;
  subject: string;
  status: string;
  detail: string;
  fix?: string;
}

interface Report {
  schemaVersion: number;
  ok: boolean;
  problems: number;
  warnings: number;
  checks: Check[];
}

async function validate(json: string): Promise<Report> {
  const schema: object = JSON.parse(await readFile(SCHEMA, 'utf8'));
  const ajv = new Ajv({ allErrors: true });
  const check = ajv.compile(schema);
  const report: unknown = JSON.parse(json);
  if (!check(report)) throw new Error(ajv.errorsText(check.errors));
  return report as Report;
}

/** A port on this machine with nothing listening on it. */
async function closedPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address !== null ? address.port : 0;
      server.close(() => resolve(port));
    });
  });
}

/** Stand-ins that print a version, in `dir`. */
async function writeVersionTools(dir: string, tools: Record<string, string>): Promise<void> {
  await mkdir(dir, { recursive: true });
  for (const [tool, output] of Object.entries(tools)) {
    if (WINDOWS) {
      await writeFile(path.join(dir, `${tool}.cmd`), `@echo off\r\necho ${output}\r\n`);
    } else {
      const file = path.join(dir, tool);
      await writeFile(file, `#!/bin/sh\necho "${output}"\n`);
      await chmod(file, 0o755);
    }
  }
}

/** A PATH with only the stand-ins and the system's own folders. */
function toolPath(dir: string): Record<string, string | undefined> {
  const system = WINDOWS
    ? [path.join(process.env['SystemRoot'] ?? 'C:\\Windows', 'System32')]
    : ['/usr/bin', '/bin'];
  // One key: runCli drops every other spelling of it, such as Windows' `Path`.
  return { PATH: [dir, ...system].join(path.delimiter) };
}

/** Copies the python stand-in into `<project>/.venv` as its interpreter. */
async function makeVenv(project: string): Promise<string> {
  const venv = path.join(project, '.venv');
  const bin = WINDOWS ? path.join(venv, 'Scripts') : path.join(venv, 'bin');
  await mkdir(bin, { recursive: true });
  await copyFile(PYTHON_STUB, path.join(bin, WINDOWS ? 'python.exe' : 'python'));
  const site = WINDOWS
    ? path.join(venv, 'Lib', 'site-packages')
    : path.join(venv, 'lib', 'python3.12', 'site-packages');
  await mkdir(site, { recursive: true });
  return site;
}

/** The same text on every OS: the venv interpreter as on macOS and Linux. */
function sameOnEveryOs(text: string, port: number): string {
  return text
    .replaceAll(String(port), '<port>')
    .replaceAll('.venv\\\\Scripts\\\\python.exe', '.venv/bin/python')
    .replaceAll('.venv\\Scripts\\python.exe', '.venv/bin/python');
}

const SECRETS = ['super-secret-value-123', 'env-secret-456', 'Pw0rdInUrl', 'orders_db'];

/** A repo with one problem of each kind. */
async function brokenRepo() {
  const port = await closedPort();
  const repo = await fixtures.create({
    files: {
      'package.json': JSON.stringify({
        name: 'shop',
        packageManager: 'bun@1.1.0',
        engines: { node: '>=20' },
        dependencies: { 'left-pad': '1.3.0' },
      }),
      'bun.lock': '{}\n',
      'index.js': '',
      '.env.example': [
        'API_KEY=',
        `DATABASE_URL=postgresql://shop:Pw0rdInUrl@localhost:${port}/orders_db`,
        'SECRET_TOKEN=',
        '',
      ].join('\n'),
      '.env': 'API_KEY=super-secret-value-123\n',
      'backend/requirements.txt': 'requests==2.31.0\nflask==3.0.0\n',
      'backend/main.py': '',
      'backend/.python-version': '3.12\n',
    },
  });
  const site = await makeVenv(path.join(repo.root, 'backend'));
  await mkdir(path.join(site, 'requests-2.30.0.dist-info'));
  const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
  await writeVersionTools(tools, { node: 'v18.19.0' });
  const env = {
    ...toolPath(tools),
    SECRET_TOKEN: 'env-secret-456',
    FSCALE_STUB_PYTHON_VERSION: '3.11.9',
  };
  return { repo, port, env };
}

describe('fscale doctor', () => {
  it('reports a missing tool, a wrong version, a missing variable, missing dependencies and a service, each with a next step', async () => {
    const { repo, port, env } = await brokenRepo();
    const result = await runCli(['doctor'], repo.root, env);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const out = sameOnEveryOs(result.stdout, port);
    expect(out).toMatchSnapshot();
    // Each problem has its next step.
    expect(out).toContain(
      'PROBLEM  bun: bun is not installed, or not on the PATH; package.json (packageManager) asks for bun@1.1.0.\n           next: install Bun',
    );
    expect(out).toContain(
      'PROBLEM  node: node 18.19.0 is installed, but package.json (engines.node) asks for >=20.',
    );
    expect(out).toContain(
      'PROBLEM  .env.example: 1 of 3 variables are not set in the environment or .env: DATABASE_URL.\n           next: add DATABASE_URL to .env',
    );
    expect(out).toContain(
      'PROBLEM  node_modules: the dependencies of package.json are not installed.\n           next: bun install',
    );
    expect(out).toContain(
      'PROBLEM  backend packages: 1 of 2 packages in backend/requirements.txt are not installed: flask; 1 does not match its pinned version: requests 2.30.0 (wants 2.31.0).',
    );
    expect(out).toContain(
      'PROBLEM  backend python: python 3.11.9 is used for backend, but backend/.python-version asks for 3.12.',
    );
    expect(out).toContain(
      'PROBLEM  PostgreSQL on localhost:<port>: not reachable: nothing listens on port <port> (.env.example (DATABASE_URL)).\n           next: start PostgreSQL so it listens on port <port>',
    );
  });

  it('never prints the value of an env variable, a password or a connection string', async () => {
    const { repo, env } = await brokenRepo();
    for (const argv of [['doctor'], ['doctor', '--json']]) {
      const result = await runCli(argv, repo.root, env);
      for (const secret of SECRETS) {
        expect(result.stdout + result.stderr, `${argv.join(' ')}: ${secret}`).not.toContain(secret);
      }
      expect(result.stdout).not.toContain('postgresql://');
    }
  });

  it('prints JSON that matches its schema', async () => {
    const { repo, port, env } = await brokenRepo();
    const result = await runCli(['doctor', '--json'], repo.root, env);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = await validate(result.stdout);
    expect(report.ok).toBe(false);
    expect(report.problems).toBe(report.checks.filter((c) => c.status === 'problem').length);
    expect(sameOnEveryOs(result.stdout, port)).toMatchSnapshot();
  });

  it('exits 0 when everything is in place, and reaches a service that listens', async () => {
    const server = createServer();
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    const port = typeof address === 'object' && address !== null ? address.port : 0;
    try {
      const repo = await fixtures.create({
        files: {
          'backend/requirements.txt': 'requests==2.31\n',
          'backend/main.py': '',
          'backend/.env.example': `REDIS_URL=redis://127.0.0.1:${port}/0\n`,
          'backend/.env': `REDIS_URL=redis://127.0.0.1:${port}/0\n`,
        },
      });
      const site = await makeVenv(path.join(repo.root, 'backend'));
      await mkdir(path.join(site, 'requests-2.31.0.dist-info'));
      const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
      await writeVersionTools(tools, {});
      const result = await runCli(['doctor', '--json'], repo.root, toolPath(tools));
      const report = await validate(result.stdout);
      expect(report.checks.filter((c) => c.status !== 'ok')).toEqual([]);
      expect(result.code).toBe(EXIT_OK);
      expect(report.checks.map((c) => `${c.section} ${c.subject}`)).toEqual([
        'env backend/.env.example',
        'dependencies backend/.venv',
        'dependencies backend packages',
        `services Redis on 127.0.0.1:${port}`,
      ]);
    } finally {
      server.close();
    }
  });

  it('never lets a version command download anything', async () => {
    // Review of M12: go.mod can make `go version` fetch a toolchain, and
    // corepack can fetch a package manager.
    const repo = await fixtures.create({
      files: {
        'svc/go.mod': 'module example.com/svc\n\ngo 1.99.0\n',
        'svc/main.go': 'package main\n',
        'package.json': JSON.stringify({ name: 'x', packageManager: 'pnpm@9.1.0' }),
        'pnpm-lock.yaml': '',
      },
    });
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await mkdir(tools, { recursive: true });
    const shims: [string, string, string, string][] = [
      ['go', 'GOTOOLCHAIN', 'local', 'go version go1.22.0 linux/amd64'],
      ['pnpm', 'COREPACK_ENABLE_NETWORK', '0', '9.1.0'],
    ];
    for (const [tool, variable, wanted, output] of shims) {
      if (WINDOWS) {
        await writeFile(
          path.join(tools, `${tool}.cmd`),
          `@echo off\r\nif not "%${variable}%"=="${wanted}" exit /b 1\r\necho ${output}\r\n`,
        );
      } else {
        const file = path.join(tools, tool);
        const check = '[ "$' + variable + '" = "' + wanted + '" ] || exit 1';
        await writeFile(file, `#!/bin/sh\n${check}\necho "${output}"\n`);
        await chmod(file, 0o755);
      }
    }
    await writeVersionTools(tools, { node: 'v22.0.0' });
    const result = await runCli(['doctor', '--json'], repo.root, toolPath(tools));
    const report = await validate(result.stdout);
    const go = report.checks.find((c) => c.subject === 'svc go');
    expect(go?.detail).toBe('go 1.22.0 is installed, but svc/go.mod asks for go 1.99.0 or later.');
    const pnpm = report.checks.find((c) => c.subject === 'pnpm');
    expect(pnpm?.status).toBe('ok');
  });

  it('checks the active virtual environment that verify uses', async () => {
    const repo = await fixtures.create({
      files: { 'requirements.txt': 'requests==2.31.0\n', 'main.py': '' },
    });
    const outside = path.join(repo.root, '..', `${path.basename(repo.root)}-env`);
    const site = await makeVenv(outside);
    await mkdir(path.join(site, 'requests-2.31.0.dist-info'));
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, {});
    const result = await runCli(['doctor', '--json'], repo.root, {
      ...toolPath(tools),
      VIRTUAL_ENV: path.join(outside, '.venv'),
    });
    const report = await validate(result.stdout);
    expect(report.checks.filter((c) => c.status !== 'ok')).toEqual([]);
    expect(report.checks.map((c) => c.subject)).toContain('virtual environment (active)');
  });

  it('exits 0 on this repo on a machine that can build it', async () => {
    const result = await runCli(['doctor'], REPO);
    expect(result.stdout).toContain('doctor: OK');
    expect(result.code).toBe(EXIT_OK);
  });

  it('takes no arguments', async () => {
    const result = await runCli(['doctor', 'now'], REPO);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('doctor takes no arguments, got "now".');
  });
});

describe('verify points at doctor', () => {
  it('prints a one-line hint to run fscale doctor when a step needs setup', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: {
            lint: false,
            typecheck: false,
            test: `"${process.execPath}" -e "console.error('ModuleNotFoundError: No module named \\'jwt\\''); process.exit(1)"`,
          },
        }),
      },
    });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      "\nRun `fscale doctor` to check this machine's tools, env variables, dependencies and services in one go.\n",
    );
    const passing = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['core'],
          verify: { lint: false, typecheck: false, test: 'echo ok' },
        }),
      },
    });
    expect((await runCli(['verify'], passing.root)).stdout).not.toContain('fscale doctor');
  });
});
