import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { runGit } from '@firescale/sdk';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/** Success, and no enforced rule failed. */
export const EXIT_OK = 0;
/** An enforced rule has violations. */
export const EXIT_VIOLATIONS = 1;
/** Bad arguments, bad config, or an internal error. */
export const EXIT_USAGE = 2;

const REPO_ROOT = fileURLToPath(new URL('../../../', import.meta.url));

/** The `fscale` binary under test: `FSCALE_BIN`, or the debug build of this checkout. */
export const BIN =
  process.env['FSCALE_BIN'] ??
  path.join(REPO_ROOT, 'target', 'debug', process.platform === 'win32' ? 'fscale.exe' : 'fscale');

/** The built Node host, passed to the binary so tests never depend on how it is found. */
export const HOST = fileURLToPath(new URL('../dist/host.js', import.meta.url));

export const REPO = REPO_ROOT;

/** The stand-in for `node` built next to the `fscale` binary (see crates/testkit). */
export const NODE_STUB = path.join(
  path.dirname(BIN),
  process.platform === 'win32' ? 'fscale-node-stub.exe' : 'fscale-node-stub',
);

/**
 * The stand-in for a virtual environment's `python` built next to the
 * `fscale` binary (see crates/testkit/src/python_stub.rs).
 */
/** The stand-in agent of the guarded runner's tests (M27): never a real model. */
export const FAKE_AGENT = path.join(
  path.dirname(BIN),
  process.platform === 'win32' ? 'fscale-fake-agent.exe' : 'fscale-fake-agent',
);

export const PYTHON_STUB = path.join(
  path.dirname(BIN),
  process.platform === 'win32' ? 'fscale-python-stub.exe' : 'fscale-python-stub',
);

export interface RunOptions {
  /** Run this binary instead of `BIN`. */
  bin?: string;
  /** Use exactly this environment instead of `process.env` plus the defaults. */
  fullEnv?: NodeJS.ProcessEnv;
  /** Text written to the command's stdin. Stdin is always closed afterwards. */
  input?: string;
}

export interface CliResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Runs the `fscale` binary and captures what it prints. Never rejects on a non-zero exit. */
export function runCli(
  argv: string[],
  cwd?: string,
  env: Record<string, string | undefined> = {},
  options: RunOptions = {},
): Promise<CliResult> {
  const bin = options.bin ?? BIN;
  // An active virtual environment of the person running the tests must not
  // change what fscale detects; a test that wants one passes VIRTUAL_ENV.
  const base: NodeJS.ProcessEnv = options.fullEnv ?? {
    ...Object.fromEntries(
      Object.entries(process.env).filter(([key]) => key.toUpperCase() !== 'VIRTUAL_ENV'),
    ),
    FSCALE_NODE_HOST: HOST,
  };
  const removed = new Set(
    Object.entries(env).flatMap(([key, value]) => (value === undefined ? [key.toLowerCase()] : [])),
  );
  const overridden = new Set(Object.keys(env).map((key) => key.toLowerCase()));
  // An undefined value removes the key (case-insensitively, for Windows' `Path`).
  const childEnv: NodeJS.ProcessEnv = Object.fromEntries(
    Object.entries(base).filter(([key]) => !overridden.has(key.toLowerCase())),
  );
  for (const [key, value] of Object.entries(env)) {
    if (value !== undefined && !removed.has(key.toLowerCase())) childEnv[key] = value;
  }
  return new Promise((resolve, reject) => {
    const child = execFile(
      bin,
      argv,
      {
        cwd: cwd ?? REPO_ROOT,
        encoding: 'utf8',
        windowsHide: true,
        env: childEnv,
      },
      (error, stdout, stderr) => {
        if (error && typeof error.code !== 'number') {
          reject(
            new Error(`Could not run ${bin}: ${error.message}. Run \`pnpm build\` first.`, {
              cause: error,
            }),
          );
          return;
        }
        resolve({ code: error && typeof error.code === 'number' ? error.code : 0, stdout, stderr });
      },
    );
    // A command that reads stdin (the stop hook) must see its end. Closing can
    // race with a child that already exited, which is not an error here.
    child.stdin?.on('error', () => undefined);
    child.stdin?.end(options.input ?? '');
  });
}

/** Fake env file content for fixtures. Never use anything that looks like a real secret. */
export const FAKE_ENV = 'EXAMPLE=1\n';

/** Every built-in rule of the core pack, sorted. */
export const ALL_CORE_RULES = [
  'core/ci-exists',
  'core/env-example',
  'core/gitignore-basics',
  'core/no-committed-env',
  'core/no-secrets-in-source',
  'core/readme-setup',
  'core/scripts-present',
  'core/single-lockfile',
] as const;

/** Every built-in rule of the agent-ready pack, sorted. */
export const ALL_AGENT_READY_RULES = [
  'agent-ready/agent-permissions',
  'agent-ready/agent-skills',
  'agent-ready/agents-md',
  'agent-ready/commands-documented',
  'agent-ready/single-test-command',
  'agent-ready/verify-hook',
] as const;

/** The core rules other than core/no-committed-env. */
export const OTHER_CORE_RULES = ALL_CORE_RULES.filter((id) => id !== 'core/no-committed-env');

/** The rules map that runs only core/no-committed-env, at the given status. */
export function onlyEnvRules(
  status: 'enforced' | 'evaluated' | 'off' = 'enforced',
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    ...Object.fromEntries(OTHER_CORE_RULES.map((id) => [id, 'off'])),
    'core/no-committed-env': status,
    ...extra,
  };
}

/** A firescale.config.json that runs only core/no-committed-env, at the given status. */
export function onlyEnvRule(
  status: 'enforced' | 'evaluated' | 'off' = 'enforced',
  extra: Record<string, unknown> = {},
): string {
  return `${JSON.stringify({ rules: onlyEnvRules(status), ...extra }, null, 2)}\n`;
}

export const ENFORCED_CONFIG = onlyEnvRule('enforced');

/**
 * The commands a tool stand-in logged, without the `--version` probes that
 * verify's step cache makes to key a step (ADR 0036) and without the calls
 * of the base run (ADR 0037): only the commands of the head's steps. `stepcache.rs` and `verify.test.ts` test the probes.
 */
export async function stepCalls(log: string): Promise<string[]> {
  return (await readFile(log, 'utf8').catch(() => ''))
    .split(/\r?\n/)
    .filter(Boolean)
    .filter((call) => !/^\S+ (--version|version)( in |$)/.test(call))
    .filter((call) => !call.endsWith(' [base]'));
}

/** The calls the base run of blame-aware verify made (ADR 0037), without " [base]". */
export async function baseCalls(log: string): Promise<string[]> {
  return (await readFile(log, 'utf8').catch(() => ''))
    .split(/\r?\n/)
    .filter((call) => call.endsWith(' [base]'))
    .map((call) => call.slice(0, -' [base]'.length))
    .filter((call) => !/^\S+ (--version|version)( in |$)/.test(call));
}

/** Commits every file of the repo at `root`, with a fixed identity and no hooks. */
export async function commitAll(root: string, message: string): Promise<void> {
  for (const args of [
    ['add', '--all'],
    [
      '-c',
      'user.name=fixture',
      '-c',
      'user.email=fixture@example.invalid',
      '-c',
      'commit.gpgsign=false',
      'commit',
      '--quiet',
      '--no-verify',
      '-m',
      message,
    ],
  ]) {
    const result = await runGit(root, args);
    if (result.code !== 0) throw new Error(`git ${args.join(' ')} failed: ${result.stderr}`);
  }
}
