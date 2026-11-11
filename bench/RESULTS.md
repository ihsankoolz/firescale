# Benchmark results

Numbers are wall-clock times for the whole process, measured with `bench/startup.mjs`, which runs a command repeatedly after three warm-up runs and reports min, p50, p95 and mean.

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

CI_RESULTS_PLACEHOLDER

### Reading these numbers

**With today's single rule, the cache makes `check` slower, not faster.** A warm run still has to walk 50k files and read the git index to prove nothing changed, which costs about 60 ms here. The rule itself only asks git for its file list, which costs about 30 ms. The cache avoids the rule but pays for the walk.

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
