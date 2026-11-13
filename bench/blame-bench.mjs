// Measures blame-aware verify (M21, ADR 0039): the time the base run adds,
// with a cold and a warm base cache, and the stop hook on a course-project
// shaped repo where one service changed.
//
// Usage: node bench/blame-bench.mjs [--bin <path>] [--runs <n>] [--label <text>]
// Prints a Markdown table: one row per scenario with p50, p99 and max.
//
// Scenarios, each in a throwaway git repo on a `feature` branch off `main`:
//   no failure        verify where every step passes: no base run at all
//   pre-existing      a test that also fails on the base: cold (the base
//                     store emptied before each run, so a worktree is made
//                     and the step runs there) and warm (kept from before)
//   stop hook         `fscale hooks run --agent claude` in five Python
//                     services and a frontend (stand-ins for python and npx),
//                     one service changed, its test failing on the base too:
//                     cold and warm, as above
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index === -1 ? fallback : args[index + 1];
};
const repoRoot = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const exe = process.platform === 'win32' ? 'fscale.exe' : 'fscale';
const bin = path.resolve(option('--bin', path.join(repoRoot, 'target', 'release', exe)));
const runs = Number(option('--runs', '30'));
const label = option('--label', process.platform);

function git(cwd, ...argv) {
  const result = spawnSync(
    'git',
    [
      '-c',
      'user.name=bench',
      '-c',
      'user.email=bench@example.invalid',
      '-c',
      'commit.gpgsign=false',
      ...argv,
    ],
    { cwd, windowsHide: true, encoding: 'utf8' },
  );
  if (result.status !== 0) {
    console.error(
      `git ${argv.join(' ')} failed: ${result.stderr}. Install git, then run this again.`,
    );
    process.exit(2);
  }
}

function makeRepo(files) {
  const root = mkdtempSync(path.join(tmpdir(), 'firescale-blame-bench-'));
  for (const [file, content] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    writeFileSync(path.join(root, file), content);
  }
  git(root, 'init', '--quiet', '--initial-branch=main');
  git(root, 'add', '--all');
  git(root, 'commit', '--quiet', '--no-verify', '-m', 'base');
  git(root, 'checkout', '--quiet', '-b', 'feature');
  return root;
}

function measure(cwd, argv, { before = () => {}, input, env = {} } = {}) {
  const once = () => {
    before();
    const start = performance.now();
    const result = spawnSync(bin, argv, {
      cwd,
      input,
      stdio: ['pipe', 'ignore', 'ignore'],
      windowsHide: true,
      env: { ...process.env, ...env },
    });
    if (result.error) {
      console.error(
        `Could not run ${bin}: ${result.error.message}. Build it with \`cargo build --release -p firescale-cli\`, or pass --bin.`,
      );
      process.exit(2);
    }
    if (result.status !== 0) {
      console.error(
        `${bin} ${argv.join(' ')} exited ${result.status}; the benchmark expects it to pass.`,
      );
      process.exit(2);
    }
    return performance.now() - start;
  };
  for (let i = 0; i < 2; i += 1) once();
  const samples = Array.from({ length: runs }, once).sort((a, b) => a - b);
  const at = (fraction) =>
    samples[Math.min(samples.length - 1, Math.floor(samples.length * fraction))];
  return { p50: at(0.5), p99: at(0.99), max: samples[samples.length - 1] };
}

const failing =
  "process.exit(require('fs').readFileSync('state.cfg', 'utf8').includes('fail') ? 1 : 0)";
const config = (test) =>
  JSON.stringify({ packs: ['core'], verify: { lint: 'node -e ""', typecheck: false, test } });

// 1. A plain repo: a test that passes, or one that fails on the base too.
const passing = makeRepo({
  'firescale.config.json': config('node test.js'),
  '.gitignore': '.firescale/\n',
  'test.js': failing,
  'state.cfg': 'ok\n',
});
writeFileSync(path.join(passing, 'change.cfg'), 'a change\n');
const old = makeRepo({
  'firescale.config.json': config('node test.js'),
  '.gitignore': '.firescale/\n',
  'test.js': failing,
  'state.cfg': 'fail\n',
});
writeFileSync(path.join(old, 'change.cfg'), 'a change\n');
const emptyStore = (root) => () =>
  rmSync(path.join(root, '.firescale', 'base'), { recursive: true, force: true });

// 2. A course-project shaped repo for the stop hook.
const services = [
  'audit_service',
  'auth_service',
  'ledger_service',
  'market_service',
  'realtime_service',
];
const course = {
  '.gitignore': '.firescale/\n',
  'frontend/package.json': '{"name":"frontend","scripts":{"test":"vitest run"}}',
  'frontend/package-lock.json': '{}\n',
  'frontend/src/main.ts': 'export const app = 1;\n',
};
for (const service of services) {
  course[`backend/${service}/main.py`] = 'app = None\n';
  course[`backend/${service}/pytest.ini`] = '[pytest]\n';
  course[`backend/${service}/requirements.txt`] = 'pytest==8.3.3\n';
}
const hookRepo = makeRepo(course);
writeFileSync(path.join(hookRepo, 'backend', 'audit_service', 'main.py'), 'app = 1\n');
// Stand-ins for python and npx: pytest fails in every service, on the head
// and on the base, so the changed service's failure is pre-existing.
const shims = mkdtempSync(path.join(tmpdir(), 'firescale-blame-bench-tools-'));
for (const tool of ['python', 'npx', 'npm']) {
  if (process.platform === 'win32') {
    writeFileSync(
      path.join(shims, `${tool}.cmd`),
      `@echo off\r\nif "%~1"=="--version" (echo ${tool} 1.0.0& exit /b 0)\r\nif "${tool}"=="python" (echo FAILED tests/test_a.py::test_x& exit /b 1)\r\nexit /b 0\r\n`,
    );
  } else {
    const file = path.join(shims, tool);
    writeFileSync(
      file,
      `#!/bin/sh\nif [ "$1" = "--version" ]; then echo "${tool} 1.0.0"; exit 0; fi\nif [ "${tool}" = "python" ]; then echo "FAILED tests/test_a.py::test_x"; exit 1; fi\nexit 0\n`,
    );
    chmodSync(file, 0o755);
  }
}
const hookEnv = { PATH: `${shims}${path.delimiter}${process.env.PATH ?? process.env.Path ?? ''}` };
const hookInput = JSON.stringify({ session_id: 'bench', stop_hook_active: false });
const hookArgs = ['hooks', 'run', '--agent', 'claude'];

const rows = [
  ['no failure: `fscale verify`, no base run', measure(passing, ['verify'])],
  [
    'pre-existing failure, cold base cache: `fscale verify`',
    measure(old, ['verify'], { before: emptyStore(old) }),
  ],
  ['pre-existing failure, warm base cache: `fscale verify`', measure(old, ['verify'])],
  [
    'stop hook, one of 6 projects changed, cold base cache',
    measure(hookRepo, hookArgs, { before: emptyStore(hookRepo), input: hookInput, env: hookEnv }),
  ],
  [
    'stop hook, one of 6 projects changed, warm base cache',
    measure(hookRepo, hookArgs, { input: hookInput, env: hookEnv }),
  ],
];
for (const root of [passing, old, hookRepo, shims]) rmSync(root, { recursive: true, force: true });

const ms = (value) => `${(Math.round(value * 10) / 10).toFixed(1)} ms`;
console.log(`${label}, ${runs} runs each\n`);
console.log('| Scenario | p50 | p99 | max |');
console.log('| --- | ---: | ---: | ---: |');
for (const [name, result] of rows) {
  console.log(`| ${name} | ${ms(result.p50)} | ${ms(result.p99)} | ${ms(result.max)} |`);
}
