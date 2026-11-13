# Decisions log

A running log, newest milestone first. For each milestone: what was built, decisions made on your behalf (with the ADR), known limits, and what needs your decision. Anything you could reasonably disagree with is under "Needs your decision".

## CI owed

Nothing is owed. The last open item, M6's npm install test on Windows, passed on 2026-11-11 in the CI run you started by hand on `main` (run 42, commit `9190058`): all ten jobs were green, the checks on Linux, macOS and Windows, the three benchmark jobs and the three npm install jobs.

History: GitHub Actions stopped starting jobs on 2026-11-11 (the account's monthly limit), so M6 was merged on Linux results only, and M7's first two runs did not execute. Both were made up later the same day. If jobs ever end within seconds with no logs again, list here what is unproven on macOS or Windows.

Still not measured anywhere: verify latency on macOS and Windows. The `bench` job runs the 50k-file check benchmark, not `bench/verify-bench.mjs`.

## Real repos used for testing

Picked once on 2026-11-12 and cloned (shallow, read-only) into a temporary folder outside this repo. After each milestone the freshly built binary runs `facts`, `check`, `adopt --dry-run`, `doctor` and that milestone's new commands on them. Their scripts, hooks and installs are never run, and nothing is ever opened on those repositories.

| Shape                                                         | Repository                               | Commit                                     |
| ------------------------------------------------------------- | ---------------------------------------- | ------------------------------------------ |
| Python backend with a JavaScript frontend in subfolders       | `fastapi/full-stack-fastapi-template`    | `aabb90126964a164e02aff39e7c385bb0598236d` |
| A Next.js app                                                 | `vercel/commerce`                        | `f7521d57d29f0eb3d23c1399cf9bb299d61fcd21` |
| Several Python services, each with its own `requirements.txt` | `GoogleCloudPlatform/microservices-demo` | `c7cb6d5bfd6ea3aed768634ebbcb98cb7a523712` |

## Needs your decision (all milestones, most important first)

- **(0.3.0) `fixed` costs a run in a worktree of the head**, in the same run as the base, and the pairing is kept (ADR 0040). A `fixed` label now appears only after a base run that a failure needed. A step that fails in any worktree (this repo's `pnpm test` without its build) is never `fixed`; say if you would rather drop the label than pay for the run.
- **(M21) The stop hook judges only failures on the base**: a passing step never runs there in the hook, so `fixed` labels appear only in a full `fscale verify` (ADR 0039).
- **(M21) A timed-out step is not run again** before it blocks: a second timeout would double the wait.
- **(M21) A run that stops at its first failure is never `pre-existing`**: `cargo test` without `--no-fail-fast`, pytest `-x`, Jest `--bail`. This repo's own `pnpm test` runs `cargo test --workspace`, so a failing Rust test there always blocks. Say if verify should add `--no-fail-fast` itself (it does not edit commands today).

- **(M20) Vitest and Jest results come from their default reporters' text**, not their JSON reporters (ADR 0038): the JSON reporters need flags on your command, which verify does not edit. A custom reporter gives no per-test results, and the step is compared as a whole.
- **(M20) Per-test results can make a failure `new`, never `pre-existing` on their own**: the output evidence of M19 still has to hold. So an old failure in a renamed test file, whose output names the new path, blocks with a note instead of passing.

- **(M19) The base worktree links the head's dependency folders instead of installing** (ADR 0037): `node_modules`, `.venv` and `venv` wherever they are. No network, no install into a shared Python. When a lockfile or `requirements*.txt` changed, the base is not judged without `verify.baseInstall`, and failures block as before. PLAN.md said "run the repo's install command there"; say if you want an install by default.
- **(M19) Tests that need a build output the repo does not commit** (a compiled binary, `dist/`) fail on the base worktree for want of it, and a head failure there would read as `pre-existing`. Such a repo needs its build in `verify.baseInstall`; this repo sets `"baseInstall": "pnpm install --frozen-lockfile --offline && pnpm build"`. pnpm workspaces need `verify.baseInstall` anyway: pnpm refuses a linked `node_modules` for `pnpm -r` (that base result counts as setup needed, so it never makes a failure `pre-existing`). firescale cannot tell such a failure from a code failure at step level; M20's per-test labels narrow it for Vitest, Jest and pytest.
- **(M19) "Pre-existing" needs evidence:** the head's failing output must be contained in the base's, line by line with numbers folded (ADR 0037). This blocks some failures that really are old but print something run-specific (a temp path, a random seed). Blocking an old failure is the safe side.
- **(M19) On a default branch with no upstream there is no base**, so every failure blocks there, as before. Work on a branch, or pass `--base`, to get labels.
- **(M19) A run whose only failures are pre-existing exits 0** with the result `pre-existing` and the headline `verify: OK - no new failures; pre-existing (...)`. The stop hook lets an agent stop then.
- **(M19) Passing steps run on the base only when a failure needs the worktree anyway**, so `fixed` is not always found; a clean run costs no base run.
- **(M19) The base budget is 5 minutes** (`verify.baseBudget`).

- **(M18) The step cache is on by default** (ADR 0036). A second `fscale verify` with no change runs no lint, typecheck or test command; `--no-cache` runs everything. A file git ignores (a `.env`, a generated file) is not in the key, so a change there can leave a stale pass until `--no-cache`.
- **(M18) Files that belong to no project, Markdown and `docs/` included, are in every project's key.** `--affected` treats such a change as affecting nothing (ADR 0034), but a test may read them; the cache takes the safe side, at the cost of misses after a README edit.
- **(M18) The environment allowlist is a fixed list of 21 variables** (`docs/verify.md`), with no config key to add one. Say if you want `verify.cacheEnv`.
- **(M18) Limits: 30 days and 500 entries**, cleaned at the end of each run that stored something.

- **(M17) The binary is 4.3 MB larger** (3.9 to 8.2 MB, ADR 0033): tree-sitter's TypeScript and Python grammars. The tamper guard needs them too. Say if size matters enough to load grammars some other way.
- **(M17) A dynamic import also depends on every shared folder with Python in it**, not only on every other Python project, since it can load those too (ADR 0032).
- **(M17) With a root project, a file directly in the repo root folder runs everything** (it may be `ruff.toml` or `tsconfig.base.json`, read by projects below). A file in a subfolder of the root's project runs only the root's steps.
- **(M17) `fscale check --affected` keeps its old rules** (workspace graph only; a clean tree reports nothing). Only `verify --affected` uses the full graph and the "unsure means everything" rules. Say if `check` should follow.
- **(M17) An affected run's headline is `verify: OK (affected only) - 2 of 6 projects ran, 4 not affected`**; the JSON `result` stays `ok` and the new `scope` is `affected`. The repo root's own steps count as one project.

- **(M16) The Python starter has no type checker,** so `fscale verify` there is `PARTIAL` (typecheck skipped). Say if the starter should add mypy to `requirements-dev.txt`.
- **(M16) The Python pack never edits a requirements file a person wrote.** A project with tests whose development list lacks pytest gets an agent finding rather than an appended line.
- **(M15) The `nextjs` pack pins next, react and react-dom exactly** (16.4.6, 19.3.1), as create-next-app does, and uses ranges for the rest (`typescript ^5`, `eslint ^9`). Say if you prefer ranges everywhere. A new app is named `next-app` (never the folder's name, so dry runs match).
- **(M15) A new Next.js app still gets a `core/scripts-present` finding for a `test` script**, because Next.js ships no test runner. Say if the pack should add one (Vitest, for example).
- **(M14) The generators live in `packages/cli/src/generators/`**, inside the `firescale` npm package, not in `packages/generators` as SPEC §9 lays out; that package is a placeholder pointing there. A separate package would need its own publish step (ADR 0028).
- **(M14) Default folders:** `fscale add package` writes to `packages/<name>`, `fscale add rule` to `firescale-rules/` (the docs' `tools/rules` would give `tools/rules/rules/<rule>`).
- **(M13) A repo with CLAUDE.md and no AGENTS.md:** `adopt` (with `agent-ready` on) and `sync` create AGENTS.md and append a three-line managed block with `@AGENTS.md` to the end of CLAUDE.md, because Claude Code reads only CLAUDE.md when it exists. Nothing else in CLAUDE.md changes, and nested CLAUDE.md files are untouched. The alternative is to leave CLAUDE.md alone and print a note, at the cost of Claude Code never seeing the commands (ADR 0027).
- **(M13) Codex's permission profile was not run.** `.codex/config.toml` (`default_permissions = "firescale"`, extending `:workspace`, denying env files and keys) follows Codex's source and tests at `rust-v0.167.2`; developers.openai.com was blocked. Open the repo once in Codex (as a trusted project) and check it starts. A Codex config you wrote is never edited.
- **(M13) Cursor's `.cursor/cli.json` permissions are unconfirmed** (cursor.com blocked). Cursor's hooks and skills formats were confirmed from Cursor's own GitHub examples. Allow `cursor.com` in the environment's network settings so a session can check the docs page.
- **(M13) Which env files agents may not read:** `.env`, `.env.local`, `.env.*.local`, `.env.development`, `.env.production`, `.env.staging`, `.env.test`, plus `*.pem`, `*.key` and `secrets/`. `.env.*` as a whole is not denied, because that would also block `.env.example`, which agents keep up to date. Other names (`.env.qa`) are not covered.
- **(M13) A new repo gets 45 skill files** (14 rules and the `fscale add` skill, for 3 agents). Say if you would rather install skills for one agent only, or only for the rules that are enforced.

- **(M12) A running step is stopped early by default when a service it needs is down** (`verify.stopWhenServiceDown`, default `true`, ADR 0025). It needs two probes 10 seconds apart to find nothing listening, plus either a refused-connection message for a known service or the repo's own port (then the step is setup needed), or at least 3 errors and nothing else from pytest or unittest (then the step is failed, with the service named as the likely cause). A wrong stop takes a suite that connects to such a port on purpose and would still pass. Say if the default should be `false` (print the line, let the step run on), or if the thresholds (3 errors, 10 seconds, a probe every 5 seconds) should differ.
- **(M12) An unreachable service is a problem in `fscale doctor`** (exit 1), not a warning: tests that need it cannot pass. If you often run doctor without wanting the database up, a warning would suit better (ADR 0026).
- **(M12) Doctor's `packageManager` rule:** a different major version is a problem, the same major version with another minor or patch is a warning. Say if any difference should be a problem.
- **(M12) Doctor reads only `.env` and `.env.local` beside each `.env.example`** for names (never values). `.env.development`, `.env.test` and the like are not counted.
- **(M12) A nameless root `package.json` (no `"name"`) is now the root's Node project**, as on `vercel/commerce`, unless it lists workspaces. Its name in facts is `.`. This adds it to `fscale facts` and to the checks that read projects.

- **(0.1.3) The default time limit is 10 minutes a step.** Your 13-minute pytest run would now stop at 10 minutes as `TIMED OUT`. A suite that is slow on purpose needs `"test": { "timeout": "30m" }` (or per project). Say if the default should be longer, or off (ADR 0024).
- **(0.1.3) A refused connection to a local service is setup needed even when tests also failed.** A test whose fixture starts the app on, say, port 3000, and the app crashes on a code bug, would read as "start the service". Limiting that exception to the known database ports would avoid it.
- **(0.1.3) Progress is off when stderr is not a terminal.** CI logs therefore show nothing until the end, as before. `--progress` in the generated workflow would change that; I left the workflow as it is.
- **(0.1.3) `CI=1` is on by default** and stdin is always empty, with no setting to give a step input. `verify.ci: false` turns the variable off.
- **(0.1.3) Verify's JSON is schema version 4** (results `stopped`, statuses `timed-out` and `stopped`). Blame-aware verify (M17) will take 5.

- **(M11) The default branch patterns and the guide's text.** Read them once: `fscale init --packs core,group-project` in an empty folder writes both. Say if the defaults should differ for your course (for example `<name>/<topic>` branches).
- **(M11) Should `group-project` be on in `fscale init` by default?** Today `init` turns on `core` and `agent-ready`, as SPEC §16 says.
- **(0.1.2) Should the stop hook let an agent stop when the only problem is setup?** Today it blocks, with a message that says not to edit code and to run the setup command or tell the user; the loop guard releases the agent after 3 blocks. Letting it stop at once would save those turns, but the agent can often run the install command itself (ADR 0022).
- **(0.1.2) The order of Python interpreters.** A `.venv` or `venv` in the project, then uv, poetry or pipenv, then an active `VIRTUAL_ENV`, then `python` from the PATH. Say if an activated venv should win over the project's manager (ADR 0022).
- **(0.1.2) With no virtual environment, the install command is `python -m pip install -r requirements-dev.txt`**, into the same `python` the steps use, not into a new `.venv`. A `.venv` path differs by OS; the `note:` line suggests creating one. On Debian and Ubuntu that install is refused by the system Python, which is the right outcome.
- **(0.1.2) Verify's JSON is schema version 3.** It added the status `setup-needed`. Blame-aware verify (M17) will take version 4.
- **(0.1.2) A failing `check` still stops every project.** Only the root and the projects are independent of each other. Say if `check` should not stop the rest.
- **(0.1.2) Ruff or mypy in a requirements file above a service does not count for it.** Only the service's own folder is read (or what its files include with `-r`). Ruff in `[project] dependencies` or `[packages]` (not only dev lists) also counts.
- **(Releases) No approval step between the tag and the publish.** Pushing the tag is the approval. A GitHub environment with a required reviewer can be added; it must then be entered in each package's trusted publisher too.
- **(Releases) After the first automated release works, turn off token publishing** for each package on npmjs.com ("Require two-factor authentication and disallow tokens"). npm recommends it. I left it as a step in `RELEASING.md`, since it is your account.
- **(M10) Should the agent stop hook block when verify is only partial?** Today it does not. With steps skipped for lack of a command, `fscale verify` prints `verify: PARTIAL`, exits 0, and the hook lets the agent stop with a one-line note on stderr. To make it block, set `"verify": { "failOnSkipped": true }`; then verify exits 1 and the hook blocks. I kept "do not block" as the default because a missing command is not something the agent's change broke, because the easy way past such a block is to set the steps to `false` (the weakening the tamper guard is meant to catch), and because the hook lets go after 3 blocks anyway. The cost: in a repo with nothing to run, an agent can stop with no check having run. Say if you want the hook to block by default, or want `failOnSkipped` on by default (ADR 0020).
- **(M10) A partial verify exits 0.** That follows your earlier answer that a skipped step is not a failure. CI therefore stays green on `PARTIAL` unless `failOnSkipped` is on. The starter workflow that `fscale init` writes does not turn it on. Say if generated CI should.
- **(M10) Projects are searched four folder levels deep, and the search has no setting.** Deeper projects are reached by naming their folder under `verify.projects`. Say if you want a config key for the depth or for extra folders to skip.
- **(M10) A `requirements.txt` alone is not a project.** It needs a `.py` file or a Python config file beside it. Your services qualify through `pytest.ini`, `alembic.ini` and `main.py`.
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

## 0.3.0 — two fixes to the base run, after M21

**Built** (ADR 0040, `docs/verify.md` "Only new failures block"):

1. **A changed lockfile counts per project.** A Node or Python lockfile or a requirements list (`requirements*.txt`, `constraints*.txt`, a `.txt` in `requirements/`) that a step's cache key covers and that differs between the base and the head means the linked `node_modules` or virtual environment is wrong for that step's base. With `verify.baseInstall` set, it runs as before; without it, that step has no base: its failure blocks, with a note naming the file and `verify.baseInstall`. Other projects are still judged; a root lockfile counts for every project. `Cargo.lock` and `go.sum` no longer take the base away (nothing is linked for them). The base store's key now holds the head's requirements lists too.
2. **`fixed` needs a real failure on the base.** Before a passing step is labelled `fixed`, it runs once in a worktree of the head (`HEAD` plus the working tree's changes, ignored files left out, dependencies set up as for the base), in the same run as the base. Only a pass there makes it `fixed`; otherwise no label and a note. A per-test base on which no test passed is never evidence. Only the pairing of the two is kept; a kept base failure without it is not used for a passing step. The stop hook no longer looks up base results for passing steps.
3. **`git diff` never writes your index.** It refreshes `.git/index` when a file's stat data is stale, `GIT_OPTIONAL_LOCKS=0` or not; the test "leaves the index untouched even when a file's time changed" caught it on macOS now and then. Every `git diff` firescale runs now uses a private copy of the index (`GIT_INDEX_FILE`). Proof: `git::tests::git_diff_never_writes_the_index_even_when_a_file_s_time_changed`, which sets a file's time 30 s ahead so it fails every time without the fix.
4. The version is 0.3.0 (`node scripts/bump-version.mjs 0.3.0`). No tag pushed.

**Tests changed on purpose, none weakened:** `runs::tests::a_run_that_is_still_going_is_named_and_not_waited_for` accepts `2m 06s` as well as `2m 05s`: the other run started 125 s before, and a slow Windows runner crossed one more second before the note was written.

**Decided on your behalf:** `verify.baseInstall`, when set, still runs on every base run (not only when a lockfile changed): mixing linked and installed folders in one worktree would let an install write through a link into the head (ADR 0037). The check counts any changed dependency list, as the root's steps do.

**Independent review before the pull request** reproduced three problems in the first version, all fixed with tests: a kept base failure from before an install finished was paired with a fresh run of the head and read as `fixed`; the copy into the head's worktree followed a link the commit holds out of the worktree; a clean run could make a worktree. Also fixed: `Cargo.lock` and `go.sum` took the base away, and the copy ignored Ctrl+C and the budget.

**Proof** (`blame.test.ts`, `blame::tests::the_head_overlay_*`): with two projects whose tests fail on the base, a changed lockfile in `a` leaves `a`'s failure unlabelled and blocking with the note while `b` is `pre-existing` and only `b` runs on the base; with `baseInstall`, both are judged; a changed `requirements.txt` in one project or a root lockfile behaves the same way, and makes a kept base result unused; a test that needs an ignored build output fails on the base and in the head's worktree, so it gets no `fixed` label and a note; a kept failure from before an install finished is never paired with a later run; a real repair is `fixed`, and the pairing is reused; the copy applies changes and deletions, leaves `.firescale/` out and never writes through a link. The three tests aimed at the fixes fail on the code before them. `facts::tests::dependency_lists_are_lockfiles_and_requirements_lists`.

## M21 — Blame-aware verify: reruns and the stop hook

**Built** (ADR 0039, `docs/verify.md` "The stop hook"):

1. A `new` failure runs once more before it blocks, unless files it reads changed while verify ran (its first run may have written them); a pass the second time is `flaky` and does not block (with per-test results: every new test passed, and what still fails is the base's own failure by the output evidence). Nothing about it is written: no history, no cache entry.
2. The stop hook runs only the projects the change reaches, measured from the base commit (so an agent's commits on its branch count), through M17's graph, shared-source nodes included; it runs the base only for failures, within the budget. An agent may stop when every failure is `pre-existing` or `flaky`.
3. `cargo test` per-test labels from its text output: `<crate>::<test path>`.
4. Latency in `bench/RESULTS.md` (`bench/blame-bench.mjs`).

**Independent review before the pull request** reproduced three ways an agent could stop with broken work, all fixed with tests: a new `check` violation was run again without `--base` and read as `flaky` (the check is never run again now, and its rerun command names the base); with no base the hook ran only uncommitted changes, so committed work on a local `main` was never judged (it runs everything then); a cut-short second run counted as evidence that new tests passed (now: a plain failure with more passes and only old failures). Also fixed: a first run that writes its own inputs (no rerun then), cargo's crate lines on stderr and doc tests' line numbers, fail-fast runs (`cargo test` without `--no-fail-fast`, `-x`, `--bail`: never `pre-existing`), the rerun's time budget, and the hook's note on flaky steps.

**Known limit, seen on this repo:** its own `pnpm test` also fails when run in a base worktree (after `verify.baseInstall`), for reasons of the worktree, not of the code. So on this repo a passing test step can read `[fixed]`, and a failing one blocks with a note (the output evidence never matches): safe, but the labels say little here.

**Proof** (`stop-hook-blame.test.ts`, `pertest::tests`): a test that fails on its first run only is `flaky` and does not block; nothing about it is written under `.firescale/`, and its second pass is not cached; a failure that fails again stays `new`; the stop hook lets an agent stop on a pre-existing failure and blocks on a new one; on the course project a change in one service runs that service's steps only, and only its failing step runs on the base; the `cargo test` reader.

**Tests changed on purpose:** `runners_are_found_in_commands_and_scripts` expected `cargo test` to have no reader; M21 adds one, so it now checks `cargo build`.

## M20 — Blame-aware verify: per-test labels

**Built** (ADR 0038, `docs/verify.md`):

1. Per-test results for pytest (a JUnit report asked for through `PYTEST_ADDOPTS`, read with a small reader of `<testcase>`; the command is unchanged), Vitest and Jest (their default reporters' failure lines). The runner is read from the command, or from the package script behind `npm test` and the like.
2. A test is its file and name: `tests/test_auth.py::TestLogin::test_ok`, `src/a.test.ts > math > adds`, `src/a.test.js › math › adds`.
3. A failing test that does not fail on the base makes the step `new`; the summary names it under `New failing tests:` and says how many others already failed there. `pre-existing` still needs M19's output evidence. A base with no passed test is not evidence. JSON: a step's `failedTests` with labels.
4. Renamed test files (git's rename detection) keep their tests: an old failure there is never `new`; it blocks with a note, since its output differs.
5. Other runners are compared as whole steps, with a note. The base store (format 4) keeps the failing tests.
6. The report option is quoted for pytest's shell-like splitting (forward slashes, single quotes), uses `junit_family=xunit1` for the file of each test, and is not added when you ask for a JUnit report yourself.

**Independent review before the pull request** found that the unquoted report path made pytest exit 4 on the base and the head alike when the temp folder has a space (and broke it on Windows), so a new failure read as `pre-existing`: now quoted, tested end to end with a temp folder with a space, and a pytest that refuses its options is never evidence. Also fixed: pytest's `generated xml file:` line in signatures, rename ids relative to the repo root, collection errors and capitalised packages in ids, Jest's `● Console` and display names, Vitest lines a test printed and names with `[`, a user's own `--junitxml`, report files shared between `--jobs` units, and a note printed for lint steps.

**Proof** (`blame.test.ts` "per-test labels", `pertest::tests`): with one old and one new failing test, only the new one is `new` and named, for pytest and for Vitest; a runner without per-test results falls back with a note; a renamed test file's old failure is never `new`; a base with no passed test is not trusted; the JUnit, Vitest and Jest readers, runner detection and rename mapping.

## M19 — Blame-aware verify: the base run

**Built** (ADR 0037, `docs/verify.md` "Only new failures block", verify schema 6):

1. **The base**: the merge base of `HEAD` with `--base <ref>` or the default branch (`origin/HEAD`, `origin/main`, `origin/master`, `main`, `master`). None outside git, before the first commit, with no default branch, or on the base with a clean tree: then every failure blocks as before. `--base` works without `--affected`.
2. **With a base, every step runs**: a failure does not stop its project's other steps, and a failed check stops nothing.
3. **Base results, cheapest first**: the same inputs (no file in the step's M18 cache scope differs between base and working tree, so the head result is the base result); the base store `.firescale/base/` (key: the step's cache key with the base commit's blob ids, the head's lockfiles and what the worktree gets; passed and failed only; 30 days); a run in a temporary `git worktree` of the base commit, failing steps first, within `verify.baseBudget` (default 5 min). The working tree and index are never touched.
4. **The worktree's dependencies** are the head's `node_modules`, `.venv` and `venv`, linked (symlink, or a junction on Windows), removed before the worktree. A changed lockfile or requirements list means the base is not judged without `verify.baseInstall`. Base steps get `FSCALE_BASE_RUN=1`.
5. **Labels** `new`, `pre-existing`, `fixed`; only `new` and unlabelled failures block. A step that needs setup on the head is never labelled. A base result that needed setup, timed out, ran over the budget or was stopped gives no label, and a note says the failure blocks as without a base.
6. **The check** runs on the base too when it fails on the head; enforced violations are compared by rule, file and message, and marked `[new]` or `[pre-existing]`.
7. **Output**: the result `pre-existing` (exit 0, `verify: OK - no new failures; pre-existing (...)`), `(new)` in failing headlines, `[label]` on rows, a line per new and pre-existing failure, what was fixed, and how the base results were found. JSON: a step's `label` and `base`, the report's `base`.
8. With `verify.baseInstall`, nothing is linked. pnpm's refusal of a linked `node_modules` (`ERR_PNPM_UNSAFE_TASK_RUN_STATE_PATH`) on the base is setup needed. This repo's config sets `verify.baseInstall` to `pnpm install --frozen-lockfile --offline && pnpm build`, since its scripts use `pnpm -r` and its Vitest suite runs the built binary.

**Proof** (`packages/cli/src/blame.test.ts`, `blame::tests`): a test already failing on the base passes with `pre-existing`; a failure introduced on the head, uncommitted and committed on a branch, is `new` and exits 1, and one repaired is `fixed`; a pre-existing lint failure does not hide a new test failure; a second run makes no base run, and a lockfile change runs it again (with `baseInstall`) or blocks without judging (without); the working tree, the index and the worktree list are byte-for-byte unchanged; on a clean default branch a failing test still exits 1; a base run over its budget falls back to blocking with a note; a step that needs setup on the head is never labelled; on the course project only the changed service runs on the base, the others have the same inputs.

**Found on real repos** (copies with every step replaced by a node one-liner, `test` failing everywhere, one project touched): the labels are right; on `microservices-demo` 18 of 21 steps had the same inputs and only the touched service and the root's steps ran on the base, in 0.2 s; the tree, the index and the worktree list were unchanged.

**Independent review before the pull request** reproduced eight ways a failure the change caused stopped blocking; all fixed with tests: one old failure hid a new one in the same step (now the head's output must be within the base's); a `node_modules` with workspace links made the base run the head's library (such folders and editable venvs are never linked); a missing build output or a nested root read as `pre-existing` (the output comparison catches it); a project new on the head errored the run and the stop hook let the agent go (now "not judged", and the run never errors); the same-inputs shortcut skipped the cache's refusals (now it needs the full key); a commit on a local `main` became the base (now the upstream, or no base); firescale's own `.firescale/` made a clean tree dirty (excluded, and it ignores itself); `baseInstall` wrote through links into the head (nothing is linked with it). Also fixed: `git status` rewrote `.git/index` (`GIT_OPTIONAL_LOCKS=0`), leaked worktrees are swept, Ctrl+C during the base run is `stopped`, worktrees are added with hooks and LFS off, the check counts repeated violations.

**Test changed on purpose, none weakened:** `bridge.test.ts` "runs init, adopt and sync with built-in packs without starting node" has 60 s instead of 30 s: its six runs took 25 s on the Windows runner for M18 and 30 s for M19 (as M17 did for a scaffold test).

**Found on this repo**, and fixed before the pull request: `typecheck` (`pnpm -r run typecheck`) failed on the base, first for want of `packages/cli/node_modules`, then because pnpm refuses a linked `node_modules` for recursive runs; and that failure was kept. Now every ignored `node_modules`, `.venv` and `venv` is linked, pnpm's refusal is setup needed (never a code failure), `verify.baseInstall` replaces linking, and the kept result's key holds what the worktree got.

## M18 — The local step cache

**Built** (ADR 0036, `docs/verify.md` "The step cache"):

1. A lint, typecheck or test step that **passed** is stored in `.firescale/steps/<key>.json` and served from there while its key is unchanged. Failed, setup needed, timed out, stopped and skipped steps are never stored.
2. The key: the cache format, the firescale version, the step, folder and command; the bytes of every file the step can read as they are in the working tree (`git ls-files --cached --others --exclude-standard`, so tracked files with uncommitted changes and untracked files git does not ignore; a CRLF checkout is other bytes): its project's folder, the projects and shared nodes it depends on in the M17 graph, the files directly in each folder above them, and every file no project owns; the program the command starts (by path, size and time) and the versions of known tools by exact name (never a repo script, never through a shell); the package manager's marker in `node_modules` and the `pyvenv.cfg` and packages of `.venv`; OS and CPU; and 21 allowlisted variables, hashed. Each step is keyed on the tree as it is when it starts.
3. Never cached: a step that may need a service the repo does not start (an `.env.example` it covers names a service no Compose file starts), a project with a dynamic import or a tsconfig `paths` (or one it depends on), a command that reads git history (`nx affected`, `turbo --affected`), a root inside a larger git repo, an input that could not be read (a link to a folder, a submodule, a nested repo, a program not found).
4. A pass is stored only if its files are the same at the end of the run, so a step that writes its own inputs is never stored. Entries older than 30 days and the least recently used beyond 500 are removed at the end of each run.
5. A hit reads `cached` in the summary with the time it took and when it passed; the headline says `N of M steps passed from the cache, not run now`, and a line says what the cache saved. In JSON the step stays `passed` with `durationMs: 0` and a `cache` object; the report has `cache: {read, hits, savedMs}` (schema 5, new optional fields).
6. `--no-cache` runs every step and still stores passes. `FSCALE_LOG=cache` prints hit, miss, not cached (with the reason), stored and not stored.

**Proof** (`packages/cli/src/step-cache.test.ts`; `stepcache::tests` in Rust): a second run runs nothing and says so; a tracked, an untracked and a deleted file, and CRLF bytes, run again, an ignored file does not; on the course project a change in one service runs that service only, a change in `backend/shared` the three services that import it, the frontend lockfile the frontend, a new tool version and an allowlisted variable everything, another variable nothing; a failed step is never stored; a service from `.env.example` is never cached, one Compose starts is; no value outside the allowlist reaches the key or the folder; a step that writes its own input is not stored; a dynamic import and an unreadable file are not cached; cleaning by age and count. Timings in `bench/RESULTS.md`.

**Independent review before the pull request** found ten ways to a stale pass, all fixed with regression tests (`stepcache::tests`, `step-cache.test.ts`), and four smaller points, also fixed:

- a step was keyed on the tree as it was before any step ran, so a step whose files an earlier step rewrote (`eslint --fix`, codegen, a `--jobs` peer) was served the old pass: each step is now keyed on the tree when it starts ("runs a step again when an earlier step changed its files");
- a submodule or nested repo, and a link (by target only), hid changes; a root inside a larger git repo left the files above out; a root step did not see the services of the projects below it; tsconfig `paths` aliases and `nx affected` commands depend on what the key cannot hold: none of these is cached now, or a link counts by what it points to;
- an unknown program was not keyed at all (now by path, size and time), and the version memo was not per folder;
- the version probe matched `python*` loosely and ran through the shell in the project's folder, so `scripts/python-tests.sh` or a `node.cmd` in the repo could run: probes now take exact names only, run the program found on the PATH by full path with no shell, and Yarn is never probed;
- also: only "not found" counts as a deleted file, hashes are not reused for files changed within 2 s of a snapshot, the executable bit counts, an entry past 30 days is never served, and the STOPPED headline and the hook's partial note no longer count cached steps as run.

**Tests changed on purpose, none weakened:** the stand-ins for `python`, `npx` and `npm` log every call, and the cache now asks tools for their versions to key a step. Three helpers that compare the list of commands a run made now leave out `--version` probes (`stepCalls` in `test-helpers.ts`); what they assert about the steps is unchanged. The progress test's second run passes `--no-cache`, because the first run stored the pass of the step whose progress line it checks. Two JSON snapshots gained the `cache` object.

**Found on real repos** (copies with every step replaced by `node -e 0`, so none of their scripts ran): all three key, store and hit; after a new file in one `microservices-demo` service, only that service and the repo root's own steps ran again. A folder named only in `verify.projects` (a workspace member of `fastapi/full-stack-fastapi-template`, named so by the test config) is keyed on the whole repo, as designed. The usual `facts`, `check`, `adopt --dry-run`, `doctor`, `graph` and `verify --dry-run` runs gave the same results as after M17.

## 0.2.1 — setup problems in test steps, from running 0.2.0 on the course project

**Found by the owner** (ADR 0035):

- With Redis down, three pytest projects were stopped and Redis was named as the cause. With Redis up, the same three still errored on every test and ran to the 10-minute limit. Their real cause was a refused connection to the test database on `localhost:5433`; that port appears only in the tests' own settings (`TEST_DATABASE_URL` in `.env`).
- A fourth project with 96 passes and 15 "Cannot reach the test Redis" errors was labelled FAILED.
- After a stop or a timeout the summary showed only rows of `E`.

**Built**

1. A service that is down is named as the cause only when the step's output names it: its address, its name or its client library. Otherwise it is listed as "also not running".
2. `verify.stopAfterErrors` (default 10, `false` turns it off) stops a pytest or unittest step that has reported that many errors with no pass and no failure, as likely setup.
3. After a pytest step is stopped early or times out, one more run of the same command with `--maxfail=1` (limit 1 minute) gets the first error. Its text is printed under `First error of ...` and returned in JSON as `firstError`. If it shows a refused local connection or a failed login to a local database, the step is setup needed, naming the host and port.
4. A finished pytest step whose errors (and no failures) all name a local service that is not running is setup needed, also when other tests passed.
5. A timeout whose counts show mostly errors says the tests are erroring, not slow, and does not suggest a longer limit. The counts are in JSON as `tests`.
6. "password authentication failed" (libpq, psycopg, asyncpg, Node `pg`), MySQL "Access denied ... (using password", and Prisma P1000, for a local database, are setup needed. The user name is never shown.
7. Doctor checks local database, Redis and queue URLs found in `.env` and `.env.local` (localhost, 127.0.0.1, ::1). It keeps only the variable name, host and port, and its Debug-text and output tests prove that. `docs/doctor.md` says "reachable" means the port answers, not that it is the right database.

**Proof** (stand-in pytest, `packages/cli/src/verify-run.test.ts`, "setup problems seen in a test step (ADR 0035)"):

- all errors with a down service the output never mentions: not named as the cause;
- all errors with every known service up: stopped early as likely setup, with the first error's text;
- a refused local connection in the first error: setup needed, with its host and port;
- a failed password login in the first error: setup needed, with no user name shown;
- passes plus errors that all name an unreachable service: setup needed;
- assertion failures, and errors that name no service: still FAILED with "Fix the failure";
- a slow passing suite: never stopped;
- a timeout of errors: no "raise the limit".

Doctor: `doctor.test.ts` and `services.rs` tests. Rust unit tests for the first-error reader, the evidence check, the summary rule and the config key.

**Independent review before the pull request** found seven issues, all fixed with tests (ADR 0035, "After the review"). Three could call a code failure setup needed, which tells an agent not to fix its code:

- a test that quotes "password authentication failed" beside a real assertion failure;
- an error in `test_redistribute_credits` (matched as "redis"), or `relation "orders" does not exist` while the database on 5433 was up (matched as PostgreSQL on 5432);
- your run 1, which still named Redis because a progress line named a Redis test file.

The others:

- `--maxfail=1` was appended to piped or chained commands;
- a passing suite that timed out got an extra run;
- the last lines of a failed step were not cleaned of secrets;
- port matches were prefixes.

**Needs your decision**

- **Thresholds.** The default of 10 errors, and the 1-minute limit of the extra run (it has no config key).
- **No fix command for a failed login.** It has no command that fixes it, so the stop hook names none, and an agent will ask you to fix the test settings.

## M17 follow-up — files that affect no project, and the cross-compiled builds

**Built** (ADR 0034): a changed file that no project owns and that cannot change a build or a test affects no project with `verify --affected`: Markdown, text files (not requirement or constraint lists, not `CMakeLists.txt`), a licence, and anything under the root `docs/`. `check` still runs, and a note names the files. Every other unowned file still runs everything. Tests: `a_root_readme_change_affects_no_project_but_an_unowned_root_script_affects_all` and "runs no project for a root README change, but everything for an unowned root script" in `graph.test.ts`.

**Proven: the cross-compiled release builds compile tree-sitter.** The Release workflow, started by hand on the M17 branch (run 43376342929, 2026-11-13, commit `73d44c2`), built all six platforms with nothing published: `darwin-x64` (cross-compiled on an Arm runner) and `win32-arm64` (cross-compiled on an x64 runner) passed, as did the four native builds and their npm smoke tests. Nothing to fix; no tag was needed.

## M17 — Project graph and affected verify

**Plan update first** (ADR 0031): affected-only verify (M17) and a local step-result cache (M18) now come before blame-aware verify (M19 to M21), whose base run reuses the cache; faster generated CI with a shared cache that only the default branch writes is M22 (CVE-2025-36852); the old M20 to M27 are M23 to M30; test-level selection is under "Later". PLAN.md, SPEC §8 and HANDOFF.md use the new numbers.

**Built** (ADR 0032, ADR 0033, `docs/graph.md`)

1. **The project graph**, built on demand: projects, **shared nodes** (a folder or file projects use that no project owns, such as `backend/shared`; a change there affects only its users), and edges with their evidence. Sources, most trusted first: requirements files, `pyproject.toml` path sources, `package.json` `file:`/`link:`/`portal:`/`workspace:`, Cargo `path` and dependencies by name; Dockerfile `COPY`, Compose `build.context` and `depends_on`, shared inputs (`.env.example`, OpenAPI specs, Compose files); Python and JS/TS imports read with tree-sitter; dynamic Python imports (a warning, and an edge to every other Python project and Python shared folder).
2. **`fscale graph [--explain] [--json]`**, with `graph-output.schema.json`. `fscale facts` shows the same edges (`Graph` section, `graph` in JSON; facts schema still 2).
3. **`fscale verify --affected [--base <ref>]`** runs only the projects a change reaches; the others are `not affected` (a new status, apart from skipped). The headline counts projects and never reads as a plain OK. A changed file no project owns, a root lockfile, a CI file, a Compose file or the firescale config runs everything; so does a clean tree with no `--base`, with a note. Verify schema 5 (`scope`, `affected`, `not-affected`).
4. **`fscale verify --dry-run`**: the plan, with `--affected` the reason for each project, and nothing run.
5. The course fixture's services `auth`, `ledger` and `market` import `backend/shared`, in both copies of the fixture.

**Proof** (each a test): `graph::tests` (a change in `backend/shared` affects the three services that import it; one service's change only that service; a frontend change only the frontend; the root Compose file everything; a fixture per edge source with its `--explain` text; the dynamic import's warning and wide edges); `verify::tests::affected_runs` (the same four cases through the verify plan, the headline, `not-affected`, the dry run); `packages/cli/src/graph.test.ts` (the same through the binary, in git, with the stand-in tools showing that only the affected projects' commands ran; no base and a clean tree runs everything with the note; `--base HEAD` with no change runs nothing; schema validation).

**Found on real repos** (release build; `facts`, `graph --explain`, `verify --affected --dry-run`, with one file touched in each clone and restored; nothing run):

- _Fixed:_ `fastapi/full-stack-fastapi-template`'s backend Dockerfile copies the root `package.json` and `bun.lock`. A change to such a root file affected only the backend, not the repo root's own Node project, whose file it is. The root's project now always holds files no other project holds (`a_root_file_that_a_project_copies_affects_that_project_and_the_root_project`).
- _Seen, correct:_ the same repo gets `frontend -> backend` from Compose (`playwright` depends_on `backend`), so a backend change also runs the root's steps (which cover the frontend workspace member). `microservices-demo`: ten projects, no edges (each service is built from its own folder and imports only its own generated code); touching `src/emailservice` runs 1 of 10. `vercel/commerce`: one project at the root.
- _Left:_ `microservices-demo` copies `protos/` into services with a script, not Docker, so there is no edge; `protos/` belongs to no project, so a change there runs everything, which is the safe answer.

**Independent review before the pull request** found five ways a change could run too little, all fixed with regression tests: the root's steps did not run for a change in a Cargo workspace member when the root has a `package.json` (this repo's own layout); a root project's Python imports gave no edge; with a root project a shared folder's own imports were lost; a shared node at `.` (a Docker context of the root) or at a folder of projects hid "a file nobody owns runs everything"; and a shared folder holding only subpackages was not seen as Python. Also fixed: `verify --dry-run` ran a `firescale.config.ts` through Node (it now refuses one), a root project's top-level files now affect everything, a Python file inside a Node project is read, dynamic imports reach shared `.py` files, and all-off projects are not counted. Left: paths are compared as written, so a `COPY` written in another letter case is missed on Windows and macOS.

**Tests changed on purpose, none weakened** (after the first CI run of the pull request): the scaffold test "`--check` exits 1 after a generated file is deleted" has 60 s instead of 30 s (it took 25 s on Windows before M17 and timed out after it); the Linux daemon test's command now waits until the daemon is up before the step ends, because verify could stop the step's process group before `setsid` had taken the daemon out of it (a race in the test, seen under load). Both assert what they asserted before.

**Known limits** (`docs/graph.md`): TypeScript path aliases, run-time `sys.path`, uv workspace sources, Go `replace` and Compose volumes are not read; files over 1 MiB are skipped.

## M16 — Stack pack: Python

**Built** (ADR 0030): the `python` pack, opt-in, with six rules that work per Python project: `requirements`, `test-tools`, `tests`, `pytest-config`, `ruff-config`, `gitignore`. Each has a SKILL.md and fixture tests; pack tests cover an empty folder, an app at the root, and two services of the course project's shape. **A repo of services, each with `requirements.txt`, `requirements-dev.txt`, `pytest.ini` and its own `.venv` and no `pyproject.toml`, passes as it is; a gap in one service is fixed in that service's folder; nothing creates or suggests a root `pyproject.toml`.**

**Proof by hand** (2026-11-12, Linux, Python 3.13.18, pip 26.2.2): in a folder made by `fscale init --packs core,python`: `python3 -m venv .venv`, `.venv/bin/python -m pip install -r requirements-dev.txt`, `.venv/bin/python -m pytest` (1 passed), `.venv/bin/python -m ruff check .` (All checks passed!), all exit 0. `fscale verify` there: check, lint (`.venv/bin/python -m ruff check .`) and test (`.venv/bin/python -m pytest`) passed; typecheck skipped (no mypy), so `PARTIAL`.

**Found on real repos** (copies with the pack on, `adopt --dry-run`):

- _Fixed:_ `GoogleCloudPlatform/microservices-demo`'s Python services have code and no tests; the pack offered each a `pytest.ini`, a `requirements-dev.txt` and a `ruff.toml`. Those now wait until a project has tests; only the tests finding (for an agent) and `.gitignore` lines remain (`a_service_with_no_tests_is_only_told_to_add_tests`).
- _Fixed:_ a pytest config in a folder above a project now counts, as pytest itself looks upward (`a_pytest_config_in_a_folder_above_counts`).
- _Left (correct):_ `fastapi/full-stack-fastapi-template`'s backend has tests and no pytest config anywhere, so the pack proposes `backend/pytest.ini`. It is a uv workspace member; the new file does not change its `pyproject.toml`.

**Independent review before the pull request** found five real bugs, all fixed with regression tests (`python::pack_tests::review`):

- **Your requirement, broken in one case:** services with requirements files, `app/` and `tests/` but no `pytest.ini` and no `.py` file directly in their folder are not projects to `fscale facts`, so the pack saw no Python project and proposed a starter at the root. The pack now also treats every folder with a requirements file or `pyproject.toml` as a project, and builds a root starter only when the repo has no Python code at all (`services_that_facts_cannot_see_are_checked_and_get_no_root_starter`).
- A `src/` layout project (`uv init --package`) counted as a new starter and would have been given `app/` and `tests/`. Only a repo with no Python code is a starter now; a `src/` project's `pytest.ini` says `pythonpath = src`.
- Cache inputs missed `dev-requirements.txt`, `pytest.toml`, `.gitignore` and test folders, so a cached `check` could stay stale.
- A pytest config above a project that belongs to another project (with its own `pythonpath`) counted for it, and its tests then imported the wrong package. The search upward now stops at another project's folder.
- A folder (or a dangling link) where `pytest.ini`, `ruff.toml` or `requirements-dev.txt` should go made `sync` stop; it is a manual finding now.
- Also fixed: an `app` that is not the starter's (a file, or another stack's folder) no longer gets the starter's test.
- Left: only the root `.gitignore` is read; services with their own `.gitignore` still get the lines added at the root (as `core/gitignore-basics` does).

**Known limits:** no type checker in the starter; uv and Poetry lockfiles are not created; the versions are a snapshot of 2026-11-12.

## M15 — Stack pack: Next.js

**Built** (ADR 0029): the `nextjs` pack, opt-in, with six rules whose fixes create or repair what a Next.js 16 App Router project needs: `package-json`, `app-router`, `next-config`, `tsconfig`, `eslint`, `gitignore`. Each has a SKILL.md and fixture tests (passing, failing, fixed, fixed twice). The contents follow create-next-app's `app-empty/ts` template at `v16.4.6`, read from `vercel/next.js` (nextjs.org was blocked).

**Proof by hand** (2026-11-12, Linux, Node 22.23.4, npm 10.9.9): in a folder made by `fscale init --packs core,nextjs`, `npm install` (347 packages, exit 0), `NEXT_TELEMETRY_DISABLED=1 npm run build` (Next.js 16.4.6, routes `/` and `/_not-found`, exit 0) and `npm run lint` (exit 0). Afterwards `git status` shows only the new `package-lock.json`: the build changed none of the generated files. A first attempt with a `tsconfig.json` that had `jsx: preserve` was rewritten by the build; the template's exact file is used since.

**Found on real repos:** a copy of `vercel/commerce` with the pack on gets one finding, the missing ESLint setup, as an agent finding with the install command; nothing is proposed for its files. The other two repos are not Next.js apps, so the pass does not apply to them.

**Independent review before the pull request** found three real bugs, all fixed with regression tests (`nextjs::tests::review`):

- In a workspace whose Next.js app is in a subfolder (`apps/web`), the pack would have built a second app at the root: scripts, `app/`, `next.config.ts` and a root `tsconfig.json`. The pack now applies only when the root package.json depends on `next` (or there is none yet); otherwise one manual finding says the pack checks the root.
- An app with several root layouts in route groups (`app/(shop)/layout.tsx`, a documented pattern) was given an `app/layout.tsx` that would wrap every page in a second `<html>`. Route-group layouts count now.
- `nextjs/eslint` reported a deterministic finding with no package.json but could not fix it, so `sync` stopped when `nextjs/package-json` was off. It now waits for package.json.
- Also fixed: a JavaScript app gets `layout.jsx` and an ESLint config without the TypeScript rules; eslint before 9 or eslint-config-next before 16 is an agent finding, not a config that cannot load; a new package.json says `"engines": { "node": ">=20.9.0" }`; `core/gitignore-basics` no longer writes its header twice when it runs before package.json exists.
- Left: the pack does not check a Next.js app in a subfolder (per-project checks, as the Python pack does, would be the next step).

**Known limits:** no `src/` layout for new apps (an existing one is accepted); no Tailwind; the versions are a snapshot of 2026-11-12; only an app at the repo root is checked.

## M14 — Generators: `add` and `schema`

**Built** (ADR 0028, `docs/generators.md`)

1. `fscale schema` lists the generators; `fscale schema <generator>` prints its input JSON Schema (draft-07).
2. `fscale add <generator> --json '<input>' [--dry-run]`: the input is validated against the schema in the Node host before anything is written, and a bad field is an error that names it (exit 2). `--dry-run` prints a diff per file and changes nothing. A run that would write over any file writes nothing.
3. Two generators: `package` (a JavaScript ESM package with a `node:test` test and a README; says when the workspace list does not cover it) and `rule` (a custom TypeScript rule: `rule.ts`, a SKILL.md that passes `validateSkill`, a Vitest fixture test, and `pack.ts` when there is none).
4. The `agent-ready` pack ships a skill, `firescale-add`, installed for Claude Code, Codex and Cursor by `agent-ready/agent-skills`. It tells agents to list, read the schema, dry-run, then `fscale add`.
5. This repo has the new skill (dogfooding).

**Decisions made for you:** ADR 0028 (code in `packages/cli`, three bridge methods, a small validator instead of Ajv at runtime, `--json` reused for the input, no edits to existing files).

**Found on real repos** (`add package` and `add rule` with `--dry-run` on the three repos): nothing written, as expected. On `fastapi/full-stack-fastapi-template` the root `workspaces` (`packages/*`) already covers the new package, so no note; on `vercel/commerce` and `microservices-demo`, which list no workspaces, the note says so. Nothing to fix.

**Independent review before the pull request** found six real issues, all fixed with tests:

- The npm package would have shipped only `dist/generators/index.js`: `scripts/pack-npm.mjs` matched built files to sources by file name only, so subfolders lost their files and the Node host could not start in a published install. It now matches by path; `pack.test.ts` lists every generator module.
- Rule ids such as `team/1st-rule`, `team/x-` or `team/package` produced a `pack.ts` that does not load. The id now uses the SDK's own pattern, and the import is always `rule<Name>`.
- A description with `*/` broke the rule's comment, and one with a line break broke SKILL.md's frontmatter. Descriptions are one line with something in it; the rule gets it as a quoted string and a line comment; SKILL.md's frontmatter no longer repeats it.
- A folder on the way that was a file left half the files written. That is now checked first, and a failed write removes the files the run created.
- `constructor` and other `Object.prototype` names passed the unknown-field check.
- Generated code mixed quote styles, so this repo's own Prettier check (and so `fscale verify`) failed right after an add. Generated code now uses single quotes and passes Prettier with this repo's settings.
- Also fixed: an older `firescale` package says to update it; folders cannot be inside `.git` or `node_modules` or end in a dot; `maxLength` counts characters; only the `packages:` list of `pnpm-workspace.yaml` is read.
- Left: Windows reserved names (`con`, `nul`) as folder names are not refused.

**Known limits:** two generators only; no custom generators; the validator supports what the generators use.

## M13 — Born agent-ready: agent files from `init`

**Formats confirmed first** (ADR 0027, 2026-11-12): Claude Code from its docs (code.claude.com); Codex from its source at `rust-v0.167.2` (developers.openai.com was blocked); Cursor's hooks and skills from Cursor's own GitHub repositories `cursor/cookbook` and `cursor/plugins` (cursor.com was blocked). The Cursor hook format that M7 marked unconfirmed matches those examples. **Cursor's `.cursor/cli.json` permission format is still unconfirmed** and is written with a note.

**Built**

1. `agent-ready/agents-md` has a fix: AGENTS.md with a **managed block** (`<!-- BEGIN firescale ...` to `<!-- END firescale -->`) holding the commands from `fscale facts` for the root and each project, and the definition of done. Text outside the block is kept byte for byte; a hand edit inside it is a finding, so `fscale sync --check` fails and `fscale sync` repairs it. Python commands are written `python -m ...`, the same on every machine.
2. A root CLAUDE.md that does not import AGENTS.md gets a three-line block with `@AGENTS.md` at its end.
3. `agent-ready/verify-hook` has a fix: the stop hooks for Claude Code, Codex and Cursor, started with the repo's own `fscale` script or `npx firescale`.
4. New rule `agent-ready/agent-permissions`: Claude Code `permissions.deny`/`allow`, a Codex permission profile in a new `.codex/config.toml`, Cursor `cli.json` deny entries.
5. New rule `agent-ready/agent-skills`: the SKILL.md of every enabled rule in `.claude/skills/`, `.agents/skills/` and `.cursor/skills/`, marked as generated.
6. `crates/core/src/jsonedit.rs`: existing JSON settings get new lines only; every existing line stays.
7. This repo runs its own fixes: its AGENTS.md has the block, its `.claude/settings.json` the permissions, and the skills are committed.

**Decisions made for you:** all in ADR 0027, and under "Needs your decision" above.

**Tests changed on purpose, none weakened**

- `agents-md` tests: an AGENTS.md without the block is now a finding, and the rule has a fix. The old "a file with content passes" became "a file with a current block passes".
- `verify-hook` tests: the finding is deterministic with a fix; "fix does nothing" became "the fix sets up every agent, and a second run changes nothing".
- `scaffold.test.ts`: `init` now leaves nothing for a person (snapshot updated). AGENTS.md is compared byte for byte except for the block added after it. The "no second README" check now looks at file names, not paths, because `firescale-core-readme-setup/SKILL.md` is not a README.
- `core-pack.test.ts`: 14 rules on this repo; the empty agent-ready repo passes after `sync` instead of after `hooks install` plus a hand-written AGENTS.md.

**Found on real repos** (copies of the three repos with `agent-ready` turned on, `adopt --dry-run`)

- `fastapi/full-stack-fastapi-template` already has `.claude/skills/` and `.agents/skills/` of its own. firescale's skills go in their own `firescale-*` folders beside them; nothing of theirs would change.
- `GoogleCloudPlatform/microservices-demo`: the block lists ten services, each with its commands; the Python services have only an install command (no test config), which is what facts finds.
- `vercel/commerce`: a block with the root's scripts. Nothing wrong found in this pass.

**Independent review before merging** found five real bugs, all fixed with regression tests:

- The AGENTS.md block depended on the machine: a uv project read `uv run pytest` without a local `.venv` and `python -m pytest` with one, so `sync --check` flipped. Managed Python projects are now always written through their manager (`uv run`, `uv sync`), others with `python -m` (`a_uv_project_reads_the_same_with_and_without_a_local_venv`).
- Marker lines inside a code fence (an example in CLAUDE.md) were taken for the block and replaced. Fences are skipped now (`markers_inside_a_code_fence_are_an_example_not_the_block`).
- A CLAUDE.md that is a symbolic link to AGENTS.md made every `adopt` and `sync` stop with an error. It now counts as AGENTS.md; a link elsewhere is a manual finding (`a_claude_md_that_links_to_agents_md_is_agents_md`).
- With Git for Windows' `core.autocrlf`, every generated skill read as out of date. Line endings are ignored when comparing (`a_skill_checked_out_with_crlf_is_current`).
- A Python project at the root got only its install command in the block. Its own test, lint and typecheck are listed now, and allowed in Claude Code's permissions (`a_python_project_at_the_root_lists_its_own_commands`).
- Also fixed: a settings value of another type (`"Stop": "none"`) and a CLAUDE.md that is not UTF-8 are manual findings instead of stopping the fix loop.
- Left: `jsonedit` cannot add to an empty list written on one line (`"deny": []`), so such a file is a manual finding as a whole; the doc comment now says so.

**Known limits:** a rule turned off keeps its generated skills until they are deleted by hand; skills of custom TypeScript rules are not installed; with a `firescale.config.ts`, `agent-skills` asks for a JSON config.

## M12 — `fscale doctor`, and two fixes from the 0.1.3 run

The publishing checkpoint this milestone needs is met: 0.1.0 to 0.1.3 are on npm, so a teammate can run `npx firescale doctor`. Its second box (a generated repo passing its own CI on GitHub) is still open.

**Built**

1. **Fix: the heartbeat's running count** (ADR 0025). For pytest and unittest, when their markers are the latest output, the heartbeat says `tests so far: 12 passed, 0 failed, 40 errors (in tests/test_orders.py)`; otherwise `last output:` and the last line with words. A lone `E` or a row of dots is never shown.
2. **Fix: a service that is down, seen while a step runs** (ADR 0025). Every 500 ms, new output is read for the documented refused-connection messages, and pytest and unittest counts are kept. When the output names a known local service, or every test so far has errored while a service the step's `.env.example` or Compose file names is down, a probe confirms nothing listens, and one progress line says so at once with the start command. By default the step is then stopped and reported as setup needed (`verify.stopWhenServiceDown: false` lets it run on).
3. **`fscale doctor [--json]`** (ADR 0026, `docs/doctor.md`): tools (`engines.node`, `.nvmrc`, `.node-version`, `packageManager`, `rust-toolchain.toml`, `.python-version`, `requires-python`, `go.mod`), env variables (names in each `.env.example`, set in the environment or in `.env`/`.env.local` beside it; values never kept), dependencies per project (Node packages through `node_modules` and the lockfile's time against the install marker; for Python the virtual environment, its interpreter, and every package of the requirements files or `[project] dependencies`, with `==` pins), and local services from Compose files and `.env.example` (reachable or not, with `docker compose up -d <service>`; only host, port and service name). Every problem has a `next:` command; exit 1 on a problem. JSON schema version 1.
4. **Verify's hint**: when a step needs setup, the summary ends with one line telling the reader to run `fscale doctor`.
5. **Shared service reader** `crates/core/src/services.rs`, used by verify and doctor.

**Decisions made for you:** ADR 0025 (counts, the two conditions, the probe, stop by default) and ADR 0026 (doctor's sections, version rules, what counts as a local service, names only for env files). The "this repo exits 0" test runs `fscale doctor` on the repo with the real tools, so it also proves the claim on each CI runner.

**Found on real repos** (pass on 2026-11-12, binary from this branch, read-only)

- _Fixed:_ `fastapi/full-stack-fastapi-template`'s backend is a member of a root uv workspace (`[tool.uv.workspace]`, `uv.lock` at the root). Facts called it a pip project using `python` from the PATH, so verify would have run `python -m pytest` outside its environment. It is now a uv project (`uv run pytest`); the doctor looks for the workspace's `.venv` at the root. Test: `a_member_of_a_uv_workspace_runs_with_uv`.
- _Fixed:_ the same repo's Compose override publishes ports for the proxy, backend, adminer, mailpit and playwright containers; doctor listed all of them as services to start. Only known database, cache, queue and search services count now, unless an `.env.example` names the port. Test: the service filter in `services.rs` and doctor's real-repo output.
- _Fixed:_ `vercel/commerce` has a root `package.json` with no `"name"`, so facts listed no project and doctor checked neither Node nor `node_modules`. A nameless root that is not a workspace root is now a project. Test: `a_nameless_app_at_the_root_is_still_a_project`.
- _Fixed:_ doctor printed one Node line per `package.json` with the same range; same ranges are now one line. Also "all 1 packages" wording.
- _Left (by design):_ `adopt --dry-run` on the FastAPI template untracks its committed `.env` and `frontend/.env` (the files stay on disk). The template commits development defaults on purpose; `core/no-committed-env` treats every tracked env file the same. Its "To review" note says so.
- _Left:_ on `GoogleCloudPlatform/microservices-demo`, `core/env-example` proposes one root `.env.example` listing the variables of all eleven services (27 names), because no service has its own example file. A per-service example would fit that repo better; this is the rule's "nearest example" design (M5).
- _Left:_ doctor reports each Python service of `microservices-demo` without a `.venv` as a problem. Those services are meant to be built in Docker; doctor checks a machine for local work, so the report is right for that use.

**Independent review before merging** found six real issues, all fixed with tests:

- A step that waits for its own database was stopped on the first probe. A service now counts only when a second probe, 10 seconds later, still finds it down (`a_service_that_comes_up_before_the_second_probe_is_not_down`).
- "Every test errored while a named service is down" labelled a possible code failure as setup needed, which tells an agent not to change code. It is now a failure that names the service (`LIKELY SETUP` in the progress line). Only refused connections make a step setup needed.
- `go version` in a module folder could download the toolchain `go.mod` names, and corepack could download the declared package manager (the `COREPACK_ENABLE_DOWNLOAD_PROMPT` variable does not stop that). Doctor now sets `GOTOOLCHAIN=local` and `COREPACK_ENABLE_NETWORK=0`, and each version command has a 10-second limit (`never lets a version command download anything`).
- An active `VIRTUAL_ENV`, which verify uses, was reported by doctor as "no virtual environment". It is now checked like a project venv (`checks the active virtual environment that verify uses`).
- Version ranges: npm partial versions (`<=22`, `>16`) and PEP 440 `!=3.12.*` and `==3.12` were matched wrongly (`versions_are_read_and_compared`).
- Also fixed from the smaller findings: `pytest -v` lines are counted; doctor reads services from the same folders as verify; localhost is probed on IPv4 first; one table of usual ports; interpreter paths run with no shell; a heartbeat shows a URL's password as `***`; `postgis` and `timescale` images count as PostgreSQL.
- Left: requirements with environment markers are not checked; a `host_ip:` after `published:` in a Compose port is not read; `refused_service` reports only the first refused connection in the output.

**Known limits:** in `docs/doctor.md` and `docs/verify.md`: no `volta`/`.tool-versions`; Node "matches the lockfile" is a file-time comparison; the Compose reader is a line reader; a suite that waits on a service in silence is only caught when it ends or times out.

## 0.1.3 — verify on slow, real projects

**Recorded first:** 0.1.2 was released on 2026-11-12 through the Release workflow (see "Releases" below).

Found by running 0.1.2 on your course project on Windows: verify silent for over ten minutes (a 13-minute pytest run waiting on a test database), Ctrl+C leaving two `node` and four `python` processes, later runs appearing to hang, and a frontend `test` script that watches. All decisions are in ADR 0024.

**Built**

1. **Progress on stderr**: `[verify] test in backend/audit_service: started: ...`, `... still running after 1m 00s; last output: ...`, `... FAILED after 13m 02s (exit 1)`. On in a terminal, off otherwise; `--progress` and `--no-progress` override. `--json` on stdout is unchanged.
2. **Stopping cleanly**: Ctrl+C, `SIGTERM`, `SIGHUP`, Ctrl+Break and closing the console stop every command with its whole process tree (process groups on macOS and Linux, job objects on Windows), print the summary of what finished, and exit 130. Tested with a three-level process tree on Linux and macOS (`SIGINT`, `SIGTERM`) and on Windows (a real Ctrl+Break, and fscale killed outright).
3. **Time limits**: 10 minutes a step by default; `verify.timeout`, `verify.<step>.timeout`, `verify.projects.<folder>.timeout` and `...<step>.timeout`. A step past its limit is stopped with its tree and reported as `TIMED OUT` with its last lines and the key that raises the limit. Other projects still run.
4. **No input, `CI=1`**: unless the environment sets `CI` or `verify.ci` is `false`. `docs/verify.md` lists what `CI` changes (Jest stops writing new snapshots, Playwright configs turn on `forbidOnly`).
5. **Watch mode**: `fscale facts` uses `test:run` (or `:ci`, `:once`) for a watching `test`, `lint` or `typecheck` script, with a note; without one it warns. Your frontend now runs `npm run test:run`.
6. **Services not running**: a refused connection to localhost (libpq, redis-py, MySQL, Prisma, Node, asyncio, pymongo, Python sockets) is setup needed; with a Compose file the fix is `docker compose up -d` (or `-f ../../docker-compose.yml`).
7. **`--jobs <n>`**: independent projects in parallel; the summary and JSON keep the plan's order.
8. **Stale runs**: verify takes no lock. It names another verify that is running in the repo and what it runs, and commands an earlier killed run left behind, with the command that stops them.
9. **PLAN.md**: M17 now asks for a time budget on the base run, and M19 says the stop hook runs only the projects a change touches.

**What the later runs were waiting on.** Not a lock: fscale has none. 0.1.2 waited for a step's output pipes to close rather than for the step, so any process a step left in the background (a test server, a worker) kept verify waiting, silently, after the step had ended; a new test reproduces it with the old wait and passes with the fix. On top of that, each later run repeated the 13-minute pytest run with nothing printed, while the processes the interrupted run had left kept running. I could not confirm on your machine which of the two each run hit; both are fixed, and progress now shows which step runs and what it last printed.

**Decisions made for you** (all ADR 0024)

- **Default limit 10 minutes**, counted per step; `false` for no limit; `0` is refused.
- **A timed-out step counts as a failure** (exit 1) and stops the rest of its project.
- **Exit code 130** for a stopped run; the hook lets the agent stop when verify was stopped.
- **`CI` from the environment wins** over `CI=1`.
- **Grace period 2 seconds** between `SIGTERM` and `SIGKILL`; at most 2 seconds of waiting for output after a step ends.
- **What a step leaves running is stopped when it ends.** On Windows that includes daemons (build servers), which restart on the next run.
- **Root `commands` in facts use the run-once sibling too**, so `agent-ready/commands-documented` asks for `npm run test:run`.
- **Run records live in the temp folder**, not in the repo.
- **`libc` and `windows-sys` became direct dependencies**, at the versions already in `Cargo.lock`.

**Independent review before the pull request** found eleven issues. Fixed:

- On Windows the "left behind" note could name a reused process id with a `taskkill` command; it no longer comes up on Windows, and records older than a day are dropped.
- A Ctrl+Break that also killed a step's shell was reported as FAILED; it is now STOPPED, tested with the event sent to the whole console.
- Closing the console ended fscale before the summary; it now waits for it.
- Ctrl+C during `check` gave an error; it is now a stop (exit 130).
- A second Ctrl+C now kills at once.
- A `try_wait` error was reported as a user stop.
- A record being written could be deleted by another run, and notes came in random order.
- Three tests were weak: one could kill a reused process id, the 3-second limit was tight for Windows runners, and the 2-second pipe wait had no test.

Left: the facts schema stays at version 2 although its new `notes` field fails an old validator with `additionalProperties: false`. That is the policy 0.1.2 followed for `install` and `interpreter` (ADR 0022). Also left: on Windows, if no job can be made, what a step leaves running after it ends is not stopped (documented).

**Tests changed on purpose, none weakened**

- Verify snapshots and two assertions: schema version 4. The help snapshot: the new options and exit code 130. The "unknown verify step" message now also lists the other keys.
- The course fixture's frontend gained `"test": "vitest"` and `"test:run": "vitest run"`, like your repo, so its frontend test step now runs (`npm run test:run`) instead of being skipped; the verify and facts snapshots, the plan test and the call-log tests changed with it, and the CLI tests got an `npm` stand-in.

**Known limits**

- On Unix, a process that leaves its process group (`setsid`) survives its step, and if fscale is killed with `SIGKILL` its commands keep running (the next run names them).
- The heartbeat shows only the last line of output; the output itself is not streamed.
- Setup detection still knows only the documented messages; an unknown port's refusal is reported as "a service on localhost:<port>".
- Not measured: how much `--jobs` saves on the course project.

## Releases

- **The version is 0.2.1 in every place it is recorded** (`node scripts/bump-version.mjs --check`), set in the 0.2.1 pull request. **No tag has been pushed and nothing was published.** 0.2.1 holds M17 (the project graph, `fscale graph`, `verify --affected`, `verify --dry-run`), the M17 follow-up (docs-only changes run no project) and the 0.2.1 fixes (ADR 0035). 0.2.0 was never tagged either; tag `v0.2.1` on `main` when ready (`RELEASING.md`).
- (Earlier) **The version was 0.2.0 in every place it is recorded** (`node scripts/bump-version.mjs --check`), set after M16 in its own pull request. **No tag has been pushed and nothing was published:** tag `v0.2.0` on `main` yourself when ready (`RELEASING.md`). 0.2.0 holds M12 to M16: `fscale doctor`, the heartbeat counts and early stop for a service that is down, the agent files from `init`, `fscale add` and `fscale schema`, and the `nextjs` and `python` stack packs.

- **0.1.3 was released on 2026-11-12** and run on your course project on Windows the same day. Progress lines, setup detection, parallel projects (`--jobs`) and Ctrl+C all worked, and no process was left behind after Ctrl+C. Two problems from that run are fixed in M12's pull request: the heartbeat showed `last output: E` (a pytest marker), and three services spent over six minutes with every test erroring because their test database was not running.
- **0.1.2 was released on 2026-11-12 through the Release workflow**, with npm trusted publishing, like 0.1.1.

## M11 — The group-project pack

Needed the publishing checkpoint: 0.1.0 and 0.1.1 are on npm, so the CI this pack checks for (`npx firescale verify`) works for a real team. The checkpoint's second box (a generated repo passing its own CI on GitHub) is still open. All decisions are in ADR 0023.

**Built**

- `group-project/pr-template`: a pull request template exists in the root, `docs/` or `.github/` (any letter case, or a `PULL_REQUEST_TEMPLATE/` folder). The fix creates `.github/pull_request_template.md`.
- `group-project/contributing`: a contributing guide exists in the same places. The fix creates `CONTRIBUTING.md` with each project's install, lint, typecheck and test commands from `fscale facts`, the check to run, the branch patterns and what a pull request needs.
- `group-project/ci-verify`: a GitHub Actions workflow that runs on pull requests runs `fscale verify`. With no CI at all, the fix writes the same starter workflow as `core/ci-exists` (shared code, and it runs after that rule). A hand-written workflow is never changed: the finding is for an agent, with the one step to add.
- `group-project/branch-name`: the checked-out branch, or in a pull request run the branch in `GITHUB_HEAD_REF`, matches the `branches` patterns. No fix.
- Each rule has a SKILL.md and fixture tests (passing, failing, fixed, fixed twice). CLI tests: `init --packs core,group-project` leaves the pack with no violations; the template and the guide (for an empty repo and for the course project) are snapshots; a hand-written workflow survives `adopt`, `sync` and `fix` byte for byte.
- GitHub's rules for the two files and for `GITHUB_HEAD_REF` were read on 2026-11-12 from the source of GitHub's docs (docs.github.com itself was blocked from the build session).

**Decisions made for you** (all ADR 0023)

- **A file counts only when git tracks it.** An untracked template or guide is a manual finding with the `git add` command.
- **Default branch patterns:** `main`, `master`, `develop`, and `feat/*`, `fix/*`, `docs/*`, `test/*`, `refactor/*`, `chore/*`, `ci/*`, `build/*`, `perf/*`, `style/*`, `release/*`, `hotfix/*`, `revert/*`.
- **`*` in a pattern matches any text, `/` included.**
- **The contributing rule has its own `branches` option**, with the same default, because a rule sees only its own options. Set both when you change one.
- **The guide writes Python commands as `python -m ...`**, never with this machine's `.venv` path.
- **`ci-verify` accepts any line that runs `firescale` or `fscale` with the word `verify`**, such as `npx firescale verify`, `pnpm fscale verify` or `npx firescale@0.1.2 verify`, in a workflow that runs on pull requests.
- **CI that is not GitHub Actions is a manual finding** for `ci-verify` (it cannot read it), where `core/ci-exists` says nothing.
- **Severities:** template and branch name low, guide and CI medium. All start as `evaluated`.
- **The explain test of every skill now runs with every pack on**, in a scratch repo, rather than turning `group-project` on for this repo.
- **The repo's own main branch always passes `branch-name`** (`init.defaultBranch`, `origin/HEAD`).
- **`ci-verify` reads only `run:` steps**, and the command must be firescale's verify, not a mention of it.

**Independent review before the pull request** found seven issues. Fixed: a deleted tracked template or guide in another letter case could get a second file beside it; `init` on a machine with `init.defaultBranch = trunk` failed its own branch rule (the test now isolates git config); `.gitignore` was not a cache input of the rules it can block (nor of `core/ci-exists`, which now has it too); a step's name or an `echo` that mentioned `fscale verify` passed `ci-verify`; a pull request's branch got `git branch -m` advice; an untracked file that `.gitignore` ignores got a `git add` that fails. Left: `npm run verify` is not followed into package scripts (documented), and the starter workflow still says it was created by `core/ci-exists` when `ci-verify` wrote it.

**Known limits**

- The guide is written once. `sync` does not update it when commands change.
- `ci-verify` reads only GitHub Actions workflows. It does not check that the verify step comes after the install step, and it does not follow `npm run verify` into package scripts.
- Bot branches such as `dependabot/...` fail `branch-name` unless a pattern is added. It is report-only unless you enforce it.
- `branch-name` judges only the branch that is checked out (or the pull request's in CI), not every branch on the remote.
- Templates for issues are not part of the pack.

## 0.1.2 — fixes from running 0.1.1 on your course project

**Recorded first**

- **0.1.1 was released on 2026-11-12 through the Release workflow, with npm trusted publishing.** All four stages passed: check version, the six builds, publish to npm, and the check from Linux, macOS and Windows. It was the first release that no person published by hand.
- From that release: a trusted publisher with a wrong **workflow filename** fails as `E404 Not Found` on the `PUT` of the package, not only as `ENEEDAUTH`. Added to the table in `RELEASING.md` and to its notes on setting up publishers.

**Built** (ADR 0022)

1. **Each Python project runs with its own interpreter.** A `.venv` or `venv` in the project's folder (`.venv/Scripts/python.exe` on Windows, `.venv/bin/python` elsewhere), then uv, poetry or pipenv, then an active `VIRTUAL_ENV`, then `python` from the PATH. `fscale facts` shows each project's choice (`interpreter:` and the `interpreter` object in JSON). With none found, the commands are today's, `fscale facts` says "no virtual environment found", and verify prints a `note:`.
2. **Projects are independent.** One project's failure stops only the rest of that project. Every project's result is reported, and the headline names every failure: `verify: FAILED at test in backend/audit_service, lint in frontend`.
3. **Setup needed is reported apart from failures.** A missing module, a command that is not found or dependencies that are not installed give the status `SETUP` (`setup-needed`), with the command that sets it up, from the project's new `install` command. The detection is a short documented list; when unsure, it is a normal failure. The stop hook's message starts with `SETUP NEEDED:` and tells the agent not to edit code, tests or config for it.
4. **Ruff and mypy are found in dependency lists**: requirements files (with `-r` includes), `pyproject.toml` dependency and dev lists (PEP 621, PEP 735, uv, pdm, poetry) and `Pipfile`.
5. **`core/env-example` treats `playwright.config.*`, `vitest.config.*`, `vitest.workspace.*`, `jest.config.*` and `cypress.config.*` as test code.**

- Each has a caught case and a not-a-finding case. Rust tests run on all three OSes; a Windows-only test pins `.venv\Scripts\python.exe`. New CLI tests (`python-env.test.ts`) copy a stand-in interpreter (`fscale-python-stub`, in the testkit) into a real `.venv` and run verify, so the venv's interpreter is really started on Linux, macOS and Windows.
- Version 0.1.2. **No tag was pushed.**

**Decisions made for you** (all ADR 0022)

- **The interpreter order** above. The project's own `.venv` is the most specific and never installs anything; uv and poetry pick their environment themselves; an activated venv belongs to the shell, so it only fills in.
- **A uv or poetry project with a `.venv` runs that `.venv` directly**, not through `uv run`, so verify never syncs or installs.
- **On Windows a command has a backslash path** (`.venv\Scripts\python.exe -m pytest`), because `cmd` cannot start `.venv/Scripts/python.exe`. The one exception to forward slashes in output.
- **A `.venv` without an interpreter for this platform is passed over**, and the reason is in `detail`.
- **`commands.install` is new in facts** for Python projects and Node projects that are not workspace members. The facts schema stays at 2, since the change only adds fields.
- **A failing `check` still stops everything.** The root and each project are the independent units.
- **`failedStep`, `failedProject` and `rerun` name the first failure**; every failure is in `steps` and the headline.
- **Setup needed exits 1** and makes the result `setup-needed` (or `failed`, when something else failed too).
- **A missing Python module that is a folder or `.py` file of the project, or above it, is a normal failure**, since it can be a code mistake.
- **Setup detection reads the whole output** of a failed step, not only the last 30 lines.
- **The stop hook still blocks on setup**, with the message above.
- **The CLI test helper removes `VIRTUAL_ENV`** from every test's environment unless the test sets it.

**Tests changed on purpose, none weakened**

- Facts snapshots: Python projects gained `install` and `interpreter`, Node projects that are not workspace members gained `install`.
- Verify snapshots: schema version 3, and the `note:` line for services with no venv.
- Rust tests that build a `ProjectPlan` or `VerifyPlan` by hand gained the two new fields. Rust tests that detect facts now pass "no active venv" explicitly.
- One new facts test of my own expected `pip install -r requirements-dev.txt` for a service whose dev file does not include `requirements.txt`. The review showed that would leave `jwt` missing, the exact case you hit; it now expects `-r requirements.txt -r requirements-dev.txt`.

**Independent review before the pull request** found seven issues, all fixed: a missing module among real test failures (or in a `src/` package) was called setup; a uv project with `.venv` got a pip install command (uv's venvs have no pip); the install could miss `requirements.txt`; the hook message did not say that other failures still need code fixes; a `%` in a Windows `VIRTUAL_ENV` path; `stepsRun` did not count setup steps, and the note named projects whose commands all come from the config; and a few test-runner config names (`playwright-ct.config.*`, `vitest.setup.*`, `jest.setup.*`, `vitest.e2e.config.*`, any letter case).

**Known limits**

- Setup detection knows only the messages in `docs/verify.md`. Others are normal failures.
- `VIRTUAL_ENV` is the only activation that is read. A conda environment is not recognised unless it sets `VIRTUAL_ENV`.
- A requirements file that only a Dockerfile or a CI step uses still counts as the project's dependency list.
- The pyproject reader does not handle inline-table spellings such as `optional-dependencies = { dev = [...] }` under `[project]`, or quoted dotted section names.

## Automated releases (after M10, at your request)

**Built** (ADR 0021)

- `.github/workflows/release.yml`: a pushed tag `v<version>` checks the version, builds the six platform tarballs, publishes the eight packages with npm trusted publishing (OIDC), and then checks on Linux, macOS and Windows that `npx firescale@<version> --version` prints the version. A run started by hand builds only and publishes nothing, as before.
- No npm token and no secret. Only the publish job has `id-token: write`.
- `scripts/bump-version.mjs`: sets the version in `Cargo.toml`, `Cargo.lock` and every `package.json`, or checks that they agree and match a tag.
- `scripts/publish-npm.mjs`: publishes in order (six platform packages, `@firescale/sdk`, `firescale`) and skips any version already on the registry, so a half-finished release is completed by running the job again.
- `scripts/verify-published.mjs`: the check after publishing, with up to 20 attempts 15 seconds apart.
- `RELEASING.md`: your steps, including the one-time trusted publisher setup for each of the eight packages.
- The version is 0.1.1. **No tag was pushed.**

**Checked against npm's docs first** (read on 2026-11-12 from npm's documentation repository on GitHub, because docs.npmjs.com could not be reached from the build session)

- Trusted publishing needs npm 11.5.1 or later and Node 22.14.0 or later. The publish job uses Node 24, which ships npm 11.19.1 today; the script checks both.
- The job needs `id-token: write` and a GitHub-hosted runner.
- Each package's `repository.url` must be this repository. It is.
- A new trusted publisher entry must be used within 2 days or it expires.
- `npm publish` must be among the entry's allowed actions.

**Decisions made for you** (all ADR 0021)

- **The publish job is inside `release.yml`**, not a separate workflow, because npm validates the workflow the run started from.
- **A missing or wrong tag stops the run before any build.**
- **The tarballs of `@firescale/sdk` and `firescale` come from the `linux-x64` build.** Every platform job builds a copy; one is published.
- **"Already published" is decided by asking the registry for that exact version.** Any answer other than "yes" or "not found" stops the job.
- **Node 24's own npm is used**; npm is not upgraded during a release.
- **`actions/download-artifact` v8.0.2 was added**, pinned to a commit.
- **Runs for the same tag never overlap, and a run in progress is never cancelled.**
- **A tag is only published when its commit is on `main`.**
- **A pre-release version (such as `1.0.0-rc.1`) is published under the dist-tag `next`**, never `latest`.
- **Each attempt of the check after publishing uses a new npm cache**, because npm remembers "not found" for five minutes.

**Known limits**

- The publish and verify stages are untested end to end until the first tag.
- The `linux-arm64` runner and the two cross-compiled targets (`darwin-x64`, `win32-arm64`) have still never been built by the workflow, unless you started a manual run.
- Renaming `release.yml` or moving the repository breaks publishing until the eight entries are made again.
- A version lower than the current `latest` (a fix for an older line) is refused by npm and is not handled.
- If the workflow itself has a bug, re-running cannot fix it: the tag has to be moved to the fixed commit (`RELEASING.md`).
- A release uses six build jobs, one publish job and three verify jobs of Actions time.

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
- **Skipped while searching:** everything `.gitignore` ignores, every folder starting with a dot (`.venv`, `.pytest_cache`, `.ruff_cache`), folders of examples and fixtures (`fixtures`, `__fixtures__`, `test-fixtures`, `testdata`, `examples`, `example`, `samples`, `templates`), and `node_modules`, `bower_components`, `vendor`, `site-packages`, `venv`, `env`, `__pycache__`, `dist`, `build`, `out`, `target`, `coverage`, `htmlcov`, `test-results`, `playwright-report`, `tmp`, `temp`, `logs`.
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
- **A second kind of project at the root runs too.** A repo with, say, `Cargo.toml` and a Python package at the root used to run only one of them. The other is now listed as `in the repo root (python)`. Turn it off with `"verify": { "projects": { ".": false } }`.
- **A `pyproject.toml` with only `[tool.*]` tables is config, not a project.** So a shared ruff config above your services does not hide them.
- **A linter is run only when its config file is there**, and the repo root gets no guessed `tsc`.
- **A `package.json` that cannot be parsed still counts as a project**, with every step skipped and a warning in `fscale facts`.
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
- Test code is recognised by its path only, and case-sensitively. Folders named `spec`, `testing` or `playwright` are not test folders.
- With bun, uv, poetry or pipenv, verify's commands go through `bunx`, `uv run`, `poetry run` or `pipenv run`, which can install what is missing. Only `npx --no` is certain not to. I could not reach those tools' docs from the build session to pick safer flags.
- On macOS and Windows, a `verify.projects` key written in the wrong letter case adds a second run in that folder instead of replacing the project's own.
- The stop hook's note on a partial run goes to stderr. A person or a log sees it; the agent may not.
- An independent review found fourteen issues before the pull request, two of them ways to still get `verify: OK` with steps not run. ADR 0020 lists what was fixed and what was left.
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

**0.1.0 was published by you on 2026-11-12, by hand.** `npx firescale@0.1.0 init` and `npx firescale@0.1.0 adopt --dry-run` work on Windows and Linux. Step 7 below, the generated CI passing on GitHub, is still open. **From 0.1.1 on, releases go through the Release workflow: follow `RELEASING.md` instead.** The manual steps below are kept only as a fallback:

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
