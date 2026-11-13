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
- **Skipped**: `@pytest.mark.skip`, `skipif`, `xfail` (on the test or a class around it, also through `from pytest import mark`, and through a named mark: `needs_db = pytest.mark.skipif(…)`, then `@needs_db`), `pytestmark = …` of the module or the class, `@unittest.skip`, `skipIf`, `skipUnless`, `expectedFailure` (also through aliases); `__test__ = False` in the module or the class; a module-level `pytest.importorskip(…)` (a conditional skip of the module); a body whose first statement is `pytest.skip(…)`, `pytest.xfail(…)`, `self.skipTest(…)` or `raise unittest.SkipTest(…)`, or a class whose `setUp`, `setUpClass`, `setup_method` or `setup_class` starts so; a module-level `raise unittest.SkipTest(…)` (what follows never runs); a body that does nothing (`pass`, `...`, a docstring, a bare `return` before anything runs) unless a decorator pytest does not know may supply it (pytest-bdd's `@scenario`); a definition that never runs (under `if False:`).
- **What keeps tests from being collected**: each `collect_ignore` or `collect_ignore_glob` entry in a `conftest.py`, and each autouse fixture whose first statement skips, counts as one skip and no test, so adding one is `test-skipped` and removing one (running those tests again) is no finding.
- **Rows**: `@pytest.mark.parametrize` with a list or tuple literal, or a module-level name assigned one once, weighs the test by its rows (several decorators multiply), so folding three tests into one parametrized test with three rows is not a deletion; a row given `marks=pytest.mark.skip` is one skipped run.
- **Assertions**: every `assert`; every call whose last name starts with `assert` (`self.assertEqual`, `mock.assert_called_once_with`, `np.testing.assert_allclose`); `pytest.raises` and `pytest.warns`; `self.fail(…)` and `pytest.fail(…)`.
- **Subjects**: `from app.pricing import total`, `from app import pricing` and relative imports resolve to a repo file (the test's folder and each folder above it, also under `src/`), for "deleted with its subject".

### Rust

- **Test files**: a `.rs` file in a `tests` folder, or **any** `.rs` file that holds tests on the base or now (`#[test]`, `#[cfg(test)]`, `…::test]`, `#[rstest`, `#[test_case`, `#[wasm_bindgen_test`): unit tests live next to the code.
- **Tests**: a `fn` with `#[test]`, an attribute ending in `::test` (`#[tokio::test]`), `#[rstest]` (weighed by its `#[case]` rows), `#[test_case(…)]` (one per attribute) or `#[wasm_bindgen_test]`. Id: modules and name joined with `::`.
- **Skipped**: `#[ignore]`, `#[ignore = "…"]`, `ignore` inside `#[cfg_attr(…)]` (a conditional skip, like `skipIf`), `#[cfg(any())]` (never true), and any other `#[cfg(…)]` than `cfg(test)` (`all(test, feature = "slow")`, `unix`, `not(test)`, a file's `#![cfg(feature = "x")]`: compiled only sometimes, a conditional skip) on the test or a module around it, and a body that does nothing. `#[should_panic]` is a test that runs.
- **Test modules in another file**: `#[cfg(test)] mod tests;` counts as one test, so turning it off (`cfg(any())`, a feature gate) or deleting it is a finding even when the file it compiles is not part of the change. Renaming the declaration and its file together is not.
- **Assertions**: macros whose name starts with `assert`, `debug_assert` or `prop_assert` (`assert!`, `assert_eq!`, `assert_matches!`, `insta::assert_snapshot!`).
- **Subjects**: the names a file defines outside its test code; a unit test deleted with the file that defined what it tests, none of those names left in the change, is not counted. A file whose tests sit outside `#[cfg(test)]` (an integration test file) defines no subject, so deleting a whole `tests/x.rs` with its helpers is never excused.
- **The grammar**: `tree-sitter-rust` 0.24.2 (ABI 15, which tree-sitter 0.26 reads; builds on Rust 1.85; MIT). Size and build time in `bench/RESULTS.md`.

### "Deleted with its subject", tightened (all languages)

ADR 0042 leaves out a test deleted together with every repo module it uses. Two holes found in review are closed: a module that is itself a test file (a factory, a fixture module beside the tests, or one the base's `testPaths` names) is a helper, never a subject, so `git rm -r tests/payments` with its `factories.py` is reported; and a relative import that resolves to no file (a path alias, a namespace package) makes the subject unknown, so the test is never excused.

### Code that never runs, and assertions that cannot fail (all languages)

Closed only where the syntax tree makes it certain:

| Change                                                                                                                                                                                                                                                                                                                                                                        | Read as                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| An assertion inside `if (false)`, `if False:`, `if false {}`, or a `while` with a literal false condition                                                                                                                                                                                                                                                                     | not counted, so `assertion-removed`          |
| An assertion after a `return` (or `pytest.skip(…)`) earlier in its block or a block around it                                                                                                                                                                                                                                                                                 | not counted                                  |
| A test inside `if (false)`, or after a `return` in its `describe`                                                                                                                                                                                                                                                                                                             | skipped, "never runs: …"                     |
| A test whose body runs nothing (`() => {}`, `pass`, `{}`, a `return` before anything), or `it("x")` with no function                                                                                                                                                                                                                                                          | skipped, "never runs: its body does nothing" |
| An assertion that checks only literals and certainly passes: `expect(true).toBe(true)`, `toEqual`/`toStrictEqual` of the same literal, `toBeTruthy()` of a truthy one, `assert.ok(true)`, `assert.equal("a", "a")`, `assert True`, `assert 1 == 1`, an always-true tuple `assert (x, "msg")`, `self.assertTrue(True)`, `assert!(true)`, `assert!(1 == 1)`, `assert_eq!(1, 1)` | not counted                                  |

An assertion that certainly **fails** (`assert False, "should raise"`, `assert.fail(…)`, `expect(true).toBe(false)`, `assert!(false, …)` in a `match` arm, `assert_eq!(1, 2)`) is the opposite: it marks a path that must not run, so it counts like any other, and removing it is `assertion-removed`. Arithmetic and comparisons other than equality of the same literal (`expect(1 + 1).toEqual(2)`, `assert!(-1.5 < 2.0)`) are not worked out, so they count too.

Not closed, because closing them would mean guessing: a condition that is false but not a literal (`if (0)`, `if (process.env.X)`), a `return` behind a condition, an assertion weakened but still about the code (`toBe(3)` → `toBeDefined()`, M26), an assertion on a value the test sets itself (`const x = 1; expect(x).toBe(1)`). `expect.assertions(n)` and `expect.hasAssertions()` count other assertions and are never "cannot fail"; a call with a receiver other than `self` (`mock.assert_called_once()`) never is either: the receiver is what it checks. A JS `function` declaration is hoisted, so one written after a `return` still runs when called before it.

The same rules apply to the base, so a placeholder `expect(true).toBe(true)` on both sides counts on neither, and removing dead code is not a finding.

### Lint suppressions: `suppression-added`

Read from the syntax tree of **every changed JS/TS, Python and Rust file**, test or not: a directive in a string is not one, and the directive must open its comment (as each tool requires), so prose that mentions `noqa` is not one.

| Language | Suppressions                                                                                                                                                                                                          |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| JS/TS    | `eslint-disable`, `-line`, `-next-line` (and oxlint's); `@ts-ignore`, `@ts-expect-error`, `@ts-nocheck`; `biome-ignore`, `-all`, `-start`; `deno-lint-ignore`, `-file`                                                |
| Python   | `# noqa[: codes]`, `# ruff: noqa`, `# flake8: noqa`, `# type: ignore[codes]`, `# pyright: ignore[rules]`, `# mypy: ignore-errors`, `# mypy: disable-error-code=…`, `# pylint: disable=…`, `disable-next=…`, `# nosec` |
| Rust     | `#[allow(…)]`, `#[expect(…)]`, the same inside `#[cfg_attr(…)]`, and `#![allow(…)]` for a whole module or crate                                                                                                       |

Each is keyed by tool, reach and rule (`eslint:no-console`, `eslint-range:*`, `noqa:E501`, `allow:dead_code`, `allow-module:clippy::pedantic`; `*` for a directive with no rule). A key with more suppressions over the whole change than on the base is a finding, naming each new one with file and line. `@ts-ignore` and `@ts-expect-error` share a key, and so do `#[allow]` and `#[expect]`: turning one into the stricter other is not a finding. A file-wide or module-wide form has its own key, so three line disables folded into one `/* eslint-disable */` is a finding (wider reach, fewer comments); so are a `# type: ignore` before a module's first statement (mypy then ignores the module), an `#[allow]` on a `mod` item, an inline ESLint configuration that turns a rule off (`/* eslint no-console: "off" */`), `# pylint: skip-file` and `# pyright: rule=false`.

A suppression the change removes may stand for a narrower one it adds: `# type: ignore` → `# type: ignore[union-attr]`, `# noqa` → `# noqa: F841`, a bare `eslint-disable-next-line` → one naming its rule, `#[allow(clippy::all)]` → one clippy lint. Each removed suppression covers one added one of the same tool that turns off no more and reaches no further, anywhere in the change; a removed file-wide one also covers every narrower one added in its own file (a file-wide disable split into line disables). Coverage and formatting pragmas (`istanbul ignore`, `pragma: no cover`, `prettier-ignore`, `fmt: off`) are not counted; coverage is M25's.

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
- **All of it comes from the base commit's `firescale.config.json`.** A change that widens `allow`, drops a `testPaths` entry or sets `"off"` has no effect on itself (tests for each). The working tree's value is checked for shape so a mistake fails loudly. A base value the guard cannot read keeps its mode when that is readable (else `evaluated`) and every valid `testPaths` glob, and drops `allow`: never looser than what it says.
- `fscale verify --json` lists the files `allow` left out (`tamper.allowed`), and the summary names them.

### Reading the base

The base side of every changed file comes from one `git cat-file --batch` (paths made relative to the repo's top with `git rev-parse --show-prefix`), not a `git show` per file: suppressions are read in every changed source file, and one process per file costs tens of milliseconds each on Windows.

## After the review

An independent review of the diff before the pull request reproduced these, all fixed with unit tests and corpus cases:

- **A regression**: every all-literal assertion was read as "cannot fail", so removing `assert False, "should raise"`, `assert.fail(…)` or `assert!(false)` on a path that must not run was no longer reported (M23 reported the JS one). Now only literals that certainly pass are left out.
- **Rust tests turned off without touching them**: `#[cfg(all(test, feature = "slow"))]`, `#![cfg(feature = "x")]` and `#[cfg(not(test))]` were not read, nor was `#[cfg(test)] mod tests;` turned off. Any `cfg` other than `test` is now a conditional skip, and the declaration counts.
- **pytest skips that pytest applies**: named marks, `__test__ = False`, `importorskip`, a setup or autouse fixture that skips, a module-level `SkipTest`, `collect_ignore` (each checked against real pytest by the reviewer).
- **"Deleted with its subject" accepted a test helper** (a `factories.py` deleted with its tests) as the subject; a helper never is now, and an unresolved relative import blocks the excuse. The same hole was in M23's JS reader.
- **Quadratic time**: finding code that never runs walked up from every assertion; 4,000 tests in one block took 40 s. It is now one pass per file (`reach::Dead`): 0.6 s.
- **Suppressions**: a narrower one replacing a wider one was reported; a line suppression moved to where it reaches a module was not; ESLint configuration comments, `# pylint: skip-file` and `# pyright: rule=false` were not read. A tuple `assert (x, "msg")`, which always passes, counted.
- **A base policy it cannot read** dropped `testPaths`, which loosened it; it now keeps the valid ones.

## Measuring

`bench/tamper-corpus.json` holds the cases of ADR 0042 and M24's, in four languages; `packages/cli/src/tamper.test.ts` asserts every label and that **every kind of finding has a caught case and every language has not-a-finding cases**; `bench/tamper-rates.mjs` counts them by kind and by language (`bench/RESULTS.md`). The corpus was written together with the guard, by the same hands: its counts show the guard does what this ADR says, **not a measured false-positive or miss rate**. M26 measures that on a corpus that was not written with the guard.

## Known limits

- Net counts still miss a swap: one test deleted while an unrelated one is added, one suppression of a rule removed while another is added elsewhere.
- Python: a skip call that is not literally `pytest.skip`, `pytest.xfail` or `self.skipTest` stops its block only when it is the first statement; named marks and parametrize constants are read from the same file only; `collect_ignore` is read in every Python test file, not only `conftest.py`; a file skipped as a whole (`importorskip`, a module `pytestmark`), like a JS `describe.skip`, gives `test-skipped` when a test is added to it; tests made by a fixture factory or `pytest_generate_tests`, `subTest` loops and doctests are not read.
- Rust: an `assert!` inside another macro's tokens (`proptest! { … }`) is not parsed by tree-sitter and is not counted (a miss when deleted, never a false finding); `panic!` in a `match` arm is not counted as an assertion.
- Rust: replacing `#[cfg(test)] mod tests;` and its file with an inline `mod tests { … }` reads as one test deleted. A test file moved out of what Cargo compiles (`tests/api.rs` to `tests/slow/api.rs`, which nothing declares) keeps its tests in the counts; knowing it would mean following every `mod` and `#[path]`. A `mod foo;` deleted from a file that is not a test module takes `foo.rs`'s unit tests out of the build unseen.
- A subject merged into an existing file under a new name (git sees no rename, and the old names are gone) still excuses its deleted tests.
- The base is local git state: an agent that moves the base ref (`git update-ref`), commits to a local default branch with no remote, or hides a file with `git update-index --skip-worktree` or `--assume-unchanged` changes what is compared. ADR 0042 admits the first; the backstop is CI anchored to the real base (M22, M25).
- Suppressions in other files (`.eslintrc` rules turned off, `ruff` config, `Cargo.toml` `[lints]`) are config: M25.

## Consequences

- An agent that deletes, skips, empties or disables a pytest or Rust test, wraps its assertions in `if False:`, returns before them, swaps them for `assert True`, or silences a lint is named with file and line, in report-only mode from the first run and blocking once `enforced` is on the default branch.
- A repo can protect test files the guard would not find and exempt generated or legacy files, and a change cannot edit either for itself.
- Python and Rust honest edits that keep the counts (rename, move, split, parametrize, reformat, stricter assertions, `#[allow]` to `#[expect]`) are not reported.
