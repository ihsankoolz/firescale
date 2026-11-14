# 0048 — Agent adapters and `fscale ledger`

Status: accepted (2026-11-14, M28). Builds on ADR 0047 (the guarded runner and its protocol) and ADR 0008 (Rust core, TypeScript for adapters).

## Context

PLAN M28: `packages/agents`, the adapter interface plus Claude Code and Codex adapters in TypeScript, "called through the bridge"; each CLI's flags confirmed from its current docs and pinned in tests; cost recorded when the CLI reports it; `fscale ledger [--json]`. The owner added: there is no real agent CLI to build against, so build and test against stand-ins; never invent a flag or a config format; what cannot be confirmed from the agent's docs or source is built behind the interface, marked "unverified" in the command's output and in the docs, with a list of what to check by hand.

## Decision

### Adapters are programs speaking ADR 0047's protocol, started with Node

`packages/agents/src/cli.ts` is `node cli.js <claude|codex> --version | auth status | run <request.json>`: the same protocol as the stand-in agent, so the Rust runner keeps one adapter type (`ProgramAdapter`) and every guardrail applies unchanged. `fscale fix --agent claude|codex` starts `node <agents/cli.js> <name>` as a process tree.

"Called through the bridge" is read as: started with Node the way the bridge starts its host (`FSCALE_NODE`, the script found next to the binary, `FSCALE_AGENTS` to override), but as a **separate process per run**, not a request to the long-lived Node host. The host is one process shared by a whole command; the time limit and Ctrl+C must stop the agent and everything it started, which `run_tree` does for a process tree of its own. Simplest reading that keeps guardrail 5.

The adapters ship inside the `firescale` npm package as `agents/` (`scripts/pack-npm.mjs`, checked in `pack.test.ts`); `packages/agents` stays private. No new dependency.

Each adapter is data plus two functions (`packages/agents/src/protocol.ts`): the program, its version and sign-in arguments, the list of what is **unverified**, `invocation(request)` (arguments and standard input) and `parse(stdout, exitCode)` (ok, cost, turns, and an error made by the adapter, never the agent's text). The prompt always goes on standard input, never in an argument.

### Claude Code

Confirmed from Claude Code's documentation (code.claude.com: CLI reference, headless mode, permissions, settings; changelog to 2.1.325), and from `claude --help` of 2.1.325 installed in the build environment (the CLI's own text; no model call):

| Flag                                                   | Why                                                                                                                                                                                                                                                               |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-p`, prompt on standard input                         | headless                                                                                                                                                                                                                                                          |
| `--output-format json`                                 | one result object                                                                                                                                                                                                                                                 |
| `--max-turns <n>`                                      | guardrail 5 (in the docs; not listed by `--help`)                                                                                                                                                                                                                 |
| `--max-budget-usd <usd>`                               | guardrail 5; print mode only; Claude Code's own estimate                                                                                                                                                                                                          |
| `--permission-mode dontAsk`                            | anything not allowed is denied, never asked                                                                                                                                                                                                                       |
| `--restricted`                                         | from `--help`: no settings files (user, project, local), no command-running tools or WebFetch unless `--tools` names them, file tools confined to the working folder; so the repo's own hooks, permissions and the person's settings are not loaded (guardrail 6) |
| `--no-session-persistence`                             | from `--help`: the session is not saved                                                                                                                                                                                                                           |
| `--settings '{"disableAllHooks":true}'`                | no hooks, belt and braces                                                                                                                                                                                                                                         |
| `--strict-mcp-config` and no `--mcp-config`            | no MCP server                                                                                                                                                                                                                                                     |
| `--tools Read,Glob,Grep,Edit,Write`                    | the only tools that exist in the run (the policy's `allowedTools`, narrowed by the repo's deny rules)                                                                                                                                                             |
| `--allowedTools …`                                     | the same list, allowed without asking                                                                                                                                                                                                                             |
| `--disallowedTools …`                                  | each denied command as `Bash(<cmd>)` and `Bash(<cmd> *)`; the repo's own rules as written; each denied path as `Read(<glob>)` and `Edit(<glob>)` (a Read deny keeps Grep and Glob out; an Edit deny covers every editing tool)                                    |
| `claude --version`, `claude auth status` (exit 0 or 1) | pre-flight                                                                                                                                                                                                                                                        |

The result: `total_cost_usd` becomes the ledger's `costUsd`, `num_turns` its `turns`; the run is ok when the exit code is 0, `is_error` is false and `subtype` is `success`.

**Unverified**: those four fields. The docs list them for the Agent SDK's result message, not for `claude -p --output-format json`. If they differ, a run that worked reads as failed (safe) and its cost is missing.

### Codex

Codex's documentation site could not be reached from the build environment. Confirmed from its source at tag `rust-v0.169.0` (github.com/openai/codex): `codex exec … -` reads the prompt from standard input; `--json` prints JSONL events, `turn.completed` with token `usage` and no money cost; `--ephemeral`; `--ignore-user-config` (no `$CODEX_HOME/config.toml`, where MCP servers and hooks are configured); `--skip-git-repo-check`; approvals are always `never` in `exec`; a folder that is not trusted gets no project config or MCP servers unless `--trust`, which is never passed; `codex login status` exits 0 or 1.

The run: `codex exec --json --ephemeral --ignore-user-config --skip-git-repo-check --sandbox workspace-write -c sandbox_workspace_write.network_access=false -`. Turns are counted from `turn.completed`; a `turn.failed` or `error` event, or a non-zero exit, fails the run. No cost is reported, so a Codex line in the ledger has none, and `fscale ledger` says so (`-`).

**Unverified**: `--sandbox workspace-write` and the `-c` key, which live in Codex's shared options that were not read. If wrong, Codex exits with a usage error and the run fails (safe). And: `codex exec` has no flags for a tool list, denied paths, turns or spend. Those parts of the policy cannot be passed on; the diff guard, the re-check, the tamper guard and the time limit hold a Codex run. This is said in the docs.

The unverified items are printed under `Agent:` in the output of every `fscale fix --agent` run, from the adapter's `--version` answer, and listed in docs/fix.md.

### Tests: stand-ins for the agents' own CLIs

`crates/testkit/src/fake_cli.rs` (`fscale-fake-cli`) is copied onto the PATH as `claude` or `codex`. It logs its arguments, standard input and working folder, prints a canned answer, makes canned edits and exits with a chosen code. `packages/agents/src/adapters.test.ts` pins every argument of both adapters and their result parsing; `packages/cli/src/agent-adapters.test.ts` runs `fscale fix --agent claude|codex` end to end with the stand-ins: the exact arguments, the prompt on standard input, the worktree as working folder, the cost in the ledger and in `fscale ledger`, a failed run keeping its reported cost, pre-flight failures, and the adapters found beside the binary.

**Tests never leave the stand-in off the PATH.** A first version of the "not installed" test removed it, and on the build machine the real `claude` (installed next to `node`) was found and ran one real `claude -p` on a fixture repo. The test now uses a stand-in whose `--version` fails. That accidental run passed every guardrail and its result parsed as a success, which suggests the unverified fields are right for 2.1.325, but it used an earlier flag set (`--setting-sources user` in place of `--restricted`) and its output was not kept, so it counts as a hint, not as the manual test.

### `fscale ledger`

`crates/core/src/ledger.rs`. Reads `.firescale/ledger.jsonl` and sums runs, passes, pass rate, reported cost (rounded to a millionth of a dollar, so sums print the same everywhere) and how many runs reported one, and failed runs by reason: in total, per rule and per agent. A damaged line (not JSON, a missing or mistyped field, a result other than pass or fail, a cost below 0 or not finite, a line cut short) is skipped and its number reported; exit code 0 either way. No ledger: "No agent runs yet". `--json` follows `packages/cli/schemas/ledger-output.schema.json`. Snapshot tests for both forms (`ledger.test.ts`).

## Manual checks (owner)

On a machine with each agent installed and signed in, in a scratch repo with one real violation (for example `core/env-example`: code reads an env var `.env.example` lacks):

1. **Claude Code**: `fscale fix --agent claude --rule core/env-example`. Expect PASSED, one commit on `fscale/fix/…`, a ledger line with `costUsd` and `turns`, and `fscale ledger` showing the cost. If it says FAILED with `agent-error`, look at `claude -p --output-format json` output by hand: are `total_cost_usd`, `num_turns`, `is_error`, `subtype` its field names?
2. **Claude Code limits**: in the same repo, set `agent.maxTurns` to 1 and run again; expect a failed run (`agent-error` or `over-budget`). Add `.claude/settings.json` with a `hooks` entry that writes a file; expect the file not to appear (hooks off).
3. **Codex**: `fscale fix --agent codex --rule core/env-example`. If Codex rejects `--sandbox workspace-write` or the `-c` key, the run fails with `agent-error`: check `codex exec --help` and send the right spelling.
4. **Codex sandbox**: confirm from Codex's docs that `workspace-write` with network off is what `exec` uses, and that `--ignore-user-config` keeps MCP servers out.
5. **Windows**: a `claude` or `codex` installed by npm is a `.cmd` file, which Node does not start without a shell. The adapters start the program by name, so they need the `.exe` (Claude Code's native installer). Confirm, or tell me to add a `.cmd` path.

## Known limits

- Codex: no tool list, denied paths, turn or spend limit in `exec`; no cost in the ledger.
- Claude Code's `--max-budget-usd` is its own estimate and can be passed a little; the runner checks the reported cost after the run (ADR 0047).
- On Windows the adapters need the agent as an `.exe` (above).
- The adapters' `--version` output is what `fscale fix --agent` prints under `Agent:`; an adapter cannot be asked for its unverified list on its own.
- Adapter changes to a flag need the pinned tests and this ADR updated together.

## Consequences

- `fscale fix --agent claude|codex` runs the real agents under the same guardrails as the stand-in, and the ledger records what they report.
- `fscale ledger` shows what agent fixing has cost, and how often it worked, per rule and per agent.
