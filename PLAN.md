# firescale — Build Plan

One milestone per session and per pull request. A milestone is done only when every box under "Done when" is true.

Status key: ☐ not started · ◐ in progress · ☑ done

---

## M0 — Skeleton ☑

**Build**

- pnpm workspace + Cargo workspace, laid out as in SPEC §9.
- TypeScript strict config, ESLint, Prettier, Vitest.
- Rust: `cargo fmt`, `clippy` with warnings denied, `cargo test`.
- GitHub Actions CI on Windows, macOS and Linux: install, lint, typecheck, test, build.
- A `fscale --version` command that calls one trivial function in the Rust core through the Node binding. This proves the bridge works on all three OSes.
- README with a one-paragraph pitch and dev setup.
- `docs/decisions/` folder with an ADR for each real choice made (binding approach, test runner, and so on).

**Done when**

- ☑ CI is green on all three OSes.
- ☑ `pnpm build && pnpm test` passes from a clean clone.
- ☑ `fscale --version` prints a version that came through the Rust binding.

---

## M1 — Rule SDK and the first rule ☑

**Build**

- `packages/sdk`: `defineRule`, `Diagnostic`, `FixResult`, rule context, and a fixture-based test helper.
- Config loader for `firescale.config.ts` / `.json` (SPEC §10).
- `fscale check` with human output and `--json`.
- `fscale explain <rule>`.
- One real rule end to end: `core/no-committed-env`, with check, fix, SKILL.md and tests.
- A SKILL.md template and a test that validates every SKILL.md has the required sections.

**Done when**

- ☑ `fscale check` finds a tracked `.env` in a fixture repo and exits 1 when the rule is `enforced`.
- ☑ `fscale check --json` output matches a documented schema, with a snapshot test.
- ☑ The rule test covers: passing repo, failing repo, fixed repo, fix run twice (idempotent).

---

## M2 — Rust front door ☑

Design change, see ADR 0008: `fscale` is a native Rust binary, core rules are built in, and Node is started only for TypeScript rules, `.ts` config, generators and agent adapters.

**Build**

- `crates/cli`: the `fscale` binary with `check`, `explain` and `--version`. Same behaviour, same exit codes and the same `--json` schema as M1.
- Config loading for `firescale.config.json` in Rust. `firescale.config.ts` is loaded by starting Node.
- `core/no-committed-env` ported to Rust as a built-in rule, with the same tests.
- The Node bridge: one child process, JSON lines. Runs a custom rule defined with the M1 SDK.
- Remove the napi binding and the TypeScript CLI and rule pack it replaces.
- Small fixes: flag `.ENV` case variants; the SKILL.md validator ignores headings inside code fences; `--rules <id>` runs a rule that is `off`, with a note; Node 22 in the CI matrix; GitHub Actions pinned to commit SHAs.
- `bench/startup.mjs` and startup numbers before and after in `bench/RESULTS.md`.

**Done when**

- ☑ `fscale` is a Rust binary, and the M1 CLI snapshot tests for `check`, `explain` and `--version` pass against it unchanged.
- ☑ `fscale check --json` output validates against the unchanged JSON schema.
- ☑ A repo with only built-in rules runs with no Node process, proven by a test.
- ☑ `core/no-committed-env` is built in, with tests for: passing repo, failing repo, fixed repo, fix run twice.
- ☑ A sample custom TypeScript rule runs through the bridge, and a `.ts` config loads through Node, each proven by a test.
- ☑ Startup time for `fscale --version` and `fscale check`, before and after, is recorded in `bench/RESULTS.md`.
- ☑ CI is green on all three OSes, on Node 22 and Node 24.

---

## M3 — Engine: walk, hash, cache ☑

**Build** (all Rust, in `crates/core`)

- Parallel, gitignore-aware file walker.
- Glob matching and content hashing.
- Rule-result cache keyed by rule version + input-file hashes + config hash. Stored under `.firescale/cache/`.
- Parallel scheduling of rule checks. Custom TypeScript rules are batched through the one Node child.
- `bench/`: a generator for large fixture repos and a benchmark script.
- Loose ends from M2: an airtight "no Node process" test, tests for the automatic lookup of the Node host, the `core/no-committed-env` SKILL.md mentions case variants, and benchmarks recorded from Windows and macOS CI runners.

**Done when**

- ☑ A second `fscale check` with no file changes is served from cache, proven by a test.
- ☑ Changing a file inside a rule's `inputs` invalidates only that rule's result, proven by a test.
- ☑ Benchmarks for cold and warm runs on a 50k-file fixture are recorded in `bench/RESULTS.md`.
- ☑ All Rust code passes clippy with warnings denied.

---

## M4 — `fix`, SARIF and facts ☑

**Build**

- `fscale fix` and `fscale fix --dry-run` for built-in rules, and for custom rules through the bridge.
- SARIF output (`--sarif`).
- Stack detection (`fscale facts`, `--json`) from marker files, and the simple project graph.
- `--affected` support using git diff plus the project graph.

**Done when**

- ☑ `fscale fix` fixes `core/no-committed-env` in a fixture repo, and a second run changes nothing.
- ☑ `fscale fix --dry-run` prints a diff and changes nothing.
- ☑ `--sarif` output validates against the SARIF 2.1.0 schema, with a snapshot test.
- ☑ `fscale facts --json` has a documented schema and a snapshot test.

---

## M5 — Core rule pack ☑

**Build**

- The remaining seven `core` rules from SPEC §6, in Rust, each with check, SKILL.md, tests, and a deterministic fix where one is safe.

**Done when**

- ☑ All 8 core rules pass their fixture tests on all three OSes.
- ☑ Every fix is idempotent, proven by tests.
- ☑ Every SKILL.md passes validation.
- ☑ Running `fscale check` on this repo itself reports zero violations (dogfooding).

---

## M6 — npm packaging ☑

**Build**

- Per-platform binary packages and the `firescale` launcher package, as designed in ADR 0008 §6.
- A release workflow that builds the binary for each platform and assembles the packages. Publishing itself stays a manual, approved step.
- Install smoke tests: `npm`, `pnpm` and `yarn`, on all three OSes, from locally packed tarballs.

**Done when**

- ☑ `npx firescale --version` works from packed tarballs on all three OSes with no Rust toolchain.
- ☑ A custom TypeScript rule works from an npm install, proven by a smoke test.
- ☑ The launcher's overhead is measured and recorded in `bench/RESULTS.md`.

---

## M7 — `verify` and agent hooks ☑

**Build**

- `fscale verify` per SPEC §8, in Rust, with the compact agent-readable summary and `--json`.
- Runner detection: Nx, Turborepo, or plain package scripts.
- `fscale hooks install` for Claude Code, Codex and Cursor. Confirm each agent's current hook format from its official docs before writing it.
- Loop guard and retry cap.
- The `agent-ready` rule pack, in Rust.
- From this milestone on, this repo's own agent hooks call `fscale verify`.

**Done when**

- ☑ `fscale verify` exits non-zero with a clear summary when a fixture repo has a failing test.
- ☑ Hook files are generated for three agents, with snapshot tests.
- ☑ The loop guard is covered by a test.
- ☑ p50 and p99 verify latency are recorded in `bench/RESULTS.md`.

---

## Direction change after M7

On 2026-11-11 the direction was refined (ADR 0015): firescale stays a toolkit for small teams, the rules become the scaffold, every repo is born agent-ready, and a referee keeps agents honest. Everything below was rewritten then, and reordered once more by the owner (ADR 0017): the Next.js and Python stack packs moved up, and a publishing checkpoint was added after M9. On 2026-11-12 the owner added M10 from findings on a real repo, and every later milestone moved up by one (ADR 0020). Later the same day the owner put affected-only verify and a step cache before blame-aware verify, and faster generated CI after it (ADR 0031): the blame-aware milestones moved up by two and everything after them by three. The numbers below are the current ones. Milestones are sized for one session each. Themes, in order:

| Theme                        | Milestones |
| ---------------------------- | ---------- |
| Rules are the scaffold           | M8, M9, then the publishing checkpoint |
| Detection that works on real repos | M10      |
| Group-project mode               | M11, M12   |
| Born agent-ready                 | M13, M14   |
| Stack packs: Next.js and Python  | M15, M16   |
| Referee: affected verify, cache  | M17, M18   |
| Referee: blame-aware verify      | M19 to M21 |
| Faster generated CI              | M22        |
| Referee: tamper guard            | M23 to M26 |
| Guarded agent runner, ledger     | M27, M28   |
| Stack packs: Supabase and Expo   | M29, M30   |

One thing outside the milestones that they depend on:

- **Cursor's documentation.** It could not be reached from the build environment during M7. M13 needs it.

Owner decisions that apply to everything below (ADR 0017):

- Blame-aware labels are on by default whenever there is a base commit.
- The stop hook lets an agent go after 3 blocks in a row.
- A verify step with no command is skipped and prints a visible warning.

---

## M8 — The fix loop ☑

The engine under `init`, `adopt` and `sync`: run fixes in order until nothing is left to fix.

**Build**

- A rule can declare the rules that must be fixed before it (`after`), for built-in rules and in the TypeScript SDK. A cycle or an unknown id is an error that names the rules, raised before anything is written.
- A fixed-point loop in `crates/core`: check, run the deterministic fix of every enabled rule with violations in dependency order, repeat until no deterministic violation is left.
- The loop stops with an error, and a non-zero exit code, when a pass changes nothing while deterministic violations remain, when two fixes keep rewriting the same file, or when the pass limit is reached. The error names the rules and files.
- `fscale fix` uses the loop. `fscale fix --dry-run` shows the combined diff of all passes, which means a dry run must let later fixes see what earlier ones would have written (a known limit from M4).
- An ADR for the loop: the `after` syntax, the pass limit, and how the dry run works.

**Done when**

- ☑ Two fixture rules where B must run after A are fixed in one `fscale fix`, in that order, proven by a test. The same works for a custom TypeScript rule with `after`.
- ☑ A dependency cycle and an unknown `after` id are each rejected with a clear error, proven by tests.
- ☑ Two fixture rules that undo each other stop the loop with an error naming both, and so does a loop that reaches the pass limit. The exit code is not 0.
- ☑ `fscale fix --dry-run` with two dependent fixes prints the same diff that `fscale fix` then produces, and changes nothing.
- ☑ Running `fscale fix` a second time changes nothing, for every built-in rule with a fix.

---

## M9 — `init`, `adopt` and `sync` ☑

**Build**

- `fscale init [--packs <list>]`: in an empty or new folder, run `git init` if needed, write `firescale.config.json`, then run the fix loop until `fscale check` passes. No templates. The default packs are `core` and `agent-ready`.
- Fixes that create files for the core rules that lack one and can have one safely: a starter CI workflow that runs `npx firescale verify` (`ci-exists`), and a README skeleton with install and run sections (`readme-setup`). Rules that need a real decision stay agent or manual.
- `fscale adopt [--dry-run]`: the same loop on an existing repo. It never replaces content a person wrote: fixes create missing files, merge into structured files, append, or rewrite inside a managed block. It ends with a list of what is left for a person or an agent.
- `fscale sync` and `fscale sync --check`: run the loop again to repair drift; `--check` changes nothing and exits 1 when a fix would change something.
- `docs/init.md`.

**Done when**

- ☑ `fscale init --packs core` in an empty folder yields a repo where `fscale check` reports no deterministic violations, and prints the violations that are left for a person.
- ☑ `fscale adopt --dry-run` on a fixture repo prints a diff and changes nothing; `fscale adopt` then leaves no deterministic violations.
- ☑ In a fixture repo with its own README, CI workflow and `.gitignore`, `fscale adopt` leaves every existing line in place, proven by a test.
- ☑ `fscale sync --check` exits 1 after a generated file is deleted, and 0 after `fscale sync`.
- ☑ `init`, `adopt` and `sync` start no Node process in a repo with only built-in packs, proven by a test.

---

## Checkpoint — Owner publishes 0.1.0 to npm ◐ (manual, after M9)

Not a build milestone. The owner publishes `firescale` 0.1.0 to npm by hand, following "Publishing to npm" in DECISIONS.md.

Why here: the CI workflow that `init` generates (M9) runs `npx firescale verify`, and teammates install firescale from npm. Until 0.1.0 is published, generated CI and group-project mode work only in tests.

Published by the owner on 2026-11-12. `npx firescale@0.1.0 init` and `npx firescale@0.1.0 adopt --dry-run` work on Windows and on Linux.

**Done when**

- ☑ `npx firescale@0.1.0 --version` works from an empty folder on a machine without this repo.
- ☐ A repo made by `fscale init` passes its own generated CI on GitHub.

---

## M10 — Detection that works on real repos ☑

Added by the owner after running 0.1.0 on a real course project (ADR 0020). Its root has `docker-compose.yml`, `.env.example`, a README and `.github/`, with the code in subfolders: `backend/` holds five Python services and `frontend/` a Vite app. There is no workspace file at the root. 0.1.0 reported no package manager, no languages and no projects there, printed "verify: OK" with lint, typecheck and test all skipped, and nine of the eleven `core/env-example` findings came from test code.

**Build**

- `fscale facts` finds projects in subfolders: marker files (`package.json`, `pyproject.toml`, `requirements.txt`, `setup.py`, `Pipfile`, `Cargo.toml`, `go.mod`) down to four folder levels, respecting `.gitignore` and skipping dependency, cache and build folders. Each project gets its own languages, package manager, lockfiles, tools and commands, and the repo-level summary lists them all. Linters and test runners are detected from their config files, not only from package scripts. Docker Compose at the root is a tool.
- `fscale verify` runs each discovered project's steps in that project's folder. The headline never reads as a clean pass when a step was skipped: it says `verify: PARTIAL - 1 of 4 steps ran, 3 skipped` and names each skipped step. `verify.failOnSkipped` makes skipped steps fail, and `verify.projects` sets or turns off the steps of one project. `--json` carries the same information (schema version 2).
- `core/env-example`: well-known system and tooling variables are ignored, from one documented list; variables read only by test code are reported at low severity and written under a separate "test-only" comment by the fix; a read is satisfied by the nearest `.env.example` at or above the reading file.

**Done when**

- ☑ A fixture repo shaped like the one described reports its Python services and its frontend as separate projects in `fscale facts`, with correct languages and commands, on all three OSes.
- ☑ `fscale verify` in that fixture runs the frontend's and the backend's steps, and names every step it skipped.
- ☑ No output says a plain "OK" when any step was skipped, proven by a test.
- ☑ The three env-example changes each have a caught case and a not-a-finding case.
- ☑ `init`, `adopt` and `sync` behave as before on existing fixtures.

---

## Fixes in 0.1.2 ☑ (after running 0.1.1 on the same course project)

Not a milestone; 0.1.2 was released through the Release workflow on 2026-11-12. 0.1.1 was released through the Release workflow on 2026-11-12 with npm trusted publishing, and every stage passed. Running it on the course project found five problems, fixed in one pull request (ADR 0022):

- ☑ Python steps run with the project's own `.venv` or `venv` (`Scripts/python.exe` on Windows, `bin/python` elsewhere), else uv, poetry or pipenv, else an active `VIRTUAL_ENV`, else `python` from the PATH, which is said. `fscale facts` shows the interpreter of each project.
- ☑ One project's failure no longer stops the others; the headline names every failure.
- ☑ A step that fails because the environment is not ready is reported as setup needed, with the command that fixes it, and the stop hook tells an agent not to edit code for it.
- ☑ Ruff and mypy are detected from requirements files and dev-dependency lists.
- ☑ `core/env-example` treats test-runner config files as test code.

---

## Fixes in 0.1.3 ☑ (after running 0.1.2 on the same course project)

Not a milestone. Running 0.1.2 on the course project on Windows: verify printed nothing for over ten minutes (one service's pytest took 13 minutes, waiting on a test database that was not running), Ctrl+C left node and python processes running, later runs seemed to hang, and the frontend's `test` script was `vitest` (watch mode). Fixed in one pull request (ADR 0024):

- ☑ Live progress on stderr: a line when each step starts and ends, and a heartbeat for a long step. On in a terminal, `--progress` and `--no-progress` override; `--json` stays clean.
- ☑ Ctrl+C and termination stop every command with its whole process tree (process groups on macOS and Linux, job objects on Windows), then print the summary of what finished, exit 130. Tested on all three OSes.
- ☑ A time limit per step (default 10 minutes; `verify.timeout`, `<step>.timeout`, per project and per project step). A timed-out step is stopped with its tree and reported as `TIMED OUT` with its last lines; other projects run.
- ☑ Commands get no input and run with `CI=1` unless `verify.ci` is `false`.
- ☑ A watching test script is replaced by a run-once sibling (`test:run`, `test:ci`, `test:once`) in `fscale facts`, or warned about.
- ☑ A refused connection to a local service is setup needed, with `docker compose up -d` when a compose file exists.
- ☑ `--jobs <n>` runs independent projects in parallel; the summary keeps the plan's order.
- ☑ A second verify never waits silently: verify takes no lock, says which other verify is running, and names commands an earlier killed run left behind.

---

## M11 — The group-project pack ☑

Needs the publishing checkpoint: the CI this pack checks for runs `npx firescale verify`, which only works for a real team once 0.1.0 is on npm. If it is not published yet, say so at the top of the pull request.

**Build**

- A `group-project` pack, in Rust, opt-in. Rules:
  - a pull request template exists (fix creates it),
  - a contributing guide exists (fix creates it, with the commands from `fscale facts` and the branch naming pattern),
  - CI runs `fscale verify` on pull requests (runs after `core/ci-exists`, whose fix writes that workflow; a hand-written workflow without verify is agent-fixable, not rewritten),
  - the current branch name matches a pattern from the rule's options (no fix).
- Each rule has a SKILL.md and fixture tests: passing, failing, fixed, fixed twice.

**Done when**

- ☑ All group-project rules pass their fixture tests, and every fix is idempotent.
- ☑ `fscale init --packs core,group-project` in an empty folder yields a repo where the pack reports no violations.
- ☑ The generated pull request template and contributing guide have snapshot tests.
- ☑ A hand-written CI workflow is never rewritten, proven by a test.

---

## M12 — `fscale doctor` ☑

Needs the publishing checkpoint: a teammate runs `doctor` from an npm install, and the generated CI that tells them to needs 0.1.0 on npm. If it is not published yet, say so at the top of the pull request.

Also done (owner's additions, ADR 0025 and 0026): local services from Compose files and `.env.example` (reachable or not, with the start command, never a password); per-project virtual environment and requirements checks; verify's hint to run `fscale doctor` on setup needed; the heartbeat's running count for pytest and unittest; a running step stopped early when a service it needs is down (`verify.stopWhenServiceDown`).

**Build**

- `fscale doctor [--json]`: checks the machine, not the repo.
  - Tool versions against what the repo declares (`packageManager`, `engines`, `rust-toolchain.toml`, `.nvmrc`, `.python-version`).
  - Every variable named in `.env.example`: set in the environment, or present in a local env file. Names only; values are never read out or printed.
  - Dependencies installed and matching the lockfile.
- Each problem comes with the command that fixes it. Exit 1 when something is wrong.
- A JSON schema for `--json`, and `docs/doctor.md`.

**Done when**

- ☑ `fscale doctor` reports a missing tool, a wrong version, a missing env variable and missing dependencies in fixture setups, each with a next step, proven by tests.
- ☑ `fscale doctor` exits 0 on this repo on a machine that can build it.
- ☑ A test proves that no output contains the value of an env variable.
- ☑ `--json` output validates against its schema, with a snapshot test.

---

## M13 — Born agent-ready: agent files from `init` ☑

Formats confirmed in ADR 0027 (Claude Code from its docs, Codex from its source, Cursor from its official GitHub examples; Cursor's `cli.json` permissions unconfirmed).

**Build**

- First confirm the current Claude Code, Codex and Cursor formats for skills, hooks and permissions from their official docs, and record them in an ADR. If Cursor's docs still cannot be reached, its files are written as "unconfirmed", as in M7, and the pull request says so at the top.
- Fixes for the `agent-ready` rules:
  - `agents-md`: an AGENTS.md with a **managed block** (commands from `fscale facts`, the verify rule). Text outside the block is never touched. This also satisfies `commands-documented`.
  - `verify-hook`: the hooks, using the command that starts fscale in this repo.
- Two new rules with fixes: safe default permissions for each agent (deny reading env files and secrets; allow the repo's own check commands; existing settings are merged, never replaced), and skills installed where each agent looks for them (the SKILL.md of every enabled rule, plus skills a pack ships).
- `fscale sync` repairs a hand-edited managed block.

**Done when**

- ☑ `fscale init` in an empty folder yields a repo where the whole `agent-ready` pack passes, for every agent whose format was confirmed.
- ☑ Text outside the AGENTS.md managed block survives `fscale sync` unchanged, proven by a test.
- ☑ Generated agent files have snapshot tests, and an existing agent settings file keeps its other settings.
- ☑ `fscale sync --check` fails after a hand edit inside the managed block.

---

## M14 — Generators: `add` and `schema` ☑

Built as ADR 0028 says: the generators live in `packages/cli/src/generators/` so they ship in the `firescale` package; `packages/generators` is a placeholder.

**Build**

- `packages/generators`: the generator interface in TypeScript, called through the bridge.
- `fscale add <generator> --json <input> [--dry-run]` and `fscale schema <generator>`. Input is validated against the schema before anything is written.
- Two real generators to prove the interface: a workspace package, and a custom rule (rule folder with check, SKILL.md and test).
- A skill that tells agents to call `fscale add` instead of hand-writing boilerplate, shipped by the `agent-ready` pack and installed by M13's skills rule.

**Done when**

- ☑ `fscale schema <generator>` prints a valid JSON Schema, and `fscale add` rejects input that does not match it with an error naming the field.
- ☑ `fscale add --dry-run` prints a diff and changes nothing.
- ☑ Each generator has snapshot tests, and its output passes `fscale check`.
- ☑ After `fscale sync`, the skill is present for each agent, proven by a test.

---

## M15 — Stack pack: Next.js ☑

Done (ADR 0029): six rules, `init --packs core,nextjs` builds a starter whose `npm install`, `npm run build` and `npm run lint` succeed (run by hand on 2026-11-12, output in the pull request).

## M16 — Stack pack: Python ☑

Done (ADR 0030): six rules, per project, so a repo of services with their own requirements files, `pytest.ini` and `.venv` and no `pyproject.toml` passes as it is. `init --packs core,python` builds a starter whose pip install, pytest and ruff succeed (run by hand on 2026-11-12, output in the pull request).

One milestone and one pull request per pack. Moved up from the end of the plan by the owner (ADR 0017), so that real stacks can be scaffolded before the referee is built. Every stack pack, here and in M29 and M30, has this shape.

**Build**

- Research the stack's current official setup docs first, and record what the pack assumes in an ADR.
- Five to eight rules whose fixes create or repair files, each with a SKILL.md and fixture tests.
- `fscale init --packs core,<stack>` builds a starter through the fix loop.

**Done when**

- ☐ `fscale init --packs core,<stack>` in an empty folder yields a repo where the pack reports no violations.
- ☐ In that repo, the stack's own install and build or test commands succeed. Run by hand where CI cannot install the stack, with the output in the PR.
- ☐ `fscale adopt --dry-run` proposes no change on a fixture repo of that stack that is already set up correctly, and exactly the expected change on one with a known gap.
- ☐ Every fix is idempotent, proven by tests.

---

## M17 — Project graph and affected verify ☑

Added by the owner (ADR 0031): make verify cheap before making it fair. Decisions in ADR 0032 (the graph) and ADR 0033 (tree-sitter for imports).

**Build**

- Extend the graph in `fscale facts` beyond workspace files. Edge sources, most trusted first:
  1. Local paths in manifests: requirements files (`-e ../x`, `../x`, `-r`), `pyproject.toml` path sources, `package.json` `file:`, `link:` and `workspace:`.
  2. A Dockerfile `COPY` of a sibling folder, and Compose `build.context` and `depends_on`.
  3. Python imports that resolve to a sibling project's top-level package, and JavaScript or TypeScript relative imports that cross a project root. A real parser, not regex. No new parser for TOML or YAML unless an ADR says why.
- A folder that projects import or copy but that is not a project itself (for example `backend/shared`) is a shared-source node. A change there affects only the projects that use it.
- Shared inputs: a root `.env.example`, Compose file or OpenAPI spec read by several projects is an input of each of them.
- Stay safe when unsure. A changed file that no project owns, or a changed root lockfile, CI workflow, Compose file or firescale config, affects every project. A project with a dynamic import (`importlib`, `__import__`) gets a warning and counts as depending on every other Python project.
- `fscale graph` prints projects and edges; `--explain` says which source gave each edge; `--json` has a schema. `fscale facts` shows the same edges.
- `fscale verify --affected [--base <ref>]` runs lint, typecheck and test only for affected projects: the owners of changed files and everything that depends on them. Projects left out are listed as `not affected`, a state apart from skipped. The headline says how many projects ran out of how many and never reads as a plain OK for the whole repo. Verify schema 5.
- The stop hook does not change.

**Done when** (each proven by a test)

- ☑ On the course-project fixture, with services importing a shared folder: a change in the shared folder affects every service that imports it; a change in one service affects only that service; a frontend change affects only the frontend; a change to the root Compose file affects everything.
- ☑ Each edge source has a fixture, and `--explain` names the source.
- ☑ A dynamic import gives the warning and the wide edge.
- ☑ With no base and a clean tree, everything is affected, with a note.
- ☑ `facts`, `graph --explain` and `verify --affected` planning (no installs, no scripts) run on the three real repos in DECISIONS.md. Small problems fixed with fixture tests; the rest logged.

---

## M18 — Local step-result cache and timings ☑

Decisions in ADR 0036.

**Build**

- Cache the result of a verify step that **passed**, under `.firescale/`. A failure always runs again.
- The key: the firescale version, the command, the hashes of the project's own files, the hashes of the projects and shared-source nodes it depends on (M17's graph), the lockfile, the versions of the tools the step runs, the OS and CPU architecture, and the values of an allowlist of environment variables. Nothing else from the environment.
- **Never cache a step that needs a network service that is not in the repo.** A service the repo starts itself (Compose) can be part of the key.
- Record each step's time, and show in the summary what the cache saved. `FSCALE_LOG=cache` shows hits and misses, as for `check`.
- An ADR for the key, the allowlist, where results live and how they are cleaned up.

**Done when** (each proven by a test)

- ☑ A second `fscale verify` with no change runs no step again, and says so.
- ☑ A change in a project's file, in a project or shared folder it depends on, in the lockfile, in a tool's version, or in an allowlisted variable runs that project's steps again; a change elsewhere does not.
- ☑ A failed step is never cached.
- ☑ A step that needs a service outside the repo is never cached.
- ☑ No environment value outside the allowlist reaches the key or the cache folder.
- ☑ Timings with a cold and a warm cache are recorded in `bench/RESULTS.md`.

---

## M19 — Blame-aware verify: the base run ☑

Decisions in ADR 0037. The worktree gets its dependencies by linking the head's `node_modules` and virtual environments, with `verify.baseInstall` for an install or a build (ADR 0037 says why not an install by default).

**Build**

- An ADR for blame-aware verify. It supersedes two points of ADR 0014: with a base, verify runs **every** step instead of stopping at the first failure (otherwise an old lint failure would hide a new test failure), and `--base <ref>` works without `--affected`.
- Definitions: the **head** is the working tree, uncommitted changes included, because that is what an agent's stop hook judges. The **base** is the merge base of the head commit and `--base <ref>`, or of the default branch when `--base` is not given.
- Run verify on the base commit in a temporary git worktree. Never touch the user's working tree or index. The worktree gets its dependencies by running the repo's install command there (from `fscale facts`, or a new `verify.install` config key).
- Cache base results under `.firescale/` with the M18 step cache: the same key, computed on the base commit's files.
- Label at step level and for rule diagnostics: `new`, `pre-existing`, `fixed`. Only `new` makes `fscale verify` exit 1. The labels are on by default whenever there is a base commit (owner decision, ADR 0017); there is no flag to turn them on.
- With no base that differs from the head commit and a clean tree, or outside git, verify behaves as it does today: every failure blocks.
- The summary and `--json` carry the labels (bump the schema version; M10 took version 2, 0.1.2 took version 3, 0.1.3 took version 4 and M17 took version 5, so this is 6). Update `docs/verify.md` and `docs/affected.md`.
- **The base run reuses the M18 step cache** (ADR 0031). A project whose inputs are equal on the base and the head needs no base run: its result on the head is its result on the base. Only projects whose inputs differ are run in the base worktree.
- **The base run needs a time budget** (from running 0.1.2 on the course project, where one service's tests took 13 minutes). A base run that does not finish within its budget is abandoned and verify falls back to today's behaviour for the steps it did not cover, with a note; it never makes the stop hook wait for the base. Each step of the base run keeps its own time limit (ADR 0024). The budget has a default and a config key.

**Done when**

- ☑ In a fixture repo whose test already fails on the base commit, `fscale verify` exits 0 and labels the failure `pre-existing`.
- ☑ A failure introduced on the head, committed or not, is labelled `new` and exits 1; one repaired on the head is labelled `fixed`.
- ☑ A pre-existing lint failure does not hide a new test failure: both are reported, proven by a test.
- ☑ A second run with the same base makes no base run, proven by a test; changing the lockfile invalidates the cache.
- ☑ A project whose inputs are equal on the base and the head gets no base run, proven by a test.
- ☑ The user's working tree and index are byte-for-byte unchanged after a run, proven by a test.
- ☑ On the default branch with a clean tree, a failing test still exits 1.

---

## M20 — Blame-aware verify: per-test labels ☑

Decisions in ADR 0038. Vitest and Jest results come from their default reporters' failure lines, not their JSON reporters, which need flags on the user's command.

**Build**

- Failure identity below step level, so one new failing test is not hidden by ten old ones. Read per-test results from two runners to start with: Vitest or Jest (JSON reporter) and pytest (JUnit XML). Other runners keep step-level labels.
- An ADR for how verify asks a runner for machine-readable results without changing the user's command, and for the per-test identity (file plus test name).
- The summary names each `new` failing test.

**Done when**

- ☑ With two failing tests, one old and one new, only the new one is labelled `new` and named in the summary, for both runners.
- ☑ A runner without per-test support falls back to step-level labels, with a note, proven by a test.
- ☑ A renamed test file does not turn old failures into `new` ones silently: the behaviour is defined in the ADR and tested.

---

## M21 — Blame-aware verify: reruns and the stop hook ☑

Decisions in ADR 0039. `cargo test` per-test labels are read from its text output.

**Build**

- Rerun a `new` failure once before blocking. A pass on the rerun is labelled `flaky` and does not block. No flaky history is stored.
- The stop hook (`fscale hooks run`) uses blame-aware verify, so an agent is only sent back for failures it introduced.
- **The stop hook must be fast** (from running 0.1.2 on the course project, where a full verify took over ten minutes). It runs only the projects the change touches (the projects of the changed files, as `--affected` finds them, plus the root when root files changed), never a full run, and the base run stays within its time budget (M19). Record its latency on the course-project fixture.
- `cargo test` per-test labels from its stable text output, if that proves reliable; otherwise recorded as a limit.
- p50 and p99 of blame-aware verify, with a cold and a warm base cache, in `bench/RESULTS.md`.

**Done when**

- ☑ A test that fails once and then passes is labelled `flaky` and does not block, proven with a fixture that fails on its first run only.
- ☑ Nothing about past flaky results is written to disk, proven by a test.
- ☑ The stop hook lets an agent stop when every failure is `pre-existing`, and blocks on a `new` one.
- ☑ The stop hook runs only the projects a change touches: a change in one service runs that service's steps and no other project's, proven by a test.
- ☑ Latency numbers are recorded.

---

## M22 — Faster generated CI and a shared cache ◐ (the last box needs 0.3.0 on npm)

Decisions in ADR 0041, with what GitHub's docs said on 2026-11-13. The pull request job has `cache-mode: read` and only restore steps; records are small (key, result, time, output digest, writer) and used only when written from the default branch; laptops do not read the shared cache yet (see "Later").

**Build**

- The CI workflow that `fscale init` writes (and `core/ci-exists` and `group-project/ci-verify` accept) runs `fscale verify --affected --base <the pull request's base>` on pull requests and a full `fscale verify` on pushes to the default branch.
- The M18 step cache is kept in the GitHub Actions cache. **It is written only by pushes to the default branch.** Pull requests and laptops read it and never write it (CVE-2025-36852, ADR 0031).
- A restored result is used only when its whole key matches. A run never uploads what it restored.
- Confirm the current GitHub Actions cache API and its scoping rules from GitHub's docs first, and pin what the workflow uses in tests.

**Done when**

- ☑ The generated workflow has a snapshot test, and a pull request job in it cannot write the shared cache, proven by a test that reads the workflow.
- ☑ A step result restored from the shared cache is reused only on an exact key match, proven by a test.
- ☐ A pull request in a fixture repo on GitHub runs only its affected projects and gets cache hits from the default branch. Run by hand, with the output in the pull request. Open: it needs a firescale on npm with `--shared-cache` (0.3.0, not yet published).

---

## M23 — Tamper guard: design and the first language ☑

Decisions in ADR 0042: net counts over the changed test files, from tree-sitter syntax trees of the base and the working tree; the policy (`verify.tamper`) from the base commit's config; report-only unless that says `enforced`. A test deleted with every module it uses is not counted. No new grammar.

**Build**

- Study trustbut, goodhart and skylos first. Write the tamper guard ADR: what each does, what firescale does differently (syntax-tree based, anchored to the base commit, measured false-positive rate), and how the rates will be measured.
- An ADR for tree-sitter as a dependency: binary size and build time before and after.
- Parse base and head with tree-sitter for JavaScript and TypeScript. Count, per file and in total: tests, skipped tests, assertions.
- Compare by net counts, so a moved or renamed test is not a finding. Hard-fail: tests deleted or newly skipped, and a net loss of assertions.
- Runs as part of `fscale verify`, reported with file and line.

**Done when**

- ☑ Skipping a test, deleting a test and removing an assertion are each caught in JavaScript and TypeScript fixture repos.
- ☑ Moving a test to another file, renaming it, and adding tests are not findings.
- ☑ Binary size and build time before and after are in `bench/RESULTS.md`.

---

## Fixes in 0.4.0 ☑ (after running 0.3.0 on the same course project)

Not a milestone. Running 0.3.0 on the course project (Windows 11) blocked on a head with no code change. Fixed in one pull request (ADR 0043):

- ☑ Per-test labels decide first: the same failing tests on head and base, with different durations, paths and order, are `pre-existing`; a new failing test beside them is `new`, named, and blocks; a test failing on both for a clearly different reason blocks.
- ☑ On Windows tool versions are read the way the step starts (`npm.cmd`, `npx.cmd`, `pnpm.cmd`, `yarn.cmd`), so Node steps are cached; a pass that is not stored or not reused says why, without `FSCALE_LOG`.
- ☑ Base results are kept, passed or failed, keyed as M18 keys them; a second verify with no change runs no base step again.
- ☑ The head's ignored `.env` and `.env.local` (root and each project) are copied into the base and head-check worktrees (copied, not linked, so a base step cannot overwrite yours); no value from them is written anywhere.
- ☑ A base step runs only for a step that did not pass on the head (`verify.labelFixed` brings back `fixed`).
- ☑ Time limits follow progress: 10 minutes with no new output (`verify.idleTimeout`), 60 minutes in all (`verify.timeout`), a per-project limit still wins; the time left from a printed percent.
- ☑ Lint or typecheck failing on a package that is not installed is setup needed, with the install command.
- ☑ Doctor and the setup-needed text name `npm ci` (and the frozen-lockfile install of other managers).
- ☑ Unowned images, documents, archives and media run no project; any other unowned file still runs every project.
- ☑ Several projects with the same setup reason print it once, with each project's commands under it.
- ☑ Each pass is stored in the step cache as soon as its step finishes, so a run stopped later keeps it; a config change that alters every key is said once.
- ☑ The tamper guard of M23 is unchanged. The version is 0.4.0; no tag pushed.

---

## M24 — Tamper guard: more languages and policy ☑

Decisions in ADR 0044: pytest and unittest, then Rust (`tree-sitter-rust` 0.24.2); the misses ADR 0042 admitted closed where the syntax tree makes them certain; lint suppressions brought forward from M25; the policy keys `testPaths` and `allow`, read from the base commit.

**Build**

- The same counts for Rust and Python.
- Policy in the firescale config: protected test paths and allowed exceptions. It is read from the **base commit**, never from the head.
- Added by the owner for this milestone: Python first (the owner's project is five pytest services: deleted tests, `skip`, `skipif`, `xfail`, a bare `return` or `pass` replacing the body, removed assertions); `if (false)` or an early `return` before a test's assertions, and an assertion replaced by one that cannot fail, each with a caught case and a not-a-finding case; new lint-disable comments and `allow` attributes as net counts (from M25).

**Done when**

- ☑ Skipping a test, deleting a test and removing an assertion are each caught in Rust and Python fixture repos, and moves and renames are not findings (`bench/tamper-corpus.json`, `py:` and `rs:` cases, run by `tamper.test.ts`).
- ☑ Loosening the policy in the same change has no effect, proven by tests: `"off"` or `{ "policy": "off" }`, a wider `allow`, and a dropped `testPaths` entry in the change all leave the base's policy in force.
- ☑ A file firescale cannot parse is reported as "not checked", never as clean, in every language (a `not-checked` case for each).
- ☑ Each new kind of finding has a caught case and a not-a-finding case in every language; the counts are in `bench/RESULTS.md`, marked as fixture counts, not a measured rate (M26).

---

## M25 — Tamper guard: config and CI ☑

Decisions in ADR 0045: one measure model for every config detector (compared per setting over the whole change), small readers for JSON, YAML, TOML, INI and JS object literals, `fscale tamper`, the base from GitHub's pull request event (`--pr-base`), and a `tamper-guard` job on `pull_request_target` that the pull request cannot change and that runs none of its code (changing ADR 0041 on that point).

**Build**

- More detectors, same base-versus-head method:
  - ~~new lint-disable comments and `allow` attributes~~: done in M24 (ADR 0044), counted per rule over the whole change rather than per file; suppressions turned off in linter config files stay here,
  - lowered coverage thresholds in the common config files,
  - CI steps removed or made unable to fail (`continue-on-error`, `|| true`, a deleted verify step),
  - `--no-verify` in scripts, hooks or CI.
- Added by the owner for this milestone: in CI the base comes from the pull request's base as GitHub gives it, not from local git state, and the generated workflow runs the guard that way; the ADR says what is and is not protected locally. A deleted or weakened verify step in the workflow itself is a finding, read from the base's copy of the workflow.

**Done when**

- ☑ Each of the remaining three detectors (and rules turned off in linter config) has a caught case and a not-a-finding case in fixture repos (`bench/tamper-corpus.json`, prefixes `cov:`, `ci:`, `nv:`, `lint:`; `tamper.test.ts` checks each prefix has both).
- ☑ Every finding names the file, the line, and what changed against the base ("… (on the base: …, file:line)", or "… is gone" at its line on the base commit).
- ☑ Owner's additions: `--pr-base` takes the base from GitHub's event, proven after moving the base ref and `git replace`-ing the base commit (`tamper-ci.test.ts`); the generated workflow's `pull-request` job uses it and its `tamper-guard` job runs `fscale tamper --pr-base` from the default branch's copy of the file (`shared-cache.test.ts`, `core::ci_exists::tests`); deleting the verify step from the workflow is `ci-weakened` against the base's copy (`ci: the verify step deleted from the workflow`).

---

## M26 — Tamper guard: approvals and measured rates ☑

Decisions in ADR 0046: `snapshot-changed` and `expected-changed` need a person's approval, read only from GitHub reviews in the `pull_request_target` run; rates measured on real history (labelled by fresh subagents) and on a held-out corpus of cheating changes.

**Build**

- Human approval for the ambiguous cases, changed expected values and changed snapshots: verify blocks until a person records an approval. An ADR decides where approvals live so that an agent cannot write its own.
- A labelled corpus of honest and cheating changes, and a script that measures the false-positive rate and the miss rate.
- Added by the owner for this milestone: approvals an agent in the working tree cannot write, with the ways to forge them and why each fails (CI is where they are enforced, since nothing is safe locally); at least 1,000 real commits from public JavaScript, TypeScript, Python and Rust repos and this repo, every finding labelled by a fresh subagent, rates per language with intervals; misses on a corpus of cheating changes written for that purpose and kept apart; limits beside every number; false positives fixed, each with a corpus case, rates before and after; no version change, no tag.

**Done when**

- ☑ A changed snapshot and a changed expected value each block until approved, proven by tests (`tamper-approvals.test.ts`, "a changed expected / snapshot: blocks until a person approves the pull request on GitHub").
- ☑ An approval written by the change under review does not count, proven by a test (same file, "does not count an approval the change could make itself": files and trailers in the change, the author's own review, an older commit, no write access, a bot, a dismissed approval, `--approvals` outside `pull_request_target`).
- ☑ The false-positive rate and the miss rate on the corpus are recorded in `bench/RESULTS.md`: on 1,200 public commits 36.3% of findings false before the fixes and 20.5% after, with intervals per language; 42 of 98 held-out cheats missed; the limits beside them.

---

## M27 — Guarded agent runner ☑

Decisions in ADR 0047: an adapter is a program speaking a small protocol (`--version`, `auth status`, `run <request.json>`); the runner puts the ten guardrails in order around it; `fake`, from the testkit, is the only adapter until M28.

**Build**

- The runner and every guardrail in SPEC §7, in Rust, each with its own test. That includes guardrail 10: each run appends one line to `.firescale/ledger.jsonl` (rule, agent, pass or fail, diff size, duration; cost when known).
- A fake agent adapter, so tests and CI never call a real model.
- The re-check after a run is the referee (SPEC §8): blame-aware verify plus the tamper guard.
- Added by the owner for this milestone: stand-in agent binaries in the testkit; never wider permissions than the repo's own agent settings; never push, tag or publish; the ledger stores no prompt text, no file contents and no secrets.

**Done when**

- ☑ With the fake agent: a good fix is committed, a bad fix is rolled back, a fix touching a protected path is rejected, and a fix that skips a test is rejected by the tamper guard (`agent-runner.test.ts`, guardrails 7 and 8).
- ☑ The runner refuses to start on a dirty tree or when secrets are tracked (guardrail 1).
- ☑ Every guardrail in SPEC §7 has a test named after it, and a pass and a fail each append one schema-valid ledger line (guardrails 1 to 10; the ledger lines are validated with `docs/schemas/ledger-entry.schema.json`).

---

## M28 — Agent adapters and `fscale ledger` ☐ (finish and test locally)

**Build**

- `packages/agents`: the adapter interface plus Claude Code and Codex adapters, in TypeScript, called through the bridge. Confirm each CLI's flags from its current docs and pin them in tests.
- Adapters report cost when the agent's CLI gives it, and the ledger records it.
- `fscale ledger [--json]`: runs, pass rate and total cost, per rule and per agent.

**Done when**

- ☐ `fscale ledger` summarises a fixture ledger, with a snapshot test, and copes with a damaged line.
- ☐ A fake-agent run that reports a cost shows that cost in `fscale ledger`.
- ☐ Manual local test: a real agent fixes one real violation end to end. Result noted in the PR.

---

## M29 — Stack pack: Supabase ☐

## M30 — Stack pack: Expo ☐

One milestone and one pull request per pack, with the same "Build" and "Done when" lists as M15 and M16.

---

## Later (not ordered)

- Docs site. Publishing to npm is automated from 0.1.1 on: a pushed tag `v<version>` builds, publishes with npm trusted publishing and verifies (ADR 0021, `RELEASING.md`). The tag is the owner's to push.
- Ingest alint output when alint is installed, and report it as diagnostics.
- Gemini CLI: stop hook and adapter.
- A public benchmark built from fix ledgers.
- A thin MCP server; Nx plugins for EAS and Supabase.
- Test-level selection: run only the tests a change can reach, below the project level (ADR 0031).
- Laptops read the shared step cache of CI (ADR 0041): today only CI reads it. It needs a token on each machine and a way to check who wrote a record that does not rest on the cache's scoping.
