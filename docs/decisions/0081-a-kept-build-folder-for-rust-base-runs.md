# 0081 — A kept build folder for Rust base runs

Status: accepted (2026-11-25, Track A, item 12; the owner's brief). Adds to ADR 0037 (the base run) and ADR 0043 (what the base worktree gets).

## Context

`docs/research/research-build-tools.md`, idea 10: "measure first". Each base run is made in a new worktree in the temp folder (`firescale-base-<pid>-<nanos>`). Nothing in the repo named `CARGO_TARGET_DIR` or `target/`, so a Rust base step built into the new worktree's `target/`. The brief: time a Rust base run on this repo twice, write both numbers here, and build a kept folder only if the second run rebuilds from scratch.

## The measurement

On the owner's laptop (macOS, Apple silicon), in a scratch clone of this repo at `4252be3`. Its only step was `cargo test --workspace`, and the head added one failing Rust test, so the step ran on the base. `.firescale/` was deleted before each run so the base ran again (`--no-cache`).

| Run                        | Base step `cargo test --workspace` |
| -------------------------- | ---------------------------------- |
| First, before this change  | **52.7 s**                         |
| Second, before this change | **59.1 s**                         |

**The second run rebuilt from scratch**: a new worktree has an empty `target/`. So the kept folder below was built. Most of each number is the tests running, not the build: the head's own build from nothing took 12.8 s (60 s of CPU).

After the change, the same step: 1m 00s, then 53.2 s, with the test run dominating as before. To isolate the build, a step that only compiles (`cargo test --workspace --no-run`, then a filter that matches no test on the base):

| Base step that only builds | With the kept folder emptied first (as before) | With the kept folder |
| -------------------------- | ---------------------------------------------- | -------------------- |
| First pair                 | 13.8 s                                         | 12.8 s               |
| Second pair                | 13.8 s                                         | 11.3 s               |

`cargo -v` in a new worktree with the kept folder: 57 of 61 units `Fresh`; the 4 crates of the workspace itself are built again. Their files are newer than the kept build, since the worktree was just checked out. On this machine those 4 are most of the build, so the gain here is 1 to 2.5 s. A repo with more or heavier dependencies than its own code (or a slower machine, or CI) gains more. Dependencies are reused across worktree paths, which the research report left unverified.

## Decision

**When the repo root or a Rust project of the plan has a `Cargo.toml`, base steps build into one folder per repo that is kept between base runs: `firescale-base-target/<repo key>` in the temp folder.**

- Base steps (and `verify.baseInstall`, which runs as one) get `CARGO_TARGET_DIR` set to it. The repo key is the one the run records use (`repo_key` in `runs.rs`, a hash of the repo root's real path).
- **The worktree's `target` is linked to it**, at the root and at each Rust project folder that has a `Cargo.toml` and is not below another such folder: where cargo would put `target` itself. Without the link, a test that runs a built binary by its path breaks on the base. This repo's own Vitest suite runs `target/debug/fscale`. The link is removed with the worktree; the folder stays.
- **Never the head's `target/`.** Head steps get no `CARGO_TARGET_DIR` from firescale, and a `CARGO_TARGET_DIR` already in the environment is replaced for base steps only.
- **A kept base result's key gains `cargo-target=<folders>`** for such a repo. This only adds to the key: a result kept before this change is not reused once.
- A repo with no Rust is as before: no folder, no variable, no link.

`base_cargo_target` and `cargo_folders` in `crates/core/src/blame.rs` (set up in `prepare_worktree`, key in `compare`); `environment` in `crates/core/src/verify.rs`.

## Consequences

- A Rust repo's base runs reuse their dependencies' builds. The repo's own crates are built again each time.
- One folder per repo in the temp folder, about 2 GB for this repo. Not pruned; the OS's temp cleanup or a person deletes it.
- Two verify runs of one repo at once share the folder. Cargo locks it, so the second base build waits for the first, within its base budget.

## Needs your decision

- **A step that reads `target/` without building could see an older base run's build.** Before, a new worktree had no `target/`, so such a step failed on the base with a missing file, and the head's failure blocked. Now the link holds whatever the last base run of this repo built, from another base commit. If that old build happens to fail the same way as the head, the head's failure would read `pre-existing`. A step that builds first (`cargo test`, `cargo build` in `verify.baseInstall`, as this repo does) brings the files up to date, so it is not affected. Closing it fully means no link (which breaks this repo's own base runs) or emptying the kept binaries before each base run. I left it as is and logged it here.

## Not done

- No pruning, and no size limit.
- Only cargo. Other build folders (Gradle, `dist/`) are not kept.
- The repo's own crates rebuild, because cargo compares file times. Cargo's unstable `-Zchecksum-freshness` would avoid that; it needs a nightly toolchain.
- The folder is not keyed by toolchain. Cargo keeps builds of different `rustc` versions apart in one folder by itself.

## Tests

`packages/cli/src/rust-base-target.test.ts`, written first: 3 of 4 failed on `main` (`expected null not to be null`: no `CARGO_TARGET_DIR`). The fourth, a repo with no Rust, is a guard that passed before and after. The cases: base steps get one folder in the temp folder, the same in two runs, with what the first left there seen by the second, and `target` resolving to it; the head's steps get no variable and its `target/` is untouched; two repos get two folders.
