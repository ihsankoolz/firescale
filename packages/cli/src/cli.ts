import { parseArgs } from 'node:util';
import { getVersion } from './version.js';

/** Where the CLI writes its output. Injected so tests can capture it. */
export interface CliIo {
  stdout: (text: string) => void;
  stderr: (text: string) => void;
}

export const EXIT_OK = 0;
export const EXIT_USAGE = 2;

const HELP = `fscale - knock the scale off a repo, and prove it's clean.

Usage:
  fscale [options]

Options:
  -v, --version   Print the firescale version
  -h, --help      Print this help
`;

/** Runs the CLI and returns the process exit code. */
export async function run(argv: readonly string[], io: CliIo): Promise<number> {
  let values: { version?: boolean; help?: boolean };
  try {
    ({ values } = parseArgs({
      args: [...argv],
      options: {
        version: { type: 'boolean', short: 'v' },
        help: { type: 'boolean', short: 'h' },
      },
      allowPositionals: false,
    }));
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    io.stderr(`fscale: ${reason}\nRun \`fscale --help\` to see the available options.\n`);
    return EXIT_USAGE;
  }

  if (values.version) {
    io.stdout(`${getVersion()}\n`);
    return EXIT_OK;
  }

  io.stdout(HELP);
  return EXIT_OK;
}
