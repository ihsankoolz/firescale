# 0042 — The tamper guard: design, and JavaScript and TypeScript first

Status: accepted (2026-11-13, M23). Builds on ADR 0033 (tree-sitter), ADR 0037 (the base commit) and SPEC §8 ("Tamper guard"). M24 adds Rust and Python and the policy keys; M25 the config and CI detectors; M26 approvals and measured rates.

## Context

A change can make `fscale verify` pass by weakening what verify runs: deleting a test, skipping it, or removing the assertion that failed. An agent under a stop hook has every reason to do exactly that. SPEC §8 asks for a guard that is syntax-tree based, anchored to the base commit, and honest about its error rates.

People and agents also edit tests for good reasons. A guard that reports every honest edit is turned off within a week, so what it does **not** report matters as much as what it does.

## Prior art

Read on 2026-11-13 from each project's own pages.

| Project                                                                              | What it is                                                                                                                                                                                                                                                                                                                                                        | How it works                                                                                                                                                                                                                                                                                                                                            | Differs from firescale                                                                                                                                                                                                                                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **trustbut** (<https://github.com/isAlmogK/trustbut>, Apache-2.0, alpha)             | A CLI and GitHub Action, `trustbut verify`, that writes a PASS/WARN/FAIL "receipt" for a change, also at an agent's turn end. Flags focused tests (FAIL), skipped tests (WARN), net test and assertion removals, changed snapshots, changed test or coverage config, suppressions such as `@ts-ignore`, and tests that also pass before the change. 11 languages. | A "diff-based classifier" over base..head (merge base); no parser is named. It runs the tests too. Net removal counts; "tests merged" (removed in a file that adds tests, with no net loss of assertions) is not reported.                                                                                                                              | It publishes a labelled pilot (71 findings: 5 useful, 57 "true but harmless", 9 wrong, labels drafted by a model), not a measured false-positive rate. firescale counts from syntax trees, so a comment or a string that looks like `expect(` never counts, and its policy is read from the base commit.                                                                                                                 |
| **skylos** (<https://github.com/duriantaco/skylos>, PyPI `skylos` 5.6.0, Apache-2.0) | A dead-code, security and secrets scanner with a "done gate" (`skylos done --base main`) for agent branches: deleted tests, skipped or xfail tests, loosened pytest or Jest settings and coverage floors, tests left with no countable assertion, silenced lint and type checks.                                                                                  | Compares the tree with the base by file bytes; reads settings from the base; JS/TS test files are inventoried with tree-sitter, assertions counted statically. A rewritten test with at least 35% title or body similarity is accepted with advice; a moved test with an identical body is not reported. Runs locally, as an agent stop hook and in CI. | The closest prior art. It matches tests one by one by similarity; firescale compares **net counts** over the whole change, which needs no similarity threshold and treats a rename, a move and a split alike. It publishes no overall false-positive or miss rate (its suites "are regression gates, not independent benchmarks"); firescale publishes the counts on its labelled corpus now and a measured rate in M26. |
| **goodhart** (PyPI `goodhart` 1.2.1)                                                 | A static analyser for reinforcement-learning reward configurations ("Catch reward traps before training").                                                                                                                                                                                                                                                        | —                                                                                                                                                                                                                                                                                                                                                       | Not prior art for this: it shares only the "reward hacking" theme. Named because SPEC §8 lists it.                                                                                                                                                                                                                                                                                                                       |

## Decision

### What it reads

- The **test files the change touches**: the paths that differ between the base commit and the working tree (`blame::changed_since`, so uncommitted and untracked files count, and a deleted file too), that are JavaScript or TypeScript (`.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`), and that are named `*.test.*` or `*.spec.*` or sit in a `__tests__`, `test`, `tests` or `spec` folder.
- Each is parsed **twice with tree-sitter** (the grammars of ADR 0033): as the base commit has it (`git show <base>:<path>`) and as the working tree has it.
- From each tree: every **test** (`it`, `test`, `xit`, `xtest`, `fit`, their `.skip`, `.only`, `.todo`, `.skipIf`, `.fixme`, `.each(…)(…)`, `.concurrent` forms), with its **id**, the titles of the enclosing `describe`/`suite`/`context` blocks and its own joined by `>`; whether it is **skipped** (`.skip`, `.todo`, `.skipIf`, `.fixme`, an `x` prefix, or inside a skipped suite) or **focused** (`.only`, an `f` prefix, or inside a focused suite); and every **assertion**: a call of `expect(…)`, `assert(…)` or `assert.<anything>(…)`, counted once per chain (`expect(a).not.toBe(1)` is one).
- Comments, strings and template literals are not code in the tree, so `// expect(x).toBe(1)` or `"it('x')"` never count.
- A changed test file that tree-sitter cannot read on either side (a syntax error, newer syntax than the grammar knows such as `interface Box<in out T>`, or a file that cannot be read) is a **`not-checked` finding** with the line of the first error. It counts and blocks like any other finding, so it is never taken as clean: otherwise one unreadable line would switch the guard off for that file (M24 asks this of every language). A file that is not valid UTF-8 is read with the bad bytes replaced, never taken as deleted.

### How it compares: net counts over the whole change

Over all the changed test files together:

| Finding             | When                                                                                                           | Where it points                                                                               |
| ------------------- | -------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `test-deleted`      | fewer tests in the working tree than on the base                                                               | each base test whose id is gone, up to the shortfall; file and line on the base commit        |
| `test-skipped`      | more skipped tests than on the base, or more focused tests (`.only` stops the file's other tests from running) | each skipped or focused test that was not so on the base; file and line now                   |
| `assertion-removed` | fewer assertions than on the base                                                                              | each base assertion whose text is gone, up to the shortfall; file and line on the base commit |

A test the change deleted **together with its subject** is left out of the base counts before comparing: it is gone from the working tree, it uses names imported from modules of the repo (`import`, `require`), every one of those modules is deleted in the same change (gone, and not the old side of a rename git sees), and **none of those names is left** in any JavaScript or TypeScript file of the change. The last condition catches a module renamed without git seeing it (`mv` without `git add`): its names live on in the new file, so its tests are still counted. Their assertions are left out too.

### What is reported, and what is not

Each line is a case in `bench/tamper-corpus.json`, in TypeScript or JavaScript.

| Change                                                                                                          | Reported?                                                           | Why                                                   |
| --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------- |
| A test renamed (`"subtracts"` → `"subtracts one number from another"`)                                          | no                                                                  | the count of tests is the same                        |
| A `describe` renamed                                                                                            | no                                                                  | every id changes; the counts do not                   |
| A test moved to another test file                                                                               | no                                                                  | both files are in the change; the totals are the same |
| A test file renamed (`__tests__/math.js` → `__tests__/arithmetic.test.js`)                                      | no                                                                  | a delete and an add of the same tests                 |
| A test split in two                                                                                             | no                                                                  | one test more, the same assertions                    |
| An assertion replaced by a stricter one (`toBe(0)` → `toStrictEqual(0)`, `assert.equal` → `assert.strictEqual`) | no                                                                  | one assertion for one                                 |
| The file reformatted (quotes, indentation, line breaks)                                                         | no                                                                  | the tree's counts do not change                       |
| An assertion moved into a helper function in the same file                                                      | no                                                                  | the file holds as many `expect(` calls                |
| A test deleted together with the module it tests, file and all                                                  | no                                                                  | its subject was deleted in the same change            |
| One test deleted from a file, with the function it tests, the file kept                                         | no                                                                  | the same, per test                                    |
| Tests added; a skipped test turned back on; only non-test code changed                                          | no                                                                  | nothing was taken away                                |
| A test deleted, its subject kept                                                                                | **yes**: `test-deleted`, and `assertion-removed` for its assertions |                                                       |
| A whole test file deleted, its subject kept                                                                     | **yes**: both                                                       |                                                       |
| `it` → `it.skip`, `test` → `test.skip`, `it` → `xit`, `describe` → `describe.skip`                              | **yes**: `test-skipped`                                             |                                                       |
| `it` → `it.only`                                                                                                | **yes**: `test-skipped` (the file's other tests no longer run)      |                                                       |
| A test turned into `it.todo("…")`                                                                               | **yes**: `test-skipped` and `assertion-removed`                     |                                                       |
| An assertion removed, commented out, or turned into a string                                                    | **yes**: `assertion-removed`                                        |                                                       |

Known misses and false positives:

- By design of net counts: a change that deletes one test and adds an unrelated one (the totals hold).
- An assertion weakened in place (`toBe(3)` → `toBeDefined()`, or `expect(x).toBe(3)` → `expect(true).toBe(true)`): M26's approvals of changed expected values are for these.
- Code that never reaches its assertions: `if (false) { it(…) }`, an early `return` before them. The counts are of the code as written, not as run.
- Assertions and tests the reader does not know: `should`, `t.is`, tests made by a wrapper function, a test aliased through a variable (`const t = it.skip; t(…)` reads as a deleted test, not a skipped one).
- False positives: an assertion moved into a helper module outside the test folders counts as removed; tests made in a `for` loop count once; an `each` table that is not an array literal counts as one row.
- The guard runs only with a base (ADR 0037): an agent that moves the base ref (`git update-ref`) or works on a default branch with no upstream has none, and neither has a shallow clone without the merge base. The guard does not run then, and a passing run does not say so; M25's CI detector and the base-anchored CI of M22 are the backstop.

### The base and the policy

- The base is the one blame-aware verify already resolves (ADR 0037): the merge base with `--base` or the default branch. With no base (outside git, a clean default branch with no upstream), the guard does not run.
- **The policy is read from the base commit**: `verify.tamper` in the base commit's `firescale.config.json` (`git show <base>:./firescale.config.json`), never the working tree's. A change that sets `"tamper": "off"` does not switch the guard off for itself, and one that sets `"enforced"` does not enforce it on itself; either takes effect once merged. A base with no such key, or with a `firescale.config.ts`, is `evaluated`.
- The working tree's `verify.tamper` is still checked for a known value (`off`, `evaluated`, `enforced`), so a typo fails loudly.

### Report-only by default

Like every rule (`evaluated` by default, SPEC §10), findings are **reported and do not fail verify** unless the base commit's config says `"tamper": "enforced"`. Then a run that would pass fails "at the tamper guard", exits 1, and the stop hook blocks the agent. Report-only findings are still in the headline (`verify: OK - tamper guard: 3 report-only findings`), so a run with them never reads as a plain OK.

### Output

The summary lists each finding with its kind, file and line (marked "on the base commit" for a deletion), the net counts, and the tests left out because their subject was deleted. `fscale verify --json` has a `tamper` object: `policy`, `base`, `files`, `counts`, `findings` (`kind`, `file`, `line`, `onBase`, `what`) and `notChecked` (verify schema 6, a new optional field).

### Tree-sitter as a dependency

ADR 0033 took tree-sitter with the Python, TypeScript and TSX grammars for M17, and measured it. M23 adds **no grammar** (the TSX grammar reads JavaScript); the guard is about 800 lines of Rust, tests included. Binary size and clean build time before and after are in `bench/RESULTS.md`. M24 adds the Rust grammar and measures again.

## After the review

An independent review before the pull request reproduced two bypasses and several smaller problems, all fixed with tests and corpus cases:

- A changed test file that tree-sitter could not read dropped out of the counts with only a note, so one line of newer TypeScript next to a deleted test made `enforced` pass. It is now a `not-checked` finding that counts and blocks.
- "Deleted with its subject" also excused a subject that was renamed. Renames git sees no longer count as deletions, and a subject whose names live on in the change is never deleted.
- `it.fails`, `.runIf`, `test.fail`, `expect.soft`, `expect.poll`, `expect.assertions` and `expect.hasAssertions` were not read; they are now.
- A call chain of thousands of `.then(…)` overflowed the stack; the callee walk is now a bounded loop.
- A UTF-8 byte order mark in the base config silently downgraded `enforced`; it is stripped.
- Turning tests into an `it.each` table, and Playwright's hooks and steps, gave false findings.
- A non-UTF-8 test file read as deleted.
- A new `.only` named the wrong test.
- A folder could stand for a module.

## Measuring

`bench/tamper-corpus.json` holds 37 labelled changes: 19 that must give a finding (each of the four kinds in JavaScript and in TypeScript) and 18 that must not (every honest edit in the table above). `bench/tamper-rates.mjs` runs each in a real git repo through `fscale verify --json` and counts caught, missed and false positives; `packages/cli/src/tamper.test.ts` asserts every label. The corpus was written together with the guard, so its counts show the guard does what this ADR says, not how often it is wrong on real changes: M26 builds a corpus that is not, and measures the false-positive and miss rates on it.

## Consequences

- An agent that deletes, skips or empties a test is named in the summary, with file and line, from the first run; with `"tamper": "enforced"` merged into the default branch, the stop hook sends it back.
- An honest refactor of tests is not reported as long as it keeps the counts: rename, move, split, reformat, stricter assertions.
- The guard reads only the changed test files, so it adds a few milliseconds to verify.
