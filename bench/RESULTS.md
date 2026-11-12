# Benchmark results

Numbers are wall-clock times for the whole process, measured with `bench/startup.mjs`, which runs a command repeatedly after three warm-up runs and reports min, p50, p95 and mean.

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
