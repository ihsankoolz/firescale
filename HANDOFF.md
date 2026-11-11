# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`, M5 core rule pack.
- **In review:** M6 npm packaging, on branch `m6-npm-packaging`. If its pull request is merged, M6 is done. If it is still open, read its body.
- **Not started, in order:** M7 `verify` and hooks, M8 guarded agent runner, M9 `init`/`add`/`sync`.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. If the M6 pull request is still open, check its CI and its "Not done" section, finish what is listed there, and merge it.
3. Branch `m7-verify-hooks` from `main` and build M7 from PLAN.md. Starting points:
   - `fscale facts` already detects the package manager, the `build`/`test`/`lint`/`typecheck` commands and the tools (`nx`, `turbo`): `crates/core/src/facts.rs`. `verify` should pick its commands from there.
   - Hook formats were researched on 2026-11-11 from primary sources. Claude Code: a `Stop` hook in `.claude/settings.json`, shape `{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"..."}]}]}}`; the hook gets JSON on stdin with `stop_hook_active`; it blocks with exit code 2 and its stderr, or with `{"decision":"block","reason":"..."}` on stdout; Claude Code itself ends the turn after 8 blocks in a row. Codex CLI (0.167.1): the same shape and the same blocking contract, in `<repo>/.codex/hooks.json` (confirmed in its source, `codex-rs/hooks/src/engine/discovery.rs`). **Cursor could not be checked**: cursor.com was blocked from the build session. Confirm its `.cursor/hooks.json` format from Cursor's docs before relying on it.
   - The agent-ready rule pack goes in `crates/rules/src/` next to `core/`.

## Things to know before changing code

- `pnpm build` before `pnpm test`: Vitest runs the built binary at `target/debug/fscale`, the stand-in `target/debug/fscale-node-stub`, and the built Node host.
- The `explain` snapshot embeds the rule's SKILL.md, which is compiled into the binary. After editing a SKILL.md: `pnpm build`, then update that snapshot on purpose.
- Commits must use the owner's git identity with no co-author or tool attribution. Check `git config user.name` and `user.email` in a fresh clone.
- The result cache is on by default. Use `--no-cache` when debugging a rule, and see decision 1 in `DECISIONS.md`.
- CI runs four `check` jobs per push. The `bench` and `package` jobs (benchmarks, npm install smoke tests) only run when the workflow is started by hand. The owner is short on Actions minutes and tokens: push once per milestone, keep subagent use modest.
- After changing the help text or a SKILL.md, update the affected snapshot on purpose (`pnpm build`, then `pnpm exec vitest run -u <that test file>`), and check that no other snapshot changed.

## Left for the owner to do by hand

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Nothing can be published to npm yet (M6).
- The M8 manual test with a real agent is not reachable yet.
