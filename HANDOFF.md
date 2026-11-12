# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`, M5 core rule pack, M6 npm packaging, M7 `verify` and hooks, M8 the fix loop (pull request #12, with the plan update of ADR 0017 and the CI trim of ADR 0016), M9 `init`, `adopt` and `sync`.
- **CI owed:** nothing. M6's Windows npm install test passed in a manual run on `main` on 2026-11-11.
- **Published:** the owner published 0.1.0 to npm on 2026-11-12. `npx firescale@0.1.0 init` and `adopt --dry-run` work on Windows and Linux. The second box of the publishing checkpoint (a generated repo passing its own CI on GitHub) is still open.
- **Done and merged in this session:** automated releases (ADR 0021, `RELEASING.md`): a pushed tag `v<version>` builds, publishes with npm trusted publishing and verifies. The version is 0.1.1 and **no tag has been pushed**; the owner tags after setting up the trusted publishers. Before that, M10 detection that works on real repos (pull request #14, ADR 0020): projects in subfolders in `fscale facts`, per-project steps and `PARTIAL` in `fscale verify`, and three changes to `core/env-example`.
- **Not started, in order:** M11 group-project pack, M12 `doctor`, M13 agent files from `init`, M14 generators, M15 Next.js pack, M16 Python pack, M17 to M19 blame-aware verify, M20 to M23 tamper guard, M24 guarded agent runner, M25 adapters and `fscale ledger`, M26 Supabase pack, M27 Expo pack. These numbers are one higher than before M10 was added; ADR 0020 has the table.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. Verify locally before pushing: `pnpm build && pnpm fscale verify`. That runs check, lint, typecheck and every test.
3. CI (ADR 0016): a push runs Linux only. macOS and Windows run on a pull request into `main`, so open the pull request to get them. If jobs end within seconds with no logs, the account's Actions limit is back: add anything unproven on macOS or Windows to "CI owed" in DECISIONS.md.
4. Start the next milestone in PLAN.md on a branch named `m<N>-<short-name>`. Do one milestone per session and stop.

Notes on M10, for whoever touches facts or verify next:

- Discovery is `discovered_projects` in `crates/core/src/facts.rs`. The depth (`DISCOVERY_DEPTH`) and the skipped folders (`DISCOVERY_SKIPPED_DIRS`) are constants there. Per-project detection is `node_detail`, `python_detail`, `rust_detail` and `go_detail`.
- The repo-level `commands`, `packageManager` and `lockfiles` still describe the root only, because rules read them. Do not fold the projects' commands into them without checking `agent-ready/commands-documented` and `core/readme-setup`.
- Verify plans the root's steps and then one `ProjectPlan` per discovered project (`plan_verify` and `plan_projects` in `crates/core/src/verify.rs`). A step is `Skipped` (no command, makes the run partial) or `NotNeeded` (shown as `off`). Keep that difference: a test tries every mix and fails if a headline says `OK` with a skipped step.
- Blame-aware verify (M17) must bump the verify schema to 3; M10 took 2. It also has to decide how labels work per project.
- The course-project fixture exists twice with the same files: `course_project_files` in `crates/core/src/testing.rs` and `courseProjectFiles` in `packages/cli/src/course-fixture.ts`. The CLI tests put stand-ins for `python` and `npx` on the PATH (`writeToolShims`), so they need neither tool.
- `detect_facts` is called by four rules in one `check`, and each call walks the tree. Caching it per run would win back the 7 to 15% recorded in `bench/RESULTS.md`; mind the fix loop, where files change between calls.

Notes for M11 (the group-project pack) and M13 (agent files), which stand on M9:

- `init`, `adopt` and `sync` are `crates/cli/src/scaffold.rs`. They share one function, `run`, which uses `plan_fixes`, `FixPlan::run` and `render_fixes` from `crates/cli/src/main.rs`, the same pieces as `fscale fix`. A new pack needs no change there: give its rules fixes and `fscale init --packs core,<pack>` builds it.
- A fix that creates a file must look at the disk itself before writing and write only where nothing is. `core/ci-exists` (`starter_can_be_created`) and `core/readme-setup` are the models. The check uses the same precondition, so a finding is `deterministic` only when the fix will act; otherwise the loop stops with "changed nothing".
- A fix creates a file with `ctx.create_tracked(file, content)`, never with `ctx.write`: it fails if anything is at the path or git tracks the path, refuses symbolic links, stages the file (rules that count tracked files need that), and removes the file again if staging fails. `ctx.write` is for appending to or merging into a file that exists.
- A fix tells a person what to look at through `FixResult.notes`; `init`, `adopt` and `sync` print them under "To review".
- M11's CI rule runs after `core/ci-exists`, whose fix now writes `.github/workflows/ci.yml` with `npx firescale verify`. A hand-written workflow is never rewritten; there are tests to copy in `packages/cli/src/scaffold.test.ts` ("existing work survives").
- M13's managed block is the first fix that rewrites lines. The byte-for-byte tests in `scaffold.test.ts` use an AGENTS.md with look-alike markers; keep them passing for text outside the real block.
- `fscale init` refuses a folder that is not empty. Tests that need a new repo use `fixtures.create({ track: 'no-git' })` with no files.
- Cursor's hook format is still unconfirmed (cursor.com is blocked from the cloud build environment). M13 needs it.

## Things to know before changing code

- `pnpm build` before `pnpm test`: Vitest runs the built binary at `target/debug/fscale`, the stand-in `target/debug/fscale-node-stub`, and the built Node host.
- The `explain` snapshot embeds the rule's SKILL.md, which is compiled into the binary. After editing a SKILL.md: `pnpm build`, then update that snapshot on purpose.
- Commits must use the owner's git identity with no co-author or tool attribution. Check `git config user.name` and `user.email` in a fresh clone.
- The result cache is on by default. Use `--no-cache` when debugging a rule, and see decision 1 in `DECISIONS.md`.
- CI (ADR 0016): a push to any branch runs Linux on Node 22 and 24. macOS and Windows run only on a pull request into `main`. The `bench` and `package` jobs run only when the workflow is started by hand. A new push cancels the run in progress. The owner is short on Actions minutes and tokens: push seldom, keep subagent use modest.
- This repo's agent hooks (`.claude/settings.json`, `.codex/hooks.json`, `.cursor/hooks.json`) run `pnpm --silent fscale hooks run`, which runs the whole of `fscale verify` (about 22 s) when an agent tries to stop, and sends it back up to 3 times while it fails.
- `fscale explain` only knows rules of packs that are on. Tests that explain an `agent-ready` rule need a config that lists the pack.
- `fscale fix --dry-run` copies the repo to the temp folder and runs the real fixes there. A test that asserts a dry run changed nothing should compare the repo's files and `git ls-files` before and after.
- The version is recorded in `Cargo.toml`, `Cargo.lock` and five `package.json` files. Change it only with `node scripts/bump-version.mjs <version>`. Never push a `v*` tag: it publishes to npm.
- `.github/workflows/release.yml` must keep its file name; npm's trusted publishers name it. `release.test.ts` pins that only the publish job has `id-token: write` and that the file uses no secret.
- After changing the help text or a SKILL.md, update the affected snapshot on purpose (`pnpm build`, then `pnpm exec vitest run -u <that test file>`), and check that no other snapshot changed.

## Left for the owner to do by hand

- Release 0.1.1: set up the eight trusted publishers on npmjs.com, then tag `v0.1.1` and push the tag (`RELEASING.md`).

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Tick the second box of the publishing checkpoint: see a repo made by `npx firescale@0.1.0 init` pass its generated CI on GitHub (step 7 under "Publishing to npm" in `DECISIONS.md`).
- The manual test with a real agent (now M25) is not reachable yet.
