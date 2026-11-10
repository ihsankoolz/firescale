import { type CliIo, run } from './cli.js';

export interface CliResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Runs the CLI in-process and captures what it prints. */
export async function runCli(argv: string[], cwd?: string): Promise<CliResult> {
  let stdout = '';
  let stderr = '';
  const io: CliIo = {
    stdout: (text) => (stdout += text),
    stderr: (text) => (stderr += text),
    ...(cwd === undefined ? {} : { cwd }),
  };
  const code = await run(argv, io);
  return { code, stdout, stderr };
}

/** Fake env file content for fixtures. Never use anything that looks like a real secret. */
export const FAKE_ENV = 'EXAMPLE=1\n';

export const ENFORCED_CONFIG = `${JSON.stringify(
  { rules: { 'core/no-committed-env': 'enforced' } },
  null,
  2,
)}\n`;
