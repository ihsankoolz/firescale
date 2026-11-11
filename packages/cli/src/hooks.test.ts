import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

/** The exit code that tells Claude Code and Codex "do not stop yet". */
const EXIT_BLOCK = 2;

const HOOK_FILES = {
  claude: '.claude/settings.json',
  codex: '.codex/hooks.json',
  cursor: '.cursor/hooks.json',
} as const;

const read = (root: string, file: string): Promise<string> =>
  readFile(path.join(root, ...file.split('/')), 'utf8');

function verifyConfig(test: string): Record<string, string> {
  return {
    'firescale.config.json': JSON.stringify({
      verify: { lint: false, typecheck: false, test: 'node test.js' },
    }),
    'test.js': test,
  };
}

const FAILING = verifyConfig("console.error('1 test failed');\nprocess.exit(1);\n");
const PASSING = verifyConfig("console.log('ok');\n");

const stop = (root: string, agent: string, input: object, env: Record<string, string> = {}) =>
  runCli(['hooks', 'run', '--agent', agent], root, env, { input: JSON.stringify(input) });

describe('fscale hooks install', () => {
  it('writes a hook file for each of the three agents', async () => {
    const repo = await fixtures.create();
    const result = await runCli(['hooks', 'install', '--all'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot('stdout');
    expect(result.stderr).toMatchSnapshot('stderr');
    for (const [agent, file] of Object.entries(HOOK_FILES)) {
      expect(await read(repo.root, file)).toMatchSnapshot(agent);
    }
  });

  it('changes nothing when run again', async () => {
    const repo = await fixtures.create();
    await runCli(['hooks', 'install', '--all'], repo.root);
    const before = await Promise.all(Object.values(HOOK_FILES).map((f) => read(repo.root, f)));
    const again = await runCli(['hooks', 'install', '--all'], repo.root);
    expect(again.code).toBe(EXIT_OK);
    expect(again.stdout).toContain('already installed .claude/settings.json');
    const after = await Promise.all(Object.values(HOOK_FILES).map((f) => read(repo.root, f)));
    expect(after).toEqual(before);
  });

  it('installs for one agent, keeps existing settings, and honours --bin', async () => {
    const repo = await fixtures.create({
      files: { '.claude/settings.json': '{"permissions":{"deny":["Read(./.env)"]}}\n' },
    });
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude', '--bin', 'npx firescale'],
      repo.root,
    );
    expect(result.code).toBe(EXIT_OK);
    const settings = JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
      permissions: unknown;
      hooks: { Stop: { hooks: { command: string }[] }[] };
    };
    expect(settings.permissions).toEqual({ deny: ['Read(./.env)'] });
    expect(settings.hooks.Stop[0]?.hooks[0]?.command).toBe(
      'npx firescale hooks run --agent claude',
    );
    await expect(read(repo.root, HOOK_FILES.codex)).rejects.toThrow();
  });

  it('explains bad usage', async () => {
    const repo = await fixtures.create();
    const none = await runCli(['hooks', 'install'], repo.root);
    expect(none.code).toBe(EXIT_USAGE);
    expect(none.stderr).toContain('needs --agent <claude|codex|cursor> or --all');
    const unknown = await runCli(['hooks', 'install', '--agent', 'vim'], repo.root);
    expect(unknown.code).toBe(EXIT_USAGE);
    expect(unknown.stderr).toContain('Supported agents: claude, codex, cursor.');
    const both = await runCli(['hooks', 'install', '--agent', 'claude', '--all'], repo.root);
    expect(both.code).toBe(EXIT_USAGE);
    const action = await runCli(['hooks'], repo.root);
    expect(action.code).toBe(EXIT_USAGE);
  });

  it('refuses to overwrite a settings file that is not valid JSON', async () => {
    const repo = await fixtures.create({ files: { '.claude/settings.json': '{ nope' } });
    const result = await runCli(['hooks', 'install', '--agent', 'claude'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('.claude/settings.json is not valid JSON');
    expect(await read(repo.root, HOOK_FILES.claude)).toBe('{ nope');
  });
});

describe('fscale hooks run (the stop hook)', () => {
  it('lets the agent stop, silently, when verify passes', async () => {
    const repo = await fixtures.create({ files: PASSING });
    const result = await stop(repo.root, 'claude', { session_id: 's1', stop_hook_active: false });
    expect(result).toEqual({ code: EXIT_OK, stdout: '', stderr: '' });
  });

  it('blocks Claude Code with exit code 2 and the verify summary on stderr', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const result = await stop(repo.root, 'claude', { session_id: 's1', stop_hook_active: false });
    expect(result.code).toBe(EXIT_BLOCK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatchSnapshot();
  });

  it('loop guard: blocks three times in a row, then lets the agent stop', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const first = await stop(repo.root, 'codex', { session_id: 'loop', stop_hook_active: false });
    expect(first.code).toBe(EXIT_BLOCK);
    expect(first.stderr).toContain('attempt 1 of 3');
    for (const attempt of [2, 3]) {
      const next = await stop(repo.root, 'codex', { session_id: 'loop', stop_hook_active: true });
      expect(next.code).toBe(EXIT_BLOCK);
      expect(next.stderr).toContain(`attempt ${String(attempt)} of 3`);
    }
    const released = await stop(repo.root, 'codex', { session_id: 'loop', stop_hook_active: true });
    expect(released.code).toBe(EXIT_OK);
    expect(released.stderr).toContain('verify still fails after 3 attempts');
    expect(released.stderr).toContain('allowed to stop');
    // The counter lives in .firescale/, which ignores itself, so nothing shows up in git.
    expect(await read(repo.root, '.firescale/.gitignore')).toContain('*');
  });

  it('loop guard: FSCALE_HOOK_MAX_BLOCKS changes the cap, and sessions do not share a count', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const env = { FSCALE_HOOK_MAX_BLOCKS: '1' };
    const a1 = await stop(repo.root, 'claude', { session_id: 'a', stop_hook_active: false }, env);
    expect(a1.code).toBe(EXIT_BLOCK);
    expect(a1.stderr).toContain('attempt 1 of 1');
    const b1 = await stop(repo.root, 'claude', { session_id: 'b', stop_hook_active: false }, env);
    expect(b1.code).toBe(EXIT_BLOCK);
    const a2 = await stop(repo.root, 'claude', { session_id: 'a', stop_hook_active: true }, env);
    expect(a2.code).toBe(EXIT_OK);
    expect(a2.stderr).toContain('allowed to stop');
  });

  it('answers Cursor with a follow-up message on stdout', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const blocked = await stop(repo.root, 'cursor', { conversation_id: 'c1', loop_count: 0 });
    expect(blocked.code).toBe(EXIT_OK);
    const reply = JSON.parse(blocked.stdout) as { followup_message: string };
    expect(reply.followup_message).toContain('verify: FAILED at test');
    const capped = await stop(repo.root, 'cursor', { conversation_id: 'c1', loop_count: 3 });
    expect(capped.code).toBe(EXIT_OK);
    expect(capped.stdout).toBe('{}\n');
  });

  it('does not trap the agent when verify itself cannot run', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{ nope' } });
    const result = await stop(repo.root, 'claude', { session_id: 's1' });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('the stop hook could not run verify');
  });

  it('survives input that is not JSON, and needs --agent', async () => {
    const repo = await fixtures.create({ files: PASSING });
    const odd = await runCli(['hooks', 'run', '--agent', 'claude'], repo.root, {}, { input: '??' });
    expect(odd.code).toBe(EXIT_OK);
    const missing = await runCli(['hooks', 'run'], repo.root);
    expect(missing.code).toBe(EXIT_USAGE);
  });
});
