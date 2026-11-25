# 0090 — Old tests within their limits: the hook's time, one budget, no pass shown, sure files only

Status: accepted (2026-11-25, Track B run). Amends ADR 0089 (old tests, new code) and ADR 0083 (the stop hook's time limit). The owner's decisions on four items of ADR 0089's "Not done".

## Context

ADR 0089's review left four things open: the worktree's making and removal were not timed, so the stop hook could still pass its limit; the old-tests run had a `verify.baseBudget` of its own after the base run's; a run with no failure was shown as `passed`, which reads as reassurance it is not; and `cargo test` names a binary, which could be pinned on the wrong file. The owner decided each.

## Decision

- **The stop hook's limit.** Making and removing the worktree are timed together and kept beside the step times (`.firescale/step-times.json`, step `old tests: worktree`). In the stop hook nothing starts unless the worktree and every test step to run have a kept time and together fit before the hook's limit (its `timeout` less 30 seconds, ADR 0089). With no kept time: `not judged: no time is kept yet for this run …`. A verify outside the hook keeps the times. Each step's own deadline is the limit less the kept time of removing the worktree. A hook that Claude Code ends lets the agent stop unverified, so it must not risk that.
- **One budget.** The base run records when `verify.baseBudget` ends for this verify (`BaseReport::budget_ends`, not in JSON). The old-tests run uses that same end: when it has run out, `not judged: verify.baseBudget, which the base run shares, has run out`. Where there was no base run (`fix --agent`'s referee), the budget starts with the old-tests run.
- **No pass shown.** A run with no failing test of the base's files adds nothing to the normal output. `--json` says `ran-no-failure` (it replaces `passed`), with a detail that it shows nothing about the change. If the runner's results show none of the base's tests of those files ran, it is `not judged`. For this the runners' results now name what ran and did not fail (`TestResults::ran`): passing test ids for pytest (JUnit) and `cargo test`, files for Vitest (`✓ file`) and Jest (`PASS file`).
- **Sure files only.** A failing test is tied to a file only when no other file could hold it (`oldtests::tie`). Pytest, Vitest and Jest name the file. For `cargo test`, which names a binary: when a crate below the unit has a `[package]`, `[lib]` or `[[test]]` of that name, or more than one `tests/<name>.rs` or `tests/<name>/main.rs` could build it, it is not tied. It is listed under each finding whose file it could be from, naming no file: `a test failed on the head's code with the base's test files, but it cannot be tied to one file for sure (<why>): <id>`. The run's detail counts those apart.

## Consequences

- The stop hook runs old tests only after one verify outside the hook has kept their times. The first hook run after a change says `not judged`.
- A verify spends at most one `verify.baseBudget` on base work, where it could spend about two (ADR 0089).
- The JSON status `passed` is gone; `ran-no-failure` is new. The verify output schema says so.
- **Measured** (`bench/RESULTS.md`, "Old tests within their limits (ADR 0090)"): held-out misses 37 of 98 and 89 of 100, the same case by case; labelled corpus 0 false findings (0 of 196). Unchanged by construction, as for ADR 0089.

## Not done

- The hook's estimate is of the last run, not the slowest: a run slower than the last can still be cut by the step's deadline, which leaves the worktree's removal its kept time.
- Runners that print neither passing ids nor passing files (a Vitest or Jest reporter other than the default) give `not judged` for a run with no failure.
- `FSCALE_BASE_RUN=1` is still set for the run of the head's code (ADR 0089).

## Tests

All written first, seen failing on `main`:

- `old-tests.test.ts`: "in the stop hook, says not judged when no time is kept for the run, however long the limit"; "in the stop hook, runs once the worktree's and the step's times are kept, and keeps the worktree's time"; "shares verify.baseBudget with the base run, and says not judged when it has run out"; "says not judged when the run collected none of the base's tests"; and "never lowers a finding when the base's version passes", now expecting `ran-no-failure` and nothing in the normal output (it expected `passed`, the decision this replaces).
- `oldtests::tests::a_cargo_test_is_tied_to_a_file_only_when_no_other_could_be_it` (did not compile on `main`).
- `pertest::tests::what_ran_and_did_not_fail_is_named_for_each_runner`.
- Changed on purpose: the stop-hook test's line now says which limit ran out (`… before the stop hook's limit`).
