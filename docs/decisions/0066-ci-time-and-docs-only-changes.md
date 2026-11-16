# 0066 — This repo's CI: where the Windows time goes, and changes that touch only docs

Status: proposed (2026-11-16, 0.5.2 run, pull request 5). Builds on ADR 0016 (what runs when) and ADR 0009 (actions pinned to commit SHAs). Not about the workflow `fscale init` writes (ADR 0041, `docs/ci.md`).

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

The first two were fixed at the cause in the 0.5.1 run: the verify test now builds one fixture instead of seven (8531c25), and the protected-glob test runs its eleven cases side by side (5e6a3cc). The third is not fixed. It starts `fscale verify` five times on the six-project course fixture, three of them running every project's steps through `.cmd` stand-ins. Its siblings of the same shape are close to the limit too: "runs every step again, and every project under --affected, when <setting> changes" (seven cases, four verify runs each) took 12.8 to 30.0 s in the same five runs, with 30.0 s (`verify.failOnSkipped`) and 28.5 s (`verify.stopWhenServiceDown`) in the slow attempt. That attempt was slow as a whole: its Vitest took 1611 s against 1277 s for the next attempt on the same commit.

Older timeouts of the same kind were given limits of their own before (`scaffold` 60 s, `bridge` 60 s, `step-cache` 90 s, `agent-files` 180 s); `scaffold` "--check exits 1 after a generated file is deleted" passed its 60 s once since (61.6 s in run 43612243158 attempt 1, 2026-11-14).

## Decision

(Written in the next commit, once the numbers above are in.)
