import { execFile } from 'node:child_process';
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

export interface RunOptions {
  /** Run this binary instead of `BIN`. */
  bin?: string;
  /** Use exactly this environment instead of `process.env` plus the defaults. */
  fullEnv?: NodeJS.ProcessEnv;
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
  const base: NodeJS.ProcessEnv = options.fullEnv ?? { ...process.env, FSCALE_NODE_HOST: HOST };
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
    execFile(
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
