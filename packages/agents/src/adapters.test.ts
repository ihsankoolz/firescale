import { describe, expect, it } from 'vitest';
import { claude } from './claude.js';
import { codex } from './codex.js';
import type { Request } from './protocol.js';

// The flags each adapter passes, pinned (M28, ADR 0048). A change here is a
// change to what the real agent CLI is asked to do: confirm it against the
// agent's docs or source first, and update the ADR.

const request: Request = {
  cwd: '/work/tree',
  rule: 'core/env-example',
  skill: '# skill',
  diagnostics: [],
  prompt: 'Fix core/env-example.',
  policy: {
    allowedTools: ['Read', 'Glob', 'Grep', 'Edit', 'Write'],
    deniedCommands: ['git push', 'Bash(curl *)', 'WebFetch'],
    deniedPaths: ['.env', '**/.claude/**'],
    maxTurns: 12,
    maxCostUsd: 1.5,
    timeoutSecs: 600,
    loadProjectConfig: false,
  },
};

describe('the Claude Code adapter', () => {
  it('runs claude -p with the policy as flags, the prompt on stdin', () => {
    const { args, stdin } = claude.invocation(request);
    expect(stdin).toBe('Fix core/env-example.');
    expect(args).toEqual([
      '-p',
      '--output-format',
      'json',
      '--max-turns',
      '12',
      '--max-budget-usd',
      '1.5',
      '--permission-mode',
      'dontAsk',
      '--restricted',
      '--no-session-persistence',
      '--settings',
      '{"disableAllHooks":true}',
      '--strict-mcp-config',
      '--tools',
      'Read,Glob,Grep,Edit,Write',
      '--allowedTools',
      'Read,Glob,Grep,Edit,Write',
      '--disallowedTools',
      'Bash(git push),Bash(git push *),Bash(curl *),WebFetch,Read(.env),Edit(.env),Read(**/.claude/**),Edit(**/.claude/**)',
    ]);
    // The prompt is never an argument: it could hold anything.
    expect(args.join(' ')).not.toContain('Fix core/env-example');
  });

  it('never loads the project settings, never bypasses permissions', () => {
    const { args } = claude.invocation(request);
    expect(args).toContain('--restricted');
    expect(args).not.toContain('--setting-sources');
    expect(args).not.toContain('bypassPermissions');
    expect(args).not.toContain('--dangerously-skip-permissions');
  });

  it('reads cost and turns from the JSON result', () => {
    const out = JSON.stringify({
      type: 'result',
      subtype: 'success',
      is_error: false,
      total_cost_usd: 0.0425,
      num_turns: 4,
      result: 'Done.',
    });
    expect(claude.parse(`${out}\n`, 0)).toEqual({ ok: true, costUsd: 0.0425, turns: 4 });
  });

  it('fails on an error result, a non-zero exit or output it cannot read', () => {
    const over = JSON.stringify({
      subtype: 'error_max_budget_usd',
      is_error: true,
      total_cost_usd: 1.6,
      num_turns: 9,
    });
    expect(claude.parse(over, 1)).toEqual({
      ok: false,
      costUsd: 1.6,
      turns: 9,
      error: 'Claude Code ended with error_max_budget_usd',
    });
    expect(claude.parse('', 1)).toEqual({ ok: false, error: 'Claude Code ended with exit 1' });
    expect(claude.parse('{not json', 0).ok).toBe(false);
    // Text from the agent never reaches the error.
    const text = JSON.stringify({ subtype: 'success', is_error: false, result: 'secret' });
    expect(JSON.stringify(claude.parse(text, 3))).not.toContain('secret');
  });
});

describe('the Codex adapter', () => {
  it('runs codex exec with JSON events, no user config, the prompt on stdin', () => {
    const { args, stdin } = codex.invocation(request);
    expect(stdin).toBe('Fix core/env-example.');
    expect(args).toEqual([
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
    expect(args).not.toContain('--trust');
    expect(args).not.toContain('--dangerously-bypass-approvals-and-sandbox');
  });

  it('counts turns from the events and reports no cost', () => {
    const events = [
      { type: 'thread.started', thread_id: 't' },
      { type: 'turn.started' },
      { type: 'turn.completed', usage: { input_tokens: 10, output_tokens: 5 } },
      { type: 'turn.started' },
      { type: 'turn.completed', usage: { input_tokens: 10, output_tokens: 5 } },
    ]
      .map((event) => JSON.stringify(event))
      .join('\n');
    expect(codex.parse(events, 0)).toEqual({ ok: true, turns: 2 });
  });

  it('fails on turn.failed, error events or a non-zero exit', () => {
    const failed = [{ type: 'turn.started' }, { type: 'turn.failed', error: { message: 'x' } }]
      .map((event) => JSON.stringify(event))
      .join('\n');
    expect(codex.parse(failed, 0)).toEqual({
      ok: false,
      turns: 0,
      error: 'Codex ended with turn.failed',
    });
    expect(codex.parse('', 2)).toEqual({ ok: false, turns: 0, error: 'Codex ended with exit 2' });
  });
});

describe('what each adapter says is unverified', () => {
  it('names every flag not confirmed from the agent itself', () => {
    expect(claude.unverified.join(' ')).toContain('total_cost_usd');
    expect(codex.unverified.join(' ')).toContain('--sandbox workspace-write');
  });
});
