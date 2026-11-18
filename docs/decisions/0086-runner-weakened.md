# 0086 — A test runner made unable to fail (`runner-weakened`), report only

Status: accepted (2026-11-18, Track B run). Amends ADR 0045 (config and CI detectors) and ADR 0046 (kinds that do not block on their own). Verification idea 7, first step only.

## Context

A test or lint can be made to pass without touching a test: the command that runs it can be told to ignore its result. CI's detector (ADR 0045) finds this in GitHub Actions workflows (`|| true`, `continue-on-error`), but not where the command is defined for every runner, in `package.json` scripts, nor in pytest's own entry point, `conftest.py`, which pytest imports before any test and whose hooks can rewrite the run's exit status. The research report (verification idea 7) names these as the first step of "runner laundering".

## Decision

- **A new kind, `runner-weakened`**, from a new detector (`tamper/runner.rs`) in the config-and-CI family: measures compared over the whole change, so a place already there on the base is not new.
- **`package.json`, in any folder**: a script whose name is a check (CI's detector's list: `test`, `lint`, `typecheck`, `check`, `verify`, `e2e`, …, and `name:suffix` forms) in which a command that checks, as CI's detector reads it (`ci::check_of`: `vitest`, `jest`, `eslint`, `pytest`, `npm run test:unit`, `cargo test`, …), is followed by what CI's detector calls swallowing (`|| true`, `|| :`, `|| exit 0`, `|| exit`, `|| echo …`) or by `; exit 0`. Package managers run scripts with `sh -c`, without `-e`, so `; exit 0` decides the script's result. CI's matcher is reused as it is: `commands`, `swallowed`, `is_check_script` and `check_of` (made `pub(crate)`).
- **`conftest.py`, in any folder**: `sys.exit(…)`, `os._exit(…)` or `raise SystemExit` at the top level, and a `def pytest_sessionfinish(` at the top level whose body assigns `exitstatus` (`session.exitstatus = 0`, or `setattr(…, "exitstatus", …)`), one per hook. Read line by line, not parsed.
- **Report only, even when the base enforces the guard.** `Kind::reports_only`; `TamperReport::counts_against` is false for it, so it never makes `blocks()` true. The finding is listed with `[report only: it never fails verify; a reviewer judges it]`, and when every finding is report-only, the summary's heading says so instead of "fails verify". It can be allowed with `verify.tamper.allow` like any kind.

## Consequences

- **Measured** (`bench/RESULTS.md`, "runner-weakened (ADR 0086)"), in WSL, before (`main` at `0d5c2ac`) and after:
  - First held-out corpus: 42 missed of 98 before, 40 after (2 cases of "other"). Second: 90 of 100 before, 89 after (1 of test modification). Compared case by case (counts only, no titles): no case caught before is missed after.
  - The labelled corpus (`bench/tamper-rates.mjs`): 0 false findings before (0 of 186 not-a-finding cases with a finding) and after (0 of 191, with the 5 new not-a-finding cases), and 6 of 6 new caught cases.
  - Limits: the held-out corpora were written for other detectors; the labelled cases were written with this one, so they are fixtures, not a rate.
- **What the author saw.** This session measured the second corpus on `main` before writing this detector, and the measuring script then printed the titles of its 90 missed cases (ADR 0082). It opened no case of either held-out corpus, and did not use any title to design this detector: the owner named the patterns (`|| true`, `|| exit 0`, `; exit 0`, a top-level `sys.exit`, `pytest_sessionfinish` setting `exitstatus`) before the second corpus existed. Whether a title shaped it cannot be proven from here; the second corpus's gain is one case.
- A script that loses its check altogether (`"test": "echo ok"`) is not this kind: only a check that stays and cannot fail is.
- `|| true` added in one `package.json` and removed in another, in the same change, nets to nothing.
- A `conftest.py` that exits inside `if __name__ == "__main__":`, or a hook that rewrites the result in a plugin file other than `conftest.py`, is not read.
- `pnpm test || true` in a script named `build` is not read: only check scripts are.

## Later

- Other runners' entry points: `jest.config` `testFailureExitCode`, a Vitest custom reporter that swallows failures, `pytest.ini` `addopts` with `--exitfirst`-style changes, Cargo `[[test]] harness = false` with a stub main (verification idea 7, the next steps).
- `pytest_runtest_makereport` hooks that rewrite a test's outcome.
- A blocking variant once the false-finding rate on real history is measured.

## Tests

- `bench/tamper-corpus.json`: 11 new `run:` cases (6 caught, 5 not a finding), run by `tamper.test.ts` and counted by `bench/tamper-rates.mjs` ("Runner scripts"). The 6 caught cases were seen failing on `main` in WSL; the 5 not-a-finding cases passed before and after.
- `tamper.test.ts`, "reports a runner made unable to fail, and never blocks on it, even when the base enforces the guard (ADR 0086)": seen failing first, then again for the heading.
- `tamper::runner::tests`: the script and `conftest.py` readers.
