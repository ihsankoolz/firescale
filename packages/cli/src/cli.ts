import { stat } from 'node:fs/promises';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { FirescaleError } from '@firescale/sdk';
import { formatHuman, formatJson, runCheck } from './check.js';
import { findRoot, loadConfig } from './config.js';
import { explainRule, findRule } from './explain.js';
import { getVersion } from './version.js';

/** Where the CLI writes its output. Injected so tests can capture it. */
export interface CliIo {
  stdout: (text: string) => void;
  stderr: (text: string) => void;
  /** Folder the command runs in. Defaults to the process working directory. */
  cwd?: string;
}

/** Success, and no enforced rule failed. */
export const EXIT_OK = 0;
/** An enforced rule has violations. */
export const EXIT_VIOLATIONS = 1;
/** Bad arguments, bad config, or a crash. */
export const EXIT_USAGE = 2;

const HELP = `fscale - knock the scale off a repo, and prove it's clean.

Usage:
  fscale check [--rules <glob>] [--json]   Run rules and report violations
  fscale explain <rule>                    Print what a rule checks and how to fix it
  fscale --version                         Print the firescale version
  fscale --help                            Print this help

Options:
  --rules <glob>   Only run matching rules, e.g. "core/*" (comma-separated)
  --json           Print the report as JSON (see docs/check-json.md)
  --cwd <dir>      Run as if started in <dir>

Exit codes:
  0  no enforced rule failed
  1  an enforced rule has violations
  2  bad usage, bad config, or an internal error
`;

class UsageError extends FirescaleError {}

async function check(
  io: CliIo,
  cwd: string,
  flags: { json: boolean; rules: string | undefined },
  extra: readonly string[],
): Promise<number> {
  if (extra.length > 0) {
    throw new UsageError(`check takes no arguments, got "${extra.join(' ')}".`);
  }
  const root = await findRoot(cwd);
  const config = await loadConfig(root);
  const report = await runCheck({ root, config, version: getVersion(), rulesGlob: flags.rules });
  io.stdout(flags.json ? formatJson(report) : formatHuman(report));
  return report.ok ? EXIT_OK : EXIT_VIOLATIONS;
}

async function explain(io: CliIo, cwd: string, args: readonly string[]): Promise<number> {
  const [query, ...extra] = args;
  if (query === undefined || extra.length > 0) {
    throw new UsageError(
      'explain needs exactly one rule id, for example: fscale explain core/no-committed-env',
    );
  }
  const config = await loadConfig(await findRoot(cwd));
  io.stdout(await explainRule(findRule(config, query)));
  return EXIT_OK;
}

/** Runs the CLI and returns the process exit code. */
export async function run(argv: readonly string[], io: CliIo): Promise<number> {
  try {
    let parsed;
    try {
      parsed = parseArgs({
        args: [...argv],
        options: {
          version: { type: 'boolean', short: 'v' },
          help: { type: 'boolean', short: 'h' },
          json: { type: 'boolean' },
          rules: { type: 'string' },
          cwd: { type: 'string' },
        },
        allowPositionals: true,
      });
    } catch (error) {
      throw new UsageError(error instanceof Error ? error.message : String(error));
    }
    const { values, positionals } = parsed;
    const [command, ...rest] = positionals;
    const cwd = path.resolve(io.cwd ?? process.cwd(), values.cwd ?? '.');

    const cwdIsFolder = await stat(cwd).then(
      (stats) => stats.isDirectory(),
      () => false,
    );
    if (!cwdIsFolder) {
      throw new FirescaleError(`Folder "${cwd}" does not exist. Check the --cwd value.`);
    }

    if (values.version) {
      io.stdout(`${getVersion()}\n`);
      return EXIT_OK;
    }
    if (values.help || command === undefined) {
      io.stdout(HELP);
      return EXIT_OK;
    }
    switch (command) {
      case 'check':
        return await check(io, cwd, { json: values.json ?? false, rules: values.rules }, rest);
      case 'explain':
        return await explain(io, cwd, rest);
      default:
        throw new UsageError(`unknown command "${command}".`);
    }
  } catch (error) {
    if (error instanceof UsageError) {
      io.stderr(
        `fscale: ${error.message}\nRun \`fscale --help\` to see the available commands and options.\n`,
      );
    } else if (error instanceof FirescaleError) {
      io.stderr(`fscale: ${error.message}\n`);
    } else {
      const detail = error instanceof Error ? (error.stack ?? error.message) : String(error);
      io.stderr(
        `fscale: unexpected error. Please report it at https://github.com/ihsankoolz/firescale/issues\n${detail}\n`,
      );
    }
    return EXIT_USAGE;
  }
}
