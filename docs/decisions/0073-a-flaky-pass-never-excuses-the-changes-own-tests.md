# 0073 — A flaky pass never excuses the change's own tests

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Amends ADR 0039 (a new failure that passes when run again is `flaky`).

## Context

`docs/research/research-build-tools.md`, idea 4, part 1, and `docs/research/research-verification.md`, idea 3, part (i).

A `new` failure runs once more, and a pass makes it `flaky`, which does not block (ADR 0039). No history is kept (`SPEC.md` §12). So a change that adds or edits a test so that it fails one run in three passes verify most times it fails, and an agent held by the stop hook can try again.

## Decision

**A new failing test that sits in a file the change added or edited is never labelled `flaky`.** Its step stays `new`, it is not run again, and a note says why:

```
test: not run again, and it stays new: 1 failing test is in a file this change added or edited (src/pay.test.ts: src/pay.test.ts > pay > charges). A failing test of the change itself is never labelled flaky (ADR 0073).
```

- **Which tests:** the step's failing tests that blame-aware verify labelled `new`. A test that also fails on the base commit is not the change's.
- **Which files:** the files that differ between the base commit and the head, uncommitted and untracked files included: the same list every other part of blame-aware verify uses. A test the change added or edited is in such a file, so one rule covers the three cases of the brief (added, edited, in a touched test file).
- **How a test's file is found** (`own_tests` in `crates/core/src/blame.rs`):
  - pytest, Vitest and Jest name the file in the test's identity (`tests/test_pay.py::test_charges`, `src/pay.test.ts > pay > charges`). The runner writes it from its own root folder, so a changed path counts when it is that file in the step's folder, or ends with `/` and that file.
  - `cargo test` names a test by its modules (`pay::tests::charges`), not by its file. Each changed `.rs` file is read with the tamper guard's Rust reader, and a failing test whose last name part is a test of that file counts.
- **The step is not run again.** The second run could only have decided between `flaky` and `new`, and `flaky` is ruled out. That also saves the time of a whole step.
- **Runners with per-test results only:** pytest, Vitest, Jest and `cargo test`. A step with no per-test results is run again and can be `flaky`, as before.

## Consequences

- An honest edit to a test file that holds a test which was already flaky blocks when that test fails. The way out is to run verify again, or to fix the test; verify cannot tell the two cases apart without history.
- When several failing tests are new and one is the change's own, the step is not run again, so the others are not shown as `flaky` either. The step blocks either way.
- Matching is by path ending, and for Rust by test name, so it can count a test that is not the change's: `src/pay.test.ts` in two folders of the change, or two Rust tests with one name. Both err toward `new`.

## Not done

- When git cannot list the change, nothing is known about the files, and the rerun works as before.
- A failing test in an unchanged file that imports a test helper the change edited is not the change's own by this rule.

## Later

- Part 2 of build idea 4: `verify.flaky: "block" | "allow"`, as nextest's `flaky-result`.
- Part (ii) of verification idea 3: within one agent session, a test that failed in two verify runs is not labelled flaky again. It keeps a small record in the loop guard's file, which is a step toward the history `SPEC.md` §12 rules out: the owner's call.

## Tests

- `packages/cli/src/flaky-own-tests.test.ts`, written first and seen failing in WSL: 5 of 9 failed with `expected +0 to be 1` (the step was `flaky` and verify exited 0): one per runner with the test's file edited, and one with the file added. The 4 that passed are the guards: with another file edited, the same failure is still run again and `flaky`. For `cargo test` that other file is a Rust file with a test of its own.
- `a_changed_file_is_a_tests_file_from_the_steps_folder_or_any_folder_above_it` and `only_new_failing_tests_in_files_of_the_change_are_its_own` (`crates/core/src/blame.rs`) came with the fix and call what it adds.
