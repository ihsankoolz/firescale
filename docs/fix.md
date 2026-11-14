# `fscale fix`

`fscale fix` applies the deterministic fix of every enabled rule that has one. A fix only does things that have one correct outcome; anything that needs judgement is left for a person or an agent, guided by the rule's SKILL.md.

```
fscale fix --dry-run     # show what would change, change nothing
fscale fix               # apply
fscale fix --rules "core/*"
```

- Rules that are `off` are skipped. Rules that are `evaluated` or `enforced` are fixed.
- Every fix is idempotent: running `fscale fix` again changes nothing and prints `Nothing to fix`.
- The exit code is 0 when the fixes ran and settled, even if violations remain that have no deterministic fix. Run `fscale check` afterwards to see what is left. It is 2 when the fixes cannot settle (see below).

## The fix loop

`fscale fix` does not run each fix once. It runs them in passes, until a pass finds nothing left to fix (ADR 0018). `init`, `adopt` and `sync` run the same loop over every rule that is on ([init.md](init.md)).

- **Order.** A rule can say that its fix must run after another rule's, with `after` (see [writing-rules.md](writing-rules.md)). Otherwise fixes run in order of rule id. `--rules` does not change the order of the rules it selects, and a dependency it leaves out simply does not run.
- **Passes.** A fix that needs something another fix creates, without saying so, gets it one pass later.
- **Bad declarations stop the run before anything is written:** an `after` id that no rule has, and rules that must each run after another of them. Turning one rule of such a cycle `off` breaks it.

The loop stops with an error, and exit code 2, when the fixes do not settle:

| What happened                                                                        | The error names                          |
| ------------------------------------------------------------------------------------ | ---------------------------------------- |
| A fix changed nothing twice in a row, but its rule still reports fixable violations. | The rule.                                |
| A rule wrote the same content to a file again, after another rule changed it.        | The file, and every rule that writes it. |
| Files are still changing after 10 passes.                                            | The rules and files of the last pass.    |

In each case the changes already made stay in your working tree and are listed in the error. Review them with `git status` and `git diff`. The usual cause is a bug in a custom rule; turn the rule `off` to continue.

## Dry run

`--dry-run` copies the repo to a scratch folder, runs the real loop there, and deletes the folder. Your working tree and git index are not touched. Because the fixes really run, each one sees what the fixes before it wrote, and the preview shows what `fscale fix` will do. It prints, per rule, in the order the rules run:

- one line per change (`would untrack`, `would edit`, `would create`),
- a unified diff for every file that would be written, with forward-slash paths and LF line endings. A new file is shown as a diff against `/dev/null`. When two rules change the same file, the second diff starts from the first one's result.

Removing a file from the git index is not a text change, so it appears as a `would untrack` line, not in the diff.

The copy holds every file git does not ignore, plus every tracked file. Ignored files such as `node_modules` are not copied. The copy is its own git repo, and git's `GIT_DIR` and `GIT_INDEX_FILE` variables are ignored during a dry run, so it is safe to run from a git hook.

## Custom rules

A custom TypeScript rule's `fix()` runs in the Node host, through the SDK. The SDK records every `ctx.files.write` and `ctx.git.untrack` call, and those records are what `fscale fix` reports. A fix that changes things by any other route (for example `node:fs` directly) still works, but its changes are not listed and a dry run cannot show them, so use the context. In a dry run the fix works on the scratch copy, while the pack itself is loaded from your repo.

## `fscale fix --agent`

`fscale fix --agent <name> [--rule <id>]` hands one rule's remaining violations, the ones its deterministic fix cannot repair, to a coding agent, under the guardrails of SPEC §7 (ADR 0047). Without `--rule` it takes the first rule with such a violation.

1. **Pre-flight.** The agent's CLI answers `--version` and says it is signed in; the working tree is clean; no secret (`.env`, `.env.*` other than templates, `.envrc`, `*.pem`, `*.key`, `secrets/`) is tracked, and `core/no-secrets-in-source` has no finding. The repo's own agent settings must let an agent edit (`.codex/config.toml` `sandbox_mode = "read-only"` or Claude Code's `defaultMode: "plan"` stop it).
2. **Isolation.** The agent works in a git worktree of `HEAD` on a new branch, `fscale/fix/<rule>-<time>`. Your branch, index and files are never touched.
3. **Scoped prompt.** The rule's SKILL.md and that rule's diagnostics only.
4. **Tool limits.** Read, search and edit files: no shell. Secrets and protected paths may be neither read nor written; `git push`, `git tag` and publishing are denied. Whatever the repo's `.claude/settings.json` denies or asks about is denied too (`Edit` takes `Write` with it, and `Edit(src/**)` makes `src/**` a protected path): a run is never wider than the repo's own settings, and a repo that denies every edit is refused.
5. **Budget.** `agent.maxTurns`, `agent.maxCostUsd` and `agent.timeoutSeconds` (config.md). The agent's process tree is stopped at the time limit; a run over turns or spend fails.
6. **Controlled config.** The run tells the adapter not to load the repo's own agent hooks, settings or MCP servers. Git in the run can reach no remote: every URL is rewritten for pushes and `protocol.allow` is `never`. Refs, the git config, hooks and your checkout are compared before and after the run (and after the re-check); a change there fails the run and is put back.
7. **Diff guard.** A change to a protected path (the secrets, `.git`, `firescale.config.*`, `.claude/`, `.codex/`, `.cursor/`, `.husky/`, `.firescale/`, `.mcp.json`, and `protectedPaths`), or to a file outside the rule's inputs and the files its violations name, fails the run; so do a link or a submodule, an ignored file the agent created, and a commit the agent made itself. Paths are compared without regard to letter case, and a rename counts as both its sides.
8. **Re-check.** Under the agent's environment, the rule is checked again in the worktree; the tamper guard compares the change with `HEAD` (any finding fails, including a changed snapshot or expected value); and blame-aware `verify` runs against `HEAD`.
9. **One commit.** A run that passes is one commit on its branch, under your git identity, with the trailers `Firescale-Rule`, `Firescale-Agent` and `Firescale-Outcome`. A run that fails is rolled back: its worktree and branch are removed. firescale never pushes, tags or publishes; review the branch and merge it yourself.
10. **Ledger.** Every run appends one line to `.firescale/ledger.jsonl` ([docs/schemas/ledger-entry.schema.json](schemas/ledger-entry.schema.json)): rule, agent, pass or fail and why, files and lines changed, time, and cost and turns when the agent reports them. Never the prompt, a file's contents or the agent's own words.

Exit code 0 when the run passed (or there was nothing to fix), 1 when it failed, 2 when it was refused before it started.

### The agents

| `--agent` | Runs                                                                                                                                                                                                                                                                       | Cost in the ledger                              |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| `claude`  | `claude -p --output-format json --max-turns <n> --max-budget-usd <usd> --permission-mode dontAsk --restricted --no-session-persistence --settings '{"disableAllHooks":true}' --strict-mcp-config --tools … --allowedTools … --disallowedTools …`, prompt on standard input | yes, `total_cost_usd` as Claude Code reports it |
| `codex`   | `codex exec --json --ephemeral --ignore-user-config --skip-git-repo-check --sandbox workspace-write -c sandbox_workspace_write.network_access=false -`, prompt on standard input                                                                                           | no: Codex reports tokens, not money             |
| `fake`    | the stand-in the tests use, `crates/testkit/src/fake_agent.rs`                                                                                                                                                                                                             | what its scenario says                          |

The adapters are TypeScript (`packages/agents`, shipped in the `firescale` npm package as `agents/`), started with Node (`FSCALE_NODE`; `FSCALE_AGENTS` points at another `cli.js`). Where each flag was confirmed, and what was not, is in ADR 0048. **Unverified** (printed under `Agent:` in every run's output too):

- `claude`: the fields of the JSON result of `claude -p --output-format json` (`total_cost_usd`, `num_turns`, `is_error`, `subtype`). The docs list them for the SDK's result message. If they differ, a run that worked reads as failed, and its cost is missing.
- `codex`: `--sandbox workspace-write` and `-c sandbox_workspace_write.network_access=false` (Codex's documentation could not be reached; its shared options were not read). Codex has no flags for tool lists, denied paths, turns or spend: the diff guard, the re-check and the time limit are what hold a Codex run.

What to check by hand before you trust a real agent: see "Manual checks" in ADR 0048.

## `fscale ledger`

`fscale ledger [--json]` sums `.firescale/ledger.jsonl`: runs, passes, pass rate and the cost the agents reported, in total, per rule and per agent, with failed runs counted by reason. A line it cannot read (not JSON, a field missing, a cost below 0, a line cut short) is skipped and named by its number; the exit code is 0 either way. Cost is only what agents reported: `$0.42 (2 of 3 runs)` means one run reported none. The JSON shape: [packages/cli/schemas/ledger-output.schema.json](../packages/cli/schemas/ledger-output.schema.json).

## Limits

- The diff shows all of a file's changes in a single hunk.
- A dry run of a large repo takes as long as copying its tracked and unignored files.
- A fix whose result depends on a file git ignores and does not track can preview differently from the real run, because that file is not in the copy.
- If `fscale` is killed during a dry run, a `firescale-dry-run-*` folder is left in the temp folder. It is safe to delete.
