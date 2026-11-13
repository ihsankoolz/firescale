# 0043 — Fixes after running 0.3.0 on the course project

Status: accepted (2026-11-13, for 0.4.0). Refines ADR 0036, 0037, 0038 and 0040; changes the default time limit of ADR 0024 and the `fixed` runs of ADR 0040, with the owner's approval; extends ADR 0034.

## Context

The owner ran 0.3.0 on the course project (Windows 11, PowerShell; several Python services and a JavaScript frontend). The head was `origin/main` plus two untracked files, with no code change, and verify blocked:

1. Four Vitest tests in one file fail on Windows on the head and on the base. The note said the same tests fail on the base, "but not in the same way": the M19 output guard compared the whole output, and durations, paths and order differ from run to run.
2. No Node step was ever cached: "the version of npm could not be read" (and npx). On Windows the step runs through `cmd`, which starts `npm.cmd`; the version probe found `npm`, the shell script Node installs beside it, which Windows cannot run. Nothing said so without `FSCALE_LOG=cache`.
3. Base results are kept, passed or failed, but item 2 meant a Node step had no key, so nothing was kept for it.
4. Every Python base step failed in half a second in the base worktree and in the head-check worktree: the tests read the ignored repo-root `.env`, which a worktree lacks.
5. The base run covered eight steps, three of which had passed on the head.
6. Two passing pytest suites (8% and 46% done, every test green) were stopped at the fixed 10-minute limit.
7. Frontend lint failed only because a package was not installed; doctor knew.
8. `npm install`, as doctor suggested, rewrote `package-lock.json`, which then took the base comparison away for the frontend (ADR 0040).
9. An untracked `image.png` at the repo root made all six projects run.
10. Five projects had the same setup problem; the summary printed five near-identical blocks.

## Decision

### 1. Per-test labels decide first

When a step's runner gives per-test results on the head and the base (pytest's JUnit report, Vitest's and Jest's default reporters, `cargo test`), and the head has at least one failing test:

- a failing test that does not fail on the base makes the step `new` (as before);
- otherwise, when every failing test fails on the base **for the same reason**, the step is `pre-existing`, without the M19 output guard;
- a test that fails on both for a different reason blocks: the step has no label, its note names the tests (`this test fails on the base commit abc1234 too, but for a different reason: <id>`), and the test has no label.

This reverses ADR 0038's "per-test results never make a failure pre-existing on their own", at the owner's request. The M19 guard stays for every step without per-test results, and for a head with per-test results but no failing test (the step failed for something the runner did not report as a test). The other guards stay ahead of it: a timeout, a run that stops at its first failure, and a base on which no test passed are never `pre-existing`.

**What "the same reason" means.** A test's reason is its error's lines: the failure's `message` in pytest's JUnit report; for Vitest the lines under its `FAIL` line(s) in the `Failed Tests` section, for Jest those under its `●` line, for `cargo test` those of its `---- <name> stdout ----` block, in each case up to the first stack frame (`at `, `❯ `, `File "`, `stack backtrace:`), without code excerpts (`  5| x`, `> 4 | x`, `    |  ^`) and without cargo's backtrace note. Each line is normalised:

| Taken out                                                                                        | Written as         |
| ------------------------------------------------------------------------------------------------ | ------------------ |
| the run's folder (the head, or the base worktree), in any letter case, with `\` or `/`           | `<root>`           |
| the temp folder (`std::env::temp_dir`, `/tmp`, `/var/folders`) and the one random name inside it | `<temp>/*`         |
| a duration (`12ms`, `1.5 s`, `3 seconds`, `2 min`)                                               | `<time>`           |
| a clock time or a date (`14:08:27`, `2026-11-13T10:22:33Z`)                                      | `<time>`           |
| line and column after a file name (`a.test.ts:5:15`)                                             | `a.test.ts:#`      |
| `line 12`                                                                                        | `line #`           |
| a memory address (`0x7f3a2b`)                                                                    | `0x#`              |
| runs of spaces, ANSI colours                                                                     | one space, nothing |

Each line is then hashed (16 hex digits of blake3) and the hashes sorted, so the order of lines does not count; the order of tests never counted. **Different** means: both runs give a reason for the test and the sorted hash lists differ. Every other number stays, so `expected 3 to be 4` and `expected 5 to be 4`, or an `AssertionError` and a `TypeError`, are different reasons. A reason that one side does not give is not counted as different (only a clear difference blocks). The base store keeps the hashes (store format 5); no reason text is kept anywhere.

Two kinds of failure beside the tests count as failing tests of their own, so that old failures cannot hide them: Vitest's `Unhandled Errors` (`(unhandled errors)`) and a coverage threshold that is not met in Vitest or Jest (`(coverage threshold)`).

Proof: `blame.test.ts` "labels the same failing Vitest tests pre-existing despite other durations, paths and order, and blocks a new one beside them" (a stand-in printing Vitest's real layout with random durations, the run's folder in stack frames, a random temp folder in one error and a shuffled order) and "blocks a test that fails on the base too, but for a clearly different reason"; `pertest::tests::a_reason_leaves_out_what_changes_between_runs_and_keeps_values` and `each_runner_gives_each_failing_test_its_reason`.

### 2. Tool versions on Windows, and saying why a step is not cached

On Windows a program name without one of the `PATHEXT` extensions is looked for only with each of them, in `PATHEXT` order, never bare: how `cmd` finds what a step starts (`npm` is `npm.cmd`, also `npx.cmd`, `pnpm.cmd`, `yarn.cmd`). A `.cmd` file is run by its path, which Rust's standard library starts through `cmd`. Both the version probe and the program's identity in the key use this.

A step that **passed** and that the next run cannot reuse prints one line in the summary, with no setting: `Not cached: <step> passed, but the next run cannot reuse it: <reason>.` That is when its key could not be made (a tool's version, a service the repo does not start, a dynamic import, ...) and when its pass was not stored (its files changed while verify ran, or the entry could not be written). In `--json` the step has `notCached` (an additive field; the schema stays 6).

Proof: `stepcache::tests::a_name_without_an_extension_is_tried_as_cmd_tries_it_on_windows`, `the_version_of_npm_is_read_from_npm_cmd_beside_a_shell_script` (Windows only), and `step-cache.test.ts` "caches a Node step started as npm.cmd beside the shell script named npm that Node installs" (it adds the shell script on Windows) and "says on every run, without FSCALE_LOG, why a passed step is not cached".

### 3. Base results, passed or failed

Base results were already kept whether they passed or failed (ADR 0037), keyed by the step's M18 key with the base commit's blob ids in place of the working tree's files (`StepCache::base_key`). That stays; head results stay passes only. With item 2, a Node step on Windows now has a key, so its base result is kept. The key now also holds a hash of the env files linked in (item 4). Results that say nothing about the code (setup needed, timed out, over budget, stopped) are still not kept.

Proof: `blame.test.ts` "reuses every passed step, Node steps included, and runs no base step again" (the course fixture, Python failing on both: the second run runs only the failing head steps again, and no base step).

### 4. The head's env files in the worktrees

`.env` and `.env.local`, at the repo root and in each planned project's folder, are linked from the head into the base worktree and into the head-check worktree of ADR 0040, as `node_modules` and virtual environments are: a symbolic link; on Windows without the right to make one, a hard link; failing both, a copy. Each is removed before the worktree. A file the worktree already has (tracked on that commit) is left alone, and a link that cannot be made is not a reason to leave the step unjudged. Their contents are never read into a log, a record, a note or the cache: the only use of their bytes is a 16-digit hash in the base result's key (`env=<path>:<hash>`), so a kept result is not reused after a value changes.

Proof: `blame.test.ts` "links the head's ignored root .env into the base worktree, and keeps no value from it": the test reads `.env`; it is `pre-existing` (it fails on the code change) with the base run in the worktree, and the secret is in none of the outputs (human, JSON, progress, `FSCALE_LOG=cache`), none of the files under `.firescale/`, and none of the run records. It fails without the link.

### 5. A base run only for a step that did not pass

Only steps that did not pass on the head run on the base (and in the head-check worktree). A passing step therefore gets no base result and no `fixed` label. `verify.labelFixed: true` brings back ADR 0040's runs of passing steps, for those who want the label; the stop hook never runs them (ADR 0039).

The tests of ADR 0040's `fixed` now set `labelFixed`, and the two tests that counted base runs of passing steps count one fewer: what they prove (results are kept and reused; a changed requirements list is not reused) is unchanged.

Proof: `blame.test.ts` "runs no base step for a step that passed on the head, so a repaired one has no label".

### 6. Time limits follow progress

The fixed 10-minute default of ADR 0024 becomes two limits:

- `verify.idleTimeout`, default **10 minutes**: a step that prints nothing new for that long is stopped. Any new output counts.
- `verify.timeout`, default **60 minutes**: the overall limit.

A limit set for a step (`verify.<step>.timeout`) or a project (`verify.projects.<folder>.timeout`, `....<step>.timeout`) still wins: it is then the step's only limit, with no idle limit, as in ADR 0024. `false` turns either key off. A step stopped for silence reads `(no new output for 10m, stopped after 23m 10s)`, and its `timeoutKey` is `verify.idleTimeout`.

When the runner prints a percent done at the end of a line (pytest's `[ 46%]`; nothing else is read as progress), the heartbeat says `46% done, about 10m 34s left at this pace`, and so do the timeout's detail and summary. The estimate is the time so far times (100 − p) / p.

Proof: `config::tests::a_step_is_stopped_after_ten_quiet_minutes_or_an_hour_in_all_by_default` (the defaults and the keys) and, with the same limits in seconds, `verify-run.test.ts` "lets a slow suite that keeps reporting results run past its idle limit", "stops a step that prints nothing new for its idle limit", "still stops a step that keeps printing at the overall limit" and "keeps a limit set for the project as the only one"; `activity::tests::the_percent_done_is_read_from_the_end_of_pytest_lines`.

### 7. Lint and typecheck on a package that is not installed

The setup detection of tests applies to lint and typecheck too, with these messages: CommonJS `Cannot find module 'x'` (with `Require stack:` or `MODULE_NOT_FOUND`), ESLint 8's `Failed to load plugin`/`parser`, `ESLint couldn't find the plugin "x"`, `ESLint couldn't find the config "c" to extend from` and `Failed to load config "c" to extend from` (the config name turned into its package as ESLint does: `next` is `eslint-config-next`, `@scope` is `@scope/eslint-config`, `@scope/x` is `@scope/eslint-config-x`), TypeScript `TS2307` and `TS2688`, mypy's `Cannot find implementation or library stub for module named "x"`, and the existing ESM `Cannot find package 'x'`.

A Node, ESLint or TypeScript message about a bare package is setup needed only when the package (`x` up to the first `/`, or the second for a scoped name) is in no `node_modules` from the step's folder, and from the folder of the importing file the message names, up to the repo root, and some `package.json` in those folders lists it (dependencies, devDependencies, peerDependencies or optionalDependencies); with no `package.json` there it is setup needed. TS2307 and TS2688 count `@types/x` as the package being present. Paths, drive letters, names with `:` or `\`, `node:` built-ins, `#` imports and aliases starting `@/`, `~` or `$` are never a package. A folder with `.pnp.cjs` or `.pnp.js` (Yarn Plug'n'Play) is never setup needed. mypy's message is setup needed unless the module is part of the project.

Proof: `setup::tests::lint_and_typecheck_on_a_package_that_is_not_installed_are_setup_problems` and its not-a-finding twins; `verify.test.ts` "Lint that fails on a missing package is setup needed, not failed".

### 8. Installs that keep the lockfile

Doctor and the setup-needed text name the install that does not rewrite the lockfile (`setup::locked_install`): `npm ci` with `package-lock.json` or `npm-shrinkwrap.json`; `pnpm install --frozen-lockfile`; `yarn install --immutable` for Yarn 2 or later (`packageManager: yarn@>=2`, else a `.yarnrc.yml` up to the root, else `__metadata:` in `yarn.lock`) and `yarn install --frozen-lockfile` for Yarn 1; `bun install --frozen-lockfile`; `uv sync --locked` (`uv.lock` in the folder or above); `pipenv sync --dev`; `poetry install` and pip unchanged (they never rewrite a lock). The lockfile must be in the folder the install runs in, except for uv. `Commands.install` in `fscale facts`, and what the rules write into AGENTS.md, are unchanged. The npm, pnpm, bun, uv and pipenv documentation sites were blocked from the build environment; the flags were checked against the installed tools' own help (npm 10.9, pnpm 10.28, bun 1.4, uv 0.11, Yarn 1.22, Poetry 2.4) and yarnpkg.com.

Proof: `setup::tests::npm_with_a_lockfile_installs_with_npm_ci` and its siblings; `doctor.test.ts` "names the install that keeps the lockfile".

### 9. Files that are plainly not code or config

Extends ADR 0034. A changed file that no project holds (the root's project included) and no narrow shared node holds affects no project when its name ends, in any letter case, in one of:

- images: `png jpg jpeg gif bmp webp avif ico tif tiff heic heif psd`
- documents: `pdf doc docx odt rtf ppt pptx odp`
- archives: `zip tar tgz 7z rar`, and names ending `.tar.gz`, `.tar.bz2`, `.tar.xz`
- video: `mp4 mov avi mkv webm`
- audio: `mp3 wav m4a flac ogg`

Left out on purpose, so they still run every project: `svg` (bundlers compile it and tests render it), spreadsheets and `csv` (data code reads), fonts (CSS loads them), `key` (also a TLS key), and a bare `.gz`, `.bz2` or `.xz` (often data a program loads, such as `seed.sql.gz`). Everything else no project owns still runs every project (`deploy.sh`, `setup.py`, `Makefile`, `foo.js`). A file a project holds stays that project's input, so a fixture image still runs its tests. With a root project, such a file runs that project only. The widening rules of ADR 0032 come first.

The same files, and ADR 0034's, are left out of every project's step cache key (`stepcache::covers`), so plain `fscale verify`, not only `--affected`, runs nothing for them. The repo root's own steps keep the whole repo in their key.

Proof: `graph::tests::an_unowned_root_image_affects_no_project_but_an_unowned_root_script_affects_all`, `an_image_a_project_imports_or_a_root_project_holds_is_not_inert`; `graph.test.ts` "An untracked root image runs no project; an unowned root script runs all."; `step-cache.test.ts` "runs again only the projects a change reaches" (a root image and README run nothing).

### 10. One block for one setup reason

When two or more unlabelled setup-needed steps have the same reason (the text after the command in the step's detail, without timings), the summary prints `Setup needed for N steps, all for one reason: <reason>. ...` once and lists each step with its own `Set it up` and `Then run again` lines. Only the first step's last lines and first error are shown; `--json` has every step's output. One step keeps the old layout; the JSON is unchanged.

Proof: `verify::tests::projects_with_the_same_setup_problem_share_one_block`; `python-env.test.ts` "prints a reason that several services share once".

## Consequences

- An old failure that prints run-specific text no longer blocks, when its runner gives per-test results; the M19 guard still protects every other step.
- `fixed` labels are gone from a default run; a full verify with failures costs base runs only for those failures.
- A suite that keeps printing can run up to an hour by default; one that hangs silently is still stopped at 10 minutes.
- The tamper guard of ADR 0042 is unchanged (`tamper.test.ts`, `bench/tamper-rates.mjs`).
- Known limits: a failure beside the tests that the runner does not report as a test, other than unhandled errors and coverage thresholds, can hide behind old test failures; reasons are read from default reporters' text, so a custom reporter gives none and the step falls back to the M19 guard when no test is named. On Windows, without the right to make symbolic links and with the temp folder on another drive, an env file is copied into the worktree for the length of the base run.
