//! The Rust reader of the tamper guard (M24, ADR 0044).
//!
//! - A test is a `fn` with `#[test]` or an attribute whose path ends in
//!   `test` (`#[tokio::test]`, `#[async_std::test(…)]`), or `#[rstest]`,
//!   `#[test_case(…)]`, `#[wasm_bindgen_test]`. Its id is the names of the
//!   modules around it and its own, joined with `::` (`tests::adds`).
//! - It is skipped with `#[ignore]`, with an `ignore` inside `cfg_attr`
//!   (it runs only sometimes), under `#[cfg(any())]` (never compiled), or
//!   when its body does nothing. `#[should_panic]` is a running test.
//! - An assertion is a macro call whose name starts with `assert`,
//!   `debug_assert` or `prop_assert`, unless it never runs or cannot fail
//!   (`assert!(true)`, `assert_eq!(1, 1)`).
//! - The names a file defines outside its test code are its own subject
//!   ([`super::SELF`]), so unit tests deleted together with the file they
//!   sit in are not a finding.
//!
//! tree-sitter does not parse what is inside a macro call: tests and
//! assertions written inside one (`proptest! { #[test] fn … }`, an
//! `assert!` inside `quickcheck! { … }`) are not counted, on either side.

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::{Assertion, TestCase, TestFile, SELF};
use crate::imports::{parse, text, walk};

/// What the Rust grammar calls the nodes [`reach`] looks at.
const GRAMMAR: Grammar = Grammar {
    blocks: &["block"],
    returns: &["return_expression"],
    ifs: &["if_expression"],
    whiles: &["while_expression"],
    hoisted: &["function_item", "mod_item", "impl_item", "trait_item"],
    stops: None,
    noop: is_noop,
};

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 9] = [
    "integer_literal",
    "float_literal",
    "string_literal",
    "raw_string_literal",
    "string_content",
    "char_literal",
    "boolean_literal",
    "escape_sequence",
    "token_tree",
];

/// Items whose name a file defines: its subject, when outside test code.
const ITEMS: [&str; 9] = [
    "function_item",
    "struct_item",
    "enum_item",
    "union_item",
    "const_item",
    "static_item",
    "trait_item",
    "type_item",
    "macro_definition",
];

/// Why `why` says a test does not run under `#[cfg(any())]`.
const NEVER_COMPILED: &str = "under #[cfg(any())], which is never true";

fn is_noop(statement: Node<'_>, _src: &[u8]) -> bool {
    statement.kind() == "empty_statement"
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

/// The `attribute`s written right before an item: its `attribute_item`
/// siblings, comments between them skipped, in source order.
fn outer_attributes(item: Node<'_>) -> Vec<Node<'_>> {
    let mut found = Vec::new();
    let mut at = item.prev_sibling();
    while let Some(sibling) = at {
        if sibling.kind() == "attribute_item" {
            if let Some(attribute) = sibling.named_child(0) {
                found.push(attribute);
            }
        } else if !is_comment(sibling) {
            break;
        }
        at = sibling.prev_sibling();
    }
    found.reverse();
    found
}

/// The `#![…]` attributes written directly in `container`: a file or a
/// module's body.
fn inner_attributes(container: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "inner_attribute_item")
        .filter_map(|child| child.named_child(0))
        .collect()
}

/// An attribute's path without white space (`tokio::test`), and the last
/// segment of it (`test`).
fn attribute_path<'s>(attribute: Node<'_>, src: &'s [u8]) -> (String, &'s str) {
    let Some(path) = attribute.named_child(0) else {
        return (String::new(), "");
    };
    let last = if path.kind() == "scoped_identifier" {
        path.child_by_field_name("name")
            .map_or("", |name| text(name, src))
    } else {
        text(path, src)
    };
    (text(path, src).split_whitespace().collect(), last)
}

/// The arguments of an attribute (`(any())`) without white space.
fn attribute_arguments(attribute: Node<'_>, src: &[u8]) -> String {
    attribute
        .child_by_field_name("arguments")
        .map(|arguments| text(arguments, src).split_whitespace().collect())
        .unwrap_or_default()
}

/// The top-level arguments of a token tree (`(a, b + 1, "c")`): the nodes
/// between its delimiters, split at its own commas.
fn split_arguments(tree: Node<'_>) -> Vec<Vec<Node<'_>>> {
    let mut cursor = tree.walk();
    let children: Vec<Node<'_>> = tree.children(&mut cursor).collect();
    let delimiter = |node: &Node<'_>| {
        !node.is_named() && matches!(node.kind(), "(" | ")" | "[" | "]" | "{" | "}")
    };
    let mut inner: &[Node<'_>] = &children;
    if inner.first().is_some_and(delimiter) {
        inner = &inner[1..];
    }
    if inner.last().is_some_and(delimiter) {
        inner = &inner[..inner.len() - 1];
    }
    let mut arguments: Vec<Vec<Node<'_>>> = vec![Vec::new()];
    for node in inner {
        if !node.is_named() && node.kind() == "," {
            arguments.push(Vec::new());
        } else if let Some(argument) = arguments.last_mut() {
            argument.push(*node);
        }
    }
    if arguments.last().is_some_and(Vec::is_empty) {
        // A trailing comma, or no arguments at all.
        arguments.pop();
    }
    arguments
}

/// What the attributes of an item say about it.
#[derive(Default)]
struct Marks {
    /// A test attribute: `#[test]` and its forms.
    test: bool,
    /// `#[ignore]`, or an `ignore` inside `cfg_attr`.
    ignored: bool,
    /// `#[cfg(any())]`: never compiled.
    never: bool,
    /// `#[cfg(test)]`: compiled only for tests.
    cfg_test: bool,
    /// Any other `#[cfg(…)]` (`feature = "slow"`, `unix`, `not(test)`,
    /// `all(test, …)`): compiled only sometimes, like a conditional skip.
    gated: bool,
    /// A `#[cfg(…)]` that names `test` (`test`, `all(test, …)`).
    names_test: bool,
    /// `#[rstest]`.
    rstest: bool,
    /// `#[case(…)]` attributes, for `rstest`.
    cases: usize,
    /// `#[test_case(…)]` attributes.
    test_cases: usize,
}

fn marks(attributes: &[Node<'_>], src: &[u8]) -> Marks {
    let mut found = Marks::default();
    for attribute in attributes {
        let (path, last) = attribute_path(*attribute, src);
        match last {
            "test" | "wasm_bindgen_test" => found.test = true,
            "rstest" => {
                found.test = true;
                found.rstest = true;
            }
            "test_case" => {
                found.test = true;
                found.test_cases += 1;
            }
            _ => {}
        }
        if path == "case" || path.starts_with("case::") {
            found.cases += 1;
        }
        match path.as_str() {
            "ignore" => found.ignored = true,
            "cfg" => {
                let arguments = attribute_arguments(*attribute, src);
                found.names_test |= arguments
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|word| word == "test");
                match arguments.as_str() {
                    "(any())" => found.never = true,
                    "(test)" => found.cfg_test = true,
                    _ => found.gated = true,
                }
            }
            "cfg_attr" => {
                // `#[cfg_attr(windows, ignore)]`, `ignore = "…"`: the test
                // runs only sometimes.
                let ignores = attribute
                    .child_by_field_name("arguments")
                    .map(split_arguments)
                    .is_some_and(|arguments| {
                        arguments.iter().skip(1).any(|argument| {
                            argument
                                .iter()
                                .find(|node| !node.is_extra())
                                .is_some_and(|first| {
                                    first.kind() == "identifier" && text(*first, src) == "ignore"
                                })
                        })
                    });
                found.ignored |= ignores;
            }
            _ => {}
        }
    }
    found
}

/// The marks of a module: its outer attributes and the inner ones of its
/// body.
fn module_marks(module: Node<'_>, src: &[u8]) -> Marks {
    let mut attributes = outer_attributes(module);
    if let Some(body) = module.child_by_field_name("body") {
        attributes.extend(inner_attributes(body));
    }
    marks(&attributes, src)
}

/// The name of an assertion macro (the last segment of its path), or
/// `None` for any other macro.
fn assertion_name<'s>(invocation: Node<'_>, src: &'s [u8]) -> Option<&'s str> {
    let path = invocation.child_by_field_name("macro")?;
    let name = if path.kind() == "scoped_identifier" {
        text(path.child_by_field_name("name")?, src)
    } else {
        text(path, src)
    };
    (name.starts_with("assert")
        || name.starts_with("debug_assert")
        || name.starts_with("prop_assert"))
    .then_some(name)
}

/// True for an assertion that cannot fail: it checks only literals, and
/// they certainly pass (`assert!(true)`, `assert!(1 == 1)`,
/// `assert_eq!(1, 1)`). One that certainly fails (`assert!(false, "…")`,
/// `assert_eq!(1, 2)`) is an assertion like any other: it marks code that
/// must not be reached. Only `assert` and `*_eq` forms are judged.
fn cannot_fail(invocation: Node<'_>, name: &str, src: &[u8]) -> bool {
    let checked = if matches!(name, "assert" | "debug_assert" | "prop_assert") {
        1
    } else if name.ends_with("_eq") {
        2
    } else {
        return false;
    };
    let mut cursor = invocation.walk();
    let Some(tree) = invocation
        .named_children(&mut cursor)
        .find(|child| child.kind() == "token_tree")
    else {
        return false;
    };
    let arguments = split_arguments(tree);
    if arguments.len() < checked {
        return false;
    }
    let mut texts: Vec<String> = Vec::new();
    for argument in &arguments[..checked] {
        let mut named = argument
            .iter()
            .filter(|node| node.is_named() && !node.is_extra())
            .peekable();
        if named.peek().is_none() || !named.all(|node| reach::constant(*node, &LITERALS)) {
            return false;
        }
        texts.push(
            argument
                .iter()
                .filter(|node| !node.is_extra())
                .map(|node| text(*node, src))
                .collect::<String>()
                .split_whitespace()
                .collect(),
        );
    }
    match texts.as_slice() {
        [condition] => {
            condition == "true"
                || condition
                    .split_once("==")
                    .is_some_and(|(left, right)| !right.contains('=') && left == right)
        }
        [left, right] => left == right,
        _ => false,
    }
}

/// Reads the tests, skips and assertions of a Rust file.
#[must_use]
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
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

    // The modules and functions around the node being visited.
    struct Scope {
        end: usize,
        /// A module's name; `None` for a function.
        module: Option<String>,
        never: bool,
        cfg_test: bool,
        /// Under a `#[cfg(…)]` other than `test` and `any()`.
        gated: bool,
        /// A function, not a module or the file.
        function: bool,
        /// For a function: the index of the test it is.
        test: Option<usize>,
    }
    let file_marks = marks(&inner_attributes(root), src);
    let mut open: Vec<Scope> = vec![Scope {
        end: usize::MAX,
        module: None,
        never: file_marks.never,
        cfg_test: file_marks.cfg_test,
        gated: file_marks.gated,
        function: false,
        test: None,
    }];
    // A test outside `#[cfg(test)]` code: the file is a test file (as in
    // `tests/`), so the items it defines are its helpers, not a subject.
    let mut loose_test = false;
    walk(root, |node| {
        while open.len() > 1
            && open
                .last()
                .is_some_and(|scope| node.start_byte() >= scope.end)
        {
            open.pop();
        }
        match node.kind() {
            "mod_item" => {
                let own = module_marks(node, src);
                let (never, cfg_test, gated) = open.last().map_or((false, false, false), |scope| {
                    (scope.never, scope.cfg_test, scope.gated)
                });
                let name = node
                    .child_by_field_name("name")
                    .map(|name| text(name, src).to_owned());
                // `#[cfg(test)] mod tests;`: the tests are in another file,
                // which this declaration compiles. It counts as one test,
                // so turning it off or deleting it is a finding even when
                // that file is not part of the change.
                if node.child_by_field_name("body").is_none() && (cfg_test || own.names_test) {
                    let id = open
                        .iter()
                        .filter_map(|scope| scope.module.as_deref())
                        .chain(name.as_deref())
                        .collect::<Vec<_>>()
                        .join("::");
                    let never = never || own.never;
                    file.tests.push(TestCase {
                        id: format!("mod {id}; (its tests are in another file)"),
                        line: node.start_position().row + 1,
                        skipped: never || gated || own.gated,
                        skipped_rows: 0,
                        why: never.then_some(NEVER_COMPILED),
                        focused: false,
                        uses: BTreeSet::new(),
                        weight: 1,
                    });
                }
                open.push(Scope {
                    end: node.end_byte(),
                    module: name,
                    never: never || own.never,
                    cfg_test: cfg_test || own.cfg_test,
                    gated: gated || own.gated,
                    function: false,
                    test: None,
                });
                true
            }
            "function_item" => {
                let own = marks(&outer_attributes(node), src);
                let (never, cfg_test, gated) = open.last().map_or((false, false, false), |scope| {
                    (scope.never, scope.cfg_test, scope.gated)
                });
                let mut test = None;
                if own.test {
                    let name = node
                        .child_by_field_name("name")
                        .map_or("", |name| text(name, src));
                    let id = open
                        .iter()
                        .filter_map(|scope| scope.module.as_deref())
                        .chain(std::iter::once(name))
                        .collect::<Vec<_>>()
                        .join("::");
                    let weight = if own.rstest {
                        own.cases.max(1)
                    } else {
                        own.test_cases.max(1)
                    };
                    let why = if own.ignored {
                        None
                    } else if never || own.never {
                        Some(NEVER_COMPILED)
                    } else if node
                        .child_by_field_name("body")
                        .is_some_and(|body| reach::does_nothing(body, src, &GRAMMAR))
                    {
                        Some("its body does nothing")
                    } else {
                        None
                    };
                    let mut uses = BTreeSet::new();
                    walk(node, |inner| {
                        if matches!(inner.kind(), "identifier" | "type_identifier") {
                            uses.insert(text(inner, src).to_owned());
                        }
                        true
                    });
                    loose_test |= !(cfg_test || own.cfg_test);
                    test = Some(file.tests.len());
                    file.tests.push(TestCase {
                        id,
                        line: node.start_position().row + 1,
                        skipped: own.ignored || gated || own.gated || why.is_some(),
                        skipped_rows: 0,
                        why,
                        focused: false,
                        uses,
                        weight,
                    });
                }
                open.push(Scope {
                    end: node.end_byte(),
                    module: None,
                    never: never || own.never,
                    cfg_test: cfg_test || own.cfg_test,
                    gated: gated || own.gated,
                    function: true,
                    test,
                });
                true
            }
            "macro_invocation" => {
                let Some(name) = assertion_name(node, src) else {
                    return false;
                };
                if dead.why(node).is_some() || cannot_fail(node, name, src) {
                    return false;
                }
                // The function it is in, if that is a test.
                let test = open
                    .iter()
                    .rev()
                    .find(|scope| scope.function)
                    .and_then(|scope| scope.test);
                let whole = text(node, src);
                file.assertions.push(Assertion {
                    line: node.start_position().row + 1,
                    shape: whole.split_whitespace().collect(),
                    text: whole.split_whitespace().collect::<Vec<_>>().join(" "),
                    weight: test
                        .and_then(|index| file.tests.get(index))
                        .map_or(1, |test| test.weight),
                    test,
                });
                // What is inside is a token tree: no more macro calls.
                false
            }
            "token_tree" | "line_comment" | "block_comment" | "string_literal"
            | "raw_string_literal" => false,
            _ => true,
        }
    });
    if !loose_test && !file_marks.cfg_test && !file_marks.never {
        file.imports = defined(root, src);
    }
    file
}

/// The names a file defines outside its test code (`#[cfg(test)]` modules,
/// test functions), each mapped to [`SELF`]: at the top level and in
/// modules that are not test code.
fn defined(root: Node<'_>, src: &[u8]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    walk(root, |node| match node.kind() {
        "source_file" | "declaration_list" => true,
        "mod_item" => {
            let own = module_marks(node, src);
            !(own.cfg_test || own.never)
        }
        kind if ITEMS.contains(&kind) => {
            let is_test = kind == "function_item" && marks(&outer_attributes(node), src).test;
            if !is_test {
                if let Some(name) = node.child_by_field_name("name") {
                    found.insert(text(name, src).to_owned(), SELF.to_owned());
                }
            }
            false
        }
        _ => false,
    });
    found
}

/// True for a path the guard reads as a test file by its folder alone: a
/// `.rs` file in a folder named `tests`. Rust files elsewhere are test files
/// when they hold tests ([`holds_tests`]).
#[must_use]
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let is_rust = name
        .rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && extension.eq_ignore_ascii_case("rs"));
    is_rust && folders.contains(&"tests")
}

/// A cheap check that Rust source may hold tests, before it is parsed.
#[must_use]
pub(super) fn holds_tests(source: &str) -> bool {
    [
        "#[test]",
        "::test]",
        "::test(",
        "#[cfg(test)]",
        "#[rstest",
        "#[test_case",
        "#[wasm_bindgen_test",
    ]
    .iter()
    .any(|needle| source.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(file: &TestFile) -> Vec<(&str, bool, Option<&str>, usize)> {
        file.tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.why, t.weight))
            .collect()
    }

    fn shapes(file: &TestFile) -> Vec<&str> {
        file.assertions.iter().map(|a| a.shape.as_str()).collect()
    }

    #[test]
    fn unit_tests_in_a_cfg_test_module_are_read() {
        let file = read(
            r#"pub fn total(items: &[u32]) -> u32 {
    items.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(total(&[1, 2]), 3);
        assert!(total(&[]) == 0, "empty is {}", 0);
    }
}
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(ids(&file), [("tests::adds", false, None, 1)]);
        assert_eq!(file.tests[0].line, 10);
        assert!(file.tests[0].uses.contains("total"));
        assert_eq!(
            shapes(&file),
            [
                "assert_eq!(total(&[1,2]),3)",
                "assert!(total(&[])==0,\"emptyis{}\",0)"
            ]
        );
        assert_eq!(file.assertions[0].line, 11);
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(
            file.assertions[1].text,
            "assert!(total(&[]) == 0, \"empty is {}\", 0)"
        );
    }

    #[test]
    fn every_test_attribute_form_is_a_test() {
        let file = read(
            r#"
#[test]
fn plain() { assert!(a); }
#[tokio::test]
async fn on_tokio() { assert!(a); }
#[tokio::test(flavor = "multi_thread")]
async fn on_threads() { assert!(a); }
#[async_std::test]
async fn on_async_std() { assert!(a); }
#[wasm_bindgen_test]
fn in_wasm() { assert!(a); }
#[test]
#[should_panic(expected = "boom")]
fn panics() { boom(); }
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("plain", false, None, 1),
                ("on_tokio", false, None, 1),
                ("on_threads", false, None, 1),
                ("on_async_std", false, None, 1),
                ("in_wasm", false, None, 1),
                ("panics", false, None, 1),
            ]
        );
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r##"
// #[test]
// fn commented() { assert!(x); }
/// ```
/// assert_eq!(doc(), 1);
/// ```
const S: &str = "#[test] fn in_a_string() { assert!(x); }";
#[cfg(test)]
fn helper() {}
#[derive(Debug)]
struct Testing;
fn not_a_test() { let r = r"assert!(y)"; /* assert!(z) */ }
"##,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn assertions_outside_tests_count_once() {
        let file = read("fn check(x: u32) {\n    debug_assert!(x > 0);\n    prop_assert_eq!(x, 2);\n    insta::assert_snapshot!(x);\n    println!(\"{x}\");\n}\n");
        assert_eq!(
            shapes(&file),
            [
                "debug_assert!(x>0)",
                "prop_assert_eq!(x,2)",
                "insta::assert_snapshot!(x)"
            ]
        );
        assert!(file
            .assertions
            .iter()
            .all(|a| a.test.is_none() && a.weight == 1));
    }

    #[test]
    fn ignore_and_conditional_ignore_skip_a_test() {
        let file = read(
            r#"
#[test]
#[ignore]
fn plain_ignore() { assert!(a); }
#[test]
#[ignore = "slow"]
fn ignore_with_reason() { assert!(a); }
#[test]
#[cfg_attr(windows, ignore)]
fn not_on_windows() { assert!(a); }
#[test]
#[cfg_attr(not(feature = "net"), ignore = "needs the network")]
fn needs_net() { assert!(a); }
#[test]
#[cfg_attr(windows, allow(unused))]
fn runs() { assert!(a); }
#[test]
#[cfg_attr(ignore, allow(unused))]
fn also_runs() { assert!(a); }
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("plain_ignore", true, None, 1),
                ("ignore_with_reason", true, None, 1),
                ("not_on_windows", true, None, 1),
                ("needs_net", true, None, 1),
                ("runs", false, None, 1),
                ("also_runs", false, None, 1),
            ]
        );
    }

    #[test]
    fn cfg_any_on_the_test_or_its_module_skips_it() {
        let file = read(
            r#"
#[cfg(any())]
mod off {
    #[test]
    fn inside() { assert!(a); }
}
mod on {
    #[test]
    #[cfg(any())]
    fn alone() { assert!(a); }
    #[test]
    #[cfg(any(unix))]
    fn on_unix() { assert!(a); }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("off::inside", true, Some(NEVER_COMPILED), 1),
                ("on::alone", true, Some(NEVER_COMPILED), 1),
                // Any other `cfg` compiles the test only sometimes: a
                // conditional skip, like `cfg_attr(…, ignore)`.
                ("on::on_unix", true, None, 1),
            ]
        );
    }

    #[test]
    fn a_cfg_other_than_test_is_a_conditional_skip() {
        let file = read(
            r#"
#[cfg(test)]
mod plain {
    #[test]
    fn runs() { assert!(a); }
}
#[cfg(all(test, feature = "slow-tests"))]
mod slow {
    #[test]
    fn only_with_the_feature() { assert!(a); }
}
#[cfg(not(test))]
mod never_in_tests {
    #[test]
    fn hidden() { assert!(a); }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("plain::runs", false, None, 1),
                ("slow::only_with_the_feature", true, None, 1),
                ("never_in_tests::hidden", true, None, 1),
            ]
        );
        let gated = read("#![cfg(feature = \"integration\")]\n#[test]\nfn t() { assert!(a); }\n");
        assert_eq!(ids(&gated), [("t", true, None, 1)]);
    }

    #[test]
    fn a_test_module_in_another_file_counts_as_one_test() {
        let on = read("#[cfg(test)]\nmod tests;\nmod helpers;\n");
        assert_eq!(
            ids(&on),
            [("mod tests; (its tests are in another file)", false, None, 1)]
        );
        let off = read("#[cfg(any())]\nmod tests;\n");
        assert!(off.tests.is_empty());
        let gated = read("#[cfg(all(test, feature = \"slow\"))]\nmod tests;\n");
        assert_eq!(
            ids(&gated),
            [("mod tests; (its tests are in another file)", true, None, 1)]
        );
    }

    #[test]
    fn a_body_that_does_nothing_is_a_skip() {
        let file = read(
            "#[test]\nfn empty() {}\n#[test]\nfn semicolons() { ; // nothing\n }\n#[test]\nfn returns() { return; }\n#[test]\nfn calls() { run(); }\n",
        );
        let why = Some("its body does nothing");
        assert_eq!(
            ids(&file),
            [
                ("empty", true, why, 1),
                ("semicolons", true, why, 1),
                ("returns", true, why, 1),
                ("calls", false, None, 1),
            ]
        );
    }

    #[test]
    fn rstest_and_test_case_weigh_their_cases() {
        let file = read(
            r#"
#[rstest]
#[case(1, 2)]
#[case::named(2, 4)]
#[case(3, 6)]
fn doubles(#[case] a: u32, #[case] b: u32) { assert_eq!(a * 2, b); }
#[rstest]
fn fixture(value: u32) { assert!(value > 0); }
#[test_case(1 ; "one")]
#[test_case(2 ; "two")]
fn positive(n: u32) { assert!(n > 0); }
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("doubles", false, None, 3),
                ("fixture", false, None, 1),
                ("positive", false, None, 2),
            ]
        );
        let weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(weights, [3, 1, 2]);
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"
#[test]
fn t() {
    assert!(true);
    assert!(true, "msg {}", x);
    assert_eq!(1, 1);
    assert_ne!("a", 'b');
    assert!(1 == 1);
    assert!(-1.5 < 2.0);
    debug_assert!(r"x" != "y\n");
    assert_eq!(x, 1);
    assert!(ok);
    assert!(true && ok);
    assert_eq!(1, 1 + y);
    assert_matches!(1, 1);
    insta::assert_snapshot!("fixed");
    assert!(false, "must not be reached");
    assert_eq!(1, 2);
}
"#,
        );
        // Only literals that certainly pass are left out. Comparisons other
        // than `==` are not worked out, and one that certainly fails marks
        // code that must not be reached: both count.
        assert_eq!(
            shapes(&file),
            [
                "assert_ne!(\"a\",'b')",
                "assert!(-1.5<2.0)",
                "debug_assert!(r\"x\"!=\"y\\n\")",
                "assert_eq!(x,1)",
                "assert!(ok)",
                "assert!(true&&ok)",
                "assert_eq!(1,1+y)",
                "assert_matches!(1,1)",
                "insta::assert_snapshot!(\"fixed\")",
                "assert!(false,\"mustnotbereached\")",
                "assert_eq!(1,2)",
            ]
        );
    }

    #[test]
    fn assertions_that_never_run_are_not_counted() {
        let file = read(
            r#"
#[test]
fn after_return() {
    let total = 3;
    return;
    assert_eq!(total, 3);
}
#[test]
fn inside_if_false() {
    if false {
        assert_eq!(total(), 3);
    }
    while false { assert!(x); }
    if (false) { assert!(y); }
}
#[test]
fn reachable() {
    if true { assert!(a); }
    let f = || { return; };
    assert!(b);
}
"#,
        );
        assert_eq!(shapes(&file), ["assert!(a)", "assert!(b)"]);
        assert_eq!(file.assertions[0].test, Some(2));
        assert!(!file.tests[0].skipped);
    }

    #[test]
    fn ids_name_nested_modules() {
        let file = read(
            r#"
mod outer {
    pub mod inner {
        #[test]
        fn deep() { assert!(a); }
    }
    #[test]
    fn shallow() { assert!(a); }
}
#[test]
fn top() { assert!(a); }
"#,
        );
        let names: Vec<&str> = file.tests.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(names, ["outer::inner::deep", "outer::shallow", "top"]);
    }

    #[test]
    fn a_test_attribute_after_a_comment_still_counts() {
        let file =
            read("#[test]\n// why it is here\n/* more */\n#[ignore]\nfn t() { assert!(a); }\n");
        assert_eq!(ids(&file), [("t", true, None, 1)]);
        assert_eq!(file.tests[0].line, 5);
    }

    #[test]
    fn imports_are_the_items_a_source_file_defines() {
        let file = read(
            r#"
pub fn total(items: &[u32]) -> u32 { items.iter().sum() }
pub struct Cart;
enum Kind { A }
const LIMIT: u32 = 3;
static NAME: &str = "x";
trait Priced {}
type Price = u32;
macro_rules! price { () => {} }
mod nested { pub fn inner() {} }
#[cfg(any())]
mod gone { fn never() {} }
impl Cart { fn method() {} }

#[cfg(test)]
mod tests {
    use super::*;
    fn helper() {}
    #[test]
    fn adds() { assert_eq!(total(&[1]), 1); }
}
"#,
        );
        let names: Vec<&str> = file.imports.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["Cart", "Kind", "LIMIT", "NAME", "Price", "Priced", "inner", "price", "total"]
        );
        assert!(file.imports.values().all(|spec| spec == SELF));
    }

    #[test]
    fn a_file_of_loose_tests_defines_no_subject() {
        // As in `tests/`: its helpers are test code.
        let file = read("fn setup() -> u32 { 1 }\n#[test]\nfn t() { assert_eq!(setup(), 1); }\n");
        assert!(file.imports.is_empty());
        let file =
            read("#![cfg(test)]\nfn setup() {}\nmod tests {\n#[test]\nfn t() { setup(); }\n}\n");
        assert!(file.imports.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read("#[test]\nfn t() {\n    assert_eq!(1, x);\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn macros_inside_token_trees_are_not_read() {
        let file = read(
            "proptest! {\n    #[test]\n    fn p(x in 0..10u32) { prop_assert!(x < 10); }\n}\n",
        );
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        // Code that never runs is found in one pass from the root
        // (`reach::Dead`), so depth costs nothing more than width.
        let source = format!(
            "#[test]\nfn t() {{ {} assert!(x); {} }}\n",
            "{ ".repeat(1_000),
            "} ".repeat(1_000)
        );
        let file = read(&source);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
        let source = format!(
            "fn t() {{ assert!({}x{}); }}\n",
            "(".repeat(20_000),
            ")".repeat(20_000)
        );
        assert_eq!(read(&source).assertions.len(), 1);
    }

    #[test]
    fn test_paths() {
        for path in [
            "tests/pricing.rs",
            "crates/core/tests/a.rs",
            "src/tests/mod.rs",
            "tests/A.RS",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "src/pricing.rs",
            "testing/a.rs",
            "tests/a.py",
            "tests.rs",
            "tests/.rs",
            "tests",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_looks_for_test_attributes() {
        for source in [
            "#[test]\nfn a() {}",
            "#[tokio::test]",
            "#[tokio::test(flavor = \"multi_thread\")]",
            "#[cfg(test)]\nmod tests {}",
            "#[rstest]",
            "#[test_case(1)]",
            "#[wasm_bindgen_test]",
        ] {
            assert!(holds_tests(source), "{source}");
        }
        assert!(!holds_tests("fn main() { println!(\"tested\"); }"));
    }
}
