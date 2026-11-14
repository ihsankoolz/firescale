//! The tamper guard (M23, M24; ADR 0042, ADR 0044): finds a change that
//! passes verify by weakening its tests or silencing its checks, in
//! JavaScript, TypeScript, Python (pytest and unittest) and Rust; and since
//! M32 (ADR 0052) also in Swift (XCTest and Swift Testing) and Dart
//! (`package:test` and `flutter_test`).
//!
//! - It compares syntax trees, not diff lines: each changed test file is
//!   parsed with tree-sitter on the base commit and in the working tree, and
//!   its tests (by suite path and name), skipped tests and assertions are
//!   counted. A reformatted test, or one moved within or between files,
//!   counts the same. Code that certainly never runs (`if (false)`, after a
//!   `return`) and assertions that cannot fail (`expect(true).toBe(true)`,
//!   `assert True`) are not counted.
//! - It compares **net counts over the whole change**, so a renamed test, a
//!   test moved to another file, a test split in two and an assertion
//!   replaced by a stricter one are not findings. Lint suppressions
//!   (`eslint-disable`, `# noqa`, `#[allow]`, `// swiftlint:disable`,
//!   `// ignore:`) are counted the same way, per rule, in every changed
//!   source file.
//! - A test deleted together with every module of the repo it uses (its
//!   subject) is not a finding.
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
mod rust;
mod suppress;
mod swift;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Serialize;

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
    /// C++: GoogleTest and Catch2 (M32).
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
/// `tests`; Swift `*Tests.swift`, `*Test.swift` or in `tests` or a folder
/// ending in `Tests` (`Tests/`, `AppTests/`, `AppUITests/`); Dart
/// `*_test.dart` or in `test`, `integration_test`. A Rust, Swift or Dart
/// file elsewhere is a test file when it holds tests ([`holds_tests`]); the
/// policy can name more (`verify.tamper.testPaths`).
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
/// Rust unit tests (`#[test]`, `#[cfg(test)]`) next to the code, Swift that
/// imports XCTest or Testing and defines a test, Dart that imports
/// `package:test` or `package:flutter_test` and calls `test`.
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
        Language::Rust
        | Language::Java
        | Language::Kotlin
        | Language::C
        | Language::Cpp
        | Language::CSharp
        | Language::Go => None,
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
            .map(|text| read_test_file(text, file.language));
        let head_file = match &file.head {
            Ok(Some(text)) => Some(read_test_file(text, file.language)),
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
    let mut base_shapes: Vec<(String, String, String, usize)> = Vec::new();
    let mut head_shapes: BTreeMap<String, usize> = BTreeMap::new();
    let counts = &mut report.counts;
    for (file, base_file, head_file) in &pairs {
        let path = &file.path;
        // Assertions that changed only in their values, file by file (M26).
        if let (Some(before), Some(after)) = (base_file, head_file) {
            if !allows(path, Kind::ExpectedChanged) {
                approval_findings.extend(expected_changes(path, before, after));
            }
        }
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
                for _ in 0..assertion.weight {
                    base_shapes.push((
                        assertion.shape.clone(),
                        assertion.text.clone(),
                        path.clone(),
                        assertion.line,
                    ));
                }
            }
        }
        if let Some(read) = head_file {
            for test in &read.tests {
                if count_tests && !test.marker {
                    counts.head_tests += test.weight;
                    *head_ids.entry(test.id.clone()).or_default() += test.weight;
                }
                if count_skips {
                    let skipped = skipped_weight(test);
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
                    *head_shapes.entry(assertion.shape.clone()).or_default() += assertion.weight;
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
    // Only while no test of the base went away: a test renamed and skipped
    // has a new id too, and must not pass for a new test (found in review).
    let gone_any = base_ids
        .iter()
        .any(|(id, _, _)| !head_ids.contains_key(id.as_str()));
    let excused_skips = if gone_any {
        0
    } else {
        new_skipped.min(added_tests)
    };
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
    if counts.head_assertions < counts.base_assertions {
        let mut left = head_shapes;
        let mut missing: Vec<&(String, String, String, usize)> = Vec::new();
        for entry in &base_shapes {
            match left.get_mut(&entry.0) {
                Some(count) if *count > 0 => *count -= 1,
                _ => missing.push(entry),
            }
        }
        let shortfall = counts.base_assertions - counts.head_assertions;
        let mut named: BTreeSet<(&String, usize)> = BTreeSet::new();
        for (_, shown, file, line) in missing.into_iter().take(shortfall) {
            if named.insert((file, *line)) {
                findings.push(Finding {
                    kind: Kind::AssertionRemoved,
                    file: file.clone(),
                    line: *line,
                    on_base: true,
                    what: shown.clone(),
                });
            }
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

/// The assertions of one file that the change rewrote in their values only:
/// gone from the base and new now, with the same shape once every literal
/// is masked (`toBe(30)` → `toBe(31)`, an inline snapshot's text).
fn expected_changes(path: &str, before: &TestFile, after: &TestFile) -> Vec<Finding> {
    let mut left: BTreeMap<&str, usize> = BTreeMap::new();
    for assertion in &after.assertions {
        *left.entry(assertion.shape.as_str()).or_default() += 1;
    }
    let mut gone: Vec<&Assertion> = Vec::new();
    for assertion in &before.assertions {
        match left.get_mut(assertion.shape.as_str()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => gone.push(assertion),
        }
    }
    let mut had: BTreeMap<&str, usize> = BTreeMap::new();
    for assertion in &before.assertions {
        *had.entry(assertion.shape.as_str()).or_default() += 1;
    }
    let mut new: Vec<&Assertion> = Vec::new();
    for assertion in &after.assertions {
        match had.get_mut(assertion.shape.as_str()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => new.push(assertion),
        }
    }
    // Paired only within the same test (found on real history: a new test's
    // assertion was paired with a deleted test's).
    let test_of = |file: &TestFile, assertion: &Assertion| -> String {
        assertion
            .test
            .and_then(|index| file.tests.get(index))
            .map(|test| test.id.clone())
            .unwrap_or_default()
    };
    // A test renamed with its value changed: an assertion of a test that is
    // gone may pair with one of a test that is new (found in review).
    let before_ids: BTreeSet<&str> = before.tests.iter().map(|t| t.id.as_str()).collect();
    let after_ids: BTreeSet<&str> = after.tests.iter().map(|t| t.id.as_str()).collect();
    const RENAMED: &str = "\u{0}renamed";
    let gone_ids: Vec<String> = gone
        .iter()
        .map(|a| {
            let id = test_of(before, a);
            if !id.is_empty() && !after_ids.contains(id.as_str()) {
                RENAMED.to_owned()
            } else {
                id
            }
        })
        .collect();
    let new_ids: Vec<String> = new
        .iter()
        .map(|a| {
            let id = test_of(after, a);
            if !id.is_empty() && !before_ids.contains(id.as_str()) {
                RENAMED.to_owned()
            } else {
                id
            }
        })
        .collect();
    let gone_shapes: Vec<(&str, &str)> = gone
        .iter()
        .zip(&gone_ids)
        .map(|(a, id)| (id.as_str(), a.shape.as_str()))
        .collect();
    let new_shapes: Vec<(&str, &str)> = new
        .iter()
        .zip(&new_ids)
        .map(|(a, id)| (id.as_str(), a.shape.as_str()))
        .collect();
    approval::pairs(&gone_shapes, &new_shapes)
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
}
