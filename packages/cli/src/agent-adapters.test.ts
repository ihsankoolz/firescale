import { execFile } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { BIN, commitAll, EXIT_OK, EXIT_USAGE, FAKE_AGENT, runCli } from './test-helpers.js';

// The Claude Code and Codex adapters (M28, ADR 0048), end to end through
// `fscale fix --agent`, with stand-ins for the agents' own CLIs on the PATH
// (crates/testkit/src/fake_cli.rs). No real agent, no model.

const fixtures = fixtureSet();
// Folders for the stand-ins, outside the fixture repos (which must stay
// clean for the pre-flight), removed after each test (review of M28).
const scratch: string[] = [];
afterEach(async () => {
  await fixtures.cleanup();
  await Promise.all(
    scratch.splice(0).map((dir) => rm(dir, { recursive: true, force: true, maxRetries: 5 })),
  );
});

async function scratchDir(prefix: string): Promise<string> {
  const dir = await mkdtemp(path.join(os.tmpdir(), prefix));
  scratch.push(dir);
  return dir;
}

const EXE = process.platform === 'win32' ? '.exe' : '';
const FAKE_CLI = path.join(path.dirname(BIN), `fscale-fake-cli${EXE}`);
const AGENTS = fileURLToPath(new URL('../../agents/dist/cli.js', import.meta.url));

// Built from parts, so this repo's own core/env-example does not read the
// fixture's code as code of this repo.
const APP = `export const url = ${['process', 'env', 'API_URL'].join('.')};\n`;
const RULE = 'core/env-example';

async function git(root: string, ...args: string[]): Promise<string> {
  const result = await runGit(root, ['-c', 'commit.gpgsign=false', ...args]);
  if (result.code !== 0) throw new Error(`git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

async function repo() {
  const fixture = await fixtures.create({
    files: {
      'firescale.config.json': `${JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: false },
        agent: { timeoutSeconds: 30, maxTurns: 8, maxCostUsd: 0.75 },
      })}\n`,
      '.gitignore': '.firescale/\n',
      'package.json': '{ "name": "demo", "type": "module" }\n',
      '.env.example': 'NODE_ENV=\n',
      'src/app.js': APP,
    },
    track: 'no-git',
  });
  const root = fixture.root;
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await git(root, 'config', 'user.name', 'Owner');
  await git(root, 'config', 'user.email', 'owner@example.invalid');
  await git(root, 'config', 'commit.gpgsign', 'false');
  await commitAll(root, 'base');
  return root;
}

interface Stand {
  /** What the stand-in prints on a run. */
  output?: string;
  /** Files it writes, as the agent's edits. */
  write?: Record<string, string>;
  exit?: number;
  auth?: boolean;
  /**
   * `--version` fails. The stand-in is always on the PATH first: a test
   * that left it off would find a real agent installed on the machine.
   */
  broken?: boolean;
}

interface Logged {
  args: string[];
  stdin: string;
  cwd: string;
}

/** A folder with the stand-in under the agent's name, and the run's env. */
async function standIn(_root: string, agent: 'claude' | 'codex', stand: Stand) {
  const dir = await scratchDir('fscale-cli-');
  const bin = path.join(dir, 'bin');
  await mkdir(bin, { recursive: true });
  await copyFile(FAKE_CLI, path.join(bin, `${agent}${EXE}`));
  const output = path.join(dir, 'output.txt');
  const write = path.join(dir, 'write.json');
  const log = path.join(dir, 'log.jsonl');
  await writeFile(output, stand.output ?? '');
  await writeFile(write, JSON.stringify(stand.write ?? {}));
  const env: Record<string, string | undefined> = {
    PATH: `${bin}${path.delimiter}${process.env['PATH'] ?? process.env['Path'] ?? ''}`,
    FSCALE_AGENTS: AGENTS,
    FSCALE_FAKE_CLI_LOG: log,
    FSCALE_FAKE_CLI_OUTPUT: output,
    FSCALE_FAKE_CLI_WRITE: write,
    FSCALE_FAKE_CLI_EXIT: String(stand.exit ?? 0),
    FSCALE_FAKE_CLI_AUTH: stand.auth === false ? '0' : '1',
    FSCALE_FAKE_CLI_BROKEN: stand.broken ? '1' : '0',
    GITHUB_ACTIONS: undefined,
  };
  const calls = async (): Promise<Logged[]> =>
    (await readFile(log, 'utf8').catch(() => ''))
      .split('\n')
      .filter(Boolean)
      .map((text) => JSON.parse(text) as Logged);
  return { env, calls };
}

const ledgerLines = async (root: string) =>
  (await readFile(path.join(root, '.firescale', 'ledger.jsonl'), 'utf8').catch(() => ''))
    .split('\n')
    .filter(Boolean)
    .map((text) => JSON.parse(text) as Record<string, unknown>);

const CLAUDE_OK = `${JSON.stringify({
  type: 'result',
  subtype: 'success',
  is_error: false,
  total_cost_usd: 0.031,
  num_turns: 2,
  result: 'Added API_URL to .env.example.',
})}\n`;

const CODEX_OK = `${[
  { type: 'thread.started', thread_id: 'x' },
  { type: 'turn.started' },
  { type: 'turn.completed', usage: { input_tokens: 1, output_tokens: 1 } },
]
  .map((event) => JSON.stringify(event))
  .join('\n')}\n`;

describe('fscale fix --agent claude', () => {
  it('runs claude -p in the worktree with the pinned flags, and records its cost', async () => {
    const root = await repo();
    const { env, calls } = await standIn(root, 'claude', {
      output: CLAUDE_OK,
      write: { '.env.example': 'NODE_ENV=\nAPI_URL=\n' },
    });
    const result = await runCli(['fix', '--agent', 'claude', '--rule', RULE], root, env);
    expect(result.code, result.stderr + result.stdout).toBe(EXIT_OK);
    // What was not confirmed from Claude Code itself is in the output.
    expect(result.stdout).toContain('Agent: claude 9.9.9 (stand-in)');
    expect(result.stdout).toContain('unverified (ADR 0048, check by hand): the fields');

    const logged = await calls();
    expect(logged.map((call) => call.args[0])).toEqual(['--version', 'auth', '-p']);
    expect(logged[1]?.args).toEqual(['auth', 'status']);
    const run = logged[2];
    expect(run?.args.slice(0, 13)).toEqual([
      '-p',
      '--output-format',
      'json',
      '--max-turns',
      '8',
      '--max-budget-usd',
      '0.75',
      '--permission-mode',
      'dontAsk',
      '--restricted',
      '--no-session-persistence',
      '--settings',
      '{"disableAllHooks":true}',
    ]);
    expect(run?.args).toContain('--strict-mcp-config');
    expect(run?.stdin).toContain(RULE);
    expect(path.resolve(run?.cwd ?? root)).not.toBe(path.resolve(root));

    const [entry] = await ledgerLines(root);
    expect(entry).toMatchObject({ agent: 'claude', result: 'pass', costUsd: 0.031, turns: 2 });
    const summary = await runCli(['ledger'], root);
    expect(summary.stdout).toContain('claude');
    expect(summary.stdout).toContain('$0.03');
  });

  it('fails the run when Claude Code reports an error, and keeps the cost it reported', async () => {
    const root = await repo();
    const { env } = await standIn(root, 'claude', {
      output: JSON.stringify({ subtype: 'error_max_turns', is_error: true, total_cost_usd: 0.2 }),
      exit: 1,
    });
    const result = await runCli(['fix', '--agent', 'claude', '--rule', RULE], root, env);
    expect(result.code).not.toBe(EXIT_OK);
    const [entry] = await ledgerLines(root);
    expect(entry).toMatchObject({ result: 'fail', reason: 'agent-error', costUsd: 0.2 });
  });

  it('stops before the run when Claude Code does not answer or is signed out', async () => {
    const root = await repo();
    const broken = await standIn(root, 'claude', { broken: true });
    const none = await runCli(['fix', '--agent', 'claude', '--rule', RULE], root, broken.env);
    expect(none.code).toBe(EXIT_USAGE);
    expect(none.stderr).toContain('the claude agent did not answer --version');
    expect((await broken.calls()).map((call) => call.args)).toEqual([['--version']]);
    const out = await standIn(root, 'claude', { auth: false });
    const signedOut = await runCli(['fix', '--agent', 'claude', '--rule', RULE], root, out.env);
    expect(signedOut.code).toBe(EXIT_USAGE);
    expect(signedOut.stderr).toContain('is not signed in');
    expect((await out.calls()).map((call) => call.args)).toEqual([
      ['--version'],
      ['auth', 'status'],
    ]);
    expect(await ledgerLines(root)).toEqual([]);
  });

  it('finds the adapters beside the binary without FSCALE_AGENTS', async () => {
    const root = await repo();
    const { env, calls } = await standIn(root, 'claude', {
      output: CLAUDE_OK,
      write: { '.env.example': 'NODE_ENV=\nAPI_URL=\n' },
    });
    const result = await runCli(['fix', '--agent', 'claude', '--rule', RULE], root, {
      ...env,
      FSCALE_AGENTS: undefined,
    });
    expect(result.code, result.stderr).toBe(EXIT_OK);
    expect((await calls()).length).toBe(3);
  });
});

describe('fscale fix --agent codex', () => {
  it('runs codex exec in the worktree with the pinned flags; no cost is reported', async () => {
    const root = await repo();
    const { env, calls } = await standIn(root, 'codex', {
      output: CODEX_OK,
      write: { '.env.example': 'NODE_ENV=\nAPI_URL=\n' },
    });
    const result = await runCli(['fix', '--agent', 'codex', '--rule', RULE], root, env);
    expect(result.code, result.stderr + result.stdout).toBe(EXIT_OK);
    const logged = await calls();
    expect(logged.map((call) => call.args.slice(0, 2).join(' '))).toEqual([
      '--version',
      'login status',
      'exec --json',
    ]);
    expect(logged[2]?.args).toEqual([
      'exec',
      '--json',
      '--ephemeral',
      '--ignore-user-config',
      '--skip-git-repo-check',
      '--sandbox',
      'workspace-write',
      '-c',
      'sandbox_workspace_write.network_access=false',
      '-',
    ]);
    expect(logged[2]?.stdin).toContain(RULE);
    const [entry] = await ledgerLines(root);
    expect(entry).toMatchObject({ agent: 'codex', result: 'pass', turns: 1 });
    expect(entry).not.toHaveProperty('costUsd');
  });
});

describe("fscale fix --agent codex and the repo's own settings", () => {
  it("refuses Codex when the repo's settings deny anything, since Codex cannot be told", async () => {
    const root = await repo();
    await mkdir(path.join(root, '.claude'), { recursive: true });
    await writeFile(
      path.join(root, '.claude', 'settings.json'),
      JSON.stringify({ permissions: { deny: ['Bash(curl *)', 'Read(config/private/**)'] } }),
    );
    await commitAll(root, 'settings');
    const { env, calls } = await standIn(root, 'codex', { output: CODEX_OK });
    const result = await runCli(['fix', '--agent', 'codex', '--rule', RULE], root, env);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('Codex cannot be told to');
    expect(result.stderr).toContain('`Bash(curl *)`');
    // Pre-flight only: Codex never ran.
    expect((await calls()).every((call) => call.args[0] !== 'exec')).toBe(true);
    expect(await ledgerLines(root)).toEqual([]);
  });

  it('warns on every Codex run that its reads and commands are not limited', async () => {
    const root = await repo();
    const { env } = await standIn(root, 'codex', {
      output: CODEX_OK,
      write: { '.env.example': 'NODE_ENV=\nAPI_URL=\n' },
    });
    const result = await runCli(['fix', '--agent', 'codex', '--rule', RULE], root, env);
    expect(result.code, result.stderr).toBe(EXIT_OK);
    expect(result.stdout).toContain('Warning: Codex runs with a shell');
  });

  it('fails a Codex run that completed no turn', async () => {
    const root = await repo();
    const { env } = await standIn(root, 'codex', {
      output: '',
      write: { '.env.example': 'NODE_ENV=\nAPI_URL=\n' },
    });
    const result = await runCli(['fix', '--agent', 'codex', '--rule', RULE], root, env);
    expect(result.code).not.toBe(EXIT_OK);
    const [entry] = await ledgerLines(root);
    expect(entry).toMatchObject({ result: 'fail', reason: 'agent-error' });
  });
});

describe('the adapter protocol entry point', () => {
  it('says which agent version it found, and what is unverified', async () => {
    const root = await repo();
    const { env } = await standIn(root, 'codex', {});
    const childEnv = Object.fromEntries(
      Object.entries({ ...process.env, ...env }).filter(([, value]) => value !== undefined),
    ) as NodeJS.ProcessEnv;
    // On Windows the PATH key may be spelled `Path`; one spelling only.
    if (process.platform === 'win32') {
      delete childEnv['Path'];
      childEnv['PATH'] = env['PATH'];
    }
    const stdout = await new Promise<string>((resolve, reject) => {
      execFile(process.execPath, [AGENTS, 'codex', '--version'], { env: childEnv }, (error, out) =>
        error ? reject(error) : resolve(out),
      );
    });
    expect(stdout).toContain('codex 9.9.9 (stand-in)\n');
    expect(stdout).toContain('unverified (ADR 0048, check by hand): `--sandbox workspace-write`');
  });
});

describe('fscale ledger after a run', () => {
  it('shows the cost a fake-agent run reported', async () => {
    const root = await repo();
    const dir = await scratchDir('fscale-fake-');
    const scenario = path.join(dir, 'scenario.json');
    await writeFile(
      scenario,
      JSON.stringify({
        write: { '.env.example': 'NODE_ENV=\nAPI_URL=\n' },
        result: { ok: true, costUsd: 0.42, turns: 5 },
      }),
    );
    const result = await runCli(['fix', '--agent', 'fake', '--rule', RULE], root, {
      FSCALE_FAKE_AGENT: FAKE_AGENT,
      FSCALE_FAKE_AGENT_SCENARIO: scenario,
      GITHUB_ACTIONS: undefined,
    });
    expect(result.code, result.stderr + result.stdout).toBe(EXIT_OK);
    const human = await runCli(['ledger'], root);
    expect(human.stdout).toContain('1 run, 1 passed (100%), cost $0.42');
    expect(human.stdout).toMatch(/fake\s+1\s+1\s+100%\s+\$0\.42/);
    const json = JSON.parse((await runCli(['ledger', '--json'], root)).stdout) as {
      costUsd: number;
      byAgent: { name: string; costUsd: number }[];
    };
    expect(json.costUsd).toBe(0.42);
    expect(json.byAgent).toEqual([expect.objectContaining({ name: 'fake', costUsd: 0.42 })]);
  });
});
