# 0052 — The tamper guard in Java, Kotlin, Swift, Dart, C, C++, C# and Go

Status: accepted (2026-11-14, M32). Extends ADR 0042, 0044 and 0046; nothing in them is reversed.

## Context

M31 (ADR 0051) found Java, Kotlin, Android, Swift, Xcode, C, C++, .NET and Dart projects and ran their tools, but the tamper guard still read only JavaScript, TypeScript, Python and Rust test files: an agent could delete a JUnit test or comment out an XCTest class and `fscale verify` would not see it. The owner asked to go "as deep as the options go" on these languages: project support (M31), then the tamper guard (this milestone), then starter packs (M33).

The guard compares syntax trees of the base and the change (ADR 0042). A new language needs a tree-sitter grammar, so this milestone adds dependencies, which AGENTS.md asks to record here.

## Decision

### A reader per language

Each reader gives the same `TestFile` (tests with ids, weights and skips; assertions; imports; suppressions), so the comparison of ADR 0042, the policy of ADR 0044 and the approvals of ADR 0046 are unchanged. The module comment of each reader says exactly what it reads; in short:

| Language | Module       | Tests                                                                                                                   | Skipped                                                                                                    | Assertions                                                                                                     | Suppressions                                                                                                              |
| -------- | ------------ | ----------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| Java     | `java.rs`    | JUnit 3, 4 and 5, TestNG; `@ParameterizedTest` weighed by its literal rows                                              | `@Disabled`, `@Ignore`, JUnit 5 conditions, `enabled = false`, `assume…(false)` first, an empty body       | `assert…` (JUnit, TestNG, AssertJ, Hamcrest), `fail`, Mockito `verify`, `assert`                               | `@SuppressWarnings`, `@SuppressFBWarnings`, `// NOPMD`, `// NOSONAR`, Checkstyle comments                                 |
| Kotlin   | `kotlin.rs`  | `@Test` (JUnit, kotlin.test), Kotest specs and containers                                                               | `@Ignore`, `@Disabled`, Kotest `x…`, `!name`, `enabled = false`, `enabledIf`                               | `assert…`, `assertFailsWith`, `fail`, Kotest `should…`, MockK and Mockito `verify`                             | `@Suppress`, `@file:Suppress`, `@SuppressWarnings`, `@SuppressLint`, `// ktlint-disable`                                  |
| Swift    | `swift.rs`   | XCTest `test…` methods of `XCTestCase` subclasses, Swift Testing `@Test` with `arguments:` rows                         | `XCTSkip…` first (also in `setUp`), `.disabled`, `.enabled(if:)`, an empty body                            | `XCTAssert…`, `XCTFail`, `XCTUnwrap`, `#expect`, `#require`; not inside `withKnownIssue` or `XCTExpectFailure` | `// swiftlint:disable…` and its line forms, `// swift-format-ignore[-file]`                                               |
| Dart     | `dart.rs`    | `test`, `testWidgets`, `testGoldens` in `group`s                                                                        | `skip:`, `@Skip`, `markTestSkipped` first; `solo:` focuses                                                 | `expect`, `expectLater`, `expectAsync…`, `fail`, mockito and mocktail `verify…`                                | `// ignore:`, `// ignore_for_file:` (`type=lint` is every rule)                                                           |
| C, C++   | `cfamily.rs` | GoogleTest, Catch2, doctest; Unity, CMocka, Check                                                                       | `DISABLED_`, `GTEST_SKIP`, hidden tags, `doctest::skip`, `TEST_IGNORE`, `skip()`, `#if 0`, any other `#if` | `EXPECT_*`, `ASSERT_*`, `REQUIRE*`, `CHECK*`, `TEST_ASSERT*`, `assert_*`, `ck_assert*`, `assert`               | `NOLINT…`, `#pragma … diagnostic ignored`, `#pragma warning(disable: …)`, `// cppcheck-suppress`                          |
| C#       | `csharp.rs`  | xUnit, NUnit, MSTest; `InlineData`, `TestCase`, `DataRow` rows                                                          | `Skip =`, `[Ignore]`, `[Explicit]`, `Assert.Ignore()` and kin first, `#if`                                 | `Assert.…` and its kin, FluentAssertions, Shouldly, Moq `Verify…`, Verify, NSubstitute, FakeItEasy             | `#pragma warning disable`, `[SuppressMessage]`, `[UnconditionalSuppressMessage]`, ReSharper comments, `#nullable disable` |
| Go       | `go.rs`      | `func TestX(t *testing.T)`, `t.Run` subtests with literal names, table rows, testify suites, examples with `// Output:` | `t.Skip…` first, a `//go:build` constraint                                                                 | `t.Error…`, `t.Fatal…`, `t.Fail…`, testify and gotest.tools `assert`/`require`, quicktest, gomega              | `//nolint`, `//lint:ignore`, `//lint:file-ignore`, `// #nosec`, `//revive:disable`                                        |

What is the same in every language, as for the first four:

- **Ids** are the names around a test and its own (`CartTests::testTotal`, `TestTotal/empty`), never the file, so a test moved between files is not a deletion.
- **Code that never runs** (`if (false)`, after a `return`, under `#if 0`) and **assertions that cannot fail** (`assertTrue(true)`, `XCTAssertEqual(1, 1)`, `expect(true, isTrue)`) do not count (ADR 0044). A test whose body does nothing is skipped.
- **Subjects**: a test deleted with the code it tests is not a finding (ADR 0042). Java and Kotlin resolve imports and same-package classes in the usual source folders; Swift `@testable import` modules in `Sources/`; Dart `package:` imports in `lib/`; C and C++ quoted includes by stem; Go the file a test file is named after and its package. C# namespaces are not resolved to files, so a C# test deleted with its subject still counts.
- **Suppressions** are keyed `tool:rule`, and the net-count rules of ADR 0044 and 0046 apply unchanged (narrower for wider, copies of base lines in new files).

### Parse errors that cannot hide a test

ADR 0042 makes a test file that tree-sitter cannot read a finding of its own (`not-checked`): a change could hide anything in it. Real C, C++, Swift and C# files hit errors that are not in the change at all: tree-sitter does not expand macros (`int CJSON_CDECL main(void)`, `FMT_BEGIN_NAMESPACE`), cannot read an `#if` that gives one block two heads (Humanizer's `#if NET5_0_OR_GREATER` around two `foreach` lines), and has quirks of its own (a missing `!` in some Swift `!=`). On the first run, 116 of 516 findings were `not-checked`, and 89 of them were labelled false: in fmt, 66 of 68 findings.

`crates/core/src/tamper/recover.rs` now tells harmful errors from harmless ones, for the eight new languages:

- **An error node is harmless when none of its words could name a test, a skip or an assertion** (`test`, `assert`, `expect`, `check`, `require`, `verify`, `should`, `skip`, `ignore`, `disable`, `fail`, `fatal`, `error`, `fact`, `theory`, `suite`, `section`, `xct` in any word, and `it`, `describe`, `run`, `t`, `given`, `when`, `then` as whole words). The list is generous on purpose: a word found keeps the file `not-checked`, as before. fmt's `TEST(ostream_test, template)` (a keyword passed to a macro) still is.
- **A missing token is harmless unless it is a closing `}`, `)` or `]`**: a `;` after a macro call, a type name, Swift's `!`. A missing closing bracket is code cut off or unbalanced, which macros do not cause; such a file is still not read.
- **C and C++: a macro alone on its line is blanked before parsing** (`FMT_BEGIN_NAMESPACE`, `G_BEGIN_DECLS`, `FMT_PRAGMA_GCC(diagnostic ignored "-Wx")`): an upper-case name with a `_`, naming no test or assertion, its arguments closed on the same line, outside parentheses, not continuing a macro, and not followed by a `{` on the next line. Blanking keeps every line where it was.
- **C, C++, C# and Swift: a file that still cannot be read is read again by the first branch of each `#if`** (every `#elif`, `#elseif` and `#else` branch blanked), when no branch left out names a test, a skip or an assertion. The reading is used only if it has no harmful error.

### Naming the assertion that went

Found on real history: Alamofire commented out the first of several identical assertions, and the guard named the last copy, which was still there (46 findings at the wrong line in one commit). A removed assertion is now paired with what is left in its own test first, then anywhere. The counts did not change, only which lines are named; this is shared by every language.

### Dependencies

Eight grammar crates, each pinned exactly in the workspace `Cargo.toml`, all MIT-licensed, all built from C sources by `cc` as the existing grammars are: `tree-sitter-java` 0.23.5, `tree-sitter-kotlin-ng` 1.1.0 (the Kotlin grammar maintained under tree-sitter-grammars), `tree-sitter-swift` 0.7.4, `tree-sitter-c` 0.24.2, `tree-sitter-cpp` 0.23.4, `tree-sitter-c-sharp` 0.23.5, `tree-sitter-go` 0.25.0 and `tree-sitter-dart` 0.3.0. Each is loaded by `tree-sitter` 0.26.3 through `tree-sitter-language`, and every reader's tests parse with it.

## Proof

- **Unit tests** in each reader (`crates/core/src/tamper/{java,kotlin,swift,dart,cfamily,csharp,go}.rs`) and in `recover.rs`, on fixtures written from each framework's documentation.
- **The labelled corpus** (`bench/tamper-corpus.json`, run by `node bench/tamper-rates.mjs` and by `packages/cli/src/tamper.test.ts`): 439 cases, 184 of them in the new languages (15 to 27 each), a caught case for every kind of finding and not-a-finding cases in every language; 0 missed, 0 false positives.
- **Real history, measured as in M26** (ADR 0046): the last 125 non-merge commits of one public repo per language (Alamofire/Alamofire, DaveGamble/cJSON, Humanizr/Humanizer, apache/commons-lang, dart-lang/http, fmtlib/fmt, spf13/cobra, square/okhttp), shallow clones, read-only, nothing of theirs run. Every finding was labelled true or false by a fresh subagent given the commit message, file list and the diff of the files named (a diff of over 1,500 lines trimmed to the hunks holding the claims, with how often the claimed text appears on removed lines elsewhere), the claim in neutral words, and ADR 0046's brief; a `not-checked` finding was true when the change to that file weakens a test. Ten labellers, 724 labels, in `bench/tamper-history-m32.json`; `node bench/tamper-history-report.mjs --data bench/tamper-history-m32.json` prints the tables in `bench/RESULTS.md`.

| Language | Repo                | Findings before → after | False before → after | False-positive rate before → after | Commits with a false finding before → after |
| -------- | ------------------- | ----------------------- | -------------------- | ---------------------------------- | ------------------------------------------- |
| Swift    | Alamofire/Alamofire | 294 → 298               | 57 → 6               | 19.4% → 2.0%                       | 8 → 2 of 125                                |
| C        | DaveGamble/cJSON    | 34 → 2                  | 27 → 1               | 79.4% → 50.0%                      | 24 → 1 of 125                               |
| C#       | Humanizr/Humanizer  | 34 → 22                 | 16 → 5               | 47.1% → 22.7%                      | 9 → 3 of 125                                |
| Java     | apache/commons-lang | 2 → 2                   | 1 → 1                | 50.0% → 50.0%                      | 1 → 1 of 125                                |
| Dart     | dart-lang/http      | 38 → 38                 | 2 → 2                | 5.3% → 5.3%                        | 1 → 1 of 125                                |
| C++      | fmtlib/fmt          | 68 → 100                | 49 → 18              | 72.1% → 18.0%                      | 42 → 11 of 125                              |
| Go       | spf13/cobra         | 13 → 13                 | 0 → 0                | 0.0% → 0.0%                        | 0 → 0 of 125                                |
| Kotlin   | square/okhttp       | 33 → 33                 | 0 → 0                | 0.0% → 0.0%                        | 0 → 0 of 125                                |
| **All**  | 1,000 commits       | **516 → 508**           | **152 → 33**         | **29.5% → 6.5% (4.7–9.0%)**        | **85 → 19, 8.5% → 1.9% (1.2–2.9%)**         |

By kind, after: `assertion-removed` 14 false of 333 (4.2%), `test-deleted` 4 of 85 (4.7%), `suppression-added` 0 of 43, `expected-changed` 12 of 40 (30.0%), `test-skipped` 1 of 3, `not-checked` 2 of 2, `ci-weakened` 0 of 2.

- **What the fixes cost**: 27 findings labelled true went away, all `not-checked`. 19 of those changes are now reported as what they are (an assertion removed, a test deleted) in the same file; 8 are not reported now, each a weakening the guard reads in no language: an expected value that is a name (`NULL` to `end` in cJSON), a helper's checks loosened, expected values computed at run time, one assertion traded for a different one. They were found before only because the file could not be read at all.
- **M26's repos again**, on the same 1,200 commits, with `main`'s guard and M32's: the shared fixes remove 3 `test-skipped` findings M26 labelled false and name 22 assertion removals at the right lines (19 of them labelled false where `main` named them; 22 of the 23 M32 names are true, by a fresh labeller). Over labelled findings, false findings fall from 20.9% (17.0–25.5%) to 14.7% (11.4–18.9%); `bench/RESULTS.md` has the table.
- **M26's held-out cheating corpus** (JavaScript, TypeScript, Python, Rust and config) gives the same result as in M26: 48 caught, 8 approval only, 42 missed of 98. The shared changes (naming the assertion that went, new skipped tests) change no count there.

## Not checked, and limits

- **One repo per language, 125 commits each**: a rate is that repo's style more than its language's. Java (commons-lang) and C (cJSON) had two findings each after the fixes; their rates say almost nothing.
- **Labels**: one subagent per finding, no second labeller, no human check, the same model family that wrote the guard. Two briefs differed slightly: the labellers of the 46 relocated Alamofire findings and of four Humanizer findings were told that a claim naming a line the commit did not change is false, which the first labellers had applied on their own.
- **The after rate is optimistic**: the fixes were made against these commits.
- **What stays false** (after): `expected-changed` pairs assertions with every literal masked, so it cannot tell a changed input from a changed expected value (`Assert.Equal("IWillBreakStuff", "i.will_break-stuff".Pascalize())` with a new input), or a failure message from a value (`TEST_ASSERT_EQUAL_STRING_MESSAGE`); telling them apart needs each framework's argument order, and the kind only asks for an approval. An assertion rewritten to an equivalent (`assertAll(…)` unwrapped, `wrappedValue` to `read { $0 }`) and tests merged into one are counted as removed and deleted, as in every language (ADR 0046). fmt's `TEST(ostream_test, template)` and a module test that throws inside `EXPECT_THROW` still cannot be read.
- **Not seen**: tests made in loops (Kotest `withData`, Go subtests with computed names, C++ `INSTANTIATE_TEST_SUITE_P` with generators), tests registered at run time, and C# subjects (namespaces are not resolved to files, so a test deleted with the class it tests still counts).
- **No cheating corpus for the new languages**: their misses were not measured. The readers were written from the frameworks' documentation; none of the frameworks was run here, so a framework's real behaviour (which `#if` a build defines, which `skip` a runner honours) is assumed, not observed.
- **Swift, C#, Dart and Kotlin tools did not run** on these files: the trees are tree-sitter's, which can differ from the compiler's on new syntax.

## Needs your decision

- **The binary grew from 11.5 MB to 31.0 MB** (+19.5 MB, +170%), almost all of it the eight grammars' parse tables (C# 5.4 MB, Swift 4.3 MB, Kotlin 3.5 MB, C++ 3.5 MB). It is downloaded once per platform with the npm package, and start-up does not change: `fscale --version`, 50 runs each with `bench/startup.mjs`, p50 2.9 ms before and 2.8 ms after. If the size matters, the choices are: keep it (what this milestone does); build the grammars as an optional Cargo feature and publish two packages; or load grammars as WebAssembly on demand, which needs tree-sitter's `wasm` feature and a runtime (a larger change, and slower). Nothing is published until a release is tagged, which is yours to do.
- **`not-checked` is still a blocking finding when enforced.** It is rare after these fixes (2 in 1,000 commits, both false), but each blocks an agent that cannot do anything about tree-sitter's grammar. The alternative is to report it without blocking.

## Consequences

- An agent that deletes, skips or weakens tests in a Java, Kotlin, Android, Swift, Dart or Flutter, C, C++, .NET or Go project is reported by `fscale verify` and the stop hook, as in JavaScript, TypeScript, Python and Rust.
- A ninth language is a reader module, its corpus cases and a history run; the comparison does not change.
- The binary is about 2.7 times its size before (see "Needs your decision").
- The history data and labels of M32 are kept apart from M26's, in `bench/tamper-history-m32.json`.
