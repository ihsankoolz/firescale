//! The tamper guard (M23, M24; ADR 0042, ADR 0044): finds a change that
//! passes verify by weakening its tests or silencing its checks, in
//! JavaScript, TypeScript, Python (pytest and unittest) and Rust.
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
//!   (`eslint-disable`, `# noqa`, `#[allow]`) are counted the same way, per
//!   rule, in every changed source file.
//! - A test deleted together with every module of the repo it uses (its
//!   subject) is not a finding.
//! - The base is the one blame-aware verify resolves (ADR 0037). The policy
//!   (`verify.tamper`: mode, extra test paths, allowed exceptions) is read
//!   from the base commit's config, never the working tree's, so a change
//!   cannot loosen the guard for itself.
//! - Findings are report-only unless the base's policy is `enforced`.

mod js;
mod policy;
mod python;
mod reach;
mod rust;
mod suppress;

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
    }
}

/// True for a path the guard reads as a test file by its name or folder
/// alone: JS/TS `*.test.*`, `*.spec.*` or in `__tests__`, `test`, `tests`,
/// `spec`; Python `test_*.py`, `*_test.py` or in `test`, `tests`; Rust in
/// `tests`. A Rust file elsewhere is a test file when it holds tests
/// ([`holds_tests`]); the policy can name more (`verify.tamper.testPaths`).
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    match Language::of(path) {
        Some(Language::Js(_)) => js::is_test_path(path),
        Some(Language::Python) => python::is_test_path(path),
        Some(Language::Rust) => rust::is_test_path(path),
        None => false,
    }
}

/// True for source that holds tests although its path does not say so:
/// Rust unit tests (`#[test]`, `#[cfg(test)]`) next to the code.
#[must_use]
pub fn holds_tests(source: &str, language: Language) -> bool {
    language == Language::Rust && rust::holds_tests(source)
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
        Language::Rust => None,
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
    /// A changed test file tree-sitter cannot read, on the base or now: its
    /// tests and assertions could not be compared.
    NotChecked,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TestDeleted => "test deleted",
            Self::TestSkipped => "test skipped",
            Self::AssertionRemoved => "assertion removed",
            Self::SuppressionAdded => "suppression added",
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
}

impl TamperReport {
    /// True when the findings fail verify.
    #[must_use]
    pub fn blocks(&self) -> bool {
        self.policy == Policy::Enforced && !self.findings.is_empty()
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
    let policy = base_policy(root, base);
    let mut report = TamperReport {
        policy: policy.mode,
        base: base.commit.clone(),
        files: 0,
        counts: Counts::default(),
        findings: Vec::new(),
        not_checked: Vec::new(),
        allowed: Vec::new(),
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
    let wanted: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
    let mut on_base = crate::git::files_at(root, &base.commit, &wanted);
    let files: Vec<Changed> = sources
        .into_iter()
        .map(|(path, language)| Changed {
            path: path.clone(),
            language,
            base: on_base
                .remove(path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            head: match fs::read(on_disk(path)) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(()),
            },
        })
        .collect();

    // What the change deleted: on the base, gone from the working tree, and
    // not the old side of a rename git sees.
    let renamed: BTreeSet<String> = crate::blame::renames_since(root, base)
        .into_keys()
        .collect();
    let deleted: BTreeSet<&String> = changed
        .iter()
        .filter(|path| !on_disk(path).exists() && !renamed.contains(*path))
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
    const KINDS: [Kind; 5] = [
        Kind::TestDeleted,
        Kind::TestSkipped,
        Kind::AssertionRemoved,
        Kind::SuppressionAdded,
        Kind::NotChecked,
    ];
    for file in &files {
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
        if let Some(text) = &file.base {
            for found in suppressions(text, file.language) {
                *base_keys.entry(found.key.clone()).or_default() += 1;
                *base_texts
                    .entry((file.path.clone(), found.key, found.text))
                    .or_default() += 1;
                report.counts.base_suppressions += 1;
            }
        }
        if let Ok(Some(text)) = &file.head {
            for found in suppressions(text, file.language) {
                report.counts.head_suppressions += 1;
                head_suppressions.push((file.path.clone(), found));
            }
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
    let existed_on_base =
        |path: &str| deleted.contains(&path.to_owned()) || on_disk(path).is_file();
    let excused = |file: &Changed, read: &TestFile, test: &TestCase| -> bool {
        let mut modules: BTreeSet<String> = BTreeSet::new();
        let mut names: Vec<&String> = Vec::new();
        for name in &test.uses {
            if let Some(module) = read
                .imports
                .get(name)
                .and_then(|spec| resolve(file.language, &file.path, spec, &existed_on_base))
            {
                modules.insert(module);
                names.push(name);
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
                if count_tests {
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
                if count_tests {
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
    if counts.head_skipped > counts.base_skipped {
        let mut shortfall = counts.head_skipped - counts.base_skipped;
        let mut left = base_skipped_ids;
        for skip in &head_skips {
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
        findings.push(Finding {
            kind: Kind::SuppressionAdded,
            file: path.clone(),
            line: found.line,
            on_base: false,
            what: format!("{} ({})", found.text, found.key),
        });
    }
    findings.sort_by(|a, b| (a.kind, &a.file, a.line).cmp(&(b.kind, &b.file, b.line)));
    report.findings = findings;
    report
}
