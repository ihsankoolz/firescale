# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache (pull request #4), M4 `fix`, SARIF, facts and `--affected` (pull request #5).
- **Started, not merged, not reviewed:** M5 core rule pack, on branch `m5-core-rules`. See "M5 draft" below.
- **Not started, in order:** M6 npm packaging, M7 `verify` and hooks, M8 guarded agent runner, M9 `init`/`add`/`sync`.

The last session was asked to finish M3 through M9 in one sitting, with a hard stop at 11:45 Singapore time on 11 Nov. It had about 75 minutes. That covered M3 and M4, both reviewed, green on CI and merged, and a first pass at M5.

## M5 draft

Branch `m5-core-rules` holds a first pass of the seven remaining `core` rules, written by three parallel agents in the last half hour of the session:

`gitignore-basics`, `single-lockfile`, `scripts-present`, `ci-exists`, `readme-setup`, `env-example`, `no-secrets-in-source`.

Treat it as a draft, not as finished work:

- **It has not been reviewed.** Every earlier milestone's review found real bugs. Run a fresh reviewer agent on the diff before trusting any of it, with extra care for `no-secrets-in-source` (false positives, and that no message or test contains a real-looking token) and for the two rules with fixes (`gitignore-basics`, `env-example`).
- **Check what is really there.** If the branch has a pull request, its body says what was finished and what its CI showed. If it has none, run the full suite on the branch and look at each rule folder: a rule whose `mod.rs` still says "Placeholder" was not written.
- **Dogfooding is not done.** The last "Done when" item is that `fscale check` on this repo reports nothing. Expect findings from the new rules on this repo, and decide for each whether to fix the repo, fix the rule, or configure it.
- PLAN.md, DECISIONS.md and an ADR for the rules' heuristics still have to be written for M5.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. Check out `m5-core-rules`, merge `main` into it, and run `pnpm install && pnpm build && pnpm lint && pnpm typecheck && pnpm test`. Trust that result over this file.
3. Finish M5: review, fixes with regression tests, dogfooding, ADR, PLAN.md ticks, DECISIONS.md and HANDOFF.md updates, CI green on all nine jobs, merge.
4. Continue with M6 (npm packaging) from PLAN.md. ADR 0008 §6 has the design.

Notes for M5 rule work:

- `crates/rules/src/core/no-committed-env/` is the reference rule: `mod.rs`, `SKILL.md`, `tests.rs`. Rules are registered in `crates/rules/src/core/mod.rs`.
- A fix must go through the context (`ctx.write`, `ctx.untrack`) so `--dry-run` and the change list work.
- Declare every file a rule reads in its `inputs`, or the cache will serve stale results.
- Adding rules changes the `check` snapshots (rule counts). Update them on purpose and check nothing else changed.

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
