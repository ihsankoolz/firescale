# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`.
- **In review:** M5 core rule pack, on branch `m5-core-rules`. If its pull request is merged, M5 is done. If it is still open, read its body: it says what its CI and review showed.
- **Not started, in order:** M6 npm packaging, M7 `verify` and hooks, M8 guarded agent runner, M9 `init`/`add`/`sync`.

The last session was asked to finish M3 through M9 in one sitting, with a hard stop at 11:45 Singapore time on 11 Nov. It had about 75 minutes, which covered M3, M4 and M5. The M5 rules were written and reviewed in the last half hour, so give them a second look (see DECISIONS.md, decision 0).

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. If the M5 pull request is still open, check its CI and its "Not done" section, finish what is listed there, and merge it.
3. Branch `m6-npm-packaging` from `main` and build M6 from PLAN.md. ADR 0008 §6 has the design: one binary package per platform in `optionalDependencies`, a small JavaScript launcher as `bin`, an optional hard-link step. Starting points:
   - `packages/cli` is the `firescale` npm package. It has no `bin` yet and is not publishable as is: `@firescale/sdk` is `private` and is a `workspace:*` dependency, so decide whether to publish the SDK or bundle it.
   - The binary finds the Node host through `FSCALE_NODE_HOST`, then next to itself, then through `import 'firescale/host'` (`crates/core/src/node.rs`). The launcher should set `FSCALE_NODE_HOST`.
   - A release workflow needs cross-platform builds. The CI `bench` job already builds a release binary on all three OSes.
   - Do not publish. Write the exact publish steps into DECISIONS.md.

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
