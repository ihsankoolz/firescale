# 0038 — Blame-aware verify: per-test labels

Status: accepted (2026-11-13, M20). Builds on ADR 0037.

## Context

M19 labels whole steps. One new failing test among ten old ones makes the step fail on the base and on the head, so the step reads `pre-existing` and the new failure does not block. M20 compares failing tests, not steps, for the runners most of our users have: pytest, Vitest and Jest.

## Decision

### Asking a runner for its results without changing the user's command

- **pytest**: verify sets `PYTEST_ADDOPTS` to the user's own value (kept first) plus `-o junit_family=xunit1 '--junitxml=<a file in the temp folder>'`. pytest reads that variable as extra command-line options, so the command line the user wrote is unchanged, and no file is written in the repo. pytest splits the variable like a POSIX shell, so the path is in single quotes and written with forward slashes (a Windows temp folder, or one with a space, stays one path). `xunit1` makes each test case carry its `file`. The report's name has a constant shape (`firescale-junit-<10 digits>-<6 digits>.xml`, unique per step), and pytest's `generated xml file:` line is left out of a failure's signature. **When the user asks for a JUnit report already** (on the command line, in `PYTEST_ADDOPTS`, or in a pytest config file's `addopts` in the step's folder or above), verify adds nothing and compares the step as a whole. If pytest refuses its options (exit 4) and writes no report, the failure's output is never evidence for `pre-existing`. The file is read and deleted after the step. JUnit XML is pytest's own format; it is parsed with a small reader of `<testcase>` elements (no XML dependency).
- **Vitest and Jest**: their JSON reporters need `--reporter=json` / `--json` plus an output file on the command line, and appending flags to `npm test` reaches the script only with `--`, and not at all for a script that chains commands. verify does not edit commands (ADR 0035 appends `--maxfail=1` only for a plain pytest command, for one extra run). So verify reads their **default reporters' failure lines**, which both print in CI mode (`CI=1`, no colour): Vitest's ` FAIL  file > suite > test` lines inside its `Failed Tests` and `Failed Suites` sections (not a `FAIL` a test printed) and its `Tests  N failed | M passed` line; Jest's `FAIL file` lines (the file is the last path-like word, after a project's display name) and the `● suite › test` lines inside a failing file, without Jest's own notices (`● Console`, deprecation and validation warnings), and its `Tests:` line. Both formats have been stable across their recent major versions; a format change makes verify fall back to step labels, never to wrong test labels (an unparsed report gives no per-test results).
- Which runner a step uses is read from its command, and for a package script (`npm test`, `pnpm run test`) from the script's text in `package.json`.

### Identity

A test is its file (repo-relative, forward slashes) and its name with its suites: `tests/test_auth.py::TestLogin::test_ok` (pytest's node id, from the JUnit `classname` and `name`), `src/a.test.ts > math > adds` (Vitest), `src/a.test.js › math › adds` (Jest). A failure of a whole file (an import error, a syntax error) is the file alone.

### Labels

With per-test results on the head and on the base, for a step that fails on both:

- a test that fails on the head and not on the base is **new**: the step is `new` and blocks, and the summary names each new test (`New failing tests:`), saying how many others already failed on the base;
- if no failing test is new, the step is `pre-existing` **only if its output on the head is also within the base's** (the evidence rule of ADR 0037, unchanged). Per-test results can make a failure `new`; they never make one `pre-existing` on their own, because a base whose environment fails exactly the head's failing tests (a missing build output, say) would otherwise let a broken change through. When the same tests fail but the output differs, the step gets no label and blocks, with a note;
- **a base on which no test passed is not evidence**: when the base reported no passed test and the head did, its run failed for its environment, not its code. The step gets no label and blocks, with a note.

Each failing test is listed in the step's `failedTests` in JSON, with `new` or `pre-existing`. A step that passes on the base and fails on the head is `new`, and so is each of its failing tests.

When either side has no per-test results (another runner, a report that could not be read, a base result kept before M20), the step is compared as a whole, as in ADR 0037, and a note says its runner gives no per-test results.

### Renamed test files

A test file renamed between the base and the head (`git diff -M --name-status <base>`, git's default 50% similarity; a rename git sees, staged or committed) keeps its identity: the base's failing tests in the old path are moved to the new path before comparing, whether the ids are relative to the project's folder or to the repo root (a pytest config at the root). So an old failure in a renamed file is **never labelled `new`**: its test is listed as `pre-existing`. Its output names the new path, so the output evidence does not hold and the step gets no label: it blocks, with a note that the same tests fail on the base. A test renamed inside a file, or a file changed beyond git's rename detection, is a different test, so its failure is `new`. Both are on purpose: an unknown failure blocks.

### Stored base results

The base store (ADR 0037, now format 4) keeps the failing test ids and the count of passed tests with a base result, so a second run with the same base labels per test without a base run.

### Also

`PYTEST_ADDOPTS` is in the step cache's allowlist (ADR 0036), read from fscale's own environment, so the report option verify adds never changes a key. `core/env-example` already ignores `PYTEST_*` variables.

## After the review

An independent review before the pull request found that the unquoted report path broke pytest's option splitting for a temp folder with a space (and on Windows), identically on the base and the head, so a new failure read as `pre-existing`; that pytest's `generated xml file:` line made a kept base result differ from the head's output; that renames missed ids relative to the repo root, and collection errors and capitalised packages gave odd ids; that Jest's `● Console`, display names, tests that print `FAIL`, and Vitest names with `[` gave false failing tests; and that a user's own `--junitxml` in a pytest config was overridden. All are fixed as described above, with tests.

## Consequences

- One new failing test among old ones blocks, and the agent is told which.
- A user's own `--junitxml` on the command line wins over verify's (pytest keeps the last one), and that step is compared as a whole.
- A Vitest or Jest reporter other than the default (a custom one, `--reporter=dot`) gives no per-test results; the step is compared as a whole.
