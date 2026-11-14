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
import { readFile } from 'node:fs/promises';
import { claude } from './claude.js';
import { codex } from './codex.js';
import type { Adapter, AgentResult, Request } from './protocol.js';

export const ADAPTERS: Record<string, Adapter> = { claude, codex };

interface Ran {
  code: number;
  stdout: string;
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
    if (version.code !== 0) return 127;
    const line = version.stdout.trim().split('\n')[0] ?? '';
    const unverified =
      adapter.unverified.length > 0 ? `; unverified: ${adapter.unverified.join('; ')}` : '';
    process.stdout.write(`${adapter.name} ${line} (firescale adapter${unverified})\n`);
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
