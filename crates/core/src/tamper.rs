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
    /// How many tests it stands for: the rows of an `each` table that is an
    /// array literal (its own and its suites'), else 1.
    pub weight: usize,
}

/// One assertion: an `expect(…)` or `assert…(…)` call.
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
    /// Imported names and the module each comes from (`import`, `require`).
    pub imports: BTreeMap<String, String>,
    /// tree-sitter found syntax errors, so the counts cannot be trusted.
    pub has_errors: bool,
    /// The line of the first one.
    pub error_line: Option<usize>,
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
/// Modifiers that make a test not run, run only sometimes, or pass when it
/// fails (`it.fails`, Playwright's `test.fail`).
const SKIP_MODIFIERS: [&str; 7] = ["skip", "todo", "skipIf", "runIf", "fixme", "fails", "fail"];

/// Members of `test` in Playwright that are not tests: hooks, steps and
/// settings. `test.describe` is a suite.
const NOT_TESTS: [&str; 11] = [
    "beforeEach",
    "afterEach",
    "beforeAll",
    "afterAll",
    "step",
    "use",
    "extend",
    "info",
    "setTimeout",
    "slow",
    "expect",
];

/// How deep a callee or an assertion chain is followed: real test code
/// never comes near it, and it keeps a chain of thousands of calls linear.
const MAX_CHAIN: usize = 32;

/// The dotted name a call's function is called by (`it`, `it.skip`,
/// `test.each` for `test.each(table)(…)`), or `None` for anything else.
/// A loop, not recursion, and at most [`MAX_CHAIN`] steps deep.
fn callee(node: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    let mut path: Vec<String> = Vec::new();
    let mut at = node;
    for _ in 0..MAX_CHAIN {
        match at.kind() {
            "identifier" => {
                path.push(text(at, src).to_owned());
                path.reverse();
                return Some(path);
            }
            "member_expression" => {
                path.push(text(at.child_by_field_name("property")?, src).to_owned());
                at = at.child_by_field_name("object")?;
            }
            // `it.each(table)("adds %i", …)` and `test.skipIf(cond)("…", …)`.
            "call_expression" => at = at.child_by_field_name("function")?,
            _ => return None,
        }
    }
    None
}

/// For `it.each([…])(…)` and `describe.each([…])(…)`: the number of rows of
/// the table when it is an array literal, else 1.
fn table_rows(function: Node<'_>) -> usize {
    if function.kind() != "call_expression" {
        return 1;
    }
    let Some(arguments) = function.child_by_field_name("arguments") else {
        return 1;
    };
    let mut cursor = arguments.walk();
    let first = arguments.named_children(&mut cursor).next();
    match first {
        Some(array) if array.kind() == "array" => {
            let mut cursor = array.walk();
            array.named_children(&mut cursor).count().max(1)
        }
        _ => 1,
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
/// `assert.equal(…)`, and `expect.soft(…)`, `expect.poll(…)`,
/// `expect.assertions(…)`, `expect.hasAssertions()`.
fn is_assertion(function: Node<'_>, src: &[u8]) -> bool {
    match function.kind() {
        "identifier" => matches!(text(function, src), "expect" | "assert"),
        "member_expression" => {
            let property = function
                .child_by_field_name("property")
                .map_or("", |property| text(property, src));
            function
                .child_by_field_name("object")
                .is_some_and(|object| {
                    object.kind() == "identifier"
                        && match text(object, src) {
                            "assert" => true,
                            "expect" => {
                                matches!(property, "soft" | "poll" | "assertions" | "hasAssertions")
                            }
                            _ => false,
                        }
                })
        }
        _ => false,
    }
}

/// The outermost call or member chain that starts with `node`
/// (`expect(a)` → `expect(a).not.toBe(1)`).
fn chain(node: Node<'_>) -> Node<'_> {
    let mut top = node;
    for _ in 0..MAX_CHAIN {
        let Some(parent) = top.parent() else {
            break;
        };
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
    if file.has_errors {
        walk(root, |node| {
            if file.error_line.is_none() && (node.is_error() || node.is_missing()) {
                file.error_line = Some(node.start_position().row + 1);
            }
            file.error_line.is_none() && node.has_error()
        });
    }
    // Each suite and test call, with the byte range it covers.
    struct Block {
        end: usize,
        title: String,
        skipped: bool,
        focused: bool,
        test: Option<usize>,
        /// The rows of its `each` table times its suites'.
        weight: usize,
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
            let inside = open.iter().rev().find(|block| block.test.is_some());
            file.assertions.push(Assertion {
                line: node.start_position().row + 1,
                shape: text(whole, src).split_whitespace().collect(),
                text: text(whole, src)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                test: inside.and_then(|block| block.test),
                weight: inside.map_or(1, |block| block.weight),
            });
            return true;
        }
        let Some(path) = callee(function, src) else {
            return true;
        };
        let first = path[0].as_str();
        let second = path.get(1).map(String::as_str);
        // Playwright: `test.describe(…)` is a suite; `test.beforeEach`,
        // `test.step` and the like are not tests.
        let playwright_suite = first == "test" && second == Some("describe");
        let not_a_test = first == "test" && second.is_some_and(|name| NOT_TESTS.contains(&name));
        let is_test = TEST_NAMES.contains(&first) && !playwright_suite && !not_a_test;
        let is_suite = SUITE_NAMES.contains(&first) || playwright_suite;
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
        let rows = if path.iter().any(|part| part == "each") {
            table_rows(function)
        } else {
            1
        };
        let weight = rows.saturating_mul(open.last().map_or(1, |block| block.weight));
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
                weight,
            });
        }
        open.push(Block {
            end: node.end_byte(),
            title: own_title,
            skipped,
            focused,
            test,
            weight,
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
        .and_then(|text| {
            serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok()
        })
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
    // What the change deleted: on the base, gone from the working tree, and
    // not the old side of a rename git sees.
    let renamed: BTreeSet<String> = crate::blame::renames_since(root, base)
        .into_keys()
        .collect();
    let deleted: BTreeSet<&String> = changed
        .iter()
        .filter(|path| !on_disk(path).exists() && !renamed.contains(*path))
        .collect();
    // Every identifier in the JS/TS files of the change that still exist:
    // a name a deleted test used is "deleted with it" only if it is in none
    // of them (a module renamed without git seeing it keeps its names).
    let mut surviving: BTreeSet<String> = BTreeSet::new();
    for path in changed.iter().filter(|path| js_dialect(path).is_some()) {
        if let Ok(bytes) = fs::read(on_disk(path)) {
            for word in String::from_utf8_lossy(&bytes)
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                .filter(|word| !word.is_empty())
            {
                surviving.insert(word.to_owned());
            }
        }
    }
    let mut not_checked: Vec<Finding> = Vec::new();
    let mut pairs: Vec<(String, Option<TestFile>, Option<TestFile>)> = Vec::new();
    for path in changed.iter().filter(|path| is_test_path(path)) {
        let Some(dialect) = js_dialect(path) else {
            continue;
        };
        let base_file = show(root, &base.commit, path).map(|text| read_test_file(&text, dialect));
        // Bytes, decoded leniently: a stray Latin-1 byte in a comment must
        // not make a file read as deleted.
        let head_file = match fs::read(on_disk(path)) {
            Ok(bytes) => Some(read_test_file(&String::from_utf8_lossy(&bytes), dialect)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(TestFile {
                has_errors: true,
                ..TestFile::default()
            }),
        };
        if base_file.is_none() && head_file.is_none() {
            continue;
        }
        // A file tree-sitter cannot read is never taken as clean: it is a
        // finding of its own, which counts and blocks like the others.
        let broken = [(&head_file, false), (&base_file, true)]
            .into_iter()
            .find_map(|(file, on_base)| {
                file.as_ref()
                    .filter(|file| file.has_errors)
                    .map(|file| (file.error_line.unwrap_or(1), on_base))
            });
        if let Some((line, on_base)) = broken {
            report.not_checked.push(path.clone());
            not_checked.push(Finding {
                kind: Kind::NotChecked,
                file: path.clone(),
                line,
                on_base,
                what: "tree-sitter cannot read this file, so its tests and assertions were not compared".to_owned(),
            });
            continue;
        }
        pairs.push((path.clone(), base_file, head_file));
    }
    report.files = pairs.len();

    // A test whose subject the change deleted: it uses names imported from
    // modules of the repo, every such module is deleted, and none of those
    // names is left in the change.
    let existed_on_base =
        |path: &str| deleted.contains(&path.to_owned()) || on_disk(path).is_file();
    let excused = |path: &str, file: &TestFile, test: &TestCase| -> bool {
        let mut modules: BTreeSet<String> = BTreeSet::new();
        let mut names: Vec<&String> = Vec::new();
        for name in &test.uses {
            if let Some(module) = file
                .imports
                .get(name)
                .and_then(|spec| resolve(path, spec, &existed_on_base))
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
    let mut base_skipped_ids: BTreeSet<String> = BTreeSet::new();
    let mut head_skips: Vec<(String, usize, String)> = Vec::new();
    let mut base_focus: BTreeSet<String> = BTreeSet::new();
    let mut head_focus: Vec<(String, usize, String)> = Vec::new();
    let mut base_shapes: Vec<(String, String, String, usize)> = Vec::new();
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
                    counts.excused_tests += test.weight;
                    continue;
                }
                counts.base_tests += test.weight;
                if test.skipped {
                    counts.base_skipped += test.weight;
                    base_skipped_ids.insert(test.id.clone());
                }
                if test.focused {
                    base_focus.insert(test.id.clone());
                }
                for _ in 0..test.weight {
                    base_ids.push((test.id.clone(), path.clone(), test.line));
                }
            }
            for assertion in &file.assertions {
                if assertion
                    .test
                    .is_some_and(|test| excused_tests.contains(&test))
                {
                    counts.excused_assertions += assertion.weight;
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
        if let Some(file) = head_file {
            for test in &file.tests {
                counts.head_tests += test.weight;
                *head_ids.entry(test.id.clone()).or_default() += test.weight;
                if test.skipped {
                    counts.head_skipped += test.weight;
                    head_skips.push((path.clone(), test.line, test.id.clone()));
                }
                if test.focused {
                    head_focus.push((path.clone(), test.line, test.id.clone()));
                }
            }
            for assertion in &file.assertions {
                counts.head_assertions += assertion.weight;
                *head_shapes.entry(assertion.shape.clone()).or_default() += assertion.weight;
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
    // More skipped tests.
    if counts.head_skipped > counts.base_skipped {
        for (file, line, id) in head_skips
            .iter()
            .filter(|(_, _, id)| !base_skipped_ids.contains(id))
            .take(counts.head_skipped - counts.base_skipped)
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
    fn a_very_long_call_chain_is_read_without_deep_recursion() {
        let chain = ".then(f)".repeat(20_000);
        let source = format!("it(\"t\", () => {{ expect(p{chain}).toBe(1); }});\n");
        let started = std::time::Instant::now();
        let file = ts(&source);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
        // Linear: far below what a quadratic walk of 20,000 calls takes.
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn table_rows_weigh_their_tests_and_assertions() {
        let file = ts("describe.each([[1], [2]])(\"s\", () => {\n  it.each([[1], [2], [3]])(\"t\", () => { expect(1).toBe(1); });\n});\n");
        assert_eq!(file.tests[0].weight, 6);
        assert_eq!(file.assertions[0].weight, 6);
    }

    #[test]
    fn playwright_hooks_and_steps_are_not_tests() {
        let file = ts("test.describe(\"home\", () => {\n  test.beforeEach(async () => {});\n  test(\"a\", async () => { await test.step(\"s\", async () => {}); });\n  test.describe.skip(\"off\", () => { test(\"b\", () => {}); });\n});\n");
        let ids: Vec<(&str, bool)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped))
            .collect();
        assert_eq!(ids, [("home > a", false), ("home > off > b", true)]);
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
