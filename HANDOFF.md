# Handoff

Current state of the project, so a new session can resume cold. Updated in every milestone's pull request.

## In progress

Nothing is half-finished. This session (2026-11-13, from 07:06 UTC) built M18 to M21, one pull request each, and stopped after M21 as asked. **No tag has been pushed and the version is unchanged (0.2.1).**

- **Branch:** `claude/amazing-planck-g1745y` (the cloud session may push only to this branch; it is restarted from `main` after each merge, so each pull request has its own history).
- **Done in this session:** M18, the local step cache (#28, ADR 0036); M19, the base run (#29, ADR 0037); M20, per-test labels (#30, ADR 0038); M21, reruns and the stop hook (#31, ADR 0039).
- **Next, in order:** M22 (faster generated CI and a shared cache); before it, the owner may want to tag `v0.2.1` or a later version (`RELEASING.md`).
- The `v0.2.0` tag exists and 0.2.0 is on npm. `v0.2.1` has not been tagged.
- Real repos for the after-milestone pass are listed in `DECISIONS.md` ("Real repos used for testing").

## Where things stand

- **Done and merged:** M0 skeleton, M1 rule SDK, M2 Rust front door, M3 engine and cache, M4 `fix`, SARIF, facts and `--affected`, M5 core rule pack, M6 npm packaging, M7 `verify` and hooks, M8 the fix loop (pull request #12, with the plan update of ADR 0017 and the CI trim of ADR 0016), M9 `init`, `adopt` and `sync`.
- **CI owed:** nothing. M6's Windows npm install test passed in a manual run on `main` on 2026-11-11.
- **Published:** the owner published 0.1.0 to npm by hand on 2026-11-12. **0.1.1 and 0.1.2 were released on 2026-11-12 through the Release workflow with trusted publishing; every stage passed.** The second box of the publishing checkpoint (a generated repo passing its own CI on GitHub) is still open.
- **Done in the latest session:** the 0.1.2 fixes from running 0.1.1 on the course project (ADR 0022): each Python project runs with its own `.venv` interpreter (or uv, poetry, pipenv, an active `VIRTUAL_ENV`, then `python`), projects are independent in verify, `setup-needed` is reported apart from failures, ruff and mypy come from dependency lists, and test-runner config files are test code for `core/env-example`. 0.1.2 was released through the Release workflow on 2026-11-12. Earlier: automated releases (ADR 0021, `RELEASING.md`) and M10 (ADR 0020).
- **Done in an earlier session, after 0.1.2:** M11 the group-project pack (ADR 0023).
- **Done in the latest session:** 0.1.3, verify on slow, real projects (ADR 0024): progress on stderr, stopping whole process trees on Ctrl+C and termination, time limits, `CI=1` with no input, run-once sibling scripts for watch mode, refused local connections as setup needed, `--jobs`, and run records instead of locks. **0.1.3 was released on 2026-11-12** and run on the course project on Windows: progress lines, setup detection, parallel projects and Ctrl+C worked, and no process was left behind.
- **Done in the session of 2026-11-12 (evening):** M12 `doctor` and the two fixes from the 0.1.3 run (ADR 0025, 0026), M13 agent files (ADR 0027), M14 generators (ADR 0028), M15 Next.js pack (ADR 0029), M16 Python pack (ADR 0030), and the 0.2.0 version.
- **Done in the session of 2026-11-12/13:** the plan update (ADR 0031) and M17, the project graph and affected verify (ADR 0032, ADR 0033).
- **Done in the session of 2026-11-13 (day):** M18, the local step cache (ADR 0036); M19 to M21, blame-aware verify (ADR 0037 to 0039, verify schema 6).
- **Not started, in order:** M22 faster generated CI and a shared cache, M23 to M26 tamper guard, M27 guarded agent runner, M28 adapters and `fscale ledger`, M29 Supabase pack, M30 Expo pack. ADR 0031 has the table from the old numbers.

## How to resume

1. Read `SPEC.md`, `PLAN.md`, `AGENTS.md`, `docs/decisions/` and `DECISIONS.md`.
2. Verify locally before pushing: `pnpm build && pnpm fscale verify`. That runs check, lint, typecheck and every test.
3. CI (ADR 0016): a push runs Linux only. macOS and Windows run on a pull request into `main`, so open the pull request to get them. If jobs end within seconds with no logs, the account's Actions limit is back: add anything unproven on macOS or Windows to "CI owed" in DECISIONS.md.
4. Start the next milestone in PLAN.md on a branch named `m<N>-<short-name>`. Do one milestone per session and stop.

Notes on M20 and M21, for whoever touches per-test labels, reruns or the stop hook (ADR 0038, 0039):

- Per-test results are `crates/core/src/pertest.rs`: `TestRunner::of` (command, or the package script behind it), `parse_junit`, `parse_vitest`, `parse_jest`, `parse_cargo`, `follow_renames`. `run_step` sets `PYTEST_ADDOPTS=--junitxml=<temp file>` for pytest and reads the file after.
- In `blame::compare`, per-test results may make a step `new`; `pre-existing` still needs `signature_within`. Keep it that way.
- `rerun_new_failures` runs a `new` failure once more through `verify::run_again` (no cache): never write anything about a flaky result.
- The stop hook is `verify_report(.., VerifyMode::Hook)` in `crates/cli/src/main.rs`: affected projects measured from the base commit, `judge_passes: false`.

Notes on M19, for whoever touches blame-aware verify (ADR 0037):

- `crates/core/src/blame.rs`: `choose_base` (no base on a default branch without upstream), `compare` (never fails the run; labels by evidence), `run_in_worktree`, `Worktree` (links removed before the worktree), the base store (`.firescale/base/`, format 3, with signatures).
- `pre-existing` needs `signature_within(head, base)` (`verify.rs`, `failure_signature`): never loosen that to "the base failed too". Every review found a way an environment difference turned into a pass.
- Base steps run through `run_base_step` with `FSCALE_BASE_RUN=1`; the course stand-ins log ` [base]` then, and `stepCalls` leaves those out (`baseCalls` has them).
- Every git command sets `GIT_OPTIONAL_LOCKS=0` (`git.rs`): a test checks the index bytes.
- Test fixtures for blame work on a `feature` branch: on `main` with no upstream there is no base.

Notes on M18, for whoever touches the step cache (ADR 0036):

- The cache is `crates/core/src/stepcache.rs`. `StepCache::key` takes a new snapshot of the tree before each step (`Snapshot::capture_reusing`, which reuses a file's hash only when its size and time are unchanged and older than the last snapshot by 2 s), so never memoise a unit's digest across steps.
- What a unit's key covers is `unit_scope` (graph closure, folders above, files no project owns) and `covers`; keep it at least as wide as `graph::affected`.
- Tool versions: `probes` (exact names only) and `program_identity` (path, size, time, no run). Never run anything from the repo for a version; `doctor::ThisMachine::run` with an absolute path runs it through `sh -c 'exec "$0" "$@"'` on Unix.
- The stand-ins in `course-fixture.ts` answer `--version` only with `writeToolShims(dir, { versions: true })`; without it their steps are never cached. `stepCalls` leaves out `--version` probes.

Notes on 0.2.1, for whoever touches how verify judges a test step (ADR 0035):

- The early stops are in `run_step`'s `on_output` closure in `crates/core/src/verify.rs`: `service_down_while_running` (refused connections, and a named service that is down: `output_names` decides "named") and, after it, the `stopAfterErrors` stop (`EarlyKind::NoPass`).
- After an early stop or a timeout, the `first_error` closure runs the command again with `--maxfail=1` under `FIRST_ERROR_LIMIT`; `first_error_lines` picks the lines, `hide_user` and `activity::redact` hide secrets.
- A finished step: `setup_problem` (in `setup.rs`, now with `auth_failure`), then `errors_name_unreachable_service` (pytest's short summary).
- Doctor reads `.env`/`.env.local` through `services::services_with_local_env`; verify's `services_for_folder` must keep not reading them.
- The CLI tests use `fakePytest` in `packages/cli/src/verify-run.test.ts`: a stand-in that answers `--maxfail=1` with a first error.

Notes on M17, for whoever builds M18 (the step cache) or touches the graph:

- The graph is `crates/core/src/graph.rs`: `build_graph(root, &facts)`, `affected(&graph, &changed)`, `widening_reason`. It is never built by `detect_facts` (rules call that on every check). Imports come from `crates/core/src/imports.rs` (tree-sitter), manifest, Dockerfile and Compose paths from `crates/core/src/references.rs` (line readers, no file access).
- Node ids are `<kind>:<folder>` and `shared:<path>` (`graph::project_id`). M18's key needs "the hashes of the projects and shared nodes a project depends on": walk `graph.edges` from the project's id, transitively, and hash the files each node holds (a shared file node is one file; a project holds its folder minus deeper projects).
- Verify marks units with `limit_to_affected`; a unit that does not run reports `StepStatus::NotAffected`. The CLI's `verify_plan` in `crates/cli/src/main.rs` builds the graph and handles the "no base and a clean tree" case.
- `fscale check --affected` still uses `affected::narrow_report` with `dependsOn` only (ADR 0032 says why).
- Verify schema is 5. Blame-aware verify takes 6 (ADR 0031).
- The course fixture's `auth`, `ledger` and `market` services import `backend/shared` (`COURSE_SHARED_USERS` in both copies of the fixture).

Notes on M10, for whoever touches facts or verify next:

- Discovery is `discovered_projects` in `crates/core/src/facts.rs`. The depth (`DISCOVERY_DEPTH`) and the skipped folders (`DISCOVERY_SKIPPED_DIRS`) are constants there. Per-project detection is `node_detail`, `python_detail`, `rust_detail` and `go_detail`.
- The repo-level `commands`, `packageManager` and `lockfiles` still describe the root only, because rules read them. Do not fold the projects' commands into them without checking `agent-ready/commands-documented` and `core/readme-setup`.
- Verify plans the root's steps and then one `ProjectPlan` per discovered project (`plan_verify` and `plan_projects` in `crates/core/src/verify.rs`). A step is `Skipped` (no command, makes the run partial) or `NotNeeded` (shown as `off`). Keep that difference: a test tries every mix and fails if a headline says `OK` with a skipped step.
- Blame-aware verify (now M19) must bump the verify schema to 6; M10 took 2, 0.1.2 took 3, 0.1.3 took 4 and M17 took 5. It also has to decide how labels work per project, now that projects run independently, and needs a time budget for the base run (PLAN.md).
- The course-project fixture exists twice with the same files: `course_project_files` in `crates/core/src/testing.rs` and `courseProjectFiles` in `packages/cli/src/course-fixture.ts`. The CLI tests put stand-ins for `python` and `npx` on the PATH (`writeToolShims`), so they need neither tool.
- `detect_facts` is called by four rules in one `check`, and each call walks the tree. Caching it per run would win back the 7 to 15% recorded in `bench/RESULTS.md`; mind the fix loop, where files change between calls.

Notes on 0.1.2, for whoever touches verify or Python detection next:

- The interpreter is chosen in `python_interpreter` in `crates/core/src/facts.rs`. `detect_facts` reads `VIRTUAL_ENV`; tests call `detect_facts_with(root, None)` instead, and the CLI test helper (`runCli`) drops `VIRTUAL_ENV` unless a test sets it.
- Commands on Windows name `.venv\Scripts\python.exe` with backslashes, on purpose (ADR 0022). Snapshot tests use the course fixture, which has no service venv, so they stay the same on every OS.
- Setup detection is `crates/core/src/setup.rs`: one function, a fixed list of messages. Add a caught case and a not-a-finding case for every new message.
- In `run_verify` the root and each project are units; `blocks()` is what stops a unit. A failed `check` stops all of them.
- `crates/testkit/src/python_stub.rs` builds `fscale-python-stub`, which the CLI tests copy into a `.venv` as its interpreter (`PYTHON_STUB` in `test-helpers.ts`).

Notes on 0.1.3, for whoever touches how verify runs commands:

- A command runs through `run_tree` in `crates/core/src/process.rs`: process group (Unix) or job object (Windows), the stop flag `STOP`, the time limit and the heartbeat. Change it only with the tests in `packages/cli/src/verify-run.test.ts`, which check with real process trees (`fscale-tree-stub` in the testkit) that nothing survives, on every OS. On Windows, `fscale-ctrl-break` sends a real Ctrl+Break.
- `run_verify` keeps its old signature (one job, no progress, default limits); `run_verify_with` takes `RunOptions`. Units run in parallel with `--jobs`, but reports are put back in plan order.
- Time limits are resolved by `VerifyConfig::timeout_for`. Progress lines are `crates/core/src/progress.rs`; run records `crates/core/src/runs.rs` (in `<temp>/firescale-runs`).
- Watch-mode detection is `watch_tool` and `script_commands` in `crates/core/src/facts.rs`; local-service detection is `service_problem` in `crates/core/src/setup.rs`.
- The M21 stop hook must stay fast: only the projects a change touches, and a time budget for the base run (PLAN.md).

Notes on M11, for whoever touches the group-project pack or M12 (`doctor`, the other half of SPEC §17):

- The pack is `crates/rules/src/group-project/`. `mod.rs` holds what the rules share: GitHub's three folders (`COMMUNITY_FOLDERS`), file lookup in any letter case, "counts only when tracked" (`counted`), and the fix precondition (`create_blocker`).
- `group-project/ci-verify` reuses `core/ci-exists`'s workflow reader and its `create_starter`; both are `pub(crate)` in `crates/rules/src/core/ci-exists/mod.rs`. Change the starter there and both rules follow.
- `group-project/branch-name` reads `GITHUB_HEAD_REF` only with a detached HEAD. Its tests call `check_with` so the test process's own environment cannot leak in.
- `Git::current_branch` (in `crates/core/src/git.rs`) gives the branch name also before the first commit.
- The skills test runs `fscale explain` in a scratch repo with every built-in pack on; a new pack needs nothing there.

Notes on M12, for whoever touches doctor or verify's service watch:

- Doctor is `crates/core/src/doctor.rs`; everything it asks the machine goes through the `Machine` trait (`ThisMachine` in production), so Rust tests can fake it. The CLI tests (`packages/cli/src/doctor.test.ts`) put version stand-ins on a PATH they control.
- Services come from `crates/core/src/services.rs` (Compose and `.env.example`), shared with verify. Never add a field that keeps a URL, a user or a password there; a test checks the `Debug` text.
- The live view of a running step's output is `crates/core/src/activity.rs` (heartbeat counts, the last 200 lines). Verify's early stop is `service_down_while_running` in `verify.rs`; a command stopped that way ends as `Ending::GaveUp`.
- The python stand-in answers `--version` with `Python <FSCALE_STUB_PYTHON_VERSION>` (default 3.12.4).
- The format research for M13 (Claude Code, Codex, Cursor) was done on 2026-11-12; see ADR 0027 once M13 lands. Cursor's hook format was confirmed from Cursor's public GitHub repositories (`cursor/cookbook`, `cursor/plugins`); cursor.com itself is still blocked.

Notes on M14, for whoever adds a generator:

- A generator is a `Generator` in `packages/cli/src/generators/` (`types.ts`), listed in `GENERATORS` in `index.ts`. It returns files and notes and writes nothing; the binary (`add` in `crates/cli/src/main.rs`) checks the paths and writes.
- Its schema may use only what `validate.ts` supports. The tests compile every printed schema with Ajv in strict mode.
- Each generator needs an example input in `generators.test.ts` (`INPUTS`): it gets the dry-run, snapshot and "adds no violation" tests from there.

Notes on M13, for whoever touches the agent files:

- The shared pieces are `crates/rules/src/agent-ready/managed.rs`: the block markers, `with_block` (append or replace, keeping line endings), `fscale_command` (how this repo starts fscale), `agents_block`, `claude_block`, and `FACTS_INPUTS` (cache inputs of rules whose output lists commands).
- Existing JSON settings are only ever changed through `firescale_core::jsonedit::insert_merge`, which adds new lines and refuses otherwise. Keep it that way: the owner's course project has hand-made `.claude` settings.
- `agent-skills` reads `firescale.config.json` itself (`resolve_config` with custom packs removed). M14's generator skill goes in `pack_skills`.
- `fscale hooks install` still re-serialises a settings file (M7); only the fixes use `insert_merge`.

Notes for M13 (agent files), written before it, which stands on M9 like M11 did:

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

- Tag `v0.2.1` on `main` and push the tag when you are ready to release (`RELEASING.md`). `v0.2.0` is tagged and 0.2.0 is on npm.
- Open the repo once in Codex as a trusted project and check that `.codex/config.toml` (the `firescale` permission profile) loads; it follows Codex's source but was not run (ADR 0027).
- Check `.cursor/cli.json` against Cursor's docs, which could not be read from the build session.
- Run 0.2.0 on the course project: `fscale doctor`, and `fscale adopt --dry-run` with `agent-ready` and `python` in `packs`, to see the agent files and the per-service checks on the real repo.

- Consider turning off token publishing for the eight packages now that two workflow releases have worked (`RELEASING.md`).

- Decide the items under "Needs your decision" in `DECISIONS.md`.
- Tick the second box of the publishing checkpoint: see a repo made by `npx firescale@0.1.0 init` pass its generated CI on GitHub (step 7 under "Publishing to npm" in `DECISIONS.md`).
- The manual test with a real agent (now M28) is not reachable yet.
