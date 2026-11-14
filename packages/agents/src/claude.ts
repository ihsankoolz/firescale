import type { Adapter, AgentResult, Invocation, Request } from './protocol.js';

/**
 * Claude Code, run headless (`claude -p`). Every flag was read from Claude
 * Code's documentation on 2026-11-14 (code.claude.com: CLI reference,
 * headless mode, permissions, permission modes, settings; changelog top
 * 2.1.325):
 *
 * - `-p` print mode; the prompt on standard input.
 * - `--output-format json`.
 * - `--max-turns`, `--max-budget-usd` (print mode only; the budget is the
 *   agent's own estimate).
 * - `--permission-mode dontAsk`: anything not allowed is denied, never asked.
 * - `--tools`: the only built-in tools that exist in the run (no Bash).
 * - `--allowedTools` and `--disallowedTools` with rules such as `Edit`,
 *   `Read(./.env)` (a Read deny also blocks Edit and Write on the path; paths
 *   are gitignore patterns from the working folder) and
 *   `Bash(git push *)`.
 * - `--settings '{"disableAllHooks":true}'` turns hooks off;
 *   `--strict-mcp-config` with no `--mcp-config` loads no MCP server.
 * - `claude auth status` exits 0 when signed in; `claude --version`.
 *
 * From `claude --help` of 2.1.325 (the CLI's own text; `--max-turns` is in
 * the docs but not in that help):
 *
 * - `--restricted`: no settings files at all (user, project, local: the
 *   repo's hooks and permissions are not loaded; the runner passes the
 *   repo's deny rules itself), no command-running tools or WebFetch unless
 *   `--tools` names them, and the file tools confined to the working folder.
 * - `--no-session-persistence`: the session is not saved.
 */
export const claude: Adapter = {
  name: 'claude',
  program: 'claude',
  versionArgs: ['--version'],
  authArgs: ['auth', 'status'],
  unverified: [
    'the fields of the print-mode JSON result (total_cost_usd, num_turns, is_error, subtype): the docs list them for the SDK result, not for `claude -p --output-format json`',
  ],
  invocation(request: Request): Invocation {
    const { policy } = request;
    // A command is denied with and without arguments (`Bash(git push)`,
    // `Bash(git push *)`); a rule the repo wrote (`Bash(curl *)`, `Write`)
    // is passed as it is. A Read deny keeps Grep and Glob out too, and an
    // Edit deny covers every tool that edits files.
    const denied = [
      ...policy.deniedCommands.flatMap((command) =>
        command.includes('(') || /^[A-Z][A-Za-z]*$/.test(command)
          ? [command]
          : [`Bash(${command})`, `Bash(${command} *)`],
      ),
      ...policy.deniedPaths.flatMap((glob) => [`Read(${glob})`, `Edit(${glob})`]),
    ];
    return {
      args: [
        '-p',
        '--output-format',
        'json',
        '--max-turns',
        String(policy.maxTurns),
        '--max-budget-usd',
        String(policy.maxCostUsd),
        '--permission-mode',
        'dontAsk',
        '--restricted',
        '--no-session-persistence',
        '--settings',
        JSON.stringify({ disableAllHooks: true }),
        '--strict-mcp-config',
        '--tools',
        policy.allowedTools.join(','),
        '--allowedTools',
        policy.allowedTools.join(','),
        '--disallowedTools',
        denied.join(','),
      ],
      stdin: request.prompt,
    };
  },
  parse(stdout: string, code: number): AgentResult {
    const last = stdout
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.startsWith('{'))
      .pop();
    let parsed: Record<string, unknown>;
    try {
      parsed = last ? (JSON.parse(last) as Record<string, unknown>) : {};
    } catch {
      return { ok: false, error: "Claude Code's answer could not be read" };
    }
    const cost = typeof parsed.total_cost_usd === 'number' ? parsed.total_cost_usd : undefined;
    const turns = typeof parsed.num_turns === 'number' ? parsed.num_turns : undefined;
    const subtype = typeof parsed.subtype === 'string' ? parsed.subtype : undefined;
    const ok = code === 0 && parsed.is_error === false && subtype === 'success';
    return {
      ok,
      ...(cost === undefined ? {} : { costUsd: cost }),
      ...(turns === undefined ? {} : { turns }),
      ...(ok ? {} : { error: `Claude Code ended with ${subtype ?? `exit ${code}`}` }),
    };
  },
};
