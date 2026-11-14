//! The Java reader of the tamper guard (M32, ADR 0052): JUnit 4 and 5,
//! TestNG and JUnit 3.
//!
//! - A test is a method annotated `@Test` (JUnit 4, JUnit 5, TestNG),
//!   `@ParameterizedTest`, `@RepeatedTest`, `@TestFactory` or
//!   `@TestTemplate`, or a `test*` method of a class that extends
//!   `…TestCase` (JUnit 3), declared in a class that is not anonymous or
//!   local. Its id is the names of the classes around it (`@Nested` ones
//!   included) and its own, joined with `::` (`CartTest::Empty::totals`).
//! - It is skipped with `@Disabled` or `@Ignore` on it or a class around
//!   it, with a JUnit 5 condition (`@DisabledOnOs`, `@EnabledIf…`: it runs
//!   only sometimes, like Rust's `cfg_attr(…, ignore)`), with TestNG's
//!   `@Test(enabled = false)`, when its first statement skips it
//!   (`assumeTrue(false)`, `throw new SkipException(…)`), or when its body
//!   does nothing.
//! - A `@ParameterizedTest` weighs the rows of its `@ValueSource`,
//!   `@CsvSource` (an array literal or a text block), `@NullSource`,
//!   `@EmptySource` and `@NullAndEmptySource`; any other test weighs 1.
//! - An assertion is a call named `assert…` (JUnit, TestNG, AssertJ
//!   `assertThat(…)…` chains and Hamcrest, qualified or not), `fail(…)`,
//!   Mockito's `verify(…)…`, or an `assert` statement; not one that never
//!   runs (inside `if (false)`, after a skip) or cannot fail
//!   (`assertTrue(true)`, `assertEquals(1, 1)`, `assertThat(1).isEqualTo(1)`).
//! - Imports (`import com.acme.Cart;`) and the classes of the file's own
//!   package it names without importing them are its subjects, found in
//!   the usual source folders by [`resolve`].
//! - Suppressions: `@SuppressWarnings`, SpotBugs' `@SuppressFBWarnings`,
//!   `// NOPMD`, `// NOSONAR`, `// CHECKSTYLE:OFF` and `CHECKSTYLE.OFF:
//!   Rule`, `// SUPPRESS CHECKSTYLE Rule`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::suppress::{directive, keyed, shown};
use super::{Assertion, Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// What the Java grammar calls the nodes [`reach`] looks at.
const GRAMMAR: Grammar = Grammar {
    blocks: &["block"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &["while_statement"],
    hoisted: &[
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "record_declaration",
        "method_declaration",
    ],
    stops: Some(stops),
    noop: never_noop,
};

/// The annotations that make a method a test.
const TESTS: [&str; 5] = [
    "Test",
    "ParameterizedTest",
    "RepeatedTest",
    "TestFactory",
    "TestTemplate",
];

/// The annotations that turn a test, or every test of a class, off.
pub(super) const SKIPS: [&str; 3] = ["Disabled", "Ignore", "Ignored"];

/// The starts of JUnit 5's conditions: a test under one runs only
/// sometimes (`@DisabledOnOs`, `@EnabledIfEnvironmentVariable`).
pub(super) const CONDITIONS: [&str; 8] = [
    "DisabledOn",
    "DisabledIf",
    "DisabledFor",
    "DisabledIn",
    "EnabledOn",
    "EnabledIf",
    "EnabledFor",
    "EnabledIn",
];

/// Mockito's verifications: assertions on how a mock was called.
const VERIFIES: [&str; 4] = [
    "verify",
    "verifyNoMoreInteractions",
    "verifyNoInteractions",
    "verifyZeroInteractions",
];

/// The exceptions whose `throw` skips a test (TestNG, JUnit 5, JUnit 4).
const SKIP_EXCEPTIONS: [&str; 3] = [
    "SkipException",
    "TestAbortedException",
    "AssumptionViolatedException",
];

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 18] = [
    "decimal_integer_literal",
    "hex_integer_literal",
    "octal_integer_literal",
    "binary_integer_literal",
    "decimal_floating_point_literal",
    "hex_floating_point_literal",
    "true",
    "false",
    "null_literal",
    "character_literal",
    "string_literal",
    "string_fragment",
    "multiline_string_fragment",
    "escape_sequence",
    "unary_expression",
    "binary_expression",
    "parenthesized_expression",
    "class_literal",
];

/// The declarations whose name is part of a test's id.
const CLASSES: [&str; 4] = [
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
];

/// Why a test whose first statement skips it does not run.
pub(super) const SKIPS_FIRST: &str = "its first statement skips it";
/// Why a test whose body does nothing does not run.
pub(super) const DOES_NOTHING: &str = "its body does nothing";

/// No Java statement does nothing: an empty `;` is not a named node.
fn never_noop(_statement: Node<'_>, _src: &[u8]) -> bool {
    false
}

/// The named children of a node that are not comments.
pub(super) fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .collect()
}

/// tree-sitter found syntax errors, and the line of the first one.
pub(super) fn syntax_errors(root: Node<'_>) -> (bool, Option<usize>) {
    if !root.has_error() {
        return (false, None);
    }
    let mut line = None;
    walk(root, |node| {
        if line.is_none() && (node.is_error() || node.is_missing()) {
            line = Some(node.start_position().row + 1);
        }
        line.is_none() && node.has_error()
    });
    (true, line)
}

/// A node's text with white space removed: what is compared.
pub(super) fn squeezed(node: Node<'_>, src: &[u8]) -> String {
    text(node, src).split_whitespace().collect()
}

/// An assertion found at `node`, not yet given its test.
pub(super) fn assertion(node: Node<'_>, src: &[u8]) -> Assertion {
    let whole = text(node, src);
    Assertion {
        line: node.start_position().row + 1,
        shape: whole.split_whitespace().collect(),
        text: whole.split_whitespace().collect::<Vec<_>>().join(" "),
        weight: 1,
        test: None,
    }
}

/// Gives each assertion the test whose node holds it, and that test's
/// weight. `ranges` are the byte ranges of the tests, by index; tests do
/// not nest.
pub(super) fn assign(file: &mut TestFile, ranges: &[(usize, usize)], starts: &[usize]) {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by_key(|index| ranges[*index].0);
    for (assertion, start) in file.assertions.iter_mut().zip(starts) {
        let at = order.partition_point(|index| ranges[*index].0 <= *start);
        let test = at
            .checked_sub(1)
            .map(|position| order[position])
            .filter(|index| *start < ranges[*index].1);
        assertion.test = test;
        assertion.weight = test
            .and_then(|index| file.tests.get(index))
            .map_or(1, |test| test.weight);
    }
}

/// One argument of an assertion, when it is a literal: its text without
/// white space or outer parentheses, and whether it is a string.
pub(super) struct Literal {
    pub(super) text: String,
    pub(super) string: bool,
}

impl Literal {
    /// A literal from its squeezed text, outer parentheses dropped.
    pub(super) fn new(raw: String, string: bool) -> Self {
        let mut text = raw.as_str();
        while text.len() >= 2 && text.starts_with('(') && text.ends_with(')') {
            text = &text[1..text.len() - 1];
        }
        Self {
            text: text.to_owned(),
            string,
        }
    }
}

/// True for a JUnit or kotlin.test assertion given only literals that
/// certainly pass: `assertTrue(true)`, `assertFalse(false, "msg")`,
/// `assertEquals(1, 1)`, `assertEquals("msg", 2, 2)` (JUnit 4's message
/// first), `assertEquals(2, 2, "msg")` (JUnit 5's message last). When a
/// string could be the message or a value (`assertEquals("a", "a", "b")`),
/// it may fail, so it counts.
pub(super) fn passes(name: &str, arguments: &[Literal]) -> bool {
    match name {
        "assertTrue" => arguments.iter().any(|argument| argument.text == "true"),
        "assertFalse" => arguments.iter().any(|argument| argument.text == "false"),
        "assertEquals" | "assertSame" => match arguments {
            [a, b] => a.text == b.text,
            [a, b, c] => {
                (a.text == b.text && !(a.string && c.string))
                    || (b.text == c.text && a.string && !b.string)
            }
            _ => false,
        },
        _ => false,
    }
}

/// True for a call that skips the test it is in: `assumeTrue(false)`,
/// `assumeFalse(true)` (JUnit 4 and 5, with a message or not), and
/// `abort()`, by the call's name and its literal arguments.
pub(super) fn assumes_false(name: &str, arguments: &[Option<Literal>]) -> bool {
    let all: Option<Vec<&Literal>> = arguments.iter().map(Option::as_ref).collect();
    let Some(all) = all else {
        return false;
    };
    match name {
        "assumeTrue" => all.iter().any(|argument| argument.text == "false"),
        "assumeFalse" => all.iter().any(|argument| argument.text == "true"),
        "abort" => true,
        _ => false,
    }
}

/// True for a statement after which the rest of its block does not run:
/// `assumeTrue(false)`, `Assumptions.abort()`, `throw new
/// SkipException(…)`.
fn stops(statement: Node<'_>, src: &[u8]) -> bool {
    match statement.kind() {
        "expression_statement" => statement
            .named_child(0)
            .filter(|call| call.kind() == "method_invocation")
            .is_some_and(|call| {
                let name = call
                    .child_by_field_name("name")
                    .map_or("", |name| text(name, src));
                let arguments: Vec<Option<Literal>> = call
                    .child_by_field_name("arguments")
                    .map(children)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|argument| literal(argument, src))
                    .collect();
                assumes_false(name, &arguments)
            }),
        "throw_statement" => statement
            .named_child(0)
            .filter(|thrown| thrown.kind() == "object_creation_expression")
            .and_then(|thrown| thrown.child_by_field_name("type"))
            .is_some_and(|kind| {
                let name = text(kind, src);
                let last = name.rsplit('.').next().unwrap_or(name);
                SKIP_EXCEPTIONS.contains(&last)
            }),
        _ => false,
    }
}

/// The argument as a [`Literal`], or `None` when it is not one.
fn literal(node: Node<'_>, src: &[u8]) -> Option<Literal> {
    reach::constant(node, &LITERALS).then(|| {
        Literal::new(
            squeezed(node, src),
            node.kind() == "string_literal" && !text(node, src).starts_with("\"\"\""),
        )
    })
}

/// The annotations written on a declaration, in source order.
fn annotations(declaration: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = declaration.walk();
    let Some(modifiers) = declaration
        .named_children(&mut cursor)
        .find(|child| child.kind() == "modifiers")
    else {
        return Vec::new();
    };
    children(modifiers)
        .into_iter()
        .filter(|child| matches!(child.kind(), "annotation" | "marker_annotation"))
        .collect()
}

/// The last segment of an annotation's name: `Test` for
/// `@org.junit.jupiter.api.Test`.
fn annotation_name<'s>(annotation: Node<'_>, src: &'s [u8]) -> &'s str {
    let Some(name) = annotation.child_by_field_name("name") else {
        return "";
    };
    if name.kind() == "scoped_identifier" {
        name.child_by_field_name("name")
            .map_or("", |last| text(last, src))
    } else {
        text(name, src)
    }
}

/// The arguments of an annotation: `(key, value)` for `key = value`, and
/// `(None, value)` for one written alone.
fn annotation_arguments<'t>(annotation: Node<'t>, src: &[u8]) -> Vec<(Option<String>, Node<'t>)> {
    let Some(list) = annotation.child_by_field_name("arguments") else {
        return Vec::new();
    };
    children(list)
        .into_iter()
        .filter_map(|argument| {
            if argument.kind() == "element_value_pair" {
                let key = argument
                    .child_by_field_name("key")
                    .map(|key| text(key, src).to_owned());
                argument
                    .child_by_field_name("value")
                    .map(|value| (key, value))
            } else {
                Some((None, argument))
            }
        })
        .collect()
}

/// How many values an annotation's value holds: the elements of an array
/// literal, else 1.
fn values(value: Node<'_>) -> usize {
    if value.kind() == "element_value_array_initializer" {
        children(value).len()
    } else {
        1
    }
}

/// The rows of a CSV text block: its lines that are not blank or a `#`
/// comment.
pub(super) fn text_block_rows(raw: &str) -> usize {
    let inner = raw.trim().trim_matches('"');
    inner
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .count()
}

/// What the annotations of a method or class say about it.
#[derive(Default)]
struct Marks {
    /// A test annotation.
    test: bool,
    /// `@ParameterizedTest`.
    parameterized: bool,
    /// `@Disabled`, `@Ignore`, a condition, `@Test(enabled = false)`.
    skipped: bool,
    /// The rows of its argument sources that are written out.
    rows: usize,
}

/// Reads the annotations of a method or class.
fn marks(declaration: Node<'_>, src: &[u8]) -> Marks {
    let mut found = Marks::default();
    for annotation in annotations(declaration) {
        let name = annotation_name(annotation, src);
        if TESTS.contains(&name) {
            found.test = true;
        }
        if SKIPS.contains(&name) || CONDITIONS.iter().any(|start| name.starts_with(start)) {
            found.skipped = true;
        }
        let arguments = annotation_arguments(annotation, src);
        match name {
            "ParameterizedTest" => found.parameterized = true,
            // TestNG: `@Test(enabled = false)`; anything but `true` may
            // turn it off.
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
                        Some("textBlock") => found.rows += text_block_rows(text(*value, src)),
                        Some(_) => {}
                    }
                }
            }
            "NullSource" | "EmptySource" => found.rows += 1,
            "NullAndEmptySource" => found.rows += 2,
            _ => {}
        }
    }
    found
}

/// The name of a method call (`assertEquals` in `Assert.assertEquals(…)`).
fn call_name<'s>(call: Node<'_>, src: &'s [u8]) -> &'s str {
    call.child_by_field_name("name")
        .map_or("", |name| text(name, src))
}

/// True for what `fail(…)` or `verify(…)` is called on when it is the
/// assertion library's: nothing (a static import), `Assertions`, `Assert`,
/// `Mockito`, `kotlin.test`. `signature.verify(data)` or `promise.fail(e)`
/// is not an assertion.
pub(super) fn library_receiver(receiver: Option<&str>) -> bool {
    receiver.is_none_or(|receiver| {
        let last = receiver.rsplit('.').next().unwrap_or(receiver);
        last.contains("Assert") || matches!(last, "Mockito" | "BDDMockito" | "test")
    })
}

/// True for a call that asserts: one named `assert…`, or `fail(…)` and
/// Mockito's `verify(…)` called on their library.
fn asserts(call: Node<'_>, src: &[u8]) -> bool {
    let name = call_name(call, src);
    if name.starts_with("assert") {
        return true;
    }
    (name == "fail" || VERIFIES.contains(&name))
        && library_receiver(
            call.child_by_field_name("object")
                .map(|object| text(object, src)),
        )
}

/// The calls of a chain, outermost first: `assertThat(x).isEqualTo(3)` is
/// `isEqualTo(…)` then `assertThat(…)`.
fn chain(call: Node<'_>) -> Vec<Node<'_>> {
    let mut links = vec![call];
    let mut at = call;
    while let Some(object) = at
        .child_by_field_name("object")
        .filter(|object| object.kind() == "method_invocation")
    {
        links.push(object);
        at = object;
    }
    links
}

/// The arguments of a call, each as a [`Literal`] when it is one.
fn call_arguments(call: Node<'_>, src: &[u8]) -> Vec<Option<Literal>> {
    call.child_by_field_name("arguments")
        .map(children)
        .unwrap_or_default()
        .into_iter()
        .map(|argument| literal(argument, src))
        .collect()
}

/// True for an assertion that cannot fail: a JUnit one given literals that
/// pass ([`passes`]), or AssertJ's `assertThat(1).isEqualTo(1)`,
/// `assertThat(true).isTrue()`.
fn cannot_fail(links: &[Node<'_>], src: &[u8]) -> bool {
    let all = |call: Node<'_>| -> Option<Vec<Literal>> {
        call_arguments(call, src).into_iter().collect()
    };
    match links {
        [call] => all(*call).is_some_and(|arguments| passes(call_name(*call, src), &arguments)),
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

/// True for an `assert` statement that cannot fail: `assert true;`.
fn statement_cannot_fail(statement: Node<'_>, src: &[u8]) -> bool {
    statement
        .named_child(0)
        .and_then(|condition| literal(condition, src))
        .is_some_and(|condition| condition.text == "true")
}

/// The identifiers and type names under a node.
fn identifiers(node: Node<'_>, src: &[u8], into: &mut BTreeSet<String>) {
    walk(node, |inner| {
        if matches!(inner.kind(), "identifier" | "type_identifier") {
            into.insert(text(inner, src).to_owned());
        }
        true
    });
}

/// The identifiers the fields of a class body use: tests use them too.
fn field_identifiers(body: Option<Node<'_>>, src: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for field in body
        .map(children)
        .unwrap_or_default()
        .into_iter()
        .filter(|child| child.kind() == "field_declaration")
    {
        identifiers(field, src, &mut found);
    }
    found
}

/// True for a class that extends JUnit 3's `TestCase` (or a class of the
/// project named `…TestCase`).
fn extends_test_case(class: Node<'_>, src: &[u8]) -> bool {
    class
        .child_by_field_name("superclass")
        .is_some_and(|superclass| {
            text(superclass, src)
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .rfind(|part| !part.is_empty())
                .is_some_and(|name| name.ends_with("TestCase"))
        })
}

/// True for a method a test runner can find: declared in the body of a
/// named class, interface, enum or record (not an anonymous class).
fn in_declared_class(method: Node<'_>) -> bool {
    let Some(body) = method.parent() else {
        return false;
    };
    let owner = match body.kind() {
        "class_body" | "interface_body" => body.parent(),
        "enum_body_declarations" => body.parent().and_then(|enum_body| enum_body.parent()),
        _ => None,
    };
    owner.is_some_and(|owner| CLASSES.contains(&owner.kind()))
}

/// Why a test that is not marked skipped does not run, if it does not.
fn not_run(method: Node<'_>, src: &[u8]) -> Option<&'static str> {
    let body = method.child_by_field_name("body")?;
    let first = children(body).first().copied();
    // A `@TestFactory` returns the tests it makes.
    let returns_value = first
        .is_some_and(|first| first.kind() == "return_statement" && first.named_child(0).is_some());
    if !returns_value && reach::does_nothing(body, src, &GRAMMAR) {
        return Some(DOES_NOTHING);
    }
    first
        .is_some_and(|first| stops(first, src))
        .then_some(SKIPS_FIRST)
}

/// A class or method open around the node being visited.
enum Scope {
    Class {
        name: String,
        /// It or a class around it is turned off.
        skipped: bool,
        /// It extends `…TestCase`: its `test*` methods are tests.
        junit3: bool,
        /// The identifiers its fields use.
        fields: BTreeSet<String>,
    },
    Method,
}

/// What a method needs to know of the classes around it.
struct Around<'a> {
    names: Vec<&'a str>,
    skipped: bool,
    junit3: bool,
    fields: Vec<&'a BTreeSet<String>>,
}

impl<'a> Around<'a> {
    /// What the classes open around a method say.
    fn of(open: &'a [(usize, Scope)]) -> Self {
        let mut around = Self {
            names: Vec::new(),
            skipped: false,
            junit3: false,
            fields: Vec::new(),
        };
        for (_, scope) in open {
            if let Scope::Class {
                name,
                skipped,
                junit3,
                fields,
            } = scope
            {
                around.names.push(name);
                around.skipped |= *skipped;
                around.junit3 = *junit3;
                around.fields.push(fields);
            }
        }
        around
    }
}

/// The test a method is, if it is one.
fn test_case(method: Node<'_>, src: &[u8], around: &Around<'_>) -> Option<TestCase> {
    let own = marks(method, src);
    let name_node = method.child_by_field_name("name");
    let name = name_node.map_or("", |name| text(name, src));
    let junit3 = around.junit3 && name.starts_with("test") && !own.test;
    if !own.test && !junit3 {
        return None;
    }
    let skipped = own.skipped || around.skipped;
    let why = if skipped { None } else { not_run(method, src) };
    let mut uses: BTreeSet<String> = around.fields.iter().copied().flatten().cloned().collect();
    identifiers(method, src, &mut uses);
    let id = around
        .names
        .iter()
        .copied()
        .chain(std::iter::once(name))
        .collect::<Vec<_>>()
        .join("::");
    Some(TestCase {
        id,
        line: name_node.unwrap_or(method).start_position().row + 1,
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

/// Reads the tests, skips and assertions of a Java test file.
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_java::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    let dead = reach::Dead::of(root, src, &GRAMMAR);
    let (has_errors, error_line) = syntax_errors(root);
    let mut file = TestFile {
        has_errors,
        error_line,
        imports: imports(root, src),
        ..TestFile::default()
    };
    let mut open: Vec<(usize, Scope)> = Vec::new();
    // The byte ranges of the tests, and where each assertion starts.
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
        let kind = node.kind();
        if CLASSES.contains(&kind) {
            let name = node
                .child_by_field_name("name")
                .map_or("", |name| text(name, src))
                .to_owned();
            let outer_skipped = open
                .iter()
                .any(|(_, scope)| matches!(scope, Scope::Class { skipped: true, .. }));
            open.push((
                node.end_byte(),
                Scope::Class {
                    name,
                    skipped: outer_skipped || marks(node, src).skipped,
                    junit3: kind == "class_declaration" && extends_test_case(node, src),
                    fields: field_identifiers(node.child_by_field_name("body"), src),
                },
            ));
            return true;
        }
        match kind {
            "method_declaration" => {
                // A method of a local class is not found by a runner.
                let local = open.iter().any(|(_, scope)| matches!(scope, Scope::Method));
                if !local && in_declared_class(node) {
                    if let Some(test) = test_case(node, src, &Around::of(&open)) {
                        ranges.push((node.start_byte(), node.end_byte()));
                        file.tests.push(test);
                    }
                }
                open.push((node.end_byte(), Scope::Method));
                true
            }
            "method_invocation" => {
                if chained.contains(&node.id()) {
                    return true;
                }
                let links = chain(node);
                chained.extend(links.iter().skip(1).map(|link| link.id()));
                if links.iter().any(|link| asserts(*link, src))
                    && dead.why(node).is_none()
                    && !cannot_fail(&links, src)
                {
                    file.assertions.push(assertion(node, src));
                    starts.push(node.start_byte());
                }
                true
            }
            "assert_statement" => {
                if dead.why(node).is_none() && !statement_cannot_fail(node, src) {
                    file.assertions.push(assertion(node, src));
                    starts.push(node.start_byte());
                }
                true
            }
            "line_comment" | "block_comment" | "string_literal" => false,
            _ => true,
        }
    });
    assign(&mut file, &ranges, &starts);
    file
}

/// The names a Java file imports, each with what it names
/// (`Cart` → `com.acme.Cart`, a static `total` → `com.acme.Prices.total`),
/// and the classes of its own package it names without an import
/// (`Cart` → `com.acme.Cart` in `package com.acme;`). Wildcard imports
/// name nothing.
fn imports(root: Node<'_>, src: &[u8]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut package = String::new();
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for child in children(root) {
        match child.kind() {
            "package_declaration" => {
                if let Some(name) = child.named_child(0) {
                    package = squeezed(name, src);
                }
            }
            "import_declaration" => {
                let parts = children(child);
                if parts.iter().any(|part| part.kind() == "asterisk") {
                    continue;
                }
                if let Some(path) = parts.first() {
                    let spec = squeezed(*path, src);
                    let name = spec.rsplit('.').next().unwrap_or(&spec).to_owned();
                    found.insert(name, spec);
                }
            }
            _ => {}
        }
    }
    walk(root, |node| {
        if CLASSES.contains(&node.kind()) {
            if let Some(name) = node.child_by_field_name("name") {
                declared.insert(text(name, src).to_owned());
            }
        }
        !matches!(
            node.kind(),
            "import_declaration" | "package_declaration" | "string_literal"
        )
    });
    same_package(
        root,
        src,
        &package,
        &declared,
        &mut found,
        &["type_identifier", "identifier"],
    );
    found
}

/// Adds the capitalised names a file uses that it neither imports nor
/// declares, as classes of its own package.
pub(super) fn same_package(
    root: Node<'_>,
    src: &[u8],
    package: &str,
    declared: &BTreeSet<String>,
    found: &mut BTreeMap<String, String>,
    kinds: &[&str],
) {
    let mut names: BTreeSet<String> = BTreeSet::new();
    walk(root, |node| {
        if kinds.contains(&node.kind()) {
            let name = text(node, src);
            if name.chars().next().is_some_and(char::is_uppercase) {
                names.insert(name.to_owned());
            }
        }
        !matches!(
            node.kind(),
            "import_declaration" | "package_declaration" | "import" | "package_header"
        )
    });
    for name in names {
        if declared.contains(&name) || found.contains_key(&name) {
            continue;
        }
        let spec = if package.is_empty() {
            name.clone()
        } else {
            format!("{package}.{name}")
        };
        found.insert(name, spec);
    }
}

/// The source folders a JVM build reads, under each folder of the repo.
const SOURCE_ROOTS: [&str; 6] = [
    "src/main/java",
    "src/main/kotlin",
    "src/commonMain/kotlin",
    "src/jvmMain/kotlin",
    "src",
    "",
];

/// The repo file a Java or Kotlin import names (`com.acme.Cart`), among
/// the paths `exists` accepts: `com/acme/Cart.java` or `.kt` in a source
/// folder (`src/main/java`, `src/main/kotlin`, `src`, or none) of the
/// folder of `from` or one above it. A nested class or a static member
/// (`com.acme.Cart.Line`) names the file of its outer class. `None` for a
/// library or anything else.
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let parts: Vec<&str> = spec.split('.').collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || !part
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    }) {
        return None;
    }
    let mut folder: Vec<&str> = from.split('/').filter(|part| !part.is_empty()).collect();
    folder.pop();
    loop {
        for length in (1..=parts.len()).rev() {
            let module = parts[..length].join("/");
            for root in SOURCE_ROOTS {
                for extension in ["java", "kt"] {
                    let path = folder
                        .iter()
                        .copied()
                        .chain(root.split('/').filter(|part| !part.is_empty()))
                        .chain(std::iter::once(module.as_str()))
                        .collect::<Vec<_>>()
                        .join("/");
                    let path = format!("{path}.{extension}");
                    if exists(&path) {
                        return Some(path);
                    }
                }
            }
        }
        folder.pop()?;
    }
}

/// True for a path in a JVM test folder: the folder after the last `src`
/// is `test` or ends with `Test` (`androidTest`, `jvmTest`,
/// `integrationTest`); with no `src`, a folder named `test` or `tests`.
pub(super) fn in_test_folder(folders: &[&str]) -> bool {
    match folders.iter().rposition(|folder| *folder == "src") {
        Some(at) => folders
            .get(at + 1)
            .is_some_and(|set| *set == "test" || set.ends_with("Test")),
        None => folders
            .iter()
            .any(|folder| matches!(*folder, "test" | "tests")),
    }
}

/// The stem of a file name with one of `extensions` (any letter case),
/// and the folders above it; `None` for any other file.
pub(super) fn split_path<'p>(path: &'p str, extension: &str) -> Option<(&'p str, Vec<&'p str>)> {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop()?;
    let (stem, found) = name.rsplit_once('.')?;
    (!stem.is_empty() && found.eq_ignore_ascii_case(extension)).then_some((stem, parts))
}

/// True for a stem that starts with `Test` as a word: `Test`, `TestCart`,
/// not `Testimonial`.
pub(super) fn starts_with_test(stem: &str) -> bool {
    stem.strip_prefix("Test")
        .is_some_and(|rest| rest.chars().next().is_none_or(char::is_uppercase))
}

/// True for a path the guard reads as a Java test file: in a test folder
/// ([`in_test_folder`]), or named as Maven runs tests: `*Test.java`,
/// `*Tests.java`, `*TestCase.java`, `*IT.java`, `*ITCase.java`,
/// `Test*.java`.
pub(super) fn is_test_path(path: &str) -> bool {
    let Some((stem, folders)) = split_path(path, "java") else {
        return false;
    };
    ["Test", "Tests", "TestCase", "IT", "ITCase"]
        .iter()
        .any(|end| stem.ends_with(end))
        || starts_with_test(stem)
        || in_test_folder(&folders)
}

/// A cheap check that Java source may hold tests, before it is parsed: a
/// test annotation, or a class that extends JUnit 3's `TestCase`.
pub(super) fn holds_tests(source: &str) -> bool {
    [
        "@Test",
        "@ParameterizedTest",
        "@RepeatedTest",
        "@org.junit",
        "@org.testng",
        "extends TestCase",
        "extends junit.framework.TestCase",
    ]
    .iter()
    .any(|needle| source.contains(needle))
}

/// The string literals an annotation's value holds (`"a"` or `{"a",
/// "b"}`), and whether it held something else.
fn strings(value: Node<'_>, src: &[u8]) -> (Vec<String>, bool) {
    let mut found = Vec::new();
    let mut other = false;
    let items = if value.kind() == "element_value_array_initializer" {
        children(value)
    } else {
        vec![value]
    };
    for item in items {
        if item.kind() == "string_literal" {
            let mut content = String::new();
            for part in children(item) {
                content.push_str(text(part, src));
            }
            found.push(content);
        } else {
            other = true;
        }
    }
    (found, other)
}

/// The keys of a suppressing annotation: `@SuppressWarnings` and
/// `@SuppressFBWarnings`, one per rule it names, `*` for a value that is
/// not a literal or for none at all (SpotBugs' marker form).
fn annotation_keys(annotation: Node<'_>, src: &[u8]) -> Vec<String> {
    let tool = match annotation_name(annotation, src) {
        "SuppressWarnings" => "suppress-warnings",
        "SuppressFBWarnings" => "spotbugs",
        _ => return Vec::new(),
    };
    let mut rules = Vec::new();
    let mut other = annotation.kind() == "marker_annotation";
    for (key, value) in annotation_arguments(annotation, src) {
        if matches!(key.as_deref(), None | Some("value")) {
            let (found, not_literal) = strings(value, src);
            rules.extend(found);
            other |= not_literal;
        }
    }
    if rules.is_empty() && !other {
        return Vec::new();
    }
    keyed(tool, rules)
}

/// The text of a comment without its delimiters, trimmed.
pub(super) fn comment_body(raw: &str) -> &str {
    let body = raw
        .strip_prefix("//")
        .or_else(|| {
            raw.strip_prefix("/*")
                .map(|inner| inner.strip_suffix("*/").unwrap_or(inner))
        })
        .unwrap_or(raw);
    body.trim_start_matches(['*', '!']).trim()
}

/// The keys of a suppressing comment: PMD's `// NOPMD`, SonarQube's
/// `// NOSONAR`, Checkstyle's `CHECKSTYLE:OFF`, `CHECKSTYLE.OFF: Rule` and
/// `SUPPRESS CHECKSTYLE Rule`. The marker must open the comment.
fn comment_keys(raw: &str, line_comment: bool) -> Vec<String> {
    let body = comment_body(raw);
    if line_comment && directive(body, "NOPMD").is_some() {
        return vec!["nopmd:*".to_owned()];
    }
    if directive(body, "NOSONAR").is_some() {
        return vec!["nosonar:*".to_owned()];
    }
    if directive(body, "CHECKSTYLE:OFF").is_some() {
        return vec!["checkstyle-off:*".to_owned()];
    }
    if let Some(rest) = directive(body, "CHECKSTYLE.OFF") {
        let rules = rest
            .trim_start()
            .trim_start_matches(':')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        return keyed("checkstyle-off", rules.split(['|', ',']).map(str::to_owned));
    }
    if let Some(rest) = directive(body, "SUPPRESS CHECKSTYLE") {
        let rule = rest.split_whitespace().next().unwrap_or_default();
        return keyed("checkstyle", [rule.to_owned()]);
    }
    Vec::new()
}

/// The lint suppressions in a Java source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let language: Language = tree_sitter_java::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| {
        let keys = match node.kind() {
            "line_comment" => comment_keys(text(node, src), true),
            "block_comment" => comment_keys(text(node, src), false),
            "annotation" | "marker_annotation" => annotation_keys(node, src),
            "string_literal" => return false,
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
            r#"package com.acme;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.assertEquals;

class CartTest {
    private final Cart cart = new Cart();

    @Test
    void totals() {
        cart.add(2);
        assertEquals(2, cart.total());
        Assertions.assertTrue(cart.total() > 0, "positive");
    }

    private int helper() { return 1; }
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
                "Assertions.assertTrue(cart.total()>0,\"positive\")"
            ]
        );
        assert_eq!(file.assertions[0].line, 12);
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(
            file.assertions[1].text,
            "Assertions.assertTrue(cart.total() > 0, \"positive\")"
        );
    }

    #[test]
    fn every_test_annotation_is_a_test() {
        let file = read(
            r#"
class T {
    @Test void plain() { check(); }
    @org.junit.Test public void junit4() { check(); }
    @org.junit.jupiter.api.Test void qualified() { check(); }
    @Test(expected = IllegalStateException.class) public void throwing() { boom(); }
    @Test(timeout = 100) public void timed() { check(); }
    @RepeatedTest(3) void repeated() { check(); }
    @TestFactory Stream<DynamicTest> factory() { return Stream.empty(); }
    @TestTemplate void template() { check(); }
    void notATest() { check(); }
    @BeforeEach void setUp() { check(); }
}
"#,
        );
        let names: Vec<&str> = file.tests.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            names,
            [
                "T::plain",
                "T::junit4",
                "T::qualified",
                "T::throwing",
                "T::timed",
                "T::repeated",
                "T::factory",
                "T::template"
            ]
        );
        assert!(file.tests.iter().all(|t| !t.skipped));
    }

    #[test]
    fn nested_classes_name_the_id() {
        let file = read(
            r#"
class CartTest {
    @Nested
    class WhenEmpty {
        @Nested class AndClosed { @Test void rejects() { check(); } }
        @Test void totalsZero() { check(); }
    }
    @Test void top() { check(); }
}
interface Contract { @Test default void holds() { check(); } }
enum Kinds { A; @Test void kind() { check(); } }
"#,
        );
        let names: Vec<&str> = file.tests.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            names,
            [
                "CartTest::WhenEmpty::AndClosed::rejects",
                "CartTest::WhenEmpty::totalsZero",
                "CartTest::top",
                "Contract::holds",
                "Kinds::kind"
            ]
        );
    }

    #[test]
    fn anonymous_and_local_classes_hold_no_tests() {
        let file = read(
            r#"
class T {
    Object o = new Object() { @Test void anon() { check(); } };
    @Test void outer() {
        class Local { @Test void inner() { check(); } }
        Runnable r = new Runnable() { @Test public void run() { assertTrue(x); } };
    }
}
"#,
        );
        assert_eq!(ids(&file), [("T::outer", false, None, 1)]);
        // An assertion in a class inside a test belongs to that test.
        assert_eq!(file.assertions.len(), 1);
        assert_eq!(file.assertions[0].test, Some(0));
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r#"
class T {
    // @Test void commented() { assertTrue(x); }
    /* @Test */ void blockCommented() { }
    String s = "@Test void inString() { assertTrue(x); }";
    String t = """
        @Test void inTextBlock() { assertEquals(1, x); }
        """;
    @Testing void lookAlike() { }
    @TestInstance(PER_CLASS) void instance() { }
}
"#,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn disabled_and_ignored_tests_are_skipped() {
        let file = read(
            r#"
class T {
    @Test @Disabled void disabled() { check(); }
    @Test @Disabled("flaky") void withReason() { check(); }
    @Test @Ignore public void ignored() { check(); }
    @Ignore("later") @Test public void ignoreFirst() { check(); }
    @Test void runs() { check(); }
}
@Disabled
class Off {
    @Test void inside() { check(); }
    @Nested class Deeper { @Test void deep() { check(); } }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::disabled", true, None, 1),
                ("T::withReason", true, None, 1),
                ("T::ignored", true, None, 1),
                ("T::ignoreFirst", true, None, 1),
                ("T::runs", false, None, 1),
                ("Off::inside", true, None, 1),
                ("Off::Deeper::deep", true, None, 1),
            ]
        );
    }

    #[test]
    fn conditions_and_testng_enabled_false_are_skips() {
        let file = read(
            r#"
class T {
    @Test @DisabledOnOs(OS.WINDOWS) void notOnWindows() { check(); }
    @Test @EnabledIfEnvironmentVariable(named = "CI", matches = "true") void onCi() { check(); }
    @Test @EnabledOnJre(JRE.JAVA_17) void onJava17() { check(); }
    @Test @DisabledIf("slow") void notSlow() { check(); }
    @Test(enabled = false) public void testng() { check(); }
    @Test(enabled = true) public void testngOn() { check(); }
    @Test(enabled = ENABLED) public void testngMaybe() { check(); }
    @Test @Tag("slow") void tagged() { check(); }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::notOnWindows", true, None, 1),
                ("T::onCi", true, None, 1),
                ("T::onJava17", true, None, 1),
                ("T::notSlow", true, None, 1),
                ("T::testng", true, None, 1),
                ("T::testngOn", false, None, 1),
                ("T::testngMaybe", true, None, 1),
                ("T::tagged", false, None, 1),
            ]
        );
    }

    #[test]
    fn a_body_that_does_nothing_or_assumes_false_first_is_a_skip() {
        let file = read(
            r#"
class T {
    @Test void empty() {}
    @Test void semicolons() { ; // nothing
    }
    @Test void returns() { return; }
    @Test void assumes() { Assumptions.assumeTrue(false); assertEquals(1, x); }
    @Test void assumesJunit4() { Assume.assumeTrue("off", false); check(); }
    @Test void assumesNot() { assumeFalse(true); check(); }
    @Test void aborts() { Assumptions.abort(); check(); }
    @Test void testng() { throw new SkipException("later"); }
    @Test void assumesSometimes() { assumeTrue(onCi()); check(); }
    @Test void calls() { run(); }
}
"#,
        );
        let nothing = Some(DOES_NOTHING);
        let first = Some(SKIPS_FIRST);
        assert_eq!(
            ids(&file),
            [
                ("T::empty", true, nothing, 1),
                ("T::semicolons", true, nothing, 1),
                ("T::returns", true, nothing, 1),
                ("T::assumes", true, first, 1),
                ("T::assumesJunit4", true, first, 1),
                ("T::assumesNot", true, first, 1),
                ("T::aborts", true, first, 1),
                ("T::testng", true, first, 1),
                ("T::assumesSometimes", false, None, 1),
                ("T::calls", false, None, 1),
            ]
        );
        // The assertion after the skip never runs.
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn junit3_test_methods_are_tests() {
        let file = read(
            r#"
public class CartTest extends junit.framework.TestCase {
    public void testTotal() { assertEquals(1, total()); }
    public void helper() { }
}
class Plain { public void testNothing() { } }
"#,
        );
        assert_eq!(ids(&file), [("CartTest::testTotal", false, None, 1)]);
    }

    #[test]
    fn parameterized_tests_weigh_their_written_rows() {
        let file = read(
            r#"
class T {
    @ParameterizedTest
    @ValueSource(ints = {1, 2, 3})
    void ints(int n) { assertTrue(n > 0); }

    @ParameterizedTest
    @ValueSource(strings = "one")
    void one(String s) { assertNotNull(s); }

    @ParameterizedTest
    @CsvSource({"a, 1", "b, 2"})
    void csv(String s, int n) { check(); }

    @ParameterizedTest
    @CsvSource(value = {"a, 1", "b, 2", "c, 3"}, delimiter = ',')
    void csvValue(String s, int n) { check(); }

    @ParameterizedTest
    @CsvSource(textBlock = """
        # a comment
        a, 1

        b, 2
        """)
    void block(String s, int n) { check(); }

    @ParameterizedTest
    @NullAndEmptySource
    @ValueSource(strings = {" ", "\t"})
    void blank(String s) { check(); }

    @ParameterizedTest
    @MethodSource("cases")
    void provided(int n) { check(); }

    @Test
    @ValueSource(ints = {1, 2})
    void notParameterized() { check(); }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::ints", false, None, 3),
                ("T::one", false, None, 1),
                ("T::csv", false, None, 2),
                ("T::csvValue", false, None, 3),
                ("T::block", false, None, 2),
                ("T::blank", false, None, 4),
                ("T::provided", false, None, 1),
                ("T::notParameterized", false, None, 1),
            ]
        );
        let weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(weights, [3, 1]);
    }

    #[test]
    fn every_assertion_form_counts_once() {
        let file = read(
            r#"
class T {
    @Test void t() {
        assertEquals(3, total());
        org.junit.Assert.assertEquals("msg", 3, total());
        assertThat(total()).isEqualTo(3).isPositive();
        softly.assertThat(name).startsWith("a");
        assertThat(total(), is(3));
        assertThrows(IllegalStateException.class, () -> cart.close());
        assertAll(() -> assertEquals(1, a()), () -> assertEquals(2, b()));
        verify(repository, times(2)).save(cart);
        verifyNoMoreInteractions(repository);
        fail("must not get here");
        assert total() == 3 : "total";
        Assert.fail();
        Mockito.verify(repository).load();
        when(repository.load()).thenReturn(cart);
        signature.verify(data);
        promise.fail(error);
        System.out.println(total());
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assertEquals(3,total())",
                "org.junit.Assert.assertEquals(\"msg\",3,total())",
                "assertThat(total()).isEqualTo(3).isPositive()",
                "softly.assertThat(name).startsWith(\"a\")",
                "assertThat(total(),is(3))",
                "assertThrows(IllegalStateException.class,()->cart.close())",
                "assertAll(()->assertEquals(1,a()),()->assertEquals(2,b()))",
                "assertEquals(1,a())",
                "assertEquals(2,b())",
                "verify(repository,times(2)).save(cart)",
                "verifyNoMoreInteractions(repository)",
                "fail(\"mustnotgethere\")",
                "asserttotal()==3:\"total\";",
                "Assert.fail()",
                "Mockito.verify(repository).load()",
            ]
        );
        assert!(file.assertions.iter().all(|a| a.test == Some(0)));
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"
class T {
    @Test void t() {
        assertTrue(true);
        Assertions.assertTrue(true, "placeholder");
        assertTrue("placeholder", true);
        assertFalse(false);
        assertEquals(1, 1);
        assertEquals("a", "a");
        assertEquals(-1.5, -1.5, 0.01);
        assertEquals(2, 2, "two");
        assertEquals("two", 2, 2);
        assertThat(1).isEqualTo(1);
        assertThat(true).isTrue();
        assert true;
        assertEquals("a", "a", "b");
        assertEquals(1, 2);
        assertTrue(false);
        assertTrue(ok);
        assertEquals(1, total());
        assertThat(1).isEqualTo(2);
        assertThat(true).isTrue().isNotNull();
        assert false : "never";
    }
}
"#,
        );
        // Only literals that certainly pass are left out; one that
        // certainly fails marks code that must not be reached.
        assert_eq!(
            shapes(&file),
            [
                "assertEquals(\"a\",\"a\",\"b\")",
                "assertEquals(1,2)",
                "assertTrue(false)",
                "assertTrue(ok)",
                "assertEquals(1,total())",
                "assertThat(1).isEqualTo(2)",
                "assertThat(true).isTrue().isNotNull()",
                "assertfalse:\"never\";",
            ]
        );
    }

    #[test]
    fn assertions_that_never_run_are_not_counted() {
        let file = read(
            r#"
class T {
    @Test void insideIfFalse() {
        if (false) {
            assertEquals(3, total());
        }
        if ((false)) assertTrue(x);
        while (false) { assertTrue(y); }
        assertTrue(a);
    }
    @Test void afterReturn() {
        assertTrue(b);
        return;
        assertTrue(c);
    }
    @Test void reachable() {
        if (true) { assertTrue(d); }
        Runnable r = () -> { return; };
        assertTrue(e);
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assertTrue(a)",
                "assertTrue(b)",
                "assertTrue(d)",
                "assertTrue(e)"
            ]
        );
        assert!(!file.tests[0].skipped);
    }

    #[test]
    fn assertions_outside_tests_count_once() {
        let file = read(
            r#"
class Checks {
    static void checkTotal(Cart cart) { assertEquals(3, cart.total()); }
    @ParameterizedTest @ValueSource(ints = {1, 2}) void t(int n) { checkTotal(cart(n)); }
}
"#,
        );
        assert_eq!(file.assertions.len(), 1);
        assert_eq!(file.assertions[0].test, None);
        assert_eq!(file.assertions[0].weight, 1);
    }

    #[test]
    fn imports_name_their_classes_and_the_package_its_own() {
        let file = read(
            r#"package com.acme.shop;

import com.acme.pricing.Prices;
import static com.acme.pricing.Taxes.vat;
import static org.junit.jupiter.api.Assertions.*;
import java.util.*;

class CartTest {
    @Test void totals() { assertEquals(3, new Cart(Prices.of(vat(1))).total()); }
    static class Helper {}
}
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
                ("Prices", "com.acme.pricing.Prices"),
                ("Test", "com.acme.shop.Test"),
                ("vat", "com.acme.pricing.Taxes.vat"),
            ]
        );
    }

    #[test]
    fn resolve_finds_classes_in_source_folders() {
        let files = [
            "shop/src/main/java/com/acme/Cart.java",
            "shop/src/main/kotlin/com/acme/Prices.kt",
            "src/com/acme/Legacy.java",
        ];
        let exists = |path: &str| files.contains(&path);
        let from = "shop/src/test/java/com/acme/CartTest.java";
        assert_eq!(
            resolve(from, "com.acme.Cart", &exists).as_deref(),
            Some(files[0])
        );
        assert_eq!(
            resolve(from, "com.acme.Cart.Line", &exists).as_deref(),
            Some(files[0])
        );
        assert_eq!(
            resolve(from, "com.acme.Prices", &exists).as_deref(),
            Some(files[1])
        );
        assert_eq!(
            resolve(from, "com.acme.Legacy", &exists).as_deref(),
            Some(files[2])
        );
        assert_eq!(resolve(from, "org.junit.Test", &exists), None);
        assert_eq!(resolve(from, "com..Cart", &exists), None);
        assert_eq!(resolve(from, "com/acme.Cart", &exists), None);
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read("class T {\n    @Test void t() {\n        assertEquals(1, x);\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let source = format!(
            "class T {{ @Test void t() {{ {} assertTrue(x); {} }} }}\n",
            "{ ".repeat(1_000),
            "} ".repeat(1_000)
        );
        let file = read(&source);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
    }

    #[test]
    fn test_paths() {
        for path in [
            "src/test/java/com/acme/Cart.java",
            "shop/src/test/java/com/acme/Helpers.java",
            "app/src/androidTest/java/com/acme/CartUi.java",
            "src/integrationTest/java/Db.java",
            "CartTest.java",
            "lib/CartTests.java",
            "lib/CartTestCase.java",
            "lib/CartIT.java",
            "lib/CartITCase.java",
            "lib/TestCart.java",
            "lib/Test.java",
            "test/com/acme/Cart.java",
            "lib/CartTest.JAVA",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "src/main/java/com/acme/Cart.java",
            "src/main/java/com/acme/test/Fixtures.java",
            "lib/Testimonial.java",
            "lib/Contest.java",
            "src/test/java/CartTest.kt",
            "CartTest.javax",
            ".java",
            "src/test",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_looks_for_test_annotations() {
        for source in [
            "@Test void t() {}",
            "@ParameterizedTest",
            "@RepeatedTest(2)",
            "@TestFactory",
            "@org.junit.jupiter.api.Test",
            "class T extends TestCase {}",
        ] {
            assert!(holds_tests(source), "{source}");
        }
        assert!(!holds_tests("class Cart { int total() { return 0; } }"));
        assert!(!holds_tests("class TestCaseRunner { }"));
    }

    #[test]
    fn suppress_warnings_has_a_key_per_rule() {
        assert_eq!(
            keys(
                r#"
@SuppressWarnings("unchecked")
class A {
    @SuppressWarnings({"rawtypes", "PMD.UnusedLocal"}) void a() {}
    @java.lang.SuppressWarnings(value = {"squid:S1234"}) void b() {}
    @SuppressWarnings(ALL) void c() {}
    @SuppressWarnings({}) void d() {}
    @Override public String toString() { return ""; }
}
"#
            ),
            [
                "suppress-warnings:unchecked",
                "suppress-warnings:rawtypes",
                "suppress-warnings:PMD.UnusedLocal",
                "suppress-warnings:squid:S1234",
                "suppress-warnings:*",
            ]
        );
    }

    #[test]
    fn spotbugs_annotations_are_suppressions() {
        assert_eq!(
            keys(
                r#"
class A {
    @SuppressFBWarnings(value = {"EI_EXPOSE_REP", "EI_EXPOSE_REP2"}, justification = "copied") int[] a;
    @SuppressFBWarnings("NP_NULL_ON_SOME_PATH") void b() {}
    @SuppressFBWarnings void c() {}
}
"#
            ),
            [
                "spotbugs:EI_EXPOSE_REP",
                "spotbugs:EI_EXPOSE_REP2",
                "spotbugs:NP_NULL_ON_SOME_PATH",
                "spotbugs:*",
            ]
        );
    }

    #[test]
    fn suppressing_comments_are_read_from_comments_only() {
        let found = suppressions(
            r#"
class A {
    int a = 1; // NOPMD - generated
    int b = 2; //NOSONAR
    /* NOSONAR */ int c = 3;
    // CHECKSTYLE:OFF
    // CHECKSTYLE.OFF: MagicNumber|ParameterNumber
    // CHECKSTYLE.OFF
    // SUPPRESS CHECKSTYLE MagicNumber
    // CHECKSTYLE:ON
    /* NOPMD */ int d = 4;
    // we used to need NOPMD here
    // NOPMDX
    String e = "// NOPMD";
    String f = "@SuppressWarnings(\"all\")";
    String g = """
        // NOSONAR
        """;
}
"#,
        );
        let keys: Vec<&str> = found.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "nopmd:*",
                "nosonar:*",
                "nosonar:*",
                "checkstyle-off:*",
                "checkstyle-off:MagicNumber",
                "checkstyle-off:ParameterNumber",
                "checkstyle-off:*",
                "checkstyle:MagicNumber",
            ]
        );
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].text, "// NOPMD - generated");
    }
}
