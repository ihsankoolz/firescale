#!/usr/bin/env node
/**
 * The adapter protocol (ADR 0047) for the real agents:
 *
 *   node dist/cli.js <claude|codex> --version
 *   node dist/cli.js <claude|codex> auth status
 *   node dist/cli.js <claude|codex> run <request.json>
 *
 * `run` starts the agent's own CLI in the request's folder with the prompt
 * on its standard input, and prints one JSON result line.
 */
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { claude } from './claude.js';
import { codex } from './codex.js';
import type { Adapter, AgentResult, Request } from './protocol.js';

export const ADAPTERS: Record<string, Adapter> = { claude, codex };

interface Ran {
  code: number;
  stdout: string;
}

/**
 * On Windows an agent installed by npm is a `.cmd` file, which Node starts
 * only through a shell; the adapters never use one (arguments would need
 * quoting for `cmd.exe`). Names the file, so the error can say so.
 */
function windowsShim(program: string): string | undefined {
  if (process.platform !== 'win32') return undefined;
  const dirs = (process.env['PATH'] ?? process.env['Path'] ?? '').split(path.delimiter);
  for (const dir of dirs.filter(Boolean)) {
    for (const ext of ['.cmd', '.bat']) {
      const candidate = path.join(dir, `${program}${ext}`);
      if (existsSync(candidate)) return candidate;
    }
  }
  return undefined;
}

function run(program: string, args: string[], cwd: string, stdin: string): Promise<Ran> {
  return new Promise((resolve) => {
    const child = spawn(program, args, { cwd, stdio: ['pipe', 'pipe', 'ignore'] });
    let stdout = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk: string) => {
      stdout += chunk;
    });
    child.on('error', () => resolve({ code: 127, stdout: '' }));
    child.on('close', (code) => resolve({ code: code ?? 1, stdout }));
    child.stdin.on('error', () => undefined);
    child.stdin.end(stdin);
  });
}

async function main(argv: string[]): Promise<number> {
  const [name, ...rest] = argv;
  const adapter = name === undefined ? undefined : ADAPTERS[name];
  if (!adapter) {
    process.stderr.write(
      `firescale-agents: unknown agent "${name ?? ''}". Known: ${Object.keys(ADAPTERS).join(', ')}.\n`,
    );
    return 2;
  }
  if (rest.length === 1 && rest[0] === '--version') {
    const version = await run(adapter.program, adapter.versionArgs, process.cwd(), '');
    if (version.code !== 0) {
      const shim = version.code === 127 ? windowsShim(adapter.program) : undefined;
      process.stderr.write(
        shim
          ? `${adapter.program} is installed as ${shim}, an npm .cmd file, which firescale does not start (it would need a shell). Install ${adapter.program} as an .exe (for Claude Code, its native installer).\n`
          : `${adapter.program} did not answer ${adapter.versionArgs.join(' ')}: is it installed and on the PATH?\n`,
      );
      return 127;
    }
    const line = version.stdout.trim().split('\n')[0] ?? '';
    // `fscale fix --agent` prints this under "Agent:", so what was not
    // confirmed from the agent itself is in every run's output.
    const unverified = adapter.unverified.map(
      (item) => `unverified (ADR 0048, check by hand): ${item}\n`,
    );
    process.stdout.write(`${adapter.name} ${line}\n${unverified.join('')}`);
    return 0;
  }
  if (rest.length === 2 && rest[0] === 'auth' && rest[1] === 'status') {
    const auth = await run(adapter.program, adapter.authArgs, process.cwd(), '');
    return auth.code === 0 ? 0 : 1;
  }
  if (rest.length === 2 && rest[0] === 'run' && rest[1] !== undefined) {
    const request = JSON.parse(await readFile(rest[1], 'utf8')) as Request;
    const invocation = adapter.invocation(request);
    const ran = await run(adapter.program, invocation.args, request.cwd, invocation.stdin);
    const result: AgentResult =
      ran.code === 127 && ran.stdout === ''
        ? { ok: false, error: `${adapter.program} could not be started` }
        : adapter.parse(ran.stdout, ran.code);
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return 0;
  }
  process.stderr.write(
    'usage: firescale-agents <agent> --version | auth status | run <request.json>\n',
  );
  return 2;
}

process.exitCode = await main(process.argv.slice(2));
