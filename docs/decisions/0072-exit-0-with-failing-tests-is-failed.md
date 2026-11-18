# 0072 — A test command that exits 0 with failing tests is FAILED

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Amends ADR 0038 and ADR 0039 (per-test results were only used to label a failure) and ADR 0036 (what is cached).

## Context

`docs/research/research-verification.md`, idea 2, step one. Verify judged a step by its exit code alone: exit 0 was a pass, and a pass is cached. The per-test results that verify already reads for pytest, Vitest, Jest and `cargo test` were looked at only after a failure.

So a change could pass verify with failing tests by making the command lie about its exit code:

- a test script that ends in `|| exit 0`, `|| true` or `; exit 0`,
- a `conftest.py` hook that sets `session.exitstatus = 0`.

The held-out cheat corpus has both (`bench/RESULTS.md`, "Misses on cheating changes", category "other").

## Decision

**A step that exits 0 while its per-test results hold a test that failed or errored is `FAILED`.** Its detail reads `<command> (exit 0): the command reported success but N tests failed` (`1 test failed` for one). The progress line reads `FAILED after <time> (exit 0, but N tests failed)`.

- It applies to the four runners with per-test results: pytest (the JUnit report asked for through `PYTEST_ADDOPTS`), Vitest and Jest (their default reporters' failure lines) and `cargo test` (its text output).
- The step is a failure in every way: it fails the run, it is never cached and its time is not kept as a pass, its failing tests are listed, and it has a failure signature, so blame-aware verify labels it like any other failure.
- In JSON the step has `status: "failed"` and `exitCode: 0`. No schema change.
- Only tests count. The two identities that stand for something beside the tests, `(coverage threshold)` and `(unhandled errors)`, do not turn an exit code of 0 into a failure.
- The same code runs the base commit's steps, so a base whose command hides its failures is read as failing too.

## Consequences

- A repo whose test script swallows its exit code on purpose now fails verify while a test fails. That is the point, and there is no setting to turn it off. `verify.test` can name another command.
- A command that runs the tests twice and passes the second time (`cargo test || cargo test`, a hand-made retry) is `FAILED` when the first run's failures are in the output: the results hold a failed test. For pytest only the last session's report is read, so a retry there passes as before.
- A test that prints text shaped like its runner's failure lines to the step's output can be read as a failing test. This was already so for labels; it now also decides the status.
- A hook that rewrites failed reports to passed defeats this: the results then hold no failure (research idea 2, risks).

## Later

- Step two of the idea: keep the set of executed test ids with each kept pass and base result, and report a test that ran on the base and did not run on the head (`test-not-run`). It catches `sys.exit(0)` in a conftest and `process.exit(0)` in a global setup, where no test runs at all. Not started.

## Tests

- `packages/cli/src/exit-zero.test.ts`, written first and seen failing on `main` in WSL: 5 of 9 failed, each with `expected +0 to be 1` (verify exited 0), one per runner and one for the singular. The 4 that passed are the guards: the same stand-ins with no failing test still pass and are cached.
