//! The tamper guard (M23, ADR 0042): finds a change that passes verify by
//! weakening its tests, for JavaScript and TypeScript.
//!
//! - It compares syntax trees, not diff lines: each changed test file is
//!   parsed with tree-sitter on the base commit and in the working tree, and
//!   its tests (by `describe` path and title), skipped tests and assertions
//!   are counted. A reformatted test, or one moved within or between files,
//!   counts the same.
//! - It compares **net counts over the whole change**, so a renamed test, a
//!   test moved to another file, a test split in two and an assertion
//!   replaced by a stricter one are not findings.
//! - A test deleted together with every module of the repo it uses (its
//!   subject) is not a finding.
//! - The base is the one blame-aware verify resolves (ADR 0037). The policy
//!   (`verify.tamper`) is read from the base commit's config, never the
//!   working tree's, so a change cannot switch the guard off for itself.
//! - Findings are report-only unless the base's policy is `enforced`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Serialize;
use tree_sitter::{Language, Node};

use crate::blame::BaseCommit;
use crate::git::run_git;
use crate::imports::{js_dialect, js_string, parse, text, walk, JsDialect};

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

/// One test call (`it`, `test` and their forms) in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// The titles of the enclosing `describe` blocks and its own, joined
    /// with ` > `. A title that is not a plain string is its source text.
    pub id: String,
    /// 1-based line of the call.
    pub line: usize,
    /// `.skip`, `.todo`, `.skipIf`, `.fixme`, `x…`, or inside a skipped
    /// `describe`.
    pub skipped: bool,
    /// `.only`, `f…`, or inside a focused `describe`: the file's other tests
    /// do not run.
    pub focused: bool,
    /// The identifiers its arguments use: what it tests.
    pub uses: BTreeSet<String>,
}

/// One assertion: an `expect(…)` or `assert…(…)` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub line: usize,
    /// The whole chained call (`expect(a).toBe(1)`), without white space.
    pub shape: String,
    /// The index of the test it is in, if any.
    pub test: Option<usize>,
}

/// What a test file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestFile {
    pub tests: Vec<TestCase>,
    pub assertions: Vec<Assertion>,
    /// Imported names and the module each comes from (`import`, `require`).
    pub imports: BTreeMap<String, String>,
    /// tree-sitter found syntax errors, so the counts cannot be trusted.
    pub has_errors: bool,
}

const TEST_NAMES: [&str; 5] = ["it", "test", "xit", "xtest", "fit"];
const SUITE_NAMES: [&str; 6] = [
    "describe",
    "xdescribe",
    "fdescribe",
    "suite",
    "context",
    "xcontext",
];
const SKIP_MODIFIERS: [&str; 4] = ["skip", "todo", "skipIf", "fixme"];

/// The dotted name a call's function is called by (`it`, `it.skip`,
/// `test.each` for `test.each(table)(…)`), or `None` for anything else.
fn callee(node: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" => Some(vec![text(node, src).to_owned()]),
        "member_expression" => {
            let mut path = callee(node.child_by_field_name("object")?, src)?;
            path.push(text(node.child_by_field_name("property")?, src).to_owned());
            Some(path)
        }
        // `it.each(table)("adds %i", …)` and `test.skipIf(cond)("…", …)`.
        "call_expression" => callee(node.child_by_field_name("function")?, src),
        _ => None,
    }
}

/// The title of a test or suite call: its first argument, as a string when
/// it is one, else its source text.
fn title(call: Node<'_>, src: &[u8]) -> String {
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return String::new();
    };
    let mut cursor = arguments.walk();
    let Some(first) = arguments.named_children(&mut cursor).next() else {
        return String::new();
    };
    js_string(first, src).unwrap_or_else(|| {
        text(first, src)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// How a test or suite call is marked.
fn marks(path: &[String]) -> (bool, bool) {
    let first = path.first().map_or("", String::as_str);
    let skipped = first.starts_with('x')
        || path[1..]
            .iter()
            .any(|part| SKIP_MODIFIERS.contains(&part.as_str()));
    let focused =
        (first.starts_with('f') && first != "fixme") || path[1..].iter().any(|part| part == "only");
    (skipped, focused)
}

/// True for the function of an assertion call: `expect(…)`, `assert(…)`,
/// `assert.equal(…)`.
fn is_assertion(function: Node<'_>, src: &[u8]) -> bool {
    match function.kind() {
        "identifier" => matches!(text(function, src), "expect" | "assert"),
        "member_expression" => function
            .child_by_field_name("object")
            .is_some_and(|object| object.kind() == "identifier" && text(object, src) == "assert"),
        _ => false,
    }
}

/// The outermost call or member chain that starts with `node`
/// (`expect(a)` → `expect(a).not.toBe(1)`).
fn chain(node: Node<'_>) -> Node<'_> {
    let mut top = node;
    while let Some(parent) = top.parent() {
        let continues = match parent.kind() {
            "member_expression" => parent.child_by_field_name("object") == Some(top),
            "call_expression" => parent.child_by_field_name("function") == Some(top),
            "await_expression" => true,
            _ => false,
        };
        if !continues {
            break;
        }
        top = parent;
    }
    top
}

/// The names a file imports, with the module each comes from.
fn imports(root: Node<'_>, src: &[u8]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    walk(root, |node| match node.kind() {
        "import_statement" => {
            let Some(source) = node
                .child_by_field_name("source")
                .and_then(|source| js_string(source, src))
            else {
                return false;
            };
            walk(node, |inner| match inner.kind() {
                "import_specifier" => {
                    let local = inner
                        .child_by_field_name("alias")
                        .or_else(|| inner.child_by_field_name("name"));
                    if let Some(local) = local {
                        found.insert(text(local, src).to_owned(), source.clone());
                    }
                    false
                }
                "identifier" => {
                    found.insert(text(inner, src).to_owned(), source.clone());
                    false
                }
                "string" => false,
                _ => true,
            });
            false
        }
        "variable_declarator" => {
            let required = node.child_by_field_name("value").and_then(|value| {
                let function = value.child_by_field_name("function")?;
                if value.kind() != "call_expression" || text(function, src) != "require" {
                    return None;
                }
                let arguments = value.child_by_field_name("arguments")?;
                let mut cursor = arguments.walk();
                let first = arguments.named_children(&mut cursor).next()?;
                js_string(first, src)
            });
            if let (Some(source), Some(name)) = (required, node.child_by_field_name("name")) {
                walk(name, |inner| {
                    if matches!(
                        inner.kind(),
                        "identifier" | "shorthand_property_identifier_pattern"
                    ) {
                        found.insert(text(inner, src).to_owned(), source.clone());
                    }
                    true
                });
            }
            true
        }
        _ => true,
    });
    found
}

/// Reads the tests, skips and assertions of a JS/TS test file.
#[must_use]
pub fn read_test_file(source: &str, dialect: JsDialect) -> TestFile {
    let language: Language = match dialect {
        JsDialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        JsDialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    };
    let Some(tree) = parse(source, &language) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    let mut file = TestFile {
        imports: imports(root, src),
        has_errors: root.has_error(),
        ..TestFile::default()
    };
    // Each suite and test call, with the byte range it covers.
    struct Block {
        end: usize,
        title: String,
        skipped: bool,
        focused: bool,
        test: Option<usize>,
    }
    let mut open: Vec<Block> = Vec::new();
    walk(root, |node| {
        while open
            .last()
            .is_some_and(|block| node.start_byte() >= block.end)
        {
            open.pop();
        }
        if node.kind() != "call_expression" {
            return true;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return true;
        };
        if is_assertion(function, src) {
            let whole = chain(node);
            file.assertions.push(Assertion {
                line: node.start_position().row + 1,
                shape: text(whole, src).split_whitespace().collect(),
                test: open.iter().rev().find_map(|block| block.test),
            });
            return true;
        }
        let Some(path) = callee(function, src) else {
            return true;
        };
        let first = path[0].as_str();
        let is_test = TEST_NAMES.contains(&first);
        let is_suite = SUITE_NAMES.contains(&first);
        // `it.each(table)` alone is the inner call of `it.each(table)(…)`;
        // only the outer call is the test.
        let inner_of_table = node.parent().is_some_and(|parent| {
            parent.kind() == "call_expression"
                && parent.child_by_field_name("function") == Some(node)
        });
        if !(is_test || is_suite) || inner_of_table {
            return true;
        }
        let (own_skip, own_focus) = marks(&path);
        let skipped = own_skip || open.iter().any(|block| block.skipped);
        let focused = own_focus || open.iter().any(|block| block.focused);
        let own_title = title(node, src);
        let mut test = None;
        if is_test {
            let id = open
                .iter()
                .map(|block| block.title.as_str())
                .chain(std::iter::once(own_title.as_str()))
                .collect::<Vec<_>>()
                .join(" > ");
            let mut uses = BTreeSet::new();
            if let Some(arguments) = node.child_by_field_name("arguments") {
                walk(arguments, |inner| {
                    if inner.kind() == "identifier" {
                        uses.insert(text(inner, src).to_owned());
                    }
                    true
                });
            }
            test = Some(file.tests.len());
            file.tests.push(TestCase {
                id,
                line: node.start_position().row + 1,
                skipped,
                focused,
                uses,
            });
        }
        open.push(Block {
            end: node.end_byte(),
            title: own_title,
            skipped,
            focused,
            test,
        });
        true
    });
    file
}

/// True for a path the guard reads as a test file: a JS/TS file named
/// `*.test.*` or `*.spec.*`, or in a folder named `__tests__`, `test`,
/// `tests` or `spec`.
#[must_use]
pub fn is_test_path(path: &str) -> bool {
    if js_dialect(path).is_none() {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let name = parts
        .last()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains(".test.")
        || name.contains(".spec.")
        || parts[..parts.len() - 1]
            .iter()
            .any(|part| matches!(*part, "__tests__" | "test" | "tests" | "spec"))
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
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TestDeleted => "test deleted",
            Self::TestSkipped => "test skipped",
            Self::AssertionRemoved => "assertion removed",
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
    /// The test's id, or the assertion's text.
    pub what: String,
}

/// Net counts over the changed test files.
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
}

impl TamperReport {
    /// True when the findings fail verify.
    #[must_use]
    pub fn blocks(&self) -> bool {
        self.policy == Policy::Enforced && !self.findings.is_empty()
    }
}

/// The policy in the base commit's `firescale.config.json`: `evaluated`
/// when it has none, or its config cannot be read as JSON (a
/// `firescale.config.ts` is not run for this).
#[must_use]
pub fn base_policy(root: &Path, base: &BaseCommit) -> Policy {
    show(root, &base.commit, "firescale.config.json")
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|config| {
            config
                .get("verify")?
                .get("tamper")?
                .as_str()
                .and_then(Policy::parse)
        })
        .unwrap_or(Policy::Evaluated)
}

/// A file as the base commit has it, `None` when it has none.
fn show(root: &Path, commit: &str, path: &str) -> Option<String> {
    let out = run_git(root, &["show", &format!("{commit}:./{path}")]).ok()?;
    (out.code == 0).then_some(out.stdout)
}

/// The repo path an import of `spec` from `from` (a file) names, among
/// `exists`; `None` for a package or a path that names no such file.
fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if !spec.starts_with('.') {
        return None;
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in spec.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    let stem = joined
        .strip_suffix(".js")
        .or_else(|| joined.strip_suffix(".jsx"))
        .or_else(|| joined.strip_suffix(".mjs"))
        .unwrap_or(&joined)
        .to_owned();
    let mut candidates = vec![joined.clone()];
    for extension in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"] {
        candidates.push(format!("{stem}.{extension}"));
        candidates.push(format!("{joined}/index.{extension}"));
    }
    candidates.into_iter().find(|candidate| exists(candidate))
}

/// Compares the test files that `changed` (paths that differ between the
/// base commit and the working tree) holds, on the base and in the working
/// tree.
#[must_use]
pub fn guard(root: &Path, base: &BaseCommit, changed: &BTreeSet<String>) -> TamperReport {
    let policy = base_policy(root, base);
    let mut report = TamperReport {
        policy,
        base: base.commit.clone(),
        files: 0,
        counts: Counts::default(),
        findings: Vec::new(),
        not_checked: Vec::new(),
    };
    if policy == Policy::Off {
        return report;
    }
    let on_disk = |path: &str| {
        path.split('/')
            .fold(root.to_path_buf(), |dir, part| dir.join(part))
    };
    // What the change deleted: on the base, gone from the working tree.
    let deleted: BTreeSet<&String> = changed
        .iter()
        .filter(|path| !on_disk(path).exists())
        .collect();
    let mut pairs: Vec<(String, Option<TestFile>, Option<TestFile>)> = Vec::new();
    for path in changed.iter().filter(|path| is_test_path(path)) {
        let Some(dialect) = js_dialect(path) else {
            continue;
        };
        let base_file = show(root, &base.commit, path).map(|text| read_test_file(&text, dialect));
        let head_file = fs::read_to_string(on_disk(path))
            .ok()
            .map(|text| read_test_file(&text, dialect));
        if base_file.is_none() && head_file.is_none() {
            continue;
        }
        let broken = |file: &Option<TestFile>| file.as_ref().is_some_and(|file| file.has_errors);
        if broken(&base_file) || broken(&head_file) {
            report.not_checked.push(path.clone());
            continue;
        }
        pairs.push((path.clone(), base_file, head_file));
    }
    report.files = pairs.len();

    // A test whose subject the change deleted: it uses names imported from
    // modules of the repo, and every such module is deleted.
    let existed_on_base = |path: &str| deleted.contains(&path.to_owned()) || on_disk(path).exists();
    let excused = |path: &str, file: &TestFile, test: &TestCase| -> bool {
        let modules: BTreeSet<String> = test
            .uses
            .iter()
            .filter_map(|name| file.imports.get(name))
            .filter_map(|spec| resolve(path, spec, &existed_on_base))
            .collect();
        !modules.is_empty() && modules.iter().all(|module| deleted.contains(module))
    };

    let mut base_ids: Vec<(String, String, usize)> = Vec::new();
    let mut head_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut base_skipped_ids: BTreeSet<String> = BTreeSet::new();
    let mut head_skips: Vec<(String, usize, String)> = Vec::new();
    let mut base_focus = 0;
    let mut head_focus: Vec<(String, usize, String)> = Vec::new();
    let mut base_shapes: Vec<(String, String, usize)> = Vec::new();
    let mut head_shapes: BTreeMap<String, usize> = BTreeMap::new();
    let counts = &mut report.counts;
    for (path, base_file, head_file) in &pairs {
        let head_has: BTreeSet<&str> = head_file
            .iter()
            .flat_map(|file| file.tests.iter().map(|test| test.id.as_str()))
            .collect();
        if let Some(file) = base_file {
            let mut excused_tests: BTreeSet<usize> = BTreeSet::new();
            for (index, test) in file.tests.iter().enumerate() {
                if !head_has.contains(test.id.as_str()) && excused(path, file, test) {
                    excused_tests.insert(index);
                    counts.excused_tests += 1;
                    continue;
                }
                counts.base_tests += 1;
                if test.skipped {
                    counts.base_skipped += 1;
                    base_skipped_ids.insert(test.id.clone());
                }
                if test.focused {
                    base_focus += 1;
                }
                base_ids.push((test.id.clone(), path.clone(), test.line));
            }
            for assertion in &file.assertions {
                if assertion
                    .test
                    .is_some_and(|test| excused_tests.contains(&test))
                {
                    counts.excused_assertions += 1;
                    continue;
                }
                counts.base_assertions += 1;
                base_shapes.push((assertion.shape.clone(), path.clone(), assertion.line));
            }
        }
        if let Some(file) = head_file {
            for test in &file.tests {
                counts.head_tests += 1;
                *head_ids.entry(test.id.clone()).or_default() += 1;
                if test.skipped {
                    counts.head_skipped += 1;
                    head_skips.push((path.clone(), test.line, test.id.clone()));
                }
                if test.focused {
                    head_focus.push((path.clone(), test.line, test.id.clone()));
                }
            }
            counts.head_assertions += file.assertions.len();
            for assertion in &file.assertions {
                *head_shapes.entry(assertion.shape.clone()).or_default() += 1;
            }
        }
    }

    let mut findings = Vec::new();
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
        for (id, file, line) in missing.into_iter().take(shortfall) {
            findings.push(Finding {
                kind: Kind::TestDeleted,
                file: file.clone(),
                line: *line,
                on_base: true,
                what: id.clone(),
            });
        }
    }
    // More skipped tests, or a new focus.
    if counts.head_skipped > counts.base_skipped {
        let extra = counts.head_skipped - counts.base_skipped;
        for (file, line, id) in head_skips
            .iter()
            .filter(|(_, _, id)| !base_skipped_ids.contains(id))
            .take(extra)
        {
            findings.push(Finding {
                kind: Kind::TestSkipped,
                file: file.clone(),
                line: *line,
                on_base: false,
                what: id.clone(),
            });
        }
    }
    if head_focus.len() > base_focus {
        for (file, line, id) in head_focus.iter().take(head_focus.len() - base_focus) {
            findings.push(Finding {
                kind: Kind::TestSkipped,
                file: file.clone(),
                line: *line,
                on_base: false,
                what: format!("{id} (.only: the file's other tests do not run)"),
            });
        }
    }
    // Fewer assertions: name those whose text is gone, up to the shortfall.
    if counts.head_assertions < counts.base_assertions {
        let mut left = head_shapes;
        let mut missing: Vec<&(String, String, usize)> = Vec::new();
        for entry in &base_shapes {
            match left.get_mut(&entry.0) {
                Some(count) if *count > 0 => *count -= 1,
                _ => missing.push(entry),
            }
        }
        let shortfall = counts.base_assertions - counts.head_assertions;
        for (shape, file, line) in missing.into_iter().take(shortfall) {
            findings.push(Finding {
                kind: Kind::AssertionRemoved,
                file: file.clone(),
                line: *line,
                on_base: true,
                what: shape.clone(),
            });
        }
    }
    findings.sort_by(|a, b| (a.kind, &a.file, a.line).cmp(&(b.kind, &b.file, b.line)));
    report.findings = findings;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(source: &str) -> TestFile {
        read_test_file(source, JsDialect::TypeScript)
    }

    #[test]
    fn tests_suites_skips_focus_and_assertions_are_read() {
        let file = ts(r#"
import { add } from "../src/math";
describe("math", () => {
  it("adds", () => {
    expect(add(1, 2)).toBe(3);
    expect(add(0, 0)).not.toBe(1);
  });
  it.skip("skipped", () => { assert.equal(1, 1); });
  test.todo("later");
  xit("x skipped", () => {});
  describe.skip("off", () => { test("inside", () => { assert(true); }); });
  it.only("focused", async () => { await expect(p).resolves.toBe(1); });
  it.each([1, 2])("table %i", (n) => { expect(n).toBeTruthy(); });
});
"#);
        assert!(!file.has_errors);
        let ids: Vec<(&str, bool, bool)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.focused))
            .collect();
        assert_eq!(
            ids,
            [
                ("math > adds", false, false),
                ("math > skipped", true, false),
                ("math > later", true, false),
                ("math > x skipped", true, false),
                ("math > off > inside", true, false),
                ("math > focused", false, true),
                ("math > table %i", false, false),
            ]
        );
        let shapes: Vec<&str> = file.assertions.iter().map(|a| a.shape.as_str()).collect();
        assert_eq!(
            shapes,
            [
                "expect(add(1,2)).toBe(3)",
                "expect(add(0,0)).not.toBe(1)",
                "assert.equal(1,1)",
                "assert(true)",
                "awaitexpect(p).resolves.toBe(1)",
                "expect(n).toBeTruthy()",
            ]
        );
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(
            file.imports.get("add").map(String::as_str),
            Some("../src/math")
        );
        assert!(file.tests[0].uses.contains("add"));
    }

    #[test]
    fn calls_in_strings_and_comments_are_not_tests() {
        let file = ts("// it('no', () => {})\nconst s = \"it('no')\";\n");
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        assert!(ts("it('x', () => { expect(1).toBe(1) ").has_errors);
    }

    #[test]
    fn test_paths() {
        for path in [
            "src/a.test.ts",
            "a.spec.js",
            "src/__tests__/a.ts",
            "test/a.mjs",
            "pkg/tests/x.tsx",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in ["src/a.ts", "testing/a.ts", "src/a.test.py", "README.md"] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn imports_resolve_to_repo_files() {
        let exists = |path: &str| matches!(path, "src/math.ts" | "src/util/index.js");
        assert_eq!(
            resolve("test/a.test.ts", "../src/math", &exists).as_deref(),
            Some("src/math.ts")
        );
        assert_eq!(
            resolve("test/a.test.ts", "../src/math.js", &exists).as_deref(),
            Some("src/math.ts")
        );
        assert_eq!(
            resolve("test/a.test.ts", "../src/util", &exists).as_deref(),
            Some("src/util/index.js")
        );
        assert_eq!(resolve("test/a.test.ts", "vitest", &exists), None);
    }
}
