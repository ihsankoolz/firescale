# 0047 — The guarded agent runner

Status: accepted (2026-11-14, M27). Builds on SPEC §7, ADR 0042–0046 (the referee) and ADR 0037 (blame-aware verify).

## Context

SPEC §7 lists ten guardrails for `fscale fix --agent`. M27 builds the runner and every guardrail in Rust, with a fake agent so tests and CI never call a real model; the re-check is the referee (blame-aware verify and the tamper guard). The owner added: build and test against stand-in agent binaries; never run with wider permissions than the repo's own agent settings give; never push, tag or publish; the ledger stores no prompt text, no file contents and no secrets.

## Decision

### The adapter interface

An adapter is a **program that speaks a small protocol**, so an agent CLI, a Node adapter behind the bridge (M28) and the test stand-in all fit the same trait (`firescale_core::agentrun::Adapter`):

- `<adapter> --version`: exit 0 and a version line, or it is "not installed";
- `<adapter> auth status`: exit 0 when signed in;
- `<adapter> run <request.json>`: reads the request (cwd, rule, SKILL.md, diagnostics, prompt, policy), works in `cwd`, and prints one JSON line, `{ "ok": true|false, "costUsd"?, "turns"?, "error"? }`.

The request file is written to the system's temporary folder and removed after the run. The run is started as a process tree (`process::run_tree`), so the time limit and Ctrl+C stop the agent with everything it started. The only adapter in M27 is `fake`, `crates/testkit/src/fake_agent.rs`, which follows a scenario file (files to write or delete, commands to run, the result to print, how long to sleep, whether it is installed and signed in) and logs every request it gets, so tests can see exactly what an agent was given.

### The guardrails, in order

| #   | Guardrail           | How                                                                                                                                                                                                                                                                                                                                                                                                           | Test (`packages/cli/src/agent-runner.test.ts`) |
| --- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| 1   | Pre-flight          | the adapter's `--version` and `auth status`; `git status` clean (firescale's own folder aside); no tracked secret (`is_secret`: `.env` and `.env.*` other than `.example`, `.sample`, `.template`, `.dist`; `*.pem`, `*.key`, `secrets/`, `id_rsa*`)                                                                                                                                                          | guardrail 1                                    |
| 2   | Isolation           | `git worktree add -b fscale/fix/<rule>-<ms>` of `HEAD` in the temporary folder                                                                                                                                                                                                                                                                                                                                | guardrail 2                                    |
| 3   | Scoped prompt       | the rule's SKILL.md and that rule's diagnostics (`fscale check --json` shape), nothing of other rules                                                                                                                                                                                                                                                                                                         | guardrail 3                                    |
| 4   | Tool limits         | `Policy::strict`: Read, Glob, Grep, Edit, Write; denied commands `git push`, `git tag`, `git remote`, `npm`/`pnpm`/`yarn`/`cargo publish`; denied paths the secrets and the protected paths; narrowed by the repo's own `.claude/settings.json` `permissions.deny`; a repo whose settings let no agent edit (`.codex/config.toml` `sandbox_mode = "read-only"`, Claude Code `defaultMode: "plan"`) is refused | guardrail 4                                    |
| 5   | Budget limits       | `agent.maxTurns`, `agent.maxCostUsd`, `agent.timeoutSeconds` passed on; a reported cost or turn count over the limit fails the run; the time limit stops the tree                                                                                                                                                                                                                                             | guardrail 5                                    |
| 6   | Controlled config   | `policy.loadProjectConfig: false` (each adapter turns it into its agent's flags in M28); every agent process gets `url.file:///firescale-runner-never-pushes/.pushInsteadOf` set empty through `GIT_CONFIG_*`, so any push goes to a path that does not exist, and `GIT_TERMINAL_PROMPT=0`                                                                                                                    | guardrail 6, "never pushes or tags"            |
| 7   | Diff guard          | every changed path, tracked or new: a protected path (the secrets, `.git/`, `firescale.config.*`, `.claude/`, `.codex/`, `.cursor/`, `.husky/`, `.firescale/`, `.mcp.json`, and the config's `protectedPaths`) or one outside the rule's `inputs` and the files its diagnostics name fails the run                                                                                                            | guardrail 7                                    |
| 8   | Re-check            | the rule checked again in the worktree (no violation may remain); the tamper guard over the change against `HEAD` (any finding fails, approval kinds included: no approval exists locally); blame-aware `verify` against `HEAD`                                                                                                                                                                               | guardrail 8                                    |
| 9   | One rule per commit | one commit on the run's branch under the repo's git identity, message `fix(<rule>): <description>` with `Firescale-Rule`, `Firescale-Agent`, `Firescale-Outcome: pass` trailers; a failed run's worktree and branch are removed                                                                                                                                                                               | guardrail 9                                    |
| 10  | Ledger              | one line per run in `.firescale/ledger.jsonl`, validated against `docs/schemas/ledger-entry.schema.json` in the test: schema, time, rule, agent, result, a reason code, files and lines changed, duration, and cost and turns when reported. Nothing else: no prompt, no file contents, no agent text (an adapter's error is cut to 160 characters and never written to the ledger)                           | guardrail 10                                   |

**Never pushes, tags or publishes**: the runner has no push, tag or publish step at all; the policy denies them; pushes from inside the run are rewritten to nowhere; a tag that appears during the run fails it and is deleted (test "never pushes or tags").

### Why these choices

- **A worktree, not a branch switch**: the person's checkout, index and stash are never touched, even when the run is stopped.
- **The rule's inputs as the allowed globs**: every rule already declares what its check reads; a fix has no business outside it. A rule with broad inputs (env-example reads every source file) allows a broad fix, and the tamper guard and verify are what stand behind it.
- **Any tamper finding fails a run**, approval kinds too: the run is local, and ADR 0046 decides nothing can be approved locally.
- **Failure leaves nothing**: the worktree and the branch go; only the ledger line stays.

## After the review

A fresh subagent tried to break the runner with the stand-in agent and reproduced, all fixed with tests (`agent-runner.test.ts`, "after review", and unit tests):

- **The agent could commit protected files to the run's own branch**, which the diff (against the worktree's `HEAD`) did not show. The diff is now against the commit the run started from, and a run whose branch moved is `agent-committed`.
- **A rename hid a protected file's deletion.** The diff is `--no-renames`, so both sides of every path are checked.
- **Code the agent wrote ran unguarded during the re-check** (a test that pushes). The re-check and verify now run under the agent's environment, and the repo's refs, config, hooks and checkout are compared again after them.
- **A remote with its own `pushurl` ignored `pushInsteadOf`.** Every agent process also gets `protocol.allow=never` and `GIT_SSH_COMMAND=false`: no transport at all.
- **Refs, the git config, hooks and the checkout outside the worktree** could be changed through the shared repo (`git update-ref`, `git config`, a new branch, a write through an absolute path). They are compared before and after; refs, config and hooks are put back, and the run fails.
- **A locked worktree, or an error on the way out, left the worktree behind.** The worktree is a drop guard: every way out of a run unlocks and removes it, deletes its folder and its branch.
- **Repo rules were half applied**: `Edit` denied left `Write` allowed; `Edit(src/**)` was not enforced; `ask` was ignored. Now `Edit` takes `Write` with it, scoped `Edit`/`Write`/`Read` globs are protected paths for the diff guard, `ask` counts as `deny`, and a repo that denies every edit is refused.
- **Secrets in the code** (not in file names) did not stop a run: `core/no-secrets-in-source` findings now refuse it.
- **The diff guard missed secrets the pre-flight knew** (`api/.env.production`, `.envrc`), letter case (`.ENV`, `.CLAUDE/`), nested agent folders and `.mcp.json`, and `\` in `protectedPaths`. It now uses the same `is_secret`, compares in lower case with `/`, and matches the agent folders at any depth.
- **Links and submodules** were committed (mode 120000, 160000): now rejected.
- **The agent could choose the ledger's reason** ("hit the time limit" read as `timeout`) and write a negative cost. The reason comes from the runner only, and a cost below 0 is an `agent-error`.
- **Ignored files** the agent created were outside the diff but seen by the re-check: a run that creates one fails.

## Known limits

- The `fake` adapter is the only one; whether a real agent respects `loadProjectConfig: false`, the tool lists and the budget depends on its adapter's flags (M28).
- The push guard is a git setting in the environment: an agent with a shell could unset it. The policy denies the shell, and the runner itself never pushes.
- **A process the agent detaches** (`setsid`, a daemon) outlives the run's process tree: it can change files or refs after every check has passed. Stopping it would need a cgroup or a job object around the whole run; not done.
- Other files in `.git` (`info/exclude`, reflogs) are not compared; files changed in your checkout are reported, not put back.
- The `agent` limits are read from `firescale.config.json` only, not from a `firescale.config.ts`.
- The cost limit is checked after the run, on what the agent reports; an agent that does not report cost is held only by the turn and time limits.
- Tags are compared by name before and after; a tag created and deleted inside the run, or a ref other than a tag, is not seen.
- Re-check runs the whole of verify against `HEAD` in the worktree, so a run costs a verify; the worktree has no installed dependencies unless the repo's `verify.baseInstall` provides them (ADR 0037).

## Consequences

- `fscale fix --agent fake --rule <id>` runs end to end in tests, on every OS, with no model.
- M28 adds the Claude Code and Codex adapters behind the same protocol, and `fscale ledger`.
