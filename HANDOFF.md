# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`, M5 core rule pack, M7 `verify` and hooks, M8 the fix loop (pull request #12, with the plan update of ADR 0017 and the CI trim of ADR 0016), M9 `init`, `adopt` and `sync`.
- **CI owed:** nothing. M6's Windows npm install test passed in a manual run on `main` on 2026-11-11.
- **Waiting on the owner:** the publishing checkpoint. 0.1.0 is not on npm yet. Check with `npm view firescale version` before starting M10; if it is not published, say so at the top of the M10 pull request.
- **Not started, in order:** M10 group-project pack, M11 `doctor`, M12 agent files from `init`, M13 generators, M14 Next.js pack, M15 Python pack, M16 to M18 blame-aware verify, M19 to M22 tamper guard, M23 guarded agent runner, M24 adapters and `fscale ledger`, M25 Supabase pack, M26 Expo pack.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. Verify locally before pushing: `pnpm build && pnpm fscale verify`. That runs check, lint, typecheck and every test.
3. CI (ADR 0016): a push runs Linux only. macOS and Windows run on a pull request into `main`, so open the pull request to get them. If jobs end within seconds with no logs, the account's Actions limit is back: add anything unproven on macOS or Windows to "CI owed" in DECISIONS.md.
4. Start the next milestone in PLAN.md on a branch named `m<N>-<short-name>`. Do one milestone per session and stop.

Notes for M10 (the group-project pack) and M12 (agent files), which stand on M9:

- `init`, `adopt` and `sync` are `crates/cli/src/scaffold.rs`. They share one function, `run`, which uses `plan_fixes`, `FixPlan::run` and `render_fixes` from `crates/cli/src/main.rs`, the same pieces as `fscale fix`. A new pack needs no change there: give its rules fixes and `fscale init --packs core,<pack>` builds it.
- A fix that creates a file must look at the disk itself before writing and write only where nothing is. `core/ci-exists` (`starter_can_be_created`) and `core/readme-setup` are the models. The check uses the same precondition, so a finding is `deterministic` only when the fix will act; otherwise the loop stops with "changed nothing".
- If the rule counts tracked files only, the fix must call `ctx.track(file)` after `ctx.write`, or its own check still fails. `RuleContext::track` is a no-op outside git.
- A fix tells a person what to look at through `FixResult.notes`; `init`, `adopt` and `sync` print them under "To review".
- M10's CI rule runs after `core/ci-exists`, whose fix now writes `.github/workflows/ci.yml` with `npx firescale verify`. A hand-written workflow is never rewritten; there are tests to copy in `packages/cli/src/scaffold.test.ts` ("existing work survives").
- M12's managed block is the first fix that rewrites lines. The byte-for-byte tests in `scaffold.test.ts` use an AGENTS.md with look-alike markers; keep them passing for text outside the real block.
- `fscale init` refuses a folder that is not empty. Tests that need a new repo use `fixtures.create({ track: 'no-git' })` with no files.
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
- Publish 0.1.0 to npm: the checkpoint after M9. The steps are under "Publishing to npm" in `DECISIONS.md`.
- The manual test with a real agent (now M24) is not reachable yet.
