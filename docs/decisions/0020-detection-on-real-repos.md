# 0020 — Detection that works on real repos

Status: accepted (M10). Changes ADR 0011 (facts: projects are no longer only workspace members, schema version 2), ADR 0014 and ADR 0017 (verify: a skipped step is still not a failure by default, but it is no longer reported as a pass; schema version 2) and ADR 0012 (`core/env-example`). Renumbers the plan of ADR 0017.

## Context

The owner published 0.1.0 on 2026-11-12 and ran it on a real course project. Its root holds `docker-compose.yml`, `.env.example`, a README, `CLAUDE.md` and `.github/`. The code is in subfolders: `backend/` with five Python services (each with `requirements.txt`, `pytest.ini`, a `Dockerfile`, its own `.env.example` and a `unit_test/` folder; no `pyproject.toml`) and `frontend/` with a Vite app (npm, Playwright, oxlint). There is no workspace file at the root. Three things went wrong:

1. `fscale facts` reported no package manager, no languages and 0 projects. It only looked at the root and at workspace members.
2. `fscale verify` printed `verify: OK` while lint, typecheck and test were all skipped. A false green.
3. `core/env-example` produced 11 findings: 9 from test code and 1 for `DOCKER_HOST`.

This milestone was inserted before the group-project pack. Every later milestone moved up by one:

| Was        | Now        | Milestone                                 |
| ---------- | ---------- | ----------------------------------------- |
| —          | M10        | Detection that works on real repos        |
| M10, M11   | M11, M12   | Group-project pack, `doctor`              |
| M12, M13   | M13, M14   | Agent files from `init`, generators       |
| M14, M15   | M15, M16   | Next.js pack, Python pack                 |
| M16 to M18 | M17 to M19 | Blame-aware verify                        |
| M19 to M22 | M20 to M23 | Tamper guard                              |
| M23, M24   | M24, M25   | Guarded agent runner, adapters and ledger |
| M25, M26   | M26, M27   | Supabase pack, Expo pack                  |

Older ADRs and the older sections of DECISIONS.md keep the numbers they were written with.

## Decision

### Facts: projects in subfolders

- **Search depth: four folder levels below the root.** `backend/auth_service` is level 2. Four covers `apps/web`, `services/api/v1` and `packages/group/name/sub` without walking a whole tree. It is a constant (`DISCOVERY_DEPTH`), not a setting: `fscale facts` reads no config, by design (ADR 0011).
- **What is skipped.** Everything `.gitignore` ignores, every folder that starts with a dot, and a fixed list of dependency, cache and build folders (`DISCOVERY_SKIPPED_DIRS`). Only `.gitignore` files inside the repo are used, not the machine's global ignore file or `.git/info/exclude`, so two machines give the same answer. The search also works outside git.
- **Marker files.** `package.json` (node), `Cargo.toml` (rust), `go.mod` (go), and for python `pyproject.toml`, `setup.py` or `Pipfile`. A `requirements.txt` counts only with a `.py` file or a Python config file beside it. A lone `requirements.txt` is usually the input of a docs build or a container image, and listing it as a project would only produce skipped steps.
- **Nested projects: a project's folder is not searched further.** A marker file inside a project belongs to that project: an example app, a test fixture, a `fuzz/` crate. Listing them would make verify run their commands, which nobody asked for. The root is the exception, since the whole point is to look below it.
- **A workspace file is the authority for its ecosystem.** With a Node workspace at the root, the Node projects are its members; with a Cargo workspace, likewise for Rust. A folder a workspace leaves out (`!packages/skip`) stays out. Two existing tests pinned exactly this, and it keeps every repo that already had a workspace file unchanged. Other ecosystems are still searched: a pnpm monorepo with a Python service gets the service.
- **A folder can hold two projects**, one per kind, with the same `root`.
- **Each project has its own facts**: `languages`, `packageManager`, `lockfiles`, `tools`, `commands`, and `source` (`root`, `workspace` or `discovered`). The commands are meant to be run from the project's folder.
- **The repo-level fields keep their old meaning and gain a summary.** `packageManager`, `lockfiles` and `commands` still describe the repo root, because five rules read them and must behave as before. `languages` now also includes every project's languages, and the new `packageManagers` lists every package manager in use. `tools` gains `docker-compose`.
- **Tools come from config files and dependencies**, not only scripts. Where a project has no script for a step, the command comes from the tool: `oxlint`, `eslint .` or `biome check .` for lint; `vitest run` or `jest` for test; `tsc --noEmit` for typecheck.
  - Playwright and Cypress are reported as tools but never used as the test command. End-to-end tests need browsers and running services; running them from a stop hook would fail for reasons that have nothing to do with the change.
  - A `tsconfig.json` with `references` gets no typecheck command. `tsc --noEmit` checks nothing there, and `tsc -b` may write files. Verify must not change the repo.
  - npm tools run through `npx --no`, which fails instead of downloading a missing tool. Verify must not install things. (Checked against npm 10.9: with stdin not a terminal, plain `npx` assumes "yes".)
- **Python commands are `python -m pytest`, `python -m ruff check .` and `python -m mypy .`** with pip, and `uv run`, `poetry run` or `pipenv run` otherwise. The module form works in any environment where the tool is installed and puts the project's folder on the import path, which a service with `main.py` beside `unit_test/` needs. Each command exists only when its tool was found: pytest and mypy in the project's folder or its requirements files, ruff also in a config file in a folder above, since ruff itself looks upward.
- **A project at the root with no name is called `.`.** A folder name would differ between a real run and a dry run in a scratch copy. `core/readme-setup` ignores that name for its title.
- **Schema version 2.** New enum values (`python`, `go`) and new required fields could break a strict consumer.

### Verify: projects, and no false green

- **Each discovered project runs its own steps in its own folder**, after the root's steps, in folder order. Workspace members are covered by the root's commands, as before.
- **The root keeps exactly its old behaviour** when it has commands of its own. When it has none and projects exist, it lists no steps and `runner` is `projects`. A Python or Go project at the root now supplies the root's steps (before, such a repo skipped everything).
- **At the root, a missing script falls back to the tool's config file**, as in projects. The root `commands` fact stays "scripts only".
- **Three outcomes instead of two.** `ok`: every step ran and passed. `partial`: nothing failed but a step was skipped because no command was found. `failed`. The headline is `verify: OK` only for `ok`. For `partial` it is `verify: PARTIAL - 1 of 4 steps ran, 3 skipped`, every skipped step has its own line with the reason, and a `warning:` line names them all.
- **`off` is a new step status**, for a step the config sets to `false` or that another step covers. It was shown as `skipped` before. If it stayed "skipped", a repo that deliberately has no typecheck could never reach a plain OK. An off step is not counted in "N of M".
- **A partial run exits 0 by default.** The owner decided before M8 that a skipped step is not a failure (ADR 0017), and SPEC §10 says adopting the tool never breaks a build on day one. The headline and `result` carry the truth; the exit code keeps its meaning.
- **`verify.failOnSkipped: true` makes a partial run fail** (exit 1). It is the option for a team that has given every step a command and wants a missing one to be noticed.
- **`verify.projects`** sets things per project, keyed by folder: `false` turns a project off, an object sets or turns off single steps. A folder that is not a discovered project runs exactly the commands it is given, which is the way to verify a project below the search depth. A folder that does not exist is an error, so a typo cannot silently drop a check.
- **A top-level `verify.<step>` is a setting for the whole repo.** A command runs once from the root in place of every project's command for that step; `false` turns the step off everywhere. That is what the key meant before projects existed, so no existing config changes meaning. A project's own entry wins for that project.
- **Projects always run in full**, also with `--affected`, like package scripts and Cargo.
- **Schema version 2**: `result`, `stepsTotal`, `stepsRun`, `skipped`, `failOnSkipped`, `failedProject`, a `project` on each step, and the status `off`. `ok` now means "exit code 0". PLAN.md had reserved version 2 for blame-aware verify (now M17); that milestone bumps it again.

### The stop hook: a partial run does not block

The hook blocks only when verify fails. On a partial run it lets the agent stop and prints one line on stderr saying that the run was not a clean pass and which steps were skipped. With `verify.failOnSkipped` on, the hook blocks, because verify fails.

Why not block by default:

- A missing lint or test command is a fact about the repo, not something the agent's change broke. SPEC principle 10 is "blame the change, not the repo".
- The cheapest way for an agent to get past a block on skipped steps is to set them to `false` in the config. That is the very weakening the tamper guard is being built to catch. A default that pushes agents toward it is the wrong default.
- The hook gives up after 3 blocks anyway (ADR 0017), so blocking would cost three wasted turns in every repo without a complete setup and then let the agent go.

The cost: in a repo with nothing to run, an agent can stop without any check having run. It is told so, and so is anyone reading the hook's output. This is listed under "Needs your decision" in DECISIONS.md.

### `core/env-example`

- **Well-known variables** are ignored from one list, `crates/rules/src/core/env-example/well_known.rs`: exact names and prefixes set by the operating system, the shell, Docker, CI services and development tools. It is printed in the rule's SKILL.md (`fscale explain core/env-example`) and in `docs/rules.md`, and a test keeps all three equal. Docker variables are exact names, with no `DOCKER_` prefix, so a project's own `DOCKER_REGISTRY_URL` is still asked for. There is no option to take a name off the list; the `ignore` option adds names.
- **Test-only variables.** A name read only by test code is reported at low severity, and the fix writes it under its own comment, `# Test-only variables: read only by test code.` Test code is found by path: test folders (`test`, `tests`, `__tests__`, `unit_test`, `e2e`, `spec` and similar), `conftest.py`, `test_*.py`, `*_test.py`, `*_test.go`, `*.test.*` and `*.spec.*`. A name read by tests and by other code is a normal finding that points at the other code.
- **Why low severity and not silence.** A teammate who runs the tests still needs to know the variable exists. It is not what breaks the app.
- **A finding can carry its own severity.** `Finding` and `Diagnostic` gained an optional `severity`. The check report already had a severity per diagnostic, so its JSON format did not change. Custom TypeScript rules cannot set it yet.
- **The nearest `.env.example`.** A read is satisfied when any `.env.example` in the reading file's folder or a folder above it, up to the root, lists the name. A sibling folder's file does not count. A missing name goes into the nearest existing example file at or above the read, or the root file when there is none; the fix never creates a nested file. Findings are per name and target file, and name the file.
- **Test-only is decided per target file.** Only the reads that belong to one example file count for it.
- Example files are read from disk whether git tracks them or not, as the root file already was.

### Other rules

- `core/gitignore-basics` asks for the entries of every project's language, not only the root's. A repo with a Python service in a subfolder needs `__pycache__` and `.venv` ignored. Its cache inputs now include the marker files in subfolders.
- `agent-ready/single-test-command` still looks at the repo root only, so its results did not change.
- `init`, `adopt` and `sync` were not touched. Every existing scaffold test passes unchanged.

## Consequences

- The course project now reports six projects, and verify runs pytest in each service and oxlint and tsc in the frontend. It reports `PARTIAL` until each remaining step has a command or is set to `false`.
- A repo that printed `verify: OK` with skipped steps now prints `verify: PARTIAL`. Its exit code is unchanged. Anything that matched on the text `verify: OK` sees the difference; anything that reads `ok` in the JSON does not, and should read `result`.
- A repo with a root manifest and an unlisted project in a subfolder (a Python service beside a pnpm workspace) now has that project's steps run by verify. `verify.projects` turns it off.
- Tests changed on purpose, none weakened:
  - `verify`: the unit test that asserted `verify: OK` for a run with a skipped step now asserts `PARTIAL`; that assertion was the bug. One CLI test expects `off` instead of `skipped` for a step the config turns off. Five snapshots show `off` for such steps.
  - `facts`: the schema version is 2; the mixed-repo test lists the root's new Python project; the human-output test and two snapshots show the new per-project lines.
- `fscale check` on a 50,000-file repo is 7 to 15% slower (p50 cold 674 to 719 ms, `--no-cache` 352 to 406 ms on the build machine): four rules each call `detect_facts`, which now walks four levels of folders, and `core/env-example` looks for nested example files. Detecting the facts once per run would win this back.
- Known limits are listed in `docs/facts.md` and `docs/verify.md`. The main ones: no project graph for Python or Go; a discovered project that is itself a workspace is one project; Python commands assume `python` on the PATH with the tools installed; test code is recognised by path only; projects run one after another.
