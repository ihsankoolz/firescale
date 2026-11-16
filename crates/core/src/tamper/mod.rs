//! The tamper guard (M23, M24, M32; ADR 0042, ADR 0044, ADR 0052): finds a
//! change that passes verify by weakening its tests or silencing its
//! checks, in JavaScript, TypeScript, Python (pytest and unittest), Rust,
//! Java (JUnit and TestNG), Kotlin (JUnit, kotlin.test and Kotest), Swift
//! (XCTest and Swift Testing), Dart (`package:test` and `flutter_test`), C
//! (Unity, CMocka, Check, `assert`), C++ (GoogleTest, Catch2, doctest), C#
//! (xUnit, NUnit, MSTest) and Go (`testing`, testify).
//!
//! - It compares syntax trees, not diff lines: each changed test file is
//!   parsed with tree-sitter on the base commit and in the working tree, and
//!   its tests (by suite path and name), skipped tests and assertions are
//!   counted. A reformatted test, or one moved within or between files,
//!   counts the same. Code that certainly never runs (`if (false)`, after a
//!   `return`, C's `#if 0`) and assertions that cannot fail
//!   (`expect(true).toBe(true)`, `assert True`, `EXPECT_TRUE(true)`) are
//!   not counted. C and C++ test macros (`TEST(…)`, `TEST_CASE(…)`) are
//!   read in the shapes tree-sitter gives them.
//! - It compares **net counts over the whole change**, so a renamed test, a
//!   test moved to another file, a test split in two and an assertion
//!   replaced by a stricter one are not findings. Lint suppressions
//!   (`eslint-disable`, `# noqa`, `#[allow]`, `// swiftlint:disable`,
//!   `// ignore:`, `// NOLINT`, `#pragma … diagnostic ignored`) are counted
//!   the same way, per rule, in every changed source file.
//! - A test deleted together with every module of the repo it uses (its
//!   subject; in C and C++ the headers its file includes in quotes) is not
//!   a finding.
//! - The base is the one blame-aware verify resolves (ADR 0037). The policy
//!   (`verify.tamper`: mode, extra test paths, allowed exceptions) is read
//!   from the base commit's config, never the working tree's, so a change
//!   cannot loosen the guard for itself.
//! - Findings are report-only unless the base's policy is `enforced`.

pub mod approval;
mod cfamily;
mod ci;
mod config;
mod coverage;
mod csharp;
mod dart;
mod formats;
pub mod github;
mod go;
mod java;
mod js;
mod kotlin;
mod lintconfig;
mod noverify;
mod policy;
mod python;
mod reach;
mod recover;
mod rust;
mod suppress;
mod swift;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Serialize;
use tree_sitter::Node;

use crate::blame::BaseCommit;
use crate::git::run_git;
use crate::imports::{js_dialect, JsDialect};

pub use policy::{Allowance, TamperPolicy};
pub use suppress::{suppressions, Suppression};

/// The languages the guard reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// JavaScript or TypeScript, with the grammar to read it with.
    Js(JsDialect),
    /// Python: pytest and unittest.
    Python,
    /// Rust: `#[test]` and its forms.
    Rust,
    /// Java: JUnit and TestNG (M32).
    Java,
    /// Kotlin: JUnit, kotlin.test and Kotest (M32).
    Kotlin,
    /// Swift: XCTest and Swift Testing (M32).
    Swift,
    /// C: Unity, CMocka, Check, plain `assert` (M32).
    C,
    /// C++: GoogleTest, Catch2 and doctest (M32). Headers too: the C++
    /// grammar reads C.
    Cpp,
    /// C#: xUnit, NUnit and MSTest (M32).
    CSharp,
    /// Go: `testing` and testify (M32).
    Go,
    /// Dart: `package:test` and `flutter_test` (M32).
    Dart,
}

impl Language {
    /// The language of a file, from its extension.
    #[must_use]
    pub fn of(path: &str) -> Option<Self> {
        if let Some(dialect) = js_dialect(path) {
            return Some(Self::Js(dialect));
        }
        let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "py" => Some(Self::Python),
            "rs" => Some(Self::Rust),
            "java" => Some(Self::Java),
            "kt" => Some(Self::Kotlin),
            "swift" => Some(Self::Swift),
            "c" => Some(Self::C),
            // A header may be C or C++: the C++ grammar reads both.
            "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" => Some(Self::Cpp),
            "cs" => Some(Self::CSharp),
            "go" => Some(Self::Go),
            "dart" => Some(Self::Dart),
            _ => None,
        }
    }
}

/// Reads the tests, skips and assertions of a test file.
#[must_use]
pub fn read_test_file(source: &str, language: Language) -> TestFile {
    let file = read_as_written(source, language);
    // An `#if` that tree-sitter cannot read: read the first branch of each,
    // when the others hold no test (M32, ADR 0052).
    let has_branches = matches!(
        language,
        Language::C | Language::Cpp | Language::CSharp | Language::Swift
    );
    if file.has_errors && has_branches {
        if let Some(first) = recover::first_branches(source) {
            let retry = read_as_written(&first, language);
            if !retry.has_errors {
                return retry;
            }
        }
    }
    file
}

/// Why `why` says the tests of a Dart file not named `*_test.dart` do not
/// run.
const DART_NOT_A_TEST_FILE: &str = "dart test runs only files named *_test.dart";

/// Reads a test file, and what its name says about its tests (found in
/// review): a Go file named for one platform (`calc_linux_test.go`) is built
/// only there, a conditional skip like `//go:build linux`; `dart test` runs
/// only files named `*_test.dart`, so the tests of any other file (a helper
/// that a test file calls) run only through one, and count as not run on
/// both sides.
fn read_named(path: &str, source: &str, language: Language) -> TestFile {
    let mut file = read_test_file(source, language);
    let name = path.rsplit('/').next().unwrap_or(path);
    match language {
        Language::Go if go::platform_only(path) => {
            for test in &mut file.tests {
                test.skipped = true;
            }
        }
        Language::Dart if !name.ends_with("_test.dart") => {
            for test in &mut file.tests {
                if !test.skipped {
                    test.skipped = true;
                    test.why = Some(DART_NOT_A_TEST_FILE);
                }
            }
        }
        _ => {}
    }
    file
}

fn read_as_written(source: &str, language: Language) -> TestFile {
    match language {
        Language::Js(dialect) => js::read(source, dialect),
        Language::Python => python::read(source),
        Language::Rust => rust::read(source),
        Language::Java => java::read(source),
        Language::Kotlin => kotlin::read(source),
        Language::Swift => swift::read(source),
        Language::C => cfamily::read(source, false),
        Language::Cpp => cfamily::read(source, true),
        Language::CSharp => csharp::read(source),
        Language::Go => go::read(source),
        Language::Dart => dart::read(source),
    }
}

/// True for a path the guard reads as a test file by its name or folder
/// alone: JS/TS `*.test.*`, `*.spec.*` or in `__tests__`, `test`, `tests`,
/// `spec`; Python `test_*.py`, `*_test.py` or in `test`, `tests`; Rust in
/// `tests`; Java `*Test.java`, `*Tests.java`, `*IT.java`, `Test*.java` or
/// in `src/test/`; Kotlin `*Test.kt`, `*Tests.kt`, `*Spec.kt` or in
/// `src/test/`, `src/*Test/` (`androidTest`, `jvmTest`); Swift
/// `*Tests.swift`, `*Test.swift` or in `tests` or a folder ending in `Tests`
/// (`Tests/`, `AppTests/`, `AppUITests/`); Dart `*_test.dart` or in `test`,
/// `integration_test`; C and C++ `*_test.c`, `*_test.cc`, `*_unittest.cc`,
/// `test_*.c`, `*Test.cpp`, `*Tests.cpp` or in `test`, `tests`,
/// `unittest(s)`; C# `*Tests.cs`, `*Test.cs` or in `test`, `tests`,
/// `*.Tests`, `*.Test`, `*Tests` (a test project); Go `*_test.go` (not under
/// `testdata` or a folder the go tool ignores). A file elsewhere is a test
/// file when it holds tests ([`holds_tests`]); the policy can name more
/// (`verify.tamper.testPaths`).
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    match Language::of(path) {
        Some(Language::Js(_)) => js::is_test_path(path),
        Some(Language::Python) => python::is_test_path(path),
        Some(Language::Rust) => rust::is_test_path(path),
        Some(Language::Java) => java::is_test_path(path),
        Some(Language::Kotlin) => kotlin::is_test_path(path),
        Some(Language::Swift) => swift::is_test_path(path),
        Some(Language::C | Language::Cpp) => cfamily::is_test_path(path),
        Some(Language::CSharp) => csharp::is_test_path(path),
        Some(Language::Go) => go::is_test_path(path),
        Some(Language::Dart) => dart::is_test_path(path),
        None => false,
    }
}

/// True for source that holds tests although its path does not say so:
/// Rust unit tests (`#[test]`, `#[cfg(test)]`) next to the code; Java and
/// Kotlin test annotations (`@Test`) and Kotest specs; Swift that imports
/// XCTest or Testing and defines a test; Dart that imports `package:test`
/// or `package:flutter_test` and calls `test`; a C or C++ file that
/// includes a test framework's header and defines a test.
#[must_use]
pub fn holds_tests(source: &str, language: Language) -> bool {
    match language {
        Language::Rust => rust::holds_tests(source),
        Language::Java => java::holds_tests(source),
        Language::Kotlin => kotlin::holds_tests(source),
        Language::Swift => swift::holds_tests(source),
        Language::C => cfamily::holds_tests(source, false),
        Language::Cpp => cfamily::holds_tests(source, true),
        Language::CSharp => csharp::holds_tests(source),
        Language::Go => go::holds_tests(source),
        Language::Dart => dart::holds_tests(source),
        Language::Js(_) | Language::Python => false,
    }
}

/// The repo path an import of `spec` in the file `from` names, among the
/// paths `exists` accepts; `None` for a package or anything else. The spec
/// `"self"` is the file itself (Rust: the items a file defines).
fn resolve(
    language: Language,
    from: &str,
    spec: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Option<String> {
    if spec == SELF {
        return Some(from.to_owned());
    }
    match language {
        Language::Js(_) => js::resolve(from, spec, exists),
        Language::Python => python::resolve(from, spec, exists),
        Language::Swift => swift::resolve(from, spec, exists),
        Language::Dart => dart::resolve(from, spec, exists),
        Language::C | Language::Cpp => cfamily::resolve(from, spec, exists),
        // Kotlin reads Java classes and Java reads Kotlin ones.
        Language::Java | Language::Kotlin => java::resolve(from, spec, exists),
        Language::Go => go::resolve(from, spec, exists),
        Language::Rust | Language::CSharp => None,
    }
}

/// The module spec of a name a file defines itself.
pub(crate) const SELF: &str = "self";

/// What `verify.tamper` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    /// The guard does not run.
    Off,
    /// Findings are reported and do not fail verify. The default.
    Evaluated,
    /// Findings fail verify.
    Enforced,
}

impl Policy {
    /// The policy a config value names, or `None` for anything else.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "evaluated" => Some(Self::Evaluated),
            "enforced" => Some(Self::Enforced),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Evaluated => "evaluated",
            Self::Enforced => "enforced",
        }
    }
}

// ------------------------------------------------------------ reading files

/// One test in a file: a JS/TS `it` or `test` call, a pytest or unittest
/// test function, a Rust `#[test]` function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// The names of the enclosing suites and its own: JS/TS `describe`
    /// titles joined with ` > ` (a title that is not a plain string is its
    /// source text), Python classes and Rust modules joined with `::`.
    pub id: String,
    /// 1-based line of the call or definition.
    pub line: usize,
    /// It does not run: `.skip`, `.todo`, `.skipIf`, `.fixme`, `x…`, a
    /// skipped `describe`; `@pytest.mark.skip`, `skipif`, `xfail`,
    /// `unittest.skip…`; `#[ignore]`; or a body that does nothing, or code
    /// that never runs ([`TestCase::why`]).
    pub skipped: bool,
    /// When `skipped` is false: how many of its `weight` rows are skipped
    /// one by one (`pytest.param(…, marks=pytest.mark.skip)`).
    pub skipped_rows: usize,
    /// Why it does not run when no marker says so: `"its body does
    /// nothing"`, `"after a return"`, `"inside if (false)"`.
    pub why: Option<&'static str>,
    /// Not a test but something that keeps tests from running: a
    /// `collect_ignore` entry, an autouse fixture that skips. It counts
    /// with the skips only, so adding one is `test-skipped` and removing
    /// one (running those tests again) is no finding.
    pub marker: bool,
    /// `.only`, `f…`, or inside a focused `describe`: the file's other tests
    /// do not run.
    pub focused: bool,
    /// The identifiers it uses: what it tests.
    pub uses: BTreeSet<String>,
    /// How many tests it stands for: the rows of an `each` table that is an
    /// array literal (its own and its suites'), of `@pytest.mark.parametrize`
    /// lists, of `#[case]` attributes; else 1.
    pub weight: usize,
}

/// One assertion that can fail and runs: an `expect(…)` or `assert…(…)`
/// call, a Python `assert` statement, a Rust `assert!` and its forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub line: usize,
    /// The whole chained call (`expect(a).toBe(1)`), without white space:
    /// what is compared.
    pub shape: String,
    /// The same, with white space squeezed to single spaces: what is shown.
    pub text: String,
    /// How many times it runs: the weight of the test it is in.
    pub weight: usize,
    /// The index of the test it is in, if any.
    pub test: Option<usize>,
    /// It runs only sometimes inside its test: under an `#if` (C, C++, C#,
    /// Swift) that does not also hold the test, or after a `return` under
    /// one. Counted, and also counted apart: an assertion made conditional
    /// is a removal (ADR 0052, found in review).
    pub conditional: bool,
    /// The shape without the failure message, when the assertion has one
    /// (`assert x == 1, "why"`, `pytest.fail("why")`, `assert_eq!(a, b,
    /// "why")`): what it checks. Only the expected values of what it checks
    /// need an approval (ADR 0063).
    pub checked: Option<String>,
}

impl Assertion {
    /// What the assertion checks: its shape without its failure message.
    #[must_use]
    pub fn checked(&self) -> &str {
        self.checked.as_deref().unwrap_or(&self.shape)
    }
}

/// `node`'s text without white space and without the parts in `cut`: an
/// assertion's shape with its failure message left out, or `None` when
/// nothing is cut.
pub(super) fn without(node: Node<'_>, src: &[u8], cut: &[Node<'_>]) -> Option<String> {
    let mut ranges: Vec<(usize, usize)> = cut
        .iter()
        .map(|part| (part.start_byte(), part.end_byte()))
        .filter(|(start, end)| *start >= node.start_byte() && *end <= node.end_byte())
        .collect();
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_unstable();
    let mut kept: Vec<u8> = Vec::new();
    let mut at = node.start_byte();
    for (start, end) in ranges {
        if start > at {
            kept.extend_from_slice(&src[at..start]);
        }
        at = at.max(end);
    }
    kept.extend_from_slice(&src[at..node.end_byte()]);
    Some(String::from_utf8_lossy(&kept).split_whitespace().collect())
}

/// What a call whose every argument is its message checks
/// (`pytest.fail("…")`, `t.Errorf("…")`, `XCTFail("…")`), when it is the
/// branch of something that holds its expected value: the condition of an
/// `if` (`!(…)` in an `else`) or a Swift `guard`, or the exception an
/// `except` or `catch` names; then the call without its arguments. So a
/// changed condition pairs as a changed value and a changed message does
/// not (ADR 0063). Anywhere else (a `switch`, a `match`, a loop, the test's
/// own body) `None`: the call is compared with its message, as before,
/// since the message may be the only place its value is written.
pub(super) fn failed_when(call: Node<'_>, src: &[u8], arguments: &[Node<'_>]) -> Option<String> {
    let squeeze = |start: usize, end: usize| -> String {
        String::from_utf8_lossy(&src[start..end])
            .split_whitespace()
            .collect()
    };
    let inside = |outer: Node<'_>| {
        outer.start_byte() <= call.start_byte() && call.end_byte() <= outer.end_byte()
    };
    let bare = || {
        without(call, src, arguments).unwrap_or_else(|| squeeze(call.start_byte(), call.end_byte()))
    };
    let mut child = call;
    for _ in 0..6 {
        let parent = child.parent()?;
        let kind = parent.kind();
        if kind.starts_with("if_") || kind == "elif_clause" || kind == "guard_statement" {
            let mut cursor = parent.walk();
            let conditions: Vec<Node<'_>> = parent
                .children_by_field_name("condition", &mut cursor)
                .collect();
            let (first, last) = (conditions.first()?, conditions.last()?);
            if conditions.iter().any(|condition| inside(*condition)) {
                return None;
            }
            let condition = squeeze(first.start_byte(), last.end_byte());
            let otherwise = parent
                .child_by_field_name("alternative")
                .is_some_and(&inside);
            let word = if kind == "guard_statement" {
                "guard"
            } else {
                "if"
            };
            let condition = if otherwise {
                format!("!({condition})")
            } else {
                condition
            };
            return Some(format!("{word}({condition}):{}", bare()));
        }
        if kind == "except_clause" || kind == "catch_clause" {
            let mut cursor = parent.walk();
            let body = parent
                .named_children(&mut cursor)
                .find(|part| part.kind().contains("block") && inside(*part))?;
            let header = squeeze(parent.start_byte(), body.start_byte());
            return Some(format!("{header}:{}", bare()));
        }
        // Only a block, a statement around the call, or an `else` lies
        // between such a branch and the call.
        let wrapper = kind.contains("block")
            || kind.contains("else")
            || matches!(
                kind,
                "expression_statement"
                    | "statement_list"
                    | "statements"
                    | "control_structure_body"
                    | "compound_statement"
            );
        if !wrapper {
            return None;
        }
        child = parent;
    }
    None
}

/// Which arguments of an assertion call are its failure message, by the
/// name of what is called (its last name: `assertEqual`, `XCTAssertEqual`),
/// from each framework's documented signatures (ADR 0063).
pub(super) struct Messages {
    /// Calls whose every argument is the message: `fail("why")`.
    pub all: &'static [&'static str],
    /// Calls that check this many values first: `assertEqual(first,
    /// second, msg)` is 2. The message is the first positional argument at
    /// or after that index that is written as a string, with what follows
    /// it (the values it is formatted with). An argument that is not a
    /// string there (a `delta`, a `places`, a comparer) is a value.
    pub from: &'static [(&'static str, usize)],
    /// Argument labels that name a message (`msg=`, `reason:`), read only in
    /// the calls of `all` and `from`.
    pub labels: &'static [&'static str],
}

impl Messages {
    /// The message arguments of a call named `name`, given its arguments in
    /// order, each with its label if it has one.
    pub(super) fn of<'t>(
        &self,
        name: &str,
        arguments: &[(Option<&str>, Node<'t>)],
        src: &[u8],
    ) -> Vec<Node<'t>> {
        if self.all.contains(&name) {
            return arguments.iter().map(|(_, node)| *node).collect();
        }
        let Some(from) = self
            .from
            .iter()
            .find(|(called, _)| *called == name)
            .map(|(_, from)| *from)
        else {
            return Vec::new();
        };
        let mut positional = 0;
        let mut message = false;
        let mut out = Vec::new();
        for (label, node) in arguments {
            match label {
                Some(label) if self.labels.contains(label) => out.push(*node),
                Some(_) => {}
                None => {
                    message = message
                        || (positional >= from
                            && string_like(&String::from_utf8_lossy(
                                &src[node.start_byte()..node.end_byte()],
                            )));
                    if message {
                        out.push(*node);
                    }
                    positional += 1;
                }
            }
        }
        out
    }
}

/// True when an expression is written as a string: a literal in any of the
/// languages' quotes, with a prefix (`f"…"`, `r"…"`, `$"…"`, `@"…"`), or a
/// concatenation or format that starts with one (`"a: " + x`).
pub(super) fn string_like(expression: &str) -> bool {
    let rest = expression.trim_start();
    // Raw strings take `#`s too: Rust's `r#"…"#`, Swift's `#"…"#`.
    let rest = rest.trim_start_matches(|c: char| {
        matches!(
            c,
            'f' | 'F' | 'r' | 'R' | 'b' | 'B' | 'u' | 'U' | '$' | '@' | '#'
        )
    });
    rest.starts_with(['"', '\'', '`']) && expression.trim_start().len() - rest.len() <= 4
}

/// The named arguments of a call's argument list, comments left out, each
/// with its label when it has one (`msg="…"`, `reason: "…"`, `message =
/// "…"`, `because: "…"`). Grammars that wrap each argument
/// (`value_argument`, `argument`) are read through the wrapper.
pub(super) fn labelled_arguments<'t, 's>(
    list: Node<'t>,
    src: &'s [u8],
) -> Vec<(Option<&'s str>, Node<'t>)> {
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|node| !node.is_extra() && !node.kind().contains("comment"))
        .map(|node| {
            let whole = std::str::from_utf8(&src[node.start_byte()..node.end_byte()]).unwrap_or("");
            // `name=`, `name:` or `name =` before the value, but not `==`
            // or `::`.
            let name_end = whole
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(whole.len());
            let after = whole[name_end..].trim_start();
            let labelled = name_end > 0
                && !whole.starts_with(|c: char| c.is_ascii_digit())
                && (after.starts_with('=') && !after.starts_with("==")
                    || after.starts_with(':') && !after.starts_with("::"));
            (labelled.then(|| &whole[..name_end]), node)
        })
        .collect()
}

/// What a test file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestFile {
    pub tests: Vec<TestCase>,
    pub assertions: Vec<Assertion>,
    /// Imported names and the module each comes from, as the language
    /// writes it (`../src/math`, `.models`, `app.models`), or [`SELF`] for
    /// a name the file defines itself (Rust).
    pub imports: BTreeMap<String, String>,
    /// tree-sitter found syntax errors, so the counts cannot be trusted.
    pub has_errors: bool,
    /// The line of the first one.
    pub error_line: Option<usize>,
}

// ------------------------------------------------------------ the guard

/// What kind of weakening a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Fewer tests than on the base.
    TestDeleted,
    /// More skipped tests than on the base, or a new `.only`.
    TestSkipped,
    /// Fewer assertions than on the base.
    AssertionRemoved,
    /// More lint suppressions of a rule than on the base.
    SuppressionAdded,
    /// A coverage threshold lower than on the base, or gone (M25).
    CoverageLowered,
    /// A CI check step removed, or made unable to fail (M25).
    CiWeakened,
    /// More ways to skip git hooks (`--no-verify`) than on the base (M25).
    NoVerify,
    /// A lint or type-check rule turned off or made weaker in a config file
    /// (M25).
    RuleDisabled,
    /// A stored snapshot rewritten (M26): needs a person's approval.
    SnapshotChanged,
    /// An assertion whose expected value changed and nothing else (M26):
    /// needs a person's approval.
    ExpectedChanged,
    /// A changed test file tree-sitter cannot read, on the base or now: its
    /// tests and assertions could not be compared.
    NotChecked,
}

impl Kind {
    /// A change that may be right and needs a person to say so (M26, ADR
    /// 0046), rather than a clear cheat.
    #[must_use]
    pub fn needs_approval(self) -> bool {
        matches!(self, Self::SnapshotChanged | Self::ExpectedChanged)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TestDeleted => "test deleted",
            Self::TestSkipped => "test skipped",
            Self::AssertionRemoved => "assertion removed",
            Self::SuppressionAdded => "suppression added",
            Self::CoverageLowered => "coverage lowered",
            Self::CiWeakened => "CI weakened",
            Self::NoVerify => "hooks skipped",
            Self::RuleDisabled => "rule turned off",
            Self::SnapshotChanged => "snapshot changed",
            Self::ExpectedChanged => "expected value changed",
            Self::NotChecked => "not checked",
        }
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub kind: Kind,
    pub file: String,
    pub line: usize,
    /// True when the line is in the base commit's version of the file (a
    /// deleted test or assertion), false when it is in the working tree.
    pub on_base: bool,
    /// The test's id, the assertion's text, or the suppression's comment
    /// or attribute with its key.
    pub what: String,
}

/// Net counts over the changed files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub base_tests: usize,
    pub head_tests: usize,
    pub base_skipped: usize,
    pub head_skipped: usize,
    pub base_assertions: usize,
    pub head_assertions: usize,
    /// Tests deleted with their subject, left out of the base counts.
    pub excused_tests: usize,
    pub excused_assertions: usize,
    /// Lint suppressions in every changed source file.
    pub base_suppressions: usize,
    pub head_suppressions: usize,
}

/// A changed file that `verify.tamper.allow` on the base commit leaves
/// out of the counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedFile {
    pub file: String,
    /// The kinds of finding left out for it.
    pub kinds: Vec<Kind>,
}

/// The `tamper` object of `fscale verify --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TamperReport {
    /// The policy, from the base commit's config.
    pub policy: Policy,
    /// The base commit compared with.
    pub base: String,
    /// Test files compared.
    pub files: usize,
    pub counts: Counts,
    pub findings: Vec<Finding>,
    /// Test files with syntax errors on either side: not compared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_checked: Vec<String>,
    /// Changed files the base commit's `verify.tamper.allow` leaves out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<AllowedFile>,
    /// Who approved the change, when the approvals were read from GitHub
    /// (`fscale tamper --pr-base --approvals`, ADR 0046). `None` everywhere
    /// else: no approval can be recorded where the change itself runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals: Option<Approvals>,
}

/// The approvals of a pull request, read from GitHub's reviews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approvals {
    /// The commit they had to approve: the pull request's head.
    pub head: String,
    /// The people whose latest review approved that commit, other than its
    /// author, with write access.
    pub by: Vec<String>,
}

impl TamperReport {
    /// True when the findings fail verify: the base's policy is `enforced`
    /// and a clear finding remains, or a finding that needs approval has
    /// none where approvals are read (CI). Where the change runs (a laptop,
    /// the pull request's own job) nothing can approve, so those findings
    /// are reported and CI decides (ADR 0046).
    #[must_use]
    pub fn blocks(&self) -> bool {
        self.policy == Policy::Enforced
            && self
                .findings
                .iter()
                .any(|finding| self.counts_against(finding))
    }

    /// True when a finding counts against the change now.
    #[must_use]
    pub fn counts_against(&self, finding: &Finding) -> bool {
        if !finding.kind.needs_approval() {
            return true;
        }
        self.approvals
            .as_ref()
            .is_some_and(|approvals| approvals.by.is_empty())
    }
}

/// The policy in the base commit's `firescale.config.json`: `evaluated`
/// with no test paths or exceptions when it has none, or its config cannot
/// be read as JSON (a `firescale.config.ts` is not run for this).
#[must_use]
pub fn base_policy(root: &Path, base: &BaseCommit) -> TamperPolicy {
    show(root, &base.commit, "firescale.config.json")
        .and_then(|text| {
            serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok()
        })
        .and_then(|config| {
            config
                .get("verify")?
                .get("tamper")
                .map(TamperPolicy::lenient)
        })
        .unwrap_or_default()
}

/// A file as the base commit has it, `None` when it has none.
fn show(root: &Path, commit: &str, path: &str) -> Option<String> {
    let out = run_git(root, &["show", &format!("{commit}:./{path}")]).ok()?;
    (out.code == 0).then_some(out.stdout)
}

/// One changed file in a language the guard reads.
struct Changed {
    path: String,
    language: Language,
    /// As the base commit has it; `None` when it has none.
    base: Option<String>,
    /// In the working tree: `Ok(None)` when it is gone, `Err` when it
    /// cannot be read.
    head: Result<Option<String>, ()>,
}

/// A test the working tree runs fewer of than the base, for naming.
struct Skip<'a> {
    file: &'a str,
    line: usize,
    test: &'a TestCase,
    weight: usize,
}

/// How many runs of a test are skipped: all of them when it is skipped,
/// else the rows skipped one by one.
fn skipped_weight(test: &TestCase) -> usize {
    if test.skipped {
        test.weight
    } else {
        test.skipped_rows.min(test.weight)
    }
}

/// Compares the files that `changed` (paths that differ between the base
/// commit and the working tree) holds, on the base and in the working
/// tree: the tests, skips and assertions of its test files, and the lint
/// suppressions of all its source files.
#[must_use]
pub fn guard(root: &Path, base: &BaseCommit, changed: &BTreeSet<String>) -> TamperReport {
    guard_at(root, base, &Head::WorkingTree, changed)
}

/// What the base is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// The files on disk: what verify and the stop hook judge.
    WorkingTree,
    /// A commit, read from git's objects only: nothing is checked out, so
    /// a commit can be judged without running or writing any of its files
    /// (`fscale tamper --head`, the CI job of ADR 0045).
    Commit(String),
}

/// The files that differ between the base commit and a head commit,
/// repo-relative (paths under the folder `root` is in).
///
/// # Errors
///
/// When git cannot compare the two commits.
pub fn changed_between(
    root: &Path,
    base: &BaseCommit,
    head: &str,
) -> crate::Result<BTreeSet<String>> {
    let out = run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-renames",
            "--name-only",
            "--relative",
            "-z",
            &base.commit,
            head,
            "--",
        ],
    )?;
    if out.code != 0 {
        return Err(crate::Error::user(format!(
            "git could not compare {} with {head}: {}",
            base.short(),
            out.stderr.trim()
        )));
    }
    Ok(out
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect())
}

/// The renames git sees between the base commit and a head commit.
fn renames_between(root: &Path, base: &BaseCommit, head: &str) -> BTreeSet<String> {
    let Ok(out) = run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "-M",
            "--name-status",
            "--relative",
            "-z",
            &base.commit,
            head,
            "--",
        ],
    ) else {
        return BTreeSet::new();
    };
    let parts: Vec<&str> = out.stdout.split('\0').collect();
    let mut renamed = BTreeSet::new();
    let mut index = 0;
    while index < parts.len() {
        let status = parts[index];
        if (status.starts_with('R') || status.starts_with('C')) && index + 2 < parts.len() {
            if status.starts_with('R') {
                renamed.insert(parts[index + 1].replace('\\', "/"));
            }
            index += 3;
        } else {
            index += 2;
        }
    }
    renamed
}

/// Every file a commit holds, under the folder `root` is in.
fn files_in(root: &Path, commit: &str) -> BTreeSet<String> {
    run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            commit,
        ],
    )
    .ok()
    .filter(|out| out.code == 0)
    .map(|out| {
        out.stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(|path| path.replace('\\', "/"))
            .collect()
    })
    .unwrap_or_default()
}

/// Tracked files whose changes git does not show: `assume-unchanged`, and
/// `skip-worktree` outside a sparse checkout (where it marks the folders
/// left out, which are not on disk on purpose).
pub(crate) fn hidden_from_git(root: &Path) -> BTreeSet<String> {
    let sparse = run_git(root, &["config", "--bool", "core.sparseCheckout"])
        .is_ok_and(|out| out.stdout.trim() == "true");
    let Ok(out) = run_git(root, &["-c", "core.quotepath=off", "ls-files", "-v", "-z"]) else {
        return BTreeSet::new();
    };
    out.stdout
        .split('\0')
        .filter_map(|line| {
            let (tag, path) = line.split_once(' ')?;
            let tag = tag.chars().next()?;
            let hidden = tag.is_ascii_lowercase() || (tag == 'S' && !sparse);
            hidden.then(|| path.replace('\\', "/"))
        })
        .collect()
}

/// [`guard`] against a chosen head: the working tree, or a commit.
#[must_use]
pub fn guard_at(
    root: &Path,
    base: &BaseCommit,
    head: &Head,
    changed: &BTreeSet<String>,
) -> TamperReport {
    // Files git is told not to look at are compared all the same: an agent
    // can hide a deleted test from `git diff` with `git update-index
    // --skip-worktree` or `--assume-unchanged` (ADR 0045).
    let mut changed = changed.clone();
    if *head == Head::WorkingTree {
        changed.extend(hidden_from_git(root));
    }
    let changed = &changed;
    let policy = base_policy(root, base);
    let mut report = TamperReport {
        policy: policy.mode,
        base: base.commit.clone(),
        files: 0,
        counts: Counts::default(),
        findings: Vec::new(),
        not_checked: Vec::new(),
        allowed: Vec::new(),
        approvals: None,
    };
    if policy.mode == Policy::Off {
        return report;
    }
    let on_disk = |path: &str| {
        path.split('/')
            .fold(root.to_path_buf(), |dir, part| dir.join(part))
    };

    // Every changed file in a language the guard reads, on both sides. The
    // base side comes from one `git cat-file --batch`. Bytes are decoded
    // leniently: a stray Latin-1 byte in a comment must not make a file
    // read as deleted.
    let sources: Vec<(&String, Language)> = changed
        .iter()
        .filter_map(|path| Language::of(path).map(|language| (path, language)))
        .collect();
    // Config and CI files the detectors of M25 read (ADR 0045).
    // With every config file of the same tool in its folder: adding one can
    // hide another that did not change.
    let mut configs: BTreeSet<String> = changed
        .iter()
        .filter(|path| config::is_config(path))
        .cloned()
        .collect();
    let related: Vec<String> = configs
        .iter()
        .flat_map(|path| config::siblings(path))
        .collect();
    configs.extend(related);
    let snapshot_paths: Vec<&String> = changed
        .iter()
        .filter(|path| approval::is_snapshot_path(path))
        .collect();
    let mut wanted: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
    wanted.extend(configs.iter().map(|path| path.as_str()));
    wanted.extend(snapshot_paths.iter().map(|path| path.as_str()));
    wanted.sort_unstable();
    wanted.dedup();
    let mut on_base = crate::git::files_at(root, &base.commit, &wanted);
    // The head side: from disk, or from the head commit's objects.
    // The head commit's files that are wanted, and every path it holds.
    type InHead = (BTreeMap<String, Vec<u8>>, BTreeSet<String>);
    let in_head: Option<InHead> = match head {
        Head::WorkingTree => None,
        Head::Commit(commit) => Some((
            crate::git::files_at(root, commit, &wanted),
            files_in(root, commit),
        )),
    };
    let read_head = |path: &str| -> Result<Option<String>, ()> {
        match &in_head {
            Some((files, _)) => Ok(files
                .get(path)
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())),
            None => match fs::read(on_disk(path)) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(()),
            },
        }
    };
    let exists_now = |path: &str| -> bool {
        match &in_head {
            Some((_, all)) => all.contains(path),
            None => on_disk(path).is_file(),
        }
    };
    let config_files: Vec<Changed> = configs
        .iter()
        .map(|path| Changed {
            path: (*path).clone(),
            language: Language::Python,
            base: on_base
                .get(path.as_str())
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
            head: read_head(path),
        })
        .collect();
    let files: Vec<Changed> = sources
        .into_iter()
        .map(|(path, language)| Changed {
            path: path.clone(),
            language,
            base: on_base
                .remove(path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            head: read_head(path),
        })
        .collect();

    // What the change deleted: on the base, gone from the working tree, and
    // not the old side of a rename git sees.
    let renamed: BTreeSet<String> = match head {
        Head::WorkingTree => crate::blame::renames_since(root, base)
            .into_keys()
            .collect(),
        Head::Commit(commit) => renames_between(root, base, commit),
    };
    let deleted: BTreeSet<&String> = changed
        .iter()
        .filter(|path| match head {
            Head::WorkingTree => !on_disk(path).exists(),
            Head::Commit(_) => !exists_now(path),
        })
        .filter(|path| !renamed.contains(*path))
        .collect();
    // Every identifier in the files of the change that still exist: a name
    // a deleted test used is "deleted with it" only if it is in none of
    // them (a module renamed without git seeing it keeps its names).
    let mut surviving: BTreeSet<String> = BTreeSet::new();
    for file in &files {
        if let Ok(Some(text)) = &file.head {
            for word in text
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                .filter(|word| !word.is_empty())
            {
                surviving.insert(word.to_owned());
            }
        }
    }

    // Files the base's `allow` leaves out, for the kinds it names.
    const KINDS: [Kind; 11] = [
        Kind::TestDeleted,
        Kind::TestSkipped,
        Kind::AssertionRemoved,
        Kind::SuppressionAdded,
        Kind::CoverageLowered,
        Kind::CiWeakened,
        Kind::NoVerify,
        Kind::RuleDisabled,
        Kind::SnapshotChanged,
        Kind::ExpectedChanged,
        Kind::NotChecked,
    ];
    for file in files.iter().chain(
        config_files
            .iter()
            .filter(|file| Language::of(&file.path).is_none()),
    ) {
        let kinds: Vec<Kind> = KINDS
            .into_iter()
            .filter(|kind| policy.allows(&file.path, *kind))
            .collect();
        if !kinds.is_empty() {
            report.allowed.push(AllowedFile {
                file: file.path.clone(),
                kinds,
            });
        }
    }
    let allows = |path: &str, kind: Kind| policy.allows(path, kind);

    // Lint suppressions, by key, in every changed source file.
    let mut base_keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut base_texts: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut head_suppressions: Vec<(String, Suppression)> = Vec::new();
    for file in files
        .iter()
        .filter(|file| !allows(&file.path, Kind::SuppressionAdded))
    {
        // In a test file `@ts-expect-error` asserts that a type error
        // happens (a type test fails when the error goes away): it is a
        // check, not a suppression (ADR 0046, found on real history).
        let type_test = is_test_path(&file.path) || policy.names_test(&file.path);
        let counted = |found: &Suppression| {
            let directive = found.text.trim_start_matches(['/', '*', ' ', '{']);
            !(type_test
                && matches!(file.language, Language::Js(_))
                && directive.starts_with("@ts-expect-error"))
        };
        if let Some(text) = &file.base {
            for found in suppressions(text, file.language)
                .into_iter()
                .filter(|found| counted(found))
            {
                *base_keys.entry(found.key.clone()).or_default() += 1;
                *base_texts
                    .entry((file.path.clone(), found.key, found.text))
                    .or_default() += 1;
                report.counts.base_suppressions += 1;
            }
        }
        if let Ok(Some(text)) = &file.head {
            for found in suppressions(text, file.language)
                .into_iter()
                .filter(|found| counted(found))
            {
                report.counts.head_suppressions += 1;
                head_suppressions.push((file.path.clone(), found));
            }
        }
    }

    // Coverage thresholds, CI steps, hook skips and rules in config files,
    // compared per key over the whole change.
    let mut config_findings: Vec<Finding> = Vec::new();
    {
        let mut base_measures: Vec<config::Located> = Vec::new();
        let mut head_measures: Vec<config::Located> = Vec::new();
        for file in &config_files {
            let sides = [
                (file.base.as_ref().map(|text| Ok(Some(text.as_str()))), true),
                (Some(file.head.as_ref().map(|text| text.as_deref())), false),
            ];
            for (side, is_base) in sides {
                let Some(side) = side else { continue };
                let measures = match side {
                    Ok(Some(text)) => config::measures_of(&file.path, text),
                    Ok(None) => Ok(Vec::new()),
                    Err(()) => Err(1),
                };
                match measures {
                    Ok(measures) => {
                        let target = if is_base {
                            &mut base_measures
                        } else {
                            &mut head_measures
                        };
                        target.extend(
                            measures
                                .into_iter()
                                .filter(|measure| !allows(&file.path, measure.kind))
                                .map(|measure| config::Located {
                                    file: file.path.clone(),
                                    measure,
                                }),
                        );
                    }
                    Err(line) => {
                        if !allows(&file.path, Kind::NotChecked)
                            && !report.not_checked.contains(&file.path)
                        {
                            report.not_checked.push(file.path.clone());
                            config_findings.push(Finding {
                                kind: Kind::NotChecked,
                                file: file.path.clone(),
                                line,
                                on_base: is_base,
                                what: "this config file cannot be read, so its settings were not compared".to_owned(),
                            });
                        }
                    }
                }
            }
        }
        // A file that cannot be read on one side is compared on neither.
        base_measures.retain(|located| !report.not_checked.contains(&located.file));
        head_measures.retain(|located| !report.not_checked.contains(&located.file));
        // Settings in a file its tool does not read, because another of its
        // files in the folder comes first, do not count.
        let side_texts = |base_side: bool| -> BTreeMap<String, String> {
            config_files
                .iter()
                .filter_map(|file| {
                    let text = if base_side {
                        file.base.clone()
                    } else {
                        file.head.clone().ok().flatten()
                    };
                    text.map(|text| (file.path.clone(), text))
                })
                .collect()
        };
        config::drop_shadowed(&mut base_measures, &side_texts(true));
        config::drop_shadowed(&mut head_measures, &side_texts(false));
        // Checks the working tree still turns on by implication (M25).
        let implied = lintconfig::implied(&base_measures, &head_measures);
        head_measures.extend(implied);
        config_findings.extend(config::compare(&base_measures, &head_measures));
    }

    // Snapshots rewritten: on both sides, with a different body (M26).
    let mut approval_findings: Vec<Finding> = Vec::new();
    for path in &snapshot_paths {
        if allows(path, Kind::SnapshotChanged) {
            continue;
        }
        let base_text = on_base
            .get(path.as_str())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let (Some(base_text), Ok(Some(head_text))) = (base_text, read_head(path)) else {
            continue;
        };
        for (name, line) in approval::rewritten(path, &base_text, &head_text) {
            approval_findings.push(Finding {
                kind: Kind::SnapshotChanged,
                file: (*path).clone(),
                line,
                on_base: false,
                what: format!("snapshot `{name}` was rewritten"),
            });
        }
    }

    // The test files: by name or folder, by the base's `testPaths`, or (Rust)
    // because they hold tests on either side.
    let mut not_checked: Vec<Finding> = Vec::new();
    let mut pairs: Vec<(&Changed, Option<TestFile>, Option<TestFile>)> = Vec::new();
    for file in &files {
        let holds = |text: &Option<String>| {
            text.as_deref()
                .is_some_and(|text| holds_tests(text, file.language))
        };
        let is_test = is_test_path(&file.path)
            || policy.names_test(&file.path)
            || holds(&file.base)
            || file.head.as_ref().is_ok_and(holds);
        if !is_test {
            continue;
        }
        let base_file = file
            .base
            .as_deref()
            .map(|text| read_named(&file.path, text, file.language));
        let head_file = match &file.head {
            Ok(Some(text)) => Some(read_named(&file.path, text, file.language)),
            Ok(None) => None,
            Err(()) => Some(TestFile {
                has_errors: true,
                ..TestFile::default()
            }),
        };
        if base_file.is_none() && head_file.is_none() {
            continue;
        }
        // A file tree-sitter cannot read is never taken as clean: it is a
        // finding of its own, which counts and blocks like the others,
        // unless the base's `allow` leaves it out.
        let broken = [(&head_file, false), (&base_file, true)]
            .into_iter()
            .find_map(|(read, on_base)| {
                read.as_ref()
                    .filter(|read| read.has_errors)
                    .map(|read| (read.error_line.unwrap_or(1), on_base))
            });
        if let Some((line, on_base)) = broken {
            if !allows(&file.path, Kind::NotChecked) {
                report.not_checked.push(file.path.clone());
                not_checked.push(Finding {
                    kind: Kind::NotChecked,
                    file: file.path.clone(),
                    line,
                    on_base,
                    what: "tree-sitter cannot read this file, so its tests and assertions were not compared".to_owned(),
                });
            }
            continue;
        }
        pairs.push((file, base_file, head_file));
    }
    report.files = pairs.len();

    // A test whose subject the change deleted: it uses names imported from
    // modules of the repo, every such module is deleted, and none of those
    // names is left in the change.
    let existed_on_base = |path: &str| deleted.contains(&path.to_owned()) || exists_now(path);
    // Go names no import for its own package: the names each deleted Go
    // file declared, so a test is excused only by the file that declared
    // what it uses, not by another file of its package (M32).
    let go_declared: BTreeMap<&str, BTreeSet<String>> = files
        .iter()
        .filter(|file| file.language == Language::Go && deleted.contains(&file.path))
        .filter_map(|file| {
            let text = file.base.as_deref()?;
            Some((file.path.as_str(), go::top_level_names(text)))
        })
        .collect();
    let excused = |file: &Changed, read: &TestFile, test: &TestCase| -> bool {
        let mut modules: BTreeSet<String> = BTreeSet::new();
        let mut names: Vec<&String> = Vec::new();
        for name in &test.uses {
            let Some(spec) = read.imports.get(name) else {
                continue;
            };
            // Python `from app import pricing` may name the module
            // `app.pricing`: try that first.
            let module = (file.language == Language::Python && spec != SELF)
                .then(|| {
                    let joined = if spec.ends_with('.') {
                        format!("{spec}{name}")
                    } else {
                        format!("{spec}.{name}")
                    };
                    resolve(file.language, &file.path, &joined, &existed_on_base)
                })
                .flatten()
                .or_else(|| resolve(file.language, &file.path, spec, &existed_on_base));
            match module {
                // A helper of the tests (a factory, a fixture module) is
                // not what they test: deleting it with them excuses nothing.
                Some(module) if is_test_path(&module) || policy.names_test(&module) => {}
                Some(module)
                    if spec == go::PACKAGE
                        && !go_declared
                            .get(module.as_str())
                            .is_some_and(|declared| declared.contains(name)) => {}
                Some(module) => {
                    modules.insert(module);
                    names.push(name);
                }
                // A relative import that names no file the guard can find
                // (a path alias, a namespace package): the subject is not
                // known, so the test is never excused.
                None if spec.starts_with('.') => return false,
                None => {}
            }
        }
        !modules.is_empty()
            && modules.iter().all(|module| deleted.contains(module))
            && names.iter().all(|name| !surviving.contains(*name))
    };

    let mut base_ids: Vec<(String, String, usize)> = Vec::new();
    let mut head_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut base_skipped_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut head_skips: Vec<Skip<'_>> = Vec::new();
    let mut base_focus: BTreeSet<String> = BTreeSet::new();
    let mut head_focus: Vec<(String, usize, String)> = Vec::new();
    // Each assertion of the base: its shape, text, file, line and test.
    let mut base_shapes: Vec<BaseAssertion> = Vec::new();
    let mut head_shapes: BTreeMap<String, usize> = BTreeMap::new();
    // Assertions that run whenever their test does (not `conditional`), on
    // each side, and those of the head by shape and by test.
    let (mut base_always, mut head_always) = (0usize, 0usize);
    let mut head_always_shapes: BTreeMap<String, usize> = BTreeMap::new();
    let mut head_always_in_test: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    // The same, by file and test: which one went is named by its test first.
    let mut head_in_test: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    // Assertions that renamed tests lost (ADR 0063).
    let mut lost_in_renames: Vec<Finding> = Vec::new();
    // What each changed test file holds: its new tests, and how many more
    // of each check than on the base. A test or a check that went from one
    // file may be in another (ADR 0063).
    let own: Vec<(BTreeSet<String>, BTreeMap<String, isize>)> = pairs
        .iter()
        .map(|(_, base_file, head_file)| {
            let ids = |read: &Option<TestFile>| -> BTreeSet<String> {
                read.iter()
                    .flat_map(|read| read.tests.iter().map(|test| test.id.clone()))
                    .collect()
            };
            let new_tests: BTreeSet<String> = ids(head_file)
                .difference(&ids(base_file))
                .cloned()
                .collect();
            let mut more: BTreeMap<String, isize> = BTreeMap::new();
            for (read, step) in [(head_file, 1isize), (base_file, -1)] {
                for assertion in read.iter().flat_map(|read| &read.assertions) {
                    *more.entry(assertion.checked().to_owned()).or_default() += step;
                }
            }
            (new_tests, more)
        })
        .collect();
    let mut all_new: BTreeMap<&str, usize> = BTreeMap::new();
    let mut all_more: BTreeMap<&str, isize> = BTreeMap::new();
    for (new_tests, more) in &own {
        for id in new_tests {
            *all_new.entry(id.as_str()).or_default() += 1;
        }
        for (shape, count) in more {
            *all_more.entry(shape.as_str()).or_default() += count;
        }
    }
    let counts = &mut report.counts;
    for (index, (file, base_file, head_file)) in pairs.iter().enumerate() {
        let path = &file.path;
        let (own_new, own_more) = &own[index];
        let elsewhere = Elsewhere {
            new_tests: all_new
                .iter()
                .filter(|(id, count)| **count > usize::from(own_new.contains(**id)))
                .map(|(id, _)| (*id).to_owned())
                .collect(),
            more: all_more
                .iter()
                .map(|(shape, count)| {
                    let mine = own_more.get(*shape).copied().unwrap_or(0);
                    ((*shape).to_owned(), count - mine)
                })
                .filter(|(_, count)| *count != 0)
                .collect(),
        };
        // The tests this file's change renamed, merged or split (ADR 0063).
        let renamed = match (base_file, head_file) {
            (Some(before), Some(after)) => successors(before, after, &elsewhere),
            _ => Successors::default(),
        };
        // What followed tests lost (ADR 0063), found below.
        let mut file_losses: Vec<Finding> = Vec::new();
        let count_tests = !allows(path, Kind::TestDeleted);
        let count_skips = !allows(path, Kind::TestSkipped);
        let count_assertions = !allows(path, Kind::AssertionRemoved);
        let head_has: BTreeSet<&str> = head_file
            .iter()
            .flat_map(|read| read.tests.iter().map(|test| test.id.as_str()))
            .collect();
        if let Some(read) = base_file {
            let mut excused_tests: BTreeSet<usize> = BTreeSet::new();
            for (index, test) in read.tests.iter().enumerate() {
                if !head_has.contains(test.id.as_str()) && excused(file, read, test) {
                    excused_tests.insert(index);
                    counts.excused_tests += test.weight;
                    continue;
                }
                if count_tests && !test.marker {
                    counts.base_tests += test.weight;
                    for _ in 0..test.weight {
                        base_ids.push((test.id.clone(), path.clone(), test.line));
                    }
                }
                if count_skips {
                    let skipped = skipped_weight(test);
                    counts.base_skipped += skipped;
                    if skipped > 0 {
                        *base_skipped_ids.entry(test.id.clone()).or_default() += skipped;
                    }
                    if test.focused {
                        base_focus.insert(test.id.clone());
                    }
                }
            }
            for assertion in &read.assertions {
                if assertion
                    .test
                    .is_some_and(|test| excused_tests.contains(&test))
                {
                    counts.excused_assertions += assertion.weight;
                    continue;
                }
                if !count_assertions {
                    continue;
                }
                counts.base_assertions += assertion.weight;
                if !assertion.conditional {
                    base_always += assertion.weight;
                }
                let test = test_id(read, assertion);
                for _ in 0..assertion.weight {
                    base_shapes.push(BaseAssertion {
                        shape: assertion.checked().to_owned(),
                        text: assertion.text.clone(),
                        file: path.clone(),
                        line: assertion.line,
                        test: test.clone(),
                        conditional: assertion.conditional,
                    });
                }
            }
            if let (true, Some(after)) = (count_assertions, head_file) {
                let excused_ids: BTreeSet<&str> = excused_tests
                    .iter()
                    .filter_map(|index| read.tests.get(*index))
                    .map(|test| test.id.as_str())
                    .collect();
                file_losses =
                    followed_losses(path, read, after, &renamed, &excused_ids, &elsewhere);
            }
        }
        // Assertions that changed only in their values, file by file (M26).
        if let (Some(before), Some(after)) = (base_file, head_file) {
            if !allows(path, Kind::ExpectedChanged) {
                let lost: BTreeSet<usize> = file_losses.iter().map(|lost| lost.line).collect();
                approval_findings.extend(expected_changes(path, before, after, &renamed, &lost));
            }
        }
        lost_in_renames.extend(file_losses);
        // A file whose every test is skipped on the base and now (the whole
        // file under `#if canImport(…)`, `@TestOn(…)`, a platform's name): a
        // new test in it is no more skipped than the others were (found on
        // real history, M32: Alamofire's `#if canImport(_Concurrency)`).
        let all_skipped = |read: &TestFile| {
            let tests: Vec<&TestCase> = read.tests.iter().filter(|t| !t.marker).collect();
            !tests.is_empty() && tests.iter().all(|t| skipped_weight(t) >= t.weight)
        };
        let file_wide = base_file.as_ref().is_some_and(all_skipped)
            && head_file.as_ref().is_some_and(all_skipped);
        let base_here: BTreeSet<&str> = base_file
            .iter()
            .flat_map(|read| read.tests.iter().map(|t| t.id.as_str()))
            .collect();
        if let Some(read) = head_file {
            for test in &read.tests {
                if count_tests && !test.marker {
                    counts.head_tests += test.weight;
                    *head_ids.entry(test.id.clone()).or_default() += test.weight;
                }
                if count_skips {
                    let skipped =
                        if file_wide && !test.marker && !base_here.contains(test.id.as_str()) {
                            0
                        } else {
                            skipped_weight(test)
                        };
                    counts.head_skipped += skipped;
                    if skipped > 0 {
                        head_skips.push(Skip {
                            file: path,
                            line: test.line,
                            test,
                            weight: skipped,
                        });
                    }
                    if test.focused {
                        head_focus.push((path.clone(), test.line, test.id.clone()));
                    }
                }
            }
            if count_assertions {
                for assertion in &read.assertions {
                    counts.head_assertions += assertion.weight;
                    // By what it checks, without its message (ADR 0063).
                    let checked = assertion.checked().to_owned();
                    let key = (checked.clone(), path.clone(), test_id(read, assertion));
                    *head_shapes.entry(checked.clone()).or_default() += assertion.weight;
                    *head_in_test.entry(key.clone()).or_default() += assertion.weight;
                    if !assertion.conditional {
                        head_always += assertion.weight;
                        *head_always_shapes.entry(checked).or_default() += assertion.weight;
                        *head_always_in_test.entry(key).or_default() += assertion.weight;
                    }
                }
            }
        }
    }

    let mut findings = not_checked;
    findings.extend(config_findings);
    findings.extend(approval_findings);
    // Fewer tests: name those whose id is gone, up to the shortfall.
    if counts.head_tests < counts.base_tests {
        let mut left = head_ids.clone();
        let mut missing: Vec<&(String, String, usize)> = Vec::new();
        for entry in &base_ids {
            match left.get_mut(&entry.0) {
                Some(count) if *count > 0 => *count -= 1,
                _ => missing.push(entry),
            }
        }
        let shortfall = counts.base_tests - counts.head_tests;
        let mut named: BTreeSet<(&String, &String, usize)> = BTreeSet::new();
        for (id, file, line) in missing.into_iter().take(shortfall) {
            if named.insert((id, file, *line)) {
                findings.push(Finding {
                    kind: Kind::TestDeleted,
                    file: file.clone(),
                    line: *line,
                    on_base: true,
                    what: id.clone(),
                });
            }
        }
    }
    // More skipped runs: name the tests that skip more runs than on the
    // base, up to the shortfall.
    // New tests that are skipped or run only sometimes (a new `#[cfg(unix)]`
    // test, a test added inside an existing `describe.runIf`) take nothing
    // away while the change adds at least as many tests: they are left out
    // of the shortfall. A test renamed and skipped keeps the count of tests
    // the same, so it is still found (ADR 0046, found on real history).
    let had_ids: BTreeSet<&str> = base_ids.iter().map(|(id, _, _)| id.as_str()).collect();
    let new_skipped: usize = head_skips
        .iter()
        .filter(|skip| !had_ids.contains(skip.test.id.as_str()) && !skip.test.marker)
        .map(|skip| skip.weight)
        .sum();
    let added_tests = counts.head_tests.saturating_sub(counts.base_tests);
    // Less one for each run of a base test that went away: a test renamed
    // and skipped has a new id too, and must not pass for a new test (found
    // in review). At most that many of the new skipped tests can be such
    // (found on real history, M32: Humanizer renamed one test and added six
    // more under the `#if NET6_0_OR_GREATER` its file already had).
    let gone_runs = base_ids
        .iter()
        .filter(|(id, _, _)| !head_ids.contains_key(id.as_str()))
        .count();
    let excused_skips = new_skipped.saturating_sub(gone_runs).min(added_tests);
    // Tests the base had, run there, and skipped now (less those the base
    // skipped and the change runs again) are counted on their own, so a new
    // skipped test cannot hide an old one made to not run.
    let mut skipped_now: BTreeMap<&str, usize> = BTreeMap::new();
    for skip in &head_skips {
        *skipped_now.entry(skip.test.id.as_str()).or_default() += skip.weight;
    }
    let newly: usize = skipped_now
        .iter()
        .filter(|(id, _)| had_ids.contains(*id))
        .map(|(id, now)| now.saturating_sub(base_skipped_ids.get(*id).copied().unwrap_or(0)))
        .sum();
    let run_again: usize = base_skipped_ids
        .iter()
        .filter(|(id, _)| head_ids.contains_key(id.as_str()))
        .map(|(id, had)| had.saturating_sub(skipped_now.get(id.as_str()).copied().unwrap_or(0)))
        .sum();
    let converted = newly.saturating_sub(run_again);
    let net = counts
        .head_skipped
        .saturating_sub(counts.base_skipped + excused_skips);
    if net.max(converted) > 0 {
        let mut shortfall = net.max(converted);
        let mut left = base_skipped_ids;
        // Tests the base had first: those are the ones made to not run.
        let ordered = head_skips
            .iter()
            .filter(|skip| had_ids.contains(skip.test.id.as_str()))
            .chain(
                head_skips
                    .iter()
                    .filter(|skip| !had_ids.contains(skip.test.id.as_str())),
            );
        for skip in ordered {
            if shortfall == 0 {
                break;
            }
            let had = left.entry(skip.test.id.clone()).or_default();
            let covered = skip.weight.min(*had);
            *had -= covered;
            let more = skip.weight - covered;
            if more == 0 {
                continue;
            }
            shortfall = shortfall.saturating_sub(more);
            let mut what = skip.test.id.clone();
            if let Some(why) = skip.test.why {
                what.push_str(&format!(" (never runs: {why})"));
            } else if !skip.test.skipped {
                what.push_str(&format!(
                    " ({} of its {} rows skipped)",
                    skip.weight, skip.test.weight
                ));
            }
            findings.push(Finding {
                kind: Kind::TestSkipped,
                file: skip.file.to_owned(),
                line: skip.line,
                on_base: false,
                what,
            });
        }
    }
    // A new focus: `.only` on a test that did not have it.
    for (file, line, id) in head_focus
        .iter()
        .filter(|(_, _, id)| !base_focus.contains(id))
    {
        findings.push(Finding {
            kind: Kind::TestSkipped,
            file: file.clone(),
            line: *line,
            on_base: false,
            what: format!("{id} (.only: the file's other tests do not run)"),
        });
    }
    // Fewer assertions: name those whose text is gone, up to the shortfall.
    // The same text kept in the same test pairs first, then anywhere: a
    // copy the change kept in another test is not taken for the one it
    // removed (found on real history: Alamofire commented out the first of
    // several identical assertions, and the last was named). Fewer
    // assertions that run whenever their test does is a removal too: an
    // assertion moved under an `#if` inside its test (ADR 0052, found in
    // review); the larger shortfall is named.
    let total_short = counts
        .base_assertions
        .saturating_sub(counts.head_assertions);
    let always_short = base_always.saturating_sub(head_always);
    if total_short > 0 || always_short > 0 {
        let (missing, shortfall) = if always_short > total_short {
            let always: Vec<&BaseAssertion> = base_shapes
                .iter()
                .filter(|entry| !entry.conditional)
                .collect();
            (
                unpaired(&always, head_always_shapes, head_always_in_test),
                always_short,
            )
        } else {
            let all: Vec<&BaseAssertion> = base_shapes.iter().collect();
            (unpaired(&all, head_shapes, head_in_test), total_short)
        };
        let mut named: BTreeSet<(&String, usize)> = BTreeSet::new();
        for entry in missing.into_iter().take(shortfall) {
            if named.insert((&entry.file, entry.line)) {
                findings.push(Finding {
                    kind: Kind::AssertionRemoved,
                    file: entry.file.clone(),
                    line: entry.line,
                    on_base: true,
                    what: entry.text.clone(),
                });
            }
        }
    }
    // An assertion a renamed test lost. One the net count named already
    // (both match by what an assertion checks, without its message) is named
    // once, with the test it was in.
    for lost in lost_in_renames {
        let named = findings.iter_mut().find(|finding| {
            finding.kind == Kind::AssertionRemoved
                && finding.on_base
                && finding.file == lost.file
                && finding.line == lost.line
        });
        match named {
            Some(finding) => finding.what = lost.what,
            None => findings.push(lost),
        }
    }
    // More suppressions of a rule: name the new ones, up to the shortfall
    // per key. One that the base has in the same file, with the same text,
    // is not new.
    let mut head_keys: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, found) in &head_suppressions {
        *head_keys.entry(found.key.as_str()).or_default() += 1;
    }
    let mut shortfall: BTreeMap<&str, usize> = head_keys
        .iter()
        .filter_map(|(key, head)| {
            let base = base_keys.get(*key).copied().unwrap_or(0);
            (*head > base).then(|| (*key, head - base))
        })
        .collect();
    // A suppression the change removed may stand for a narrower one it
    // added: `# type: ignore` turned into `# type: ignore[union-attr]`,
    // `#[allow(clippy::all)]` into one clippy lint, one for another. Each
    // removed one covers one added one, anywhere in the change; a removed
    // file-wide one also covers every narrower one added in its own file.
    let mut file_keys: BTreeMap<(String, String), isize> = BTreeMap::new();
    for ((path, key, _), count) in &base_texts {
        *file_keys.entry((path.clone(), key.clone())).or_default() += *count as isize;
    }
    for (path, found) in &head_suppressions {
        *file_keys
            .entry((path.clone(), found.key.clone()))
            .or_default() -= 1;
    }
    let mut surplus: BTreeMap<String, usize> = base_keys
        .iter()
        .filter_map(|(key, base)| {
            let head = head_keys.get(key.as_str()).copied().unwrap_or(0);
            (*base > head).then(|| (key.clone(), base - head))
        })
        .collect();
    let new_files: BTreeMap<&str, &str> = files
        .iter()
        .filter(|file| file.base.is_none())
        .filter_map(|file| match &file.head {
            Ok(Some(text)) => Some((file.path.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    let mut copy_checks = 0usize;
    let mut copied_from_base = |path: &str, line: usize, key: &str| -> bool {
        // A file-wide or module-wide suppression covers new code wherever it
        // is copied: never excused (found in review). And at most 50 lines
        // are looked up per change, so a file of many suppressions cannot
        // make the guard slow.
        if reach_rank(key) > 0 || copy_checks >= 50 {
            return false;
        }
        copy_checks += 1;
        let Some(text) = new_files.get(path) else {
            return false;
        };
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let Some(source) = lines.get(line.saturating_sub(1)).copied() else {
            return false;
        };
        // A comment or attribute on a line of its own suppresses the code
        // under it: the base must hold both, one above the other. A line
        // of code with its suppression must be there word for word.
        let alone = ["//", "#", "/*", "*", "--"]
            .iter()
            .any(|mark| source.starts_with(mark));
        let next = if alone {
            match lines[line.min(lines.len())..]
                .iter()
                .find(|l| !l.is_empty())
            {
                Some(next) => Some(*next),
                None => return false,
            }
        } else {
            None
        };
        if source.len() < 12 && next.is_none() {
            return false;
        }
        // Files of the same kind only (a code block in a README is not code).
        let extension = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let pathspec = format!("*.{extension}");
        let Ok(out) = run_git(
            root,
            &[
                "grep",
                "-l",
                "-F",
                "-e",
                source,
                &base.commit,
                "--",
                &pathspec,
            ],
        ) else {
            return false;
        };
        if out.code != 0 {
            return false;
        }
        let prefix = format!("{}:", base.commit);
        let candidates: Vec<String> = out
            .stdout
            .lines()
            .filter_map(|l| l.strip_prefix(&prefix).map(str::to_owned))
            .take(20)
            .collect();
        let wanted: Vec<&str> = candidates.iter().map(String::as_str).collect();
        crate::git::files_at(root, &base.commit, &wanted)
            .values()
            .any(|bytes| {
                let text = String::from_utf8_lossy(bytes);
                let base_lines: Vec<&str> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                match next {
                    Some(next) => base_lines
                        .windows(2)
                        .any(|pair| pair[0] == source && pair[1] == next),
                    // The whole line, not a part of a longer one.
                    None => base_lines.contains(&source),
                }
            })
    };
    let mut left = base_texts;
    for (path, found) in &head_suppressions {
        let Some(more) = shortfall.get_mut(found.key.as_str()) else {
            continue;
        };
        if *more == 0 {
            continue;
        }
        if let Some(had) = left
            .get_mut(&(path.clone(), found.key.clone(), found.text.clone()))
            .filter(|had| **had > 0)
        {
            *had -= 1;
            continue;
        }
        *more -= 1;
        let wider_here = file_keys.iter().any(|((file, key), removed)| {
            file == path
                && *removed > 0
                && covers(key, &found.key)
                && reach_rank(key) > reach_rank(&found.key)
        });
        if wider_here {
            continue;
        }
        if let Some(count) = surplus
            .iter_mut()
            .find(|(key, count)| **count > 0 && covers(key, &found.key))
            .map(|(_, count)| count)
        {
            *count -= 1;
            continue;
        }
        // In a new file, a line that the base holds word for word elsewhere
        // is a copy of code that keeps its suppression (a file split off,
        // an adapter copied): not a new one (ADR 0046, real history).
        if copied_from_base(path, found.line, &found.key) {
            continue;
        }
        findings.push(Finding {
            kind: Kind::SuppressionAdded,
            file: path.clone(),
            line: found.line,
            on_base: false,
            what: format!("{} ({})", found.text, found.key),
        });
    }
    findings.sort_by(|a, b| (a.kind, &a.file, a.line).cmp(&(b.kind, &b.file, b.line)));
    // One finding per place and kind (a CI check counted for several
    // events, gone from each, is one finding).
    findings.dedup_by(|a, b| {
        a.kind == Kind::CiWeakened
            && a.kind == b.kind
            && a.file == b.file
            && a.line == b.line
            && a.on_base == b.on_base
    });
    report.findings = findings;
    report
}

/// The approvals of the pull request `pr`, from GitHub's list of its reviews
/// (the text of `gh api --paginate repos/{owner}/{repo}/pulls/{n}/reviews`).
///
/// # Errors
///
/// When the reviews are not that JSON, or the event names no author.
pub fn approvals_from_reviews(reviews: &str, pr: &github::PullRequest) -> crate::Result<Approvals> {
    if pr.author.is_empty() {
        return Err(crate::Error::user(
            "--approvals: the event names no pull_request.user.login, so a review by the author could not be told apart.",
        ));
    }
    let by = approval::approvers(reviews, &pr.author, &pr.head)
        .map_err(|message| crate::Error::user(format!("--approvals: {message}")))?;
    Ok(Approvals {
        head: pr.head.clone(),
        by,
    })
}

/// One run of an assertion of the base.
struct BaseAssertion {
    shape: String,
    text: String,
    file: String,
    line: usize,
    test: String,
    conditional: bool,
}

/// The assertions of the base that no assertion of the head pairs with:
/// one with the same shape in the same file and test first, then one with
/// the same shape anywhere.
fn unpaired<'a>(
    base: &[&'a BaseAssertion],
    mut left: BTreeMap<String, usize>,
    mut in_test: BTreeMap<(String, String, String), usize>,
) -> Vec<&'a BaseAssertion> {
    let mut kept = vec![false; base.len()];
    for (index, entry) in base.iter().enumerate() {
        let key = (entry.shape.clone(), entry.file.clone(), entry.test.clone());
        if let Some(count) = in_test.get_mut(&key).filter(|count| **count > 0) {
            *count -= 1;
            if let Some(total) = left.get_mut(&entry.shape) {
                *total = total.saturating_sub(1);
            }
            kept[index] = true;
        }
    }
    let mut missing = Vec::new();
    for (entry, kept) in base.iter().zip(kept) {
        if kept {
            continue;
        }
        match left.get_mut(&entry.shape) {
            Some(count) if *count > 0 => *count -= 1,
            _ => missing.push(*entry),
        }
    }
    missing
}

/// The id of the test an assertion is in, or "" outside any test.
fn test_id(file: &TestFile, assertion: &Assertion) -> String {
    assertion
        .test
        .and_then(|index| file.tests.get(index))
        .map(|test| test.id.clone())
        .unwrap_or_default()
}

/// The id of the test an assertion is in, or "" outside any test.
fn test_of<'f>(file: &'f TestFile, assertion: &Assertion) -> &'f str {
    assertion
        .test
        .and_then(|index| file.tests.get(index))
        .map_or("", |test| test.id.as_str())
}

/// At most this many (gone, new) pairs of tests are weighed per file when
/// following renames; a file rewritten past that follows none.
const MAX_RENAME_PAIRS: usize = 40_000;

/// What the other changed test files say about one file's change: the tests
/// that are new there, and how many more of each check (with literals as
/// written, without the message) they hold now than on the base.
#[derive(Debug, Default)]
struct Elsewhere {
    new_tests: BTreeSet<String>,
    more: BTreeMap<String, isize>,
}

/// The tests of one file that the change renamed, merged or split, in
/// groups: the base's tests that are gone and the new tests that follow
/// them (ADR 0063).
#[derive(Debug, Default)]
struct Successors {
    groups: Vec<(Vec<String>, Vec<String>)>,
    of_gone: BTreeMap<String, usize>,
    of_new: BTreeMap<String, usize>,
}

/// True when one test's name starts with the other's: `test_totals` and
/// `test_totals_b`, a part split off.
fn split_names(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Follows the tests of one file through the change. A gone test that is
/// new in another changed file moved there and is followed by nothing here.
/// Each other gone test is followed by the new test that shares the most
/// of its assertions once every literal is masked (ties: the names most
/// alike, then the ids): a rename, or with several gone tests that pick
/// the same one, a merge. A new test that no gone test picked follows the
/// gone test it shares most with when one name starts with the other: a
/// part split off (`test_totals` → `test_totals_a`, `test_totals_b`). A
/// gone test that shares no assertion with a new test was not renamed.
/// Before ADR 0063 every gone test was taken for a rename of every new one,
/// so `assert upstream.calls == 1` of a renamed test was paired with
/// `assert upstream.calls == 0` of a new, different one (finding 4 of the
/// 0.5.1 review).
fn successors(before: &TestFile, after: &TestFile, elsewhere: &Elsewhere) -> Successors {
    let before_ids: BTreeSet<&str> = before.tests.iter().map(|t| t.id.as_str()).collect();
    let after_ids: BTreeSet<&str> = after.tests.iter().map(|t| t.id.as_str()).collect();
    let gone: Vec<&str> = before_ids
        .difference(&after_ids)
        .copied()
        .filter(|id| !elsewhere.new_tests.contains(*id))
        .collect();
    let new: Vec<&str> = after_ids.difference(&before_ids).copied().collect();
    let mut out = Successors::default();
    if gone.is_empty() || new.is_empty() || gone.len() * new.len() > MAX_RENAME_PAIRS {
        return out;
    }
    let skeletons = |file: &TestFile, ids: &[&str]| -> BTreeMap<String, BTreeMap<String, usize>> {
        let mut by_test: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for assertion in &file.assertions {
            let id = test_of(file, assertion);
            if ids.contains(&id) {
                *by_test
                    .entry(id.to_owned())
                    .or_default()
                    .entry(approval::skeleton(assertion.checked()))
                    .or_default() += 1;
            }
        }
        by_test
    };
    let had = skeletons(before, &gone);
    let has = skeletons(after, &new);
    let alike = |a: &str, b: &str| -> usize {
        let start = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
        let end = a
            .chars()
            .rev()
            .zip(b.chars().rev())
            .take_while(|(x, y)| x == y)
            .count();
        (start + end).min(a.chars().count().max(b.chars().count()))
    };
    let shared = |old: &str, now: &str| -> usize {
        match (had.get(old), has.get(now)) {
            (Some(old_shapes), Some(new_shapes)) => old_shapes
                .iter()
                .map(|(shape, count)| (*count).min(new_shapes.get(shape).copied().unwrap_or(0)))
                .sum(),
            _ => 0,
        }
    };
    // The best of `options` for `id`: most shared, then most alike, then
    // the first id.
    let best = |id: &str, options: &[&str], forward: bool| -> Option<String> {
        options
            .iter()
            .map(|other| {
                let count = if forward {
                    shared(id, other)
                } else {
                    shared(other, id)
                };
                (count, alike(id, other), *other)
            })
            .filter(|(count, _, _)| *count > 0)
            .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(b.2.cmp(a.2)))
            .map(|(_, _, other)| other.to_owned())
    };
    let mut edges: Vec<(String, String)> = Vec::new();
    for old in &gone {
        if let Some(now) = best(old, &new, true) {
            edges.push(((*old).to_owned(), now));
        }
    }
    let picked: BTreeSet<String> = edges.iter().map(|(_, now)| now.clone()).collect();
    for now in new.iter().filter(|now| !picked.contains(**now)) {
        if let Some(old) = best(now, &gone, false).filter(|old| split_names(old, now)) {
            edges.push((old, (*now).to_owned()));
        }
    }
    // Groups: the tests joined by an edge, through any number of edges.
    let mut group: BTreeMap<(bool, String), usize> = BTreeMap::new();
    for (old, now) in &edges {
        let at_old = group.get(&(false, old.clone())).copied();
        let at_new = group.get(&(true, now.clone())).copied();
        let index = match (at_old, at_new) {
            (Some(a), Some(b)) if a != b => {
                let (keep, drop) = (a.min(b), a.max(b));
                for value in group.values_mut() {
                    if *value == drop {
                        *value = keep;
                    }
                }
                keep
            }
            (Some(a), _) | (None, Some(a)) => a,
            (None, None) => group.values().max().map_or(0, |m| m + 1),
        };
        group.insert((false, old.clone()), index);
        group.insert((true, now.clone()), index);
    }
    let mut numbered: BTreeMap<usize, usize> = BTreeMap::new();
    for ((is_new, id), index) in &group {
        let next = numbered.len();
        let at = *numbered.entry(*index).or_insert(next);
        if out.groups.len() <= at {
            out.groups.push((Vec::new(), Vec::new()));
        }
        if *is_new {
            out.groups[at].1.push(id.clone());
            out.of_new.insert(id.clone(), at);
        } else {
            out.groups[at].0.push(id.clone());
            out.of_gone.insert(id.clone(), at);
        }
    }
    out
}

/// The assertions of one file that the change rewrote in their values only:
/// gone from the base and new now, with the same shape once every literal
/// is masked (`toBe(30)` → `toBe(31)`, an inline snapshot's text), in the
/// same test or in tests that follow one another ([`successors`]: a rename,
/// a merge, a split). The failure message is left out first
/// ([`Assertion::checked`]): a message that changed is not an expected
/// value (ADR 0063).
fn expected_changes(
    path: &str,
    before: &TestFile,
    after: &TestFile,
    followed: &Successors,
    lost: &BTreeSet<usize>,
) -> Vec<Finding> {
    let mut left: BTreeMap<&str, usize> = BTreeMap::new();
    for assertion in &after.assertions {
        *left.entry(assertion.checked()).or_default() += 1;
    }
    let mut gone: Vec<&Assertion> = Vec::new();
    for assertion in &before.assertions {
        match left.get_mut(assertion.checked()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => gone.push(assertion),
        }
    }
    let mut had: BTreeMap<&str, usize> = BTreeMap::new();
    for assertion in &before.assertions {
        *had.entry(assertion.checked()).or_default() += 1;
    }
    let mut new: Vec<&Assertion> = Vec::new();
    for assertion in &after.assertions {
        match had.get_mut(assertion.checked()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => new.push(assertion),
        }
    }
    // Paired only within the same test (found on real history: a new test's
    // assertion was paired with a deleted test's), or tests that follow one
    // another (found in review, M26; ADR 0063). A test that is gone and is
    // followed by none pairs with nothing.
    let before_ids: BTreeSet<&str> = before.tests.iter().map(|t| t.id.as_str()).collect();
    let after_ids: BTreeSet<&str> = after.tests.iter().map(|t| t.id.as_str()).collect();
    let gone_ids: Vec<String> = gone
        .iter()
        .map(|a| {
            let id = test_of(before, a);
            if id.is_empty() || after_ids.contains(id) {
                id.to_owned()
            } else {
                match followed.of_gone.get(id) {
                    Some(group) => format!("\u{0}{group}"),
                    None => format!("\u{0}\u{0}{id}"),
                }
            }
        })
        .collect();
    let new_ids: Vec<String> = new
        .iter()
        .map(|a| {
            let id = test_of(after, a);
            if id.is_empty() || before_ids.contains(id) {
                id.to_owned()
            } else {
                match followed.of_new.get(id) {
                    Some(group) => format!("\u{0}{group}"),
                    None => format!("\u{0}\u{1}{id}"),
                }
            }
        })
        .collect();
    let gone_shapes: Vec<(&str, &str)> = gone
        .iter()
        .zip(&gone_ids)
        .map(|(a, id)| (id.as_str(), a.checked()))
        .collect();
    let new_shapes: Vec<(&str, &str)> = new
        .iter()
        .zip(&new_ids)
        .map(|(a, id)| (id.as_str(), a.checked()))
        .collect();
    let mut paired = approval::pairs(&gone_shapes, &new_shapes);
    // What is left of gone tests and new tests pairs as before ADR 0063,
    // any gone test with any new one, but for an assertion named lost
    // (`lost`, base lines: [`followed_losses`]). So a value changed in
    // tests whose following was not seen is still a change, and the
    // review's `upstream.calls == 1`, named lost in its renamed test, is not
    // paired with another test's `upstream.calls == 0`.
    let (old_used, new_used): (BTreeSet<usize>, BTreeSet<usize>) = (
        paired.iter().map(|(old, _)| *old).collect(),
        paired.iter().map(|(_, now)| *now).collect(),
    );
    let in_gone_test = |id: &str| id.starts_with('\u{0}');
    let rest_gone: Vec<usize> = (0..gone.len())
        .filter(|i| !old_used.contains(i) && in_gone_test(&gone_ids[*i]))
        .filter(|i| !lost.contains(&gone[*i].line))
        .collect();
    let rest_new: Vec<usize> = (0..new.len())
        .filter(|i| !new_used.contains(i) && in_gone_test(&new_ids[*i]))
        .collect();
    let any: Vec<(&str, &str)> = rest_gone.iter().map(|i| ("", gone[*i].checked())).collect();
    let any_new: Vec<(&str, &str)> = rest_new.iter().map(|i| ("", new[*i].checked())).collect();
    paired.extend(
        approval::pairs(&any, &any_new)
            .into_iter()
            .map(|(old, now)| (rest_gone[old], rest_new[now])),
    );
    paired
        .into_iter()
        .map(|(old, now)| Finding {
            kind: Kind::ExpectedChanged,
            file: path.to_owned(),
            line: new[now].line,
            on_base: false,
            what: format!(
                "{} (on the base: {}, line {})",
                new[now].text, gone[old].text, gone[old].line
            ),
        })
        .collect()
}

/// The assertions that tests followed through a rename, a merge or a split
/// ([`successors`]) lost (ADR 0063). Assertions are counted over the whole
/// change (ADR 0042), so a new test elsewhere hides one that a renamed test
/// dropped; a group of tests that follow one another is also compared with
/// itself. Only its shortfall is named, so an assertion traded for another
/// is not a loss, nor is one rewritten in its values (`expected-changed`),
/// nor one that moved to another test of the file or to another changed
/// file (more of that check there now than on the base). `excused` tests
/// (deleted with their subject) are left out.
fn followed_losses(
    path: &str,
    before: &TestFile,
    after: &TestFile,
    followed: &Successors,
    excused: &BTreeSet<&str>,
    elsewhere: &Elsewhere,
) -> Vec<Finding> {
    let mut found = Vec::new();
    for (olds, nows) in &followed.groups {
        let in_old = |a: &&Assertion| {
            let id = test_of(before, a);
            olds.iter().any(|old| old == id) && !excused.contains(id)
        };
        let in_new = |a: &&Assertion| {
            let id = test_of(after, a);
            nows.iter().any(|now| now == id)
        };
        let had: Vec<&Assertion> = before.assertions.iter().filter(in_old).collect();
        let mut has: Vec<Option<&Assertion>> =
            after.assertions.iter().filter(in_new).map(Some).collect();
        let weight =
            |list: &mut dyn Iterator<Item = &Assertion>| list.map(|a| a.weight).sum::<usize>();
        let mut short = weight(&mut had.iter().copied())
            .saturating_sub(weight(&mut has.iter().flatten().copied()));
        if short == 0 {
            continue;
        }
        // The same check kept, then the same check with other values.
        let mut take = |a: &Assertion, same: &dyn Fn(&Assertion, &Assertion) -> bool| -> bool {
            match has.iter_mut().find(|b| b.is_some_and(|b| same(a, b))) {
                Some(slot) => {
                    *slot = None;
                    true
                }
                None => false,
            }
        };
        let unmatched: Vec<&Assertion> = had
            .iter()
            .copied()
            .filter(|a| !take(a, &|a, b| a.checked() == b.checked()))
            .collect();
        let lost: Vec<&Assertion> = unmatched
            .into_iter()
            .filter(|a| {
                !take(a, &|a, b| {
                    approval::skeleton(a.checked()) == approval::skeleton(b.checked())
                })
            })
            .collect();
        // Moved: more of that check outside the group now than outside it
        // on the base, in this file or in another changed one.
        let mut outside: BTreeMap<&str, isize> = elsewhere
            .more
            .iter()
            .map(|(shape, more)| (shape.as_str(), *more))
            .collect();
        for a in after.assertions.iter().filter(|a| !in_new(a)) {
            *outside.entry(a.checked()).or_default() += 1;
        }
        for a in before.assertions.iter().filter(|a| !in_old(a)) {
            *outside.entry(a.checked()).or_default() -= 1;
        }
        let mut named = Vec::new();
        for a in lost {
            match outside.get_mut(a.checked()) {
                Some(more) if *more > 0 => {
                    *more -= 1;
                    short = short.saturating_sub(a.weight);
                }
                _ => named.push(a),
            }
        }
        let now = nows.join(", ");
        for a in named {
            if short == 0 {
                break;
            }
            short = short.saturating_sub(a.weight);
            let old = test_of(before, a);
            let how = if olds.len() == 1 && nows.len() == 1 {
                "renamed"
            } else {
                "now"
            };
            found.push(Finding {
                kind: Kind::AssertionRemoved,
                file: path.to_owned(),
                line: a.line,
                on_base: true,
                what: format!("{} (in {old}, {how} {now})", a.text),
            });
        }
    }
    found
}

/// A suppression key's tool family, how far it reaches (0: a line or an
/// item, 1: a file, a range to its end, or a module) and its rule.
fn reach_of(key: &str) -> (&str, u8, &str) {
    let (tool, rule) = key.split_once(':').unwrap_or((key, "*"));
    match tool {
        "ts" => ("ts", u8::from(rule == "nocheck"), "*"),
        "mypy-file" => ("type-ignore", 1, rule),
        _ => match tool
            .strip_suffix("-range")
            .or_else(|| tool.strip_suffix("-file"))
            .or_else(|| tool.strip_suffix("-module"))
        {
            Some(family) => (family, 1, rule),
            None => (tool, 0, rule),
        },
    }
}

fn reach_rank(key: &str) -> u8 {
    reach_of(key).1
}

/// Clippy's lint groups: allowing one allows every clippy lint in it.
const CLIPPY_GROUPS: [&str; 10] = [
    "clippy::all",
    "clippy::pedantic",
    "clippy::nursery",
    "clippy::restriction",
    "clippy::style",
    "clippy::complexity",
    "clippy::perf",
    "clippy::correctness",
    "clippy::suspicious",
    "clippy::cargo",
];

/// True when the suppression `wide` turns off at least what `narrow` does:
/// the same tool, as far or further, and every rule or the same one.
fn covers(wide: &str, narrow: &str) -> bool {
    let (family, rank, rule) = reach_of(wide);
    let (narrow_family, narrow_rank, narrow_rule) = reach_of(narrow);
    family == narrow_family
        && rank >= narrow_rank
        && (rule == "*"
            || rule == narrow_rule
            || rule == "warnings"
            || (CLIPPY_GROUPS.contains(&rule) && narrow_rule.starts_with("clippy::")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind. A new variant does not compile here until it is listed.
    fn all_kinds() -> Vec<Kind> {
        let all = vec![
            Kind::TestDeleted,
            Kind::TestSkipped,
            Kind::AssertionRemoved,
            Kind::SuppressionAdded,
            Kind::CoverageLowered,
            Kind::CiWeakened,
            Kind::NoVerify,
            Kind::RuleDisabled,
            Kind::SnapshotChanged,
            Kind::ExpectedChanged,
            Kind::NotChecked,
        ];
        for kind in &all {
            match kind {
                Kind::TestDeleted
                | Kind::TestSkipped
                | Kind::AssertionRemoved
                | Kind::SuppressionAdded
                | Kind::CoverageLowered
                | Kind::CiWeakened
                | Kind::NoVerify
                | Kind::RuleDisabled
                | Kind::SnapshotChanged
                | Kind::ExpectedChanged
                | Kind::NotChecked => {}
            }
        }
        all
    }

    /// The `tamper` object of the schema that ships with the npm package.
    fn shipped_schema() -> serde_json::Value {
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("packages")
            .join("cli")
            .join("schemas")
            .join("verify-output.schema.json");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        schema["properties"]["tamper"].clone()
    }

    #[test]
    fn every_kind_the_guard_can_report_is_in_the_shipped_schema() {
        let kinds = serde_json::to_value(all_kinds()).unwrap();
        let schema = shipped_schema();
        assert_eq!(
            schema["properties"]["findings"]["items"]["properties"]["kind"]["enum"], kinds,
            "the kind of a finding"
        );
        assert_eq!(
            schema["properties"]["allowed"]["items"]["properties"]["kinds"]["items"]["enum"], kinds,
            "the kinds of an allowed file"
        );
    }

    /// Every language the guard reads, with the name people know it by. A
    /// new variant of [`Language`] does not compile until it is listed here.
    fn all_languages() -> Vec<(Language, &'static str)> {
        let all = vec![
            (Language::Js(JsDialect::TypeScript), "JS/TS"),
            (Language::Js(JsDialect::Tsx), "JS/TS"),
            (Language::Python, "Python"),
            (Language::Rust, "Rust"),
            (Language::Java, "Java"),
            (Language::Kotlin, "Kotlin"),
            (Language::Swift, "Swift"),
            (Language::C, "C"),
            (Language::Cpp, "C++"),
            (Language::CSharp, "C#"),
            (Language::Go, "Go"),
            (Language::Dart, "Dart"),
        ];
        for (language, _) in &all {
            match language {
                Language::Js(JsDialect::TypeScript | JsDialect::Tsx)
                | Language::Python
                | Language::Rust
                | Language::Java
                | Language::Kotlin
                | Language::Swift
                | Language::C
                | Language::Cpp
                | Language::CSharp
                | Language::Go
                | Language::Dart => {}
            }
        }
        all
    }

    #[test]
    fn the_shipped_schema_names_every_language_the_guard_reads() {
        let schema = shipped_schema();
        let description = schema["description"].as_str().unwrap();
        // Whole words only: "C" is not found in "C++", nor "Java" in "JavaScript".
        let words: Vec<&str> = description
            .split(|c: char| !(c.is_alphanumeric() || matches!(c, '+' | '#' | '/')))
            .collect();
        let missing: Vec<&str> = all_languages()
            .into_iter()
            .map(|(_, name)| name)
            .filter(|name| !words.contains(name))
            .collect();
        assert_eq!(
            missing,
            Vec::<&str>::new(),
            "the description of `tamper` in verify-output.schema.json does not name these languages the guard reads: {description}"
        );
    }

    /// The keys of `value` that `schema` does not describe, as paths.
    fn undescribed(value: &serde_json::Value, schema: &serde_json::Value, at: &str) -> Vec<String> {
        match value {
            serde_json::Value::Object(map) => map
                .iter()
                .flat_map(|(key, value)| {
                    let at = format!("{at}/{key}");
                    match schema["properties"].get(key) {
                        Some(schema) => undescribed(value, schema, &at),
                        None => vec![at],
                    }
                })
                .collect(),
            serde_json::Value::Array(items) => items
                .iter()
                .flat_map(|item| undescribed(item, &schema["items"], at))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn every_field_of_the_report_is_in_the_shipped_schema() {
        let kinds = all_kinds();
        let report = TamperReport {
            policy: Policy::Enforced,
            base: "abc".to_owned(),
            files: 1,
            counts: Counts::default(),
            findings: kinds
                .iter()
                .map(|kind| Finding {
                    kind: *kind,
                    file: "a.test.ts".to_owned(),
                    line: 1,
                    on_base: false,
                    what: "x".to_owned(),
                })
                .collect(),
            not_checked: vec!["b.test.ts".to_owned()],
            allowed: vec![AllowedFile {
                file: "c.test.ts".to_owned(),
                kinds,
            }],
            approvals: Some(Approvals {
                head: "def".to_owned(),
                by: vec!["someone".to_owned()],
            }),
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            undescribed(&value, &shipped_schema(), ""),
            Vec::<String>::new()
        );
    }

    // Humanizer: an `#if` gives one loop body two heads, and tree-sitter
    // reads the whole class as an error. The first branch is read instead.
    #[test]
    fn an_if_with_two_heads_is_read_by_its_first_branch() {
        let source = "public class GenderTests\n{\n    [Fact]\n    public void Adds()\n    {\n        foreach (var name in Names)\n        {\n#if NET5_0_OR_GREATER\n            foreach (var gender in Enum.GetValues<Gender>())\n#else\n            foreach (var gender in (Gender[])Enum.GetValues(typeof(Gender)))\n#endif\n            {\n                Assert.Equal(name, Make(name, gender));\n            }\n        }\n    }\n}\n";
        let file = read_test_file(source, Language::CSharp);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
        assert_eq!(file.assertions[0].line, 14);
        // An assertion in the other branch is never dropped unseen: the file
        // is read as written, or not read.
        let hidden = source.replace("#else\n", "#else\n            Assert.True(Legacy);\n");
        let file = read_test_file(&hidden, Language::CSharp);
        assert!(file.has_errors || file.assertions.len() == 2, "{file:?}");
    }

    #[test]
    fn a_wider_suppression_covers_a_narrower_one() {
        assert!(covers("type-ignore:*", "type-ignore:union-attr"));
        assert!(covers("noqa:*", "noqa:F841"));
        assert!(covers("eslint:*", "eslint:no-console"));
        assert!(covers("eslint-range:no-console", "eslint:no-console"));
        assert!(covers(
            "allow:clippy::all",
            "allow:clippy::needless_range_loop"
        ));
        assert!(covers("allow-module:dead_code", "allow:dead_code"));
        assert!(covers("ts:nocheck", "ts:ignore"));
        assert!(covers("mypy-file:*", "type-ignore:attr-defined"));
        assert!(!covers("type-ignore:union-attr", "type-ignore:*"));
        assert!(!covers("eslint:no-console", "eslint-range:no-console"));
        assert!(!covers("allow:dead_code", "allow-module:dead_code"));
        assert!(!covers("noqa:E501", "noqa:F841"));
        assert!(!covers("allow:clippy::all", "allow:dead_code"));
        assert!(!covers("noqa:*", "type-ignore:*"));
    }

    /// The `what` of each `expected-changed` finding between two versions
    /// of one test file.
    fn changed(path: &str, base: &str, head: &str) -> Vec<String> {
        let language = Language::of(path).expect("a language the guard reads");
        let (before, after) = (
            read_test_file(base, language),
            read_test_file(head, language),
        );
        let followed = successors(&before, &after, &Elsewhere::default());
        let lost: BTreeSet<usize> = followed_losses(
            path,
            &before,
            &after,
            &followed,
            &BTreeSet::new(),
            &Elsewhere::default(),
        )
        .iter()
        .map(|lost| lost.line)
        .collect();
        expected_changes(path, &before, &after, &followed, &lost)
            .into_iter()
            .map(|finding| finding.what)
            .collect()
    }

    #[test]
    fn a_failure_message_that_changed_is_not_an_expected_value() {
        // Finding 4 of the 0.5.1 review: test_connection_slots.py:409.
        let fail = |word: &str| {
            format!(
                "import pytest\n\n\nasync def test_a_stalled_publish_does_not_hold_the_market_s_slot():\n    try:\n        await commit()\n    except TimeoutError:\n        pytest.fail(\n            \"a third request waited for a slot while two publishes stalled: \"\n            \"{word} publish runs inside the market's slot\"\n        )\n"
            )
        };
        assert_eq!(
            changed("tests/test_slots.py", &fail("the"), &fail("a")),
            Vec::<String>::new()
        );
        // test_trade.py:1026: the message after the comma of an assert.
        let idle = |place: &str| {
            format!(
                "async def test_idle():\n    idle = await idle_in_transaction()\n    assert idle == 0, f\"{{idle}} backend(s) idle in transaction across {place}\"\n"
            )
        };
        assert_eq!(
            changed(
                "tests/test_trade.py",
                &idle("the gate's call"),
                &idle("the refresh")
            ),
            Vec::<String>::new()
        );
        // unittest's msg, positional or named, and self.fail.
        let unit = |message: &str| {
            format!(
                "import unittest\n\n\nclass TestCart(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(total([1, 2]), 3, \"{message}\")\n        self.assertTrue(total([]) == 0, msg=\"{message}\")\n        self.assertAlmostEqual(mean([1, 2]), 1.5, 7, \"{message}\")\n        if broken():\n            self.fail(\"{message}\")\n"
            )
        };
        assert_eq!(
            changed("tests/test_cart.py", &unit("adds"), &unit("sums")),
            Vec::<String>::new()
        );
        // A value that changed is still one, message or not.
        assert_eq!(
            changed(
                "tests/test_trade.py",
                &idle("the gate's call"),
                &idle("the gate's call").replace("idle == 0", "idle == 1")
            )
            .len(),
            1
        );
    }

    #[test]
    fn a_failure_message_is_not_an_expected_value_in_javascript_and_rust() {
        let ts = |message: &str| {
            format!(
                "import assert from \"node:assert\";\n\nit(\"adds\", () => {{\n  expect(total([10, 20]), \"{message}\").toBe(30);\n  assert.equal(total([1]), 1, \"{message}\");\n  assert.strictEqual(total([2]), 2, \"{message}\");\n  assert.ok(total([]) === 0, \"{message}\");\n  assert(total([3]) === 3, \"{message}\");\n  if (broken()) assert.fail(\"{message}\");\n}});\n"
            )
        };
        assert_eq!(
            changed("src/cart.test.ts", &ts("adds"), &ts("sums")),
            Vec::<String>::new()
        );
        let rs = |message: &str| {
            format!(
                "#[test]\nfn adds() {{\n    assert_eq!(add(1, 2), 3, \"{message}\");\n    assert_ne!(add(1, 2), 4, \"{message}: {{}}\", 4);\n    assert!(add(0, 0) == 0, \"{message}\");\n    debug_assert_eq!(add(2, 2), 4, \"{message}\");\n}}\n"
            )
        };
        assert_eq!(
            changed("tests/add.rs", &rs("adds"), &rs("sums")),
            Vec::<String>::new()
        );
        // The values still count.
        assert_eq!(
            changed(
                "tests/add.rs",
                &rs("adds"),
                &rs("adds").replace("add(1, 2), 3", "add(1, 2), 4")
            )
            .len(),
            1
        );
    }

    #[test]
    fn an_assertion_is_never_paired_with_one_of_a_different_test() {
        // Finding 4 of the 0.5.1 review: test_trade.py:1265 against line
        // 932. The base's test was renamed and lost `upstream.calls == 1`;
        // `upstream.calls == 0` is in a new test the base does not have.
        let base = "async def test_a_trade_holds_no_connection_while_the_gate_waits_on_market_service():\n    trade = await buy(upstream)\n    assert seen == [clean]\n    assert idle == 0, f\"{idle} backend(s) idle in transaction across the gate's call\"\n    assert upstream.calls == 1\n    assert trade.state_version == 1\n";
        let head = "async def test_a_trade_holds_no_connection_while_the_gate_refreshes_the_copy():\n    trade = await buy(upstream)\n    assert seen == [clean]\n    assert idle == 0, f\"{idle} backend(s) idle in transaction across the refresh\"\n    assert trade.state_version == 1\n\n\nasync def test_a_suspended_trader_hears_about_the_suspension_not_the_market():\n    with pytest.raises(AccountSuspended):\n        await buy(upstream, suspended=True)\n    assert upstream.calls == 0\n";
        assert_eq!(
            changed("tests/test_trade.py", base, head),
            Vec::<String>::new()
        );
        // A renamed test whose value changed is still followed.
        let one = |name: &str, value: u8| {
            format!("def test_{name}():\n    assert total([10, 20]) == {value}\n    assert total([]) == 0\n")
        };
        assert_eq!(
            changed("tests/test_cart.py", &one("adds", 30), &one("sums", 31)).len(),
            1
        );
    }
    /// One test file per language with a message `MSG` in each assertion
    /// that has one, and a value to change: (path, source, value, other
    /// value).
    const MESSAGE_CASES: [(&str, &str, &str, &str); 9] = [
        (
            "calc/calc_test.go",
            "package calc\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/assert\"\n)\n\nfunc TestAdd(t *testing.T) {\n\tgot := Add(1, 2)\n\tif got != 3 {\n\t\tt.Errorf(\"Add(1, 2) = %d: MSG\", got)\n\t}\n\tassert.Equal(t, 3, Add(2, 1), \"MSG\")\n\tassert.Equalf(t, 4, Add(2, 2), \"MSG %d\", 4)\n}\n",
            "got != 3",
            "got != 4",
        ),
        (
            "src/test/java/CalcTest.java",
            "import static org.junit.jupiter.api.Assertions.*;\nimport static org.assertj.core.api.Assertions.assertThat;\n\nimport org.junit.jupiter.api.Test;\n\nclass CalcTest {\n    @Test\n    void adds() {\n        assertEquals(3, Calc.add(1, 2), \"MSG\");\n        assertTrue(Calc.add(0, 0) == 0, \"MSG\");\n        assertThat(Calc.add(2, 2)).as(\"MSG\").isEqualTo(4);\n        if (Calc.add(1, 1) != 2) {\n            fail(\"MSG\");\n        }\n        assert Calc.add(3, 3) == 6 : \"MSG\";\n    }\n}\n",
            "isEqualTo(4)",
            "isEqualTo(5)",
        ),
        (
            "src/test/kotlin/CalcTest.kt",
            "import kotlin.test.Test\nimport kotlin.test.assertEquals\nimport kotlin.test.assertTrue\n\nclass CalcTest {\n    @Test\n    fun adds() {\n        assertEquals(3, add(1, 2), \"MSG\")\n        assertTrue(add(0, 0) == 0, message = \"MSG\")\n    }\n}\n",
            "assertEquals(3,",
            "assertEquals(4,",
        ),
        (
            "Tests/CalcTests/CalcTests.swift",
            "import XCTest\n\nfinal class CalcTests: XCTestCase {\n    func testAdds() {\n        XCTAssertEqual(add(1, 2), 3, \"MSG\")\n        XCTAssertTrue(add(0, 0) == 0, \"MSG\")\n        if add(1, 1) != 2 {\n            XCTFail(\"MSG\")\n        }\n    }\n}\n",
            "add(1, 2), 3",
            "add(1, 2), 4",
        ),
        (
            "test/calc_test.dart",
            "import 'package:test/test.dart';\n\nvoid main() {\n  test('adds', () {\n    expect(add(1, 2), equals(3), reason: 'MSG');\n    if (add(1, 1) != 2) {\n      fail('MSG');\n    }\n  });\n}\n",
            "equals(3)",
            "equals(4)",
        ),
        (
            "tests/Calc.Tests/CalcTests.cs",
            "using FluentAssertions;\nusing NUnit.Framework;\n\npublic class CalcTests\n{\n    [Test]\n    public void Adds()\n    {\n        Assert.AreEqual(3, Calc.Add(1, 2), \"MSG\");\n        Assert.That(Calc.Add(2, 2), Is.EqualTo(4), \"MSG\");\n        Assert.IsTrue(Calc.Add(0, 0) == 0, \"MSG\");\n        Calc.Add(3, 3).Should().Be(6, \"MSG\");\n    }\n}\n",
            "Be(6,",
            "Be(7,",
        ),
        (
            "test/test_calc.c",
            "#include \"unity.h\"\n#include \"calc.h\"\n\nvoid test_adds(void) {\n    TEST_ASSERT_EQUAL_INT_MESSAGE(3, add(1, 2), \"MSG\");\n    if (add(1, 1) != 2) {\n        TEST_FAIL_MESSAGE(\"MSG\");\n    }\n}\n",
            "add(1, 1) != 2",
            "add(1, 1) != 3",
        ),
        (
            "tests/test_calc.py",
            "import pytest\n\n\ndef test_adds():\n    if add(1, 1) != 2:\n        pytest.fail(\"MSG\")\n",
            "!= 2",
            "!= 3",
        ),
        (
            "src/calc.test.ts",
            "import assert from \"node:assert\";\n\nit(\"adds\", () => {\n  if (add(1, 1) !== 2) {\n    assert.fail(\"MSG\");\n  }\n});\n",
            "!== 2",
            "!== 3",
        ),
    ];

    #[test]
    fn a_failure_message_is_not_an_expected_value_in_any_language() {
        for (path, source, value, other) in MESSAGE_CASES {
            assert_eq!(
                changed(
                    path,
                    &source.replace("MSG", "one"),
                    &source.replace("MSG", "two")
                ),
                Vec::<String>::new(),
                "{path}: only the message changed"
            );
            assert_eq!(
                changed(
                    path,
                    &source.replace("MSG", "one"),
                    &source.replace("MSG", "one").replace(value, other)
                )
                .len(),
                1,
                "{path}: {value} became {other}"
            );
        }
    }
    /// The findings of the guard on a git repo whose base commit holds
    /// `base` and whose working tree holds `head`, as `Kind path:line what`.
    fn guarded(base: &[(&str, &str)], head: &[(&str, &str)]) -> Vec<String> {
        use std::process::Command;
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        let write = |files: &[(&str, &str)]| {
            for (path, text) in files {
                let file = root.join(path);
                std::fs::create_dir_all(file.parent().expect("a folder")).expect("mkdir");
                std::fs::write(file, text).expect("write");
            }
        };
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args([
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(root)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        write(base);
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&["add", "--all"]);
        git(&["commit", "--quiet", "--no-verify", "-m", "base"]);
        let commit = git(&["rev-parse", "HEAD"]);
        for (path, _) in base {
            std::fs::remove_file(root.join(path)).expect("remove");
        }
        write(head);
        let changed: BTreeSet<String> = base
            .iter()
            .chain(head)
            .map(|(path, _)| (*path).to_owned())
            .collect();
        let base = BaseCommit {
            reference: "main".to_owned(),
            commit,
        };
        guard(root, &base, &changed)
            .findings
            .into_iter()
            .map(|f| format!("{:?} {}:{} {}", f.kind, f.file, f.line, f.what))
            .collect()
    }

    #[test]
    fn a_test_moved_or_split_to_another_file_is_not_a_rename_of_a_test_left_behind() {
        let get_user = "def test_get_user():\n    resp = get(\"/users/1\")\n    assert resp.status_code == 200\n    assert resp.json()[\"name\"] == \"alice\"\n";
        let health =
            "def test_health():\n    resp = get(\"/health\")\n    assert resp.status_code == 200\n";
        assert_eq!(
            guarded(
                &[("tests/test_api.py", get_user)],
                &[
                    ("tests/test_api.py", health),
                    ("tests/test_users.py", get_user)
                ]
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            guarded(
                &[(
                    "tests/test_totals.py",
                    "def test_totals():\n    assert f() == 1\n    assert g() == 2\n"
                )],
                &[
                    (
                        "tests/test_totals.py",
                        "def test_total_f():\n    assert f() == 1\n"
                    ),
                    (
                        "tests/test_total_g.py",
                        "def test_total_g():\n    assert g() == 2\n"
                    )
                ]
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_value_changed_in_tests_merged_or_split_is_an_expected_change() {
        assert_eq!(
            guarded(
                &[(
                    "tests/test_math.py",
                    "def test_adds():\n    assert g() == 2\n\n\ndef test_sums():\n    assert f() == 1\n"
                )],
                &[(
                    "tests/test_math.py",
                    "def test_adds_and_sums():\n    assert g() == 2\n    assert f() == 5\n\n\ndef test_empty():\n    assert h() == 0\n"
                )]
            ),
            vec!["ExpectedChanged tests/test_math.py:3 assert f() == 5 (on the base: assert f() == 1, line 6)"]
        );
        assert_eq!(
            guarded(
                &[(
                    "tests/test_totals.py",
                    "def test_totals():\n    assert f() == 1\n    assert g() == 2\n"
                )],
                &[(
                    "tests/test_totals.py",
                    "def test_totals_a():\n    assert g() == 2\n\n\ndef test_totals_b():\n    assert f() == 5\n"
                )]
            ),
            vec!["ExpectedChanged tests/test_totals.py:6 assert f() == 5 (on the base: assert f() == 1, line 2)"]
        );
    }

    #[test]
    fn an_assertion_a_renamed_test_lost_is_named_once() {
        assert_eq!(
            guarded(
                &[(
                    "tests/test_x.py",
                    "def test_old():\n    assert x() == 1, \"one\"\n    assert y() == 2\n"
                )],
                &[(
                    "tests/test_x.py",
                    "def test_new():\n    assert x() == 1, \"two\"\n"
                )]
            ),
            vec!["AssertionRemoved tests/test_x.py:3 assert y() == 2 (in test_old, renamed test_new)"]
        );
    }

    /// Changes whose value sits where a message could be: each must stay a
    /// finding (ADR 0063: when unsure, a value). (path, base, head).
    const VALUES_NOT_MESSAGES: [(&str, &str, &str); 8] = [
        (
            "calc/calc_test.go",
            "package calc\n\nimport (\n\t\"testing\"\n\n\t\"gotest.tools/v3/assert\"\n)\n\nfunc TestRun(t *testing.T) {\n\terr := Run()\n\tassert.Error(t, err, \"boom\")\n}\n",
            "package calc\n\nimport (\n\t\"testing\"\n\n\t\"gotest.tools/v3/assert\"\n)\n\nfunc TestRun(t *testing.T) {\n\terr := Run()\n\tassert.Error(t, err, \"bang\")\n}\n",
        ),
        (
            "tests/Names.Tests/NamesTests.cs",
            "using FluentAssertions;\nusing Xunit;\n\npublic class NamesTests\n{\n    [Fact]\n    public void Lists()\n    {\n        Names().Should().BeEquivalentTo(\"alice\", \"bob\");\n    }\n}\n",
            "using FluentAssertions;\nusing Xunit;\n\npublic class NamesTests\n{\n    [Fact]\n    public void Lists()\n    {\n        Names().Should().BeEquivalentTo(\"alice\", \"carol\");\n    }\n}\n",
        ),
        (
            "tests/rows.rs",
            "#[test]\nfn reads_a_row() {\n    let row = first_row();\n    assert_eq!(row.get::<_, String>(0).unwrap(), \"alice\");\n}\n",
            "#[test]\nfn reads_a_row() {\n    let row = first_row();\n    assert_eq!(row.get::<_, String>(0).unwrap(), \"bob\");\n}\n",
        ),
        (
            "src/test/java/UserTest.java",
            "import static org.junit.Assert.assertEquals;\n\nimport org.junit.Test;\n\npublic class UserTest {\n    @Test\n    public void name() {\n        assertEquals(\"name\", user.getName(), \"alice\");\n    }\n}\n",
            "import static org.junit.Assert.assertEquals;\n\nimport org.junit.Test;\n\npublic class UserTest {\n    @Test\n    public void name() {\n        assertEquals(\"name\", user.getName(), \"bob\");\n    }\n}\n",
        ),
        (
            "Tests/CalcTests/CalcTests.swift",
            "import XCTest\n\nfinal class CalcTests: XCTestCase {\n    func testAdds() {\n        guard add(1, 1) == 2 else {\n            XCTFail(\"want 2\")\n            return\n        }\n    }\n}\n",
            "import XCTest\n\nfinal class CalcTests: XCTestCase {\n    func testAdds() {\n        guard add(1, 1) == 3 else {\n            XCTFail(\"want 3\")\n            return\n        }\n    }\n}\n",
        ),
        (
            "calc/switch_test.go",
            "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tgot := Add(1, 2)\n\tswitch {\n\tcase got != 3:\n\t\tt.Errorf(\"want 3\")\n\t}\n}\n",
            "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tgot := Add(1, 2)\n\tswitch {\n\tcase got != 4:\n\t\tt.Errorf(\"want 4\")\n\t}\n}\n",
        ),
        (
            "tests/test_match.py",
            "import pytest\n\n\ndef test_adds():\n    match add(1, 1):\n        case 2:\n            pass\n        case _:\n            pytest.fail(\"want 2\")\n",
            "import pytest\n\n\ndef test_adds():\n    match add(1, 1):\n        case 3:\n            pass\n        case _:\n            pytest.fail(\"want 3\")\n",
        ),
        (
            "src/switch.test.ts",
            "import assert from \"node:assert\";\n\nit(\"adds\", () => {\n  switch (add(1, 1)) {\n    case 2:\n      break;\n    default:\n      assert.fail(\"want 2\");\n  }\n});\n",
            "import assert from \"node:assert\";\n\nit(\"adds\", () => {\n  switch (add(1, 1)) {\n    case 3:\n      break;\n    default:\n      assert.fail(\"want 3\");\n  }\n});\n",
        ),
    ];

    #[test]
    fn a_value_where_a_message_could_be_is_still_an_expected_change() {
        let missed: Vec<&str> = VALUES_NOT_MESSAGES
            .iter()
            .filter(|(path, base, head)| changed(path, base, head).len() != 1)
            .map(|(path, _, _)| *path)
            .collect();
        assert_eq!(missed, Vec::<&str>::new());
    }

    #[test]
    fn a_junit_4_message_and_a_swift_guard_message_are_not_expected_values() {
        let junit4 = |message: &str| {
            format!("import static org.junit.Assert.assertEquals;\n\nimport org.junit.Test;\n\npublic class UserTest {{\n    @Test\n    public void name() {{\n        assertEquals(\"{message}\", \"alice\", user.getName());\n    }}\n}}\n")
        };
        assert_eq!(
            changed(
                "src/test/java/UserTest.java",
                &junit4("name"),
                &junit4("the name")
            ),
            Vec::<String>::new()
        );
        let guard = |message: &str| {
            format!("import XCTest\n\nfinal class CalcTests: XCTestCase {{\n    func testAdds() {{\n        guard add(1, 1) == 2 else {{\n            XCTFail(\"{message}\")\n            return\n        }}\n    }}\n}}\n")
        };
        assert_eq!(
            changed(
                "Tests/CalcTests/CalcTests.swift",
                &guard("want 2"),
                &guard("wanted 2")
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_junit_4_assert_called_on_its_class_after_a_wildcard_import_keeps_its_values() {
        // `import org.junit.*;` names no `org.junit.Assert`, and
        // `Assert.assertEquals` takes its message first (JUnit 4).
        let junit4 = |message: &str, value: &str| {
            format!("import org.junit.*;\n\npublic class UserTest {{\n    @Test\n    public void name() {{\n        Assert.assertEquals(\"{message}\", user.getName(), \"{value}\");\n    }}\n}}\n")
        };
        let path = "src/test/java/UserTest.java";
        assert_eq!(
            changed(path, &junit4("name", "alice"), &junit4("name", "bob")).len(),
            1
        );
        assert_eq!(
            changed(path, &junit4("name", "alice"), &junit4("the name", "alice")),
            Vec::<String>::new()
        );
    }
}
