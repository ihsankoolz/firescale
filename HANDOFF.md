# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache.
- **In review:** M4 `fix`, SARIF and facts, on branch `m4-fix-sarif-facts`, pull request #5. If that pull request is merged, M4 is done.
- **Next:** M5 core rule pack. Not started.
- **Remaining after that, in order:** M6 npm packaging, M7 `verify` and hooks, M8 guarded agent runner, M9 `init`/`add`/`sync`.

The last session was asked to finish M3 through M9 in one sitting with a hard stop at 11:45 Singapore time on 11 Nov. It had about 75 minutes, which covered M3 and M4. M5 to M9 have not been started and nothing for them is half-built.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. If pull request #5 is still open, check its CI and its "Not done" section, finish what is listed there, and merge it.
3. Branch `m5-core-rules` from `main` and build M5 from PLAN.md: the remaining seven `core` rules from SPEC §6, in Rust. Useful starting points:
   - `crates/rules/src/core/no-committed-env/` is the reference rule: `mod.rs`, `SKILL.md`, `tests.rs`. Register new rules in `crates/rules/src/core/mod.rs`.
   - Rules that depend on the stack (`gitignore-basics`, `single-lockfile`, `scripts-present`) can use `firescale_core::facts::detect_facts`. The rule context does not expose facts yet; add that first.
   - A fix must go through the context (`ctx.write`, `ctx.untrack`) so `--dry-run` and the change list work.
   - Declare every file a rule reads in its `inputs`, or the cache will serve stale results.
   - The last "Done when" item is dogfooding: `fscale check` on this repo must report nothing.

## Things to know before changing code

- `pnpm build` before `pnpm test`: Vitest runs the built binary at `target/debug/fscale`, the stand-in `target/debug/fscale-node-stub`, and the built Node host.
- The `explain` snapshot embeds the rule's SKILL.md, which is compiled into the binary. After editing a SKILL.md: `pnpm build`, then update that snapshot on purpose.
- Commits must use the owner's git identity with no co-author or tool attribution. Check `git config user.name` and `user.email` in a fresh clone.
- The result cache is on by default. Use `--no-cache` when debugging a rule, and see decision 1 in `DECISIONS.md`.
- CI has two jobs per OS: `check` (Node 22 and 24) and `bench` (the 50k-file benchmark, numbers in the job summary).
- After changing the help text or a SKILL.md, update the affected snapshot on purpose (`pnpm build`, then `pnpm exec vitest run -u <that test file>`), and check that no other snapshot changed.

## Left for the owner to do by hand

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Nothing can be published to npm yet (M6).
- The M8 manual test with a real agent is not reachable yet.
