# Decisions log

A running log, newest milestone first. For each milestone: what was built, decisions made on your behalf (with the ADR), known limits, and what needs your decision. Anything you could reasonably disagree with is under "Needs your decision".

## CI owed

Nothing is owed. The last open item, M6's npm install test on Windows, passed on 2026-11-11 in the CI run you started by hand on `main` (run 42, commit `9190058`): all ten jobs were green, the checks on Linux, macOS and Windows, the three benchmark jobs and the three npm install jobs.

History: GitHub Actions stopped starting jobs on 2026-11-11 (the account's monthly limit), so M6 was merged on Linux results only, and M7's first two runs did not execute. Both were made up later the same day. If jobs ever end within seconds with no logs again, list here what is unproven on macOS or Windows.

Still not measured anywhere: verify latency on macOS and Windows. The `bench` job runs the 50k-file check benchmark, not `bench/verify-bench.mjs`.

## Needs your decision (all milestones, most important first)

- **(M10) Should the agent stop hook block when verify is only partial?** Today it does not. With steps skipped for lack of a command, `fscale verify` prints `verify: PARTIAL`, exits 0, and the hook lets the agent stop with a one-line note on stderr. To make it block, set `"verify": { "failOnSkipped": true }`; then verify exits 1 and the hook blocks. I kept "do not block" as the default because a missing command is not something the agent's change broke, because the easy way past such a block is to set the steps to `false` (the weakening the tamper guard is meant to catch), and because the hook lets go after 3 blocks anyway. The cost: in a repo with nothing to run, an agent can stop with no check having run. Say if you want the hook to block by default, or want `failOnSkipped` on by default (ADR 0020).
- **(M10) A partial verify exits 0.** That follows your earlier answer that a skipped step is not a failure. CI therefore stays green on `PARTIAL` unless `failOnSkipped` is on. The starter workflow that `fscale init` writes does not turn it on. Say if generated CI should.
- **(M10) Projects are searched four folder levels deep, and the search has no setting.** Deeper projects are reached by naming their folder under `verify.projects`. Say if you want a config key for the depth or for extra folders to skip.
- **(M10) A `requirements.txt` alone is not a project.** It needs a `.py` file or a Python config file beside it. Your services qualify through `pytest.ini`, `alembic.ini` and `main.py`.
- **(M10) Python commands are `python -m pytest` and `python -m ruff check .`** They need `python` on the PATH with the tools installed, so an activated virtual environment. If your services each have their own `.venv`, verify will not find it by itself: set the command per project under `verify.projects`, or say if verify should look for `.venv` in the project's folder.
- **(M10) Ruff counts as configured** when a `ruff.toml`, `.ruff.toml` or `[tool.ruff]` section is in the service's folder or a folder above it, or when a `requirements*.txt` of the service lists `ruff`. Otherwise the service's lint step is skipped and named.
- **(M10) Playwright is listed as a tool but never run as the test step.** End-to-end tests need browsers and running services. Your frontend's test step is therefore skipped until it has a `test` script, a vitest or jest config, or a command under `verify.projects`.
- **(M10) Which variables `core/env-example` always ignores.** The list is in `docs/rules.md`. It includes `DEBUG`, `HTTP_PROXY` and `NODE_OPTIONS`, and has no `DOCKER_` prefix. There is no option to take a name off it.
- **(Plan) The second box of the publishing checkpoint is still open.** A repo made by `npx firescale@0.1.0 init` has not yet been seen passing its own generated CI on GitHub. Step 7 under "Publishing to npm" below says how to check it.
- **(M9) `init` and `adopt` run `git add` on the README and the workflow they create.** Nothing else is staged and nothing is committed (ADR 0019). The alternative is to make the two rules count untracked files. Say if you would rather have that.
- **(M9) `adopt` writes a config with `core` only; `init` uses `core` and `agent-ready`.** That follows SPEC §6. Say if `adopt` should turn on `agent-ready` too.
- **(M9) `fscale init` takes no folder argument.** It works on the current folder: `mkdir my-app && cd my-app && fscale init`. `fscale init my-app` could be added.
- **(M9) What the starter workflow contains.** It runs on pull requests and on pushes to `main` and `master`, sets up Node 24, installs with npm or pnpm when it can tell, and runs `npx firescale verify` with no version pinned. Read it once in `docs/init.md` or by running `fscale init` in an empty folder.
- **(M7) The Cursor hook format is unconfirmed.** cursor.com was blocked from the build session, twice. `fscale hooks install` writes `.cursor/hooks.json` from a best reading and prints a note. Someone with access to Cursor's docs should check it (ADR 0014), or allow `cursor.com` in the cloud environment's network settings so a session can.
- **(M7) The `agent-ready` pack is opt-in**, not on by default. Say if you want it on for every repo.

- **(M6) The npm name `fscale` is taken by an unrelated package.** `npx firescale` works; `npx fscale` would run that other package. After `npm install firescale` the command `fscale` is ours. If that risk bothers you, the options are to ask npm about the name or to make `firescale` the only command.
- **(M6) No postinstall step, so one Node start sits in front of every `fscale` run from npm.** ADR 0013 explains why. Agent hooks can call the binary directly. Say if you want the postinstall optimisation anyway.

0. **Are the core rules' heuristics right for you?** (M5, ADR 0012) Seven rules were written quickly, tuned to avoid false alarms. The table in ADR 0012 says exactly what each one flags. The ones most worth your eye: `no-secrets-in-source` (which formats, what is skipped), `readme-setup` (what counts as install and run steps), and whether `single-lockfile` and `scripts-present` should cover Rust and Python.
1. **Should the result cache be on by default right now?** (M3, ADR 0010) With the one cheap rule that exists today, a warm cached `check` on a 50k-file repo is slower than `--no-cache` on every OS (p50: 64 ms against 33 ms locally; on CI runners 109 against 27 ms on Linux, 102 against 49 ms on macOS, 370 against 144 ms on Windows), because proving "nothing changed" means walking the repo. The cache will pay off for content-scanning rules in M5. Options: (a) keep it on, as now; (b) make it opt-in until M5; (c) let each rule declare whether it is worth caching. I kept (a) because PLAN.md's "Done when" describes a cache that is simply on.
2. **Removing the napi binding outright.** (M2, ADR 0008 §3) Nothing calls from Node into Rust any more. If you want the TypeScript SDK to reuse the Rust walker or hasher in-process, a thin binding would have to come back.
3. **"No Node process" under an npm install.** (M2, ADR 0008 §6) The usual npm layout puts a small Node launcher in front of the binary. The design adds an optional hard-link step that removes it where install scripts are allowed. Decide whether that is acceptable when M6 (npm packaging) is built.
4. **Custom rule packs are listed by path only.** (M2, ADR 0008 §5) Loading a pack by npm package name is not supported.
5. **`fscale fix` also fixes rules that are only `evaluated`, and exits 0 when violations remain.** (M4, ADR 0011) If you would rather fix only `enforced` rules, or fail when something is left, say so.
6. **`--affected` has no default base.** (M4, ADR 0011) Without `--base` it only looks at uncommitted and untracked files. A default such as `origin/main` could be added.
7. **No TOML or YAML parser.** (M4, ADR 0011) `fscale facts` reads `Cargo.toml` and `pnpm-workspace.yaml` with small hand-written readers. Adding the `toml` crate would be more robust at the cost of a dependency.
8. **Custom TypeScript rules are never cached.** (M3, ADR 0010) They run on every `check`. Caching them needs a way to hash their source.
9. **`.envrc` is not flagged** by `core/no-committed-env`; `.ENV`-style names are. (M1/M2, ADR 0007 point 10)
10. **TypeScript is pinned to 6.0.3**, not 7, until `typescript-eslint` supports 7. (M0, ADR 0002)

## A note on milestone numbers

M10 was added on 2026-11-12 (ADR 0020), and every later milestone moved up by one: the group-project pack is now M11, `doctor` M12, agent files M13, generators M14, Next.js M15, Python M16, blame-aware verify M17 to M19, the tamper guard M20 to M23, the runner M24, adapters and the ledger M25, Supabase M26 and Expo M27. The sections below, from M9 down, keep the numbers they were written with.

## M10 — Detection that works on real repos

Built from what you found when running 0.1.0 on your course project. All decisions are in ADR 0020.

**Recorded first**

- You published 0.1.0 to npm on 2026-11-12. `npx firescale@0.1.0 init` and `adopt --dry-run` work on Windows and Linux. The first box of the checkpoint in PLAN.md is ticked; the second is open.

**Built**

- `fscale facts` finds projects in subfolders and reports each with its own languages, package manager, lockfiles, tools and commands. A repo-level summary lists every language and package manager. Docker Compose at the root is a tool. On a repo shaped like yours it reports five Python projects (pip, pytest, and ruff where configured) and one TypeScript project (npm, Vite, Playwright, oxlint, knip).
- `fscale verify` runs each discovered project's lint, typecheck and test in that project's folder. A run with a skipped step prints `verify: PARTIAL - 9 of 19 steps ran, 10 skipped`, never `verify: OK`, and names every skipped step with its reason.
- `verify.failOnSkipped` makes skipped steps fail. `verify.projects` sets or turns off the steps of a single project, and can name a folder the search did not find.
- `core/env-example`: system and tooling variables are ignored from one documented list; variables only test code reads are low severity and go under a "test-only" comment; a read is satisfied by the nearest `.env.example` at or above the file.
- Both JSON formats are now schema version 2: `fscale facts --json` and `fscale verify --json`.
- `docs/facts.md` and `docs/verify.md` rewritten, ADR 0020, and a benchmark entry in `bench/RESULTS.md`.

**Decisions made for you** (all ADR 0020; the ones you may disagree with are also under "Needs your decision")

- **Depth limit: four folder levels below the root.** Not configurable.
- **Skipped while searching:** everything `.gitignore` ignores, every folder starting with a dot (`.venv`, `.pytest_cache`, `.ruff_cache`), and `node_modules`, `bower_components`, `vendor`, `site-packages`, `venv`, `env`, `__pycache__`, `dist`, `build`, `out`, `target`, `coverage`, `htmlcov`, `test-results`, `playwright-report`, `tmp`, `temp`, `logs`.
- **Nested projects: a project's folder is not searched further.** A `package.json` inside a project (an example, a fixture) belongs to that project. Only the root is always searched.
- **A workspace file at the root decides its ecosystem.** With a pnpm or npm workspace, Node projects are its members only; a folder the workspace excludes stays excluded. The same for a Cargo workspace. Other ecosystems are still searched. This keeps every repo that has a workspace file as it was.
- **The repo-level `packageManager`, `lockfiles` and `commands` still describe the root only.** Five rules read them. New: `packageManagers` lists them all, and `languages` includes every project's.
- **Commands from config files:** `oxlint`, `eslint .` or `biome check .`; `vitest run` or `jest`; `tsc --noEmit`. A script of the same name wins. npm tools run through `npx --no`, which never downloads.
- **No typecheck command for a `tsconfig.json` with `references`.** `tsc --noEmit` checks nothing there and `tsc -b` could write files.
- **`off` is a new step status.** A step you set to `false`, or one that another step covers, used to show as `skipped`. It now shows as `off` and does not make a run partial.
- **A top-level `verify.lint`, `typecheck` or `test` applies to the whole repo.** A command runs once from the root in place of each project's own; `false` turns the step off everywhere. A project's entry under `verify.projects` wins for that project.
- **A folder under `verify.projects` that does not exist is an error** (exit 2).
- **Projects run in full with `--affected`.**
- **A root Python or Go project now gets commands.** Before, such a repo skipped every step.
- **`core/gitignore-basics` now asks for the entries of every project's language**, so a repo with a Python service in a subfolder needs `__pycache__` and `.venv` ignored. This is the one rule whose findings can change in an existing repo.
- **`agent-ready/single-test-command` still looks at the root only.**
- **Test-only variables are low severity, not hidden.** A teammate running the tests still needs them.
- **The fix adds a missing variable to the nearest existing `.env.example`** at or above the reading file, else to the root file. It never creates a nested one.
- **A finding can have its own severity.** Built-in rules only; the TypeScript SDK cannot set it yet.

**Tests changed on purpose, none weakened**

- The unit test that asserted `verify: OK` for a run with a skipped step now asserts `PARTIAL`. That assertion was the false green.
- One CLI test expects `off` where it expected `skipped`, for a step the config turns off. Five snapshots changed the same way.
- Facts: schema version 2; the mixed-repo test now lists the root's Python project; the human-output test and two snapshots show the new per-project lines.
- No `core/env-example` test and no `init`, `adopt` or `sync` test changed.

**Known limits**

- `fscale check` is 7 to 15% slower on a 50,000-file repo, because four rules each detect the facts. See `bench/RESULTS.md`.
- No project graph (`dependsOn`) for Python or Go projects.
- A discovered project that is itself a workspace is listed as one project.
- Two projects can share a name; `root` tells them apart.
- Projects run one after another. On your repo that is six test runs in sequence.
- Test code is recognised by its path only, and case-sensitively.
- The always-ignored variable list cannot be shortened by config.
- Custom TypeScript rules cannot give a finding its own severity.
- Other limits are in `docs/facts.md` and `docs/verify.md`.

## M9 — `init`, `adopt` and `sync`

**Built**

- `fscale init [--packs <list>] [--force]`: in an empty folder, runs `git init` if needed, writes `firescale.config.json`, then runs every fix. No templates.
- `fscale adopt [--dry-run]`: the same loop on an existing repo. Writes a config if there is none.
- `fscale sync` and `fscale sync --check`: the loop again. `--check` changes nothing and exits 1 when `sync` would change something.
- Two new fixes, each creating a file only when it is absent: `core/ci-exists` writes a starter workflow that runs `npx firescale verify`; `core/readme-setup` writes a README skeleton with Install and Usage sections.
- Every command prints what it changed, then "Left for you" (violations no fix can repair) and "To review" (what to look at in a file a fix created).
- `docs/init.md` and ADR 0019.

**How "never destroys work" is ensured**

- The config, the starter workflow and the README skeleton are created with a call that fails if anything is at that path.
- The two new fixes look at the disk again before writing, and write only where nothing is: no workflow file of any name, tracked or not; no root entry whose name starts with "readme" in any case.
- A path git already tracks is never created or staged, so staged work in the index cannot be replaced.
- No fix writes to a symbolic link or below one, so nothing outside the repo is touched.
- If a folder cannot be listed, or git cannot answer, nothing is created.
- A README without install steps and a workflow without a pull request trigger are reported, not edited.
- Tests run `adopt`, `sync` and `init --force` over repos whose README, workflow, `.gitignore` and AGENTS.md contain CRLF, a BOM, no final newline, non-ASCII text and look-alike markers, tracked, untracked and outside git, and compare every file byte for byte.

**Decisions made for you** (all ADR 0019)

- **`init` refuses a folder that is not empty and has no firescale config.** "Empty" means nothing, or only `.git`. It exits 2, changes nothing and prints the two `adopt` commands. `--force` runs anyway and is as careful as `adopt`.
- **`init` in a folder that already has a config keeps the config** and runs the fixes, like `sync`. `--packs` is an error there.
- **`init` in an empty folder inside another git repo or firescale repo refuses**, because its files would be staged in that repo. `--force` overrides it.
- **Fixes stage the file they create** (`git add` on that one file), because the two rules count tracked files only, and an existing test pins that an untracked workflow does not count.
- **A README or workflow that git tracks but that is missing from disk is not recreated.** It is reported as a manual fix (`git restore` it, or commit the deletion). A file gone from disk and from git is drift, and `sync` recreates it.
- **A fix that meets a symbolic link stops with an error** instead of writing through it. For a linked `.gitignore` that means `adopt` exits 2 until the link is replaced or the rule is turned off.
- **A path that `.gitignore` ignores is not created**; the rule says which path.
- **Every run prints `Repo root: <path>` first**, since `adopt` and `sync` work on the root found by walking up from the current folder.
- **A file that exists but is untracked is a manual fix** ("run `git add`"). Nothing is created beside it.
- **No generic "no line may change" guard in the loop.** It would break honest merges into JSON files and M12's managed block. The property is proven by tests instead.
- **Exit code 0 when the fixes settled, whatever is left**, as `fscale fix` does. 1 only for `sync --check` with drift. 2 for refusals and errors.
- **`--rules` is rejected by all three**, and an option that belongs to another command is an error that names the right one.
- **`init` has no `--dry-run`.** In an existing folder, `adopt --dry-run` is the preview.
- **The README title is the root project's name, or "Project name".** Not the folder name, which would differ in a dry run.
- **The branch name of `git init` is left to your git settings.**
- **Four existing assertions changed on purpose**, none weakened: a missing workflow and a missing README are now `deterministic` instead of `agent` (PLAN.md asks for these fixes), both rules' metadata tests now expect `has_fix`, and the README rule's cache inputs became case-insensitive. One fix-loop test got a README and a workflow added to its fixture so the two new fixes stay out of its `.gitignore` diff; its assertions are unchanged. The help snapshot changed because the help text gained the three commands.

**Known limits**

- `agent-ready` has no fixes until M12, so a default `fscale init` lists two violations under "Left for you".
- The generated workflow fails at `npx firescale verify` until 0.1.0 is on npm.
- The starter workflow has an install step for npm and pnpm only. For yarn, bun and others it leaves a comment and says so under "To review".
- In a dry run, what custom TypeScript rules would still report afterwards is not listed.
- `adopt --dry-run` and `sync --check` work in a copy that lacks files git ignores and the repo's own git settings (`.git/info/exclude`, sparse checkout). In such repos the preview can differ from the real run.
- Writes are not atomic. If the disk fills up or the process is killed while a fix appends to `.gitignore`, the file can be left cut short.
- The cached result of the two rules does not notice a new folder or link named like a README, or `.github` changing from a folder to a file. `fscale check` can then show a stale fix hint until `--no-cache`; `init`, `adopt` and `sync` do not use the cache.
- On a file system that ignores case, a workflow tracked under `.GitHub/` is reported as untracked. Not tested.
- With `GIT_DIR` set in the environment, `init` and `adopt` stage their two files in that repository, as any git command would.
- An independent review before the pull request found ten issues, three of them serious: `sync` could replace a staged README in the git index with the skeleton; an untracked README could be overwritten in a folder that could not be listed; and a linked `.github` or `.gitignore` led a fix to write outside the repo. The others: a file left on disk unreported when `git add` failed, previews that disagreed with the real run around links, empty folders and ignored paths, a `.git` left behind by `init` on a broken config, and output that never named the repo root. All are fixed with tests, except the limits listed above.
- The three commands print text only; no `--json`.
- The pinned action versions in the generated workflow will age. They are in one place, `crates/rules/src/core/ci-exists/mod.rs`.

## M8 — The fix loop

**Built**

- Rules can declare `after`: the rules whose fixes must run before their own. Built-in rules (`RuleMeta.after`) and custom TypeScript rules (`defineRule({ after })`).
- The fix loop in `crates/core/src/fixloop.rs`: fixes run in dependency order, pass after pass, until a pass finds nothing left to fix. `fscale fix` uses it.
- Errors that stop the loop and name the rules and files: a dependency cycle, an `after` id that no rule has, two fixes that undo each other, a fix that never settles (10 passes), and a fix that does nothing about the violations it claims.
- `fscale fix --dry-run` now runs the real loop in a scratch copy of the repo, so it shows the combined result of all fixes and passes.

**Decisions made for you** (all ADR 0018)

- **The pass limit is 10 and is not configurable.**
- **A dry run copies the repo** (tracked and unignored files) to the temp folder and runs the real fixes there. A virtual overlay would have been faster but could not give every rule the same view; rules read through git and straight from disk too.
- **`fscale fix` now exits 2 when the fixes cannot settle.** Before, a fix that did nothing about its own violations ended in "Nothing to fix" and exit 0.
- **Nothing is rolled back when the loop stops with an error.** The changes made so far stay and are listed in the error.
- **An `after` entry that names a rule which exists but is off places no constraint.** Only an id that no rule has is an error. Turning one rule of a cycle off breaks the cycle.
- **`after` only orders fixes.** It has no effect on `check`.
- **`core/no-committed-env` now declares `after: core/gitignore-basics`.** It is the first built-in use, and it is true: one creates `.gitignore`, the other adds to it.
- **A write that does not change a file's content is no longer reported as a change**, for built-in and custom fixes.
- **A fix counts as stuck only after two passes in a row** with violations and no change, so that a custom fix writing files by itself is not called broken.
- **One existing test changed its fixture, not its assertion:** "shows a new file as a diff against /dev/null" asserted a preview that the old dry run got wrong when two rules touch `.gitignore`. It now turns on one rule; a new test covers two.

**Known limits**

- A dry run of a very large repo takes seconds, because of the copy.
- A fix that depends on a file git ignores (and does not track) can preview differently from the real run.
- If `fscale` is killed during a dry run, a `firescale-dry-run-*` folder stays in the temp folder.
- A custom fix that writes files without using its context is invisible to the loop.
- An `after` id that no rule has stops `fscale fix`, also when it belongs to a custom pack the repo has not listed.
- An independent review before the pull request found three real defects, all fixed with tests: a dry run started from a git hook could untrack files in the real index (git's `GIT_DIR` and `GIT_INDEX_FILE` variables were inherited); the "fixes undo each other" check fired on one rule writing its own file twice; and a custom fix that writes files without its context failed the run.

## Plan update and your answers (before M8)

**Changed** (ADR 0017)

- The `nextjs` and `python` stack packs moved to directly after M13 (now M14 and M15). Later milestones were renumbered: blame-aware verify is M16 to M18, the tamper guard M19 to M22, the runner M23, adapters and `fscale ledger` M24. `supabase` and `expo` are still M25 and M26. ADR 0017 has the full old-to-new table; the "Direction change" section below still uses the old numbers.
- A publishing checkpoint after M9: you publish 0.1.0 to npm by hand. M10 and M11 note that generated CI needs it.
- CI trimmed (ADR 0016, see "CI compute" below). This replaces pull request #11, which was closed unmerged.

**Your answers, now recorded as decisions**

- Blame-aware labels are on by default when there is a base commit. No flag.
- The stop hook releases an agent after 3 blocks in a row.
- A verify step with no command is skipped and prints a visible warning.

**Decisions made for you**

- **The warning was built now**, in the same pull request as M8, because it is a small change to what M7 built and would otherwise sit recorded but untrue until the next verify milestone. `fscale verify` prints a `warning:` line under the summary, and `--json` has a `warnings` list. A step you turn off with `false` gives no warning. The verify snapshots changed on purpose.
- **Next.js is M14 and Python M15**, in the order you named them.
- **The checkpoint is not numbered**, so no milestone number shifts because of it.

## Direction change (after M7, docs only)

**Written**

- ADR 0015: firescale stays a toolkit for small teams and gains five changes: rules are the scaffold, born agent-ready, a built-in referee (blame-aware verify and a tamper guard), group-project mode, and a ledger.
- SPEC.md updated to match. Sections 1 to 14 keep their numbers, because ADRs and code comments cite them; the new sections are 15 (rules as the scaffold), 16 (born agent-ready) and 17 (group-project mode). The referee is in section 8, with verify.
- PLAN.md rewritten from M8 onward: nineteen milestones, M8 to M26, each sized for one session.
- README.md's first paragraph now gives the new pitch.

**Decisions made for you** (all ADR 0015)

- **The docs describe the product in its own words.** Your brief described it by comparison with another product, by name. AGENTS.md's originality rule says that name must never be used in this repo, so the ADR, SPEC and PLAN say "a toolkit for small teams" instead. The meaning is unchanged.
- **Milestone order: yours, with adjustments.** (1) The fix loop (ordering by declared dependencies, the convergence error) is its own milestone, M8, before `init`, because `init`, `adopt` and `sync` all stand on it. (2) The study of trustbut, goodhart and skylos opens the tamper guard (M17) instead of being a milestone of its own. (3) The ledger file is written by the runner (M21), since that is one of its guardrails; `fscale ledger` and cost come with the adapters (M22).
- **Themes are split so a milestone fits a session:** scaffold (M8 loop, M9 commands), group project (M10 pack, M11 doctor), agent-ready (M12 agent files, M13 generators), blame-aware verify (M14 base run, M15 per-test labels, M16 reruns and the stop hook), tamper guard (M17 design and JavaScript, M18 Rust, Python and policy, M19 config and CI detectors, M20 approvals and measured rates), runner (M21 guardrails and ledger file, M22 adapters and `fscale ledger`).
- **`fscale init` turns on `agent-ready` by default**, because you asked that `init` also writes the agent files. For `fscale check` in an existing repo the pack stays opt-in (ADR 0014).
- **Verify with a base runs every step** instead of stopping at the first failure, and `--base` works without `--affected`. Both change what M7 built, and are needed so that an old failure cannot hide a new one.
- **"Head" means the working tree**, uncommitted changes included, because that is what a stop hook judges.
- **Stack packs: one milestone each, Python first, then Next.js, Supabase, Expo** (simplest toolchain first).
- **`init` no longer needs Node.** It is the fix loop, in Rust. Only `add` and `schema` use the TypeScript generators.
- **A fix never replaces content a person wrote.** It creates, merges, appends, or rewrites a managed block.
- **Gemini CLI support is under "Later".** SPEC names Gemini, but nothing in your brief asks for it.
- **A docs review before the pull request** found milestones that were too large (the tamper guard and blame-aware verify were split further), contradictions with M7's behaviour, and missing test boxes. All were addressed.
- **Reading alint output is under "Later"**, not a numbered milestone, since you called it optional.
- **This pull request was opened on top of the M7 branch**, because M7's CI had not yet run at that point. M7 has since passed CI and merged, and this pull request targets `main`.

**Left open on purpose, each for its milestone's ADR**

- The exact `after` syntax and the loop's pass limit (M8).
- Which agent permission settings count as "safe" (M12).
- How verify gets per-test results from a runner without changing the user's command (M15).
- Which tree-sitter grammars ship, and what they cost in binary size (M17).
- Where human approvals for changed snapshots are stored, so that an agent cannot write its own (M20).
- How the base worktree gets its dependencies installed, and what that costs (M14).

## M7 — `verify` and agent hooks

**Built**

- `fscale verify` (`--affected`, `--json`): check, lint, typecheck, test, with one compact summary. Commands come from the `verify` config key, Nx, Turborepo, package scripts or Cargo.
- `fscale hooks install` for Claude Code, Codex and Cursor, and `fscale hooks run`, the stop hook with a loop guard.
- The `agent-ready` rule pack: `agents-md`, `commands-documented`, `single-test-command`, `verify-hook`.
- This repo runs its own hooks: `.claude/`, `.codex/` and `.cursor/` call `fscale verify`.
- `bench/verify-bench.mjs` and verify latency in `bench/RESULTS.md` (firescale's share: p50 8.8 ms, p99 18.0 ms).
- Docs: `docs/verify.md`, `docs/hooks.md`, and the JSON schema for `verify --json`.

**Decisions made for you** (all ADR 0014)

- Steps stop at the first failure. A step with no command is skipped and does not fail.
- The hook command is `fscale hooks run --agent <name>`, so the loop guard and each agent's reply format live in the binary.
- Hooks merge into existing agent settings and never touch user-level settings. `--bin` chooses how the hook starts fscale; for npm installs use `npx firescale`.
- The loop guard allows 3 blocks in a row per session, then lets the agent stop with a note.
- `agent-ready` is opt-in and has no deterministic fixes yet.
- `fscale explain` now says which pack to turn on when a rule's pack is off.
- Two test expectations were updated because the text they pin changed on purpose: the config error's list of allowed keys (now includes `verify`; this was the known failing test), and the help snapshot.

**Known limits**

- Cursor's hook format is unconfirmed (see above).
- Verify latency was measured on Linux only.
- No time limit on verify steps; output is captured, not streamed.
- No `hooks uninstall`. Gemini CLI has no hook support here.
- `agent-ready/verify-hook` can serve a stale cached result when the hook file is untracked and git-ignored.
- An independent review before the pull request found eight issues; all were fixed with tests (listed in ADR 0014, "Consequences"). The most important: the loop guard did not cap an agent that omits `stop_hook_active`, and quoted commands were mangled on Windows.
- A subagent started the `agent-ready` pack and stopped at a usage limit after the first rule; the other three were written in the main session.

## CI compute (after M7, at your request)

**Changed** (ADR 0016)

- A push to any branch runs Linux only, on Node 22 and 24.
- macOS and Windows run only on pull requests into `main` and when you start the workflow by hand, on Node 24.
- The benchmark job runs only when you start the workflow by hand.
- A new push cancels the run still in progress for the same branch or pull request.
- pnpm and cargo caching are unchanged.

**Known limits**

- A new pull request builds cold on macOS and Windows, because those jobs never run on `main` and so save no cache there. Start the workflow by hand on `main` now and then to seed their caches.
- Pointing a stacked pull request at `main` does not start macOS and Windows; its next push does.

**Decisions made for you**

- **The npm install job (`package`) is also manual-only now.** It runs on three OSes and ends with the launcher benchmark, so I counted it as a benchmark job. The `ci:bench` and `ci:package` labels no longer do anything. Say if you want the label back for `package`.
- **No job runs twice for one commit.** On a pull request, Linux comes from the push and macOS and Windows from the pull request, so you still see the same four checks.
- **`main` itself is tested on Linux only** after a merge. macOS and Windows ran on the pull request before it merged.
- **Cancelling also applies to `main`.** Two merges close together leave one finished run, for the later commit.
- A pull request into a branch other than `main` gets Linux only.

## M6 — npm packaging

**Built**

- The `fscale` launcher in the `firescale` package, and `scripts/pack-npm.mjs`, which builds the `firescale`, `@firescale/sdk` and per-platform tarballs.
- `scripts/smoke-npm.mjs`: installs the tarballs into an empty project with npm, pnpm or Yarn Classic and runs `fscale --version`, `check`, `explain` and a custom TypeScript rule. A CI job runs it on Linux, macOS and Windows.
- A Release workflow that builds six platforms and uploads tarballs. It does not publish.
- Launcher overhead measured in `bench/RESULTS.md`.

**Decisions made for you** (all ADR 0013)

- Eight packages at one version: `firescale`, `@firescale/sdk`, and six `@firescale/cli-<os>-<arch>` binary packages.
- **No postinstall step.** ADR 0008 proposed one to remove the Node launcher from the path. It is not built: package managers block install scripts too often for it to be reliable.
- The launcher passes its own Node and host script to the binary, so TypeScript rules use the same Node.
- `@firescale/sdk` is now publishable (it was private).

**CI was trimmed at your request** (ADR 0009 amendment): each push ran 4 check jobs instead of 12 jobs. This was trimmed again later; see "CI compute" above and ADR 0016 for what runs now.

**Known limits**

- No musl (Alpine) build. Yarn Berry with Plug'n'Play is untested.
- The arm64 targets and the two cross-compiled targets of the Release workflow have never run.
- "No Rust toolchain" is shown by installing only from tarballs into an empty project; CI runners do have Rust installed.
- `npx fscale` runs someone else's package. The command to document is `npx firescale`.

## M5 — Core rule pack

**Built**

- The seven remaining `core` rules, in Rust, each with a check, a SKILL.md and fixture tests: `gitignore-basics`, `single-lockfile`, `scripts-present`, `ci-exists`, `readme-setup`, `env-example`, `no-secrets-in-source`.
- Deterministic fixes for `gitignore-basics` and `env-example`.
- `fscale check` on this repo reports nothing with all eight rules, and a test keeps it that way.

**Decisions made for you** (all ADR 0012)

- Every heuristic prefers missing a problem to raising a false alarm.
- `single-lockfile` and `scripts-present` only look at JavaScript projects for now.
- `no-secrets-in-source` matches well-known token formats only; no entropy scoring, no generic password patterns. Findings never contain the secret.
- `ci-exists` understands GitHub Actions only and stays silent when another CI system's config is present.
- Only `gitignore-basics` and `env-example` have fixes. The others are marked for an agent or a person.
- This repo now has a `firescale.config.json`. It excludes the `env-example` rule's own test fixtures from that rule, because they contain env reads on purpose. Dogfooding found this in CI.

**Known limits**

- These rules were written and reviewed in the last half hour of the session. They have tests and passed a short review, but less scrutiny than M2 to M4. Treat the heuristics as a first version.
- No Rust or Python equivalents of the lockfile and scripts rules.
- `no-secrets-in-source` hashes every tracked file for its cache key, so the cache gives it little on large repos.
- The secrets rule misses tokens in formats it does not know.
- Raised by the review and not fixed:
  - `no-secrets-in-source` does not scan `dist/`, `build/` or `vendor/` folders, so a secret committed there is missed.
  - `env-example` scans test files too, so variables only tests use are reported.
  - `env-example` can still pick up an env read written inside a multi-line string, and misses destructuring and computed names.
  - `single-lockfile` looks at parent folders up to the repo root, which are not in its cache inputs when firescale runs inside a workspace member.
  - `gitignore-basics`' fix writes only its comment line when no language is detected.

## M4 — `fix`, SARIF and facts

**Built**

- `fscale fix` and `fscale fix --dry-run`, for built-in rules and, through the bridge, for custom TypeScript rules.
- `fscale check --sarif` (SARIF 2.1.0, validated against the official schema in tests).
- `fscale facts` and `fscale facts --json`, with a JSON schema: package manager, lockfiles, languages, tools, commands and a project graph from workspace files.
- `fscale check --affected [--base <ref>]`.

**Decisions made for you** (all ADR 0011)

- A dry run is the real fix running in record-only mode, not a separate plan.
- `fscale fix` exits 0 even when violations remain, and fixes rules that are only `evaluated`.
- SARIF levels: report-only rules are `note`; enforced rules are `error` (blocker, high), `warning` (medium) or `note` (low).
- `facts` uses small hand-written readers for `Cargo.toml` and `pnpm-workspace.yaml`, with no TOML or YAML dependency.
- `--affected` filters what is reported and does not skip rules. It has no default base ref.
- The help text was reorganised around `[options]`, which changed the help snapshot on purpose.

**Known limits**

- Nx's project graph and uv workspaces are not read. `nx.json` is only reported as a tool.
- A custom rule that writes files without using its context cannot be previewed by `--dry-run`.
- The diff in a dry run has one hunk per file, shows CRLF as LF, and falls back to "remove all, add all" for very large rewrites.
- In a dry run, a fix that reads back a file it has just written sees the old content, so its preview can differ from the real run. No built-in fix does this.
- `fscale facts` skips a workspace member whose folder does not exist without a warning, and does not handle two projects with the same name.
- `--affected` does not make `check` faster.
- The facts readers handle common `Cargo.toml` and workspace forms only; see `docs/facts.md`.

## M3 — Engine: walk, hash, cache

**Built**

- A parallel, gitignore-aware file walker, glob matching for rule inputs, and BLAKE3 content hashing, in `crates/core`.
- A rule-result cache in `.firescale/cache/`. `fscale check --no-cache` bypasses it. `FSCALE_LOG=cache` shows hits and misses.
- Built-in rules that miss the cache run in parallel.
- `bench/generate-fixture.mjs` and `bench/check-bench.mjs`, plus a CI job that runs the 50k-file benchmark on Linux, macOS and Windows.
- The four loose ends from M2: an airtight "no Node process" test (a stand-in `node` binary records whether it was started), tests for the automatic lookup of the Node host, the `core/no-committed-env` SKILL.md now covers case variants (its `explain` snapshot was updated on purpose), and per-OS benchmark numbers from CI.
- PLAN.md reordered: npm packaging is now M6.

**Decisions made for you**

- Walker, globs and hashing use the `ignore`, `globset` and `blake3` crates (ADR 0010).
- The cache key includes whether git tracks each input file, not only file contents, because the one existing rule depends on the git index (ADR 0010).
- "Rule version" in the key is the firescale version, since built-in rules ship inside the binary (ADR 0010).
- firescale writes `.firescale/.gitignore` so the cache is never committed (ADR 0010).
- Custom TypeScript rules are not cached and run one at a time through the single Node host (ADR 0010).
- A new test-only crate, `crates/testkit`, holds the stand-in `node` binary. It is not shipped.
- The help text gained a `--no-cache` line, which changed the help snapshot on purpose.

**Known limits**

- The cache costs a repo walk on every run (see decision 1).
- A rule that reads a file outside its declared `inputs` would be served stale results. Nothing checks this automatically.
- If an input file changes while a check is running, the result can be stored under the key of the old content. The next run recomputes the key and misses, so the stale entry is not served unless the file is changed back.
- The "no Node process" test puts a stand-in `node` first on `PATH`. It would not notice Node started by an absolute path.
- The benchmark CI job records numbers but has no time budget, so a slowdown does not fail it.
- Benchmarks on macOS and Windows come from shared CI runners, so they vary from run to run.
- The host lookup next to the binary searches five parent folders, so in an unusual layout it could find a `firescale` package from a different checkout.

## M2 — Rust front door

**Built:** `fscale` as a native Rust binary with `check`, `explain` and `--version`; JSON config in Rust; `core/no-committed-env` in Rust; the Node bridge for `.ts` config and custom rules; startup benchmarks.

**Decisions made for you:** napi binding removed; TypeScript CLI and TypeScript rule pack removed; custom packs listed by path; `--rules <id>` runs an off rule as report-only with a note on stderr; `serde_json` keeps key order (ADR 0008). CI runs Node 22 and 24 with actions pinned to commit SHAs (ADR 0009).

**Known limits:** nothing is installable from npm yet; the bridge runs a custom rule's `check` but not its `fix`; usage error wording differs from the old CLI.

## M1 — Rule SDK and the first rule

**Built:** the TypeScript rule SDK, config loader, `check`, `explain`, `core/no-committed-env`, the JSON schema for `check --json`.

**Decisions made for you:** config format and strict validation (ADR 0005); fixture repos built at test time (ADR 0006); the rule API (ADR 0007).

## M0 — Skeleton

**Built:** pnpm and Cargo workspaces, CI on three OSes, lint and test tooling.

**Decisions made for you:** toolchain versions (ADR 0002); planning files renamed to `SPEC.md` and `PLAN.md` (ADR 0004).

## Left for you to do by hand

- Decide the items under "Needs your decision" at the top of this file.
- Commits made by the build sessions use your name and email but are not signed, so GitHub shows them as "Unverified". If you want verified history, sign going forward; re-signing old commits means rewriting history.
- The branches `m0-skeleton`, `m1-rule-sdk`, `m2-rust-front-door`, `m3-engine-cache` and `m4-fix-sarif-facts` are merged and can be deleted.
- The manual test with a real agent (now M25) is not reachable yet.

## Publishing to npm

**0.1.0 was published by you on 2026-11-12.** `npx firescale@0.1.0 init` and `npx firescale@0.1.0 adopt --dry-run` work on Windows and Linux. Step 7 below, the generated CI passing on GitHub, is still open. The steps are kept for the next release:

**Once, before the first publish**

1. Create the npm organisation `firescale` (the `@firescale` scope) under your npm account: npmjs.com, "Add Organization". The name was free on 2026-11-11, as was the package name `firescale`.
2. Turn on two-factor authentication for publishing on your npm account.
3. Decide the version. Everything is `0.1.0`. To change it, edit `version` under `[workspace.package]` in `Cargo.toml` and the `version` in every `package.json` (a test fails if they differ), and commit.

**For each release**

1. Tag the commit and push the tag, or run the "Release" workflow by hand from the Actions tab:

   ```bash
   git tag v0.1.0
   ```

   ```bash
   git push origin v0.1.0
   ```

2. Wait for the Release workflow. It builds all six platforms and uploads the tarballs as artifacts. Download and unpack them into one folder. You should have eight `.tgz` files: six `firescale-cli-<platform>-0.1.0.tgz`, one `firescale-sdk-0.1.0.tgz`, one `firescale-0.1.0.tgz`. (Each platform's artifact also carries its own copy of the sdk and firescale tarballs. Use the ones from the `linux-x64` artifact and discard the rest.)
3. Check one before publishing anything:

   ```bash
   tar -tzf firescale-0.1.0.tgz
   ```

4. Log in. With two-factor authentication on, npm asks for a one-time code for each of the eight publishes; add `--otp <code>` to a command to pass it up front:

   ```bash
   npm login
   ```

5. Publish the platform packages first, then the SDK, then `firescale` last, so `firescale` never points at packages that are not there yet:

   ```bash
   for f in firescale-cli-*-0.1.0.tgz; do npm publish "$f" --access public; done
   ```

   In PowerShell, the same step is:

   ```powershell
   Get-ChildItem firescale-cli-*-0.1.0.tgz | ForEach-Object { npm publish $_.FullName --access public }
   ```

   ```bash
   npm publish firescale-sdk-0.1.0.tgz --access public
   ```

   ```bash
   npm publish firescale-0.1.0.tgz
   ```

6. Check from an empty folder on a machine without this repo:

   ```bash
   npx firescale@0.1.0 --version
   ```

7. Check the second box of the checkpoint: in an empty folder run `npx firescale@0.1.0 init`, commit, push to a new GitHub repo, open a pull request, and see the generated CI pass. Then tick both boxes of the checkpoint in PLAN.md.

A published version cannot be replaced, only deprecated. If a publish fails halfway, publish the missing packages; do not bump the version to retry.
