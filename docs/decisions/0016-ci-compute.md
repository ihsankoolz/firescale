# 0016 — CI: Linux on every push, macOS and Windows on pull requests into main

Status: accepted (2026-11-11). Supersedes the M6 amendment of ADR 0009. The action pinning in ADR 0009 is unchanged.

(Number 0015 is taken by the direction ADR, which is in its own pull request.)

## Context

CI used too much GitHub Actions compute. The account reached its monthly limit during M6, and M7's first runs could not start. After the M6 amendment of ADR 0009, each push to a pull request still ran four jobs, two of them on macOS and Windows, whose minutes are billed at a multiple of Linux minutes. A second push did not always stop the run of the first.

## Decision

| Event                          | What runs                                                  |
| ------------------------------ | ---------------------------------------------------------- |
| Push to any branch             | Linux, on Node 22 and Node 24                              |
| Pull request that targets main | macOS and Windows, on Node 24                              |
| Started by hand                | All four of the above, plus the `bench` and `package` jobs |

- **Linux runs on every push**, to any branch. It is the cheapest runner and catches most failures.
- **macOS and Windows run only for pull requests into `main`, and for manual runs.** A pull request into another branch (a stacked pull request) gets Linux only, until it is pointed at `main`.
- **Both Node versions are tested on Linux only.** macOS and Windows use Node 24. Node only affects the TypeScript packages and the bridge.
- **No job runs twice for one commit.** A pull request from this repository already has a push run for its head commit, so the pull request event adds only macOS and Windows. A pull request from a fork has no push run here, so it gets Linux from the pull request event as well.
- **Benchmarks run only when the workflow is started by hand.** That covers the `bench` job and the `package` job (npm install smoke tests, which end with the launcher benchmark and run on three OSes). The `ci:bench` and `ci:package` labels no longer do anything.
- **A new push cancels the run in progress for the same branch**, and a new push to a pull request cancels that pull request's run in progress. Push runs and pull request runs are separate groups and never cancel each other. This also applies to `main`: if two merges land close together, only the later one finishes.
- **Caching is kept**: the pnpm store through `actions/setup-node`, and the cargo registry and `target` through `Swatinem/rust-cache`.
- The steps are written out in both jobs instead of being shared through a local action. A shared action would have to name a shell for every step, which would move the Windows job from PowerShell to bash and change what it tests.
- A pushed version tag starts only the Release workflow, not CI.

## Consequences

- A branch that is pushed several times before its pull request is opened costs two Linux jobs per push and nothing else.
- A pull request into `main` shows four checks, as before, with the same names (`ubuntu-latest / node 22`, `ubuntu-latest / node 24`, `macos-latest / node 24`, `windows-latest / node 24`). Two come from the push run and two from the pull request run.
- **A commit on `main` is tested on Linux only.** macOS and Windows were tested on the pull request's merge result before it merged. If two pull requests that pass alone break each other on macOS or Windows, the next pull request shows it, or a manual run does.
- A change to the npm packaging or to performance is checked only when someone starts the workflow by hand. Do that before a release.
- The two job step lists must be kept identical by hand.
- Each OS keeps its own caches. A new branch or pull request starts from the caches saved on `main`.
