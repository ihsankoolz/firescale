//! The C# reader of the tamper guard (M32, ADR 0052): xUnit, NUnit and
//! MSTest.
//!
//! - A test is a method with a test attribute: xUnit `[Fact]`, `[Theory]`
//!   (and Xunit.SkippableFact's `[SkippableFact]`, `[SkippableTheory]`),
//!   NUnit `[Test]`, `[TestCase(…)]`, `[TestCaseSource(…)]`, MSTest
//!   `[TestMethod]`, `[DataTestMethod]`. Its id is the names of the classes
//!   around it (nested ones too, without the namespace) and its own,
//!   joined with `::` (`PricingTests::Discounts::Applies`).
//! - Its weight is its rows: one per `[InlineData]`, `[TestCase]` or
//!   `[DataRow]`, and one per `[MemberData]`, `[ClassData]`,
//!   `[TestCaseSource]` or `[DynamicData]` (rows not known); a row with
//!   `Skip =`, `Ignore =`, `IgnoreReason =`, `IgnoreMessage =` or
//!   `Explicit = true` is a skipped row.
//! - It is skipped with `[Fact(Skip = "…")]` (any test attribute's `Skip`),
//!   `Explicit = true`, NUnit `[Ignore("…")]` and `[Explicit]` or MSTest
//!   `[Ignore]` on it or on its class (also `[TestFixture(Ignore = "…")]`),
//!   when its first statement is `Assert.Ignore(…)`, `Assert.Inconclusive(…)`,
//!   `Assert.Pass(…)`, `Assert.Skip(…)` or `Skip.If(true)`, when its body
//!   does nothing, or when it sits under `#if false` (never compiled) or
//!   another `#if` (compiled only sometimes, a conditional skip).
//! - An assertion is a call on `Assert` (xUnit, NUnit classic and
//!   constraint, MSTest; also `ClassicAssert`, `CollectionAssert`,
//!   `StringAssert`, `FileAssert`, `DirectoryAssert`) other than those that
//!   skip or group (`Ignore`, `Inconclusive`, `Pass`, `Skip`, `Multiple`,
//!   `Warn`); a FluentAssertions chain (`x.Should().Be(…)`); a Shouldly
//!   call (`x.ShouldBe(…)`, `Should.Throw(…)`); Moq's `mock.Verify(…)`,
//!   `VerifyAll()` and their kin (not any `x.Verify…()`); Verify's
//!   snapshots (`Verify(…)`); NSubstitute's `Received()` chains; FakeItEasy's
//!   `MustHaveHappened…`. Not one that never runs (`if (false)`, after a
//!   `return`, under `#if false`) or cannot fail (`Assert.True(true)`,
//!   `Assert.Equal(1, 1)`, `true.Should().BeTrue()`).
//! - `using` directives are its imports (`using Foo.Bar;` gives `Bar`).
//!   The guard resolves no C# namespace to a file, so a C# test deleted
//!   with its subject is still counted.
//! - Suppressions: `#pragma warning disable` (`pragma-disable:CS0168`, `*`
//!   when it names none), `[SuppressMessage("…", "CA1822:…")]` and
//!   `[UnconditionalSuppressMessage]` (`suppress-message:CA1822`),
//!   `// ReSharper disable [once] Rule` (`resharper:Rule`) and
//!   `#nullable disable` (`nullable-disable:*`).

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::{Assertion, Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// What the C# grammar calls the nodes [`reach`] looks at.
const GRAMMAR: Grammar = Grammar {
    blocks: &["block"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &["while_statement"],
    hoisted: &[
        "method_declaration",
        "local_function_statement",
        "constructor_declaration",
        "class_declaration",
        "struct_declaration",
        "record_declaration",
        "interface_declaration",
        "namespace_declaration",
    ],
    stops: Some(stops),
    noop: is_noop,
};

/// How deep a chain of calls is followed: real code never comes near it.
const MAX_CHAIN: usize = 32;

/// Attributes that make a method a test.
const TEST_ATTRIBUTES: [&str; 9] = [
    "Fact",
    "Theory",
    "SkippableFact",
    "SkippableTheory",
    "Test",
    "TestCase",
    "TestCaseSource",
    "TestMethod",
    "DataTestMethod",
];

/// Attributes that each give a test one row written out.
const ROW_ATTRIBUTES: [&str; 3] = ["InlineData", "TestCase", "DataRow"];

/// Attributes that each give a test rows that are not written out: one.
const SOURCE_ATTRIBUTES: [&str; 4] = ["MemberData", "ClassData", "TestCaseSource", "DynamicData"];

/// Classes whose calls are assertions.
const ASSERT_CLASSES: [&str; 6] = [
    "Assert",
    "ClassicAssert",
    "CollectionAssert",
    "StringAssert",
    "FileAssert",
    "DirectoryAssert",
];

/// Calls on `Assert` that are not assertions: they skip, pass, group
/// others or only warn.
const NOT_ASSERTIONS: [&str; 12] = [
    "Ignore",
    "Inconclusive",
    "Pass",
    "Skip",
    "SkipWhen",
    "SkipUnless",
    "Multiple",
    "MultipleAsync",
    "EnterMultipleScope",
    "Warn",
    "Equals",
    "ReferenceEquals",
];

/// NSubstitute's calls that turn the next call into a check.
const RECEIVED: [&str; 4] = [
    "Received",
    "DidNotReceive",
    "ReceivedWithAnyArgs",
    "DidNotReceiveWithAnyArgs",
];

/// How Shouldly's assertions start (`ShouldBe`, `ShouldNotBeNull`,
/// `ShouldContain`): not every method named `Should…` is one
/// (`ShouldRetry`).
const SHOULDLY: [&str; 11] = [
    "ShouldBe",
    "ShouldNot",
    "ShouldContain",
    "ShouldThrow",
    "ShouldStartWith",
    "ShouldEndWith",
    "ShouldMatch",
    "ShouldAllBe",
    "ShouldHave",
    "ShouldSatisfy",
    "ShouldComplete",
];

/// Moq's checks of what a mock was called with.
const MOQ_VERIFY: [&str; 7] = [
    "Verify",
    "VerifyAll",
    "VerifyNoOtherCalls",
    "VerifyGet",
    "VerifySet",
    "VerifyAdd",
    "VerifyRemove",
];

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 13] = [
    "integer_literal",
    "real_literal",
    "boolean_literal",
    "null_literal",
    "character_literal",
    "string_literal",
    "string_literal_content",
    "verbatim_string_literal",
    "raw_string_literal",
    "raw_string_start",
    "raw_string_content",
    "raw_string_end",
    "escape_sequence",
];

/// Why `why` says a test does not run under `#if false`.
const NEVER_COMPILED: &str = "under #if false, which is never true";

/// Class-like declarations: their names make a test's id.
const CLASSES: [&str; 4] = [
    "class_declaration",
    "struct_declaration",
    "record_declaration",
    "interface_declaration",
];

/// A statement that does nothing: `;`, or code under a plain `#if false`.
fn is_noop(statement: Node<'_>, src: &[u8]) -> bool {
    match statement.kind() {
        "empty_statement" => true,
        "preproc_if" => {
            statement.child_by_field_name("alternative").is_none()
                && statement
                    .child_by_field_name("condition")
                    .is_some_and(|condition| condition_of(condition, src) == Cond::False)
        }
        _ => false,
    }
}

/// A statement after which the rest of its block does not run: a call
/// that skips or passes the test there and then.
fn stops(statement: Node<'_>, src: &[u8]) -> bool {
    skip_call(statement, src)
}

/// True for a statement that is only a call that ends the test without
/// failing it: `Assert.Ignore(…)`, `Assert.Inconclusive(…)`,
/// `Assert.Pass(…)`, xUnit v3's `Assert.Skip(…)`, `Assert.SkipWhen(true,
/// …)`, `Assert.SkipUnless(false, …)`, and Xunit.SkippableFact's
/// `Skip.If(true)` and `Skip.IfNot(false)`.
fn skip_call(statement: Node<'_>, src: &[u8]) -> bool {
    if statement.kind() != "expression_statement" {
        return false;
    }
    let Some(call) = statement.named_child(0) else {
        return false;
    };
    if call.kind() != "invocation_expression" {
        return false;
    }
    let Some((Some(receiver), name)) = callee(call, src) else {
        return false;
    };
    let receiver = last_segment(receiver, src);
    let first = arguments(call).first().map(|argument| text(*argument, src));
    match (receiver, name) {
        ("Assert", "Ignore" | "Inconclusive" | "Pass" | "Skip") => true,
        ("Assert", "SkipWhen") | ("Skip", "If") => first == Some("true"),
        ("Assert", "SkipUnless") | ("Skip", "IfNot") => first == Some("false"),
        _ => false,
    }
}

/// What a preprocessor condition certainly is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cond {
    True,
    False,
    /// A symbol or an expression: true only sometimes.
    Other,
}

impl Cond {
    /// What "an earlier condition may have been true, and this one is
    /// `self`" certainly is: never false.
    fn max_with_other(self) -> Self {
        match self {
            Self::True => Self::True,
            Self::False | Self::Other => Self::Other,
        }
    }
}

/// The value of an `#if` condition: `true` and `false` (in parentheses or
/// not) are certain, anything else is not.
fn condition_of(condition: Node<'_>, src: &[u8]) -> Cond {
    let mut at = condition;
    for _ in 0..8 {
        match at.kind() {
            "parenthesized_expression" => match at.named_child(0) {
                Some(inner) => at = inner,
                None => return Cond::Other,
            },
            "boolean_literal" => {
                return if text(at, src) == "true" {
                    Cond::True
                } else {
                    Cond::False
                }
            }
            _ => return Cond::Other,
        }
    }
    Cond::Other
}

/// The receiver and the name of a call: `(Some(Assert), "Equal")` for
/// `Assert.Equal(…)`, `(None, "Verify")` for `Verify(…)`. A generic name
/// is its identifier (`ThrowsAsync` for `ThrowsAsync<T>`).
fn callee<'t, 's>(call: Node<'t>, src: &'s [u8]) -> Option<(Option<Node<'t>>, &'s str)> {
    let function = call.child_by_field_name("function")?;
    match function.kind() {
        "member_access_expression" => {
            let name = function.child_by_field_name("name")?;
            Some((
                function.child_by_field_name("expression"),
                name_text(name, src),
            ))
        }
        "identifier" | "generic_name" => Some((None, name_text(function, src))),
        _ => None,
    }
}

/// The text of a simple or generic name, without its type arguments.
fn name_text<'s>(name: Node<'_>, src: &'s [u8]) -> &'s str {
    if name.kind() == "generic_name" {
        name.named_child(0)
            .map_or("", |identifier| text(identifier, src))
    } else {
        text(name, src)
    }
}

/// The last name of a receiver: `Assert` for `Xunit.Assert`, `""` for a
/// receiver that is not a name.
fn last_segment<'s>(receiver: Node<'_>, src: &'s [u8]) -> &'s str {
    match receiver.kind() {
        "identifier" | "generic_name" => name_text(receiver, src),
        "member_access_expression" | "qualified_name" => receiver
            .child_by_field_name("name")
            .map_or("", |name| name_text(name, src)),
        _ => "",
    }
}

/// The expressions a call passes, in order (named arguments included).
fn arguments(call: Node<'_>) -> Vec<Node<'_>> {
    let Some(list) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|argument| argument.kind() == "argument")
        .filter_map(|argument| {
            let mut inner = argument.walk();
            let last = argument
                .named_children(&mut inner)
                .filter(|child| !child.is_extra())
                .last();
            last
        })
        .collect()
}

/// True when a chain of calls under `node` (through receivers, not
/// arguments) holds a call named one of `names`.
fn chain_has(node: Node<'_>, src: &[u8], names: &[&str]) -> bool {
    let mut at = node;
    for _ in 0..MAX_CHAIN {
        match at.kind() {
            "invocation_expression" => {
                if callee(at, src).is_some_and(|(_, name)| names.contains(&name)) {
                    return true;
                }
                match at.child_by_field_name("function") {
                    Some(function) => at = function,
                    None => return false,
                }
            }
            "member_access_expression" | "conditional_access_expression" => {
                match at
                    .child_by_field_name("expression")
                    .or_else(|| at.named_child(0))
                {
                    Some(inner) => at = inner,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
    false
}

/// True for a call that is an assertion.
fn is_assertion(call: Node<'_>, src: &[u8]) -> bool {
    let Some((receiver, name)) = callee(call, src) else {
        return false;
    };
    if let Some(receiver) = receiver {
        let class = last_segment(receiver, src);
        if ASSERT_CLASSES.contains(&class) {
            return !NOT_ASSERTIONS.contains(&name);
        }
        // Shouldly: `x.ShouldBe(…)`, `Should.Throw(…)`.
        if SHOULDLY.iter().any(|prefix| name.starts_with(prefix)) || class == "Should" {
            return true;
        }
        // FluentAssertions: a chain through `.Should()`.
        if name != "Should" && chain_has(receiver, src, &["Should"]) {
            return true;
        }
        // NSubstitute: `sub.Received().Add(1)`.
        if !RECEIVED.contains(&name) && chain_has(receiver, src, &RECEIVED) {
            return true;
        }
        // FakeItEasy.
        if name.starts_with("MustHaveHappened") || name.starts_with("MustNotHaveHappened") {
            return true;
        }
        // Moq: `mock.Verify(…)`, `mock.VerifyAll()`; not any method whose
        // name starts so (`signer.VerifySignature(…)`).
        return MOQ_VERIFY.contains(&name);
    }
    // Verify's snapshots: `Verify(result)`, `VerifyJson(text)`.
    name.starts_with("Verify")
}

/// The source text of a literal, without white space, or `None` when the
/// node is not a literal.
fn literal(node: Node<'_>, src: &[u8]) -> Option<String> {
    reach::constant(node, &LITERALS).then(|| text(node, src).split_whitespace().collect())
}

/// True for an assertion that cannot fail: it checks only literals, and
/// they certainly pass (`Assert.True(true)`, `Assert.Equal(1, 1)`,
/// `Assert.That(1, Is.EqualTo(1))`, `true.Should().BeTrue()`,
/// `1.ShouldBe(1)`). One that certainly fails counts like any other.
fn cannot_fail(call: Node<'_>, src: &[u8]) -> bool {
    let Some((receiver, name)) = callee(call, src) else {
        return false;
    };
    let Some(receiver) = receiver else {
        return false;
    };
    let args = arguments(call);
    let literal_at = |index: usize| args.get(index).and_then(|node| literal(*node, src));
    let passes = |name: &str, subject: Option<String>, expected: Option<String>| -> bool {
        let Some(subject) = subject else {
            return false;
        };
        match name {
            "True" | "IsTrue" | "BeTrue" => subject == "true",
            "False" | "IsFalse" | "BeFalse" => subject == "false",
            "Null" | "IsNull" | "BeNull" => subject == "null",
            "Equal" | "AreEqual" | "StrictEqual" | "Be" | "EqualTo" => {
                expected.is_some_and(|expected| expected == subject)
            }
            _ => false,
        }
    };
    if ASSERT_CLASSES.contains(&last_segment(receiver, src)) {
        if name == "That" {
            // `Assert.That(true)`, `Assert.That(x, Is.True)`,
            // `Assert.That(x, Is.EqualTo(y))`.
            let subject = literal_at(0);
            return match args.get(1) {
                None => subject.as_deref() == Some("true"),
                Some(constraint) => match constraint.kind() {
                    "member_access_expression" => {
                        let property = constraint
                            .child_by_field_name("name")
                            .map_or("", |n| text(n, src));
                        passes(property, subject, None)
                    }
                    "invocation_expression" => {
                        callee(*constraint, src).is_some_and(|(_, matcher)| {
                            let expected = arguments(*constraint)
                                .first()
                                .and_then(|node| literal(*node, src));
                            passes(matcher, subject, expected)
                        })
                    }
                    _ => false,
                },
            };
        }
        return passes(name, literal_at(0), literal_at(1));
    }
    // `subject.Should().Be(expected)`.
    if receiver.kind() == "invocation_expression" {
        if let Some((Some(subject), "Should")) = callee(receiver, src) {
            return passes(name, literal(subject, src), literal_at(0));
        }
        return false;
    }
    // Shouldly: `subject.ShouldBe(expected)`.
    if let Some(rest) = name.strip_prefix("Should") {
        return passes(rest, literal(receiver, src), literal_at(0));
    }
    false
}

/// The name of an attribute without its namespace or `Attribute` suffix.
fn attribute_name<'s>(attribute: Node<'_>, src: &'s [u8]) -> &'s str {
    let Some(name) = attribute.child_by_field_name("name") else {
        return "";
    };
    let last = match name.kind() {
        "qualified_name" | "alias_qualified_name" => name
            .child_by_field_name("name")
            .map_or("", |inner| name_text(inner, src)),
        _ => name_text(name, src),
    };
    last.strip_suffix("Attribute").unwrap_or(last)
}

/// The arguments of an attribute: each one's name when it is named
/// (`Skip = "…"`), and its value.
fn attribute_arguments<'t>(attribute: Node<'t>) -> Vec<(Option<Node<'t>>, Option<Node<'t>>)> {
    let mut cursor = attribute.walk();
    let Some(list) = attribute
        .named_children(&mut cursor)
        .find(|child| child.kind() == "attribute_argument_list")
    else {
        return Vec::new();
    };
    let mut inner = list.walk();
    list.named_children(&mut inner)
        .filter(|argument| argument.kind() == "attribute_argument")
        .map(|argument| {
            let name = argument.child_by_field_name("name");
            let mut values = argument.walk();
            let value = argument
                .named_children(&mut values)
                .filter(|child| !child.is_extra() && Some(*child) != name)
                .last();
            (name, value)
        })
        .collect()
}

/// True when an attribute has a named argument among `names` that turns
/// something off: a `Skip`, `Ignore` or `IgnoreReason` that is not `null`
/// or `""`, or `Explicit = true`.
fn turned_off(attribute: Node<'_>, src: &[u8], names: &[&str]) -> bool {
    attribute_arguments(attribute)
        .into_iter()
        .any(|(name, value)| {
            let Some(name) = name.map(|name| text(name, src)) else {
                return false;
            };
            if !names.contains(&name) {
                return false;
            }
            let value = value.map_or("", |value| text(value, src).trim());
            if name == "Explicit" {
                value == "true"
            } else {
                !matches!(value, "null" | "\"\"" | "")
            }
        })
}

/// The attributes written on a declaration, in source order.
fn attributes(declaration: Node<'_>) -> Vec<Node<'_>> {
    let mut found = Vec::new();
    let mut cursor = declaration.walk();
    for list in declaration
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "attribute_list")
    {
        let mut inner = list.walk();
        found.extend(
            list.named_children(&mut inner)
                .filter(|child| child.kind() == "attribute"),
        );
    }
    found
}

/// What the attributes of a method say about it.
#[derive(Default)]
struct Marks {
    test: bool,
    /// Skipped as a whole: `Skip =`, `[Ignore]`, `[Explicit]`.
    skipped: bool,
    rows: usize,
    skipped_rows: usize,
}

/// What the attributes of a method say: a test, skipped, its rows.
fn method_marks(method: Node<'_>, src: &[u8]) -> Marks {
    let mut marks = Marks::default();
    for attribute in attributes(method) {
        let name = attribute_name(attribute, src);
        if TEST_ATTRIBUTES.contains(&name) {
            marks.test = true;
        }
        if ROW_ATTRIBUTES.contains(&name) {
            marks.rows += 1;
            if turned_off(
                attribute,
                src,
                &[
                    "Skip",
                    "Ignore",
                    "IgnoreReason",
                    "IgnoreMessage",
                    "Explicit",
                ],
            ) {
                marks.skipped_rows += 1;
            }
        } else if SOURCE_ATTRIBUTES.contains(&name) {
            marks.rows += 1;
        }
        if TEST_ATTRIBUTES.contains(&name)
            && !ROW_ATTRIBUTES.contains(&name)
            && turned_off(attribute, src, &["Skip", "Explicit"])
        {
            marks.skipped = true;
        }
        if matches!(name, "Ignore" | "Explicit") {
            marks.skipped = true;
        }
    }
    marks
}

/// True for a class whose tests do not run: `[Ignore]`, `[Explicit]`, or
/// `[TestFixture(Ignore = "…")]`.
fn class_skipped(class: Node<'_>, src: &[u8]) -> bool {
    attributes(class).into_iter().any(|attribute| {
        let name = attribute_name(attribute, src);
        matches!(name, "Ignore" | "Explicit")
            || (matches!(name, "TestFixture" | "TestClass")
                && turned_off(attribute, src, &["Ignore", "IgnoreReason", "Explicit"]))
    })
}

/// The first statement of a block that does something: not `;` or a
/// comment.
fn first_doing<'t>(body: Node<'t>, src: &[u8]) -> Option<Node<'t>> {
    let mut cursor = body.walk();
    let first = body
        .named_children(&mut cursor)
        .find(|statement| !statement.is_extra() && !is_noop(*statement, src));
    first
}

/// The code a part of the file is compiled under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compiled {
    Always,
    /// Under `#if` of a symbol: only sometimes.
    Sometimes,
    Never,
}

impl Compiled {
    /// The narrower of two.
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::Sometimes, _) | (_, Self::Sometimes) => Self::Sometimes,
            _ => Self::Always,
        }
    }

    /// How the code under a condition is compiled.
    fn of(cond: Cond) -> Self {
        match cond {
            Cond::True => Self::Always,
            Cond::False => Self::Never,
            Cond::Other => Self::Sometimes,
        }
    }
}

/// Reads the tests, skips and assertions of a C# test file.
#[must_use]
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_c_sharp::LANGUAGE.into();
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

    /// The classes, methods and `#if` parts around the node being visited.
    struct Scope {
        end: usize,
        /// A class's name.
        class: Option<String>,
        /// For a class: its tests do not run.
        class_skipped: bool,
        compiled: Compiled,
        /// For an `#if` or `#elif`: how the code before its alternative
        /// was compiled, and what its conditions so far certainly are.
        preproc: Option<(Compiled, Cond)>,
        /// A method: the index of the test it is, if it is one.
        method: bool,
        test: Option<usize>,
    }
    let mut open: Vec<Scope> = vec![Scope {
        end: usize::MAX,
        class: None,
        class_skipped: false,
        compiled: Compiled::Always,
        preproc: None,
        method: false,
        test: None,
    }];
    walk(root, |node| {
        while open.len() > 1
            && open
                .last()
                .is_some_and(|scope| node.start_byte() >= scope.end)
        {
            open.pop();
        }
        let compiled = open.last().map_or(Compiled::Always, |scope| scope.compiled);
        match node.kind() {
            "comment"
            | "string_literal"
            | "verbatim_string_literal"
            | "raw_string_literal"
            | "preproc_pragma"
            | "preproc_nullable" => false,
            "preproc_if" => {
                let cond = node
                    .child_by_field_name("condition")
                    .map_or(Cond::Other, |condition| condition_of(condition, src));
                open.push(Scope {
                    end: node.end_byte(),
                    class: None,
                    class_skipped: false,
                    compiled: compiled.and(Compiled::of(cond)),
                    preproc: Some((compiled, cond)),
                    method: false,
                    test: None,
                });
                true
            }
            "preproc_elif" | "preproc_else" => {
                // The alternative of the `#if` or `#elif` it sits in.
                let (outer, before) = open
                    .iter()
                    .rev()
                    .find_map(|scope| scope.preproc)
                    .unwrap_or((compiled, Cond::Other));
                let own = if node.kind() == "preproc_elif" {
                    node.child_by_field_name("condition")
                        .map_or(Cond::Other, |condition| condition_of(condition, src))
                } else {
                    Cond::True
                };
                let (here, after) = match before {
                    Cond::True => (Compiled::Never, Cond::True),
                    Cond::False => (Compiled::of(own), own),
                    Cond::Other => (
                        Compiled::Sometimes.and(Compiled::of(own)),
                        own.max_with_other(),
                    ),
                };
                open.push(Scope {
                    end: node.end_byte(),
                    class: None,
                    class_skipped: false,
                    compiled: outer.and(here),
                    preproc: Some((outer, after)),
                    method: false,
                    test: None,
                });
                true
            }
            kind if CLASSES.contains(&kind) => {
                let name = node
                    .child_by_field_name("name")
                    .map(|name| text(name, src).to_owned());
                open.push(Scope {
                    end: node.end_byte(),
                    class: name,
                    class_skipped: class_skipped(node, src),
                    compiled,
                    preproc: None,
                    method: false,
                    test: None,
                });
                true
            }
            "method_declaration" => {
                let marks = method_marks(node, src);
                let mut test = None;
                if marks.test {
                    let name = node
                        .child_by_field_name("name")
                        .map_or("", |name| text(name, src));
                    let id = open
                        .iter()
                        .filter_map(|scope| scope.class.as_deref())
                        .chain(std::iter::once(name))
                        .collect::<Vec<_>>()
                        .join("::");
                    let class_skipped = open
                        .iter()
                        .rev()
                        .find(|scope| scope.class.is_some())
                        .is_some_and(|scope| scope.class_skipped);
                    let body = node
                        .child_by_field_name("body")
                        .filter(|body| body.kind() == "block");
                    let why = if marks.skipped || class_skipped {
                        None
                    } else if compiled == Compiled::Never {
                        Some(NEVER_COMPILED)
                    } else if body.is_some_and(|body| {
                        first_doing(body, src).is_some_and(|first| skip_call(first, src))
                    }) {
                        Some("it calls skip first")
                    } else if body.is_some_and(|body| reach::does_nothing(body, src, &GRAMMAR)) {
                        Some("its body does nothing")
                    } else {
                        None
                    };
                    let mut uses = BTreeSet::new();
                    walk(node, |inner| {
                        if inner.kind() == "identifier" {
                            uses.insert(text(inner, src).to_owned());
                        }
                        true
                    });
                    let weight = marks.rows.max(1);
                    test = Some(file.tests.len());
                    file.tests.push(TestCase {
                        id,
                        // The line of its name, not of its attributes.
                        line: node
                            .child_by_field_name("name")
                            .unwrap_or(node)
                            .start_position()
                            .row
                            + 1,
                        skipped: marks.skipped
                            || class_skipped
                            || compiled != Compiled::Always
                            || why.is_some(),
                        skipped_rows: marks.skipped_rows,
                        why,
                        marker: false,
                        focused: false,
                        uses,
                        weight,
                    });
                }
                open.push(Scope {
                    end: node.end_byte(),
                    class: None,
                    class_skipped: false,
                    compiled,
                    preproc: None,
                    method: true,
                    test,
                });
                true
            }
            "invocation_expression" => {
                if !is_assertion(node, src) {
                    return true;
                }
                if compiled == Compiled::Never || dead.why(node).is_some() || cannot_fail(node, src)
                {
                    return false;
                }
                let test = open
                    .iter()
                    .rev()
                    .find(|scope| scope.method)
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
                false
            }
            _ => true,
        }
    });
    file.imports = usings(root, src);
    file
}

/// The names the `using` directives of a file bring in, each with the
/// namespace or type it names: `using Foo.Bar;` gives `Bar`, `using X =
/// Foo.Bar;` gives `X`, `using static Foo.Bar;` gives `Bar`.
fn usings(root: Node<'_>, src: &[u8]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    walk(root, |node| match node.kind() {
        "compilation_unit"
        | "namespace_declaration"
        | "file_scoped_namespace_declaration"
        | "declaration_list" => true,
        "using_directive" => {
            let alias = node.child_by_field_name("name");
            let mut cursor = node.walk();
            let target = node
                .named_children(&mut cursor)
                .filter(|child| Some(*child) != alias && !child.is_extra())
                .find(|child| {
                    matches!(
                        child.kind(),
                        "identifier" | "qualified_name" | "generic_name" | "alias_qualified_name"
                    )
                });
            if let Some(target) = target {
                let spec: String = text(target, src).split_whitespace().collect();
                let name = match alias {
                    Some(alias) => text(alias, src).to_owned(),
                    None => last_segment(target, src).to_owned(),
                };
                if !name.is_empty() {
                    found.insert(name, spec);
                }
            }
            false
        }
        _ => false,
    });
    found
}

/// True for a path the guard reads as a C# test file by its name or
/// folder alone: a `.cs` file named `*Tests.cs` or `*Test.cs`, or in a
/// folder or project named `test`, `tests`, `*.Tests`, `*.Test` or
/// `*Tests` (`MyApp.Tests/`, `UnitTests/`). Others are test files when
/// they hold tests ([`holds_tests`]).
#[must_use]
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || !extension.eq_ignore_ascii_case("cs") {
        return false;
    }
    let test_folder = |folder: &&str| {
        let lower = folder.to_ascii_lowercase();
        lower == "test"
            || lower == "tests"
            || lower.ends_with(".tests")
            || lower.ends_with(".test")
            || folder.ends_with("Tests")
    };
    stem.ends_with("Tests") || stem.ends_with("Test") || folders.iter().any(test_folder)
}

/// A cheap check that C# source may hold tests, before it is parsed: an
/// attribute list that opens with a test attribute (`[Fact]`,
/// `[Xunit.Theory]`, `[TestMethod]`, `[TestFixture]`, `[TestClass]`…).
#[must_use]
pub(super) fn holds_tests(source: &str) -> bool {
    source.match_indices('[').any(|(at, _)| {
        let rest = source[at + 1..].trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .collect();
        let last = name.rsplit('.').next().unwrap_or_default();
        let last = last.strip_suffix("Attribute").unwrap_or(last);
        TEST_ATTRIBUTES.contains(&last) || matches!(last, "TestFixture" | "TestClass")
    })
}

/// White space squeezed, at most 120 characters, `…` when cut.
fn shown(raw: &str) -> String {
    let squeezed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if squeezed.chars().count() <= 120 {
        return squeezed;
    }
    let mut cut: String = squeezed.chars().take(119).collect();
    cut.push('…');
    cut
}

/// Keys of `tool:rule` for each rule in a list, or `tool:*` for none.
fn keyed(tool: &str, rules: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = rules
        .iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty())
        .map(|rule| format!("{tool}:{rule}"))
        .collect();
    if keys.is_empty() {
        keys.push(format!("{tool}:*"));
    }
    keys
}

/// The keys of a `#pragma warning disable …` line: one per warning it
/// names, `*` when it names none; none for `restore` or another pragma.
fn pragma(raw: &str) -> Vec<String> {
    let line = raw.split("//").next().unwrap_or_default();
    let mut words = line.split_whitespace();
    if words.next() != Some("#pragma") || words.next() != Some("warning") {
        return Vec::new();
    }
    if words.next() != Some("disable") {
        return Vec::new();
    }
    let rest: Vec<&str> = words.collect();
    let rest = rest.join(" ");
    keyed("pragma-disable", &rest.split(',').collect::<Vec<_>>())
}

/// The keys of a `#nullable` line that turns nullable warnings off:
/// `#nullable disable` and `#nullable disable warnings`.
fn nullable(raw: &str) -> Vec<String> {
    let line = raw.split("//").next().unwrap_or_default();
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["#nullable", "disable"] | ["#nullable", "disable", "warnings"] => {
            vec!["nullable-disable:*".to_owned()]
        }
        _ => Vec::new(),
    }
}

/// The keys of a ReSharper comment: `// ReSharper disable [once] A, B`
/// gives `resharper:A` and `resharper:B`; `All` is `*`.
fn resharper(raw: &str) -> Vec<String> {
    let Some(body) = raw.strip_prefix("//") else {
        return Vec::new();
    };
    let Some(rest) = body.trim_start().strip_prefix("ReSharper disable") else {
        return Vec::new();
    };
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return Vec::new();
    }
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("once")
        .filter(|after| after.is_empty() || after.starts_with(char::is_whitespace))
        .unwrap_or(rest);
    let rules: Vec<&str> = rest
        .split(',')
        .map(str::trim)
        .map(|rule| if rule == "All" { "" } else { rule })
        .collect();
    keyed("resharper", &rules)
}

/// The key of a `[SuppressMessage("Category", "CA1822:Name")]` attribute
/// (also `UnconditionalSuppressMessage`, with or without `Attribute`):
/// the id before the colon, `*` when it names none.
fn suppress_message(attribute: Node<'_>, src: &[u8]) -> Vec<String> {
    if !matches!(
        attribute_name(attribute, src),
        "SuppressMessage" | "UnconditionalSuppressMessage"
    ) {
        return Vec::new();
    }
    let arguments = attribute_arguments(attribute);
    let named = arguments
        .iter()
        .find(|(name, _)| name.is_some_and(|name| text(name, src) == "CheckId"));
    let check = named
        .or_else(|| arguments.iter().filter(|(name, _)| name.is_none()).nth(1))
        .and_then(|(_, value)| *value)
        .map(|value| {
            let mut content = String::new();
            walk(value, |inner| {
                if inner.kind() == "string_literal_content" {
                    content.push_str(text(inner, src));
                }
                true
            });
            content
        })
        .unwrap_or_default();
    let id = check
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    keyed("suppress-message", &[id.as_str()])
}

/// The lint suppressions in a C# source file: `#pragma warning disable`,
/// `#nullable disable`, `[SuppressMessage]` and ReSharper comments. One in
/// a string, or in prose in a comment, is not one.
#[must_use]
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let language: Language = tree_sitter_c_sharp::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| {
        let (keys, shown_node) = match node.kind() {
            "preproc_pragma" => (pragma(text(node, src)), node),
            "preproc_nullable" => (nullable(text(node, src)), node),
            "comment" => (resharper(text(node, src)), node),
            "attribute" => (suppress_message(node, src), node.parent().unwrap_or(node)),
            "string_literal"
            | "verbatim_string_literal"
            | "raw_string_literal"
            | "interpolated_string_expression" => return false,
            _ => return true,
        };
        let line = node.start_position().row + 1;
        for key in keys {
            found.push(Suppression {
                key,
                line,
                text: shown(text(shown_node, src).split('\n').next().unwrap_or_default()),
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
    fn xunit_nunit_and_mstest_tests_are_read() {
        let file = read(
            r#"using Xunit;
using App.Pricing;

namespace App.Tests
{
    public class PricingTests
    {
        [Fact]
        public void Adds()
        {
            Assert.Equal(3, Pricing.Total(1, 2));
        }

        [Test]
        public void NUnitStyle() { Assert.That(Pricing.Total(1, 2), Is.EqualTo(3)); }

        [TestMethod]
        public async Task MsTestStyle() { Assert.AreEqual(3, await Pricing.TotalAsync()); }

        [Xunit.Fact]
        public void Qualified() { Xunit.Assert.True(ok); }

        [FactAttribute]
        public void WithSuffix() { Assert.NotNull(x); }

        public void Helper() { Assert.True(y); }
    }
}
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(
            ids(&file),
            [
                ("PricingTests::Adds", false, None, 1),
                ("PricingTests::NUnitStyle", false, None, 1),
                ("PricingTests::MsTestStyle", false, None, 1),
                ("PricingTests::Qualified", false, None, 1),
                ("PricingTests::WithSuffix", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 9);
        assert!(file.tests[0].uses.contains("Pricing"));
        assert_eq!(
            shapes(&file),
            [
                "Assert.Equal(3,Pricing.Total(1,2))",
                "Assert.That(Pricing.Total(1,2),Is.EqualTo(3))",
                "Assert.AreEqual(3,awaitPricing.TotalAsync())",
                "Xunit.Assert.True(ok)",
                "Assert.NotNull(x)",
                "Assert.True(y)",
            ]
        );
        assert_eq!(file.assertions[0].line, 11);
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(file.assertions[5].test, None);
        assert_eq!(
            file.assertions[1].text,
            "Assert.That(Pricing.Total(1, 2), Is.EqualTo(3))"
        );
        assert_eq!(file.imports.get("Xunit").map(String::as_str), Some("Xunit"));
        assert_eq!(
            file.imports.get("Pricing").map(String::as_str),
            Some("App.Pricing")
        );
    }

    #[test]
    fn ids_name_nested_classes_without_the_namespace() {
        let file = read(
            r#"namespace App.Tests;
public class Outer
{
    public class Inner<T>
    {
        [Fact] public void Deep() { Assert.True(a); }
    }
    [Fact] public void Shallow() { Assert.True(a); }
}
public record struct R { [Fact] public void InRecord() { Assert.True(a); } }
"#,
        );
        let names: Vec<&str> = file.tests.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            names,
            ["Outer::Inner::Deep", "Outer::Shallow", "R::InRecord"]
        );
    }

    #[test]
    fn rows_weigh_a_test() {
        let file = read(
            r#"public class T
{
    [Theory]
    [InlineData(1, 2)]
    [InlineData(2, 4)]
    [InlineData(3, 6, Skip = "flaky")]
    public void Doubles(int a, int b) { Assert.Equal(b, a * 2); }

    [TestCase(1)]
    [TestCase(2, Ignore = "later")]
    [TestCase(3, Explicit = true)]
    public void Positive(int n) { Assert.That(n, Is.Positive); }

    [DataTestMethod]
    [DataRow(1)]
    [DataRow(2)]
    public void Rows(int n) { Assert.IsTrue(n > 0); }

    [Theory, MemberData(nameof(Cases))]
    public void FromMember(int n) { Assert.True(n > 0); }

    [TestCaseSource(nameof(Cases))]
    public void FromSource(int n) { Assert.That(n > 0); }
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::Doubles", false, None, 3),
                ("T::Positive", false, None, 3),
                ("T::Rows", false, None, 2),
                ("T::FromMember", false, None, 1),
                ("T::FromSource", false, None, 1),
            ]
        );
        let skipped: Vec<usize> = file.tests.iter().map(|t| t.skipped_rows).collect();
        assert_eq!(skipped, [1, 2, 0, 0, 0]);
        let weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(weights, [3, 3, 2, 1, 1]);
    }

    #[test]
    fn every_kind_of_skip() {
        let file = read(
            r#"public class Skips
{
    [Fact(Skip = "later")] public void XunitSkip() { Assert.True(a); }
    [Theory(Skip = "later")] [InlineData(1)] public void TheorySkip(int n) { Assert.True(a); }
    [Fact(Skip = null)] public void NullSkipRuns() { Assert.True(a); }
    [Test, Ignore("later")] public void NUnitIgnore() { Assert.True(a); }
    [Test] [Explicit] public void NUnitExplicit() { Assert.True(a); }
    [TestMethod, Ignore] public void MsTestIgnore() { Assert.IsTrue(a); }
    [Fact(Explicit = true)] public void XunitExplicit() { Assert.True(a); }
    [Test] public void IgnoredFirst() { Assert.Ignore("later"); Assert.True(a); }
    [Test] public void InconclusiveFirst() { Assert.Inconclusive(); Assert.True(a); }
    [Test] public void PassFirst() { Assert.Pass(); Assert.True(a); }
    [SkippableFact] public void SkipIfTrue() { Skip.If(true); Assert.True(a); }
    [SkippableFact] public void SkipIfCondition() { Skip.If(OnWindows); Assert.True(a); }
    [Fact] public void Empty() { }
    [Fact] public void OnlyComments() { /* later */ ; }
    [Fact] public void ReturnsFirst() { return; Assert.True(a); }
    [Fact] public void Arrow() => Assert.True(a);
}
[TestFixture, Ignore("broken")]
public class IgnoredFixture
{
    [Test] public void Inside() { Assert.True(a); }
    public class Nested { [Test] public void NotIgnored() { Assert.True(a); } }
}
[TestClass]
[Ignore]
public class IgnoredClass { [TestMethod] public void Inside() { Assert.IsTrue(a); } }
[TestFixture(IgnoreReason = "broken")]
public class IgnoredByReason { [Test] public void Inside() { Assert.True(a); } }
"#,
        );
        let skip = Some("it calls skip first");
        let nothing = Some("its body does nothing");
        assert_eq!(
            ids(&file),
            [
                ("Skips::XunitSkip", true, None, 1),
                ("Skips::TheorySkip", true, None, 1),
                ("Skips::NullSkipRuns", false, None, 1),
                ("Skips::NUnitIgnore", true, None, 1),
                ("Skips::NUnitExplicit", true, None, 1),
                ("Skips::MsTestIgnore", true, None, 1),
                ("Skips::XunitExplicit", true, None, 1),
                ("Skips::IgnoredFirst", true, skip, 1),
                ("Skips::InconclusiveFirst", true, skip, 1),
                ("Skips::PassFirst", true, skip, 1),
                ("Skips::SkipIfTrue", true, skip, 1),
                ("Skips::SkipIfCondition", false, None, 1),
                ("Skips::Empty", true, nothing, 1),
                ("Skips::OnlyComments", true, nothing, 1),
                ("Skips::ReturnsFirst", true, nothing, 1),
                ("Skips::Arrow", false, None, 1),
                ("IgnoredFixture::Inside", true, None, 1),
                ("IgnoredFixture::Nested::NotIgnored", false, None, 1),
                ("IgnoredClass::Inside", true, None, 1),
                ("IgnoredByReason::Inside", true, None, 1),
            ]
        );
        // After a skip or a return the assertions never run.
        let lines: Vec<usize> = file.assertions.iter().map(|a| a.line).collect();
        assert_eq!(lines, [3, 4, 5, 6, 7, 8, 9, 14, 18, 23, 24, 28, 30]);
    }

    #[test]
    fn preprocessor_conditions_turn_tests_off() {
        let file = read(
            r#"public class P
{
#if false
    [Fact] public void Never() { Assert.True(a); }
#else
    [Fact] public void Instead() { Assert.True(b); }
#endif
#if DEBUG
    [Fact] public void OnlyInDebug() { Assert.True(c); }
#elif false
    [Fact] public void NeverEither() { Assert.True(d); }
#endif
#if true
    [Fact] public void Always() { Assert.True(e); }
#endif
    [Fact]
    public void InsideABody()
    {
#if false
        Assert.True(f);
#endif
        Assert.True(g);
    }
    [Fact]
    public void BodyOff()
    {
#if false
        Assert.True(h);
#endif
    }
}
"#,
        );
        assert!(!file.has_errors);
        let nothing = Some("its body does nothing");
        assert_eq!(
            ids(&file),
            [
                ("P::Never", true, Some(NEVER_COMPILED), 1),
                ("P::Instead", false, None, 1),
                ("P::OnlyInDebug", true, None, 1),
                ("P::NeverEither", true, Some(NEVER_COMPILED), 1),
                ("P::Always", false, None, 1),
                ("P::InsideABody", false, None, 1),
                ("P::BodyOff", true, nothing, 1),
            ]
        );
        assert_eq!(
            shapes(&file),
            [
                "Assert.True(b)",
                "Assert.True(c)",
                "Assert.True(e)",
                "Assert.True(g)"
            ]
        );
    }

    #[test]
    fn assertion_libraries() {
        let file = read(
            r#"public class A
{
    [Fact]
    public async Task All()
    {
        Assert.Throws<ArgumentException>(() => Run());
        await Assert.ThrowsAsync<X>(() => RunAsync());
        Assert.Fail("never here");
        CollectionAssert.AreEqual(a, b);
        StringAssert.Contains("x", s);
        ClassicAssert.AreEqual(1, n);
        result.Should().Be(3);
        list.Should().NotBeNull().And.HaveCount(2);
        await act.Should().ThrowAsync<Exception>();
        value.ShouldBe(2);
        Should.Throw<X>(() => Run());
        mock.Verify(m => m.Save(), Times.Once);
        mock.VerifyAll();
        await Verify(result);
        sub.Received().Add(1, 2);
        A.CallTo(() => f.Run()).MustHaveHappenedOnceExactly();
        Assert.Multiple(() =>
        {
            Assert.That(a, Is.EqualTo(1));
            Assert.That(b, Is.EqualTo(2));
        });
        Assert.Warn("only a warning");
        x.Should();
        policy.ShouldRetry(3);
        mock.Setup(m => m.Run()).Verifiable();
        signer.VerifySignature(token);
        Debug.Assert(ok);
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "Assert.Throws<ArgumentException>(()=>Run())",
                "Assert.ThrowsAsync<X>(()=>RunAsync())",
                "Assert.Fail(\"neverhere\")",
                "CollectionAssert.AreEqual(a,b)",
                "StringAssert.Contains(\"x\",s)",
                "ClassicAssert.AreEqual(1,n)",
                "result.Should().Be(3)",
                "list.Should().NotBeNull().And.HaveCount(2)",
                "act.Should().ThrowAsync<Exception>()",
                "value.ShouldBe(2)",
                "Should.Throw<X>(()=>Run())",
                "mock.Verify(m=>m.Save(),Times.Once)",
                "mock.VerifyAll()",
                "Verify(result)",
                "sub.Received().Add(1,2)",
                "A.CallTo(()=>f.Run()).MustHaveHappenedOnceExactly()",
                "Assert.That(a,Is.EqualTo(1))",
                "Assert.That(b,Is.EqualTo(2))",
            ]
        );
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"public class C
{
    [Fact]
    public void T()
    {
        Assert.True(true);
        Assert.True(true, "message");
        Assert.IsTrue(true);
        Assert.False(false);
        Assert.Equal(1, 1);
        Assert.AreEqual("a", "a");
        Assert.Null(null);
        Assert.That(true);
        Assert.That(true, Is.True);
        Assert.That(1, Is.EqualTo(1));
        true.Should().BeTrue();
        1.Should().Be(1);
        true.ShouldBeTrue();
        2.ShouldBe(2);
        Assert.True(ok);
        Assert.Equal(1, n);
        Assert.Equal(1, 2);
        Assert.False(true);
        Assert.That(1, Is.EqualTo(2));
        true.Should().BeFalse();
        Assert.Fail();
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "Assert.True(ok)",
                "Assert.Equal(1,n)",
                "Assert.Equal(1,2)",
                "Assert.False(true)",
                "Assert.That(1,Is.EqualTo(2))",
                "true.Should().BeFalse()",
                "Assert.Fail()",
            ]
        );
    }

    #[test]
    fn assertions_that_never_run_are_not_counted() {
        let file = read(
            r#"public class N
{
    [Fact]
    public void T()
    {
        if (false) { Assert.True(a); }
        while (false) { Assert.True(b); }
        if (true) { Assert.True(c); }
        Func<int> f = () => { return 1; };
        Assert.True(d);
        return;
        Assert.True(e);
    }
    [Test]
    public void SkipsHalfway()
    {
        Assert.True(f);
        Assert.Ignore("rest is broken");
        Assert.True(g);
    }
}
"#,
        );
        assert_eq!(
            shapes(&file),
            ["Assert.True(c)", "Assert.True(d)", "Assert.True(f)"]
        );
        assert!(!file.tests[0].skipped);
        assert!(!file.tests[1].skipped);
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r##"public class L
{
    // [Fact] public void Commented() { Assert.True(x); }
    const string S = "[Fact] public void InAString() { Assert.True(x); }";
    [Obsolete] public void NotATest() { var s = @"Assert.True(x)"; }
    [Fact] public string Property { get; set; }
}
"##,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file =
            read("public class T {\n    [Fact]\n    public void A() {\n        Assert.True(x);\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let source = format!(
            "public class T {{ [Fact] public void A() {{ {} Assert.True(x); {} }} }}\n",
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
            "tests/App.Tests/PricingTests.cs",
            "src/App.Tests/Pricing.cs",
            "src/App.UnitTests/Helpers/Cart.cs",
            "test/Pricing.cs",
            "Tests/Pricing.cs",
            "App.Test/Pricing.cs",
            "src/PricingTests.cs",
            "src/PricingTest.cs",
            "IntegrationTests/Api.CS",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "src/App/Pricing.cs",
            "src/Contests/Pricing.cs",
            "src/Latest.cs",
            "src/App.Tests/Pricing.ts",
            "tests/.cs",
            "testing/Pricing.cs",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_looks_for_test_attributes() {
        for source in [
            "[Fact]\npublic void A() {}",
            "[ Theory ]",
            "[Xunit.Fact]",
            "[TestMethod]",
            "[TestClass]",
            "[TestFixture, Ignore]",
            "[TestCase(1)]",
            "[NUnit.Framework.TestAttribute]",
        ] {
            assert!(holds_tests(source), "{source}");
        }
        for source in [
            "public class A { int[] xs = new int[3]; }",
            "[Obsolete]",
            "[Testing]",
            "",
        ] {
            assert!(!holds_tests(source), "{source}");
        }
    }

    #[test]
    fn suppressions_are_read_from_pragmas_attributes_and_comments() {
        let source = r##"[assembly: SuppressMessage("Style", "IDE0001:Simplify names")]
#pragma warning disable CS0168, IDE0059 // unused on purpose
#pragma warning disable
#pragma warning restore CS0168
#nullable disable
#nullable enable
public class A
{
    [SuppressMessage("Performance", "CA1822:Mark members as static", Justification = "x")]
    public void B() { }
    [System.Diagnostics.CodeAnalysis.SuppressMessageAttribute("Usage", "CA2201")]
    public void C() { }
    [UnconditionalSuppressMessage("Trimming", "IL2026")]
    public void D() { }
    [SuppressMessage("Usage", CheckId = "CA1000")]
    public void E() { }
    public void F()
    {
        // ReSharper disable once UnusedVariable
        var x = 1;
        // ReSharper disable InconsistentNaming, UnusedMember.Local
        // ReSharper disable All
        // ReSharper restore InconsistentNaming
    }
}
"##;
        assert_eq!(
            keys(source),
            [
                "suppress-message:IDE0001",
                "pragma-disable:CS0168",
                "pragma-disable:IDE0059",
                "pragma-disable:*",
                "nullable-disable:*",
                "suppress-message:CA1822",
                "suppress-message:CA2201",
                "suppress-message:IL2026",
                "suppress-message:CA1000",
                "resharper:UnusedVariable",
                "resharper:InconsistentNaming",
                "resharper:UnusedMember.Local",
                "resharper:*",
            ]
        );
        let found = suppressions(source);
        assert_eq!(found[1].line, 2);
        assert_eq!(
            found[1].text,
            "#pragma warning disable CS0168, IDE0059 // unused on purpose"
        );
        assert_eq!(
            found[5].text,
            "[SuppressMessage(\"Performance\", \"CA1822:Mark members as static\", Justification = \"x\")]"
        );
    }

    #[test]
    fn look_alikes_are_not_suppressions() {
        let source = r##"public class A
{
    // Do not add #pragma warning disable here.
    // We use ReSharper disable comments nowhere.
    // ReSharper disabled is not a directive
    const string S = "#pragma warning disable CS0168";
    const string T = @"// ReSharper disable Foo";
    const string U = """
        [SuppressMessage("a", "CA1822")]
        """;
    [Obsolete("SuppressMessage")]
    public void B() { }
}
"##;
        assert!(keys(source).is_empty());
    }
}
