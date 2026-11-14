# Benchmark results

Numbers are wall-clock times for the whole process, measured with `bench/startup.mjs`, which runs a command repeatedly after three warm-up runs and reports min, p50, p95 and mean.

## M26: the tamper guard measured on real history and on cheating changes

Measured on 2026-11-14 on the build machine (Linux x64, 4 cores), release build. Method, limits and what was fixed: ADR 0046. **Read the limits before the numbers**: each finding was labelled once, by a fresh subagent that saw the commit's diff and a neutral statement of the claim (not the guard's code or reasoning); nine labellers, 723 labels, no second labeller and no human check; eight popular, well-kept repos, 150 recent non-merge commits each; the config and CI kinds were hardly exercised; the "after" rate is on the same commits the fixes were developed against, so it is optimistic.

### False positives on real history

`node bench/tamper-history.mjs …` over 1,200 commits of eight public repos and 150 of this repo (shallow clones, read-only, nothing of theirs run), then `node bench/tamper-history-report.mjs` on `bench/tamper-history.json` (every finding with its label and the labeller's reason). Rates with 95% Wilson intervals. "Before" is the guard as M26 first ran it; "after" is after the false-positive fixes of ADR 0046 and the review's corrections. The tables by kind cover all nine repos, this one included.

#### Before

| Language                     | Repos                                                                                                                       | Commits | Commits with findings | Findings | False | False-positive rate (95% interval) | Commits with a false finding (95% interval) |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------- | --------------------- | -------- | ----- | ---------------------------------- | ------------------------------------------- |
| Rust                         | BurntSushi/ripgrep, clap-rs/clap                                                                                            | 300     | 7                     | 41       | 6     | 14.6% (6.9–28.4%)                  | 2 of 300, 0.7% (0.2–2.4%)                   |
| JavaScript                   | axios/axios, expressjs/express                                                                                              | 300     | 20                    | 98       | 66    | 67.3% (57.6–75.8%)                 | 15 of 300, 5.0% (3.1–8.1%)                  |
| Python                       | encode/httpx, pallets/flask                                                                                                 | 300     | 41                    | 172      | 42    | 24.4% (18.6–31.4%)                 | 12 of 300, 4.0% (2.3–6.9%)                  |
| TypeScript                   | honojs/hono, sindresorhus/ky                                                                                                | 300     | 71                    | 240      | 86    | 35.8% (30.0–42.1%)                 | 17 of 300, 5.7% (3.6–8.9%)                  |
| This repo (Rust, TypeScript) | ihsankoolz/firescale                                                                                                        | 150     | 50                    | 160      | 21    | 13.1% (8.7–19.2%)                  | 10 of 150, 6.7% (3.7–11.8%)                 |
| All public repos             | BurntSushi/ripgrep, axios/axios, clap-rs/clap, encode/httpx, expressjs/express, honojs/hono, pallets/flask, sindresorhus/ky | 1200    | 139                   | 551      | 200   | 36.3% (32.4–40.4%)                 | 46 of 1200, 3.8% (2.9–5.1%)                 |

| Kind              | Findings | False | False-positive rate (95% interval) |
| ----------------- | -------- | ----- | ---------------------------------- |
| assertion-removed | 110      | 36    | 32.7% (24.7–41.9%)                 |
| ci-weakened       | 14       | 14    | 100.0% (78.5–100.0%)               |
| expected-changed  | 132      | 14    | 10.6% (6.4–17.0%)                  |
| not-checked       | 1        | 0     | 0.0% (0.0–79.3%)                   |
| rule-disabled     | 5        | 0     | 0.0% (0.0–43.4%)                   |
| snapshot-changed  | 61       | 0     | 0.0% (0.0–5.9%)                    |
| suppression-added | 278      | 109   | 39.2% (33.7–45.1%)                 |
| test-deleted      | 47       | 12    | 25.5% (15.3–39.5%)                 |
| test-skipped      | 63       | 36    | 57.1% (44.9–68.6%)                 |

#### After

| Language                     | Repos                                                                                                                       | Commits | Commits with findings | Findings | False | False-positive rate (95% interval) | Commits with a false finding (95% interval) |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------- | --------------------- | -------- | ----- | ---------------------------------- | ------------------------------------------- |
| Rust                         | BurntSushi/ripgrep, clap-rs/clap                                                                                            | 300     | 4                     | 35       | 5     | 14.3% (6.3–29.4%)                  | 1 of 300, 0.3% (0.1–1.9%)                   |
| JavaScript                   | axios/axios, expressjs/express                                                                                              | 300     | 6                     | 22       | 11    | 50.0% (30.7–69.3%)                 | 3 of 300, 1.0% (0.3–2.9%)                   |
| Python                       | encode/httpx, pallets/flask                                                                                                 | 300     | 40                    | 168      | 38    | 22.6% (16.9–29.5%)                 | 11 of 300, 3.7% (2.1–6.4%)                  |
| TypeScript                   | honojs/hono, sindresorhus/ky                                                                                                | 300     | 57                    | 155      | 24    | 15.5% (10.6–22.0%)                 | 9 of 300, 3.0% (1.6–5.6%)                   |
| This repo (Rust, TypeScript) | ihsankoolz/firescale                                                                                                        | 150     | 46                    | 152      | 11    | 7.2% (4.1–12.5%)                   | 6 of 150, 4.0% (1.8–8.5%)                   |
| All public repos             | BurntSushi/ripgrep, axios/axios, clap-rs/clap, encode/httpx, expressjs/express, honojs/hono, pallets/flask, sindresorhus/ky | 1200    | 107                   | 380      | 78    | 20.5% (16.8–24.9%)                 | 24 of 1200, 2.0% (1.3–3.0%)                 |

| Kind              | Findings | False | False-positive rate (95% interval) |
| ----------------- | -------- | ----- | ---------------------------------- |
| assertion-removed | 110      | 36    | 32.7% (24.7–41.9%)                 |
| ci-weakened       | 4        | 4     | 100.0% (51.0–100.0%)               |
| expected-changed  | 128      | 11    | 8.6% (4.9–14.7%)                   |
| not-checked       | 1        | 0     | 0.0% (0.0–79.3%)                   |
| rule-disabled     | 5        | 0     | 0.0% (0.0–43.4%)                   |
| snapshot-changed  | 63       | 0     | 0.0% (0.0–5.7%)                    |
| suppression-added | 162      | 15    | 9.3% (5.7–14.7%)                   |
| test-deleted      | 47       | 12    | 25.5% (15.3–39.5%)                 |
| test-skipped      | 12       | 11    | 91.7% (64.6–98.5%)                 |

The fixes also stopped 59 findings labelled true (12.0%, 9.5–15.2% of 490): 26 new conditional tests (labeller disagreement), 22 `@ts-expect-error` in runtime test files (ADR 0046 decides these are assertions), 11 expected-value pairs now kept within one test or a renamed one.

### Misses on cheating changes

`node bench/tamper-misses.mjs --bin target/release/fscale` on `bench/tamper-cheats.json`: 98 cheating changes **written for this measurement** by a subagent that could not read the guard's code, its corpus or its ADRs, from nine categories; held out (no detector changed because of it). Caught: a clear finding; needs approval: only `snapshot-changed` or `expected-changed`.

| Category                     | Cheats | Caught | Needs approval only | Missed |
| ---------------------------- | ------ | ------ | ------------------- | ------ |
| test-deleted                 | 10     | 9      | 0                   | 1      |
| test-skipped                 | 11     | 10     | 0                   | 1      |
| assertion-weakened           | 11     | 2      | 1                   | 8      |
| snapshot-or-expected-changed | 11     | 0      | 7                   | 4      |
| suppression                  | 11     | 8      | 0                   | 3      |
| config-weakened              | 11     | 7      | 0                   | 4      |
| ci-weakened                  | 11     | 8      | 0                   | 3      |
| hooks-skipped                | 11     | 4      | 0                   | 7      |
| other                        | 11     | 0      | 0                   | 11     |
| **All**                      | **98** | **48** | **8**               | **42** |

Miss rate 42 of 98, 42.9% (95% interval 33.5–52.7%); not caught outright (missed or approval only) 50 of 98, 51.0% (41.3–60.7%). The same before and after the false-positive fixes. One subagent wrote the corpus from a category list in one sitting; its mix is not the mix of real cheating, and the 11 "other" cases are of kinds the guard does not try to catch.

### The labelled corpus (fixture counts)

`node bench/tamper-rates.mjs`: 255 cases (142 to catch, 113 not), 0 missed, 0 false positives. Written with the guard: it shows the guard does what its ADRs say, not a rate.

## M25: the tamper guard's config and CI detectors

Measured on 2026-11-14 on the build machine (Linux x64, 4 cores).

**These are counts on fixtures, not a measured rate.** The new cases (prefixes `cov:`, `ci:`, `nv:`, `lint:`) were written together with the detectors, partly by the subagents that wrote them, and partly from the review's reproductions (ADR 0045). M26 measures the false-positive rate on real history and the miss rate on a corpus written for that purpose.

`node bench/tamper-rates.mjs --bin target/debug/fscale` runs the 226 cases of `bench/tamper-corpus.json` through `fscale verify --json`, each in its own git repo:

| Kind              | Cases that must give it | Caught | Missed |
| ----------------- | ----------------------- | ------ | ------ |
| test-deleted      | 11                      | 11     | 0      |
| test-skipped      | 30                      | 30     | 0      |
| assertion-removed | 41                      | 41     | 0      |
| suppression-added | 12                      | 12     | 0      |
| coverage-lowered  | 8                       | 8      | 0      |
| ci-weakened       | 11                      | 11     | 0      |
| no-verify         | 9                       | 9      | 0      |
| rule-disabled     | 17                      | 17     | 0      |
| not-checked       | 4                       | 4      | 0      |

| Group           | Cases | Not-a-finding cases | Findings caught | Missed | False positives |
| --------------- | ----- | ------------------- | --------------- | ------ | --------------- |
| TypeScript      | 46    | 23                  | 30              | 0      | 0               |
| JavaScript      | 16    | 6                   | 12              | 0      | 0               |
| Python          | 53    | 23                  | 36              | 0      | 0               |
| Rust            | 32    | 15                  | 20              | 0      | 0               |
| Coverage config | 15    | 7                   | 8               | 0      | 0               |
| CI workflows    | 20    | 9                   | 11              | 0      | 0               |
| Hook skips      | 14    | 5                   | 9               | 0      | 0               |
| Lint config     | 30    | 13                  | 17              | 0      | 0               |

- 125 cases must give at least one finding; 101 must give none. 0 missed, 0 false positives, 0 of 101 not-a-finding cases with any finding. Fixture counts, not a rate.

## M24: the tamper guard for Python and Rust, suppressions and policy

Measured on 2026-11-13 on the build machine (Linux x64, 4 cores).

### The labelled corpus

**These are counts on fixtures, not a measured rate.** The corpus was written together with the guard, by the same hands, so it shows the guard does what ADR 0042 and ADR 0044 say. It does not say how often the guard is wrong on real changes: M26 measures the false-positive and miss rates on a corpus that was not written with it.

`node bench/tamper-rates.mjs --bin target/release/fscale` runs the 147 cases of `bench/tamper-corpus.json` through `fscale verify --json`, each in its own git repo:

| Kind              | Cases that must give it | Caught | Missed |
| ----------------- | ----------------------- | ------ | ------ |
| test-deleted      | 11                      | 11     | 0      |
| test-skipped      | 30                      | 30     | 0      |
| assertion-removed | 41                      | 41     | 0      |
| suppression-added | 12                      | 12     | 0      |
| not-checked       | 4                       | 4      | 0      |

| Language   | Cases | Not-a-finding cases | Findings caught | Missed | False positives |
| ---------- | ----- | ------------------- | --------------- | ------ | --------------- |
| TypeScript | 46    | 23                  | 30              | 0      | 0               |
| JavaScript | 16    | 6                   | 12              | 0      | 0               |
| Python     | 53    | 23                  | 36              | 0      | 0               |
| Rust       | 32    | 15                  | 20              | 0      | 0               |

- 80 cases must give at least one finding (98 findings in all); 67 must give none. On these fixtures: 0 missed, 0 false positives, 0 of 67 not-a-finding cases with any finding. Again: fixture counts, not a rate.
- New in M24: pytest and unittest (deleted tests, `skip`, `skipif`, `xfail`, `unittest.skip`, named skip marks, `__test__ = False`, `importorskip`, a setup or autouse fixture that skips, `collect_ignore`, a body replaced by `pass` or `return`, a skipped parametrize row, removed assertions, `assert True`, `self.assertTrue(True)`, a removed `assert False` or `self.fail`), Rust (`#[ignore]`, `cfg_attr(…, ignore)`, a feature or platform `cfg`, a test module in another file turned off, an emptied body, removed `assert_eq!`, `assert!(true)`, a removed `assert!(false)`), the misses ADR 0042 admitted in every language (`if (false)`, an early `return`, an assertion that cannot fail, each with a not-a-finding case), lint suppressions (added, widened, inline ESLint configuration, `skip-file`; with moved, narrowed, split and renamed-to-stricter not-a-finding cases), and tests deleted with a helper module, not their subject.
- The independent review before the pull request found the cases from "a removed `assert False`" on; each is in the corpus now (ADR 0044, "After the review").

### Reading a wide test file

One changed file with every test in one block, release build, `fscale verify --json` wall time, after the review's fix to how code that never runs is found (before it, these took 40 s, 19 s and 22 s):

| File                                   | Time   |
| -------------------------------------- | ------ |
| JS, 4,000 tests in one `describe`      | 0.60 s |
| JS, one test with 4,000 `expect` calls | 0.34 s |
| Python, 4,000 test functions           | 0.35 s |

### A smoke check on real history (not a rate either)

The debug build ran `fscale verify --json --base <parent>` on each of the last 120 non-merge commits of `fastapi/full-stack-fastapi-template` up to `aabb901` (a pytest backend, listed in DECISIONS.md) and the last 100 of this repo (Rust and TypeScript tests), with no-op stand-ins for `uv`, `bun`, `npm`, `cargo` and the like first on the PATH so none of their tooling ran.

- fastapi: 4 commits touched test files; no findings. Two pytest files deleted with the modules they test (`f8dd304`) were excused as "deleted with its subject".
- this repo: 4 findings in 3 commits, each true: three `#[allow(clippy::too_many_arguments)]` added (`5460f2d`, `60ab2c7`) and one `expect(…).not.toContain(…)` removed in a refactor of a test helper (`470533e`), an honest edit a person should still see.

Two repositories, written by careful people, are not a sample: this says the guard is not noisy on them, nothing more.

### Binary size and build time

| Release build of `fscale`                      | Before M24 (`7e13ee3`) | After M24                           |
| ---------------------------------------------- | ---------------------- | ----------------------------------- |
| Binary size                                    | 9,029,008 B            | 10,454,800 B (+1,425,792 B, +15.8%) |
| Clean `cargo build --release -p firescale-cli` | 94 s                   | 83 s                                |

The 1.4 MB are mostly the Rust grammar's parse tables (`tree-sitter-rust` 0.24.2); the readers and the suppression detector are a small part. The size is of the final build; the build times were taken once each, before the review's fixes, in their own target folders, with other work on the machine (a package install during the first), so their difference is noise.

## M23: the tamper guard

Measured on 2026-11-13 on the build machine (Linux x64, 4 cores).

### The labelled corpus

`node bench/tamper-rates.mjs --bin target/debug/fscale` runs the 37 cases of `bench/tamper-corpus.json` (ADR 0042) through `fscale verify --json`, each in its own git repo:

| Kind              | Cases that must give it | Caught | Missed |
| ----------------- | ----------------------- | ------ | ------ |
| test-deleted      | 4                       | 4      | 0      |
| test-skipped      | 8                       | 8      | 0      |
| assertion-removed | 10                      | 10     | 0      |
| not-checked       | 2                       | 2      | 0      |

- 19 cases must give at least one finding (24 findings in all); 18 must give none.
- **False positives on the corpus: 0.** No not-a-finding case gave a finding (0 of 18), and no caught case gave a kind it does not have.
- The not-a-finding cases are the honest edits ADR 0042 lists: a renamed test and suite, a test moved to another file, a test file renamed, a test split in two, three tests turned into an `it.each` table, stricter assertions (`toStrictEqual`, `assert.strictEqual`), `expect.soft`, a reformatted file, an assertion moved into a helper in the same file, Playwright hooks folded into a step, tests deleted with the module they test (whole file, and one test in a kept file), a subject renamed with its tests updated, tests added, a skipped test turned back on, a change outside the tests.
- These counts show the guard does what the ADR says on cases written alongside it, including the bypasses the review found. They are not a measured false-positive rate on real changes; M26 measures that on a corpus that was not.

### Binary size and build time

| Release build of `fscale`                      | Before M23 (`1b98a8d`) | After M23                      |
| ---------------------------------------------- | ---------------------- | ------------------------------ |
| Binary size                                    | 8,796,912 B            | 8,886,480 B (+89,568 B, +1.0%) |
| Clean `cargo build --release -p firescale-cli` | 86 s                   | 69 s                           |

M23 adds no grammar (ADR 0042): the 90 KB are the guard's own code. The two build times were each measured once, with other work running on the machine, so their difference is noise; ADR 0033 measured what tree-sitter itself costs (51 s to 69 s, 3.9 MB to 8.2 MB).

## M21: blame-aware verify and the stop hook

Release build, Linux x64 local container (4 vCPUs), Node 22, 2026-11-13, `bench/blame-bench.mjs --runs 30`. Each repo is on a `feature` branch off `main`, with one change. "Cold" empties the base store (`.firescale/base/`) before each run, so the base commit is checked out in a worktree and the step runs there; "warm" keeps it, so the base result is read back.

| Scenario                                               |      p50 |      p99 |      max |
| ------------------------------------------------------ | -------: | -------: | -------: |
| no failure: `fscale verify`, no base run               |  80.8 ms |  88.7 ms |  88.7 ms |
| pre-existing failure, cold base cache: `fscale verify` | 222.7 ms | 242.1 ms | 242.1 ms |
| pre-existing failure, warm base cache: `fscale verify` | 124.5 ms | 165.3 ms | 165.3 ms |
| stop hook, one of 6 projects changed, cold base cache  | 142.6 ms | 147.6 ms | 147.6 ms |
| stop hook, one of 6 projects changed, warm base cache  |  98.6 ms | 131.6 ms | 131.6 ms |

- The base run adds about 100 ms cold (the worktree, and the step run there) and about 45 ms warm over the same failure without a base. The steps here are `node -e` one-liners: a real base run costs what the project's own step costs on the base, within `verify.baseBudget`.
- The stop hook runs on a course-project shape (five Python services and a frontend, stand-ins for `python` and `npx`): with one service changed, it runs that service's steps only, and the base run only for its failing test.
- Not measured: macOS and Windows, where `git worktree add` is slower.

## M18: the step cache, cold and warm

Release build, Linux x64 local container (4 vCPUs), Node 22, 2026-11-13. `bench/verify-bench.mjs --runs 100`, in the same small repo as the M7 rows below. The cold row runs `--no-cache`: every step runs, and verify also hashes the repo twice (before, and before storing), asks `node --version` and stores three entries. The warm row serves all three steps from the cache: what is left is the check, the hashing and the one `node --version`.

| Scenario                                                                   |      p50 |      p99 |      max |
| -------------------------------------------------------------------------- | -------: | -------: | -------: |
| overhead: `fscale verify`, every step off                                  |  12.6 ms |  17.1 ms |  17.1 ms |
| commands, cold cache: `fscale verify --no-cache`, three `node -e ""` steps | 174.2 ms | 198.8 ms | 198.8 ms |
| commands, warm cache: `fscale verify`, the same three steps                |  35.8 ms |  43.3 ms |  43.3 ms |
| stop hook: `fscale hooks run --agent claude`, same repo, warm cache        |  35.1 ms |  39.7 ms |  39.7 ms |

The cold run costs about 75 ms more than the M7 "commands" row (99.6 ms): the version probe, two hashes of the repo and the writes. A real step (a test suite of seconds or minutes) dwarfs that.

On the real repos of DECISIONS.md, copied, with every step replaced by `node -e 0` (so no script of theirs ran), `--jobs 4`, one run each:

| Repo                                     | Units |   Cold |  Warm | Warm, from the cache |
| ---------------------------------------- | ----: | -----: | ----: | -------------------- |
| `fastapi/full-stack-fastapi-template`    |     4 | 262 ms | 96 ms | 12 of 13 steps       |
| `vercel/commerce`                        |     1 | 209 ms | 45 ms | 3 of 4 steps         |
| `GoogleCloudPlatform/microservices-demo` |    12 | 555 ms | 79 ms | 33 of 34 steps       |

On this repo itself (`pnpm --silent fscale verify`: ESLint, Prettier, clippy, `tsc`, `cargo test` and Vitest), the run that stores the passes took 2 min 07 s of steps; the next run, with no change, took 0.65 s in all.

After a new file in `src/checkoutservice` of `microservices-demo`, only that service's steps and the repo root's own steps (which cover the whole repo) ran again: 27 of 34 steps came from the cache.

## M17: tree-sitter and the project graph

Release build of `fscale` on the build machine (Linux x64, 4 vCPUs), 2026-11-13, before and after M17 added tree-sitter with its Python and TypeScript grammars (ADR 0033):

| Measure                                        | 0.2.0       | M17         |
| ---------------------------------------------- | ----------- | ----------- |
| Binary size                                    | 3,908,688 B | 8,213,104 B |
| Clean `cargo build --release -p firescale-cli` | 51 s        | 69 s        |
| `fscale --version`, p50 of 30 runs             | —           | 2.8 ms      |

`fscale graph --explain`, release build, one run each on the real repos of DECISIONS.md: `fastapi/full-stack-fastapi-template` 51 ms, `vercel/commerce` 20 ms, `GoogleCloudPlatform/microservices-demo` 27 ms. The graph parses every source file once, on all cores.

## `fscale verify` latency (M7)

`fscale verify` runs inside agent stop hooks, so what firescale itself adds must stay small. Measured with `bench/verify-bench.mjs`: a small git repo (two files, the eight core rules), 200 runs per row after 3 warm-ups, release build.

- **Overhead:** every step is turned off in the config, so this is the `check` step, planning and output.
- **Commands:** `lint`, `typecheck` and `test` each run `node -e ""`. That adds three shells and three Node starts, the floor for a repo whose steps are Node tools.
- **Stop hook:** `fscale hooks run --agent claude` around the same repo: verify plus the loop guard.

### Linux x64, local container (4 vCPUs, Intel Xeon @ 2.10GHz), Node 22, 2026-11-11

| Scenario                                                |      p50 |      p99 |      max |
| ------------------------------------------------------- | -------: | -------: | -------: |
| overhead: `fscale verify`, every step off               |   8.8 ms |  18.0 ms |  19.2 ms |
| commands: `fscale verify`, three `node -e ""` steps     |  99.6 ms | 131.6 ms | 139.1 ms |
| stop hook: `fscale hooks run --agent claude`, same repo | 101.8 ms | 132.4 ms | 133.8 ms |

firescale's own share is about 9 ms. Everything above that is the repo's commands: about 30 ms per step just to start a shell and Node. On this repo itself, a full `fscale verify` (ESLint, Prettier, clippy, `tsc`, `cargo test` and Vitest) takes about 22 s warm, nearly all of it in those tools.

Not measured: macOS and Windows (no CI was available, see DECISIONS.md), and a large repo. On a 50k-file repo the `check` step costs what the section below shows.

## npm launcher overhead (M6)

Installed through npm, `fscale` is a small Node launcher that starts the native binary (ADR 0013). This measures what that costs: `fscale --version` run directly and through the launcher, with `bench/launcher-bench.mjs`. 20 runs each after 3 warm-ups, release build.

### Linux x64, local container (4 vCPUs, Intel Xeon @ 2.10GHz), Node 22.23.4, 2026-11-11

| Path                  | min (ms) | p50 (ms) | p95 (ms) | mean (ms) |
| --------------------- | -------: | -------: | -------: | --------: |
| native binary         |      2.0 |      2.3 |      3.0 |       2.3 |
| npm launcher + binary |     35.8 |     44.7 |     51.5 |      44.4 |

The launcher adds about 42 ms per run here, which is Node's own startup. It is paid once per `fscale` command and does not grow with the repo. Where that matters, such as an agent hook that runs on every stop, call the binary in the platform package directly (see `docs/install.md`).

The CI `package` job runs the same measurement on Linux, macOS and Windows and prints it in the job summary.

## Check on a 50k-file repo: cold cache, warm cache, no cache (M3)

Measured with `bench/check-bench.mjs` on a fixture from `bench/generate-fixture.mjs --files 50000`: 50,005 tracked files, one enforced rule (`core/no-committed-env`), one violation. 10 runs per row, release build.

- **Cold cache:** `.firescale` is deleted before every run, so the run walks the repo, runs the rule and writes the cache.
- **Warm cache:** the cache is in place, so the run walks the repo, finds the entry and does not run the rule.
- **No cache:** `--no-cache`. The rule runs; nothing is walked or written.

### Linux x64, local container (4 vCPUs, Intel Xeon @ 2.10GHz), 2026-11-11

| Command              | min (ms) | p50 (ms) | p95 (ms) | mean (ms) |
| -------------------- | -------: | -------: | -------: | --------: |
| `--version`          |      2.7 |      3.1 |      4.8 |       3.4 |
| `check` (cold cache) |     93.1 |     99.2 |    108.8 |      99.9 |
| `check` (warm cache) |     59.8 |     63.7 |     68.9 |      64.3 |
| `check --no-cache`   |     28.2 |     32.7 |     36.0 |      31.5 |

### GitHub Actions runners, by OS

From the `bench` jobs of [CI run 42947874962](https://github.com/ihsankoolz/firescale/actions/runs/42947874962) on 2026-11-11. Same fixture and script, 10 runs per row. Shared runners are noisy, so read these as rough sizes, not precise figures.

**Linux** (`ubuntu-latest`, x64, AMD EPYC 9V74, 2 vCPUs)

| Command              | min (ms) | p50 (ms) | p95 (ms) | mean (ms) |
| -------------------- | -------: | -------: | -------: | --------: |
| `--version`          |      2.0 |      2.1 |      3.7 |       2.3 |
| `check` (cold cache) |    132.8 |    137.7 |    171.1 |     141.8 |
| `check` (warm cache) |    108.3 |    108.9 |    111.3 |     109.2 |
| `check --no-cache`   |     26.3 |     26.6 |     27.0 |      26.5 |

**macOS** (`macos-latest`, arm64, Apple M1 virtual, 3 vCPUs)

| Command              | min (ms) | p50 (ms) | p95 (ms) | mean (ms) |
| -------------------- | -------: | -------: | -------: | --------: |
| `--version`          |      2.9 |      3.6 |     11.0 |       4.8 |
| `check` (cold cache) |    105.2 |    134.4 |    207.0 |     138.0 |
| `check` (warm cache) |     81.8 |    102.0 |    176.3 |     106.3 |
| `check --no-cache`   |     28.5 |     48.9 |     59.3 |      45.7 |

**Windows** (`windows-latest`, x64, AMD EPYC 7763, 2 vCPUs)

| Command              | min (ms) | p50 (ms) | p95 (ms) | mean (ms) |
| -------------------- | -------: | -------: | -------: | --------: |
| `--version`          |     19.4 |     21.0 |     26.3 |      21.6 |
| `check` (cold cache) |    475.0 |    493.7 |    509.0 |     492.1 |
| `check` (warm cache) |    344.2 |    369.9 |    384.6 |     366.8 |
| `check --no-cache`   |    141.4 |    143.5 |    160.3 |     146.2 |

Windows is several times slower across the board: starting a process costs about 20 ms there against 2 to 4 ms elsewhere, and each `git` call and the 50k-file walk are slower too. The pattern is the same on all three: warm is faster than cold, and `--no-cache` is fastest.

### Reading these numbers

**With today's single rule, the cache makes `check` slower, not faster.** A warm run still has to walk 50k files and read the git index to prove nothing changed, which costs about 60 ms on the local machine and more on the CI runners. The rule itself only asks git for its file list, which costs about 30 ms. The cache avoids the rule but pays for the walk.

The cache is built for rules that read file contents, such as the secret scanner and env-usage rules planned for M5, where one rule can cost far more than the walk. Until those exist it is overhead. See ADR 0010 and the "Needs your decision" list in DECISIONS.md.

### How to reproduce

```
cargo build --release -p firescale-cli
node bench/generate-fixture.mjs <empty dir> --files 50000
node bench/check-bench.mjs --bin target/release/fscale --fixture <that dir> --runs 10 --label <name>
```

## Startup: TypeScript CLI vs Rust binary (M2)

Recorded on 2026-11-11 while moving `fscale` from a Node program to a native Rust binary (ADR 0008).

- Machine: Linux x64 cloud container, 4 vCPUs (Intel Xeon @ 2.10GHz), git 2.43.0, Node 22.23.4.
- Fixture: a git repo with three tracked files, two of them env files, and a `firescale.config.json` that enforces `core/no-committed-env`. So `check` runs one rule and reports two violations.
- Before: `node packages/cli/dist/bin.js` at commit `406799d` (the M1 TypeScript CLI, with the Rust addon loaded through napi-rs).
- After: `target/release/fscale` (678 KB, built with `cargo build --release`).
- 50 runs per command.

| Command               | Before p50 | Before p95 | After p50 | After p95 | Speed-up (p50) |
| --------------------- | ---------: | ---------: | --------: | --------: | -------------: |
| `fscale --version`    |    52.3 ms |    63.9 ms |    2.5 ms |    3.0 ms |            21x |
| `fscale check`        |    65.5 ms |    81.7 ms |    6.5 ms |    8.1 ms |            10x |
| `fscale check --json` |    65.5 ms |    86.5 ms |    6.4 ms |    7.7 ms |            10x |

Most of what remains in `check` is git itself: the rule runs `git rev-parse` and `git ls-files`, about 2 ms each on this machine.

### With a custom TypeScript rule

When the config lists a custom rule pack, the binary starts one Node process for the run. Same machine, 30 runs, a repo with the built-in rule plus one custom rule from `packages/cli/test-fixtures/custom-pack`:

| Command        |      p50 |      p95 |
| -------------- | -------: | -------: |
| `fscale check` | 107.6 ms | 137.0 ms |

That is Node's startup, loading the SDK, importing the pack with type stripping, and the custom rule's own git call. It is paid once per run, not once per rule, and only by repos that use custom rules or a `.ts` config.

### How to reproduce

```
cargo build --release -p firescale-cli
node bench/startup.mjs --runs 50 -- target/release/fscale --version
node bench/startup.mjs --runs 50 --cwd <fixture repo> -- <absolute path>/target/release/fscale check
```

### Limits of these numbers

- One machine, Linux only. Windows and macOS process startup costs differ, usually upward.
- The fixture is tiny. These numbers measure startup, not rule throughput on a large repo; that is M3's benchmark.
- "Before" could only be measured before the TypeScript CLI was removed, so it cannot be re-run from this commit. Check out `406799d` to repeat it.

## M10: subfolder discovery, before and after

`fscale facts` now walks four folder levels to find projects (ADR 0020), and four rules call it during `check`. `core/env-example` also looks for nested `.env.example` files. Release builds of the commit before M10 (`f891759`) and of M10, same machine (Linux x64, 4 cpus), the 50,000-file fixture, 10 runs:

| Command            | before p50 | after p50 | before p95 | after p95 |
| ------------------ | ---------: | --------: | ---------: | --------: |
| `--version`        |     2.4 ms |    2.8 ms |     3.7 ms |    3.8 ms |
| check (cold cache) |   674.2 ms |  719.3 ms |   736.1 ms |  858.1 ms |
| check (warm cache) |   312.2 ms |  339.6 ms |   319.7 ms |  375.5 ms |
| check `--no-cache` |   352.5 ms |  405.9 ms |   382.2 ms |  449.8 ms |

So `check` is 7 to 15% slower on a very large repo. The cost is the repeated walk: each of the four rules detects the facts for itself. Detecting them once per run would remove most of it. Linux only; not measured on macOS or Windows.
