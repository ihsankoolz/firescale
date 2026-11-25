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

**For each folder of the head that holds a `Cargo.toml` git tracks, the base worktree's `target` is a link to a build folder of its own, kept between base runs, in `firescale-base-target/<repo key>/` in the temp folder.** Cargo builds into the link as it would into `target/`.

- **One kept folder per folder with a `Cargo.toml`** (named by a hash of its path), so two workspaces of one repo never share a `debug/` folder. A workspace member's link is never used: cargo builds a member in its workspace's folder.
- **A step that runs a built binary by its path finds it.** This repo's Vitest suite runs `target/debug/fscale`.
- **Base steps never get `CARGO_TARGET_DIR`.** One the environment sets is taken away for base steps, so a base build never goes into a folder the environment names, which may be the head's. Head steps are as before.
- **One base run of a repo at a time.** A base run takes a lock on the repo's kept folders (`File::try_lock`, let go when the run ends or dies) and holds it until its worktree is removed. Another base run of the same repo at that moment builds from nothing in its own worktree, as before, and its progress says so. Otherwise one run's step could run a binary the other built from another base commit, and keep that result.
- **A kept base result's key gains `cargo-target=<folders>`.** This only adds to the key: a result kept before this change is not reused once.
- The links are removed with the worktree, and by the sweep of left-behind worktrees; the kept folders stay.
- A repo with no tracked `Cargo.toml` is as before, but for `CARGO_TARGET_DIR`, which no base step gets.

`base_cargo_target`, `kept_target`, `cargo_folders` and `lock_kept_targets` in `crates/core/src/blame.rs` (set up in `prepare_worktree`, key in `compare`); `environment` in `crates/core/src/verify.rs`.

**Decided on your behalf:** the brief named `CARGO_TARGET_DIR`. The first form set it, to one kept folder. The fresh review showed that one folder for every cargo invocation breaks a crate below the root that is a workspace of its own (its `target/debug/<name>` is missing on the base), and lets two workspaces overwrite each other's binaries. Links per folder keep what the variable was for, with neither problem.

After the change, the build-only step again: 14.4 s with no kept folder, then 11.0 s and 11.3 s.

## Consequences

- A Rust repo's base runs reuse their dependencies' builds. The repo's own crates are built again each time.
- Kept folders in the temp folder, about 2 GB for this repo. Not pruned; the OS's temp cleanup or a person deletes them.

## Needs your decision

- **A step that reads `target/` without building could see an older base run's build.** Before, a new worktree had no `target/`, so such a step failed on the base with a missing file, and the head's failure blocked. Now the link holds whatever the last base run of this repo built, from another base commit. If that old build happens to fail the same way as the head, the head's failure would read `pre-existing`. A step that builds first (`cargo test`, `cargo build` in `verify.baseInstall`, as this repo does) brings the files up to date, so it is not affected. Closing it fully means no link (which breaks this repo's own base runs) or emptying the kept binaries before each base run. I left it as is and logged it here.

## Not done

- No pruning, and no size limit.
- Only cargo. Other build folders (Gradle, `dist/`) are not kept.
- The repo's own crates rebuild, because cargo compares file times. Cargo's unstable `-Zchecksum-freshness` would avoid that; it needs a nightly toolchain.
- A tracked `Cargo.toml` in a test fixture's folder gets a `target` link in the base worktree too; a test that lists that folder sees it there and not on the head.
- A folder that has a `Cargo.toml` at the head and not at the base gets no link (the folder is checked first); one created by a base step later builds in the worktree.

## From the fresh review

It found two ways the first form could turn a head failure into `pre-existing`: a standalone crate below the root had no `target` on the base, and two base runs at once shared one folder, so one could run the other's binary and keep that result. Both are fixed as above, with tests: `gives a crate below the root that is a workspace of its own a folder of its own` (seen failing on the first reworked form, which took the folders from the plan: a top-level step leaves the plan with no projects), and `only_one_base_run_at_a_time_takes_the_kept_rust_build_folders` and `each_folder_with_a_cargo_toml_has_a_kept_folder_of_its_own` in `blame.rs`, written with the fix. Also from it: the sweep of left-behind worktrees now removes `target` links too. It confirmed head steps never got the variable, the key only adds, and the links are removed without touching what they point to, on Unix and on Windows.

## Tests

`packages/cli/src/rust-base-target.test.ts`, written first: 3 of 4 failed on `main` (`expected null not to be null`). The fourth, a repo with no Rust, is a guard that passed before and after. After the review the cases were rewritten for the links: the worktree's `target` is a folder in the temp folder, the same in two runs, with what the first left there seen by the second, and no `CARGO_TARGET_DIR`; with the head's `target/` named in `CARGO_TARGET_DIR`, the base does not use it, the head's steps keep it, and nothing is written there; a crate below the root that is a workspace of its own gets a folder of its own; two repos get two folders; a repo with no Rust is as before.
