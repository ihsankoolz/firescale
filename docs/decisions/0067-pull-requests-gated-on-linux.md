# 0067 — Pull requests are gated on Linux; macOS and Windows run at night and on release

Status: accepted (2026-11-16, 0.5.2 run; the owner's decision). Amends ADR 0016 (what runs when) and ADR 0066 (this repo's CI).

## Context

Since ADR 0016, every pull request into `main` ran the tests on macOS and Windows as well as Linux. ADR 0066 measured the cost: the Windows job took 21 to 34 minutes against about 4.5 on Linux, and it was the slowest step between a pushed fix and a merge. In the 0.5.2 run the owner asked for the run to finish faster and decided that pull requests wait for Linux only.

## Decision

`.github/workflows/ci.yml`:

- **A pull request is gated on Linux only.** A push runs Linux on Node 22 and 24, as before; the pull request run of the same commit no longer runs macOS or Windows. `tests (push)` and `tests (pull request)` stay the checks to require (ADR 0066).
- **macOS and Windows run** (the `desktop` job, Node 24):
  - every night on `main` (`schedule`, 18:17 UTC, 02:17 in the owner's time zone), with the Linux jobs; the `tests` job is then named `tests (nightly)`;
  - when the workflow is started by hand (`workflow_dispatch`), as before, with the benchmark and package jobs;
  - on a pull request with the label `release`. The pull request run also starts on `labeled`, so adding the label to an open pull request starts them without a new push. The version pull request of each release carries the label.
- A scheduled run counts as "every job runs" in the `changes` job, like a manual one: nothing is skipped as docs only.
- Nothing else changes: the docs-only rule, the `tests` job, the concurrency groups of ADR 0066.

`.github/workflows/tamper-bench.yml` (new, started by hand only): it builds fscale (release) from two refs, `before` and `after`, clones each repo of `bench/tamper-history.json` and `bench/tamper-history-m32.json` at the head recorded there (shallow, read-only), runs `bench/tamper-history.mjs` of `after` with both binaries over the same commits, one Linux job per repo, and uploads the two JSON Lines files of each repo. It measures the tamper guard on real commits off this machine, where the 0.5.2 run could not finish it (bench/RESULTS.md). It labels nothing: the files are compared with the labels already recorded, and new findings are labelled by hand or by a fresh labeller as in ADR 0046.

## Consequences

- **A bug that shows only on macOS or Windows is found at night, or on the release pull request, not on the pull request that brings it.** By then the change is merged, and other changes may be merged on top of it. The fix is a new pull request, found from the nightly run's failure. Windows has had the most such bugs (paths, `cmd`, CRLF, file locks, timing), so this is a real cost, accepted for speed.
- A pull request is mergeable after about 5 minutes of Linux instead of 20 to 35 minutes of Windows, and uses fewer of the owner's Actions minutes.
- The nightly run spends macOS and Windows minutes on `main` every night, even when nothing changed. A scheduled workflow of a repository with no activity for 60 days is turned off by GitHub.
- A pull request from a fork gets Linux from its pull request run, as before, and macOS and Windows only with the label.
- Adding any other label also starts a pull request run; it runs only the `changes`, `docs` (for forks) and `tests` jobs, and cancels a pull request run in progress for the same pull request.
- No test covers this repo's workflow (as in ADR 0066). The runs show it: this change's own pull request run skips macOS and Windows; the version pull request, with the label, runs them.
