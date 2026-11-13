import { chmod, mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

/**
 * The services whose `main.py` imports the shared folder `backend/shared`
 * (`from shared.db import URL`). The same as `COURSE_SHARED_USERS` in
 * `crates/core/src/testing.rs`.
 */
export const COURSE_SHARED_USERS: readonly string[] = [
  'auth_service',
  'ledger_service',
  'market_service',
];

/** The Python services of the course-project fixture, under `backend/`. */
export const COURSE_SERVICES = [
  'audit_service',
  'auth_service',
  'ledger_service',
  'market_service',
  'realtime_service',
] as const;

/**
 * A repo shaped like a real course project (ADR 0020): Docker Compose and
 * docs at the root with no manifest, five Python services under `backend/`
 * and a Vite app under `frontend/`. Only `auth_service` has a ruff config.
 * The folders discovery must skip each hold a marker file. The same files as
 * `course_project_files` in `crates/core/src/testing.rs`.
 */
export function courseProjectFiles(): Record<string, string> {
  const files: Record<string, string> = {
    'docker-compose.yml': 'services:\n  auth:\n    build: ./backend/auth_service\n',
    '.env.example': 'EXAMPLE_DATABASE_URL=\n',
    'README.md': '# Course project\n',
    'CLAUDE.md': '# Notes for agents\n',
    '.github/workflows/ci.yml': 'on: [pull_request]\n',
    '.gitignore': 'generated/\n',
    'backend/.coveragerc': '[run]\nbranch = True\n',
    'backend/shared/__init__.py': '',
    'backend/shared/db.py': 'URL = None\n',
    'backend/auth_service/ruff.toml': 'line-length = 100\n',
    'frontend/package.json': JSON.stringify({
      name: 'frontend',
      private: true,
      scripts: {
        dev: 'vite',
        build: 'vite build',
        test: 'vitest',
        'test:run': 'vitest run',
        'test:e2e': 'playwright test',
      },
      devDependencies: { typescript: '5.9.0' },
    }),
    'frontend/package-lock.json': '{}\n',
    'frontend/vite.config.ts': 'export default {};\n',
    'frontend/tsconfig.json': '{ "compilerOptions": { "strict": true } }\n',
    'frontend/playwright.config.ts': 'export default {};\n',
    'frontend/.oxlintrc.json': '{}\n',
    'frontend/knip.jsonc': '{}\n',
    'frontend/src/main.ts': 'export const app = 1;\n',
    'frontend/e2e/login.spec.ts': 'export {};\n',
    // Never searched: dependency, cache and build folders, and what .gitignore ignores.
    'frontend/node_modules/left-pad/package.json': '{"name":"left-pad"}',
    'frontend/dist/package.json': '{"name":"built"}',
    'node_modules/tool/package.json': '{"name":"tool"}',
    'dist/package.json': '{"name":"dist"}',
    'test-results/package.json': '{"name":"results"}',
    'generated/client/package.json': '{"name":"generated"}',
    'backend/.venv/lib/pkg/pyproject.toml': '[project]\nname = "vendored"\n',
    'backend/__pycache__/x/setup.py': '',
    'backend/.pytest_cache/x/pyproject.toml': '',
    'backend/.ruff_cache/x/pyproject.toml': '',
  };
  for (const service of COURSE_SERVICES) {
    const dir = `backend/${service}`;
    files[`${dir}/requirements.txt`] = 'fastapi==0.115.0\nsqlalchemy==2.0.36\n';
    files[`${dir}/requirements-dev.txt`] = '-r requirements.txt\npytest==8.3.3\n';
    files[`${dir}/pytest.ini`] = '[pytest]\ntestpaths = unit_test\n';
    files[`${dir}/Dockerfile`] = 'FROM python:3.13-slim\n';
    files[`${dir}/alembic.ini`] = '[alembic]\nscript_location = migrations\n';
    files[`${dir}/main.py`] = COURSE_SHARED_USERS.includes(service)
      ? 'from shared.db import URL\n\napp = None\n'
      : 'app = None\n';
    files[`${dir}/.env.example`] = 'EXAMPLE_SERVICE_URL=\n';
    files[`${dir}/unit_test/conftest.py`] = 'import pytest\n';
  }
  return files;
}

/**
 * Writes stand-ins for `python`, `npx` and `npm` into `dir`, so a test can run the
 * fixture's verify commands with none of them installed. Each one appends
 * "<tool> <arguments> in <name of the current folder>" to the file named by
 * `SHIM_LOG`, and exits 1 when `SHIM_FAIL` is the tool's name.
 */
export async function writeToolShims(
  dir: string,
  options: { versions?: boolean } = {},
): Promise<void> {
  await mkdir(dir, { recursive: true });
  for (const tool of ['python', 'npx', 'npm']) {
    if (process.platform === 'win32') {
      // With `versions`, `--version` prints "<tool> <SHIM_VERSION or 1.0.0>",
      // as the step cache needs a tool's version to key a step (ADR 0036).
      const version = options.versions
        ? [
            'if not defined SHIM_VERSION set SHIM_VERSION=1.0.0',
            `if "%~1"=="--version" (echo ${tool} %SHIM_VERSION%& exit /b 0)`,
          ]
        : [];
      const script = [
        '@echo off',
        // A call from the base run of blame-aware verify ends in " [base]".
        `set "SHIM_WHERE="`,
        `if defined FSCALE_BASE_RUN set "SHIM_WHERE= [base]"`,
        `for %%I in (.) do echo ${tool} %* in %%~nxI%SHIM_WHERE%>> "%SHIM_LOG%"`,
        ...version,
        `if "%SHIM_FAIL%"=="${tool}" exit /b 1`,
        'exit /b 0',
        '',
      ].join('\r\n');
      await writeFile(path.join(dir, `${tool}.cmd`), script);
    } else {
      const version = options.versions
        ? [`if [ "$1" = "--version" ]; then echo "${tool} \${SHIM_VERSION:-1.0.0}"; exit 0; fi`]
        : [];
      const script = [
        '#!/bin/sh',
        // A call from the base run of blame-aware verify ends in " [base]".
        `echo "${tool} $* in $(basename "$PWD")\${FSCALE_BASE_RUN:+ [base]}" >> "$SHIM_LOG"`,
        ...version,
        `if [ "$SHIM_FAIL" = "${tool}" ]; then echo "${tool} failed" >&2; exit 1; fi`,
        'exit 0',
        '',
      ].join('\n');
      const file = path.join(dir, tool);
      await writeFile(file, script);
      await chmod(file, 0o755);
    }
  }
}

/** The environment that puts the stand-ins first on the PATH. */
export function shimEnv(dir: string, log: string, fail = ''): Record<string, string> {
  const current = process.env['PATH'] ?? process.env['Path'] ?? '';
  return { PATH: `${dir}${path.delimiter}${current}`, SHIM_LOG: log, SHIM_FAIL: fail };
}
