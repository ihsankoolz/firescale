import type { Adapter, AgentResult, Invocation, Request } from './protocol.js';

/**
 * Codex, run headless (`codex exec`). Its documentation site could not be
 * reached from the build environment on 2026-11-14; what is used was read
 * from its source at the tag `rust-v0.169.0` (github.com/openai/codex):
 *
 * - `codex exec … -`: the prompt on standard input (`exec/src/cli.rs`).
 * - `--json`: JSONL events; `turn.completed` carries `usage` (tokens), and
 *   no money cost is reported (`exec/src/exec_events.rs`).
 * - `--ephemeral`, `--ignore-user-config` (`$CODEX_HOME/config.toml`, where
 *   MCP servers and hooks live, is not loaded), `--skip-git-repo-check`.
 * - Approvals are always `never` in `codex exec` (`exec/src/lib.rs`).
 * - Project configuration and MCP servers stay off in a folder that is not
 *   trusted unless `--trust` is passed (release notes of 0.169.0); the
 *   run's worktree is never trusted, and `--trust` is never passed.
 * - `codex login status` exits 0 when signed in, 1 when not
 *   (`cli/src/login.rs`).
 *
 * Unverified (in Codex's shared options, which were not read): `--sandbox
 * workspace-write`, `-c sandbox_workspace_write.network_access=false`.
 */
export const codex: Adapter = {
  name: 'codex',
  program: 'codex',
  versionArgs: ['--version'],
  authArgs: ['login', 'status'],
  unverified: [
    '`--sandbox workspace-write` and `-c sandbox_workspace_write.network_access=false` (Codex shared options, not read)',
    'tool lists, denied paths and turn or spend limits: `codex exec` has no flags for them; the runner enforces paths with its diff guard, and time with its time limit',
  ],
  invocation(request: Request): Invocation {
    return {
      args: [
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
      ],
      stdin: request.prompt,
    };
  },
  parse(stdout: string, code: number): AgentResult {
    let turns = 0;
    let failed: string | undefined;
    for (const line of stdout.split('\n')) {
      if (!line.trim().startsWith('{')) continue;
      let event: { type?: unknown };
      try {
        event = JSON.parse(line) as { type?: unknown };
      } catch {
        continue;
      }
      if (event.type === 'turn.completed') turns += 1;
      if (event.type === 'turn.failed' || event.type === 'error') failed = String(event.type);
    }
    const ok = code === 0 && failed === undefined;
    return {
      ok,
      turns,
      ...(ok ? {} : { error: `Codex ended with ${failed ?? `exit ${code}`}` }),
    };
  },
};
