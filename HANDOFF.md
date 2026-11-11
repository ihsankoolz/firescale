# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door.
- **In review:** M3 engine and cache, on branch `m3-engine-cache`, pull request #4.
- **Next:** M4 `fix`, SARIF and facts. Not started.
- **Remaining after that, in order:** M5 core rule pack, M6 npm packaging, M7 `verify` and hooks, M8 guarded agent runner, M9 `init`/`add`/`sync`.

The last session was asked to finish M3 through M9 in one sitting with a hard stop at 11:45 Singapore time on 11 Nov. It had about 70 minutes, which was enough for M3 only. M4 to M9 have not been started and nothing for them is half-built.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. If pull request #4 is not merged, check its CI and merge it. Then branch `m4-fix-sarif-facts` from `main`.
3. Build M4 from PLAN.md. Useful starting points:
   - `fix` for built-in rules already exists as a library function: `firescale_core::rule::fix_rule`. The CLI command, `--dry-run` (print a diff, change nothing) and the bridge method for a custom rule's `fix` are missing.
   - SARIF: add a second renderer next to `format_json` in `crates/core/src/report.rs`.
   - `--affected` and `--sarif` are not parsed yet in `crates/cli/src/main.rs`.
   - `facts` is new. `RepoFacts` exists only as a TypeScript type in `packages/sdk/src/types.ts`.

## Things to know before changing code

- `pnpm build` before `pnpm test`: Vitest runs the built binary at `target/debug/fscale`, the stand-in `target/debug/fscale-node-stub`, and the built Node host.
- The `explain` snapshot embeds the rule's SKILL.md, which is compiled into the binary. After editing a SKILL.md: `pnpm build`, then update that snapshot on purpose.
- Commits must use the owner's git identity with no co-author or tool attribution. Check `git config user.name` and `user.email` in a fresh clone.
- The result cache is on by default. Use `--no-cache` when debugging a rule, and see decision 1 in `DECISIONS.md`.
- CI has two jobs per OS: `check` (Node 22 and 24) and `bench` (the 50k-file benchmark, numbers in the job summary).

## Left for the owner to do by hand

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Nothing can be published to npm yet (M6).
- The M8 manual test with a real agent is not reachable yet.
