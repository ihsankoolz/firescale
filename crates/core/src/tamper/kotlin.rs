//! The Kotlin reader of the tamper guard (M32, ADR 0052): JUnit 4 and 5,
//! kotlin.test and Kotest.
//!
//! - A test is a function annotated `@Test` (JUnit, kotlin.test),
//!   `@ParameterizedTest`, `@RepeatedTest`, `@TestFactory` or
//!   `@TestTemplate` in a class or object that is not anonymous or local;
//!   its id is the names of the classes around it and its own, joined with
//!   `::`. In a Kotest spec (`FunSpec`, `StringSpec`, `DescribeSpec`,
//!   `ShouldSpec`, `FreeSpec`, `ExpectSpec`, `FeatureSpec`), a test is a
//!   call written in the spec's body: `test("…") { }`, `it`, `should`,
//!   `expect`, `scenario`, or a string called with a lambda (`"…" { }`),
//!   in containers (`context`, `describe`, `feature`, FreeSpec's
//!   `"…" - { }`); its id is the spec's name, its containers' and its own.
//!   Tests made in a loop or by `withData` are not counted.
//! - It is skipped with `@Ignore`, `@Disabled`, Kotest's `@Ignored`, a
//!   JUnit 5 condition (on it or its class), Kotest's `x…` forms (`xtest`,
//!   `xit`, `xdescribe`), a name starting with `!`, `.config(enabled =
//!   false)` or an `enabledIf` (it runs only sometimes), when its first
//!   statement skips it (`assumeTrue(false)`), or when its body does
//!   nothing. A top-level Kotest name starting with `f:` is focused.
//! - An assertion is a call named `assert…` (kotlin.test, JUnit, AssertJ
//!   chains), `assertFailsWith<…> { }`, `fail(…)`, a call or infix call
//!   whose name starts with `should` (Kotest's `x shouldBe y`,
//!   `shouldThrow<…> { }`), or a MockK or Mockito verification
//!   (`verify { … }`); not one that never runs or cannot fail
//!   (`assertTrue(true)`, `1 shouldBe 1`).
//! - Imports and the capitalised names of the file's own package are its
//!   subjects, as in Java ([`super::java::resolve`]).
//! - Suppressions: `@Suppress` and `@file:Suppress` (Kotlin, detekt,
//!   ktlint's `ktlint:rule`), `@SuppressWarnings`, Android's
//!   `@SuppressLint`, `// ktlint-disable [rules]`.
//!
//! The grammar reads `true`, `false` and `null` as identifiers, and gives
//! `if` and `while` no field for their body: code under `if (false)` is
//! found here, not by [`reach`].

use std::collections::{BTreeMap, BTreeSet, HashSet};

use tree_sitter::{Language, Node};

use super::java::{
    assertion, assign, assumes_false, children, comment_body, in_test_folder, library_receiver,
    passes, same_package, split_path, squeezed, starts_with_test, syntax_errors, text_block_rows,
    Literal, CONDITIONS, DOES_NOTHING, SKIPS, SKIPS_FIRST,
};
use super::reach::{self, Grammar};
use super::suppress::{directive, keyed, shown};
use super::{Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// What the Kotlin grammar calls the nodes [`reach`] looks at. Its `if`
/// and `while` have no body field: [`never_run`] finds what they hold.
const GRAMMAR: Grammar = Grammar {
    blocks: &["block", "lambda_literal"],
    returns: &["return_expression"],
    ifs: &[],
    whiles: &[],
    hoisted: &[
        "class_declaration",
        "object_declaration",
        "function_declaration",
    ],
    stops: Some(stops),
    noop,
};

/// The annotations that make a function a test.
const TESTS: [&str; 5] = [
    "Test",
    "ParameterizedTest",
    "RepeatedTest",
    "TestFactory",
    "TestTemplate",
];

/// The Kotest specs whose tests are read.
const SPECS: [&str; 7] = [
    "FunSpec",
    "StringSpec",
    "DescribeSpec",
    "ShouldSpec",
    "FreeSpec",
    "ExpectSpec",
    "FeatureSpec",
];

/// Kotest's calls that make one test.
const LEAVES: [&str; 5] = ["test", "it", "should", "expect", "scenario"];
/// Kotest's calls that make a group of tests.
const CONTAINERS: [&str; 3] = ["context", "describe", "feature"];

/// MockK's and Mockito's verifications.
const VERIFIES: [&str; 12] = [
    "verify",
    "coVerify",
    "verifyAll",
    "coVerifyAll",
    "verifyOrder",
    "coVerifyOrder",
    "verifySequence",
    "coVerifySequence",
    "confirmVerified",
    "verifyNoMoreInteractions",
    "verifyNoInteractions",
    "verifyZeroInteractions",
];

/// The nodes of a value that is the same on every run, besides the
/// identifiers `true`, `false` and `null`.
const LITERALS: [&str; 13] = [
    "number_literal",
    "float_literal",
    "long_literal",
    "unsigned_literal",
    "hex_literal",
    "bin_literal",
    "character_literal",
    "string_literal",
    "string_content",
    "escape_sequence",
    "parenthesized_expression",
    "unary_expression",
    "binary_expression",
];

/// True for what a lambda holds besides its statements: its parameters.
fn noop(statement: Node<'_>, _src: &[u8]) -> bool {
    statement.kind() == "lambda_parameters"
}

/// The node a call calls: `assertEquals`, `kotlin.test.assertEquals`,
/// `test("…")` for `test("…") { }`.
fn callee(call: Node<'_>) -> Option<Node<'_>> {
    children(call).first().copied()
}

/// The name of a call: the last identifier of what it calls, through a
/// trailing lambda (`verify { }`, `test("…") { }`).
fn call_name<'s>(call: Node<'_>, src: &'s [u8]) -> &'s str {
    let mut at = call;
    for _ in 0..8 {
        let Some(called) = callee(at) else {
            return "";
        };
        match called.kind() {
            "identifier" => return text(called, src),
            "navigation_expression" => {
                return children(called)
                    .last()
                    .filter(|last| last.kind() == "identifier")
                    .map_or("", |last| text(*last, src));
            }
            "call_expression" => at = called,
            _ => return "",
        }
    }
    ""
}

/// The value arguments of a call: its own, or (with a trailing lambda)
/// those of the call it wraps.
fn value_arguments(call: Node<'_>) -> Vec<Node<'_>> {
    let mut at = call;
    for _ in 0..8 {
        let parts = children(at);
        if let Some(arguments) = parts.iter().find(|part| part.kind() == "value_arguments") {
            return children(*arguments);
        }
        match parts.first() {
            Some(inner) if inner.kind() == "call_expression" => at = *inner,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// A value argument's name (`enabled` in `enabled = false`) and value.
fn argument<'t>(value_argument: Node<'t>, src: &[u8]) -> (Option<String>, Option<Node<'t>>) {
    let parts = children(value_argument);
    let mut cursor = value_argument.walk();
    let named = value_argument
        .children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == "=");
    if named && parts.len() >= 2 {
        (
            parts.first().map(|name| text(*name, src).to_owned()),
            parts.last().copied(),
        )
    } else {
        (None, parts.last().copied())
    }
}

/// The value as a [`Literal`], or `None` when it is not one.
fn literal(node: Node<'_>, src: &[u8]) -> Option<Literal> {
    let mut constant = true;
    walk(node, |inner| {
        if !constant {
            return false;
        }
        if inner.is_named() && !inner.is_extra() {
            constant = LITERALS.contains(&inner.kind())
                || (inner.kind() == "identifier"
                    && matches!(text(inner, src), "true" | "false" | "null"));
        }
        constant
    });
    constant.then(|| Literal::new(squeezed(node, src), node.kind() == "string_literal"))
}

/// The arguments of a call, each as a [`Literal`] when it is one.
fn literal_arguments(call: Node<'_>, src: &[u8]) -> Vec<Option<Literal>> {
    value_arguments(call)
        .into_iter()
        .map(|value_argument| {
            argument(value_argument, src)
                .1
                .and_then(|value| literal(value, src))
        })
        .collect()
}

/// True for a statement after which the rest of its block does not run:
/// `assumeTrue(false)`, `Assumptions.abort()`.
fn stops(statement: Node<'_>, src: &[u8]) -> bool {
    statement.kind() == "call_expression"
        && assumes_false(
            call_name(statement, src),
            &literal_arguments(statement, src),
        )
}

/// True for the name of a call that asserts whatever it is called on.
/// Kotest's `should("…") { }` makes a test and asserts nothing.
fn asserting_name(name: &str) -> bool {
    name.starts_with("assert") || (name.starts_with("should") && name != "should")
}

/// True for a call that asserts: one named `assert…` or `should…`, or
/// `fail(…)` and the verifications of MockK and Mockito called on their
/// library ([`library_receiver`]).
fn asserts(call: Node<'_>, src: &[u8]) -> bool {
    let name = call_name(call, src);
    if asserting_name(name) {
        return true;
    }
    let receiver = callee(call)
        .filter(|called| called.kind() == "navigation_expression")
        .and_then(|called| children(called).first().copied())
        .map(|receiver| text(receiver, src));
    (name == "fail" || VERIFIES.contains(&name)) && library_receiver(receiver)
}

/// The calls of a chain, outermost first: `assertThat(x).isEqualTo(3)` is
/// `isEqualTo(…)` then `assertThat(…)`; `verify(m) { }` is the call with
/// its lambda, then `verify(m)`.
fn chain(call: Node<'_>) -> Vec<Node<'_>> {
    let mut links = vec![call];
    let mut at = call;
    while let Some(called) = callee(at) {
        let next = match called.kind() {
            "call_expression" => Some(called),
            "navigation_expression" => children(called)
                .first()
                .copied()
                .filter(|receiver| receiver.kind() == "call_expression"),
            _ => None,
        };
        let Some(next) = next else {
            break;
        };
        links.push(next);
        at = next;
    }
    links
}

/// True for a call chain that cannot fail: an assertion given literals
/// that pass ([`passes`]), `1.shouldBe(1)`, AssertJ's
/// `assertThat(1).isEqualTo(1)`.
fn cannot_fail(links: &[Node<'_>], src: &[u8]) -> bool {
    let all = |call: Node<'_>| -> Option<Vec<Literal>> {
        literal_arguments(call, src).into_iter().collect()
    };
    match links {
        [call] => {
            let name = call_name(*call, src);
            let Some(arguments) = all(*call) else {
                return false;
            };
            if name == "shouldBe" {
                let receiver = callee(*call)
                    .filter(|called| called.kind() == "navigation_expression")
                    .and_then(|called| children(called).first().copied())
                    .and_then(|receiver| literal(receiver, src));
                return match (receiver, arguments.as_slice()) {
                    (Some(receiver), [expected]) => receiver.text == expected.text,
                    _ => false,
                };
            }
            passes(name, &arguments)
        }
        [outer, head] if call_name(*head, src) == "assertThat" => {
            let (Some(subject), Some(expected)) = (all(*head), all(*outer)) else {
                return false;
            };
            let [subject] = subject.as_slice() else {
                return false;
            };
            match (call_name(*outer, src), expected.as_slice()) {
                ("isEqualTo" | "isSameAs", [expected]) => subject.text == expected.text,
                ("isTrue", []) => subject.text == "true",
                ("isFalse", []) => subject.text == "false",
                _ => false,
            }
        }
        _ => false,
    }
}

/// An infix assertion (`total shouldBe 3`): its name, and whether it
/// cannot fail (`1 shouldBe 1`). `None` for any other infix call.
fn infix_assertion(node: Node<'_>, src: &[u8]) -> Option<bool> {
    let parts = children(node);
    let [left, name, right] = parts.as_slice() else {
        return None;
    };
    let name = text(*name, src);
    if !name.starts_with("should") {
        return None;
    }
    let same = match (literal(*left, src), literal(*right, src)) {
        (Some(left), Some(right)) => name == "shouldBe" && left.text == right.text,
        _ => false,
    };
    Some(same)
}

/// True for `assertFailsWith<E> { }` or `shouldThrow<E> { }`, which the
/// grammar reads as two comparisons: `(name < E) > { … }`.
fn generic_assertion(node: Node<'_>, src: &[u8]) -> bool {
    let operator = |binary: Node<'_>| {
        binary
            .child_by_field_name("operator")
            .map_or("", |operator| text(operator, src))
    };
    if operator(node) != ">" {
        return false;
    }
    let Some(left) = node
        .child_by_field_name("left")
        .filter(|left| left.kind() == "binary_expression" && operator(*left) == "<")
    else {
        return false;
    };
    let lambda = node
        .child_by_field_name("right")
        .is_some_and(|right| right.kind() == "lambda_literal");
    lambda
        && left
            .child_by_field_name("left")
            .is_some_and(|name| name.kind() == "identifier" && asserting_name(text(name, src)))
}

/// True for a condition that is the literal `false`, in parentheses or not.
fn is_false(condition: Node<'_>, src: &[u8]) -> bool {
    let mut at = condition;
    for _ in 0..8 {
        match at.kind() {
            "parenthesized_expression" => match children(at).first() {
                Some(inner) => at = *inner,
                None => return false,
            },
            "identifier" => return text(at, src) == "false",
            _ => return false,
        }
    }
    false
}

/// The byte ranges that never run: the body of `if (false)` and of
/// `while (false)` (not an `else`).
fn never_run(root: Node<'_>, src: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    walk(root, |node| {
        if matches!(node.kind(), "if_expression" | "while_statement") {
            if let Some(condition) = node.child_by_field_name("condition") {
                if is_false(condition, src) {
                    let body = children(node)
                        .into_iter()
                        .find(|child| child.start_byte() >= condition.end_byte());
                    if let Some(body) = body {
                        ranges.push((body.start_byte(), body.end_byte()));
                    }
                }
            }
        }
        true
    });
    ranges
}

/// The annotations written on a declaration, in source order.
fn annotations(declaration: Node<'_>) -> Vec<Node<'_>> {
    children(declaration)
        .into_iter()
        .find(|child| child.kind() == "modifiers")
        .map(children)
        .unwrap_or_default()
        .into_iter()
        .filter(|child| child.kind() == "annotation")
        .collect()
}

/// The last identifier of an annotation's type (`Test` for
/// `@org.junit.Test`), and its arguments, if it has any. A
/// `file_annotation` may hold several.
fn annotation_parts<'t>(annotation: Node<'t>, src: &[u8]) -> Vec<(String, Vec<Node<'t>>)> {
    let last = |user_type: Node<'_>| {
        children(user_type)
            .last()
            .map(|name| text(*name, src).to_owned())
            .unwrap_or_default()
    };
    children(annotation)
        .into_iter()
        .filter_map(|part| match part.kind() {
            "user_type" => Some((last(part), Vec::new())),
            "constructor_invocation" => {
                let inner = children(part);
                let name = inner
                    .iter()
                    .find(|child| child.kind() == "user_type")
                    .map(|user_type| last(*user_type))?;
                let arguments = inner
                    .iter()
                    .find(|child| child.kind() == "value_arguments")
                    .map(|arguments| children(*arguments))
                    .unwrap_or_default();
                Some((name, arguments))
            }
            _ => None,
        })
        .collect()
}

/// How many values an annotation argument holds: the elements of `[…]`
/// or `arrayOf(…)`, else 1.
fn values(value: Node<'_>) -> usize {
    match value.kind() {
        "collection_literal" => children(value).len(),
        "call_expression" => value_arguments(value).len(),
        _ => 1,
    }
}

/// What the annotations of a function or class say about it.
#[derive(Default)]
struct Marks {
    test: bool,
    parameterized: bool,
    skipped: bool,
    rows: usize,
}

/// Reads the annotations of a function or class.
fn marks(declaration: Node<'_>, src: &[u8]) -> Marks {
    let mut found = Marks::default();
    for annotation in annotations(declaration) {
        for (name, arguments) in annotation_parts(annotation, src) {
            let name = name.as_str();
            found.test |= TESTS.contains(&name);
            found.skipped |=
                SKIPS.contains(&name) || CONDITIONS.iter().any(|start| name.starts_with(start));
            let arguments: Vec<(Option<String>, Node<'_>)> = arguments
                .into_iter()
                .filter_map(|value_argument| {
                    let (key, value) = argument(value_argument, src);
                    value.map(|value| (key, value))
                })
                .collect();
            match name {
                "ParameterizedTest" => found.parameterized = true,
                "Test" => {
                    found.skipped |= arguments.iter().any(|(key, value)| {
                        key.as_deref() == Some("enabled") && text(*value, src) != "true"
                    });
                }
                "ValueSource" => {
                    found.rows += arguments
                        .iter()
                        .map(|(_, value)| values(*value))
                        .sum::<usize>();
                }
                "CsvSource" => {
                    for (key, value) in &arguments {
                        match key.as_deref() {
                            None | Some("value") => found.rows += values(*value),
                            Some("textBlock") => {
                                found.rows += text_block_rows(text(*value, src));
                            }
                            Some(_) => {}
                        }
                    }
                }
                "NullSource" | "EmptySource" => found.rows += 1,
                "NullAndEmptySource" => found.rows += 2,
                _ => {}
            }
        }
    }
    found
}

/// The identifiers under a node.
fn identifiers(node: Node<'_>, src: &[u8], into: &mut BTreeSet<String>) {
    walk(node, |inner| {
        if inner.kind() == "identifier" {
            into.insert(text(inner, src).to_owned());
        }
        true
    });
}

/// The identifiers the properties among `nodes` use: tests use them too.
fn property_identifiers(nodes: &[Node<'_>], src: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for property in nodes
        .iter()
        .filter(|node| node.kind() == "property_declaration")
    {
        identifiers(*property, src, &mut found);
    }
    found
}

/// The body of a class or object: its `class_body`, if any.
fn class_body(declaration: Node<'_>) -> Option<Node<'_>> {
    children(declaration)
        .into_iter()
        .find(|child| child.kind() == "class_body")
}

/// True for a function a test runner can find: declared in the body of a
/// named class or object, not of an anonymous object or a companion.
fn in_declared_class(function: Node<'_>) -> bool {
    function
        .parent()
        .filter(|body| body.kind() == "class_body")
        .and_then(|body| body.parent())
        .is_some_and(|owner| matches!(owner.kind(), "class_declaration" | "object_declaration"))
}

/// Why a test function that is not marked skipped does not run, if it
/// does not: an empty body, `= Unit`, or a first statement that skips.
fn not_run(function: Node<'_>, src: &[u8]) -> Option<&'static str> {
    let body = children(function)
        .into_iter()
        .find(|child| child.kind() == "function_body")?;
    let inner = children(body).first().copied()?;
    match inner.kind() {
        "block" => body_not_run(inner, src),
        "identifier" if text(inner, src) == "Unit" => Some(DOES_NOTHING),
        _ => None,
    }
}

/// Why a block or lambda that is a test's body does not run, if it does
/// not.
fn body_not_run(body: Node<'_>, src: &[u8]) -> Option<&'static str> {
    if reach::does_nothing(body, src, &GRAMMAR) {
        return Some(DOES_NOTHING);
    }
    children(body)
        .into_iter()
        .find(|statement| statement.kind() != "lambda_parameters")
        .is_some_and(|first| stops(first, src))
        .then_some(SKIPS_FIRST)
}

/// A class or function open around the node being visited.
enum Scope {
    Class {
        name: String,
        skipped: bool,
        properties: BTreeSet<String>,
    },
    Function,
}

/// What a test needs to know of the classes around it.
struct Around {
    names: Vec<String>,
    skipped: bool,
    properties: BTreeSet<String>,
}

impl Around {
    /// What the classes open around a test say.
    fn of(open: &[(usize, Scope)]) -> Self {
        let mut around = Self {
            names: Vec::new(),
            skipped: false,
            properties: BTreeSet::new(),
        };
        for (_, scope) in open {
            if let Scope::Class {
                name,
                skipped,
                properties,
            } = scope
            {
                around.names.push(name.clone());
                around.skipped |= *skipped;
                around.properties.extend(properties.iter().cloned());
            }
        }
        around
    }

    /// The id of a test named `own` in the containers `names`.
    fn id(&self, names: &[String], own: &str) -> String {
        self.names
            .iter()
            .chain(names)
            .map(String::as_str)
            .chain(std::iter::once(own))
            .collect::<Vec<_>>()
            .join("::")
    }
}

/// The test a function is, if it is one, in the classes `open`.
fn test_function(function: Node<'_>, src: &[u8], open: &[(usize, Scope)]) -> Option<TestCase> {
    let own = marks(function, src);
    if !own.test {
        return None;
    }
    let around = &Around::of(open);
    let name_node = function.child_by_field_name("name");
    let name = name_node.map_or("", |name| text(name, src));
    let skipped = own.skipped || around.skipped;
    let why = if skipped {
        None
    } else {
        not_run(function, src)
    };
    let mut uses = around.properties.clone();
    identifiers(function, src, &mut uses);
    Some(TestCase {
        id: around.id(&[], name),
        line: name_node.unwrap_or(function).start_position().row + 1,
        skipped: skipped || why.is_some(),
        skipped_rows: 0,
        why,
        marker: false,
        focused: false,
        uses,
        weight: if own.parameterized {
            own.rows.max(1)
        } else {
            1
        },
    })
}

/// The bodies of the Kotest spec a class or object extends (the lambda
/// given to its constructor, its `init` blocks), if it extends one.
fn spec_lambdas<'t>(declaration: Node<'t>, src: &[u8]) -> Option<Vec<Node<'t>>> {
    let invocation = children(declaration)
        .into_iter()
        .find(|child| child.kind() == "delegation_specifiers")
        .map(children)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|specifier| children(specifier).first().copied())
        .find(|invocation| invocation.kind() == "constructor_invocation")?;
    let parts = children(invocation);
    let name = parts
        .iter()
        .find(|part| part.kind() == "user_type")
        .and_then(|user_type| children(*user_type).last().copied())
        .map_or("", |name| text(name, src));
    if !SPECS.contains(&name) {
        return None;
    }
    // The body passed to the constructor, and the class's `init` blocks.
    let mut bodies: Vec<Node<'_>> = parts
        .iter()
        .filter(|part| part.kind() == "value_arguments")
        .flat_map(|arguments| children(*arguments))
        .filter_map(|value_argument| children(value_argument).last().copied())
        .filter(|value| value.kind() == "lambda_literal")
        .collect();
    bodies.extend(
        class_body(declaration)
            .map(children)
            .unwrap_or_default()
            .into_iter()
            .filter(|member| member.kind() == "anonymous_initializer")
            .filter_map(|initializer| {
                children(initializer)
                    .into_iter()
                    .find(|child| child.kind() == "block")
            }),
    );
    Some(bodies)
}

/// One Kotest call: a test or a group of tests.
struct Kotest<'t> {
    name: String,
    container: bool,
    skipped: bool,
    focused: bool,
    body: Node<'t>,
}

/// The text of a string literal without its quotes.
fn string_text(literal: Node<'_>, src: &[u8]) -> String {
    text(literal, src).trim_matches('"').to_owned()
}

/// True when `.config(…)` arguments turn a test off, always (`enabled =
/// false`) or sometimes (`enabled = onCi`, `enabledIf = { … }`).
fn config_skips(arguments: &[Node<'_>], src: &[u8]) -> bool {
    arguments.iter().any(|value_argument| {
        let (key, value) = argument(*value_argument, src);
        match key.as_deref() {
            Some("enabled") => value.is_none_or(|value| text(value, src) != "true"),
            Some("enabledIf" | "enabledOrReasonIf") => true,
            _ => false,
        }
    })
}

/// What a statement of a spec's body is, if it is a Kotest call.
fn kotest<'t>(statement: Node<'t>, src: &[u8]) -> Option<Kotest<'t>> {
    let parts = children(statement);
    match statement.kind() {
        // FreeSpec's `"group" - { … }`.
        "binary_expression" => {
            let operator = statement
                .child_by_field_name("operator")
                .map_or("", |operator| text(operator, src));
            let left = statement.child_by_field_name("left")?;
            let right = statement.child_by_field_name("right")?;
            (operator == "-" && left.kind() == "string_literal" && right.kind() == "lambda_literal")
                .then(|| named(string_text(left, src), true, false, right))
        }
        "call_expression" => {
            let lambda = parts
                .last()
                .filter(|last| last.kind() == "annotated_lambda")
                .and_then(|annotated| {
                    children(*annotated)
                        .into_iter()
                        .find(|child| child.kind() == "lambda_literal")
                })?;
            let called = parts.first().copied()?;
            let (function, title, config) = head(called, src)?;
            let title = string_text(title, src);
            let config_off = config.is_some_and(|arguments| config_skips(&arguments, src));
            let mut found = match function {
                None => named(title, false, false, lambda),
                Some(function) => {
                    let (off, base) = match function.strip_prefix('x') {
                        Some(base) if LEAVES.contains(&base) || CONTAINERS.contains(&base) => {
                            (true, base)
                        }
                        _ => (false, function),
                    };
                    if LEAVES.contains(&base) {
                        named(title, false, off, lambda)
                    } else if CONTAINERS.contains(&base) {
                        named(title, true, off, lambda)
                    } else {
                        return None;
                    }
                }
            };
            found.skipped |= config_off;
            Some(found)
        }
        _ => None,
    }
}

/// A Kotest call with its title read: `!` turns it off, `f:` focuses it.
fn named(title: String, container: bool, skipped: bool, body: Node<'_>) -> Kotest<'_> {
    let (title, bang) = match title.strip_prefix('!') {
        Some(rest) => (rest.to_owned(), true),
        None => (title, false),
    };
    let (name, focused) = match title.strip_prefix("f:") {
        Some(rest) => (rest.to_owned(), true),
        None => (title, false),
    };
    Kotest {
        name,
        container,
        skipped: skipped || bang,
        focused,
        body,
    }
}

/// What a Kotest call calls: `test` and its title, `"title"` alone, and the
/// arguments of a `.config(…)` after it.
type Head<'t> = (Option<&'t str>, Node<'t>, Option<Vec<Node<'t>>>);

/// The function and title of `test("title")`, or the title of `"title"`.
fn titled<'t>(called: Node<'t>, src: &'t [u8]) -> Option<(Option<&'t str>, Node<'t>)> {
    match called.kind() {
        "string_literal" => Some((None, called)),
        "call_expression" => {
            let parts = children(called);
            let target = parts
                .first()
                .copied()
                .filter(|target| target.kind() == "identifier")?;
            let title = parts
                .iter()
                .find(|part| part.kind() == "value_arguments")
                .and_then(|arguments| children(*arguments).first().copied())
                .and_then(|first| argument(first, src).1)
                .filter(|value| value.kind() == "string_literal")?;
            Some((Some(text(target, src)), title))
        }
        _ => None,
    }
}

/// What a Kotest call calls ([`Head`]): `test("title")`, `"title"`, or
/// either followed by `.config(…)`.
fn head<'t>(called: Node<'t>, src: &'t [u8]) -> Option<Head<'t>> {
    if let Some((function, title)) = titled(called, src) {
        return Some((function, title, None));
    }
    let parts = children(called);
    let navigation = parts
        .first()
        .filter(|target| {
            called.kind() == "call_expression" && target.kind() == "navigation_expression"
        })
        .map(|navigation| children(*navigation))?;
    let [receiver, config] = navigation.as_slice() else {
        return None;
    };
    if text(*config, src) != "config" {
        return None;
    }
    let (function, title) = titled(*receiver, src)?;
    let arguments = parts
        .iter()
        .find(|part| part.kind() == "value_arguments")
        .map(|arguments| children(*arguments))
        .unwrap_or_default();
    Some((function, title, Some(arguments)))
}

/// How deep Kotest containers are followed: real specs never come near.
const MAX_DEPTH: usize = 32;

/// What [`spec_tests`] needs, and what it finds.
struct SpecWalk<'a> {
    src: &'a [u8],
    around: &'a Around,
    /// Identifiers of the spec's properties.
    shared: BTreeSet<String>,
    /// Each test, with the byte range of its call.
    found: Vec<((usize, usize), TestCase)>,
}

/// The Kotest tests among the statements of `body`, in containers named
/// `path`.
fn spec_tests(walker: &mut SpecWalk<'_>, body: Node<'_>, path: &[String], skipped: bool) {
    if path.len() > MAX_DEPTH {
        return;
    }
    let src = walker.src;
    for statement in children(body) {
        let Some(call) = kotest(statement, src) else {
            continue;
        };
        let off = skipped || call.skipped;
        if call.container {
            let mut inner = path.to_vec();
            inner.push(call.name);
            spec_tests(walker, call.body, &inner, off);
            continue;
        }
        let why = if off || walker.around.skipped {
            None
        } else {
            body_not_run(call.body, src)
        };
        let mut uses = walker.shared.clone();
        uses.extend(walker.around.properties.iter().cloned());
        identifiers(statement, src, &mut uses);
        let test = TestCase {
            id: walker.around.id(path, &call.name),
            line: statement.start_position().row + 1,
            skipped: off || walker.around.skipped || why.is_some(),
            skipped_rows: 0,
            why,
            marker: false,
            focused: call.focused && path.is_empty(),
            uses,
            weight: 1,
        };
        walker
            .found
            .push(((statement.start_byte(), statement.end_byte()), test));
    }
}

/// Reads the tests, skips and assertions of a Kotlin test file.
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    let dead = reach::Dead::of(root, src, &GRAMMAR);
    let dead_ranges = never_run(root, src);
    let never = |node: Node<'_>| {
        dead.why(node).is_some()
            || dead_ranges
                .iter()
                .any(|(start, end)| *start <= node.start_byte() && node.end_byte() <= *end)
    };
    let (has_errors, error_line) = syntax_errors(root);
    let mut file = TestFile {
        has_errors,
        error_line,
        imports: imports(root, src),
        ..TestFile::default()
    };
    let mut open: Vec<(usize, Scope)> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    // Calls inside a chain already judged with its outermost call.
    let mut chained: HashSet<usize> = HashSet::new();
    walk(root, |node| {
        while open
            .last()
            .is_some_and(|(end, _)| node.start_byte() >= *end)
        {
            open.pop();
        }
        match node.kind() {
            "class_declaration" | "object_declaration" => {
                let local = open
                    .iter()
                    .any(|(_, scope)| matches!(scope, Scope::Function));
                let outer_skipped = open
                    .iter()
                    .any(|(_, scope)| matches!(scope, Scope::Class { skipped: true, .. }));
                let body = class_body(node).map(children).unwrap_or_default();
                open.push((
                    node.end_byte(),
                    Scope::Class {
                        name: node
                            .child_by_field_name("name")
                            .map_or("", |name| text(name, src))
                            .to_owned(),
                        skipped: outer_skipped || marks(node, src).skipped,
                        properties: property_identifiers(&body, src),
                    },
                ));
                if !local {
                    if let Some(bodies) = spec_lambdas(node, src) {
                        let around = Around::of(&open);
                        let mut walker = SpecWalk {
                            src,
                            around: &around,
                            shared: BTreeSet::new(),
                            found: Vec::new(),
                        };
                        for body in &bodies {
                            walker
                                .shared
                                .extend(property_identifiers(&children(*body), src));
                        }
                        for body in bodies {
                            spec_tests(&mut walker, body, &[], false);
                        }
                        for (range, test) in walker.found {
                            ranges.push(range);
                            file.tests.push(test);
                        }
                    }
                }
                true
            }
            "function_declaration" => {
                let local = open
                    .iter()
                    .any(|(_, scope)| matches!(scope, Scope::Function));
                if !local && in_declared_class(node) {
                    if let Some(test) = test_function(node, src, &open) {
                        ranges.push((node.start_byte(), node.end_byte()));
                        file.tests.push(test);
                    }
                }
                open.push((node.end_byte(), Scope::Function));
                true
            }
            "call_expression" => {
                if chained.contains(&node.id()) {
                    return true;
                }
                let links = chain(node);
                chained.extend(links.iter().skip(1).map(|link| link.id()));
                if links.iter().any(|link| asserts(*link, src))
                    && !never(node)
                    && !cannot_fail(&links, src)
                {
                    file.assertions.push(assertion(node, src));
                    starts.push(node.start_byte());
                }
                true
            }
            "infix_expression" => {
                if infix_assertion(node, src) == Some(false) && !never(node) {
                    file.assertions.push(assertion(node, src));
                    starts.push(node.start_byte());
                }
                true
            }
            "binary_expression" => {
                if generic_assertion(node, src) && !never(node) {
                    file.assertions.push(assertion(node, src));
                    starts.push(node.start_byte());
                }
                true
            }
            "line_comment" | "block_comment" | "string_literal" | "multiline_string_literal" => {
                false
            }
            _ => true,
        }
    });
    // Kotest tests were found before the walk reached them: assertions are
    // given their test by where they are.
    assign(&mut file, &ranges, &starts);
    file
}

/// The names a Kotlin file imports, each with what it names
/// (`Cart` → `com.acme.Cart`, `Money` → `com.acme.Price` for `import
/// com.acme.Price as Money`), and the capitalised names of its own package
/// it uses without an import. Wildcard imports name nothing.
fn imports(root: Node<'_>, src: &[u8]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut package = String::new();
    for child in children(root) {
        match child.kind() {
            "package_header" => {
                if let Some(name) = children(child).first() {
                    package = squeezed(*name, src);
                }
            }
            "import" => {
                let mut cursor = child.walk();
                if child.children(&mut cursor).any(|part| part.kind() == "*") {
                    continue;
                }
                let parts = children(child);
                let Some(path) = parts.first() else {
                    continue;
                };
                let spec = squeezed(*path, src);
                let name = match parts.get(1) {
                    Some(alias) if alias.kind() == "identifier" => text(*alias, src).to_owned(),
                    _ => spec.rsplit('.').next().unwrap_or(&spec).to_owned(),
                };
                found.insert(name, spec);
            }
            _ => {}
        }
    }
    let mut declared: BTreeSet<String> = BTreeSet::new();
    walk(root, |node| {
        if matches!(node.kind(), "class_declaration" | "object_declaration") {
            if let Some(name) = node.child_by_field_name("name") {
                declared.insert(text(name, src).to_owned());
            }
        }
        !matches!(node.kind(), "string_literal" | "multiline_string_literal")
    });
    same_package(root, src, &package, &declared, &mut found, &["identifier"]);
    found
}

/// True for a path the guard reads as a Kotlin test file: in a test
/// folder (`src/test/`, `src/androidTest/`, `src/jvmTest/`,
/// `src/commonTest/`, …), or named `*Test.kt`, `*Tests.kt`, `*Spec.kt`,
/// `*IT.kt` or `Test*.kt`.
pub(super) fn is_test_path(path: &str) -> bool {
    let Some((stem, folders)) = split_path(path, "kt") else {
        return false;
    };
    ["Test", "Tests", "Spec", "IT"]
        .iter()
        .any(|end| stem.ends_with(end))
        || starts_with_test(stem)
        || in_test_folder(&folders)
}

/// A cheap check that Kotlin source may hold tests, before it is parsed: a
/// test annotation, or a Kotest spec it reads.
pub(super) fn holds_tests(source: &str) -> bool {
    [
        "@Test",
        "@ParameterizedTest",
        "@RepeatedTest",
        "@org.junit",
        "@kotlin.test",
    ]
    .iter()
    .chain(SPECS.iter())
    .any(|needle| source.contains(needle))
}

/// The keys of a suppressing annotation: `@Suppress` and
/// `@SuppressWarnings` (`suppress:rule`), `@SuppressLint`
/// (`android-lint:rule`), one per rule it names, `*` for a value that is
/// not a string.
fn annotation_keys(annotation: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut keys = Vec::new();
    for (name, arguments) in annotation_parts(annotation, src) {
        let tool = match name.as_str() {
            "Suppress" | "SuppressWarnings" => "suppress",
            "SuppressLint" => "android-lint",
            _ => continue,
        };
        let mut rules: Vec<String> = Vec::new();
        let mut other = false;
        for value_argument in arguments {
            let Some(value) = argument(value_argument, src).1 else {
                continue;
            };
            let items = match value.kind() {
                "collection_literal" => children(value),
                "call_expression" => value_arguments(value)
                    .into_iter()
                    .filter_map(|inner| argument(inner, src).1)
                    .collect(),
                _ => vec![value],
            };
            for item in items {
                if item.kind() == "string_literal" {
                    rules.push(string_text(item, src));
                } else {
                    other = true;
                }
            }
        }
        if !rules.is_empty() || other {
            keys.extend(keyed(tool, rules));
        }
    }
    keys
}

/// The keys of a suppressing comment: ktlint's `ktlint-disable`, with the
/// rules it names or every rule. The directive must open the comment.
fn comment_keys(raw: &str) -> Vec<String> {
    directive(comment_body(raw), "ktlint-disable")
        .map(|rest| keyed("ktlint", rest.split_whitespace().map(str::to_owned)))
        .unwrap_or_default()
}

/// The lint suppressions in a Kotlin source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let language: Language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| {
        let keys = match node.kind() {
            "line_comment" | "block_comment" => comment_keys(text(node, src)),
            "annotation" | "file_annotation" => annotation_keys(node, src),
            "string_literal" | "multiline_string_literal" => return false,
            _ => return true,
        };
        for key in keys {
            found.push(Suppression {
                key,
                line: node.start_position().row + 1,
                text: shown(text(node, src)),
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
    fn junit_tests_are_read_with_their_assertions() {
        let file = read(
            r#"package com.acme

import kotlin.test.Test
import kotlin.test.assertEquals

class CartTest {
    private val cart = Cart()

    @Test
    fun totals() {
        cart.add(2)
        assertEquals(2, cart.total())
        kotlin.test.assertTrue(cart.total() > 0, "positive")
    }

    private fun helper() = 1
}
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(ids(&file), [("CartTest::totals", false, None, 1)]);
        assert_eq!(file.tests[0].line, 10);
        assert!(file.tests[0].uses.contains("Cart"));
        assert_eq!(
            shapes(&file),
            [
                "assertEquals(2,cart.total())",
                "kotlin.test.assertTrue(cart.total()>0,\"positive\")"
            ]
        );
        assert_eq!(file.assertions[0].line, 12);
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(
            file.assertions[1].text,
            "kotlin.test.assertTrue(cart.total() > 0, \"positive\")"
        );
    }

    #[test]
    fn every_test_annotation_is_a_test_and_ids_name_classes() {
        let file = read(
            r#"
class T {
    @Test fun plain() { check() }
    @org.junit.Test fun junit4() { check() }
    @Test(expected = IllegalStateException::class) fun throwing() { boom() }
    @Test fun `reads a backticked name`() = runTest { check() }
    @RepeatedTest(3) fun repeated() { check() }
    @TestFactory fun factory() = listOf(dynamicTest("a") { check() })
    fun notATest() { check() }
    @BeforeEach fun setUp() { check() }
    @Nested inner class WhenEmpty {
        @Test fun zero() { check() }
    }
    companion object {
        @Test fun hidden() { check() }
    }
}
object Suite { @Test fun inObject() { check() } }
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::plain", false, None, 1),
                ("T::junit4", false, None, 1),
                ("T::throwing", false, None, 1),
                ("T::`reads a backticked name`", false, None, 1),
                ("T::repeated", false, None, 1),
                ("T::factory", false, None, 1),
                ("T::WhenEmpty::zero", false, None, 1),
                ("Suite::inObject", false, None, 1),
            ]
        );
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r#"
class T {
    // @Test fun commented() { assertTrue(x) }
    val s = "@Test fun inString() { assertTrue(x) }"
    val t = """
        "name" { 1 shouldBe 2 }
    """
    @Testing fun lookAlike() { }
    fun local() {
        class L {
            @Test fun inner() { }
        }
    }
    val o = object {
        @Test fun anon() { }
    }
}
@Test fun topLevel() { }
"#,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn ignored_disabled_and_conditional_tests_are_skipped() {
        let file = read(
            r#"
class T {
    @Test @Ignore fun ignored() { check() }
    @Test @Disabled("flaky") fun disabled() { check() }
    @Test @DisabledOnOs(OS.WINDOWS) fun notOnWindows() { check() }
    @Test @EnabledIfEnvironmentVariable(named = "CI", matches = "true") fun onCi() { check() }
    @Test(enabled = false) fun testng() { check() }
    @Test fun runs() { check() }
}
@Ignore
class Off { @Test fun inside() { check() } }
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::ignored", true, None, 1),
                ("T::disabled", true, None, 1),
                ("T::notOnWindows", true, None, 1),
                ("T::onCi", true, None, 1),
                ("T::testng", true, None, 1),
                ("T::runs", false, None, 1),
                ("Off::inside", true, None, 1),
            ]
        );
    }

    #[test]
    fn a_body_that_does_nothing_or_assumes_false_first_is_a_skip() {
        let file = read(
            r#"
class T {
    @Test fun empty() {}
    @Test fun unit() = Unit
    @Test fun returns() { return }
    @Test fun assumes() { Assumptions.assumeTrue(false); assertEquals(1, x) }
    @Test fun assumesNot() { assumeFalse(true, "off") }
    @Test fun sometimes() { assumeTrue(onCi()); check() }
    @Test fun calls() { run() }
}
"#,
        );
        let nothing = Some(DOES_NOTHING);
        let first = Some(SKIPS_FIRST);
        assert_eq!(
            ids(&file),
            [
                ("T::empty", true, nothing, 1),
                ("T::unit", true, nothing, 1),
                ("T::returns", true, nothing, 1),
                ("T::assumes", true, first, 1),
                ("T::assumesNot", true, first, 1),
                ("T::sometimes", false, None, 1),
                ("T::calls", false, None, 1),
            ]
        );
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn parameterized_tests_weigh_their_written_rows() {
        let file = read(
            r#"
class T {
    @ParameterizedTest
    @ValueSource(ints = [1, 2, 3])
    fun ints(n: Int) { assertTrue(n > 0) }

    @ParameterizedTest
    @ValueSource(strings = arrayOf("a", "b"))
    fun strings(s: String) { check() }

    @ParameterizedTest
    @CsvSource("a, 1", "b, 2")
    fun csv(s: String, n: Int) { check() }

    @ParameterizedTest
    @CsvSource(value = ["a, 1", "b, 2", "c, 3"], delimiter = ',')
    fun csvValue(s: String, n: Int) { check() }

    @ParameterizedTest
    @CsvSource(textBlock = """
        a, 1
        # skipped
        b, 2
    """)
    fun block(s: String, n: Int) { check() }

    @ParameterizedTest
    @MethodSource("cases")
    fun provided(n: Int) { check() }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::ints", false, None, 3),
                ("T::strings", false, None, 2),
                ("T::csv", false, None, 2),
                ("T::csvValue", false, None, 3),
                ("T::block", false, None, 2),
                ("T::provided", false, None, 1),
            ]
        );
        assert_eq!(file.assertions[0].weight, 3);
    }

    #[test]
    fn kotest_specs_are_read() {
        let file = read(
            r#"
class CartSpec : FunSpec({
    val cart = Cart()
    test("totals") { cart.total() shouldBe 0 }
    context("when full") {
        test("rejects") { shouldThrow<IllegalStateException> { cart.add(1) } }
        xtest("later") { cart.total() shouldBe 3 }
        test("configured").config(enabled = false) { cart.total() shouldBe 1 }
        test("on ci").config(enabledIf = { onCi() }) { cart.total() shouldBe 1 }
        test("empty") { }
    }
    xcontext("off") { test("inside") { cart.total() shouldBe 2 } }
})

class NameSpec : StringSpec() {
    init {
        "length" { "abc".length shouldBe 3 }
        "!bang" { "abc".length shouldBe 4 }
        "f:focus" { "abc".length shouldBe 5 }
        "configured".config(enabled = false) { "a".length shouldBe 1 }
    }
}

object TreeSpec : DescribeSpec({
    describe("a tree") {
        it("grows") { tree.height() shouldBeGreaterThan 0 }
        xit("falls") { tree.fall() }
        context("in winter") { it("sleeps") { tree.awake() shouldBe false } }
    }
    xdescribe("off") { it("hidden") { tree.x() shouldBe 1 } }
})

class ShouldSpecTest : ShouldSpec({
    should("work") { work() shouldBe true }
    context("group") { should("nest") { nest() shouldBe true } }
})

class FreeSpecTest : FreeSpec({
    "outer" - {
        "inner" { inner() shouldBe 1 }
    }
})

class NotASpec : Other({ test("no") { x shouldBe 1 } })
"#,
        );
        let nothing = Some(DOES_NOTHING);
        assert_eq!(
            ids(&file),
            [
                ("CartSpec::totals", false, None, 1),
                ("CartSpec::when full::rejects", false, None, 1),
                ("CartSpec::when full::later", true, None, 1),
                ("CartSpec::when full::configured", true, None, 1),
                ("CartSpec::when full::on ci", true, None, 1),
                ("CartSpec::when full::empty", true, nothing, 1),
                ("CartSpec::off::inside", true, None, 1),
                ("NameSpec::length", false, None, 1),
                ("NameSpec::bang", true, None, 1),
                ("NameSpec::focus", false, None, 1),
                ("NameSpec::configured", true, None, 1),
                ("TreeSpec::a tree::grows", false, None, 1),
                ("TreeSpec::a tree::falls", true, None, 1),
                ("TreeSpec::a tree::in winter::sleeps", false, None, 1),
                ("TreeSpec::off::hidden", true, None, 1),
                ("ShouldSpecTest::work", false, None, 1),
                ("ShouldSpecTest::group::nest", false, None, 1),
                ("FreeSpecTest::outer::inner", false, None, 1),
            ]
        );
        let focused: Vec<&str> = file
            .tests
            .iter()
            .filter(|t| t.focused)
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(focused, ["NameSpec::focus"]);
        assert!(file.tests[0].uses.contains("Cart"));
        // Each assertion belongs to its test; the one in NotASpec to none.
        let tests: Vec<Option<usize>> = file.assertions.iter().map(|a| a.test).collect();
        assert_eq!(
            tests,
            [
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(6),
                Some(7),
                Some(8),
                Some(9),
                Some(10),
                Some(11),
                Some(13),
                Some(14),
                Some(15),
                Some(16),
                Some(17),
                None
            ]
        );
    }

    #[test]
    fn every_assertion_form_counts_once() {
        let file = read(
            r#"
class T {
    @Test fun t() {
        assertEquals(3, total())
        kotlin.test.assertNotNull(cart)
        assertFailsWith<IllegalStateException> { cart.close() }
        val e = shouldThrow<IllegalStateException> { cart.close() }
        total() shouldBe 3
        cart.items shouldContain item
        total().shouldBe(3)
        assertThat(total()).isEqualTo(3).isPositive()
        verify { repository.save(cart) }
        coVerify(exactly = 1) { repository.load() }
        confirmVerified(repository)
        fail("must not get here")
        shouldThrowAny { cart.close() }
        kotlin.test.fail("unreachable")
        every { repository.load() } returns cart
        signature.verify(data)
        promise.fail(error)
        println(total())
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assertEquals(3,total())",
                "kotlin.test.assertNotNull(cart)",
                "assertFailsWith<IllegalStateException>{cart.close()}",
                "shouldThrow<IllegalStateException>{cart.close()}",
                "total()shouldBe3",
                "cart.itemsshouldContainitem",
                "total().shouldBe(3)",
                "assertThat(total()).isEqualTo(3).isPositive()",
                "verify{repository.save(cart)}",
                "coVerify(exactly=1){repository.load()}",
                "confirmVerified(repository)",
                "fail(\"mustnotgethere\")",
                "shouldThrowAny{cart.close()}",
                "kotlin.test.fail(\"unreachable\")",
            ]
        );
        assert!(file.assertions.iter().all(|a| a.test == Some(0)));
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"
class T {
    @Test fun t() {
        assertTrue(true)
        assertTrue(true, "placeholder")
        assertFalse(false)
        assertEquals(1, 1)
        assertEquals(1, 1, "same")
        1 shouldBe 1
        true shouldBe true
        (null) shouldBe null
        "a".shouldBe("a")
        assertThat(1).isEqualTo(1)
        assertEquals(1, 2)
        assertTrue(false)
        1 shouldBe 2
        x shouldBe x
        1 shouldNotBe 1
        assertEquals(1, total())
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assertEquals(1,2)",
                "assertTrue(false)",
                "1shouldBe2",
                "xshouldBex",
                "1shouldNotBe1",
                "assertEquals(1,total())",
            ]
        );
    }

    #[test]
    fn assertions_that_never_run_are_not_counted() {
        let file = read(
            r#"
class T {
    @Test fun insideIfFalse() {
        if (false) {
            assertEquals(3, total())
        }
        if (false) a shouldBe 1 else b shouldBe 2
        while (false) { c shouldBe 3 }
        d shouldBe 4
    }
    @Test fun afterReturn() {
        e shouldBe 5
        return
        f shouldBe 6
    }
}
class S : FunSpec({
    test("labelled return") {
        g shouldBe 7
        return@test
        h shouldBe 8
    }
})
"#,
        );
        assert_eq!(
            shapes(&file),
            ["bshouldBe2", "dshouldBe4", "eshouldBe5", "gshouldBe7"]
        );
        assert!(!file.tests[0].skipped);
    }

    #[test]
    fn imports_name_their_classes_and_the_package_its_own() {
        let file = read(
            r#"package com.acme.shop

import com.acme.pricing.Prices
import com.acme.pricing.Taxes as Vat
import io.kotest.matchers.*

class CartTest : FunSpec({
    test("totals") { Cart(Prices.of(Vat.rate())).total() shouldBe 3 }
})
"#,
        );
        let imports: Vec<(&str, &str)> = file
            .imports
            .iter()
            .map(|(name, spec)| (name.as_str(), spec.as_str()))
            .collect();
        assert_eq!(
            imports,
            [
                ("Cart", "com.acme.shop.Cart"),
                ("FunSpec", "com.acme.shop.FunSpec"),
                ("Prices", "com.acme.pricing.Prices"),
                ("Vat", "com.acme.pricing.Taxes"),
            ]
        );
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read("class T {\n    @Test fun t() {\n        assertEquals(1, x)\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let source = format!(
            "class T {{ @Test fun t() {{ {} x shouldBe 1 {} }} }}\n",
            "run { ".repeat(500),
            "} ".repeat(500)
        );
        let file = read(&source);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
    }

    #[test]
    fn test_paths() {
        for path in [
            "src/test/kotlin/com/acme/Cart.kt",
            "app/src/androidTest/java/com/acme/CartUi.kt",
            "shared/src/commonTest/kotlin/Cart.kt",
            "shared/src/jvmTest/kotlin/Cart.kt",
            "CartTest.kt",
            "lib/CartTests.kt",
            "lib/CartSpec.kt",
            "lib/CartIT.kt",
            "lib/TestCart.kt",
            "lib/CartTest.KT",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "src/main/kotlin/com/acme/Cart.kt",
            "shared/src/commonMain/kotlin/Cart.kt",
            "lib/Testimonial.kt",
            "lib/Inspect.kt",
            "src/test/kotlin/CartTest.java",
            "build.gradle.kts",
            ".kt",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_looks_for_tests_and_specs() {
        for source in [
            "@Test fun t() {}",
            "@ParameterizedTest",
            "@kotlin.test.Test",
            "class S : FunSpec({})",
            "class S : StringSpec()",
        ] {
            assert!(holds_tests(source), "{source}");
        }
        assert!(!holds_tests("class Cart { fun total() = 0 }"));
    }

    #[test]
    fn suppress_annotations_have_a_key_per_rule() {
        assert_eq!(
            keys(
                r#"@file:Suppress("UNUSED_PARAMETER", "ktlint:standard:filename")
package com.acme

@Suppress("MagicNumber")
class A {
    @Suppress(names = ["a", "b"]) fun a() {}
    @SuppressWarnings("unchecked") fun b() {}
    @SuppressLint("NewApi") fun c() {}
    @Suppress(ALL) fun d() {}
    @JvmStatic fun e() {}
}
"#
            ),
            [
                "suppress:UNUSED_PARAMETER",
                "suppress:ktlint:standard:filename",
                "suppress:MagicNumber",
                "suppress:a",
                "suppress:b",
                "suppress:unchecked",
                "android-lint:NewApi",
                "suppress:*",
            ]
        );
    }

    #[test]
    fn ktlint_comments_are_read_from_comments_only() {
        let found = suppressions(
            r#"
import a.* // ktlint-disable no-wildcard-imports
/* ktlint-disable */
// ktlint-disable max-line-length indent
// we used to need ktlint-disable here
// ktlint-enable
val s = "// ktlint-disable"
val t = """
    @Suppress("x")
"""
"#,
        );
        let keys: Vec<&str> = found.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "ktlint:no-wildcard-imports",
                "ktlint:*",
                "ktlint:max-line-length",
                "ktlint:indent",
            ]
        );
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].text, "// ktlint-disable no-wildcard-imports");
    }
}
