// How `fscale verify` runs its commands on slow, real projects (ADR 0024):
// live progress on stderr, stopping every child process on Ctrl+C or a
// termination signal, time limits, no input with CI=1, parallel projects,
// and runs that never wait silently on each other.
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { BIN, EXIT_OK, EXIT_VIOLATIONS, HOST, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
const started: ChildProcess[] = [];
afterEach(async () => {
  for (const child of started.splice(0)) child.kill('SIGKILL');
  await fixtures.cleanup();
});

const WINDOWS = process.platform === 'win32';
const EXIT_STOPPED = 130;
const EXE = WINDOWS ? '.exe' : '';
const TREE_STUB = path.join(path.dirname(BIN), `fscale-tree-stub${EXE}`);
const CTRL_BREAK = path.join(path.dirname(BIN), `fscale-ctrl-break${EXE}`);

/** A command line that runs the process tree stub, for sh and for cmd. */
function treeCommand(folder: string, depth = 2): string {
  return `"${TREE_STUB}" "${folder}" ${depth}`;
}

/** A command line that runs a Node script file in the step's folder. */
function nodeScript(file: string): string {
  return `"${process.execPath}" ${file}`;
}

interface Step {
  name: string;
  project?: string;
  status: string;
  timeoutMs?: number;
  timeoutKey?: string;
  outputTail?: string[];
}

interface Report {
  schemaVersion: number;
  result: string;
  ok: boolean;
  steps: Step[];
}

/** A repo whose verify runs only what `verify` says, in folders `folders`. */
async function repoWith(verify: Record<string, unknown>, files: Record<string, string> = {}) {
  return fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({ packs: ['core'], verify }),
      ...files,
    },
  });
}

/** True while a process with this id exists and is not a zombie. */
async function alive(pid: number): Promise<boolean> {
  try {
    process.kill(pid, 0);
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
  if (process.platform === 'linux') {
    const stat = await readFile(`/proc/${pid}/stat`, 'utf8').catch(() => '');
    // The state follows the command name in parentheses; Z is a zombie.
    const state = stat.slice(stat.lastIndexOf(')') + 2, stat.lastIndexOf(')') + 3);
    return stat !== '' && state !== 'Z';
  }
  return true;
}

/** The process ids the tree stub wrote into `folder`. */
async function treePids(folder: string): Promise<number[]> {
  const names = await readdir(folder).catch(() => [] as string[]);
  return names.filter((name) => name.endsWith('.pid')).map((name) => Number.parseInt(name, 10));
}

/** Waits until `check` is true, or fails after `ms`. */
async function until(check: () => boolean | Promise<boolean>, ms = 30_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`gave up after ${ms} ms`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

/** Expects every process of the tree to be gone within a few seconds. */
async function expectTreeGone(folder: string, count: number): Promise<void> {
  const pids = await treePids(folder);
  expect(pids).toHaveLength(count);
  await until(async () => (await Promise.all(pids.map(alive))).every((up) => !up), 10_000).catch(
    () => undefined,
  );
  // Survivors are only named, never killed here: on Windows an id can be
  // reused quickly by an unrelated process.
  const survivors: number[] = [];
  for (const pid of pids) if (await alive(pid)) survivors.push(pid);
  expect(survivors).toEqual([]);
}

/** The environment of a started fscale, like `runCli` gives it. */
function childEnv(extra: Record<string, string> = {}): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, FSCALE_NODE_HOST: HOST, ...extra };
  delete env['VIRTUAL_ENV'];
  return env;
}

/** Starts `fscale verify` and collects what it prints. */
function startVerify(cwd: string, args: string[] = []) {
  const child = spawn(BIN, ['verify', '--progress', ...args], {
    cwd,
    env: childEnv(),
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
  started.push(child);
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (chunk: Buffer) => (stdout += chunk.toString()));
  child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
  const exited = new Promise<number | null>((resolve) =>
    child.on('close', (code) => resolve(code)),
  );
  return { child, exited, out: () => ({ stdout, stderr }) };
}

describe('stopping verify stops every process it started', () => {
  it.skipIf(WINDOWS).each(['SIGINT', 'SIGTERM'] as const)(
    'on %s, stops the whole tree and prints the summary of what finished',
    async (signal) => {
      const scratch = await fixtures.create({ files: {}, track: 'no-git' });
      const pids = path.join(scratch.root, 'pids');
      const repo = await repoWith({
        lint: 'echo lint-ran',
        typecheck: false,
        test: treeCommand(pids),
      });
      const run = startVerify(repo.root);
      await until(() => existsSync(path.join(pids, 'ready')));
      run.child.kill(signal);
      const code = await run.exited;
      const { stdout, stderr } = run.out();
      expect(code).toBe(EXIT_STOPPED);
      expect(stdout).toMatch(/^verify: STOPPED - interrupted after 2 of 3 steps ran\n/);
      expect(stdout).toContain('  lint       passed   echo lint-ran\n');
      expect(stdout).toMatch(/ {2}test {7}STOPPED /);
      expect(stdout).toContain('Stopped: verify was interrupted');
      expect(stderr).toContain('[verify] test: STOPPED after');
      await expectTreeGone(pids, 3);
    },
  );

  it.skipIf(WINDOWS)(
    'on a second Ctrl+C, kills the commands at once and ends without waiting',
    async () => {
      const scratch = await fixtures.create({ files: {}, track: 'no-git' });
      const pidFile = path.join(scratch.root, 'sleep.pid');
      // The command ignores SIGTERM, so the first Ctrl+C has to wait for
      // the grace period before SIGKILL; the second one does not wait.
      const repo = await repoWith({
        lint: false,
        typecheck: false,
        test: `trap '' TERM; sleep 120 & echo $! > "${pidFile}"; wait`,
      });
      const run = startVerify(repo.root);
      await until(() => existsSync(pidFile));
      const pid = Number.parseInt(await readFile(pidFile, 'utf8'), 10);
      run.child.kill('SIGINT');
      await new Promise((resolve) => setTimeout(resolve, 200));
      const begun = Date.now();
      run.child.kill('SIGINT');
      await run.exited;
      expect(run.child.signalCode).toBe('SIGINT');
      expect(Date.now() - begun).toBeLessThan(1500);
      await until(async () => !(await alive(pid)), 5_000).catch(() => undefined);
      expect(await alive(pid)).toBe(false);
    },
  );

  it.runIf(WINDOWS).each([
    { mode: 'group', to: "fscale's process group only" },
    { mode: 'console', to: 'every process on the console, as a key press does' },
  ] as const)(
    'on Ctrl+Break sent to $to, stops the whole job and prints the summary of what finished',
    async ({ mode }) => {
      const scratch = await fixtures.create({ files: {}, track: 'no-git' });
      const pids = path.join(scratch.root, 'pids');
      const output = path.join(scratch.root, 'verify.out');
      const repo = await repoWith({
        lint: 'echo lint-ran',
        typecheck: false,
        test: treeCommand(pids),
      });
      const helper = spawn(
        CTRL_BREAK,
        [mode, path.join(pids, 'ready'), output, BIN, 'verify', '--progress'],
        { cwd: repo.root, env: childEnv(), stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true },
      );
      started.push(helper);
      let helperErr = '';
      helper.stderr.on('data', (chunk: Buffer) => (helperErr += chunk.toString()));
      const code = await new Promise<number | null>((resolve) => helper.on('close', resolve));
      expect(helperErr).toBe('');
      const stdout = await readFile(output, 'utf8');
      expect(code).toBe(EXIT_STOPPED);
      expect(stdout).toMatch(/^verify: STOPPED - interrupted after 2 of 3 steps ran\r?\n/);
      // The step whose commands got the event too is stopped, not failed.
      expect(stdout).toMatch(/ {2}test {7}STOPPED /);
      expect(stdout).not.toContain('FAILED');
      expect(stdout).toContain('Stopped: verify was interrupted');
      expect(await readFile(`${output}.err`, 'utf8')).toContain('[verify] test: STOPPED after');
      await expectTreeGone(pids, 3);
    },
  );

  it.runIf(WINDOWS)(
    'when fscale itself is killed, the job takes its processes with it',
    async () => {
      const scratch = await fixtures.create({ files: {}, track: 'no-git' });
      const pids = path.join(scratch.root, 'pids');
      const repo = await repoWith({ lint: false, typecheck: false, test: treeCommand(pids) });
      const run = startVerify(repo.root);
      await until(() => existsSync(path.join(pids, 'ready')));
      // TerminateProcess: no handler runs. The job object's last handle closes.
      run.child.kill();
      await run.exited;
      await expectTreeGone(pids, 3);
    },
  );
});

describe('a process a step leaves behind', () => {
  it('does not keep verify waiting on its output, and is stopped with the step', async () => {
    const scratch = await fixtures.create({ files: {}, track: 'no-git' });
    const pids = path.join(scratch.root, 'pids');
    // The command starts the stub in the background, which holds the
    // output pipe open, and ends at once. 0.1.2 waited for the pipe.
    const background = WINDOWS
      ? `start /B "" ${treeCommand(pids, 0)} & ping -n 3 127.0.0.1 > NUL`
      : `${treeCommand(pids, 0)} & sleep 2`;
    const repo = await repoWith({ lint: false, typecheck: false, test: background });
    const begun = Date.now();
    const result = await runCli(['verify'], repo.root);
    expect(Date.now() - begun).toBeLessThan(30_000);
    expect(result.code).toBe(EXIT_OK);
    expect(existsSync(path.join(pids, 'ready'))).toBe(true);
    await expectTreeGone(pids, 1);
  });
});

describe("a daemon that leaves the step's process group", () => {
  // setsid(1) is part of util-linux, so this runs on Linux only.
  it.runIf(process.platform === 'linux')(
    'holds the output pipe, but verify waits at most 2 seconds for it',
    async () => {
      const scratch = await fixtures.create({ files: {}, track: 'no-git' });
      const pids = path.join(scratch.root, 'pids');
      const repo = await repoWith({
        lint: false,
        typecheck: false,
        // The step waits (at most 10 s) until the daemon is up before it
        // ends. Without that, verify could stop the step's process group
        // before setsid has taken the daemon out of it, and on a busy
        // machine it sometimes did: the daemon never started.
        test: `setsid ${treeCommand(pids, 0)} & i=0; while [ ! -f "${path.join(pids, 'ready')}" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done; echo daemon-started`,
      });
      const begun = Date.now();
      const result = await runCli(['verify'], repo.root);
      const took = Date.now() - begun;
      await until(() => existsSync(path.join(pids, 'ready')), 10_000);
      const [daemon] = await treePids(pids);
      // It left the group, so verify cannot stop it (documented): this test does.
      if (daemon !== undefined && (await alive(daemon))) process.kill(daemon, 'SIGKILL');
      expect(result.code).toBe(EXIT_OK);
      expect(took).toBeLessThan(15_000);
      expect(daemon).toBeDefined();
    },
  );
});

describe('time limits', () => {
  it('stops a step that runs too long with its whole tree, says so, and runs the other projects', async () => {
    const scratch = await fixtures.create({ files: {}, track: 'no-git' });
    const pids = path.join(scratch.root, 'pids');
    const repo = await repoWith(
      {
        lint: false,
        typecheck: false,
        test: false,
        projects: {
          'backend/slow': { test: { command: treeCommand(pids), timeout: '8s' } },
          frontend: { test: 'echo frontend-ran' },
        },
      },
      { 'backend/slow/.keep': '', 'frontend/.keep': '' },
    );
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toMatch(/^verify: TIMED OUT at test in backend\/slow\r?\n/);
    expect(result.stdout).toContain('    test       TIMED OUT ');
    expect(result.stdout).toContain('(no result within 8s)');
    // The last lines of its output, as for a failure.
    expect(result.stdout).toContain('Last lines of test in backend/slow:\n  tree-stub level');
    expect(result.stdout).toContain(
      'Timed out: test in backend/slow gave no result within 8s, so it was stopped with every process it started.',
    );
    expect(result.stdout).toContain(
      'If it is only slow, raise verify.projects["backend/slow"].test.timeout in your firescale config.',
    );
    // Not a finding: the other project still ran and passed.
    expect(result.stdout).toContain('  in frontend:\n    test       passed   echo frontend-ran\n');
    await expectTreeGone(pids, 3);

    const json = await runCli(['verify', '--json'], repo.root);
    const report = JSON.parse(json.stdout) as Report;
    expect(report.schemaVersion).toBe(5);
    expect(report.result).toBe('failed');
    const step = report.steps.find((s) => s.project === 'backend/slow');
    expect(step?.status).toBe('timed-out');
    expect(step?.timeoutMs).toBe(8000);
    expect(step?.timeoutKey).toBe('verify.projects["backend/slow"].test.timeout');
  });

  it('rejects a time limit it cannot read, and names the key', async () => {
    const repo = await repoWith({ test: { command: 'echo x', timeout: 'soon' } });
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(2);
    expect(result.stderr).toContain(
      'verify.test.timeout must be a time such as "10m", "90s" or "1h 30m", a number of seconds, or false for no limit.',
    );
  });
});

describe('progress', () => {
  it('prints a line when each step starts and ends, and a heartbeat, on stderr only', async () => {
    const repo = await repoWith(
      { lint: 'echo lint-ran', typecheck: false, test: nodeScript('slow.js') },
      { 'slow.js': "console.log('working');\nsetTimeout(() => {}, 1500);\n" },
    );
    const result = await runCli(['verify', '--progress', '--json'], repo.root, {
      FSCALE_HEARTBEAT_MS: '400',
    });
    expect(result.code).toBe(EXIT_OK);
    // stdout stays clean JSON.
    expect((JSON.parse(result.stdout) as Report).result).toBe('ok');
    const lines = result.stderr.split(/\r?\n/).filter(Boolean);
    expect(lines[0]).toBe('[verify] check: started: fscale check');
    expect(lines[1]).toMatch(/^\[verify\] check: passed after \d/);
    expect(lines).toContain('[verify] lint: started: echo lint-ran');
    expect(lines.some((line) => /^\[verify\] lint: passed after \d/.test(line))).toBe(true);
    expect(lines).toContain(`[verify] test: started: ${nodeScript('slow.js')}`);
    expect(
      lines.some((line) =>
        /^\[verify\] test: still running after \d.*; last output: working$/.test(line),
      ),
    ).toBe(true);
    expect(lines.at(-1)).toMatch(/^\[verify\] test: passed after \d/);
  });

  it('gives a running count of tests in the heartbeat when the last output is pytest markers', async () => {
    // pytest writes one marker per test with no line end; "E" alone tells
    // a person nothing (ADR 0025).
    const repo = await repoWith(
      { lint: false, typecheck: false, test: nodeScript('fake-pytest.js') },
      {
        'fake-pytest.js': [
          "process.stdout.write('==== test session starts ====\\ncollected 9 items\\n\\ntests/test_orders.py ..F');",
          "for (let n = 0; n < 6; n++) process.stdout.write('E');",
          'setTimeout(() => process.exit(1), 3000);',
          '',
        ].join('\n'),
      },
    );
    const result = await runCli(['verify', '--progress'], repo.root, {
      FSCALE_HEARTBEAT_MS: '1500',
    });
    expect(result.stderr).toMatch(
      /\[verify\] test: still running after [\d.]+s; tests so far: 2 passed, 1 failed, 6 errors \(in tests\/test_orders\.py\)\r?\n/,
    );
    expect(result.stderr).not.toContain('last output: E');
  });

  it('shows the last line with words when the last output is only symbols', async () => {
    const repo = await repoWith(
      { lint: false, typecheck: false, test: nodeScript('dots.js') },
      {
        'dots.js':
          "console.log('compiling the schema');\nprocess.stdout.write('....');\nsetTimeout(() => {}, 1500);\n",
      },
    );
    const result = await runCli(['verify', '--progress'], repo.root, {
      FSCALE_HEARTBEAT_MS: '700',
    });
    expect(result.stderr).toMatch(
      /\[verify\] test: still running after \d.*; last output: compiling the schema\r?\n/,
    );
  });

  it('is quiet when stderr is not a terminal, unless --progress is given', async () => {
    const repo = await repoWith({ lint: 'echo lint-ran', typecheck: false, test: false });
    const quiet = await runCli(['verify'], repo.root);
    expect(quiet.code).toBe(EXIT_OK);
    expect(quiet.stderr).not.toContain('[verify]');
    const loud = await runCli(['verify', '--progress'], repo.root);
    expect(loud.stderr).toContain('[verify] lint: started: echo lint-ran');
    const off = await runCli(['verify', '--progress', '--no-progress'], repo.root);
    expect(off.stderr).not.toContain('[verify]');
  });
});

/** A port on this machine with nothing listening on it. */
async function closedPort(): Promise<number> {
  const { createServer } = await import('node:net');
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

/** A listening TCP server on localhost, closed after the test. */
async function openPort(): Promise<{ port: number; close: () => Promise<void> }> {
  const { createServer } = await import('node:net');
  const server = createServer((socket) => socket.end());
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  const port = typeof address === 'object' && address !== null ? address.port : 0;
  return { port, close: () => new Promise((resolve) => server.close(() => resolve())) };
}

/**
 * A stand-in for pytest. Run normally, it prints pytest's header and then
 * `marker` every 100 ms (until `exitAfter` ms, then the `summary` lines and
 * exit 1). Run with --maxfail=1, as verify does to find the first error, it
 * prints `first` and exits 1.
 */
function fakePytest(options: {
  marker: string;
  first?: string[];
  summary?: string[];
  exitAfter?: number;
  code?: number;
}): string {
  const { marker, first = [], summary = [], exitAfter = 30000, code = 1 } = options;
  return [
    `if (process.argv.includes('--maxfail=1')) {`,
    `  console.log(${JSON.stringify(['==== test session starts ====', 'tests/test_a.py E', '', '==== ERRORS ====', ...first].join('\n'))});`,
    '  process.exit(1);',
    '}',
    "process.stdout.write('==== test session starts ====\\ntests/test_a.py ');",
    `const timer = setInterval(() => process.stdout.write(${JSON.stringify(marker)}), 100);`,
    'setTimeout(() => {',
    '  clearInterval(timer);',
    `  console.log(${JSON.stringify(['', '==== short test summary info ====', ...summary, '==== done ===='].join('\n'))});`,
    `  process.exit(${code});`,
    `}, ${exitAfter});`,
    '',
  ].join('\n');
}

interface SetupStep extends Step {
  detail?: string;
  fix?: string;
  firstError?: string[];
  tests?: { passed: number; failed: number; errors: number };
}

function testStep(json: string): SetupStep | undefined {
  return (JSON.parse(json) as { steps: SetupStep[] }).steps.find((s) => s.name === 'test');
}

describe('setup problems seen in a test step (ADR 0035)', () => {
  const run = { lint: false, typecheck: false, test: nodeScript('fake-pytest.js') };

  it('does not name a service that is down as the cause when the output never mentions it', async () => {
    const port = await closedPort();
    const repo = await repoWith(run, {
      'fake-pytest.js': fakePytest({
        marker: 'E',
        first: [
          "E   KeyError: 'SECRET_KEY'",
          "ERROR tests/test_a.py::test_x - KeyError: 'SECRET_KEY'",
        ],
      }),
      '.env.example': `DATABASE_URL=postgresql://app:Sup3rSecret@localhost:${port}/orders_db\n`,
    });
    const result = await runCli(['verify', '--progress', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const step = testStep(result.stdout);
    expect(step?.status).toBe('failed');
    // Not the cause...
    expect(result.stderr).not.toContain(`PostgreSQL on localhost:${port} is not running`);
    expect(step?.detail).not.toContain('was not running; start it');
    // ...only listed as also not running.
    expect(step?.detail).toContain(
      `also not running, though the output does not name it: PostgreSQL on localhost:${port}`,
    );
    expect(step?.firstError).toContain("E   KeyError: 'SECRET_KEY'");
    for (const secret of ['Sup3rSecret', 'orders_db', 'postgresql://']) {
      expect(result.stdout + result.stderr).not.toContain(secret);
    }
  }, 40_000);

  it('stops a step whose every test errors as likely setup, and shows the first error, with every known service up', async () => {
    const server = await openPort();
    try {
      const repo = await repoWith(run, {
        'fake-pytest.js': fakePytest({
          marker: 'E',
          first: [
            'E   RuntimeError: TEST_SETTINGS is not set; copy tests/settings.example.py',
            'ERROR tests/test_a.py::test_x - RuntimeError: TEST_SETTINGS is not set',
          ],
        }),
        '.env.example': `REDIS_URL=redis://localhost:${server.port}/0\n`,
      });
      const before = Date.now();
      const result = await runCli(['verify', '--progress'], repo.root);
      // Stopped at 10 errors, about a second in, not after 30 s.
      expect(Date.now() - before).toBeLessThan(20_000);
      expect(result.code).toBe(EXIT_VIOLATIONS);
      expect(result.stderr).toContain(
        'LIKELY SETUP, seen while running: every test so far has errored',
      );
      expect(result.stdout).toContain('stopped early after');
      expect(result.stdout).toContain('which usually means the environment is not ready');
      expect(result.stdout).toContain('First error of test');
      expect(result.stdout).toContain(
        '  E   RuntimeError: TEST_SETTINGS is not set; copy tests/settings.example.py\n',
      );
      expect(result.stdout).not.toContain('also not running');
    } finally {
      await server.close();
    }
  }, 40_000);

  it('makes a refused local connection in the first error setup needed, with its host and port', async () => {
    const port = await closedPort();
    const repo = await repoWith(run, {
      'fake-pytest.js': fakePytest({
        marker: 'E',
        first: [
          `E   ConnectionRefusedError: [Errno 111] Connect call failed ('127.0.0.1', ${port})`,
          `ERROR tests/test_a.py::test_x - ConnectionRefusedError: [Errno 111] Connect call failed ('127.0.0.1', ${port})`,
        ],
      }),
    });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const step = testStep(result.stdout);
    expect(step?.status).toBe('setup-needed');
    expect(step?.detail).toContain(`localhost:${port}`);
    const human = await runCli(['verify'], repo.root);
    expect(human.stdout).toContain('Do not change code for it.');
  }, 40_000);

  it('makes passes plus errors that all name an unreachable service setup needed', async () => {
    const port = await closedPort();
    const lines = [1, 2, 3].map(
      (n) =>
        `ERROR tests/test_cache.py::test_${n} - RuntimeError: Cannot reach the test Redis at localhost:${port}; run docker compose up -d redis`,
    );
    const repo = await repoWith(run, {
      'fake-pytest.js': fakePytest({ marker: '...E', summary: lines, exitAfter: 1500 }),
    });
    const result = await runCli(['verify', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const step = testStep(result.stdout);
    expect(step?.status).toBe('setup-needed');
    expect(step?.detail).toContain(`every error (3) names Redis on localhost:${port}`);
    expect(step?.tests?.passed).toBeGreaterThan(0);
    const human = await runCli(['verify'], repo.root);
    expect(human.stdout).toContain('Do not change code for it.');
    expect(human.stdout).not.toContain('Fix the failure');
  }, 40_000);

  it('keeps a real code failure FAILED: assertion failures, and errors that name no service', async () => {
    const port = await closedPort();
    for (const summary of [
      [
        'FAILED tests/test_a.py::test_x - assert 1 == 2',
        `ERROR tests/test_a.py::test_y - RuntimeError: Cannot reach the test Redis at localhost:${port}`,
      ],
      ["ERROR tests/test_a.py::test_y - KeyError: 'total'"],
    ]) {
      const repo = await repoWith(run, {
        'fake-pytest.js': fakePytest({ marker: '.F.E', summary, exitAfter: 1000 }),
      });
      const result = await runCli(['verify'], repo.root);
      expect(result.code).toBe(EXIT_VIOLATIONS);
      expect(result.stdout).toContain('  test       FAILED');
      expect(result.stdout).toContain('Fix the failure, then run again');
    }
  }, 40_000);

  it('never stops a slow suite that is passing', async () => {
    const repo = await repoWith(run, {
      'fake-pytest.js': fakePytest({ marker: '.', exitAfter: 3000, code: 0 }),
    });
    const result = await runCli(['verify', '--progress', '--json'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(testStep(result.stdout)?.status).toBe('passed');
    expect(result.stderr).not.toContain('LIKELY SETUP');
  }, 40_000);

  it('does not tell a timed-out step of errors to raise its time limit', async () => {
    const repo = await repoWith(
      {
        ...run,
        test: { command: nodeScript('fake-pytest.js'), timeout: '3s' },
        stopAfterErrors: false,
      },
      {
        'fake-pytest.js': fakePytest({
          marker: 'E',
          first: ["E   KeyError: 'SECRET_KEY'"],
        }),
      },
    );
    const result = await runCli(['verify'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain('TIMED OUT');
    expect(result.stdout).toMatch(
      /Its counts \(0 passed, 0 failed, \d+ errors\) show tests erroring/,
    );
    expect(result.stdout).not.toContain('If it is only slow');
    expect(result.stdout).toContain("  E   KeyError: 'SECRET_KEY'\n");
  }, 40_000);
});

describe('commands get no input and run with CI=1', () => {
  const env = {
    'env.js':
      "const chunks = [];\nprocess.stdin.on('data', (c) => chunks.push(c));\nprocess.stdin.on('end', () => {\n  console.log(`CI=${process.env.CI ?? '(unset)'} input=${Buffer.concat(chunks).length} tty=${Boolean(process.stdin.isTTY)}`);\n  process.exit(1);\n});\n",
  };

  it('sets CI=1 and gives no input', async () => {
    const repo = await repoWith({ lint: false, typecheck: false, test: nodeScript('env.js') }, env);
    // The script fails on purpose, so its output is in the summary.
    const result = await runCli(['verify'], repo.root, { CI: undefined });
    expect(result.stdout).toContain('  CI=1 input=0 tty=false\n');
  });

  it('keeps a CI value the environment already has', async () => {
    const repo = await repoWith({ lint: false, typecheck: false, test: nodeScript('env.js') }, env);
    const result = await runCli(['verify'], repo.root, { CI: 'true' });
    expect(result.stdout).toContain('  CI=true input=0 tty=false\n');
  });

  it('leaves CI unset when verify.ci is false', async () => {
    const repo = await repoWith(
      { lint: false, typecheck: false, test: nodeScript('env.js'), ci: false },
      env,
    );
    const result = await runCli(['verify'], repo.root, { CI: undefined });
    expect(result.stdout).toContain('  CI=(unset) input=0 tty=false\n');
  });
});

describe('--jobs', () => {
  it('runs independent projects side by side, and reports them in folder order', async () => {
    // a waits for a file that only b writes: it can only pass when both run at once.
    const repo = await repoWith(
      {
        lint: false,
        typecheck: false,
        test: false,
        projects: {
          a: { test: { command: nodeScript('wait.cjs'), timeout: '20s' } },
          b: { test: nodeScript('write.cjs') },
          c: { test: 'echo c-ran' },
        },
      },
      {
        'a/wait.cjs':
          "const fs = require('node:fs');\nconst t = setInterval(() => { if (fs.existsSync('../b/done')) { clearInterval(t); console.log('a saw b'); } }, 50);\n",
        'b/write.cjs':
          "const fs = require('node:fs');\nsetTimeout(() => fs.writeFileSync('done', 'x'), 300);\n",
        'c/.keep': '',
      },
    );
    const result = await runCli(['verify', '--jobs', '3'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const places = result.stdout
      .split(/\r?\n/)
      .filter((line) => line.startsWith('  in '))
      .map((line) => line.trim());
    expect(places).toEqual(['in a:', 'in b:', 'in c:']);
  });

  it('rejects a job count that is not a whole number of 1 or more', async () => {
    const repo = await repoWith({ test: 'echo x' });
    for (const bad of ['0', 'two', '-1']) {
      const result = await runCli(['verify', `--jobs=${bad}`], repo.root);
      expect(result.code).toBe(2);
      expect(result.stderr).toContain(`--jobs needs a whole number of 1 or more, got "${bad}".`);
    }
  });
});

describe('a second verify never waits silently on another', () => {
  it('runs at once and says which other verify is running, and what it runs', async () => {
    const scratch = await fixtures.create({ files: {}, track: 'no-git' });
    const pids = path.join(scratch.root, 'pids');
    const repo = await repoWith({ lint: false, typecheck: false, test: treeCommand(pids, 0) });
    const first = startVerify(repo.root);
    await until(() => existsSync(path.join(pids, 'ready')));
    // The second run gets a quick command of its own.
    await writeFile(
      path.join(repo.root, 'firescale.config.json'),
      JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: 'echo second' },
      }),
    );
    const begun = Date.now();
    const second = await runCli(['verify'], repo.root);
    expect(Date.now() - begun).toBeLessThan(20_000);
    expect(second.code).toBe(EXIT_OK);
    expect(second.stderr).toContain(
      `fscale: another fscale verify is running in this repo (process ${first.child.pid},`,
    );
    expect(second.stderr).toContain(`running test (${treeCommand(pids, 0)})`);
    expect(second.stderr).toContain('This run does not wait for it');
    first.child.kill(WINDOWS ? undefined : 'SIGTERM');
    await first.exited;
  });
});
