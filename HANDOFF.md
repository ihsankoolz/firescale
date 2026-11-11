# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`, M5 core rule pack, M7 `verify` and hooks.
- **Merged without full CI:** M6 npm packaging. See "CI owed" at the top of DECISIONS.md: one Windows check for M6 is unproven.
- **M8 the fix loop:** built on branch `m8-fix-loop`, with the plan update (ADR 0017) and the CI trim (ADR 0016) in the same pull request.
- **Not started, in order:** M9 `init`/`adopt`/`sync`, then the owner's publishing checkpoint, M10 group-project pack, M11 `doctor`, M12 agent files from `init`, M13 generators, M14 Next.js pack, M15 Python pack, M16 to M18 blame-aware verify, M19 to M22 tamper guard, M23 guarded agent runner, M24 adapters and `fscale ledger`, M25 Supabase pack, M26 Expo pack.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. Verify locally before pushing: `pnpm build && pnpm fscale verify`. That runs check, lint, typecheck and every test.
3. CI (ADR 0016): a push runs Linux only. macOS and Windows run on a pull request into `main`, so open the pull request to get them. If jobs end within seconds with no logs, the account's Actions limit is back: add anything unproven on macOS or Windows to "CI owed" in DECISIONS.md.
4. Start the next milestone in PLAN.md on a branch named `m<N>-<short-name>`. Do one milestone per session and stop.

Notes for M9 (`init`, `adopt`, `sync`), which stands on M8:

- The loop is `crates/core/src/fixloop.rs`: `plan_order` (order and validation) and `run_fix_loop` (passes and convergence). `fix()` in `crates/cli/src/main.rs` shows how to drive it for built-in and custom rules, and `render_fixes` how to print the result. `init`, `adopt` and `sync` should call the same pieces, not copy them.
- `sync --check` is a dry run whose exit code is 1 when the report has changes. `crates/core/src/scratch.rs` (`Scratch::copy_of`) is the scratch copy the dry run uses.
- Fixes that create files must mark their finding `deterministic` and really repair it, or the loop fails with "changed nothing".
- If a new fix edits a file another fix creates, declare `after`, as `core/no-committed-env` does for `core/gitignore-basics`. A test in `crates/rules/src/lib.rs` rejects a built-in `after` that names no built-in rule, and cycles.
- A write that does not change a file is not a change: `RuleContext::write` and the SDK's `ctx.files.write` skip it.
- Cursor's hook format is still unconfirmed (cursor.com is blocked from the cloud build environment). M12 needs it.

## Things to know before changing code

- `pnpm build` before `pnpm test`: Vitest runs the built binary at `target/debug/fscale`, the stand-in `target/debug/fscale-node-stub`, and the built Node host.
- The `explain` snapshot embeds the rule's SKILL.md, which is compiled into the binary. After editing a SKILL.md: `pnpm build`, then update that snapshot on purpose.
- Commits must use the owner's git identity with no co-author or tool attribution. Check `git config user.name` and `user.email` in a fresh clone.
- The result cache is on by default. Use `--no-cache` when debugging a rule, and see decision 1 in `DECISIONS.md`.
- CI (ADR 0016): a push to any branch runs Linux on Node 22 and 24. macOS and Windows run only on a pull request into `main`. The `bench` and `package` jobs run only when the workflow is started by hand. A new push cancels the run in progress. The owner is short on Actions minutes and tokens: push seldom, keep subagent use modest.
- This repo's agent hooks (`.claude/settings.json`, `.codex/hooks.json`, `.cursor/hooks.json`) run `pnpm --silent fscale hooks run`, which runs the whole of `fscale verify` (about 22 s) when an agent tries to stop, and sends it back up to 3 times while it fails.
- `fscale explain` only knows rules of packs that are on. Tests that explain an `agent-ready` rule need a config that lists the pack.
- `fscale fix --dry-run` copies the repo to the temp folder and runs the real fixes there. A test that asserts a dry run changed nothing should compare the repo's files and `git ls-files` before and after.
- After changing the help text or a SKILL.md, update the affected snapshot on purpose (`pnpm build`, then `pnpm exec vitest run -u <that test file>`), and check that no other snapshot changed.

## Left for the owner to do by hand

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Nothing can be published to npm yet (M6).
- The manual test with a real agent (now M24) is not reachable yet.
