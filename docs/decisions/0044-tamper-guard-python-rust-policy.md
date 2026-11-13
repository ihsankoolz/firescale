# 0044 — The tamper guard: Python and Rust, code that never runs, suppressions, and its policy keys

Status: accepted (2026-11-13, M24). Extends ADR 0042; nothing in it is reversed. M25 keeps the config and CI detectors (coverage thresholds, CI steps, `--no-verify`); M26 approvals and measured rates.

## Context

ADR 0042 built the guard for JavaScript and TypeScript and admitted its misses. M24 asks for Rust and Python and for the policy keys. The owner's real project is five pytest services, so Python is the language that matters first, and asked for more in this milestone:

- the misses ADR 0042 admits, where they can be closed without guessing: `if (false)` or an early `return` before a test's assertions, and an assertion replaced by one that cannot fail (`expect(true).toBe(true)`, `assert True`);
- new lint-disable comments and `allow` attributes as findings, compared with the base as net counts (planned for M25; brought forward).

Same rules as M23: syntax trees, not diff lines; policy from the base commit; report-only by default; every kind of finding with a caught case and a not-a-finding case; counts in `bench/RESULTS.md`, which are fixture counts, not a measured rate.

## Decision

The guard is now a module per language (`crates/core/src/tamper/`: `js.rs`, `python.rs`, `rust.rs`), with what they share in `reach.rs` (code that never runs, bodies that do nothing, values that never change), `suppress.rs` (lint suppressions) and `policy.rs` (`verify.tamper`). Each reader gives the same `TestFile` (tests, skips, assertions, imports), so the comparison of ADR 0042 is unchanged.

### Python: pytest and unittest

- **Test files**: `test_*.py`, `*_test.py`, `conftest.py`, or any `.py` in a `test` or `tests` folder (pytest's defaults, and where helpers with assertions live).
- **Tests**: `def test*` (also `async def`) at module level, and `test*` methods of a class named `Test*` or one whose superclass ends with `TestCase`. Id: classes and name joined with `::` (`TestCart::test_total`), as pytest's node ids are, without the file, so a move between files keeps it.
- **Skipped**: `@pytest.mark.skip`, `skipif`, `xfail` (on the test or a class around it, also through `from pytest import mark`), `pytestmark = …` of the module or the class, `@unittest.skip`, `skipIf`, `skipUnless`, `expectedFailure` (also through aliases); a body whose first statement is `pytest.skip(…)`, `pytest.xfail(…)`, `self.skipTest(…)` or `raise unittest.SkipTest(…)`; a body that does nothing (`pass`, `...`, a docstring, a bare `return` before anything runs); a definition that never runs (under `if False:`).
- **Rows**: `@pytest.mark.parametrize` with a list or tuple literal weighs the test by its rows (several decorators multiply), so folding three tests into one parametrized test with three rows is not a deletion; a row given `marks=pytest.mark.skip` is one skipped run.
- **Assertions**: every `assert`; every call whose last name starts with `assert` (`self.assertEqual`, `mock.assert_called_once_with`, `np.testing.assert_allclose`); `pytest.raises` and `pytest.warns`.
- **Subjects**: `from app.pricing import total` and relative imports resolve to a repo file (the test's folder and each folder above it, also under `src/`), for "deleted with its subject".

### Rust

- **Test files**: a `.rs` file in a `tests` folder, or **any** `.rs` file that holds tests on the base or now (`#[test]`, `#[cfg(test)]`, `…::test]`, `#[rstest`, `#[test_case`, `#[wasm_bindgen_test`): unit tests live next to the code.
- **Tests**: a `fn` with `#[test]`, an attribute ending in `::test` (`#[tokio::test]`), `#[rstest]` (weighed by its `#[case]` rows), `#[test_case(…)]` (one per attribute) or `#[wasm_bindgen_test]`. Id: modules and name joined with `::`.
- **Skipped**: `#[ignore]`, `#[ignore = "…"]`, `ignore` inside `#[cfg_attr(…)]` (a conditional skip, like `skipIf`), `#[cfg(any())]` (never true) on the test or a module around it, a body that does nothing. `#[should_panic]` is a test that runs.
- **Assertions**: macros whose name starts with `assert`, `debug_assert` or `prop_assert` (`assert!`, `assert_eq!`, `assert_matches!`, `insta::assert_snapshot!`).
- **Subjects**: the names a file defines outside its test code; a unit test deleted with the file that defined what it tests, none of those names left in the change, is not counted. A file whose tests sit outside `#[cfg(test)]` (an integration test file) defines no subject, so deleting a whole `tests/x.rs` with its helpers is never excused.
- **The grammar**: `tree-sitter-rust` 0.24.2 (ABI 15, which tree-sitter 0.26 reads; builds on Rust 1.85; MIT). Size and build time in `bench/RESULTS.md`.

### Code that never runs, and assertions that cannot fail (all languages)

Closed only where the syntax tree makes it certain:

| Change                                                                                                                                                                                                                  | Read as                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| An assertion inside `if (false)`, `if False:`, `if false {}`, or a `while` with a literal false condition                                                                                                               | not counted, so `assertion-removed`          |
| An assertion after a `return` (or `pytest.skip(…)`) earlier in its block or a block around it                                                                                                                           | not counted                                  |
| A test inside `if (false)`, or after a `return` in its `describe`                                                                                                                                                       | skipped, "never runs: …"                     |
| A test whose body runs nothing (`() => {}`, `pass`, `{}`, a `return` before anything), or `it("x")` with no function                                                                                                    | skipped, "never runs: its body does nothing" |
| An assertion whose every argument is a literal (`expect(true).toBe(true)`, `expect(1 + 1).toEqual(2)`, `assert.ok(true)`, `assert True`, `assert 1 == 1`, `self.assertTrue(True)`, `assert!(true)`, `assert_eq!(1, 1)`) | not counted                                  |

Not closed, because closing them would mean guessing: a condition that is false but not a literal (`if (0)`, `if (process.env.X)`), a `return` behind a condition, an assertion weakened but still about the code (`toBe(3)` → `toBeDefined()`, M26), an assertion on a value the test sets itself (`const x = 1; expect(x).toBe(1)`). `expect.assertions(n)` and `expect.hasAssertions()` count other assertions and are never "cannot fail"; a call with a receiver other than `self` (`mock.assert_called_once()`) never is either: the receiver is what it checks. A JS `function` declaration is hoisted, so one written after a `return` still runs when called before it.

The same rules apply to the base, so a placeholder `expect(true).toBe(true)` on both sides counts on neither, and removing dead code is not a finding.

### Lint suppressions: `suppression-added`

Read from the syntax tree of **every changed JS/TS, Python and Rust file**, test or not: a directive in a string is not one, and the directive must open its comment (as each tool requires), so prose that mentions `noqa` is not one.

| Language | Suppressions                                                                                                                                                                                                          |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| JS/TS    | `eslint-disable`, `-line`, `-next-line` (and oxlint's); `@ts-ignore`, `@ts-expect-error`, `@ts-nocheck`; `biome-ignore`, `-all`, `-start`; `deno-lint-ignore`, `-file`                                                |
| Python   | `# noqa[: codes]`, `# ruff: noqa`, `# flake8: noqa`, `# type: ignore[codes]`, `# pyright: ignore[rules]`, `# mypy: ignore-errors`, `# mypy: disable-error-code=…`, `# pylint: disable=…`, `disable-next=…`, `# nosec` |
| Rust     | `#[allow(…)]`, `#[expect(…)]`, the same inside `#[cfg_attr(…)]`, and `#![allow(…)]` for a whole module or crate                                                                                                       |

Each is keyed by tool, reach and rule (`eslint:no-console`, `eslint-range:*`, `noqa:E501`, `allow:dead_code`, `allow-module:clippy::pedantic`; `*` for a directive with no rule). A key with more suppressions over the whole change than on the base is a finding, naming each new one with file and line. `@ts-ignore` and `@ts-expect-error` share a key, and so do `#[allow]` and `#[expect]`: turning one into the stricter other is not a finding. A file-wide or module-wide form has its own key, so three line disables folded into one `/* eslint-disable */` is a finding (wider reach, fewer comments). Coverage and formatting pragmas (`istanbul ignore`, `pragma: no cover`, `prettier-ignore`, `fmt: off`) are not counted; coverage is M25's.

**Net per rule over the whole change, not per file** as PLAN.md's M25 line said: a function moved to another file takes its `#[allow]` or `# noqa` with it, an honest edit that a per-file count would report. The cost is the swap of ADR 0042: removing one suppression of a rule in one file while adding one of the same rule elsewhere is not reported.

### The policy keys

`verify.tamper` is still a mode (`"off"`, `"evaluated"`, `"enforced"`), or now an object:

```json
"tamper": {
  "policy": "enforced",
  "testPaths": ["checks/**/*.py"],
  "allow": ["tests/legacy/**", { "path": "src/generated/**", "kinds": ["suppression-added"] }]
}
```

- **`testPaths`** ("protected test paths"): globs (`*` stays within a folder) of more files to read as test files, beside the names and folders the guard knows. Only files in a language the guard reads.
- **`allow`** ("allowed exceptions"): globs of files **left out of the counts** for every kind, or for the `kinds` listed. Leaving them out of the counts, rather than dropping findings named in them, matters: with net counts, a test deleted in an allowed file and one renamed elsewhere could otherwise name the allowed file and hide the real deletion. A test moved from an allowed file into a counted one is then a gain, and one moved the other way a loss (reported).
- **All of it comes from the base commit's `firescale.config.json`.** A change that widens `allow`, drops a `testPaths` entry or sets `"off"` has no effect on itself (tests for each). The working tree's value is checked for shape so a mistake fails loudly. A base value the guard cannot read keeps its mode when that is readable, else `evaluated`, and drops the lists: never looser than what it says.
- `fscale verify --json` lists the files `allow` left out (`tamper.allowed`), and the summary names them.

### Reading the base

The base side of every changed file comes from one `git cat-file --batch` (paths made relative to the repo's top with `git rev-parse --show-prefix`), not a `git show` per file: suppressions are read in every changed source file, and one process per file costs tens of milliseconds each on Windows.

## Measuring

`bench/tamper-corpus.json` holds the cases of ADR 0042 and M24's, in four languages; `packages/cli/src/tamper.test.ts` asserts every label and that **every kind of finding has a caught case and every language has not-a-finding cases**; `bench/tamper-rates.mjs` counts them by kind and by language (`bench/RESULTS.md`). The corpus was written together with the guard, by the same hands: its counts show the guard does what this ADR says, **not a measured false-positive or miss rate**. M26 measures that on a corpus that was not written with the guard.

## Known limits

- Net counts still miss a swap: one test deleted while an unrelated one is added, one suppression of a rule removed while another is added elsewhere.
- Python: a skip call that is not literally `pytest.skip`, `pytest.xfail` or `self.skipTest` stops its block only when it is the first statement; tests made by a fixture factory or `pytest_generate_tests`, `subTest` loops and doctests are not read; `from app import pricing` names the package, so its tests are not excused when only `app/pricing.py` is deleted (reported, never hidden).
- Rust: an `assert!` inside another macro's tokens (`proptest! { … }`) is not parsed by tree-sitter and is not counted (a miss when deleted, never a false finding); `panic!` in a `match` arm is not counted as an assertion.
- Deeply nested blocks make the reachability walk slow (tree-sitter finds a node's parent from the root): 1,000 levels take well under a second; nothing real comes near.
- Suppressions in other files (`.eslintrc` rules turned off, `ruff` config, `Cargo.toml` `[lints]`) are config: M25.

## Consequences

- An agent that deletes, skips, empties or disables a pytest or Rust test, wraps its assertions in `if False:`, returns before them, swaps them for `assert True`, or silences a lint is named with file and line, in report-only mode from the first run and blocking once `enforced` is on the default branch.
- A repo can protect test files the guard would not find and exempt generated or legacy files, and a change cannot edit either for itself.
- Python and Rust honest edits that keep the counts (rename, move, split, parametrize, reformat, stricter assertions, `#[allow]` to `#[expect]`) are not reported.
