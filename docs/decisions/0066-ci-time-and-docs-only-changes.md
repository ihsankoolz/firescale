# 0066 — This repo's CI: where the Windows time goes, and changes that touch only docs

Status: proposed (2026-11-16, 0.5.2 run, pull request 5). Amends ADR 0016 (what runs when): a change of docs only runs no test job, the test jobs are four jobs instead of two matrices, and a push to `main` is never cancelled. Keeps ADR 0009 (actions pinned to commit SHAs). Not about the workflow `fscale init` writes (ADR 0041, `docs/ci.md`).

## Context

The Windows job of a pull request took 24 to 33 minutes; Linux took about 4.5. A pull request that changed one Markdown file under `docs/` ran the whole matrix. And three tests hit their 30 s limit on Windows in two days.

### Where the time goes

Read from the step times of the API and the job logs, for the last three pull request runs that passed and the Linux push run of the same commit. All runners are GitHub's standard ones: Linux and Windows have 4 CPUs, macOS 3. Vitest runs one file per worker and starts as many workers as there are CPUs less one: 3 on Linux and Windows, 2 on macOS.

Commit 74eccc6: push run 43794633022 (Linux job 133060525093), pull request run 43794639420 attempt 2 (macOS job 133071396002, Windows job 133071393947):

| Step                                                           | Linux node 24 | macOS         | Windows       |
| -------------------------------------------------------------- | ------------- | ------------- | ------------- |
| Job, start to end                                              | 279 s         | 554 s         | 1608 s        |
| Queue                                                          | 2 s           | 3 s           | 3 s           |
| Setup and post steps (checkout, pnpm, Node, Rust, both caches) | 23 s          | 28 s          | 70 s          |
| Build / Lint / Typecheck                                       | 18 / 24 / 4 s | 16 / 19 / 4 s | 41 / 36 / 3 s |
| Test step (`pnpm test`)                                        | 205 s         | 483 s         | 1453 s        |
| `cargo test`: compile                                          | 24 s          | 27 s          | 36 s          |
| `cargo test`: firescale_core (875 tests; 859 on Windows)       | 34 s          | 38 s          | 47 s          |
| `cargo test`: firescale_rules (572; 569 on Windows)            | 5 s           | 20 s          | 90 s          |
| Vitest, wall time                                              | 141 s         | 397 s         | 1277 s        |
| Vitest, sum of the 53 files' times                             | 378 s         | 773 s         | 2687 s        |
| `tamper.test.ts` (479 tests)                                   | 64 s          | 153 s         | 871 s         |
| `config-affects.test.ts` (34)                                  | 46 s          | 139 s         | 316 s         |
| `blame.test.ts` (37)                                           | 22 s          | 69 s          | 180 s         |
| `agent-runner.test.ts` (26)                                    | 16 s          | 42 s          | 139 s         |
| `scaffold.test.ts` (32)                                        | 7 s           | 24 s          | 122 s         |
| `verify-run.test.ts` (37)                                      | 76 s          | 81 s          | 114 s         |

The two pull request runs before it, with the push runs of the same commits:

| Commit, runs (push / pull request), jobs                                          | OS      | Job    | Test step | `cargo test` compile / core / rules | Vitest wall / sum of files | `tamper.test.ts` |
| --------------------------------------------------------------------------------- | ------- | ------ | --------- | ----------------------------------- | -------------------------- | ---------------- |
| 36ad227, 43759642200 / 43759663089, jobs 132951326667, 132951394008, 132951393792 | Linux   | 264 s  | 199 s     | 22 / 34 / 5 s                       | 137 / 372 s                | 62 s             |
|                                                                                   | macOS   | 760 s  | 631 s     | 47 / 43 / 23 s                      | 515 / 988 s                | 229 s            |
|                                                                                   | Windows | 2072 s | 1896 s    | 46 / 52 / 121 s                     | 1674 / 3574 s              | 1107 s           |
| e6e31f8, 43756197218 / 43756203083, jobs 132940713292, 132940759022, 132940758831 | Linux   | 231 s  | 155 s     | 18 / 33 / 3 s                       | 101 / 274 s                | 36 s             |
|                                                                                   | macOS   | 801 s  | 664 s     | 34 / 39 / 27 s                      | 562 / 1065 s               | 248 s            |
|                                                                                   | Windows | 1586 s | 1416 s    | 36 / 46 / 96 s                      | 1236 / 2678 s              | 848 s            |

What the numbers say:

1. **Vitest is 85 % of the Windows job** (1236 to 1674 s of 1586 to 2072 s). `cargo test` is about 3 minutes; setup, build and lint together about 2.5.
2. **One file is the tail.** `tamper.test.ts` runs 479 corpus cases one after the other, 1.7 s each on Windows (median of the logged times; 0.13 s on Linux). Vitest starts the files it has no earlier results for largest first, and this file is small (the cases come from `bench/tamper-corpus.json`), so on Windows it started 388 to 567 s into the run, and for the last 506 to 713 s it was the only file still running, on one of three workers:

   | Windows run | Vitest | `tamper.test.ts` started at | ran for | alone at the end for |
   | ----------- | ------ | --------------------------- | ------- | -------------------- |
   | 43794639420 | 1277 s | +406 s                      | 871 s   | 562 s                |
   | 43759663089 | 1674 s | +567 s                      | 1107 s  | 713 s                |
   | 43756203083 | 1236 s | +388 s                      | 848 s   | 506 s                |

   On Linux and macOS the same file is alone for 10 to 47 s.

3. **Every process costs more on Windows.** The suites that start the most processes are the ones that grow most: `firescale_rules` 18 times Linux, `tamper.test.ts` 13 times, `scaffold.test.ts` 17 times, while `verify-run.test.ts`, which mostly waits on its own timers, is 1.5 times. One corpus case is four `git` commands to build the repo and one `fscale verify --json`, and that verify starts 53 `git` processes (counted with `GIT_TRACE` on this machine: 25 `git rev-parse`, 12 `git ls-files`, 4 `git diff`, and others).
4. **The fixtures live on the slow drive.** The checkout and `target` are on `D:` (`D:\a\firescale\firescale`), but `TEMP` is `C:\Users\runneradmin\AppData\Local\Temp`, so every fixture repo, every base-run worktree (`firescale-runs` under the temp folder) and every Rust `tempfile` is written on `C:`. On GitHub's Windows runners `C:` is the system drive and `D:` the faster work drive; pip measured its Windows CI 10 to 28 % faster after moving `TEMP` to `D:` (<https://sichard.ca/blog/2025/03/faster-pip-ci-on-windows-d-drive>, read on 2026-11-16). The runner image here is `windows-2025-vs2026` 20261103.298.1, and it has a `D:` drive.
5. **Not where the time goes:** queue time (2 to 4 s), the pnpm store (restored on an exact key; `pnpm install` takes 1 to 2 s), and a rebuild inside `pnpm test` (there is none: `cargo test` compiles the test harnesses, which `pnpm build` does not, and Vitest uses the `target/debug/fscale` that Build made). The Rust cache on macOS and Windows never matches its key exactly (`full match: false`: a pull request's cache is scoped to its merge ref, and `main` never runs these jobs, ADR 0016), so Build and Lint compile about 30 dependencies again: about 25 s of Build and 15 s of Lint on Windows.

### A change that touches only docs

Pull request #60 added one file, `docs/reviews/cs464-0.5.1.md`. Its runs (push 43793384135, 43794633022; pull request 43793392070, cancelled, and 43794639420, two attempts; the push to `main` 43800981275) took 27.2 minutes of Linux jobs, 72.6 of Windows and 39.0 of macOS, and the first Windows attempt failed on a test timeout and had to be run again. Pull request #48 (`docs/reviews/ponytail-audit.md`) was the same.

### The tests that hit their limit

From the Windows jobs of every failed run and failed attempt since 2026-11-15 (`gh run list --workflow ci.yml --status failure`, and attempt 1 of the runs with two attempts):

| When (UTC)       | Run (job)                            | Test                                                                                                     | Time      | Same test in the other Windows runs             |
| ---------------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------- | --------- | ----------------------------------------------- |
| 2026-11-15 06:30 | 43685125688 attempt 1 (132723890643) | `verify.test.ts` "rejects a project folder that does not exist, and a malformed projects section"        | 30,110 ms | 3.2 to 7.7 s before 8531c25; 0.7 to 5.2 s since |
| 2026-11-15 13:51 | 43720192980 (132830300525)           | `agent-runner.test.ts` "a protected glob that can match no path stops the run: it would protect nothing" | 30,798 ms | 8.3 to 15.5 s since 5e6a3cc                     |
| 2026-11-16 03:06 | 43794639420 attempt 1 (133060576937) | `config-affects.test.ts` "keeps the whole-file rule for a firescale.config.ts"                           | 30,169 ms | 14.9, 15.5, 17.3, 26.1 s                        |

The first two were fixed at the cause in the 0.5.1 run: the verify test now builds one fixture instead of seven (8531c25), and the protected-glob test runs its eleven cases side by side (5e6a3cc). The third is not fixed. It starts `fscale verify` five times on the six-project course fixture, three of them running every project's steps through `.cmd` stand-ins. Its siblings of the same shape are close to the limit too: "runs every step again, and every project under --affected, when <setting> changes" (seven cases, four verify runs each) took 12.8 to 30.0 s in the same five runs: 12.8 to 25.4 s in runs 43750897301, 43756203083 and 43794639420 attempt 2; 17.0 to 30.0 s in run 43759663089, with 30.0 s (`verify.failOnSkipped`, passing) and 28.5 s (`verify.stopWhenServiceDown`); 16.8 to 27.7 s in run 43794639420 attempt 1, the attempt of the timeout. Those two runs were slow as a whole: their Vitest took 1674 and 1611 s, against 1236 to 1296 s for the others (1277 s for the next attempt on the same commit).

Older timeouts of the same kind were given limits of their own before (`scaffold` 60 s, `bridge` 60 s, `step-cache` 90 s, `agent-files` 180 s); `scaffold` "--check exits 1 after a generated file is deleted" passed its 60 s once since (61.6 s in run 43612243158 attempt 1, 2026-11-14).

## Decision

Nothing below drops a test or a platform: every test still runs on Linux, macOS and Windows for every change that is not docs only.

### 1. The longest Vitest files start first

`vitest.config.ts` gets a sequencer, `LongestFirst`, that takes Vitest's own order and moves three files to the front, longest first: `tamper.test.ts`, `config-affects.test.ts`, `blame.test.ts` (the three longest on Windows in every run above). The other files keep Vitest's order. With three workers, `tamper.test.ts` then runs on one worker from the start while the other two take the rest, so the run takes about the longer of `tamper.test.ts` and half of the other files, instead of ending with `tamper.test.ts` alone.

Expected on Windows, from the three runs: 1277, 1674 and 1236 s of Vitest become about 910, 1235 and 915 s, 5 to 7 minutes less, a little less than that since `tamper.test.ts` no longer has the machine to itself. Linux and macOS gain 10 to 47 s. Checked here: with one worker, the files ran in the order `tamper`, `blame`, `cli`, `explain` with the sequencer and `blame`, `cli`, `tamper`, `explain` without it.

### 2. On Windows, the temp folder is on the work drive

A step before Test sets `TEMP` and `TMP` to `RUNNER_TEMP` (`D:\a\_temp`), on Windows only. Node's `os.tmpdir()` and Rust's `std::env::temp_dir()` read them, so the fixture repos, verify's base-run worktrees and `tempfile` folders are on `D:`. Expected: a saving that cannot be measured before the run; pip's was 10 to 28 % of its Windows job.

### 3. A change of docs only runs no test job

- **A first job, `changes`, decides** from `git diff --name-only --no-renames` (plain git, no new action; `--no-renames` lists both paths of a rename). It compares:
  - on a pull request: the merge commit with its first parent, the tip of the base branch;
  - on a push to a branch: the head with its merge base with the default branch. The whole branch, not the push: a docs-only push after a code push must not report the branch as tested;
  - on a push to the default branch: the head with the commit before the push (`github.event.before`);
  - started by hand: nothing; everything runs.
- **Docs only** means every changed path is a `.md` file under `docs/` or at the repo root, with these exceptions, which count as code: `docs/rules.md` (a Rust test includes it), anything under `docs/schemas/` (a Vitest test reads the ledger schema), and `README.md`, `AGENTS.md` and `CLAUDE.md` in any case of letters (the rules that `fscale check` runs on this repo read them, `core/readme-setup` finds `readme.md` too, and `README.md` goes into the npm package). Any other file under `docs/` (an image, a `.json`) counts as code. Nothing under `crates/` or `packages/` counts: each rule's `SKILL.md` is compiled into the binary and snapshotted, and snapshots and fixtures are there too. Found by reading what the build and the tests read: `include_str!` and `CARGO_MANIFEST_DIR` in the crates, `import.meta.url` and `REPO` in the tests.
- **Anything it cannot decide runs everything**: a push with no earlier commit (`0000…`) or one the clone does not hold (a force push), a branch with no merge base, a failed `git diff`, no changed file at all, and a failed `changes` job (the test jobs run whenever its output is not `false`).
- **The test jobs are skipped with `if:`**, never left out with `paths` or `paths-ignore` on the workflow. GitHub reports a skipped job as a passed check; a workflow that a path filter keeps from running reports nothing, and a required check would wait forever.
- **The test jobs are no longer matrices.** A matrix job skipped by `if:` is reported once, under its unexpanded name: run 43794639420 shows `ubuntu-latest / node ${{ matrix.node }}`, skipped, so a required `ubuntu-latest / node 22` would never be reported. The four jobs (`linux-22`, `linux-24`, `macos`, `windows`) have literal names, the same check names as before, and the same twelve steps, written out in each; the Node version is a job variable. A YAML anchor would hold one list (GitHub Actions reads anchors and aliases since 2025-09-18, <https://github.blog/changelog/2025-09-18-actions-yaml-anchors-and-non-public-workflow-templates/>), but firescale's own tamper guard does not: with the steps behind an alias, `fscale tamper --base origin/main` reported `pnpm lint`, `pnpm typecheck` and `pnpm test` as gone from both jobs (six report-only "CI weakened" findings, also for a change that only moved the steps behind an anchor), and after that it would see no command in this workflow at all. Written out, it reports nothing weakened. The four lists must be kept identical by hand, as ADR 0016 kept two.
- **The Rust cache keeps its keys.** rust-cache names a cache after the job id; the shared steps pass `shared-key: linux` or `desktop`, the old job ids, so the caches saved before are found (read in rust-cache's `src/config.ts` at the pinned commit: a shared key takes the place of the job id in the key).
- **A change of docs only still gets the checks that read docs**, in a `docs only` job on Linux (from the push, like the Linux jobs): `pnpm lint:format` (Prettier checks the Markdown) and `fscale check` on this repo with no violation at all, which is what `core-pack.test.ts` asserts (`OK: 14 rules checked, no violations.`; `core/no-secrets-in-source` reads every file). The exit code alone is not that check: this repo enforces no rule, so a report-only violation exits 0. A review showed it: a committed `docs/leak.md` with a `ghp_…`-shaped token gave `OK: 14 rules checked, 1 violation (0 enforced, 1 report-only)` and exit 0, and the next change that ran the tests would have failed on it. The job runs `pnpm --silent fscale check --json` and fails unless `summary.violations` is 0, printing the human report first; replayed here on that commit, it exits 1. This is the cheapest form of the test's check: running the test itself needs `pnpm build` of every crate and package. It leaves out the test's rule count, 14, which only `firescale.config.json` or code can change, and neither is docs. It restores the Linux Rust cache and saves nothing. When the change is not docs only, this job is skipped, which also counts as passed.
- **A job skipped because of the event has another name.** On one commit, the push run and the pull request run both list every job. Before, the push run reported `macos-latest / node 24` and `windows-latest / node 24` as skipped, beside the real ones of the pull request run (seen on 1066acc, push run 43812399243), and a pull request from this repository reported the two Linux checks as skipped beside the push run's real ones. GitHub does not say which of two checks of the same name satisfies a required check, so a skipped copy could stand for a Windows run that is still running or failed. Now the copy skipped because of the event is named `macos node 24 (pull requests only)`, `windows node 24 (pull requests only)`, `ubuntu node 22 (runs in the push run)`, `ubuntu node 24 (runs in the push run)` and `docs (runs in the push run)`. A job skipped because the change is only docs keeps the real name, since it stands for that check.
- **No required check is left waiting**, today or if the owner makes the jobs required: every job always ends as passed, failed or skipped, under its real name when it is the one that counts. To require them, require the four test checks, `ubuntu-latest / node 22`, `ubuntu-latest / node 24`, `macos-latest / node 24` and `windows-latest / node 24`, and `docs only`. Not `changes`: it runs in both the push and the pull request run under one name, and a failed `changes` job already makes every test job run. Never the names with "(pull requests only)" or "(runs in the push run)", and never `bench` or `package` (matrices that run only by hand).
- **A merge queue would need a `merge_group` trigger**, which the workflow does not have; with one, the `changes` job would also need to know the queue's base.
- **No push to `main` is cancelled.** A push to `main` is compared with the commit before it, so a docs-only merge right after a code merge must not cancel the code merge's run (ADR 0016 cancelled the older run on `main` too). Each push to `main` now has a concurrency group of its own, its run id, so its run is never cancelled nor dropped while queued (a group holds one pending run, ADR 0041). Pushes to other branches and pull requests still cancel their own older runs.

Expected for a pull request like #60: the `changes` job (about 15 s) in its push and pull request runs, and the `docs only` job (about 1 to 2 minutes, mostly building `fscale`) in its push runs, instead of 139 job-minutes.

`docs/ci.md` is about the workflow `fscale init` writes, not this one, so it is unchanged.

### 4. The test that timed out: a limit of its own, with the measured times

Of the three, two were fixed at the cause in the 0.5.1 run (one fixture instead of seven; eleven cases side by side), and their Windows times since are far from the limit. The third, `config-affects.test.ts` "keeps the whole-file rule for a firescale.config.ts", starts verify five times on the six-project course fixture, and each run that runs every project starts 18 stand-in commands through `cmd.exe`. None of the runs can be dropped or run side by side without testing something else: each depends on the cache the run before it left. The cause under it is the cost of a process on Windows (see "Where the time goes"), which items 1 and 2 reduce but do not remove.

It gets a limit of 60 s (`SEVERAL_RUNS`), as did the other tests that start verify many times (`scaffold` and `bridge` 60 s, `step-cache` 90 s). So does the test of the same shape that came within 0.1 s of the limit: "runs every step again, and every project under --affected, when <setting> changes" (seven cases, four verify runs each). Measured on Windows in five runs: the `firescale.config.ts` test 14.9 to 26.1 s when it passed and 30.2 s when it timed out (run 43794639420 attempt 1); its siblings 12.8 to 30.0 s, the slowest in run 43759663089 (30.0 and 28.5 s) and 43794639420 attempt 1 (27.7 s). On Linux 1.5 to 3.0 s, on macOS 6.8 to 9.1 s. 60 s is twice the slowest time seen. The rest of the suite keeps 30 s. What the tests assert is unchanged.

## Why not

- **Sharding the Windows test step into parallel jobs.** Vitest shards by file, so one shard still holds all of `tamper.test.ts` (850 to 1100 s), and each extra Windows job pays about 2.5 minutes of setup, build and lint again. The order of item 1 gets most of what two shards would, at no extra minutes.
- **`cargo test` and Vitest in two parallel jobs.** About 3 minutes less wall time on Windows, about 2 more job-minutes (setup and a second compile). Worth it later if wall time matters more than minutes.
- **Splitting `tamper.test.ts` into several files.** With three workers it gains little over item 1 while the other files together take longer than it, and it is a change of test code that the order makes unnecessary.
- **More Vitest workers.** The runner has four CPUs and Vitest uses three; every test starts processes of its own, and more at once is what pushed the timed-out tests over their limit.
- **A Dev Drive.** pip measured ReFS slower than plain `D:` on these runners, and it needs a virtual disk made in every job.
- **Caching Vitest's results**, so that it orders files by their last time: a pull request's cache is scoped to its merge ref and `main` never runs Windows (ADR 0016), so the cache would almost never be found.
- **`paths-ignore` on the workflow**: see item 3.
- **Fewer `git` processes in `fscale verify`.** One corpus case starts 53 (`git rev-parse --is-inside-work-tree` 14 times, `git ls-files -z --cached` 8 times, and others), and `init` stages about fifty files one `git add` each. Fewer would help every Windows user, not only CI, but it is a change of product code, not of this pull request.

## Consequences

- Expected Windows job: about 20 to 25 minutes instead of 26 to 35. The measured time of this pull request's own run is in DECISIONS.md.
- A docs-only change shows the four test checks as skipped and `docs only` as passed. A file the build reads counts as code, so a slip in the list costs minutes, not a missed test.
- On `main`, every push now runs to the end, also when another merge lands soon after: more Linux minutes when merges come close together, never a code merge left without its Linux run.
- Docs outside `docs/` and the root (`packages/cli/README.md`, `bench/RESULTS.md`) still run everything.
- `vitest.config.ts` names three files. A renamed file loses its place at the front and nothing else changes; update the list when the longest files change.
- On Windows the tests no longer write under `C:\Users\runneradmin\AppData\Local\Temp`, whose 8.3 short form (`RUNNER~1`) the fixtures already resolve with `realpath`. The owner's machine has no short form in its temp path either.
