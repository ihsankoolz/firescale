//! The Go reader of the tamper guard (M32, ADR 0052): the `testing`
//! package, testify, gotest.tools, quicktest and gomega.
//!
//! - A test is a top-level `func TestXxx(t *testing.T)` (`Test` then the
//!   end of the name or a character that is not a lower-case letter, as
//!   `go test` reads it; not `TestMain(m *testing.M)`), and a
//!   `func ExampleXxx()` with an `// Output:` comment. Benchmarks and fuzz
//!   targets are not tests. A method named `Test…` of a type that embeds
//!   testify's `suite.Suite` is a test too (`MySuite/TestX`).
//! - A subtest with a literal name, `t.Run("adds", func(t *testing.T) {
//!   … })` (or a suite's `s.Run("adds", func() { … })`), is a test of its
//!   own with the id `TestX/adds`; the test around it stays one. A test is
//!   weighed by the rows of its tables: a `for … range` loop whose body runs
//!   a subtest (`t.Run(tc.name, …)`) over a composite literal (`tests :=
//!   []struct{…}{{…}, {…}}` in the same function or the file, or written in
//!   the loop) has its rows; any other, one. A loop that only checks
//!   (`for _, f := range files { … t.Fatal(err) }`) is not read as a table:
//!   it is as often setup as cases.
//! - It is skipped when its first statement (after `t.Parallel()` and
//!   `t.Helper()`) is `t.Skip(…)`, `t.Skipf(…)` or `t.SkipNow()` (one inside
//!   `if testing.Short() { … }` is not: the test runs), when its body does
//!   nothing, when the test around a subtest is, and when the file has a
//!   build constraint (`//go:build integration`: compiled only sometimes,
//!   a conditional skip; `//go:build ignore`: never).
//! - An assertion is a call of `t.Error`, `t.Errorf`, `t.Fatal`,
//!   `t.Fatalf`, `t.Fail` or `t.FailNow` on a `*testing.T` or `testing.TB`
//!   parameter or `s.T()` (each call site counts, although it sits inside
//!   `if got != want { … }`: that is how a Go test fails); any call of a
//!   testify or gotest.tools `assert` or `require` package but `New`, of an
//!   object made by `assert.New(t)`, of a suite's own assertion methods
//!   (`s.Equal(…)`, `s.Require().NoError(…)`); quicktest's `qt.Assert` and
//!   `qt.Check`; gomega's `Expect(…).To(…)`, `Ω(…).Should(…)`. Not one that
//!   never runs (`if false`, after a `return` or an unconditional skip, in
//!   a file under `//go:build ignore`) or cannot fail (`assert.True(t, true)`, `assert.Equal(t, 1, 1)`,
//!   `Expect(true).To(BeTrue())`).
//! - Imports: each import's name and path. A test file's subject is the
//!   file it is named after (`pricing_test.go` tests `pricing.go`): the
//!   names it uses that it does not declare (and, in an external
//!   `pricing_test` package, those it takes through its import of
//!   `…/pricing`: `Total` in `pricing.Total`) map to [`PACKAGE`], which
//!   [`resolve`] turns into that file. The guard counts only the names that
//!   file declared ([`top_level_names`]), so a test of another file of the
//!   package is not excused by deleting this one.
//! - Suppressions: `//nolint` (`nolint:*`), `//nolint:errcheck,gosec`
//!   (`nolint:errcheck`, `nolint:gosec`), staticcheck's `//lint:ignore
//!   SA1019 reason` (`lint-ignore:SA1019`) and `//lint:file-ignore`
//!   (`lint-file-ignore:SA1019`), gosec's `// #nosec G101` (`nosec:G101`,
//!   `nosec:*` when it names none), and revive's `//revive:disable…`.

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::recover;
use super::{Assertion, Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// The import spec of a name that comes from the package under test: the
/// file a test file is named after ([`resolve`]).
pub(super) const PACKAGE: &str = "go:package";

/// What the Go grammar calls the nodes [`reach`] looks at. A Go block
/// holds its statements in a `statement_list`.
const GRAMMAR: Grammar = Grammar {
    blocks: &["statement_list"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &[],
    hoisted: &["function_declaration", "method_declaration"],
    stops: Some(stops),
    noop: is_noop,
};

/// How deep a chain of calls is followed: real code never comes near it.
const MAX_CHAIN: usize = 32;

/// The methods of `*testing.T` that fail the test.
const FAILS: [&str; 6] = ["Error", "Errorf", "Fatal", "Fatalf", "Fail", "FailNow"];

/// The methods of `*testing.T` that skip the test.
const SKIPS: [&str; 3] = ["Skip", "Skipf", "SkipNow"];

/// Methods of a testify suite that are not assertions.
const SUITE_METHODS: [&str; 12] = [
    "T",
    "Run",
    "Require",
    "Assert",
    "SetT",
    "SetS",
    "Suite",
    "SetupSuite",
    "SetupTest",
    "TearDownTest",
    "TearDownSuite",
    "Cleanup",
];

/// gomega's calls that start a check, and the calls that end one.
const GOMEGA_SUBJECTS: [&str; 7] = [
    "Expect",
    "Ω",
    "Eventually",
    "Consistently",
    "ExpectWithOffset",
    "EventuallyWithOffset",
    "ConsistentlyWithOffset",
];
const GOMEGA_CHECKS: [&str; 5] = ["To", "ToNot", "NotTo", "Should", "ShouldNot"];

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 12] = [
    "int_literal",
    "float_literal",
    "imaginary_literal",
    "rune_literal",
    "interpreted_string_literal",
    "interpreted_string_literal_content",
    "raw_string_literal",
    "raw_string_literal_content",
    "escape_sequence",
    "true",
    "false",
    "nil",
];

/// Go's predeclared names: never the subject of a test.
const BUILTINS: [&str; 41] = [
    "append",
    "cap",
    "clear",
    "close",
    "complex",
    "copy",
    "delete",
    "imag",
    "len",
    "make",
    "max",
    "min",
    "new",
    "panic",
    "print",
    "println",
    "real",
    "recover",
    "any",
    "bool",
    "byte",
    "comparable",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "_",
];

/// Why `why` says a test under `//go:build ignore` does not run.
const NEVER_BUILT: &str = "under //go:build ignore, which is never built";

/// A statement that does nothing: `;`.
fn is_noop(statement: Node<'_>, _src: &[u8]) -> bool {
    statement.kind() == "empty_statement"
}

/// The call a statement is, if it is only a call.
fn statement_call(statement: Node<'_>) -> Option<Node<'_>> {
    if statement.kind() != "expression_statement" {
        return None;
    }
    statement
        .named_child(0)
        .filter(|call| call.kind() == "call_expression")
}

/// The operand and the method of a call `x.Method(…)`.
fn method_call<'t, 's>(call: Node<'t>, src: &'s [u8]) -> Option<(Node<'t>, &'s str)> {
    let function = call.child_by_field_name("function")?;
    if function.kind() != "selector_expression" {
        return None;
    }
    Some((
        function.child_by_field_name("operand")?,
        text(function.child_by_field_name("field")?, src),
    ))
}

/// The arguments a call passes, in order.
fn arguments(call: Node<'_>) -> Vec<Node<'_>> {
    let Some(list) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|argument| !argument.is_extra())
        .collect()
}

/// True for `x.T()`: a testify suite's `*testing.T`.
fn is_suite_t(operand: Node<'_>, src: &[u8]) -> bool {
    operand.kind() == "call_expression"
        && arguments(operand).is_empty()
        && method_call(operand, src).is_some_and(|(_, name)| name == "T")
}

/// A statement after which the rest of its block does not run: a call of
/// `Skip`, `Skipf` or `SkipNow` on a name or on `s.T()`.
fn stops(statement: Node<'_>, src: &[u8]) -> bool {
    statement_call(statement)
        .and_then(|call| method_call(call, src))
        .is_some_and(|(operand, name)| {
            SKIPS.contains(&name) && (operand.kind() == "identifier" || is_suite_t(operand, src))
        })
}

/// The statements of a block, comments left out: none when it is empty.
fn statements(block: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = block.walk();
    let list = block
        .named_children(&mut cursor)
        .find(|child| child.kind() == "statement_list");
    list
}

/// True for a function body that does nothing: empty, comments only, or
/// statements that do nothing before a `return`.
fn does_nothing(body: Node<'_>, src: &[u8]) -> bool {
    statements(body).is_none_or(|list| reach::does_nothing(list, src, &GRAMMAR))
}

/// True for a body whose first statement, after `t.Parallel()` and
/// `t.Helper()`, skips.
fn skips_first(body: Node<'_>, src: &[u8]) -> bool {
    let Some(list) = statements(body) else {
        return false;
    };
    let mut cursor = list.walk();
    let first = list.named_children(&mut cursor).find(|statement| {
        !statement.is_extra()
            && !is_noop(*statement, src)
            && !statement_call(*statement)
                .and_then(|call| method_call(call, src))
                .is_some_and(|(_, name)| matches!(name, "Parallel" | "Helper"))
    });
    first.is_some_and(|statement| stops(statement, src))
}

/// True for a name `go test` runs with `prefix`: the prefix, then the end
/// or a character that is not a lower-case letter (`TestAdd`, `Test_add`,
/// `Test`; not `Testing`).
fn named_like(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_lowercase()))
}

/// The type of a parameter without pointers or white space
/// (`testing.T` for `*testing.T`).
fn type_name(parameter: Node<'_>, src: &[u8]) -> String {
    parameter
        .child_by_field_name("type")
        .map(|kind| {
            text(kind, src)
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '*')
                .collect()
        })
        .unwrap_or_default()
}

/// The parameter declarations of a function, method or literal.
fn parameters(function: Node<'_>) -> Vec<Node<'_>> {
    let Some(list) = function.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "parameter_declaration" | "variadic_parameter_declaration"
            )
        })
        .collect()
}

/// True for a top-level function `go test` runs as a test: `TestXxx`
/// (not `TestMain(m *testing.M)`), or an `ExampleXxx()` with an output
/// comment.
fn is_test_function(function: Node<'_>, src: &[u8]) -> bool {
    let Some(name) = function.child_by_field_name("name").map(|n| text(n, src)) else {
        return false;
    };
    if named_like(name, "Test") {
        let main = name == "TestMain"
            && parameters(function)
                .first()
                .is_some_and(|parameter| type_name(*parameter, src).ends_with("testing.M"));
        return !main;
    }
    if name.starts_with("Example") && parameters(function).is_empty() {
        let mut output = false;
        if let Some(body) = function.child_by_field_name("body") {
            walk(body, |node| {
                if node.kind() == "comment" {
                    let comment = text(node, src)
                        .trim_start_matches('/')
                        .trim_start()
                        .to_ascii_lowercase();
                    output |=
                        comment.starts_with("output:") || comment.starts_with("unordered output:");
                }
                !output
            });
        }
        return output;
    }
    false
}

/// What a file says about the names its tests use.
#[derive(Default)]
struct Names {
    /// Parameters of type `*testing.T` or `testing.TB`.
    t: BTreeSet<String>,
    /// The local names of testify's and gotest.tools' `assert` and
    /// `require` packages.
    asserts: BTreeSet<String>,
    /// The local names of quicktest.
    quicktest: BTreeSet<String>,
    /// Variables assigned `assert.New(t)` or `require.New(t)`.
    objects: BTreeSet<String>,
    /// Types that embed testify's `suite.Suite`.
    suites: BTreeSet<String>,
    /// Methods the file declares, on any type: a suite's own helpers are
    /// not assertions.
    methods: BTreeSet<String>,
}

/// The name an import is used by: its alias, or the last part of its path
/// without a major version (`yaml` for `gopkg.in/yaml.v3`, `chi` for
/// `github.com/go-chi/chi/v5`).
fn import_name(spec: Node<'_>, path: &str, src: &[u8]) -> String {
    if let Some(alias) = spec.child_by_field_name("name") {
        return text(alias, src).to_owned();
    }
    let last = import_name_bare(path);
    match last.rsplit_once('.') {
        Some((stem, version)) if is_version(version) => stem.to_owned(),
        _ => last,
    }
}

/// The path of an import, without its quotes.
fn import_path(spec: Node<'_>, src: &[u8]) -> Option<String> {
    let path = spec.child_by_field_name("path")?;
    Some(text(path, src).trim_matches(['"', '`']).to_owned())
}

/// The import specs of a file.
fn import_specs(root: Node<'_>) -> Vec<Node<'_>> {
    let mut found = Vec::new();
    walk(root, |node| match node.kind() {
        "source_file" | "import_declaration" | "import_spec_list" => true,
        "import_spec" => {
            found.push(node);
            false
        }
        _ => false,
    });
    found
}

impl Names {
    /// Reads the names of a file.
    fn of(root: Node<'_>, src: &[u8]) -> Self {
        let mut names = Self::default();
        for spec in import_specs(root) {
            let Some(path) = import_path(spec, src) else {
                continue;
            };
            let name = import_name(spec, &path, src);
            match import_name_bare(&path).as_str() {
                "assert" | "require" => {
                    names.asserts.insert(name);
                }
                "quicktest" => {
                    names.quicktest.insert(name);
                }
                _ => {}
            }
        }
        walk(root, |node| {
            match node.kind() {
                "parameter_declaration" => {
                    if matches!(type_name(node, src).as_str(), "testing.T" | "testing.TB") {
                        let mut cursor = node.walk();
                        for name in node.children_by_field_name("name", &mut cursor) {
                            names.t.insert(text(name, src).to_owned());
                        }
                    }
                }
                "short_var_declaration" | "assignment_statement" => {
                    let left = node.child_by_field_name("left");
                    let right = node.child_by_field_name("right");
                    if let (Some(left), Some(right)) = (left, right) {
                        let made = right
                            .named_child(0)
                            .filter(|call| call.kind() == "call_expression")
                            .and_then(|call| method_call(call, src))
                            .is_some_and(|(package, name)| {
                                name == "New" && names.asserts.contains(text(package, src))
                            });
                        if made {
                            if let Some(target) = left.named_child(0) {
                                names.objects.insert(text(target, src).to_owned());
                            }
                        }
                    }
                }
                "type_spec" => {
                    let embeds = node
                        .child_by_field_name("type")
                        .filter(|kind| kind.kind() == "struct_type")
                        .is_some_and(|kind| {
                            let mut found = false;
                            walk(kind, |inner| {
                                if inner.kind() == "field_declaration"
                                    && inner.child_by_field_name("name").is_none()
                                {
                                    found |= type_name(inner, src) == "suite.Suite";
                                }
                                inner.kind() != "field_declaration"
                            });
                            found
                        });
                    if embeds {
                        if let Some(name) = node.child_by_field_name("name") {
                            names.suites.insert(text(name, src).to_owned());
                        }
                    }
                }
                "method_declaration" => {
                    if let Some(name) = node.child_by_field_name("name") {
                        names.methods.insert(text(name, src).to_owned());
                    }
                }
                _ => {}
            }
            true
        });
        names
    }
}

/// The name and the type of a method's receiver (`s`, `MySuite` for
/// `(s *MySuite)`).
fn receiver(method: Node<'_>, src: &[u8]) -> Option<(String, String)> {
    let list = method.child_by_field_name("receiver")?;
    let mut cursor = list.walk();
    let parameter = list
        .named_children(&mut cursor)
        .find(|child| child.kind() == "parameter_declaration")?;
    let name = parameter
        .child_by_field_name("name")
        .map(|name| text(name, src).to_owned())
        .unwrap_or_default();
    Some((name, type_name(parameter, src)))
}

/// How a call is an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// `t.Errorf(…)` and the like.
    Fails,
    /// `assert.Equal(t, …)`: the checked values start at the second
    /// argument.
    Package,
    /// `a.Equal(…)`, `s.Equal(…)`: they start at the first.
    Object,
    /// `Expect(…).To(…)`.
    Gomega,
}

/// How a call is an assertion, or `None`. `suite` is the receiver of the
/// suite method it is in, if any.
fn assertion(call: Node<'_>, src: &[u8], names: &Names, suite: Option<&str>) -> Option<Check> {
    let (operand, method) = method_call(call, src)?;
    let exported = method.chars().next().is_some_and(char::is_uppercase);
    if operand.kind() == "identifier" {
        let operand = text(operand, src);
        if FAILS.contains(&method) && names.t.contains(operand) {
            return Some(Check::Fails);
        }
        if names.asserts.contains(operand) && exported && method != "New" {
            return Some(Check::Package);
        }
        if names.quicktest.contains(operand) && matches!(method, "Assert" | "Check") {
            return Some(Check::Package);
        }
        if names.objects.contains(operand) && exported {
            return Some(Check::Object);
        }
        if suite == Some(operand)
            && exported
            && !SUITE_METHODS.contains(&method)
            && !names.methods.contains(method)
        {
            return Some(Check::Object);
        }
    }
    if operand.kind() == "call_expression" {
        if FAILS.contains(&method) && is_suite_t(operand, src) {
            return Some(Check::Fails);
        }
        // `s.Require().NoError(err)`, `s.Assert().Equal(…)`.
        let on_suite = method_call(operand, src).is_some_and(|(inner, name)| {
            matches!(name, "Require" | "Assert")
                && inner.kind() == "identifier"
                && suite == Some(text(inner, src))
        });
        if on_suite && exported {
            return Some(Check::Object);
        }
    }
    if GOMEGA_CHECKS.contains(&method) && gomega_subject(operand, src).is_some() {
        return Some(Check::Gomega);
    }
    None
}

/// For gomega: the call that starts the check (`Expect(x)`, `g.Expect(x)`,
/// `Eventually(f).WithTimeout(t)`'s `Eventually(f)`), found through the
/// receivers of a chain.
fn gomega_subject<'t>(operand: Node<'t>, src: &[u8]) -> Option<Node<'t>> {
    let mut at = operand;
    for _ in 0..MAX_CHAIN {
        if at.kind() != "call_expression" {
            return None;
        }
        let function = at.child_by_field_name("function")?;
        let name = match function.kind() {
            "identifier" => text(function, src),
            "selector_expression" => text(function.child_by_field_name("field")?, src),
            _ => return None,
        };
        if GOMEGA_SUBJECTS.contains(&name) {
            return Some(at);
        }
        at = function.child_by_field_name("operand")?;
    }
    None
}

/// The source text of a literal, without white space, or `None` when the
/// node is not a literal.
fn literal(node: Node<'_>, src: &[u8]) -> Option<String> {
    reach::constant(node, &LITERALS).then(|| text(node, src).split_whitespace().collect())
}

/// True for a check that certainly passes: `True` of `true`, `False` of
/// `false`, `Nil` of `nil`, `Equal` of two literals written the same.
fn passes(method: &str, subject: Option<String>, expected: Option<String>) -> bool {
    let Some(subject) = subject else {
        return false;
    };
    match method {
        "True" | "BeTrue" => subject == "true",
        "False" | "BeFalse" => subject == "false",
        "Nil" | "BeNil" => subject == "nil",
        "Equal" | "EqualValues" | "Exactly" => expected.is_some_and(|expected| expected == subject),
        _ => false,
    }
}

/// True for an assertion that cannot fail: `assert.True(t, true)`,
/// `assert.Equal(t, 1, 1)`, `s.Equal(1, 1)`, `Expect(true).To(BeTrue())`,
/// `Expect(1).To(Equal(1))`. One that certainly fails counts like any
/// other.
fn cannot_fail(call: Node<'_>, check: Check, src: &[u8]) -> bool {
    let Some((operand, method)) = method_call(call, src) else {
        return false;
    };
    let args = arguments(call);
    let literal_at = |index: usize| args.get(index).and_then(|node| literal(*node, src));
    match check {
        Check::Fails => false,
        Check::Package => passes(method, literal_at(1), literal_at(2)),
        Check::Object => passes(method, literal_at(0), literal_at(1)),
        Check::Gomega => {
            if !matches!(method, "To" | "Should") {
                return false;
            }
            let Some(subject) = gomega_subject(operand, src) else {
                return false;
            };
            if subject.child_by_field_name("function").map(|f| f.kind()) != Some("identifier") {
                return false;
            }
            let subject = arguments(subject)
                .first()
                .and_then(|node| literal(*node, src));
            let Some(matcher) = args.first().filter(|m| m.kind() == "call_expression") else {
                return false;
            };
            let Some(name) = matcher
                .child_by_field_name("function")
                .filter(|f| f.kind() == "identifier")
                .map(|f| text(f, src))
            else {
                return false;
            };
            let expected = arguments(*matcher)
                .first()
                .and_then(|node| literal(*node, src));
            passes(name, subject, expected)
        }
    }
}

/// For `t.Run(name, func(t *testing.T) { … })` (or a suite's
/// `s.Run(name, func() { … })`): its name node and its function literal.
fn subtest<'t>(
    call: Node<'t>,
    src: &[u8],
    names: &Names,
    suite: Option<&str>,
) -> Option<(Node<'t>, Node<'t>)> {
    let (operand, method) = method_call(call, src)?;
    if method != "Run" {
        return None;
    }
    let on_t = (operand.kind() == "identifier" && names.t.contains(text(operand, src)))
        || is_suite_t(operand, src);
    let on_suite = operand.kind() == "identifier" && suite == Some(text(operand, src));
    if !(on_t || on_suite) {
        return None;
    }
    match arguments(call).as_slice() {
        [name, function] if function.kind() == "func_literal" => Some((*name, *function)),
        _ => None,
    }
}

/// The text of a string literal, or `None` for anything else.
fn string_literal(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "interpreted_string_literal" | "raw_string_literal" => {
            let mut content = String::new();
            let mut cursor = node.walk();
            for part in node.named_children(&mut cursor) {
                content.push_str(text(part, src));
            }
            Some(content)
        }
        _ => None,
    }
}

/// The rows of a composite literal: its elements.
fn rows_of(literal: Node<'_>) -> Option<usize> {
    if literal.kind() != "composite_literal" {
        return None;
    }
    let body = literal.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let rows = body
        .named_children(&mut cursor)
        .filter(|child| matches!(child.kind(), "literal_element" | "keyed_element"))
        .count();
    Some(rows)
}

/// The composite literal a name is given, once, by `name := …`, `var name
/// = …` in `scope` or at the top of the file: the table a loop ranges over.
fn table_named<'t>(name: &str, scope: Node<'t>, root: Node<'t>, src: &[u8]) -> Option<Node<'t>> {
    let (mut assigned, mut found) = assignments(name, scope, false, src);
    if assigned.is_empty() {
        (assigned, found) = assignments(name, root, true, src);
    }
    (assigned.len() == 1 && found.len() == 1).then(|| found[0])
}

/// The places `within` gives `name` a value (`name := …`, `name = …`,
/// `var name = …`), and the values. With `top_only`, only the file's own
/// `var` declarations are looked at.
fn assignments<'t>(
    name: &str,
    within: Node<'t>,
    top_only: bool,
    src: &[u8],
) -> (Vec<Node<'t>>, Vec<Node<'t>>) {
    let mut assigned: Vec<Node<'t>> = Vec::new();
    let mut found: Vec<Node<'t>> = Vec::new();
    let named_children = |node: Node<'t>| -> Vec<Node<'t>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter(|child| !child.is_extra())
            .collect()
    };
    walk(within, |node| {
        let (targets, values) = match node.kind() {
            "short_var_declaration" | "assignment_statement" if !top_only => (
                node.child_by_field_name("left").map(named_children),
                node.child_by_field_name("right").map(named_children),
            ),
            "var_spec" => {
                let mut cursor = node.walk();
                let targets: Vec<Node<'t>> = node
                    .children_by_field_name("name", &mut cursor)
                    .filter(|name| name.is_named())
                    .collect();
                (
                    Some(targets),
                    node.child_by_field_name("value").map(named_children),
                )
            }
            _ => (None, None),
        };
        if let Some(index) = targets
            .unwrap_or_default()
            .iter()
            .position(|target| text(*target, src) == name)
        {
            assigned.push(node);
            if let Some(value) = values.unwrap_or_default().get(index) {
                found.push(*value);
            }
        }
        !top_only
            || matches!(
                node.kind(),
                "source_file" | "var_declaration" | "var_spec_list"
            )
    });
    (assigned, found)
}

/// How many cases a test's tables stand for: the rows of each table loop
/// in `body` (a `for … range` over a composite literal whose body runs a
/// subtest), else 1. Subtests with a literal name weigh their own.
fn weight(body: Node<'_>, root: Node<'_>, src: &[u8], names: &Names, suite: Option<&str>) -> usize {
    let mut rows = 0usize;
    walk(body, |node| {
        if node.kind() == "call_expression" {
            return !subtest(node, src, names, suite)
                .is_some_and(|(name, _)| string_literal(name, src).is_some());
        }
        if node.kind() != "for_statement" {
            return true;
        }
        let mut cursor = node.walk();
        let Some(range) = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "range_clause")
        else {
            return true;
        };
        let Some(loop_body) = node.child_by_field_name("body") else {
            return true;
        };
        let mut is_table = false;
        walk(loop_body, |inner| {
            if inner.kind() == "call_expression" {
                is_table |= subtest(inner, src, names, suite).is_some();
            }
            !is_table
        });
        if !is_table {
            return true;
        }
        let table = range.child_by_field_name("right").and_then(|right| {
            if right.kind() == "identifier" {
                table_named(text(right, src), body, root, src)
            } else {
                Some(right)
            }
        });
        rows += table.and_then(rows_of).unwrap_or(1);
        false
    });
    rows.max(1)
}

/// The build constraint of a file, before its package clause: `None`, or
/// whether it is `ignore` (never built).
fn build_constraint(root: Node<'_>, src: &[u8]) -> Option<bool> {
    let mut cursor = root.walk();
    let mut constraint = None;
    for child in root.children(&mut cursor) {
        if child.kind() == "package_clause" {
            break;
        }
        if child.kind() != "comment" {
            continue;
        }
        let comment = text(child, src);
        if let Some(expression) = comment.strip_prefix("//go:build") {
            constraint = Some(expression.trim() == "ignore");
        } else if let Some(tags) = comment.strip_prefix("// +build") {
            constraint = Some(constraint.unwrap_or(false) || tags.trim() == "ignore");
        }
    }
    constraint
}

/// The names declared in a file: functions, types, variables, constants,
/// parameters, and the keys of composite literals (field names).
fn declared(root: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    walk(root, |node| {
        let fields: &[&str] = match node.kind() {
            "function_declaration"
            | "type_spec"
            | "type_alias"
            | "var_spec"
            | "const_spec"
            | "parameter_declaration"
            | "variadic_parameter_declaration"
            | "type_parameter_declaration" => &["name"],
            "short_var_declaration" | "range_clause" | "receive_statement" => &["left"],
            "type_switch_statement" => &["alias"],
            "keyed_element" => &["key"],
            _ => &[],
        };
        for field in fields {
            let mut cursor = node.walk();
            for named in node.children_by_field_name(field, &mut cursor) {
                let inner: Vec<Node<'_>> =
                    if matches!(named.kind(), "expression_list" | "literal_element") {
                        let mut list = named.walk();
                        named.named_children(&mut list).collect()
                    } else {
                        vec![named]
                    };
                for name in inner {
                    if matches!(name.kind(), "identifier" | "type_identifier") {
                        found.insert(text(name, src).to_owned());
                    }
                }
            }
        }
        true
    });
    found
}

/// The identifiers and type names a node uses (not those of another
/// package: `T` in `testing.T`), and the names it takes from the package
/// under test through one of its `tested` imports (`Total` in
/// `pricing.Total(…)`, `Cart` in `pricing.Cart`).
fn uses_of(node: Node<'_>, src: &[u8], tested: &BTreeSet<String>) -> BTreeSet<String> {
    let mut uses = BTreeSet::new();
    let from_tested = |qualifier: Option<Node<'_>>| {
        qualifier.is_some_and(|qualifier| {
            matches!(qualifier.kind(), "identifier" | "package_identifier")
                && tested.contains(text(qualifier, src))
        })
    };
    walk(node, |inner| {
        match inner.kind() {
            "identifier" | "type_identifier" => {
                uses.insert(text(inner, src).to_owned());
            }
            "selector_expression" => {
                if from_tested(inner.child_by_field_name("operand")) {
                    if let Some(field) = inner.child_by_field_name("field") {
                        uses.insert(text(field, src).to_owned());
                    }
                }
            }
            "qualified_type" => {
                if from_tested(inner.child_by_field_name("package")) {
                    if let Some(name) = inner.child_by_field_name("name") {
                        uses.insert(text(name, src).to_owned());
                    }
                }
                return false;
            }
            "comment" | "interpreted_string_literal" | "raw_string_literal" => return false,
            _ => {}
        }
        true
    });
    uses
}

/// The local names of the imports of the package under test, in an
/// external test package: `pricing` (or its alias) in `package
/// pricing_test` importing `…/pricing`.
fn tested_imports(root: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut cursor = root.walk();
    let package = root
        .named_children(&mut cursor)
        .find(|child| child.kind() == "package_clause")
        .and_then(|clause| clause.named_child(0))
        .map(|name| text(name, src).to_owned())
        .unwrap_or_default();
    let Some(tested) = package.strip_suffix("_test") else {
        return BTreeSet::new();
    };
    import_specs(root)
        .into_iter()
        .filter_map(|spec| {
            let path = import_path(spec, src)?;
            (import_name_bare(&path) == tested).then(|| import_name(spec, &path, src))
        })
        .filter(|name| !matches!(name.as_str(), "." | "_"))
        .collect()
}

/// The names Go source declares at its top level: functions, types,
/// variables and constants (not methods). What a test of its package can
/// use from it.
pub(super) fn top_level_names(source: &str) -> BTreeSet<String> {
    let language: Language = tree_sitter_go::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return BTreeSet::new();
    };
    let src = source.as_bytes();
    let mut found = BTreeSet::new();
    walk(tree.root_node(), |node| match node.kind() {
        "source_file" | "type_declaration" | "var_declaration" | "var_spec_list"
        | "const_declaration" => true,
        "function_declaration" | "type_spec" | "type_alias" | "var_spec" | "const_spec" => {
            let mut cursor = node.walk();
            // The commas of `const a, b = …` are in the field too.
            for name in node
                .children_by_field_name("name", &mut cursor)
                .filter(|name| name.is_named())
            {
                found.insert(text(name, src).to_owned());
            }
            false
        }
        _ => false,
    });
    found
}

/// Reads the tests, skips and assertions of a Go test file.
#[must_use]
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_go::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    let dead = reach::Dead::of(root, src, &GRAMMAR);
    let error_line = recover::first_harmful(root, src);
    let mut file = TestFile {
        has_errors: error_line.is_some(),
        error_line,
        ..TestFile::default()
    };
    let names = Names::of(root, src);
    let tested = tested_imports(root, src);
    // `Some(true)`: never built; `Some(false)`: built only sometimes.
    let constraint = build_constraint(root, src);

    /// The functions and subtests around the node being visited.
    struct Scope {
        end: usize,
        /// The test it is, if it is one.
        test: Option<usize>,
        /// The id its literal subtests extend: `None` when they are not
        /// counted (inside a subtest with a name that is not literal).
        path: Option<String>,
        /// The test does not run, so neither do its subtests.
        skipped: bool,
        /// The receiver of a suite method.
        suite: Option<String>,
    }
    let mut open: Vec<Scope> = vec![Scope {
        end: usize::MAX,
        test: None,
        path: None,
        skipped: false,
        suite: None,
    }];
    walk(root, |node| {
        while open.len() > 1
            && open
                .last()
                .is_some_and(|scope| node.start_byte() >= scope.end)
        {
            open.pop();
        }
        match node.kind() {
            "comment"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "import_declaration" => false,
            "function_declaration" | "method_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .map_or("", |name| text(name, src));
                let suite = if node.kind() == "method_declaration" {
                    receiver(node, src).filter(|(_, kind)| names.suites.contains(kind))
                } else {
                    None
                };
                let id = match &suite {
                    Some((_, kind)) => (named_like(name, "Test") && parameters(node).is_empty())
                        .then(|| format!("{kind}/{name}")),
                    None => (node.kind() == "function_declaration" && is_test_function(node, src))
                        .then(|| name.to_owned()),
                };
                let suite = suite.map(|(receiver, _)| receiver);
                let mut scope = Scope {
                    end: node.end_byte(),
                    test: None,
                    path: None,
                    skipped: false,
                    suite: suite.clone(),
                };
                if let (Some(id), Some(body)) = (id, node.child_by_field_name("body")) {
                    let why = if constraint == Some(true) {
                        Some(NEVER_BUILT)
                    } else if skips_first(body, src) {
                        Some("it calls skip first")
                    } else if does_nothing(body, src) {
                        Some("its body does nothing")
                    } else {
                        None
                    };
                    let skipped = constraint.is_some() || why.is_some();
                    scope.test = Some(file.tests.len());
                    scope.path = Some(id.clone());
                    scope.skipped = skipped;
                    file.tests.push(TestCase {
                        id,
                        line: node.start_position().row + 1,
                        skipped,
                        skipped_rows: 0,
                        why,
                        marker: false,
                        focused: false,
                        uses: uses_of(node, src, &tested),
                        weight: weight(body, root, src, &names, suite.as_deref()),
                    });
                }
                open.push(scope);
                true
            }
            "call_expression" => {
                let (test, path, skipped, suite) =
                    open.last().map_or((None, None, false, None), |scope| {
                        (
                            scope.test,
                            scope.path.clone(),
                            scope.skipped,
                            scope.suite.clone(),
                        )
                    });
                if let Some((name, function)) = subtest(node, src, &names, suite.as_deref()) {
                    let mut scope = Scope {
                        end: node.end_byte(),
                        test,
                        path: None,
                        skipped,
                        suite: suite.clone(),
                    };
                    let literal = string_literal(name, src);
                    if let (Some(parent), Some(literal), Some(parent_test)) = (path, literal, test)
                    {
                        let id = format!(
                            "{parent}/{}",
                            literal.split_whitespace().collect::<Vec<_>>().join("_")
                        );
                        let body = function.child_by_field_name("body");
                        let why = if let Some(why) = file.tests[parent_test].why.filter(|_| skipped)
                        {
                            Some(why)
                        } else if let Some(why) = dead.why(node) {
                            Some(why)
                        } else if body.is_some_and(|body| skips_first(body, src)) {
                            Some("it calls skip first")
                        } else if body.is_some_and(|body| does_nothing(body, src)) {
                            Some("its body does nothing")
                        } else {
                            None
                        };
                        let mut uses = uses_of(function, src, &tested);
                        uses.extend(file.tests[parent_test].uses.iter().cloned());
                        scope.test = Some(file.tests.len());
                        scope.path = Some(id.clone());
                        scope.skipped = skipped || why.is_some();
                        file.tests.push(TestCase {
                            id,
                            line: node.start_position().row + 1,
                            skipped: scope.skipped,
                            skipped_rows: 0,
                            why,
                            marker: false,
                            focused: false,
                            uses,
                            weight: body.map_or(1, |body| {
                                weight(body, root, src, &names, suite.as_deref())
                            }),
                        });
                    }
                    open.push(scope);
                    return true;
                }
                let Some(check) = assertion(node, src, &names, suite.as_deref()) else {
                    return true;
                };
                if constraint == Some(true)
                    || dead.why(node).is_some()
                    || cannot_fail(node, check, src)
                {
                    return false;
                }
                let whole = text(node, src);
                file.assertions.push(Assertion {
                    line: node.start_position().row + 1,
                    shape: whole.split_whitespace().collect(),
                    text: whole.split_whitespace().collect::<Vec<_>>().join(" "),
                    weight: test
                        .and_then(|index| file.tests.get(index))
                        .map_or(1, |test| test.weight),
                    test,
                    conditional: false,
                });
                false
            }
            _ => true,
        }
    });
    file.imports = imports(root, src, &tested);
    file
}

/// The names a test file takes from elsewhere, each with where from: every
/// import by its name and path, and the names it uses but does not
/// declare, which come from its own package ([`PACKAGE`]): unqualified in
/// an internal test, `pricing.Total` through the `tested` import of an
/// external test package. The guard counts only those that the file
/// [`PACKAGE`] resolves to declared ([`top_level_names`]).
fn imports(root: Node<'_>, src: &[u8], tested: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for spec in import_specs(root) {
        let Some(path) = import_path(spec, src) else {
            continue;
        };
        let name = import_name(spec, &path, src);
        if !matches!(name.as_str(), "." | "_") {
            found.insert(name, path);
        }
    }
    let declared = declared(root, src);
    for name in uses_of(root, src, tested) {
        if declared.contains(&name)
            || found.contains_key(&name)
            || BUILTINS.contains(&name.as_str())
        {
            continue;
        }
        found.insert(name, PACKAGE.to_owned());
    }
    found
}

/// True for a major version: `v2`, `v10`.
fn is_version(part: &str) -> bool {
    part.strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

/// The last part of an import path without a major version: the name a
/// package is usually declared with.
fn import_name_bare(path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 1 && parts.last().is_some_and(|last| is_version(last)) {
        parts.pop();
    }
    parts.last().copied().unwrap_or_default().to_owned()
}

/// The repo path [`PACKAGE`] names for a test file: the file it is named
/// after (`pricing.go` for `pricing_test.go`), if it exists on the base.
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if spec != PACKAGE {
        return None;
    }
    let stem = from.strip_suffix("_test.go")?;
    let subject = format!("{stem}.go");
    exists(&subject).then_some(subject)
}

/// True for a path `go test` reads as a test file: `*_test.go`, not
/// ignored by the go tool (a name or a folder that starts with `_` or `.`,
/// a `testdata` folder).
#[must_use]
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let ignored = |part: &str| part.starts_with('_') || part.starts_with('.');
    name.strip_suffix("_test.go")
        .is_some_and(|stem| !stem.is_empty())
        && !ignored(name)
        && !folders
            .iter()
            .any(|folder| ignored(folder) || *folder == "testdata")
}

/// Go tests live only in `_test.go` files: other Go source never holds
/// tests the go tool runs.
#[must_use]
pub(super) fn holds_tests(source: &str) -> bool {
    let _ = source;
    false
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

/// The suppressions a Go comment holds. Each directive must open the
/// comment, as its tool requires.
fn comment_keys(raw: &str) -> Vec<String> {
    let (body, line_comment) = match raw.strip_prefix("//") {
        Some(body) => (body, true),
        None => (
            raw.strip_prefix("/*")
                .map_or(raw, |inner| inner.strip_suffix("*/").unwrap_or(inner)),
            false,
        ),
    };
    // golangci-lint: `//nolint`, `//nolint:a,b // why` (a space after the
    // slashes is read too).
    if line_comment {
        if let Some(rest) = body.trim_start().strip_prefix("nolint") {
            return match rest.chars().next() {
                None => keyed("nolint", &[]),
                Some(':') => {
                    let list = rest[1..].split_whitespace().next().unwrap_or_default();
                    let rules: Vec<&str> = list
                        .split(',')
                        .map(|rule| if rule == "all" { "" } else { rule })
                        .collect();
                    keyed("nolint", &rules)
                }
                Some(c) if c.is_whitespace() => keyed("nolint", &[]),
                Some(_) => Vec::new(),
            };
        }
        // staticcheck: `//lint:ignore SA1019 reason`, no space.
        for (form, tool) in [
            ("lint:ignore", "lint-ignore"),
            ("lint:file-ignore", "lint-file-ignore"),
        ] {
            if let Some(rest) = body.strip_prefix(form) {
                if !rest.starts_with(char::is_whitespace) {
                    return Vec::new();
                }
                let checks = rest.split_whitespace().next().unwrap_or_default();
                return keyed(tool, &checks.split(',').collect::<Vec<_>>());
            }
        }
        // revive: `//revive:disable[-line|-next-line][:rule]`.
        if let Some(rest) = body.strip_prefix("revive:disable") {
            let (tool, rest) = if let Some(rest) = rest
                .strip_prefix("-next-line")
                .or_else(|| rest.strip_prefix("-line"))
            {
                ("revive", rest)
            } else {
                ("revive-range", rest)
            };
            return match rest.chars().next() {
                None => keyed(tool, &[]),
                Some(':') => keyed(
                    tool,
                    &[rest[1..].split_whitespace().next().unwrap_or_default()],
                ),
                Some(c) if c.is_whitespace() => keyed(tool, &[]),
                Some(_) => Vec::new(),
            };
        }
    }
    // gosec: `// #nosec G101 G102 -- why`.
    if let Some(rest) = body.trim_start().strip_prefix("#nosec") {
        if rest.chars().next().is_some_and(|c| c.is_alphanumeric()) {
            return Vec::new();
        }
        let rules: Vec<&str> = rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|rule| !rule.is_empty())
            .take_while(|rule| {
                rule.strip_prefix('G').is_some_and(|digits| {
                    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
                })
            })
            .collect();
        return keyed("nosec", &rules);
    }
    Vec::new()
}

/// The lint suppressions in a Go source file, read from its comments: one
/// in a string, or in prose in a comment, is not one.
#[must_use]
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let language: Language = tree_sitter_go::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| match node.kind() {
        "comment" => {
            let raw = text(node, src);
            for key in comment_keys(raw) {
                found.push(Suppression {
                    key,
                    line: node.start_position().row + 1,
                    text: shown(raw),
                });
            }
            false
        }
        "interpreted_string_literal" | "raw_string_literal" | "rune_literal" => false,
        _ => true,
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
    fn test_functions_and_their_t_calls_are_read() {
        let file = read(
            r#"package pricing

import "testing"

func TestTotal(t *testing.T) {
	if got := Total(1, 2); got != 3 {
		t.Errorf("Total(1, 2) = %d, want 3", got)
	}
	if err := Check(); err != nil {
		t.Fatal(err)
	}
}

func Test_lower(t *testing.T) { t.Fail() }

func Test(t *testing.T) { t.FailNow() }

func Testing(t *testing.T) { t.Error("not a test") }

func TestMain(m *testing.M) { m.Run() }

func BenchmarkTotal(b *testing.B) { b.Fatal("x") }

func FuzzTotal(f *testing.F) { f.Fuzz(func(t *testing.T, n int) { t.Error(n) }) }

func helper(tb testing.TB) { tb.Helper(); tb.Errorf("x") }
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(
            ids(&file),
            [
                ("TestTotal", false, None, 1),
                ("Test_lower", false, None, 1),
                ("Test", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 5);
        assert!(file.tests[0].uses.contains("Total"));
        assert_eq!(
            shapes(&file),
            [
                "t.Errorf(\"Total(1,2)=%d,want3\",got)",
                "t.Fatal(err)",
                "t.Fail()",
                "t.FailNow()",
                "t.Error(\"notatest\")",
                "t.Error(n)",
                "tb.Errorf(\"x\")",
            ]
        );
        assert_eq!(file.assertions[0].line, 7);
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(file.assertions[4].test, None);
        assert_eq!(
            file.assertions[0].text,
            "t.Errorf(\"Total(1, 2) = %d, want 3\", got)"
        );
    }

    #[test]
    fn examples_with_output_are_tests() {
        let file = read(
            r#"package pricing_test

func ExampleTotal() {
	fmt.Println(Total(1, 2))
	// Output: 3
}

func ExampleCart_Add() {
	fmt.Println(1)
	// Unordered output:
	// 1
}

func ExampleNoOutput() {
	fmt.Println(1)
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("ExampleTotal", false, None, 1),
                ("ExampleCart_Add", false, None, 1),
            ]
        );
    }

    #[test]
    fn subtests_with_literal_names_are_tests() {
        let file = read(
            r#"package pricing

func TestCart(t *testing.T) {
	cart := NewCart()
	t.Run("adds an item", func(t *testing.T) {
		cart.Add(1)
		if cart.Len() != 1 {
			t.Error("want 1")
		}
		t.Run("nested", func(t *testing.T) { t.Fail() })
	})
	t.Run(`raw`, func(t *testing.T) {
		t.Skip("later")
	})
	t.Run("empty", func(t *testing.T) {})
	name := "dynamic"
	t.Run(name, func(t *testing.T) {
		t.Run("inside", func(t *testing.T) { t.Fail() })
	})
	return
	t.Run("after return", func(t *testing.T) { t.Fail() })
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("TestCart", false, None, 1),
                ("TestCart/adds_an_item", false, None, 1),
                ("TestCart/adds_an_item/nested", false, None, 1),
                ("TestCart/raw", true, Some("it calls skip first"), 1),
                ("TestCart/empty", true, Some("its body does nothing"), 1),
                ("TestCart/after_return", true, Some("after a return"), 1),
            ]
        );
        // A subtest uses what its test set up.
        assert!(file.tests[1].uses.contains("NewCart"));
        let tests: Vec<Option<usize>> = file.assertions.iter().map(|a| a.test).collect();
        // The one inside a subtest with a name that is not literal belongs
        // to the test; the one after the return never runs.
        assert_eq!(tests, [Some(1), Some(2), Some(0)]);
    }

    #[test]
    fn table_tests_weigh_their_rows() {
        let file = read(
            r#"package pricing

var shared = []struct{ in, want int }{{1, 2}, {2, 4}}

func TestTable(t *testing.T) {
	tests := []struct {
		name string
		in   int
		want int
	}{
		{name: "one", in: 1, want: 2},
		{name: "two", in: 2, want: 4},
		{"three", 3, 6},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := Double(tc.in); got != tc.want {
				t.Errorf("got %d", got)
			}
		})
	}
}

func TestMap(t *testing.T) {
	for name, tc := range map[string]int{"a": 1, "b": 2} {
		t.Run(name, func(t *testing.T) { check(t, tc) })
	}
}

func TestShared(t *testing.T) {
	for _, tc := range shared {
		t.Run(fmt.Sprint(tc.in), func(t *testing.T) {
			if Double(tc.in) != tc.want {
				t.Fail()
			}
		})
	}
}

func TestChecksOnly(t *testing.T) {
	for _, name := range []string{"a.txt", "b.txt"} {
		if err := write(name); err != nil {
			t.Fatal(err)
		}
	}
}

func TestNotATable(t *testing.T) {
	for _, s := range []string{"a", "b", "c"} {
		fmt.Println(s)
	}
	t.Fail()
}

func TestGenerated(t *testing.T) {
	for _, tc := range load() {
		t.Run(tc.name, func(t *testing.T) { t.Fail() })
	}
}
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("TestTable", false, None, 3),
                ("TestMap", false, None, 2),
                ("TestShared", false, None, 2),
                ("TestChecksOnly", false, None, 1),
                ("TestNotATable", false, None, 1),
                ("TestGenerated", false, None, 1),
            ]
        );
        let weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(weights, [3, 2, 1, 1, 1]);
    }

    #[test]
    fn every_kind_of_skip() {
        let file = read(
            r#"package pricing

func TestSkip(t *testing.T) { t.Skip("later"); t.Fail() }

func TestSkipf(t *testing.T) {
	t.Parallel()
	t.Skipf("later %d", 1)
	t.Fail()
}

func TestSkipNow(t *testing.T) { t.SkipNow() }

func TestShort(t *testing.T) {
	if testing.Short() {
		t.Skip("slow")
	}
	t.Fail()
}

func TestSkipHalfway(t *testing.T) {
	t.Error("runs")
	t.Skip("rest")
	t.Error("never")
}

func TestEmpty(t *testing.T) {}

func TestComment(t *testing.T) {
	// later
}

func TestReturn(t *testing.T) {
	return
	t.Fail()
}

func TestIfFalse(t *testing.T) {
	if false {
		t.Fail()
	}
	t.Error("runs")
}
"#,
        );
        let skip = Some("it calls skip first");
        let nothing = Some("its body does nothing");
        assert_eq!(
            ids(&file),
            [
                ("TestSkip", true, skip, 1),
                ("TestSkipf", true, skip, 1),
                ("TestSkipNow", true, skip, 1),
                ("TestShort", false, None, 1),
                ("TestSkipHalfway", false, None, 1),
                ("TestEmpty", true, nothing, 1),
                ("TestComment", true, nothing, 1),
                ("TestReturn", true, nothing, 1),
                ("TestIfFalse", false, None, 1),
            ]
        );
        assert_eq!(
            shapes(&file),
            ["t.Fail()", "t.Error(\"runs\")", "t.Error(\"runs\")"]
        );
        assert_eq!(file.assertions[0].line, 17);
    }

    #[test]
    fn a_build_constraint_skips_every_test() {
        let gated = read(
            "//go:build integration\n\npackage db\n\nfunc TestQuery(t *testing.T) { t.Fail() }\n",
        );
        assert_eq!(ids(&gated), [("TestQuery", true, None, 1)]);
        // Built only sometimes: its assertions count; never built: not.
        assert_eq!(shapes(&gated), ["t.Fail()"]);
        let old = read(
            "// +build integration\n\npackage db\n\nfunc TestQuery(t *testing.T) { t.Fail() }\n",
        );
        assert_eq!(ids(&old), [("TestQuery", true, None, 1)]);
        let ignored = read("//go:build ignore\n\npackage db\n\nfunc TestQuery(t *testing.T) { t.Run(\"a\", func(t *testing.T) { t.Fail() }) }\n");
        assert_eq!(
            ids(&ignored),
            [
                ("TestQuery", true, Some(NEVER_BUILT), 1),
                ("TestQuery/a", true, Some(NEVER_BUILT), 1),
            ]
        );
        assert!(ignored.assertions.is_empty());
        let after =
            read("package db\n\n//go:build ignore\nfunc TestQuery(t *testing.T) { t.Fail() }\n");
        assert_eq!(ids(&after), [("TestQuery", false, None, 1)]);
    }

    #[test]
    fn assertion_libraries() {
        let file = read(
            r#"package pricing

import (
	"testing"

	"github.com/stretchr/testify/assert"
	req "github.com/stretchr/testify/require"
	qt "github.com/frankban/quicktest"
	. "github.com/onsi/gomega"
)

func TestLibraries(t *testing.T) {
	assert.Equal(t, 3, Total(1, 2))
	req.NoError(t, err)
	a := assert.New(t)
	a.Len(items, 2)
	qt.Assert(t, got, qt.Equals, 3)
	qt.Check(t, got, qt.IsNil)
	Expect(Total(1, 2)).To(Equal(3))
	Ω(err).ShouldNot(HaveOccurred())
	g := NewWithT(t)
	g.Expect(x).NotTo(BeNil())
	Eventually(f).WithTimeout(time.Second).Should(Succeed())
	assert.New(t)
	qt.CmpEquals(opts)
	log.Fatal("not on a T")
	Expect(x)
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assert.Equal(t,3,Total(1,2))",
                "req.NoError(t,err)",
                "a.Len(items,2)",
                "qt.Assert(t,got,qt.Equals,3)",
                "qt.Check(t,got,qt.IsNil)",
                "Expect(Total(1,2)).To(Equal(3))",
                "Ω(err).ShouldNot(HaveOccurred())",
                "g.Expect(x).NotTo(BeNil())",
                "Eventually(f).WithTimeout(time.Second).Should(Succeed())",
            ]
        );
    }

    #[test]
    fn testify_suites() {
        let file = read(
            r#"package pricing

type CartSuite struct {
	suite.Suite
	cart *Cart
}

func (s *CartSuite) SetupTest() { s.cart = NewCart() }

func (s *CartSuite) TestAdd() {
	s.cart.Add(1)
	s.Equal(1, s.cart.Len())
	s.Require().NoError(s.cart.Err())
	s.T().Errorf("x")
	s.Helper2()
	s.Run("twice", func() {
		s.True(s.cart.Add(2))
	})
}

func (s *CartSuite) Helper2() {}

func (s *CartSuite) TestSkipped() { s.T().Skip("later") }

type Other struct{}

func (o *Other) TestNotASuite() { o.Equal(1, 2) }

func TestCartSuite(t *testing.T) { suite.Run(t, new(CartSuite)) }
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("CartSuite/TestAdd", false, None, 1),
                ("CartSuite/TestAdd/twice", false, None, 1),
                (
                    "CartSuite/TestSkipped",
                    true,
                    Some("it calls skip first"),
                    1
                ),
                ("TestCartSuite", false, None, 1),
            ]
        );
        assert_eq!(
            shapes(&file),
            [
                "s.Equal(1,s.cart.Len())",
                "s.Require().NoError(s.cart.Err())",
                "s.T().Errorf(\"x\")",
                "s.True(s.cart.Add(2))",
            ]
        );
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"package pricing

import (
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestC(t *testing.T) {
	assert.True(t, true)
	assert.True(t, true, "message")
	require.False(t, false)
	assert.Equal(t, 1, 1)
	assert.Equal(t, "a", "a")
	assert.Nil(t, nil)
	a := assert.New(t)
	a.True(true)
	Expect(true).To(BeTrue())
	Expect(1).To(Equal(1))
	assert.True(t, ok)
	assert.Equal(t, 1, n)
	assert.Equal(t, 1, 2)
	assert.False(t, true)
	Expect(1).NotTo(Equal(1))
	t.Fail()
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assert.True(t,ok)",
                "assert.Equal(t,1,n)",
                "assert.Equal(t,1,2)",
                "assert.False(t,true)",
                "Expect(1).NotTo(Equal(1))",
                "t.Fail()",
            ]
        );
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r#"package pricing

// func TestCommented(t *testing.T) { t.Fail() }
const s = "func TestInAString(t *testing.T) { t.Fail() }"
var r = `t.Error("x")`

func testLower(t *testing.T) {}

type T struct{}

func (T) TestMethod() {}
"#,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn the_subject_is_the_file_the_test_is_named_after() {
        let internal = read(
            r#"package pricing

import (
	"testing"
	"strings"
)

type fixture struct{ in int }

func TestTotal(t *testing.T) {
	f := fixture{in: 1}
	var c Cart
	if Total(f.in) != len(strings.Repeat("a", 1)) {
		t.Fail()
	}
	_ = c
}
"#,
        );
        let subject: Vec<(&str, &str)> = internal
            .imports
            .iter()
            .map(|(name, spec)| (name.as_str(), spec.as_str()))
            .collect();
        assert_eq!(
            subject,
            [
                ("Cart", PACKAGE),
                ("Total", PACKAGE),
                ("strings", "strings"),
                ("testing", "testing")
            ]
        );
        let external = read(
            r#"package pricing_test

import (
	"testing"

	"example.com/shop/pricing"
	other "example.com/shop/tax"
)

func TestTotal(t *testing.T) {
	if pricing.Total(1) != other.Rate() {
		t.Fail()
	}
}
"#,
        );
        assert_eq!(
            external.imports.get("pricing").map(String::as_str),
            Some("example.com/shop/pricing")
        );
        assert_eq!(
            external.imports.get("Total").map(String::as_str),
            Some(PACKAGE)
        );
        assert!(external.tests[0].uses.contains("Total"));
        assert!(!external.tests[0].uses.contains("Rate"));
        assert_eq!(
            external.imports.get("other").map(String::as_str),
            Some("example.com/shop/tax")
        );
        let exists = |path: &str| path == "shop/pricing.go";
        assert_eq!(
            resolve("shop/pricing_test.go", PACKAGE, &exists).as_deref(),
            Some("shop/pricing.go")
        );
        assert_eq!(resolve("shop/cart_test.go", PACKAGE, &exists), None);
        assert_eq!(resolve("shop/pricing_test.go", "strings", &exists), None);
    }

    #[test]
    fn top_level_names_are_what_a_file_gives_its_package() {
        let names = top_level_names(
            r#"package pricing

import "fmt"

const Limit, other = 3, 4

var (
	rate = 0.2
	Name string
)

type Cart struct{ items []int }

type Alias = Cart

func Total(a, b int) int { inner := 1; return a + b + inner }

func (c *Cart) Add(item int) { fmt.Println(item) }
"#,
        );
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(
            names,
            ["Alias", "Cart", "Limit", "Name", "Total", "other", "rate"]
        );
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read("package p\n\nfunc TestA(t *testing.T) {\n\tt.Fail()\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let source = format!(
            "package p\n\nfunc TestA(t *testing.T) {{ {} t.Fail() {} }}\n",
            "{ ".repeat(1_000),
            "} ".repeat(1_000)
        );
        let file = read(&source);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
    }

    #[test]
    fn test_paths() {
        for path in ["pricing_test.go", "shop/pricing_test.go", "a/b/c_test.go"] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "pricing.go",
            "shop/pricing_test.py",
            "_test.go",
            "shop/_pricing_test.go",
            "shop/.hidden/pricing_test.go",
            "shop/testdata/pricing_test.go",
            "shop/_old/pricing_test.go",
            "tests/pricing.go",
            "pricing_test.GO",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
        assert!(!holds_tests("func TestA(t *testing.T) {}"));
    }

    #[test]
    fn suppressions_are_read_from_comments() {
        let source = r#"package p

//nolint
func a() {}

func b() {
	_ = f() //nolint:errcheck,gosec // checked above
	_ = g() // nolint:all
	//lint:ignore SA1019 the old API is all we have
	h()
	//lint:file-ignore U1000,ST1003 generated
	key := "x" // #nosec G101 -- not a secret
	/* #nosec */
	//#nosec G104 G307
	//revive:disable:unused-parameter
	//revive:disable-next-line
	//revive:disable-line:exported
}
"#;
        assert_eq!(
            keys(source),
            [
                "nolint:*",
                "nolint:errcheck",
                "nolint:gosec",
                "nolint:*",
                "lint-ignore:SA1019",
                "lint-file-ignore:U1000",
                "lint-file-ignore:ST1003",
                "nosec:G101",
                "nosec:*",
                "nosec:G104",
                "nosec:G307",
                "revive-range:unused-parameter",
                "revive:*",
                "revive:exported",
            ]
        );
        let found = suppressions(source);
        assert_eq!(found[1].line, 7);
        assert_eq!(found[1].text, "//nolint:errcheck,gosec // checked above");
    }

    #[test]
    fn look_alikes_are_not_suppressions() {
        let source = r#"package p

// Do not add nolint here: fix the error.
// nolinter is not a directive
//lint:ignored is not one either
// We mark secrets with #nosec nowhere.
//revive:enable
//go:build linux
const s = "//nolint:errcheck"
var r = `// #nosec G101`
"#;
        assert!(keys(source).is_empty());
    }
}
