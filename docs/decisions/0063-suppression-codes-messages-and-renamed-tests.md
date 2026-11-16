# 0063 — Suppression codes, failure messages and renamed tests in the tamper guard

Status: accepted (2026-11-16). Refines ADR 0044 (suppressions, keyed `tool:rule`), ADR 0046 (`expected-changed`) and ADR 0042 (net counts over the whole change).

## Context

The 0.5.1 review ran the tamper guard on 150 commits of the course project (`docs/reviews/cs464-0.5.1.md`, "Tamper guard on 150 commits of main"). It gave 61 findings: 58 `suppression-added` and 3 `expected-changed`. 15 of the 58 and all 3 of the 3 were false.

- **Finding 3.** `# noqa: E402  (after the backend is chosen)` gave six findings: `noqa:E402`, then `noqa:(after`, `noqa:the`, `noqa:backend`, `noqa:is` and `noqa:chosen)`. `codes()` in `suppress.rs` split the text after `noqa:` at commas and white space and kept every word. The same split served `pylint: disable=` and `# nosec`.
- **Finding 4, two of three.** `expected-changed` compared an assertion's whole text with its literals masked. A changed failure message is a changed literal, so `pytest.fail("… the publish …")` → `pytest.fail("… a publish …")` and `assert idle == 0, f"… the gate's call"` → `assert idle == 0, f"… the refresh"` were taken for changed expected values.
- **Finding 4, the third.** A test was renamed and lost `assert upstream.calls == 1`; a new, different test of the same file had `assert upstream.calls == 0`. Every gone test was taken for a possible rename of every new test (M26), so the two were paired as one changed value. The real change, an assertion dropped from the renamed test, was not reported: the new test's assertions made up the count (ADR 0042).

## Decision

Each item first got a failing test built from the review's exact output, then the smallest fix that made it pass. The cases went into `bench/tamper-corpus.json`. The held-out `bench/tamper-cheats.json` was only run for its numbers; its cases were not read.

### 1. A code list ends at the first word that is not a code

After `noqa:`, `pylint: disable=` (and `disable-next=`) and `nosec`, the words are read in order, split at commas and white space, and the list ends at the first word that is not a code of that tool. Words after it, such as a trailing comment, are never codes.

The shape of a code, by tool:

| Tool                          | A code is                                                                                                                                                                                                                                                                                       | Source                                                                                                                                                                                                                                                                     |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ruff and flake8 (`# noqa:`)   | uppercase ASCII letters, then digits: `E402`, `F401`, `PLC0415`.                                                                                                                                                                                                                                | Ruff's docs: codes are "sequences of uppercase ASCII characters followed by ASCII digits, separated by whitespace or commas. The list ends at the last valid code." flake8's docs: `# noqa: E731,E123`; "contents before and after the `# noqa: ...` portion are ignored". |
| pylint (`# pylint: disable=`) | a message id (`C0114`: one of `C R W E F I` then digits, or the letter alone, a category), or a name of lowercase letters, digits, `-` and `_` (`unused-import`, `typecheck`, `all`).                                                                                                           | pylint's docs ("Messages control"): a message may be given by symbolic name, numerical id, checker or group name, category letter, or `all`. Its pragma parser stops at `;` or `#`, as in the docs' example `disable=no-member; applies only to this line`.                |
| bandit (`# nosec`)            | `B` then digits (`B602`), a lowercase name with `_` (`assert_used`), or one of bandit's 13 test names of one word (`pickle`, `marshal`, `md5`, `ciphers`, `eval`, `httpsconnection`, `random`, `telnetlib`, `ftplib`, `input`, `tempnam`, `hashlib`, `trojansource`). A leading `:` is allowed. | bandit's docs: `# nosec B602, B607`, `# nosec assert_used`. Its regex `(?:(B\d+\|[a-z\d_]+),?)+` reads lowercase words too; a `# nosec` with no known test suppresses every test, which the guard keys `nosec:*` as before.                                                |

For pylint, commas separate items and a second word inside one item ends the list (`disable=unused-import because of x` is one code). This is stricter than pylint's own tokenizer, which reads every word: the simplest reading that keeps a comment's words out.

The other splitters were read and left alone, as no failing test showed a mistake there: mypy's `disable-error-code` and bracketed lists (`# type: ignore[a, b]`, `# pyright: ignore[a]`), eslint's `-- reason`, Rust's `allow(…)`, Go's gosec (already ended at the first word that is not `G` and digits), ktlint, SwiftLint, C#'s `#pragma warning disable`, ReSharper, Dart's `ignore:` and cppcheck.

### 2. A failure message is not an expected value

Each assertion now carries what it checks without its failure message (`Assertion::checked`). `expected-changed` pairs on that, and an assertion whose only change is in its message is unchanged. Which arguments are the message comes from each framework's documented signature, by the name of the call:

- **Python:** the message after `,` of an `assert`; unittest's `msg` (after 1 value for `assertTrue`, `assertFalse`, `assertIsNone`, `assertIsNotNone`; after 2 for `assertEqual`, `assertIn`, `assertAlmostEqual` and the rest of the two-value asserts); numpy.testing's `err_msg` (after 2). `msg=` and `err_msg=` by name too.
- **JS and TS:** `expect(value, message)` (Vitest, Jest with a message, Playwright); node's `assert(value, message)`, `assert.ok`, `assert.equal` and its family (after 2); chai's `assert.isTrue` and the like (after 1) and `assert.equal`, `include`, `lengthOf`, `match` (after 2).
- **Rust:** the format and its arguments after 1 value (`assert!`, `debug_assert!`, `prop_assert!`) or 2 (`…_eq!`, `…_ne!`, `…_matches!`), when the argument there starts as a string.
- **Go:** testify's `msgAndArgs` after the values each assertion takes (1 for `True`, `Nil`, `NoError` and the like, 2 for `Equal`, `Contains`, `Len`, `ErrorIs` and the like, 3 for `InDelta`, `InEpsilon`, `WithinDuration`, `Eventually`), the same for its `…f` forms, after `t` for the package functions; gomega's description after the matcher.
- **Java:** JUnit 5 and TestNG's trailing `message` (after 1 or 2 values); AssertJ's `as`, `describedAs`, `withFailMessage`, `overridingErrorMessage`; the message after `:` of an `assert`.
- **Kotlin:** kotlin.test's `message` (after 1 or 2 values, or by name), and AssertJ's descriptions.
- **Swift:** XCTest's `message` (after 1 or 2 values); `#expect` and `#require`'s comment.
- **Dart:** `reason:` of `expect` and `expectLater`.
- **C#:** NUnit's and MSTest's `message` (after 1 or 2 values, `Assert.That` after 1); xUnit's `userMessage` of `Assert.True` and `Assert.False`; FluentAssertions' `because`. xUnit's `Assert.Equal` takes no message.
- **C and C++:** the last argument of Unity's `…_MESSAGE` forms; Check's `ck_assert_msg` format. GoogleTest's `<< "…"` was never part of the call.

A message is the first argument at or after that place that is written as a string, with what follows it. An argument there that is not a string (a `delta`, `places`, a comparer) is a value, so it still counts.

**A call that is all message** (`pytest.fail`, `self.fail`, `assert.fail`, Go's `t.Error`/`t.Errorf`/`t.Fatal`/`t.Fatalf`, testify's `Fail`, `fail` in Java, Kotlin and Dart, `XCTFail`, `Assert.Fail`, `FAIL`, `TEST_FAIL_MESSAGE`, `ck_abort_msg`) checks the condition of the `if` whose branch it is (`!(…)` in an `else`). So `if got != 3 { t.Errorf("…") }` → `if got != 4 { … }` is a changed expected value, and a changed message is not. Before this ADR such a change was caught only when the message also spelled the value; without the `if`, a value change there would now be missed.

### 3. Assertions are paired only within a test and its rename, and a renamed test's losses are named

- **Renames.** A test of the base that is gone from a file is taken as renamed to the new test of the same file that shares the most assertions with it, by what they check with literals masked. Ties go to the two names with the longest common start and end, then to the ids. A gone test that shares no assertion with any new test was not renamed. Each new test is the rename of at most one gone test. Past 40,000 (gone, new) pairs in one file, no rename is followed.
- **`expected-changed`** pairs a gone assertion with a new one only in the same test, or in a test and its rename. A gone test that was not renamed pairs with nothing.
- **A renamed test that lost assertions.** For each rename, the assertions the old test had and the new one does not are named `assertion-removed` (on the base line, `… (in <old>, renamed <new>)`), up to the number it is short. One kept as it was, or with other values (`expected-changed`), is not a loss. Nor is one that now sits in another test of the file more often than it did on the base (it moved). Tests excused because their subject was deleted with them stay excused. A loss already named by the net count is not named twice.

So the review's third case is one `assertion-removed`, `assert upstream.calls == 1 (in test_a_trade_holds_no_connection_while_the_gate_waits_on_market_service, renamed test_a_trade_holds_no_connection_while_the_gate_refreshes_the_copy)`, and no `expected-changed`. Moving or renaming a test, with what it checks, is still not a finding.

## Consequences

- On the course project's 150 commits the guard gives 45 findings where it gave 61: the 15 comment words and the 3 false `expected-changed` are gone, and two assertions that renamed tests lost are named, the review's `assert upstream.calls == 1` and `assert served[0] is served[1]` in `test_auth_client_lifespan.py`, which the review had not seen (`bench/RESULTS.md`).
- `expected-changed` needs a person's approval (ADR 0046). Fewer false ones means fewer approvals asked for nothing.
- Before and after numbers on the corpus and the held-out cheats are in `bench/RESULTS.md`.

## Limits

- **Message tables are by name.** A wrapper or a framework not listed keeps its message in what it checks, as before: a changed message there is still `expected-changed`.
- **A message that is not written as a string** (a variable, `String.format(…)`, a lambda) is taken for a value.
- **JUnit 4 puts the message first** (`assertEquals(message, expected, actual)`). That message is not left out, and when the actual value there is written as a string it is taken for the message.
- **`failed_when` looks up at most six levels**, through statements and blocks only. A fail call in a loop, a `switch` or a helper keeps only its call, so a changed value there is now missed where its message spelled the value.
- **When a renamed test lost several assertions and is short fewer**, the ones named are the first in the file, not always the ones that went.
- **Rename following is a guess.** Two tests renamed and rewritten at once may be paired the wrong way; the effect is a misnamed `assertion-removed`, or an `expected-changed` missed because its two assertions were not taken for one test. The net count of `assertion-removed` (ADR 0042) is not changed by it.
- **Losses are checked per test only for renamed tests.** A test that keeps its name and loses an assertion while a new test of the file gains one is still covered by the net count (ADR 0042), as before. Doing the same for every kept test would name assertions moved between tests.
- **pylint's own reading is wider.** `disable=unused-import unused-variable` (two names with no comma) is one code here; pylint reads two. `bandit`'s list stops at a word bandit would log and ignore.
- The one-word bandit names are from bandit's current plugin list; a test added later with a one-word name is not read.

## Not done

- No new detector. Nothing about other kinds of finding changed.
- The base run, hooks, CI and the version were not touched.
