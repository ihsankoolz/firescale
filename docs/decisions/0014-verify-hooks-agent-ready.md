# 0014 — `verify`, agent hooks and the `agent-ready` pack

Status: accepted (M7)

## Context

SPEC §8 asks for one `fscale verify` command that people, CI and agents all call, hook config that stops an agent from finishing while verify fails, and a guard against hook loops. SPEC §6 lists an `agent-ready` rule pack in one line. This ADR records the choices made while building them.

## Decision

### verify

- Four steps in a fixed order: `check`, `lint`, `typecheck`, `test`. The run stops at the first failure and lists later steps as `not run`. An agent gets one failure to fix at a time, and the cheap steps come first.
- A command for each step is chosen in this order: the new `verify` config key, Nx, Turborepo, root `package.json` scripts, Cargo for a Rust-only repo. With Turborepo a step only runs if `turbo.json` defines the task.
- A step with no command is **skipped, and a skip is not a failure**. firescale must work on any repo (SPEC §3.1); failing a repo because it has no linter would break that. The summary says why each step was skipped.
- The `verify` config key maps `lint`, `typecheck` and `test` to a command string, or to `false` to skip. It is the escape hatch for every repo the detection does not fit, so the detection itself stays small.
- Commands run through the platform shell (`sh -c`, `cmd /D /S /C`) from the repo root, with stdin closed and `NO_COLOR=1`. On Windows the command line is passed to `cmd` raw, because Rust's argument quoting turns `"` into `\"`, which `cmd` does not understand. Output is captured; the summary keeps the last 30 non-empty lines of the failed step with colour codes removed. A failed `check` step shows the violations of enforced rules only, under the same cap.
- `--affected` narrows the `check` step and is passed to `nx affected` and `turbo run --affected`. `--base` is passed to Nx when it is a plain ref name; Turborepo takes its base from `TURBO_SCM_BASE`, which is left to the user. Package scripts and Cargo have no notion of "affected" and run in full.
- `--json` has its own schema (`verify-output.schema.json`, `schemaVersion: 1`). Exit codes match `check`: 0 ok, 1 failed, 2 usage or config error.

### Hooks

- The hook command is `fscale hooks run --agent <name>`, not `fscale verify`. Each agent expects a different answer (exit code 2 and stderr, or JSON on stdout) and the loop guard needs state, so both live in the binary instead of in shell snippets inside each agent's config. That also keeps the hook identical on Windows.
- Formats: Claude Code `.claude/settings.json` (`hooks.Stop`), confirmed from its documentation. Codex `.codex/hooks.json`, same shape, confirmed from its source (0.167.1). Cursor `.cursor/hooks.json` (`version: 1`, `hooks.stop`, reply `followup_message`): **not confirmed**. cursor.com could not be reached from the build environment on two attempts. The Cursor hook is still written, because the plan asks for three agents, and `hooks install` prints a note saying it is unconfirmed.
- Installing merges into the existing file and keeps everything else. A hook counts as installed when a command under the agent's stop event contains `hooks run --agent <name>`, so a second install with another `--bin` changes nothing, and the same text elsewhere in the file (a permission rule, another event) does not count. A file that is not valid JSON is never overwritten.
- `--bin` sets how the hook starts fscale. The default is `fscale`. `npx fscale` would run an unrelated npm package (ADR 0013), so the documented form for npm installs is `npx firescale`.
- Only project-level config is written. firescale never edits a user's global agent settings.

### Loop guard

- A counter per agent and session, in `.firescale/hooks/`, counts blocks in a row. At the cap (3, or `FSCALE_HOOK_MAX_BLOCKS`) the agent is let go with a note, and the counter resets. 3 is well under Claude Code's own limit of 8, so firescale's message is the one the user sees.
- A stop attempt the agent marks as fresh (`stop_hook_active: false`, or `loop_count: 0`) starts a new count, and so does a pass. **An agent that sends neither field is counted**, so the cap holds even if an agent changes its payload; a count older than one hour is ignored so that it does not leak into later work. Cursor's `loop_count` is honoured when it is higher than ours.
- If the count cannot be stored, or its path goes through a symlink, the agent is let go. A guard that cannot count must not block forever, and a repo must not be able to aim the write outside `.firescale/`.
- A hook that cannot run verify at all (bad config, say) exits 1 with the reason and does not block. The same goes for usage errors: `fscale hooks run` never exits 2 for its own mistakes, because Claude Code and Codex read 2 as "do not stop". A broken tool must not trap an agent.
- Session ids are reduced to `[A-Za-z0-9_-]` before they become a file name.

### The `agent-ready` pack

- Four rules, in Rust: `agents-md`, `commands-documented`, `single-test-command`, `verify-hook`. As with the core pack (ADR 0012), a false alarm is treated as worse than a miss.
- **The pack is opt-in.** The default `packs` stays `["core"]`. A repo that has never used an agent should not get four findings about agent setup on its first `fscale check`.
- No rule has a deterministic fix yet. AGENTS.md content must describe the real project, and which command starts fscale in a hook differs per repo. Generating these files belongs to `init`.
- `commands-documented` accepts a command as `fscale facts` prints it (`pnpm run test`) or its short form (`pnpm test`; for npm only `npm test`), matched as a whole command so `pnpm test:unit` does not count.
- `single-test-command` passes on a `test` script, a root `Cargo.toml`, pytest config, or a `Makefile` `test` target, and only reports when a language is detected.
- `fscale explain <rule>` for a rule of a built-in pack that is off now says which pack to turn on.

### This repo

- `firescale.config.json` turns on `agent-ready`, and the three hook files are committed with `--bin "pnpm --silent fscale"`, so agents working here must pass `fscale verify` before they stop.

## Consequences

- An independent review before the pull request found eight issues (the loop guard not capping without `stop_hook_active`, Windows quoting, hook detection that was too loose, an unbounded check summary, exit code 2 on hook usage errors, `--base` not reaching Nx, symlinked counter paths, and thin tests). All were fixed and are covered by tests; the decisions above describe the fixed behaviour.

- Two existing test expectations changed, both because the product's text changed on purpose: the "Allowed keys" config error now lists `verify`, and the help snapshot lists the new commands and options. The dogfooding test now expects 12 rules instead of 8.
- A hook file that is both untracked and ignored by git is not part of `verify-hook`'s cache key, so its cached result can be stale until another input changes. `--no-cache` avoids it.
- There is no time limit on verify steps and no `hooks uninstall`.
- The Cursor format must be confirmed by someone who can read Cursor's documentation; if it differs, only `merge_hook`, `parse_hook_input` and `reply` in `crates/core/src/hooks.rs` change.
- On Windows, commands go through `cmd`. A `verify` command written with POSIX-only syntax (single quotes, `&&` chains are fine, `$VAR` is not) will not work there.
