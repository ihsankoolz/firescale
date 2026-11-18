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
- The step is a failure in every way: it fails the run, it is never cached, its time is kept as a failure's (it can raise the kept time, never lower it), its failing tests are listed, and it has a failure signature, so blame-aware verify labels it like any other failure. A run that stops at its first failure (`pytest -x`, `--bail`) is never evidence for `pre-existing`, as for any failure (ADR 0039).
- In JSON the step has `status: "failed"` and `exitCode: 0`. No schema change.
- Only tests count. The two identities that stand for something beside the tests, `(coverage threshold)` and `(unhandled errors)`, do not turn an exit code of 0 into a failure.
- **The base commit's steps are judged by their exit code alone, as before.** So is the run in the head-check worktree that confirms `fixed`. Reading a base that hides its failures as failing would turn the head's failure from `new` into `pre-existing`: a pass that is easier to get than before this change, which this run may not add. The cost is under "Needs your decision".

## Consequences

- A repo whose test script swallows its exit code on purpose now fails verify while a test fails. That is the point, and there is no setting to turn it off. `verify.test` can name another command.
- A command that runs the tests twice and passes the second time (`cargo test || cargo test`, a hand-made retry) is `FAILED` when the first run's failures are in the output: the results hold a failed test. For pytest only the last session's report is read, so a retry there passes as before.
- A test that prints text shaped like its runner's failure lines to the step's output can be read as a failing test. This was already so for labels; it now also decides the status.
- A hook that rewrites failed reports to passed defeats this: the results then hold no failure (research idea 2, risks).

## Needs your decision

- **A repo whose base commit already hides failing tests behind an exit code of 0 gets `new` for them on every change**, though the change did not break them: the head is read by its results, the base by its exit code. Reading the base by its results too would be truthful and would label them `pre-existing`, which lets through a failure that blocked before this change (a head that exits 1 against such a base). Say if the base should be read by its results.

## Not done (from the fresh review)

- The step is `FAILED`, never `SETUP NEEDED`: the setup detection that reads a failing step's output (a database that is down, a missing program) is not run for it.
- Ways to keep the results from verify, so that the exit code alone decides, as before: a command that asks for its own JUnit report (`pytest --junitxml=x || true`), `cargo test -q || true` (no per-test lines), `vitest run --reporter=dot || true`, output sent to a file.
- `TestResults.partial` is not looked at: results that may not cover the whole run still fail the step when they name a failed test.
- Not tested: the JUnit output of pytest plugins that retry tests.

## Later

- Step two of the idea: keep the set of executed test ids with each kept pass and base result, and report a test that ran on the base and did not run on the head (`test-not-run`). It catches `sys.exit(0)` in a conftest and `process.exit(0)` in a global setup, where no test runs at all. Not started.

## Tests

- `packages/cli/src/exit-zero.test.ts`, written first and seen failing on `main` in WSL: 5 of 9 failed, each with `expected +0 to be 1` (verify exited 0), one per runner and one for the singular. The 4 that passed are the guards: the same stand-ins with no failing test still pass and are cached.
- Three more for what the fresh review found, each seen failing on the first fix before the second: a base commit that hides the same failing tests (`expected +0 to be 1`: the failure was read as pre-existing), a head that runs `pytest -x` and exits 0 (the same), and the kept step time (`expected 40 to be greater than or equal to 1500`: the failure's short time replaced the last pass's). A fourth is a guard that passed before and after: a coverage threshold and unhandled errors beside an exit code of 0 still pass.
