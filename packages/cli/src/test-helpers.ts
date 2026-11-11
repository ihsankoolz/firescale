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

export interface CliResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Runs the `fscale` binary and captures what it prints. Never rejects on a non-zero exit. */
export function runCli(
  argv: string[],
  cwd?: string,
  env: Record<string, string> = {},
): Promise<CliResult> {
  return new Promise((resolve, reject) => {
    execFile(
      BIN,
      argv,
      {
        cwd: cwd ?? REPO_ROOT,
        encoding: 'utf8',
        windowsHide: true,
        env: { ...process.env, FSCALE_NODE_HOST: HOST, ...env },
      },
      (error, stdout, stderr) => {
        if (error && typeof error.code !== 'number') {
          reject(
            new Error(`Could not run ${BIN}: ${error.message}. Run \`pnpm build\` first.`, {
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

export const ENFORCED_CONFIG = `${JSON.stringify(
  { rules: { 'core/no-committed-env': 'enforced' } },
  null,
  2,
)}\n`;
