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

  it('prints no note that the Cursor format is unchecked: ADR 0027 confirmed it', async () => {
    const repo = await fixtures.create();
    const result = await runCli(['hooks', 'install', '--agent', 'cursor'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe(
      'installed         .cursor/hooks.json\n\nEach agent now runs `fscale hooks run --agent <name>` before it stops, and keeps working while `fscale verify` fails.\n',
    );
    expect(result.stderr).toBe('');
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
      files: { '.claude/settings.json': '{\n"permissions":{"deny":["Read(./.env)"]}}\n' },
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

  it('adds the stop hook when the same command only appears somewhere else', async () => {
    const repo = await fixtures.create({
      files: {
        '.claude/settings.json': JSON.stringify(
          { permissions: { allow: ['Bash(fscale hooks run --agent claude)'] } },
          null,
          2,
        ),
      },
    });
    const result = await runCli(['hooks', 'install', '--agent', 'claude'], repo.root);
    expect(result.stdout).toContain('installed         .claude/settings.json');
    const settings = JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
      hooks: { Stop: { hooks: { command: string }[] }[] };
    };
    expect(settings.hooks.Stop[0]?.hooks[0]?.command).toBe('fscale hooks run --agent claude');
  });

  it("keeps every line of a person's file and adds only the hook's lines", async () => {
    const files = {
      // Four spaces, a number written 1e3, odd spacing and key order.
      [HOOK_FILES.claude]:
        '{\n    "zeta" :   1e3,\n    "alpha":[ 1,2 ],\n\n    "permissions": {\n        "deny": [\n            "Read(./.env)"\n        ]\n    }\n}\n',
      // Tabs and CRLF, no newline at the end, a stop hook of their own.
      [HOOK_FILES.codex]:
        '{\r\n\t"hooks": {\r\n\t\t"Stop": [\r\n\t\t\t{ "hooks": [ { "type": "command", "command": "echo bye" } ] }\r\n\t\t]\r\n\t}\r\n}',
      // No version, another event's hook.
      [HOOK_FILES.cursor]:
        '{\n   "hooks": {\n      "afterFileEdit": [ { "command": "fmt" } ]\n   }\n}\n',
    };
    const repo = await fixtures.create({ files });
    const result = await runCli(['hooks', 'install', '--all'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const written: string[] = [];
    for (const [file, before] of Object.entries(files)) {
      const after = await read(repo.root, file);
      // Each line a person wrote is there, with its own line ending, in order.
      const afterLines = after.split(/(?<=\n)/);
      let at = 0;
      for (const line of before.split(/(?<=\n)/)) {
        at = afterLines.indexOf(line, at) + 1;
        expect(at, `${file}: ${JSON.stringify(line)} in ${JSON.stringify(after)}`).toBeGreaterThan(
          0,
        );
      }
      expect(after).toContain('fscale hooks run --agent');
      expect(after.includes('\r\n')).toBe(before.includes('\r\n'));
      expect(after.replaceAll('\r\n', '').includes('\r')).toBe(false);
      written.push(after);
    }
    expect(written[0]).toContain('"zeta" :   1e3,');
    const again = await runCli(['hooks', 'install', '--all'], repo.root);
    expect(again.code).toBe(EXIT_OK);
    expect(await Promise.all(Object.keys(files).map((f) => read(repo.root, f)))).toEqual(written);
  });

  it('refuses a file written on one line, and says what to add by hand', async () => {
    const minified = '{"permissions":{"deny":["Read(./.env)"]}}\n';
    const repo = await fixtures.create({ files: { '.claude/settings.json': minified } });
    const result = await runCli(['hooks', 'install', '--agent', 'claude'], repo.root);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatchSnapshot('stderr');
    expect(await read(repo.root, HOOK_FILES.claude)).toBe(minified);
  });

  it('--all tries every file: it writes the ones it can, and says which it refused', async () => {
    const cases = [
      // The first file is refused: the two after it must still be written.
      { refused: ['claude'], content: '{ nope' },
      // The middle file is refused, for the other reason.
      { refused: ['codex'], content: '{"hooks":{}}\n' },
      // Two refusals: each gets its own reason.
      { refused: ['claude', 'cursor'], content: '{"a":1}\n' },
    ] as const;
    for (const { refused, content } of cases) {
      const label = refused.join(' and ');
      const files = Object.fromEntries(refused.map((agent) => [HOOK_FILES[agent], content]));
      const repo = await fixtures.create({ files });
      const result = await runCli(['hooks', 'install', '--all'], repo.root);
      expect(result.code, label).toBe(EXIT_USAGE);
      expect(result.stdout).toMatchSnapshot(`${label}: stdout`);
      expect(result.stderr).toMatchSnapshot(`${label}: stderr`);
      for (const [agent, file] of Object.entries(HOOK_FILES)) {
        const text = await read(repo.root, file);
        if ((refused as readonly string[]).includes(agent)) {
          expect(text, `${label}: ${file}`).toBe(content);
        } else {
          expect(text, `${label}: ${file}`).toContain(`fscale hooks run --agent ${agent}`);
        }
      }
    }
  });

  it('--all with every file refused says so, and claims no hook', async () => {
    const files = Object.fromEntries(Object.values(HOOK_FILES).map((file) => [file, '{"a":1}\n']));
    const repo = await fixtures.create({ files });
    const result = await runCli(['hooks', 'install', '--all'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stdout).toBe(
      'refused           .claude/settings.json\nrefused           .codex/hooks.json\nrefused           .cursor/hooks.json\n\nNot every hook is installed. Refused and left as it was: .claude/settings.json, .codex/hooks.json, .cursor/hooks.json. The reason, and what to add by hand, is printed above this list (on stderr). Do that, then run this again.\n',
    );
    expect(result.stdout).not.toContain('The hook is in');
    expect(result.stderr.match(/^fscale: /gm)).toHaveLength(3);
    for (const file of Object.values(HOOK_FILES)) {
      expect(await read(repo.root, file)).toBe('{"a":1}\n');
    }
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

  it('survives input that is not JSON', async () => {
    const repo = await fixtures.create({ files: PASSING });
    const odd = await runCli(['hooks', 'run', '--agent', 'claude'], repo.root, {}, { input: '??' });
    expect(odd.code).toBe(EXIT_OK);
  });

  it.each([
    ['a payload without stop_hook_active', 'claude', '{"session_id":"quiet"}'],
    ['input that is not JSON', 'codex', '??'],
    ['no input at all', 'claude', ''],
  ])('loop guard: still caps with %s', async (_name, agent, input) => {
    const repo = await fixtures.create({ files: FAILING });
    const codes: number[] = [];
    for (let i = 0; i < 4; i += 1) {
      const result = await runCli(['hooks', 'run', '--agent', agent], repo.root, {}, { input });
      codes.push(result.code);
    }
    expect(codes).toEqual([EXIT_BLOCK, EXIT_BLOCK, EXIT_BLOCK, EXIT_OK]);
  });

  it('loop guard: caps Cursor on its own count when Cursor sends no loop_count', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const replies: string[] = [];
    for (let i = 0; i < 4; i += 1) {
      replies.push((await stop(repo.root, 'cursor', { conversation_id: 'c2' })).stdout);
    }
    expect(replies.slice(0, 3).every((reply) => reply.includes('followup_message'))).toBe(true);
    expect(replies[3]).toBe('{}\n');
  });

  it('never answers its own errors with exit code 2, which would block the agent', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const typo = await runCli(['hooks', 'run', '--agent', 'claud'], repo.root);
    expect(typo.code).toBe(1);
    expect(typo.stderr).toContain('unknown agent "claud"');
    const missing = await runCli(['hooks', 'run'], repo.root);
    expect(missing.code).toBe(1);
    const flag = await runCli(['hooks', 'run', '--agent', 'claude', '--nope'], repo.root);
    expect(flag.code).toBe(1);
  });

  it('is the stop hook wherever its options are written, so its own errors never exit 2', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const orders = [
      ['--agent', 'claud', 'hooks', 'run'],
      ['hooks', '--agent', 'claud', 'run'],
      ['--cwd', repo.root, 'hooks', 'run', '--agent', 'claud'],
      ['--agent=claud', '--cwd', repo.root, 'hooks', '--bin', 'x', 'run'],
    ];
    for (const args of orders) {
      const typo = await runCli(args, repo.root);
      expect(typo.stderr, args.join(' ')).toContain('unknown agent "claud"');
      expect(typo.code, args.join(' ')).toBe(1);
    }
    const flag = await runCli(['--agent', 'claude', 'hooks', 'run', '--nope'], repo.root);
    expect(flag.stderr).toContain('unknown option "--nope"');
    expect(flag.code).toBe(1);
    const noValue = await runCli(['--cwd', repo.root, 'hooks', 'run', '--agent'], repo.root);
    expect(noValue.stderr).toContain('option --agent needs a value');
    expect(noValue.code).toBe(1);
  });

  it("does not take an option's value for the command: other commands keep exit code 2", async () => {
    const repo = await fixtures.create({ files: FAILING });
    // "hooks" and "run" here are values of options, not the command.
    const cases: { args: string[]; says: string }[] = [
      { args: ['--rules', 'hooks', 'run'], says: 'unknown command "run"' },
      { args: ['--agent', 'hooks', 'run', '--nope'], says: 'unknown option "--nope"' },
      { args: ['hooks', '--bin', 'run'], says: 'hooks needs an action' },
      { args: ['check', '--rules', 'hooks', 'run'], says: 'check takes no arguments, got "run"' },
      { args: ['--bin', 'hooks', '--agent', 'run', 'verify', 'extra'], says: 'verify takes no' },
    ];
    for (const { args, says } of cases) {
      const result = await runCli(args, repo.root);
      expect(result.stderr, args.join(' ')).toContain(says);
      expect(result.code, args.join(' ')).toBe(EXIT_USAGE);
    }
  });

  it('lets the agent stop on a partial run, and says that it was not a clean pass', async () => {
    // No command for lint or typecheck. The default (ADR 0020): do not block.
    const files = {
      'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
      'test.js': "console.log('ok');\n",
    };
    const repo = await fixtures.create({ files });
    const result = await stop(repo.root, 'claude', { session_id: 'p1', stop_hook_active: false });
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toBe(
      'fscale: verify was only partial, so this is not a clean pass. 2 of 4 steps ran; skipped: lint, typecheck. Run `fscale verify` for the details, or set verify.failOnSkipped to true to make skipped steps block.\n',
    );
    // Cursor reads JSON from stdout, so the note must stay off it.
    const cursor = await stop(repo.root, 'cursor', { conversation_id: 'p1', loop_count: 0 });
    expect(cursor.code).toBe(EXIT_OK);
    expect(cursor.stdout).toBe('');
    expect(cursor.stderr).toContain('verify was only partial');
  });

  it('blocks on a skipped step when verify.failOnSkipped is on', async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
        'test.js': "console.log('ok');\n",
        'firescale.config.json': JSON.stringify({ verify: { failOnSkipped: true } }),
      },
    });
    const result = await stop(repo.root, 'claude', { session_id: 'p2', stop_hook_active: false });
    expect(result.code).toBe(EXIT_BLOCK);
    expect(result.stderr).toContain(
      'verify: FAILED - 2 of 4 steps skipped, and verify.failOnSkipped is on',
    );
  });
});
