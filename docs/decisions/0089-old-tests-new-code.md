# 0089 — Old tests, new code: the base's tests on the head's code, as evidence

Status: accepted (2026-11-25, Track B run). Amends ADR 0042 and ADR 0046 (the tamper guard and its approvals), ADR 0040 (the head's worktree) and ADR 0083 (the stop hook's time limit). Verification idea 1, first step only.

## Context

The tamper guard compares the syntax of test files. A changed expected value, a rewritten snapshot or an assertion taken away cannot be told from an honest update by reading it, so those wait for a person (ADR 0046). What tells them apart is running the old tests: SWE-bench grades a patch by tests the agent could not edit, and ImpossibleBench found that making tests read-only restored honest behaviour (the research report's idea 1, read through the report).

## Decision

- **When**: the guard has an `expected-changed`, `snapshot-changed`, `assertion-removed` or `assertion-weakened` finding in a test file that the base commit has. Not otherwise: nothing extra runs on a change the guard found nothing in.
- **What runs**: for each unit (the deepest planned project folder holding the file, else the root), its planned `test` step, once, in one worktree of the head (`HEAD` with the working tree's changes, as ADR 0040's control run), set up as the base run's worktree is (the head's dependency folders linked in, or `verify.baseInstall`; its env files). Into it, from the base commit: those test files, and every changed `conftest.py`, `pytest.ini`, `vitest.config.*`, `vitest.workspace.*`, `jest.config.*` and snapshot (`__snapshots__/`, `.snap`, `.ambr`); a file the base lacks is removed. Only steps whose runner verify reads test by test: pytest, Vitest, Jest, `cargo test`.
- **Read by the results, not the exit code**: a failing test of those files in the runner's results is `failed` even when the step exited 0 (a head script ending in `|| exit 0`). `passed` says only that the step passed and named no failing test of them, not that they ran.
- **Rust**: the worktree does not borrow the base runs' kept build folders (ADR 0081): cargo builds the head's code from nothing there.
- **What it adds**: per unit, an entry in `tamper.oldTests` (`step`, `status`: `failed`, `passed` or `not-judged`, `files`, `detail`). On `failed`, each finding of a file with a failing test gets `evidence`: the test's id and its first reason lines, at most five tests. Human output: `evidence:` lines under the finding, and an `Old tests, new code (…; report only): …` line.
- **Never a verdict.** A passing run never lowers or removes a finding. Nothing here adds a finding or changes `blocks()`: report only, even with `enforced`.
- **Not judged**: no test step, a runner it does not read, a Rust file whose tests sit beside its code (`src/lib.rs`: the base's copy would bring the base's code), a run that fails with no readable results (the tests do not collect or build against the head's code), a failure outside those files, setup needed, a timeout, no time left.
- **Where**: `fscale verify` (laptop and the `pull_request` CI job), the stop hook, and `fix --agent`'s referee (its failure reason carries the first evidence line). **Never in a `pull_request_target` run**: `fscale tamper --pr-base` does not call it, and verify skips it when `GITHUB_EVENT_NAME` is `pull_request_target`.
- **Time**: `verify.baseBudget` (5 minutes by default), counted from when this run starts. In Claude Code's stop hook, also the hook's `timeout` (or 600 s) less 30 seconds, counted from the hook's start (`hooks::hook_deadline`; a value too large for the clock counts as 600 s). A step whose kept time is longer than what is left is not started, and past the limit nothing new starts: `not judged`. Making and removing the worktree are not timed: on a large repo they could take the hook past its 30-second margin (Not done).

## Consequences

- **Measured** (`bench/RESULTS.md`, "Old tests, new code (ADR 0089)"): the held-out corpora's misses are unchanged (37 of 98 and 89 of 100, case by case the same), as they must be: `tamper-misses.mjs` runs `fscale tamper`, which never calls this, and nothing here changes a finding. The labelled corpus, run through `verify`: 0 false findings (0 of 196). The held-out cases have no runnable test steps (ADR 0082), so this run's own catch rate on them could not be measured.
- A change with such a finding costs one more test run of its unit (cached passes are not used: the files differ).
- `StepReport` keeps each failing test's first three reason lines as text, in memory only (`reason_text`, never in JSON or a cache): the hashed reasons cannot be shown.
- `cargo test` names a test binary, not a file: a failing test is matched to `tests/<binary>.rs`.

## From the fresh review

Fixed, each with a test written first and seen failing (or, for the guardrail, written to guard it):

- the stop hook could panic on a huge `timeout` (`1e20`), letting the agent stop unverified (`hooks::tests::the_hooks_deadline_never_panics_on_a_huge_or_odd_timeout`);
- a step that exited 0 while the base's test failed was `passed`, with words that read as proof (`old-tests.test.ts`, "says failed, not passed…");
- `cargo test` binaries with `-` in their names, and `tests/<name>/main.rs`, were never matched;
- `fix --agent` could show another file's evidence beside the finding it named;
- the evidence kept the worktree's absolute path and backslashes (now `<root>` and forward slashes);
- a file removed from the worktree was not checked to be inside it first;
- no test of the `pull_request_target` guardrail (`old-tests.test.ts`, "never runs in a pull_request_target run").

## Not done

- The run uses the step command as planned, whole: not only the changed files' tests.
- Making and removing the worktree are not timed, so the stop hook could still pass its limit on a large repo; and `verify.baseBudget` is counted again after the base run's own, so a verify can spend about twice it on base work (from the review).
- The run sets `FSCALE_BASE_RUN=1`, which the head's code can read, and the head's own config can leave the base's tests out: `passed` is never proof (from the review).
- `cargo test`: a crate's library unit tests are a binary of the crate's name, so a failing one can be matched to `tests/<crate name>.rs`; two crates' `tests/x.rs` under one unit match each other (from the review).
- A `snapshot-changed` finding of an approval file (`*.approved.*`) is never judged; a new snapshot test added with the change fails under `CI=true` for want of a base entry and is reported as evidence (from the review).
- A Python project's pytest config in `pyproject.toml` or `setup.cfg`, and Jest's config in `package.json`, are not taken from the base (taking the whole file would take the base's dependencies too).
- Not run in the stop hooks of Codex and Cursor past `verify.baseBudget`: no time limit of theirs is known (ADR 0083).

## Later

- Run only the tests of the changed files (`pytest <files>`, `vitest run <files>`).
- With `enforced`, let a failing run block a finding that needs approval until it is approved, once its false rate on real history is measured (the research report's step 2).

## Tests

- `packages/cli/src/old-tests.test.ts`, 5 tests with a stand-in Vitest: evidence and `failed`; `passed` lowers nothing; `not judged` when nothing can be read; nothing runs without a finding; the stop hook says `not judged` past its limit. The first four were seen failing with `main`'s binary (`FSCALE_BIN`); the fifth guards that nothing runs, and passes before and after.
- `oldtests::tests`: snapshots to test files, runner config, Rust files with code, failing tests to files.
- `tamper::tests::every_field_of_the_report_is_in_the_shipped_schema` now fills `evidence` and `oldTests`; seen failing before the schema had them.
