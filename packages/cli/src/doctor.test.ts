// `fscale doctor`: is this machine ready for the repo? (SPEC §17, ADR 0026)
// The tools doctor asks for their versions are stand-ins on a PATH the test
// controls, so the results are the same on every machine.
import { chmod, copyFile, mkdir, readFile, symlink, writeFile } from 'node:fs/promises';
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
      'warning  .env.example: 1 of 3 variables are not set in the environment or .env: DATABASE_URL.\n           next: add DATABASE_URL to .env',
    );
    expect(out).toContain(
      'PROBLEM  node_modules: the dependencies of package.json are not installed.\n           next: bun install --frozen-lockfile',
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

  it('names the install that keeps the lockfile: npm ci with a package-lock.json, npm install without', async () => {
    const manifest = JSON.stringify({ name: 'web', dependencies: { 'left-pad': '1.3.0' } });
    const repo = await fixtures.create({
      files: {
        'locked/package.json': manifest,
        'locked/package-lock.json': '{}\n',
        'locked/index.js': '',
        'pnpm/package.json': manifest,
        'pnpm/pnpm-lock.yaml': "lockfileVersion: '9.0'\n",
        'pnpm/index.js': '',
        'plain/package.json': manifest,
        'plain/index.js': '',
      },
    });
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, { node: 'v22.11.0', npm: '10.9.0', pnpm: '9.12.0' });
    const result = await runCli(['doctor', '--json'], repo.root, toolPath(tools));
    const report = await validate(result.stdout);
    const fixes = Object.fromEntries(
      report.checks
        .filter((check) => check.section === 'dependencies')
        .map((check) => [check.project, check.fix]),
    );
    expect(fixes).toEqual({
      locked: 'in locked: npm ci',
      plain: 'in plain: npm install',
      pnpm: 'in pnpm: pnpm install --frozen-lockfile',
    });
  });

  it('never prints the value of an env variable, a password or a connection string', async () => {
    const { repo, env } = await brokenRepo();
    // Local database URLs in `.env` and `.env.local`, which doctor reads
    // for the host and port only.
    const envPort = await closedPort();
    const localUrls = [
      `postgresql+asyncpg://envuser:EnvPassw0rd@localhost:${envPort}/env_test_db`,
      'redis://localuser:L0calPassw0rd@127.0.0.1:6399/7',
    ];
    await writeFile(
      path.join(repo.root, '.env'),
      `API_KEY=super-secret-value-123\nTEST_DATABASE_URL=${localUrls[0]}\n`,
    );
    await writeFile(path.join(repo.root, '.env.local'), `CACHE_URL="${localUrls[1]}"\n`);
    const secrets = [
      ...SECRETS,
      ...localUrls,
      'envuser',
      'EnvPassw0rd',
      'env_test_db',
      'localuser',
      'L0calPassw0rd',
    ];
    for (const argv of [['doctor'], ['doctor', '--json']]) {
      const result = await runCli(argv, repo.root, env);
      // The services were read, so the test looks where they are printed.
      expect(result.stdout).toContain(`PostgreSQL on localhost:${envPort}`);
      expect(result.stdout).toContain('Redis on 127.0.0.1:6399');
      for (const secret of secrets) {
        expect(result.stdout + result.stderr, `${argv.join(' ')}: ${secret}`).not.toContain(secret);
      }
      for (const scheme of ['postgresql://', 'postgresql+asyncpg://', 'redis://']) {
        expect(result.stdout).not.toContain(scheme);
      }
    }
  });

  it('checks a local database that only .env names, and ignores a remote one', async () => {
    const port = await closedPort();
    const repo = await fixtures.create({
      files: {
        'backend/main.py': '',
        'backend/requirements.txt': 'requests==2.31.0\n',
        'backend/.env': [
          `TEST_DATABASE_URL=postgresql+asyncpg://user:pass@localhost:${port}/test`,
          'PROD_DATABASE_URL=postgresql://user:pass@db.example.com:5432/prod',
          '',
        ].join('\n'),
      },
    });
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, {});
    const result = await runCli(['doctor', '--json'], repo.root, toolPath(tools));
    const report = await validate(result.stdout);
    const services = report.checks.filter((c) => c.section === 'services');
    expect(services).toEqual([
      {
        section: 'services',
        subject: `PostgreSQL on localhost:${port}`,
        status: 'problem',
        detail: `not reachable: nothing listens on port ${port} (backend/.env (TEST_DATABASE_URL)).`,
        fix: expect.stringContaining(`start PostgreSQL so it listens on port ${port}`),
      },
    ]);
    expect(result.stdout).not.toContain('db.example.com');
    expect(result.code).toBe(EXIT_VIOLATIONS);
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

  // Finding 8 of the 0.5.1 run on the course project: the line read
  // "all 16 packages are in backend/audit_service/requirements-dev.txt installed".
  it('says that all the packages in a requirements file are installed, as the Node line does', async () => {
    // As in the course project: requirements-dev.txt takes in requirements.txt
    // with -r, so the install command names requirements-dev.txt.
    const names = Array.from({ length: 16 }, (_, i) => `example-pkg-${String(i + 1)}`);
    const pins = (list: string[]) => list.map((n) => `${n}==1.0.0\n`).join('');
    const repo = await fixtures.create({
      files: {
        'backend/audit_service/requirements.txt': pins(names.slice(0, 10)),
        'backend/audit_service/requirements-dev.txt': `-r requirements.txt\n${pins(names.slice(10))}`,
        'backend/audit_service/main.py': '',
        'backend/one_service/requirements.txt': 'requests==2.31.0\n',
        'backend/one_service/main.py': '',
      },
    });
    const audit = await makeVenv(path.join(repo.root, 'backend', 'audit_service'));
    for (const name of names) {
      await mkdir(path.join(audit, `${name.replaceAll('-', '_')}-1.0.0.dist-info`));
    }
    const one = await makeVenv(path.join(repo.root, 'backend', 'one_service'));
    await mkdir(path.join(one, 'requests-2.31.0.dist-info'));
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, {});
    const result = await runCli(['doctor'], repo.root, toolPath(tools));
    const lines = result.stdout
      .split(/\r?\n/)
      .filter((line) => line.includes(' packages: '))
      .map((line) => line.trim());
    expect(lines).toEqual([
      'ok       backend/audit_service packages: all 16 packages in backend/audit_service/requirements-dev.txt are installed',
      'ok       backend/one_service packages: the 1 package in backend/one_service/requirements.txt is installed',
    ]);
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

// A service's `.env.example` whose variables are set in an env file of a
// folder above it, as on a repo with one `.env` at the root (ADR 0059).
describe('doctor and env files in the folders above a project', () => {
  const ABOVE_SECRETS = ['root-env-secret-789', 'mid-secret-321', 'own-secret-654'];

  async function servicesRepo() {
    const service = { 'requirements.txt': '', 'main.py': '' };
    const files: Record<string, string> = {
      '.env': 'SVC_API_KEY=root-env-secret-789\nUNRELATED=1\n',
      'backend/.env.local': 'export SVC_REGION="mid-secret-321"\n',
      'backend/svc/.env.example': 'SVC_API_KEY=\nSVC_REGION=\n',
      'backend/other/.env.example': 'OTHER_TOKEN=\n',
      'backend/own/.env.example': 'OWN_A=\nOWN_B=\n',
      'backend/own/.env': 'OWN_A=own-secret-654\n',
      'lonely/.env.example': 'LONELY=\n',
    };
    for (const folder of ['backend/svc', 'backend/other', 'backend/own', 'lonely']) {
      for (const [name, text] of Object.entries(service)) files[`${folder}/${name}`] = text;
    }
    const repo = await fixtures.create({ files });
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, {});
    return { repo, env: toolPath(tools) };
  }

  it('finds a variable in the .env of a folder above, names the file, and never prints a value', async () => {
    const { repo, env } = await servicesRepo();
    const json = await runCli(['doctor', '--json'], repo.root, env);
    const report = await validate(json.stdout);
    const checks = report.checks
      .filter((check) => check.section === 'env')
      .map(({ subject, status, detail, fix }) => ({ subject, status, detail, fix }))
      .sort((a, b) => (a.subject < b.subject ? -1 : 1));
    expect(checks).toEqual([
      {
        subject: 'backend/other/.env.example',
        status: 'warning',
        detail:
          '1 of 1 variables are not set in the environment or backend/.env.local, .env, and there is no backend/other/.env: OTHER_TOKEN.',
        fix: 'add OTHER_TOKEN to the env file that backend/other reads: backend/.env.local, .env, or a new backend/other/.env (see backend/other/.env.example for what each one is)',
      },
      {
        subject: 'backend/own/.env.example',
        status: 'warning',
        detail:
          '1 of 2 variables are not set in the environment or backend/own/.env, backend/.env.local, .env: OWN_B.',
        fix: 'add OWN_B to backend/own/.env (see backend/own/.env.example for what each one is)',
      },
      {
        subject: 'backend/svc/.env.example',
        status: 'ok',
        detail:
          'all 2 variables are set in the environment, or in an env file of a folder above (SVC_REGION in backend/.env.local; SVC_API_KEY in .env)',
        fix: undefined,
      },
      {
        subject: 'lonely/.env.example',
        status: 'warning',
        detail:
          '1 of 1 variables are not set in the environment or .env, and there is no lonely/.env: LONELY.',
        fix: 'add LONELY to the env file that lonely reads: .env, or a new lonely/.env (see lonely/.env.example for what each one is)',
      },
    ]);
    const human = await runCli(['doctor'], repo.root, env);
    expect(human.stdout).toContain(
      '  ok       backend/svc/.env.example: all 2 variables are set in the environment, or in an env file of a folder above (SVC_REGION in backend/.env.local; SVC_API_KEY in .env)\n',
    );
    for (const out of [json, human]) {
      for (const secret of ABOVE_SECRETS) {
        expect(out.stdout + out.stderr, secret).not.toContain(secret);
      }
    }
  });

  it('says so when no folder at or above the project has an env file', async () => {
    const repo = await fixtures.create({
      files: {
        'lonely/.env.example': 'LONELY=\n',
        'lonely/requirements.txt': '',
        'lonely/main.py': '',
      },
    });
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, {});
    const result = await runCli(['doctor'], repo.root, toolPath(tools));
    expect(result.stdout).toContain(
      '  warning  lonely/.env.example: 1 of 1 variables are not set in the environment, and there is no lonely/.env, nor a .env in a folder above it: LONELY.\n           next: copy lonely/.env.example to lonely/.env and fill in the values (or set the variables in your shell)\n',
    );
    // The variable set nowhere is a warning and counts as one (ADR 0068).
    // The exit code is 1 for the one problem beside it: the project has no
    // virtual environment.
    expect(result.stdout).toContain('\ndoctor: 1 problem, 1 warning - ');
    expect(result.code).toBe(EXIT_VIOLATIONS);
  });

  // Making a symlink to a file needs a privilege on Windows.
  it.skipIf(WINDOWS)(
    'reads no env file through a symbolic link, says which file, and never prints a value',
    async () => {
      // Made-up names and values, in a file outside the repo.
      const outside = await fixtures.create({
        files: {
          'planted.env':
            'LINKED_KEY=linked-secret-159\nLINKED_DB_URL=postgresql://u:linked-pw-753@localhost:5999/linked_db\n',
        },
        track: 'no-git',
      });
      const repo = await fixtures.create({
        files: {
          'svc/.env.example': 'LINKED_KEY=\n',
          'svc/requirements.txt': '',
          'svc/main.py': '',
        },
      });
      await symlink(path.join(outside.root, 'planted.env'), path.join(repo.root, 'svc', '.env'));
      const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
      await writeVersionTools(tools, {});
      const human = await runCli(['doctor'], repo.root, toolPath(tools));
      const json = await runCli(['doctor', '--json'], repo.root, toolPath(tools));
      // The name in the linked file does not count as set.
      expect(human.stdout).toContain(
        '  warning  svc/.env.example: 1 of 1 variables are not set in the environment: LINKED_KEY. svc/.env is a symbolic link, so it was not read.\n           next: replace svc/.env with a real file that sets LINKED_KEY, or set the variables in your shell (see svc/.env.example for what each one is)\n',
      );
      // The variable set nowhere is a warning and counts as one (ADR 0068).
      // The exit code is 1 for the one problem beside it: the project has no
      // virtual environment.
      expect(human.stdout).toContain('\ndoctor: 1 problem, 1 warning - ');
      expect(human.code).toBe(EXIT_VIOLATIONS);
      // The database it names is not checked either.
      const report = await validate(json.stdout);
      expect(report.checks.filter((check) => check.section === 'services')).toEqual([]);
      for (const out of [human, json]) {
        for (const secret of ['linked-secret-159', 'linked-pw-753', 'linked_db']) {
          expect(out.stdout + out.stderr, secret).not.toContain(secret);
        }
      }
    },
  );

  it('never reads a .env above the firescale root', async () => {
    // The firescale root is `repo/`, inside the fixture's own temp folder;
    // the `.env` beside it is outside the root.
    const outer = await fixtures.create({
      files: {
        '.env': 'ABOVE_ROOT_KEY=above-root-secret-987\n',
        'repo/firescale.config.json': JSON.stringify({ packs: ['core'] }),
        'repo/svc/.env.example': 'ABOVE_ROOT_KEY=\n',
        'repo/svc/requirements.txt': '',
        'repo/svc/main.py': '',
      },
      track: 'no-git',
    });
    const tools = path.join(outer.root, 'tools');
    await writeVersionTools(tools, {});
    const result = await runCli(['doctor'], path.join(outer.root, 'repo'), toolPath(tools));
    expect(result.stdout).toContain(
      '  warning  svc/.env.example: 1 of 1 variables are not set in the environment, and there is no svc/.env, nor a .env in a folder above it: ABOVE_ROOT_KEY.\n',
    );
    expect(result.stdout + result.stderr).not.toContain('above-root-secret-987');
    // The variable set nowhere is a warning and counts as one (ADR 0068).
    // The exit code is 1 for the one problem beside it: the project has no
    // virtual environment.
    expect(result.stdout).toContain('\ndoctor: 1 problem, 1 warning - ');
    expect(result.code).toBe(EXIT_VIOLATIONS);
  });
});

// A variable that a `.env.example` names and that is set nowhere (ADR 0068).
describe('doctor and a variable that is set nowhere', () => {
  const NAME = 'FSCALE_TEST_UNSET_NAME_0068';
  const LINE = `.env.example: 1 of 1 variables are not set in the environment, and there is no .env: ${NAME}.\n           next: copy .env.example to .env and fill in the values (or set the variables in your shell)\n`;

  /** A repo with nothing but an env example, and the config given. */
  async function onlyAnExample(
    config?: Record<string, string>,
  ): Promise<{ root: string; env: Record<string, string | undefined> }> {
    const repo = await fixtures.create({ files: { '.env.example': `${NAME}=\n`, ...config } });
    const tools = path.join(repo.root, '..', `${path.basename(repo.root)}-tools`);
    await writeVersionTools(tools, {});
    return { root: repo.root, env: { ...toolPath(tools), [NAME]: undefined } };
  }

  it('is a warning by default: doctor says OK with warnings and exits 0', async () => {
    for (const config of [
      undefined,
      { 'firescale.config.json': JSON.stringify({ packs: ['core'] }) },
      { 'firescale.config.json': JSON.stringify({ doctor: { missingEnv: 'warning' } }) },
      { 'firescale.config.json': JSON.stringify({ doctor: {} }) },
    ]) {
      const { root, env } = await onlyAnExample(config);
      const human = await runCli(['doctor'], root, env);
      expect(human.stdout).toBe(
        `Environment variables\n  warning  ${LINE}\ndoctor: OK with 1 warning - read them above.\n`,
      );
      expect(human.code).toBe(EXIT_OK);
      const json = await runCli(['doctor', '--json'], root, env);
      const report = await validate(json.stdout);
      expect(report).toMatchObject({ ok: true, problems: 0, warnings: 1 });
      expect(report.checks.map((check) => check.status)).toEqual(['warning']);
      expect(json.code).toBe(EXIT_OK);
    }
  });

  it('is a problem again with doctor.missingEnv set to "problem": doctor exits 1', async () => {
    const { root, env } = await onlyAnExample({
      'firescale.config.json': JSON.stringify({ doctor: { missingEnv: 'problem' } }),
    });
    const human = await runCli(['doctor'], root, env);
    expect(human.stdout).toBe(
      `Environment variables\n  PROBLEM  ${LINE}\ndoctor: 1 problem - run the \`next:\` commands above, then \`fscale doctor\` again.\n`,
    );
    expect(human.code).toBe(EXIT_VIOLATIONS);
    const json = await runCli(['doctor', '--json'], root, env);
    const report = await validate(json.stdout);
    expect(report).toMatchObject({ ok: false, problems: 1, warnings: 0 });
    expect(json.code).toBe(EXIT_VIOLATIONS);

    // The setting is what decides: the same repo with "warning" has no
    // problem. Without this the test passed before the setting existed,
    // when every missing name was a problem.
    await writeFile(
      path.join(root, 'firescale.config.json'),
      JSON.stringify({ doctor: { missingEnv: 'warning' } }),
    );
    const warned = await runCli(['doctor'], root, env);
    expect(warned.stdout).toContain(`\n  warning  ${LINE}\n`);
    expect(warned.code).toBe(EXIT_OK);
  });

  it('refuses a doctor setting it does not know, in doctor and in check', async () => {
    for (const [doctor, message] of [
      [
        { missingEnv: 'error' },
        'firescale.config.json: doctor.missingEnv must be "warning" or "problem".',
      ],
      [
        { missingEnv: true },
        'firescale.config.json: doctor.missingEnv must be "warning" or "problem".',
      ],
      [
        { missingEnvs: 'problem' },
        'firescale.config.json: unknown key "missingEnvs" in doctor. Allowed keys: missingEnv.',
      ],
      [
        'problem',
        'firescale.config.json: "doctor" must be an object, such as { "missingEnv": "problem" }.',
      ],
    ] as const) {
      const { root, env } = await onlyAnExample({
        'firescale.config.json': JSON.stringify({ doctor }),
      });
      for (const command of ['doctor', 'check']) {
        const result = await runCli([command], root, command === 'doctor' ? env : undefined);
        expect(result.stderr, command).toBe(`fscale: ${message}\n`);
        expect(result.code, command).toBe(EXIT_USAGE);
      }
    }
  });

  it('says that a firescale.config.ts is not run, so the setting in it is not read', async () => {
    const { root, env } = await onlyAnExample({
      'firescale.config.ts': "export default { doctor: { missingEnv: 'problem' } };\n",
    });
    const human = await runCli(['doctor'], root, env);
    expect(human.stdout).toBe(
      `Environment variables\n  warning  ${LINE}  warning  firescale.config.ts: doctor does not run firescale.config.ts (it is code), so doctor.missingEnv in it is not read: a variable that is set nowhere is a warning.\n\ndoctor: OK with 2 warnings - read them above.\n`,
    );
    expect(human.code).toBe(EXIT_OK);
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
