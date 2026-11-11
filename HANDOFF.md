# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`, M5 core rule pack.
- **Merged without full CI:** M6 npm packaging. See "CI owed" at the top of DECISIONS.md: one Windows check for M6 is unproven.
- **M7 `verify` and hooks:** built and reviewed on branch `m7-verify-hooks`, pull request #9. **Not merged if you are reading this on that branch:** its CI jobs never started (Actions limit). See "CI owed" in DECISIONS.md. Re-run the failed jobs when Actions works, fix anything Windows or macOS shows, then merge.
- **Next:** the direction changed on 2026-11-11. A docs-only pull request rewrites PLAN.md from M8 onward. Read the merged PLAN.md, not this line, for what comes next.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. Verify locally before pushing: `pnpm build && pnpm fscale verify`. That runs check, lint, typecheck and every test.
3. Check whether GitHub Actions runs again (it stopped on 2026-11-11 at the account's monthly limit). If it does not, add anything unproven on macOS or Windows to "CI owed" in DECISIONS.md.
4. Start the next milestone in PLAN.md on a branch named `m<N>-<short-name>`.

Notes from M7 that the next milestones build on:

- `fscale verify` lives in `crates/core/src/verify.rs` (planning and running steps) and `crates/cli/src/main.rs` (`verify_report`). The blame-aware referee will wrap `run_verify`.
- Hooks live in `crates/core/src/hooks.rs`: `merge_hook` (file formats), `decide` (loop guard), `reply` (per-agent answers).
- **Cursor's hook format is unconfirmed.** cursor.com is blocked from the cloud build environment. Confirm `.cursor/hooks.json` from Cursor's docs before building more on it.
- The `agent-ready` pack is in `crates/rules/src/agent-ready/`, opt-in, with no fixes yet. Rules whose fixes create files are the next step.

## Things to know before changing code

- `pnpm build` before `pnpm test`: Vitest runs the built binary at `target/debug/fscale`, the stand-in `target/debug/fscale-node-stub`, and the built Node host.
- The `explain` snapshot embeds the rule's SKILL.md, which is compiled into the binary. After editing a SKILL.md: `pnpm build`, then update that snapshot on purpose.
- Commits must use the owner's git identity with no co-author or tool attribution. Check `git config user.name` and `user.email` in a fresh clone.
- The result cache is on by default. Use `--no-cache` when debugging a rule, and see decision 1 in `DECISIONS.md`.
- CI runs four `check` jobs per push. The `bench` and `package` jobs (benchmarks, npm install smoke tests) only run when the workflow is started by hand, or on a pull request labelled `ci:bench` / `ci:package` (label it before pushing). The owner is short on Actions minutes and tokens: push once per milestone, keep subagent use modest.
- This repo's agent hooks (`.claude/settings.json`, `.codex/hooks.json`, `.cursor/hooks.json`) run `pnpm --silent fscale hooks run`, which runs the whole of `fscale verify` (about 22 s) when an agent tries to stop, and sends it back up to 3 times while it fails.
- `fscale explain` only knows rules of packs that are on. Tests that explain an `agent-ready` rule need a config that lists the pack.
- After changing the help text or a SKILL.md, update the affected snapshot on purpose (`pnpm build`, then `pnpm exec vitest run -u <that test file>`), and check that no other snapshot changed.

## Left for the owner to do by hand

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Nothing can be published to npm yet (M6).
- The M8 manual test with a real agent is not reachable yet.
