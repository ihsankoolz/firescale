//! The Swift reader of the tamper guard (M32, ADR 0052): XCTest and Swift
//! Testing.
//!
//! - A test is an XCTest method, `func test…()` with no parameters, not
//!   `static` or `class`, in a class that inherits `XCTestCase` (directly,
//!   or from a class of the file that does, or from any class whose name
//!   ends in `TestCase`) or in an extension of one; or a Swift Testing
//!   function with `@Test`, anywhere. Its id is the names of the types
//!   around it and its own, joined with `::` (`CartTests::testTotal`): a
//!   `@Test` display name is not part of it.
//! - It is skipped when its first statement is `throw XCTSkip(…)`, `try
//!   XCTSkipIf(…)` or `try XCTSkipUnless(…)` (also in its class's `setUp`
//!   or `setUpWithError`), with a `.disabled(…)` trait or an `.enabled(if:
//!   …)` trait that is not literally `true` (on the test or a `@Suite`
//!   around it), or when its body does nothing. A test method renamed so it
//!   no longer starts with `test` is not a test: a deletion.
//! - `@Test(arguments: [a, b, c])` weighs the rows of its array literal
//!   (several collections multiply, `zip` pairs them).
//! - An assertion is a call of `XCTAssert…`, `XCTFail` or `XCTUnwrap`, or a
//!   `#expect(…)` or `#require(…)` macro, unless it never runs (after a
//!   `return` or a `throw`, inside `if false`, inside `withKnownIssue { }` or
//!   `XCTExpectFailure { }`, where a failure is expected) or cannot fail
//!   (`XCTAssertTrue(true)`, `XCTAssertEqual(1, 1)`, `#expect(true)`).
//! - Suppressions: `// swiftlint:disable rules` to the end of the file
//!   (`swiftlint-range:rule`), `:next`, `:this` and `:previous` for one line
//!   (`swiftlint:rule`), `all` for every rule; `// swift-format-ignore[:
//!   rules]` and `// swift-format-ignore-file`.
//! - Subjects: a type the tests use, from the module of an `@testable
//!   import` (or else the first import that is not Apple's or Swift's), is
//!   looked up as `Sources/<Module>/<Type>.swift` or `<Module>/<Type>.swift`.

use std::collections::BTreeSet;

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::recover;
use super::suppress::{keyed, shown};
use super::{Assertion, Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// What the Swift grammar calls the nodes [`reach`] looks at. Its `if` and
/// `while` have no `consequence` or `body` field, so code under a literal
/// `false` is found by [`false_branches`] instead.
const GRAMMAR: Grammar = Grammar {
    blocks: &["statements"],
    returns: &[],
    ifs: &[],
    whiles: &[],
    hoisted: &["function_declaration", "class_declaration"],
    stops: Some(ends_block),
    noop: |_, _| false,
};

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 14] = [
    "integer_literal",
    "real_literal",
    "hex_literal",
    "bin_literal",
    "oct_literal",
    "boolean_literal",
    "nil_literal",
    "line_string_literal",
    "line_str_text",
    "multi_line_string_literal",
    "multi_line_str_text",
    "str_escaped_char",
    "prefix_expression",
    "tuple_expression",
];

/// Modules of Apple's SDKs and Swift's own: never a subject.
const SYSTEM_MODULES: [&str; 22] = [
    "XCTest",
    "Testing",
    "Foundation",
    "UIKit",
    "AppKit",
    "SwiftUI",
    "Combine",
    "Dispatch",
    "Darwin",
    "Glibc",
    "os",
    "CoreData",
    "CoreGraphics",
    "CoreFoundation",
    "CoreLocation",
    "Observation",
    "Swift",
    "_Concurrency",
    "WebKit",
    "MapKit",
    "XCTestDynamicOverlay",
    "SnapshotTesting",
];

/// True for a statement after which the rest of its block never runs: a
/// `throw`, `break`, `continue` or `return`. A bare `return` followed by an
/// expression is not one: Swift returns that expression, so it runs.
fn ends_block(statement: Node<'_>, _src: &[u8]) -> bool {
    if statement.kind() != "control_transfer_statement" {
        return false;
    }
    let bare_return = statement
        .child(0)
        .is_some_and(|keyword| keyword.kind() == "return")
        && statement.named_child_count() == 0;
    if !bare_return {
        return true;
    }
    let mut next = statement.next_named_sibling();
    while let Some(sibling) = next.filter(|n| is_comment(*n)) {
        next = sibling.next_named_sibling();
    }
    next.is_none_or(|next| {
        matches!(
            next.kind(),
            "property_declaration"
                | "if_statement"
                | "guard_statement"
                | "for_statement"
                | "while_statement"
                | "repeat_while_statement"
                | "do_statement"
                | "switch_statement"
                | "control_transfer_statement"
                | "function_declaration"
                | "class_declaration"
                | "typealias_declaration"
        )
    })
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "comment" | "multiline_comment")
}

/// True for a literal `false`, in parentheses or not.
fn is_false(node: Node<'_>, src: &[u8]) -> bool {
    match node.kind() {
        "boolean_literal" => text(node, src) == "false",
        "tuple_expression" if node.named_child_count() == 1 => node
            .named_child(0)
            .is_some_and(|inner| is_false(inner, src)),
        _ => false,
    }
}

/// The byte ranges that never run, or whose failures are expected: the
/// branch of `if false` and the body of `while false`, and the closure of
/// `withKnownIssue { }` and `XCTExpectFailure { }`.
fn false_branches(root: Node<'_>, src: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    walk(root, |node| {
        match node.kind() {
            "if_statement" | "while_statement" => {
                let mut cursor = node.walk();
                let conditions: Vec<Node<'_>> = node
                    .children_by_field_name("condition", &mut cursor)
                    .collect();
                if conditions.iter().any(|condition| is_false(*condition, src)) {
                    // The first block, before any `else`.
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if child.kind() == "else" {
                            break;
                        }
                        if child.kind() == "statements" {
                            ranges.push((child.start_byte(), child.end_byte()));
                            break;
                        }
                    }
                }
            }
            "call_expression" => {
                let expected_failure = node.named_child(0).is_some_and(|callee| {
                    callee.kind() == "simple_identifier"
                        && matches!(text(callee, src), "withKnownIssue" | "XCTExpectFailure")
                });
                if expected_failure {
                    walk(node, |inner| {
                        if inner.kind() == "lambda_literal" {
                            ranges.push((inner.start_byte(), inner.end_byte()));
                            return false;
                        }
                        true
                    });
                }
            }
            _ => {}
        }
        true
    });
    ranges
}

/// The name of an assertion: an `XCTAssert…`, `XCTFail` or `XCTUnwrap`
/// call, or an `#expect` or `#require` macro.
fn assertion_name<'s>(node: Node<'_>, src: &'s [u8]) -> Option<&'s str> {
    let callee = node
        .named_child(0)
        .filter(|callee| callee.kind() == "simple_identifier")?;
    let name = text(callee, src);
    let is = match node.kind() {
        "call_expression" => {
            name.starts_with("XCTAssert") || matches!(name, "XCTFail" | "XCTUnwrap")
        }
        "macro_invocation" => matches!(name, "expect" | "require"),
        _ => false,
    };
    is.then_some(name)
}

/// The unlabelled arguments of a call or macro, in order, up to the first
/// labelled one.
fn positional(node: Node<'_>) -> Vec<Node<'_>> {
    let mut found = Vec::new();
    let mut cursor = node.walk();
    let Some(suffix) = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "call_suffix")
    else {
        return found;
    };
    let mut cursor = suffix.walk();
    let Some(arguments) = suffix
        .named_children(&mut cursor)
        .find(|child| child.kind() == "value_arguments")
    else {
        return found;
    };
    let mut cursor = arguments.walk();
    for argument in arguments
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "value_argument")
    {
        if argument.child_by_field_name("name").is_some() {
            break;
        }
        if let Some(value) = argument.child_by_field_name("value") {
            found.push(value);
        }
    }
    found
}

/// The source text of a node without white space.
fn squeezed(node: Node<'_>, src: &[u8]) -> String {
    text(node, src).split_whitespace().collect()
}

/// True for an assertion that cannot fail: it checks only literals, and
/// they certainly pass (`XCTAssertTrue(true)`, `XCTAssert(1 == 1)`,
/// `XCTAssertFalse(false)`, `XCTAssertNil(nil)`, `XCTAssertEqual(1, 1)`,
/// `#expect(true)`). One that certainly fails (`XCTFail(…)`,
/// `#expect(false)`) marks code that must not be reached, and counts.
fn cannot_fail(node: Node<'_>, name: &str, src: &[u8]) -> bool {
    let arguments = positional(node);
    let constant = |value: &Node<'_>| reach::constant(*value, &LITERALS);
    let always_true = |value: &Node<'_>| {
        if value.kind() == "boolean_literal" {
            return text(*value, src) == "true";
        }
        if value.kind() == "equality_expression" {
            let (Some(left), Some(right)) = (
                value.child_by_field_name("lhs"),
                value.child_by_field_name("rhs"),
            ) else {
                return false;
            };
            let equal = value
                .child_by_field_name("op")
                .is_some_and(|op| text(op, src) == "==");
            return equal
                && constant(&left)
                && constant(&right)
                && squeezed(left, src) == squeezed(right, src);
        }
        false
    };
    match (name, arguments.as_slice()) {
        ("XCTAssert" | "XCTAssertTrue" | "expect" | "require", [value, ..]) => always_true(value),
        ("XCTAssertFalse", [value, ..]) => {
            value.kind() == "boolean_literal" && text(*value, src) == "false"
        }
        ("XCTAssertNil", [value, ..]) => value.kind() == "nil_literal",
        ("XCTAssertEqual", [left, right, ..]) => {
            constant(left) && constant(right) && squeezed(*left, src) == squeezed(*right, src)
        }
        _ => false,
    }
}

/// The attributes of a declaration: `(name, attribute)` for each, the name
/// being the last segment of its type (`Test` for `@Testing.Test`).
fn attributes<'t>(declaration: Node<'t>, src: &[u8]) -> Vec<(String, Node<'t>)> {
    let mut found = Vec::new();
    let mut cursor = declaration.walk();
    for modifiers in declaration
        .children(&mut cursor)
        .filter(|child| child.kind() == "modifiers")
    {
        let mut inner = modifiers.walk();
        for attribute in modifiers
            .named_children(&mut inner)
            .filter(|child| child.kind() == "attribute")
        {
            let mut types = attribute.walk();
            let name = attribute
                .named_children(&mut types)
                .find(|child| child.kind() == "user_type")
                .map(|user| {
                    text(user, src)
                        .rsplit('.')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_owned()
                })
                .unwrap_or_default();
            found.push((name, attribute));
        }
    }
    found
}

/// The name a trait expression calls (`disabled` for `.disabled("x")` or
/// `ConditionTrait.disabled`), and its call, if it is one.
fn trait_name<'t>(node: Node<'t>, src: &[u8]) -> Option<(String, Option<Node<'t>>)> {
    match node.kind() {
        "prefix_expression" => node
            .child_by_field_name("target")
            .map(|target| (text(target, src).to_owned(), None)),
        "navigation_expression" => {
            let suffix = node.child_by_field_name("suffix")?;
            let name = suffix.child_by_field_name("suffix")?;
            Some((text(name, src).to_owned(), None))
        }
        "call_expression" => {
            let (name, _) = trait_name(node.named_child(0)?, src)?;
            Some((name, Some(node)))
        }
        _ => None,
    }
}

/// The value of the `if:` argument of a trait's call, if it has one.
fn if_argument<'t>(call: Node<'t>, src: &[u8]) -> Option<Node<'t>> {
    let mut found = None;
    walk(call, |node| {
        if found.is_some() {
            return false;
        }
        if node.kind() == "value_argument" {
            let label = node
                .child_by_field_name("name")
                .map(|label| text(label, src).trim());
            if label == Some("if") {
                found = node.child_by_field_name("value");
            }
            return false;
        }
        true
    });
    found
}

/// What the attributes of a declaration say about it.
#[derive(Default)]
struct Marks {
    /// `@Test`.
    test: bool,
    /// `@Suite`.
    suite: bool,
    /// A `.disabled` trait, or an `.enabled(if:)` that is not literally
    /// `true`: it does not run, or runs only sometimes.
    disabled: bool,
    /// How many runs `arguments:` stands for.
    weight: usize,
}

fn marks(declaration: Node<'_>, src: &[u8]) -> Marks {
    let mut found = Marks {
        weight: 1,
        ..Marks::default()
    };
    for (name, attribute) in attributes(declaration, src) {
        match name.as_str() {
            "Test" => found.test = true,
            "Suite" => found.suite = true,
            _ => continue,
        }
        let mut cursor = attribute.walk();
        let parts: Vec<Node<'_>> = attribute
            .children(&mut cursor)
            .filter(|child| !child.is_extra() && child.kind() != "user_type")
            .collect();
        let mut index = 0;
        while index < parts.len() {
            let part = parts[index];
            // `arguments: [1, 2], ["a"]`: the collections run to the end.
            if part.kind() == "simple_identifier"
                && text(part, src) == "arguments"
                && parts.get(index + 1).is_some_and(|next| next.kind() == ":")
            {
                let collections: Vec<Node<'_>> = parts[index + 2..]
                    .iter()
                    .copied()
                    .filter(|node| node.is_named())
                    .collect();
                found.weight = rows(&collections, src);
                break;
            }
            if let Some((trait_, call)) = trait_name(part, src) {
                let condition = call.and_then(|call| if_argument(call, src));
                let literal = |value: bool| {
                    condition.is_some_and(|condition| {
                        condition.kind() == "boolean_literal"
                            && text(condition, src) == if value { "true" } else { "false" }
                    })
                };
                match trait_.as_str() {
                    // `.disabled(if: false)` runs; any other form does not,
                    // or only sometimes.
                    "disabled" => found.disabled |= !literal(false),
                    "enabled" => found.disabled |= !literal(true),
                    _ => {}
                }
            }
            index += 1;
        }
    }
    found
}

/// The runs of `@Test(arguments: …)`: the product of the lengths of its
/// array literals, the shortest for `zip(…)` of array literals; 1 for
/// anything else.
fn rows(collections: &[Node<'_>], src: &[u8]) -> usize {
    let length = |node: Node<'_>| -> Option<usize> {
        (node.kind() == "array_literal").then(|| {
            let mut cursor = node.walk();
            node.children_by_field_name("element", &mut cursor).count()
        })
    };
    let mut product: usize = 1;
    for collection in collections {
        if let Some(count) = length(*collection) {
            product = product.saturating_mul(count.max(1));
            continue;
        }
        let zipped = (collection.kind() == "call_expression")
            .then(|| collection.named_child(0))
            .flatten()
            .is_some_and(|callee| text(callee, src) == "zip");
        if !zipped {
            return 1;
        }
        let lengths: Option<Vec<usize>> = positional(*collection).into_iter().map(length).collect();
        match lengths {
            Some(lengths) if !lengths.is_empty() => {
                product = product.saturating_mul(lengths.into_iter().min().unwrap_or(1).max(1));
            }
            _ => return 1,
        }
    }
    product
}

/// The statements of a function's body, comments left out.
fn body_statements(function: Node<'_>) -> Vec<Node<'_>> {
    let Some(body) = function.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    let Some(statements) = body
        .named_children(&mut cursor)
        .find(|child| child.kind() == "statements")
    else {
        return Vec::new();
    };
    let mut cursor = statements.walk();
    statements
        .named_children(&mut cursor)
        .filter(|child| !child.is_extra() && !is_comment(*child))
        .collect()
}

/// True when a function's first statement skips it: `throw XCTSkip(…)`,
/// `try XCTSkipIf(…)`, `try XCTSkipUnless(…)`. `XCTSkipIf` and
/// `XCTSkipUnless` with a condition other than a literal that lets the test
/// run (`XCTSkipIf(false)`, `XCTSkipUnless(true)`) skip it at least
/// sometimes.
fn skips_first(function: Node<'_>, src: &[u8]) -> bool {
    let Some(first) = body_statements(function).into_iter().next() else {
        return false;
    };
    let call = match first.kind() {
        "control_transfer_statement" => first
            .named_child(0)
            .filter(|keyword| keyword.kind() == "throw_keyword")
            .and_then(|_| first.named_child(1)),
        "try_expression" => first.child_by_field_name("expr"),
        _ => None,
    };
    let Some(call) = call.filter(|call| call.kind() == "call_expression") else {
        return false;
    };
    let Some(callee) = call.named_child(0) else {
        return false;
    };
    let condition = positional(call).into_iter().next();
    let literal = |value: &str| {
        condition.is_some_and(|c| c.kind() == "boolean_literal" && text(c, src) == value)
    };
    match text(callee, src) {
        "XCTSkip" => first.kind() == "control_transfer_statement",
        "XCTSkipIf" => !literal("false"),
        "XCTSkipUnless" => !literal("true"),
        _ => false,
    }
}

/// True when a function's body runs nothing: it is empty, or holds only a
/// bare `return`.
fn does_nothing(function: Node<'_>, src: &[u8]) -> bool {
    match body_statements(function).first() {
        None => true,
        Some(first) => {
            first.kind() == "control_transfer_statement"
                && text(*first, src).trim() == "return"
                && ends_block(*first, src)
        }
    }
}

/// The name of a type declaration: `CartTests` for a class, a struct or an
/// extension of it.
fn type_name<'s>(declaration: Node<'_>, src: &'s [u8]) -> &'s str {
    declaration
        .child_by_field_name("name")
        .map_or("", |name| text(name, src).trim())
}

/// The names of the classes of a file that are XCTest cases: they inherit
/// `XCTestCase`, a class whose name ends in `TestCase`, or another class of
/// the file that is one.
fn test_classes(root: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut parents: Vec<(String, Vec<String>)> = Vec::new();
    walk(root, |node| {
        if node.kind() == "class_declaration" {
            let mut cursor = node.walk();
            let supers: Vec<String> = node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "inheritance_specifier")
                .map(|specifier| {
                    text(specifier, src)
                        .rsplit('.')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_owned()
                })
                .collect();
            parents.push((type_name(node, src).to_owned(), supers));
        }
        !matches!(node.kind(), "function_body" | "statements")
    });
    let mut found: BTreeSet<String> = BTreeSet::new();
    loop {
        let before = found.len();
        for (name, supers) in &parents {
            if supers
                .iter()
                .any(|parent| parent.ends_with("TestCase") || found.contains(parent))
            {
                found.insert(name.clone());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// The classes of a file whose `setUp` or `setUpWithError` starts with a
/// skip: none of their tests run.
fn skipped_classes(root: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    walk(root, |node| {
        if node.kind() != "class_declaration" {
            return !matches!(node.kind(), "function_body" | "statements");
        }
        let Some(body) = node.child_by_field_name("body") else {
            return false;
        };
        let mut cursor = body.walk();
        let skips = body
            .named_children(&mut cursor)
            .filter(|member| member.kind() == "function_declaration")
            .any(|member| {
                let name = member
                    .child_by_field_name("name")
                    .map_or("", |name| text(name, src));
                matches!(name, "setUp" | "setUpWithError") && skips_first(member, src)
            });
        if skips {
            found.insert(type_name(node, src).to_owned());
        }
        true
    });
    found
}

/// True for a method XCTest runs: `func test…()` with no parameters, not
/// `static` or `class`.
fn is_test_method(function: Node<'_>, src: &[u8]) -> bool {
    let named_test = function
        .child_by_field_name("name")
        .is_some_and(|name| text(name, src).starts_with("test"));
    let mut cursor = function.walk();
    let mut type_level = false;
    let mut parameters = false;
    for child in function.children(&mut cursor) {
        match child.kind() {
            "class" => type_level = true,
            "parameter" => parameters = true,
            "modifiers" => {
                type_level |= text(child, src)
                    .split_whitespace()
                    .any(|word| matches!(word, "static" | "class"));
            }
            _ => {}
        }
    }
    named_test && !type_level && !parameters
}

/// The modules a test file imports that may hold its subject: those it
/// imports `@testable`, else those that are not Apple's or Swift's.
fn subject_modules(root: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut testable = Vec::new();
    let mut other = Vec::new();
    let mut cursor = root.walk();
    for import in root
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "import_declaration")
    {
        let mut inner = import.walk();
        let module = import
            .named_children(&mut inner)
            .find(|child| child.kind() == "identifier")
            .and_then(|identifier| identifier.named_child(0))
            .map(|first| text(first, src).to_owned());
        let Some(module) = module else {
            continue;
        };
        let mut inner = import.walk();
        let is_testable = import
            .named_children(&mut inner)
            .any(|child| child.kind() == "modifiers" && text(child, src).contains("@testable"));
        if is_testable {
            testable.push(module);
        } else if !SYSTEM_MODULES.contains(&module.as_str()) {
            other.push(module);
        }
    }
    if testable.is_empty() {
        other
    } else {
        testable
    }
}

/// Reads the tests, skips and assertions of a Swift test file.
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_swift::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    let dead = reach::Dead::of(root, src, &GRAMMAR);
    let never = false_branches(root, src);
    let in_never = |node: Node<'_>| {
        never
            .iter()
            .any(|(start, end)| node.start_byte() >= *start && node.end_byte() <= *end)
    };
    let error_line = recover::first_harmful(root, src);
    let mut file = TestFile {
        has_errors: error_line.is_some(),
        error_line,
        ..TestFile::default()
    };
    let classes = test_classes(root, src);
    let skipped_classes = skipped_classes(root, src);

    // The types and functions around the node being visited.
    struct Scope {
        end: usize,
        /// A type's name; `None` for a function.
        name: Option<String>,
        /// An XCTest case, or an extension of one.
        xctest: bool,
        /// Its tests do not run: a disabled `@Suite`, or a class whose
        /// `setUp` skips.
        skipped: bool,
        /// For a function: the index of the test it is.
        test: Option<usize>,
    }
    let mut open: Vec<Scope> = Vec::new();
    let mut used_types: BTreeSet<String> = BTreeSet::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    walk(root, |node| {
        while open
            .last()
            .is_some_and(|scope| node.start_byte() >= scope.end)
        {
            open.pop();
        }
        match node.kind() {
            "class_declaration" | "protocol_declaration" => {
                let name = type_name(node, src).to_owned();
                defined.insert(name.clone());
                let own = marks(node, src);
                let inherited = open.last().is_some_and(|scope| scope.skipped);
                open.push(Scope {
                    end: node.end_byte(),
                    xctest: classes.contains(&name),
                    skipped: inherited || own.disabled || skipped_classes.contains(&name),
                    name: Some(name),
                    test: None,
                });
                true
            }
            "function_declaration" => {
                let in_function = open.last().is_some_and(|scope| scope.name.is_none());
                let own = marks(node, src);
                let parent_type = open.last().filter(|scope| scope.name.is_some());
                let is_test = !in_function
                    && (own.test
                        || parent_type.is_some_and(|scope| scope.xctest)
                            && is_test_method(node, src));
                let mut test = None;
                if is_test {
                    let name = node
                        .child_by_field_name("name")
                        .map_or("", |name| text(name, src));
                    let id = open
                        .iter()
                        .filter_map(|scope| scope.name.as_deref())
                        .chain(std::iter::once(name))
                        .collect::<Vec<_>>()
                        .join("::");
                    let marked = own.disabled
                        || open.iter().any(|scope| scope.skipped)
                        || skips_first(node, src);
                    let why = if marked {
                        None
                    } else if does_nothing(node, src) {
                        Some("its body does nothing")
                    } else {
                        None
                    };
                    let mut uses = BTreeSet::new();
                    walk(node, |inner| {
                        if matches!(inner.kind(), "simple_identifier" | "type_identifier") {
                            uses.insert(text(inner, src).to_owned());
                        }
                        true
                    });
                    used_types.extend(
                        uses.iter()
                            .filter(|name| name.starts_with(|c: char| c.is_ascii_uppercase()))
                            .cloned(),
                    );
                    test = Some(file.tests.len());
                    file.tests.push(TestCase {
                        id,
                        line: node.start_position().row + 1,
                        skipped: marked || why.is_some(),
                        skipped_rows: 0,
                        why,
                        marker: false,
                        focused: false,
                        uses,
                        weight: own.weight,
                    });
                }
                open.push(Scope {
                    end: node.end_byte(),
                    name: None,
                    xctest: false,
                    skipped: false,
                    test,
                });
                true
            }
            "call_expression" | "macro_invocation" => {
                let Some(name) = assertion_name(node, src) else {
                    return true;
                };
                if dead.why(node).is_some() || in_never(node) || cannot_fail(node, name, src) {
                    return true;
                }
                let test = open
                    .iter()
                    .rev()
                    .find(|scope| scope.name.is_none())
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
                true
            }
            "typealias_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    defined.insert(text(name, src).trim().to_owned());
                }
                true
            }
            "comment"
            | "multiline_comment"
            | "line_string_literal"
            | "multi_line_string_literal"
            | "raw_string_literal" => false,
            _ => true,
        }
    });
    // A type the tests use, from the module they test: `<Module>/<Type>`,
    // for `resolve`.
    if let Some(module) = subject_modules(root, src).into_iter().next() {
        for name in used_types
            .into_iter()
            .filter(|name| !defined.contains(name) && !name.starts_with("XCT"))
        {
            let spec = format!("{module}/{name}");
            file.imports.insert(name, spec);
        }
    }
    file
}

/// The repo file a subject spec (`<Module>/<Type>`) names, among `exists`:
/// `Sources/<Module>/<Type>.swift` or `<Module>/<Type>.swift` in the test
/// file's folder or a folder above it (SwiftPM and Xcode layouts).
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let (module, name) = spec.split_once('/')?;
    if module.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    let mut folders: Vec<&str> = from.split('/').collect();
    folders.pop();
    loop {
        let base = folders.join("/");
        for layout in [format!("Sources/{module}"), module.to_owned()] {
            let candidate = if base.is_empty() {
                format!("{layout}/{name}.swift")
            } else {
                format!("{base}/{layout}/{name}.swift")
            };
            if exists(&candidate) {
                return Some(candidate);
            }
        }
        folders.pop()?;
    }
}

/// True for a path that holds Swift tests by its name or folder alone: a
/// `.swift` file named `*Tests.swift` or `*Test.swift`, or in a folder named
/// `Tests` or `tests` or ending in `Tests` (SwiftPM's `Tests/AppTests/`,
/// Xcode's `AppTests/` and `AppUITests/`).
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || !extension.eq_ignore_ascii_case("swift") {
        return false;
    }
    stem.ends_with("Tests")
        || stem.ends_with("Test")
        || folders
            .iter()
            .any(|folder| folder.ends_with("Tests") || *folder == "tests")
}

/// True for Swift source that holds tests although its path does not say
/// so: it imports XCTest and defines a `test…` method in a test case, or
/// imports Testing and marks a function `@Test`.
pub(super) fn holds_tests(source: &str) -> bool {
    (source.contains("import XCTest")
        && source.contains("TestCase")
        && source.contains("func test"))
        || (source.contains("import Testing") && source.contains("@Test"))
}

/// The suppressions a Swift comment holds: SwiftLint's `swiftlint:disable`
/// and its `:next`, `:this` and `:previous` forms, and swift-format's
/// `swift-format-ignore` and `swift-format-ignore-file`. The directive must
/// open the comment.
fn comment_keys(raw: &str) -> Vec<String> {
    let body = raw
        .strip_prefix("//")
        .or_else(|| {
            raw.strip_prefix("/*")
                .map(|inner| inner.strip_suffix("*/").unwrap_or(inner))
        })
        .unwrap_or(raw)
        .trim();
    if let Some(rest) = body.strip_prefix("swiftlint:disable") {
        let (tool, rest) = match rest.strip_prefix(':') {
            Some(modified) => {
                let (modifier, rest) = modified
                    .split_once(char::is_whitespace)
                    .unwrap_or((modified, ""));
                if !matches!(modifier, "next" | "this" | "previous") {
                    return Vec::new();
                }
                ("swiftlint", rest)
            }
            None if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
                ("swiftlint-range", rest)
            }
            None => return Vec::new(),
        };
        let rules: Vec<String> = rest
            .split_whitespace()
            .take_while(|word| !word.starts_with('-') && !word.starts_with("//"))
            .map(|word| if word == "all" { "*" } else { word }.to_owned())
            .collect();
        return keyed(tool, rules);
    }
    for (form, tool) in [
        ("swift-format-ignore-file", "swift-format-file"),
        ("swift-format-ignore", "swift-format"),
    ] {
        if let Some(rest) = body.strip_prefix(form) {
            let rules = match rest.chars().next() {
                None => Vec::new(),
                Some(':') => rest[1..].split(',').map(str::to_owned).collect(),
                Some(c) if c.is_whitespace() => Vec::new(),
                Some(_) => continue,
            };
            return keyed(tool, rules);
        }
    }
    Vec::new()
}

/// The lint suppressions in a Swift source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let language: Language = tree_sitter_swift::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| {
        if !is_comment(node) {
            return true;
        }
        let raw = text(node, src);
        for key in comment_keys(raw) {
            found.push(Suppression {
                key,
                line: node.start_position().row + 1,
                text: shown(raw),
            });
        }
        false
    });
    found
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

    fn keys(source: &str) -> Vec<String> {
        suppressions(source).into_iter().map(|s| s.key).collect()
    }

    #[test]
    fn xctest_methods_are_read() {
        let file = read(
            r#"import XCTest
@testable import Shop

final class CartTests: XCTestCase {
    func testTotal() throws {
        let cart = Cart()
        XCTAssertEqual(cart.total, 0)
        let item = try XCTUnwrap(cart.items.first)
        XCTAssertTrue(item.isValid, "valid")
    }

    func testAsync() async throws {
        XCTFail("not yet")
    }

    func helper() { XCTAssertNil(value) }
    func testWithArgument(_ value: Int) { XCTAssert(value > 0) }
    static func testStatic() { XCTAssert(a) }
    class func testClass() { XCTAssert(a) }
}

class BaseCase: XCTestCase {}
class DerivedTests: BaseCase {
    func testInherited() { XCTAssert(ok) }
}
class Plain {
    func testNotATest() { XCTAssert(ok) }
}
extension CartTests {
    func testInExtension() { XCTAssert(ok) }
}
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(
            ids(&file),
            [
                ("CartTests::testTotal", false, None, 1),
                ("CartTests::testAsync", false, None, 1),
                ("DerivedTests::testInherited", false, None, 1),
                ("CartTests::testInExtension", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 5);
        assert!(file.tests[0].uses.contains("Cart"));
        assert_eq!(
            shapes(&file),
            [
                "XCTAssertEqual(cart.total,0)",
                "XCTUnwrap(cart.items.first)",
                "XCTAssertTrue(item.isValid,\"valid\")",
                "XCTFail(\"notyet\")",
                "XCTAssertNil(value)",
                "XCTAssert(value>0)",
                "XCTAssert(a)",
                "XCTAssert(a)",
                "XCTAssert(ok)",
                "XCTAssert(ok)",
                "XCTAssert(ok)",
            ]
        );
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(file.assertions[4].test, None);
        assert_eq!(
            file.assertions[2].text,
            "XCTAssertTrue(item.isValid, \"valid\")"
        );
        assert_eq!(
            file.imports.get("Cart").map(String::as_str),
            Some("Shop/Cart")
        );
        assert!(!file.imports.contains_key("CartTests"));
    }

    #[test]
    fn swift_testing_functions_and_suites_are_read() {
        let file = read(
            r#"import Testing
@testable import Shop

@Suite("The cart", .serialized)
struct CartTests {
    @Test func adds() {
        #expect(total([1, 2]) == 3)
        let first = try #require(items.first)
    }

    @Test("A display name", .tags(.fast))
    func named() async throws {
        #expect(throws: CartError.self) { try checkout() }
    }

    struct Nested {
        @Testing.Test func deep() { #expect(ok) }
    }
}

@Test func free() { #expect(x == 1, "one") }
func helper() { #expect(y) }
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(
            ids(&file),
            [
                ("CartTests::adds", false, None, 1),
                ("CartTests::named", false, None, 1),
                ("CartTests::Nested::deep", false, None, 1),
                ("free", false, None, 1),
            ]
        );
        assert_eq!(
            shapes(&file),
            [
                "#expect(total([1,2])==3)",
                "#require(items.first)",
                "#expect(throws:CartError.self){trycheckout()}",
                "#expect(ok)",
                "#expect(x==1,\"one\")",
                "#expect(y)",
            ]
        );
        assert_eq!(file.assertions[5].test, None);
    }

    #[test]
    fn every_kind_of_skip_is_read() {
        let file = read(
            r#"import XCTest
import Testing

final class SkipTests: XCTestCase {
    func testThrowsSkip() throws {
        throw XCTSkip("later")
        XCTAssert(a)
    }
    func testSkipIf() throws {
        try XCTSkipIf(ProcessInfo.isCI, "slow")
        XCTAssert(a)
    }
    func testSkipIfFalse() throws {
        try XCTSkipIf(false)
        XCTAssert(a)
    }
    func testSkipUnlessTrue() throws {
        try XCTSkipUnless(true)
        XCTAssert(a)
    }
    func testLaterSkip() throws {
        XCTAssert(a)
        if slow { throw XCTSkip("slow") }
    }
    func testEmpty() {}
    func testReturns() {
        return
    }
}

final class OffTests: XCTestCase {
    override func setUpWithError() throws {
        throw XCTSkip("the whole class")
    }
    func testOne() { XCTAssert(a) }
}

struct Traits {
    @Test(.disabled("flaky")) func disabled() { #expect(a) }
    @Test(.disabled) func disabledBare() { #expect(a) }
    @Test(.disabled(if: isCI)) func disabledIf() { #expect(a) }
    @Test(.disabled(if: false)) func notDisabled() { #expect(a) }
    @Test(.enabled(if: false)) func enabledFalse() { #expect(a) }
    @Test(.enabled(if: true)) func enabledTrue() { #expect(a) }
    @Test(.bug("FB1"), .tags(.slow)) func otherTraits() { #expect(a) }
}

@Suite(.disabled("later"))
struct OffSuite {
    @Test func inside() { #expect(a) }
    struct Deeper {
        @Test func deeper() { #expect(a) }
    }
}
"#,
        );
        assert!(!file.has_errors);
        let why = Some("its body does nothing");
        assert_eq!(
            ids(&file),
            [
                ("SkipTests::testThrowsSkip", true, None, 1),
                ("SkipTests::testSkipIf", true, None, 1),
                ("SkipTests::testSkipIfFalse", false, None, 1),
                ("SkipTests::testSkipUnlessTrue", false, None, 1),
                ("SkipTests::testLaterSkip", false, None, 1),
                ("SkipTests::testEmpty", true, why, 1),
                ("SkipTests::testReturns", true, why, 1),
                ("OffTests::testOne", true, None, 1),
                ("Traits::disabled", true, None, 1),
                ("Traits::disabledBare", true, None, 1),
                ("Traits::disabledIf", true, None, 1),
                ("Traits::notDisabled", false, None, 1),
                ("Traits::enabledFalse", true, None, 1),
                ("Traits::enabledTrue", false, None, 1),
                ("Traits::otherTraits", false, None, 1),
                ("OffSuite::inside", true, None, 1),
                ("OffSuite::Deeper::deeper", true, None, 1),
            ]
        );
        // After `throw XCTSkip`, the assertion never runs.
        assert_eq!(file.assertions[0].line, 11);
    }

    #[test]
    fn arguments_weigh_a_test_and_its_assertions() {
        let file = read(
            r#"import Testing
struct T {
    @Test(arguments: [1, 2, 3]) func three(n: Int) { #expect(n > 0) }
    @Test("Named", arguments: ["a", "b"]) func two(s: String) { #expect(!s.isEmpty) }
    @Test(arguments: [1, 2], ["a", "b", "c"]) func product(n: Int, s: String) { #expect(n > 0) }
    @Test(arguments: zip([1, 2, 3], ["a", "b"])) func zipped(n: Int, s: String) { #expect(n > 0) }
    @Test(arguments: Kind.allCases) func cases(k: Kind) { #expect(k.ok) }
    @Test(arguments: []) func none(n: Int) { #expect(n > 0) }
}
"#,
        );
        let weights: Vec<(&str, usize)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.weight))
            .collect();
        assert_eq!(
            weights,
            [
                ("T::three", 3),
                ("T::two", 2),
                ("T::product", 6),
                ("T::zipped", 2),
                ("T::cases", 1),
                ("T::none", 1),
            ]
        );
        let assertion_weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(assertion_weights, [3, 2, 6, 2, 1, 1]);
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"import XCTest
final class T: XCTestCase {
    func testPlaceholders() {
        XCTAssertTrue(true)
        XCTAssert(true, "msg")
        XCTAssert(1 == 1)
        XCTAssertEqual(1, 1)
        XCTAssertEqual("a", "a", "same")
        XCTAssertEqual(-1, -1)
        XCTAssertFalse(false)
        XCTAssertNil(nil)
        #expect(true)
        #expect(2 == 2)
        XCTAssertEqual(total, 3)
        XCTAssertTrue(true && ok)
        XCTAssertEqual(1, 2)
        XCTAssertNotEqual(1, 2)
        #expect(false)
        #expect(1 != 1)
        XCTFail("unreachable")
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "XCTAssertEqual(total,3)",
                "XCTAssertTrue(true&&ok)",
                "XCTAssertEqual(1,2)",
                "XCTAssertNotEqual(1,2)",
                "#expect(false)",
                "#expect(1!=1)",
                "XCTFail(\"unreachable\")",
            ]
        );
    }

    #[test]
    fn assertions_that_never_run_are_not_counted() {
        let file = read(
            r#"import XCTest
final class T: XCTestCase {
    func testAfterReturn() {
        let total = 3
        return
        let other = 4
        XCTAssertEqual(total, 3)
    }
    func testIfFalse() {
        if false {
            XCTAssertEqual(total(), 3)
        }
        if (false) { XCTAssert(a) } else { XCTAssert(b) }
        while false { XCTAssert(c) }
        withKnownIssue { #expect(d) }
        XCTExpectFailure("flaky") { XCTAssert(e) }
    }
    func testReachable() {
        if true { XCTAssert(f) }
        guard ready else { return }
        XCTAssert(g)
    }
    func testReturnsAnExpression() {
        return
        XCTAssert(h)
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "XCTAssert(b)",
                "XCTAssert(f)",
                "XCTAssert(g)",
                "XCTAssert(h)"
            ]
        );
        assert!(file.tests.iter().all(|t| !t.skipped));
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r#"import XCTest
// func testCommented() { XCTAssert(x) }
/* XCTAssert(y) */
let s = "func testInString() { XCTAssert(x) }"
let m = """
XCTAssertEqual(a, b)
"""
func testFree() { XCTAssert(z) }
struct NotACase {
    func testNothing() {}
}
"#,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert_eq!(shapes(&file), ["XCTAssert(z)"]);
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read(
            "import XCTest\nfinal class T: XCTestCase {\n    func testA() {\n        XCTAssert(\n",
        );
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn subjects_resolve_to_a_file_per_type() {
        let exists = |path: &str| {
            matches!(
                path,
                "Sources/Shop/Cart.swift" | "App/Price.swift" | "pkg/Sources/Core/Tax.swift"
            )
        };
        assert_eq!(
            resolve("Tests/ShopTests/CartTests.swift", "Shop/Cart", &exists).as_deref(),
            Some("Sources/Shop/Cart.swift")
        );
        assert_eq!(
            resolve("AppTests/PriceTests.swift", "App/Price", &exists).as_deref(),
            Some("App/Price.swift")
        );
        assert_eq!(
            resolve("pkg/Tests/CoreTests/TaxTests.swift", "Core/Tax", &exists).as_deref(),
            Some("pkg/Sources/Core/Tax.swift")
        );
        assert_eq!(resolve("Tests/A.swift", "Shop/Missing", &exists), None);
        assert_eq!(resolve("Tests/A.swift", "Shop", &exists), None);
        // Without `@testable`, the first module that is not Apple's.
        let file = read("import XCTest\nimport Foundation\nimport Shop\nfinal class T: XCTestCase {\n    func testA() { XCTAssert(Cart().ok) }\n}\n");
        assert_eq!(
            file.imports.get("Cart").map(String::as_str),
            Some("Shop/Cart")
        );
        let file = read("import XCTest\nfinal class T: XCTestCase {\n    func testA() { XCTAssert(Cart().ok) }\n}\n");
        assert!(file.imports.is_empty());
    }

    #[test]
    fn test_paths() {
        for path in [
            "Tests/ShopTests/CartTests.swift",
            "Tests/helpers.swift",
            "AppTests/LoginTest.swift",
            "AppUITests/Flows.swift",
            "Sources/Shop/CartTests.swift",
            "tests/a.swift",
            "x/YTests/A.SWIFT",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "Sources/Shop/Cart.swift",
            "Testing/a.swift",
            "Tests/a.py",
            "Tests/.swift",
            "Tests",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_looks_for_a_framework_and_a_test() {
        assert!(holds_tests(
            "import XCTest\nfinal class A: XCTestCase {\n func testA() {}\n}"
        ));
        assert!(holds_tests("import Testing\n@Test func a() {}"));
        assert!(!holds_tests("import Foundation\nfunc testA() {}"));
        assert!(!holds_tests("import XCTest\nlet testCase = 1"));
    }

    #[test]
    fn swiftlint_and_swift_format_comments_turn_rules_off() {
        let source = r#"
// swiftlint:disable force_cast line_length
// swiftlint:disable:next force_try
let a = try! f() // swiftlint:disable:this force_try
// swiftlint:disable:previous identifier_name
/* swiftlint:disable all */
// swiftlint:disable all - legacy code
// swift-format-ignore
// swift-format-ignore: NeverForceUnwrap, NoBlockComments
// swift-format-ignore-file
"#;
        assert_eq!(
            keys(source),
            [
                "swiftlint-range:force_cast",
                "swiftlint-range:line_length",
                "swiftlint:force_try",
                "swiftlint:force_try",
                "swiftlint:identifier_name",
                "swiftlint-range:*",
                "swiftlint-range:*",
                "swift-format:*",
                "swift-format:NeverForceUnwrap",
                "swift-format:NoBlockComments",
                "swift-format-file:*",
            ]
        );
        let found = suppressions(source);
        assert_eq!(found[2].line, 3);
        assert_eq!(found[2].text, "// swiftlint:disable:next force_try");
    }

    #[test]
    fn swift_look_alikes_are_not_suppressions() {
        let source = r#"
// swiftlint:enable force_cast
// We never use swiftlint:disable here.
// swiftlint:disabled
// swiftlint:disable:sometimes force_cast
/// swiftlint:disable force_cast
let s = "// swiftlint:disable all"
let t = """
// swift-format-ignore
"""
// swift-format-ignored
"#;
        assert!(keys(source).is_empty(), "{:?}", keys(source));
    }
}
