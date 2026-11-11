# Benchmark results

Numbers are wall-clock times for the whole process, measured with `bench/startup.mjs`, which runs a command repeatedly after three warm-up runs and reports min, p50, p95 and mean.

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
