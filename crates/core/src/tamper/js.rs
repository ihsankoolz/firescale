//! The JavaScript and TypeScript reader of the tamper guard (M23, ADR 0042).

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::{Assertion, TestCase, TestFile};
use crate::imports::{js_dialect, js_string, parse, text, walk, JsDialect};

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

/// What the JS/TS grammars call the nodes of [`reach`].
const GRAMMAR: Grammar = Grammar {
    blocks: &["statement_block", "program"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &["while_statement"],
    hoisted: &["function_declaration", "generator_function_declaration"],
    stops: None,
    noop: |node, _| node.kind() == "empty_statement",
};

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 16] = [
    "number",
    "string",
    "string_fragment",
    "escape_sequence",
    "template_string",
    "true",
    "false",
    "null",
    "undefined",
    "binary_expression",
    "unary_expression",
    "parenthesized_expression",
    "array",
    "regex",
    "regex_pattern",
    "regex_flags",
];

/// True for an assertion that cannot fail: it checks only literals, and
/// they certainly pass. `expect(<literal>)` with `toBe`, `toEqual` or
/// `toStrictEqual` of the same literal, `toBeTruthy` of a truthy one or
/// `toBeFalsy` of a falsy one; `assert(<truthy>)`, `assert.ok(<truthy>)`,
/// `assert.equal`, `strictEqual`, `deepEqual` or `deepStrictEqual` of the
/// same literal twice. An assertion that certainly fails
/// (`expect(true).toBe(false)`, `assert.fail(…)`) marks code that must not
/// be reached and counts like any other; so does anything else
/// (`.not`, `.resolves`, `expect.assertions(n)`, other matchers).
fn cannot_fail(whole: Node<'_>, src: &[u8]) -> bool {
    // The chain from the outside in: the arguments of each call, and the
    // names between them, down to `expect` or `assert`.
    let mut calls: Vec<Vec<Node<'_>>> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    let mut at = whole;
    let root = loop {
        if calls.len() + names.len() > MAX_CHAIN {
            return false;
        }
        match at.kind() {
            "call_expression" => {
                let mut list = Vec::new();
                if let Some(arguments) = at.child_by_field_name("arguments") {
                    let mut cursor = arguments.walk();
                    list.extend(
                        arguments
                            .named_children(&mut cursor)
                            .filter(|n| !n.is_extra()),
                    );
                }
                calls.push(list);
                match at.child_by_field_name("function") {
                    Some(function) => at = function,
                    None => return false,
                }
            }
            "member_expression" => {
                names.push(
                    at.child_by_field_name("property")
                        .map_or("", |property| text(property, src)),
                );
                match at.child_by_field_name("object") {
                    Some(object) => at = object,
                    None => return false,
                }
            }
            "identifier" => break text(at, src),
            _ => return false,
        }
    };
    let literal = |node: &Node<'_>| reach::constant(*node, &LITERALS);
    let same = |a: &Node<'_>, b: &Node<'_>| {
        literal(a)
            && literal(b)
            && text(*a, src).split_whitespace().collect::<String>()
                == text(*b, src).split_whitespace().collect::<String>()
    };
    let truthy = |node: &Node<'_>| match node.kind() {
        "true" | "array" | "regex" => true,
        "number" => text(*node, src).parse::<f64>().is_ok_and(|n| n != 0.0),
        "string" => js_string(*node, src).is_some_and(|value| !value.is_empty()),
        _ => false,
    };
    let falsy = |node: &Node<'_>| match node.kind() {
        "false" | "null" | "undefined" => true,
        "number" => text(*node, src).parse::<f64>().is_ok_and(|n| n == 0.0),
        "string" => js_string(*node, src).is_some_and(|value| value.is_empty()),
        _ => false,
    };
    match (root, names.as_slice(), calls.as_slice()) {
        // `expect(subject).matcher(…)`: the outer call is the matcher's.
        ("expect", [matcher], [expected, subject]) if subject.len() == 1 => {
            let subject = &subject[0];
            match (*matcher, expected.as_slice()) {
                ("toBe" | "toEqual" | "toStrictEqual", [value]) => same(subject, value),
                ("toBeTruthy", []) => truthy(subject),
                ("toBeFalsy", []) => falsy(subject),
                _ => false,
            }
        }
        ("assert", [], [arguments]) | ("assert", ["ok"], [arguments]) => {
            arguments.first().is_some_and(truthy)
        }
        ("assert", [method], [arguments])
            if matches!(
                *method,
                "equal" | "strictEqual" | "deepEqual" | "deepStrictEqual"
            ) =>
        {
            matches!(arguments.as_slice(), [a, b, ..] if same(a, b))
        }
        _ => false,
    }
}

/// Why a test call runs nothing: it has no function (`it("x")`), or its
/// function's body does nothing (`() => {}`, `() => { return; … }`).
/// `None` when it runs something, or its function is not written inline.
fn empty_body(call: Node<'_>, src: &[u8]) -> Option<&'static str> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let all: Vec<Node<'_>> = arguments
        .named_children(&mut cursor)
        .filter(|n| !n.is_extra())
        .collect();
    if all.len() == 1 {
        return Some("it has no body");
    }
    let function = all.iter().rev().find(|argument| {
        matches!(
            argument.kind(),
            "arrow_function" | "function_expression" | "function"
        )
    })?;
    let body = function.child_by_field_name("body")?;
    (body.kind() == "statement_block" && reach::does_nothing(body, src, &GRAMMAR))
        .then_some("its body does nothing")
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
pub(super) fn read(source: &str, dialect: JsDialect) -> TestFile {
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
    let dead = reach::Dead::of(root, src, &GRAMMAR);
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
            // Code that never runs, and an assertion that cannot fail
            // (`expect(true).toBe(true)`), are not counted (ADR 0044).
            if dead.why(node).is_some() || cannot_fail(whole, src) {
                return true;
            }
            let inside = open.iter().rev().find(|block| block.test.is_some());
            file.assertions.push(Assertion {
                line: node.start_position().row + 1,
                shape: text(whole, src).split_whitespace().collect(),
                text: text(whole, src)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                test: inside.and_then(|block| block.test),
                conditional: false,
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
        // A test or suite that never runs, or a test whose body runs
        // nothing, is skipped as surely as one marked `.skip` (ADR 0044).
        let why = dead.why(node).or_else(|| {
            (is_test && !own_skip)
                .then(|| empty_body(node, src))
                .flatten()
        });
        let skipped = own_skip || why.is_some() || open.iter().any(|block| block.skipped);
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
                skipped_rows: 0,
                why,
                marker: false,
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
pub(super) fn is_test_path(path: &str) -> bool {
    if js_dialect(path).is_none() {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let name = parts
        .last()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    // `*.test-d.ts` and `test-d/` are tsd's type tests.
    name.contains(".test.")
        || name.contains(".spec.")
        || name.contains(".test-d.")
        || parts[..parts.len() - 1]
            .iter()
            .any(|part| matches!(*part, "__tests__" | "test" | "tests" | "spec" | "test-d"))
}

/// The repo path an import of `spec` from `from` (a file) names, among
/// `exists`; `None` for a package or a path that names no such file.
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(source: &str) -> TestFile {
        read(source, JsDialect::TypeScript)
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
  it.skip("skipped", () => { assert.equal(add(1, 0), 1); });
  test.todo("later");
  xit("x skipped", () => {});
  describe.skip("off", () => { test("inside", () => { assert(ok); }); });
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
                "assert.equal(add(1,0),1)",
                "assert(ok)",
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
        let file = ts("describe.each([[1], [2]])(\"s\", () => {\n  it.each([[1], [2], [3]])(\"t\", (n) => { expect(n).toBe(1); });\n});\n");
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
    fn code_that_never_runs_is_not_counted() {
        let file = ts(r#"
it("returns early", () => {
  return;
  expect(total()).toBe(3);
});
it("if false", () => {
  if (false) {
    expect(total()).toBe(3);
  }
  expect(other()).toBe(1);
});
if (false) {
  it("hidden", () => { expect(total()).toBe(3); });
}
describe("after return", () => {
  return;
  it("never registered", () => { expect(total()).toBe(3); });
});
it("hoisted helper", () => {
  check();
  return;
  function check() { expect(total()).toBe(3); }
});
it("conditional return", () => {
  if (ready()) { return; }
  expect(total()).toBe(3);
});
"#);
        let tests: Vec<(&str, bool, Option<&str>)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.why))
            .collect();
        assert_eq!(
            tests,
            [
                ("returns early", true, Some("its body does nothing")),
                ("if false", false, None),
                ("hidden", true, Some("inside if (false)")),
                (
                    "after return > never registered",
                    true,
                    Some("after a return")
                ),
                ("hoisted helper", false, None),
                ("conditional return", false, None),
            ]
        );
        let lines: Vec<usize> = file.assertions.iter().map(|a| a.line).collect();
        // Only `other()`, the hoisted helper's and the one after a
        // conditional return run.
        assert_eq!(lines, [10, 22, 26]);
    }

    #[test]
    fn an_assertion_that_cannot_fail_is_not_counted() {
        let file = ts(r#"
it("placeholders", () => {
  expect(true).toBe(true);
  expect(1).toBe(1);
  expect(true).toBeTruthy();
  expect(1 + 1).toEqual(2);
  assert(true);
  assert.ok(1);
  assert.equal("a", "a");
  expect.assertions(2);
  expect.hasAssertions();
  expect(total).toBe(true);
  expect(true).toBe(isReady());
  expect(screen.getByText("x")).toBeTruthy();
  expect(true).toBe(false);
  assert.fail("should have thrown");
  expect(null).toBeFalsy();
  expect(1).not.toBe(2);
});
"#);
        // Only literals that certainly pass are left out: arithmetic is not
        // worked out, and an assertion that certainly fails marks code that
        // must not be reached.
        let shapes: Vec<&str> = file.assertions.iter().map(|a| a.shape.as_str()).collect();
        assert_eq!(
            shapes,
            [
                "expect(1+1).toEqual(2)",
                "expect.assertions(2)",
                "expect.hasAssertions()",
                "expect(total).toBe(true)",
                "expect(true).toBe(isReady())",
                "expect(screen.getByText(\"x\")).toBeTruthy()",
                "expect(true).toBe(false)",
                "assert.fail(\"shouldhavethrown\")",
                "expect(1).not.toBe(2)",
            ]
        );
    }

    #[test]
    fn a_test_with_an_empty_body_is_skipped() {
        let file = ts(r#"
it("empty", () => {});
it("only a comment", () => {
  // TODO
});
it("no function");
it("expression body", () => expect(total()).toBe(3));
it("a named function", runCase);
it.todo("later");
"#);
        let tests: Vec<(&str, bool, Option<&str>)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.why))
            .collect();
        assert_eq!(
            tests,
            [
                ("empty", true, Some("its body does nothing")),
                ("only a comment", true, Some("its body does nothing")),
                ("no function", true, Some("it has no body")),
                ("expression body", false, None),
                ("a named function", false, None),
                ("later", true, None),
            ]
        );
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
            "index.test-d.ts",
            "test-d/hooks.ts",
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
