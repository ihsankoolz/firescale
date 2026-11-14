//! The Python reader of the tamper guard (M24, ADR 0044): pytest and
//! unittest.
//!
//! - Tests: `def test*` at module level, and `def test*` methods of a class
//!   named `Test*` or one whose superclass ends with `TestCase`.
//! - Skips: `@pytest.mark.skip`, `skipif`, `xfail`, `unittest.skip…` on the
//!   test or its classes, a skip mark given a name in the module
//!   (`needs_db = pytest.mark.skipif(…)`), `pytestmark`, `__test__ = False`,
//!   a module-level `pytest.importorskip(…)`, a `pytest.skip(…)` or
//!   `self.skipTest(…)` first in the test or in its class's `setUp`, a body
//!   that does nothing (unless a decorator pytest does not know may supply
//!   it), a definition that never runs (after a module-level skip).
//! - Pseudo-tests, always skipped: each entry of `collect_ignore` and
//!   `collect_ignore_glob` (conftest.py), and an autouse fixture that calls
//!   skip first: adding one skips tests the file does not name.
//! - Assertions: `assert`, any `…assert…(…)` call, `self.fail(…)`,
//!   `pytest.fail(…)`, `pytest.raises` and `pytest.warns`; not those that
//!   never run or certainly pass (`assert True`, `assert 1 == 1`,
//!   `assert (x, "message")`). One that certainly fails (`assert False`)
//!   counts: it is how a test says "never reach here".

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::Node;

use super::reach::{self, Grammar};
use super::{Assertion, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// How deep a dotted name is followed: real code never comes near it.
const MAX_CHAIN: usize = 32;

/// pytest marks that make a test not run, or pass when it fails.
const PYTEST_SKIPS: [&str; 3] = ["skip", "skipif", "xfail"];
/// unittest decorators that do the same.
const UNITTEST_SKIPS: [&str; 4] = ["skip", "skipIf", "skipUnless", "expectedFailure"];

/// The methods whose first statement, when it skips, skips every test of
/// their class.
const SETUPS: [&str; 4] = ["setUp", "setUpClass", "setup_method", "setup_class"];
/// Why the tests of a module or class that sets `__test__ = False` do not
/// run.
const TEST_FALSE: &str = "__test__ = False";
/// The conftest.py variables that keep pytest from collecting files.
const IGNORES: [&str; 2] = ["collect_ignore", "collect_ignore_glob"];
/// Splats in a literal: a list made of them alone may be empty.
const SPLATS: [&str; 3] = ["list_splat", "parenthesized_list_splat", "dictionary_splat"];

/// Values that are the same on every run: an `assert` comparing two of
/// these that are written the same, or `self.assertTrue(True)`, cannot fail.
const LITERALS: [&str; 20] = [
    "integer",
    "float",
    "string",
    "string_start",
    "string_content",
    "string_end",
    "escape_sequence",
    "concatenated_string",
    "true",
    "false",
    "none",
    "comparison_operator",
    "boolean_operator",
    "not_operator",
    "unary_operator",
    "binary_operator",
    "parenthesized_expression",
    "list",
    "tuple",
    "ellipsis",
];

/// What Python calls the nodes [`reach`] looks at.
const GRAMMAR: Grammar = Grammar {
    blocks: &["block", "module"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &["while_statement"],
    hoisted: &[],
    stops: Some(stops),
    noop,
};

/// True for a statement after which its block does not run:
/// `pytest.skip(…)`, `pytest.xfail(…)`, `self.skipTest(…)`, `raise
/// unittest.SkipTest(…)` (also `SkipTest(…)`, `pytest.skip.Exception(…)`).
/// At module level, the definitions after one are never collected.
fn stops(statement: Node<'_>, src: &[u8]) -> bool {
    if statement.kind() == "raise_statement" {
        return children(statement)
            .first()
            .and_then(|raised| mark_name(*raised, src))
            .is_some_and(|path| {
                matches!(
                    path.iter().map(String::as_str).collect::<Vec<_>>()[..],
                    ["unittest", "SkipTest"] | ["SkipTest"] | ["pytest", "skip", "Exception"]
                )
            });
    }
    called(statement, src).is_some_and(|path| {
        matches!(
            path.iter().map(String::as_str).collect::<Vec<_>>()[..],
            ["pytest", "skip" | "xfail"] | ["self", "skipTest"]
        )
    })
}

/// True for a statement that does nothing: `pass`, `...`, a docstring.
fn noop(statement: Node<'_>, _src: &[u8]) -> bool {
    match statement.kind() {
        "pass_statement" => true,
        "expression_statement" => {
            let mut cursor = statement.walk();
            let mut inner = statement
                .named_children(&mut cursor)
                .filter(|child| !child.is_extra());
            matches!(
                (inner.next().map(|child| child.kind()), inner.next()),
                (Some("string" | "concatenated_string" | "ellipsis"), None)
            )
        }
        _ => false,
    }
}

/// The named children of a node that are not comments.
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .collect()
}

/// The dotted name an expression is (`pytest.mark.skip`, `self`), or
/// `None` for anything else. A loop, at most [`MAX_CHAIN`] steps deep.
fn dotted(node: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    let mut path: Vec<String> = Vec::new();
    let mut at = node;
    for _ in 0..MAX_CHAIN {
        match at.kind() {
            "identifier" => {
                path.push(text(at, src).to_owned());
                path.reverse();
                return Some(path);
            }
            "attribute" => {
                path.push(text(at.child_by_field_name("attribute")?, src).to_owned());
                at = at.child_by_field_name("object")?;
            }
            _ => return None,
        }
    }
    None
}

/// The dotted name of a mark or decorator: the callee when it is a call.
fn mark_name(node: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    if node.kind() == "call" {
        dotted(node.child_by_field_name("function")?, src)
    } else {
        dotted(node, src)
    }
}

/// The dotted callee of a statement that is only a call (`pytest.skip()`).
fn called(statement: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    if statement.kind() != "expression_statement" {
        return None;
    }
    let inner = children(statement);
    let [call] = inner[..] else {
        return None;
    };
    if call.kind() != "call" {
        return None;
    }
    dotted(call.child_by_field_name("function")?, src)
}

/// The name and value of a statement `name = value` (also `name: T =
/// value`).
fn assigned<'t>(statement: Node<'t>, src: &[u8]) -> Option<(String, Node<'t>)> {
    if statement.kind() != "expression_statement" {
        return None;
    }
    let assignment = children(statement)
        .into_iter()
        .next()
        .filter(|inner| inner.kind() == "assignment")?;
    let left = assignment
        .child_by_field_name("left")
        .filter(|left| left.kind() == "identifier")?;
    Some((
        text(left, src).to_owned(),
        assignment.child_by_field_name("right")?,
    ))
}

/// What the file's imports bind: for `from X import a as b`, `b` → (`X`,
/// `a`); for `import X as Y`, `Y` → (`X`, `""`). And what its module-level
/// assignments bind that the reader looks at.
struct Names<'t> {
    bound: BTreeMap<String, (String, String)>,
    /// Names given a skip mark at module level: `needs_db =
    /// pytest.mark.skipif(…)`, `slow = pytest.mark.skip`.
    skip_marks: BTreeSet<String>,
    /// Names assigned once at module level, to a list or tuple literal:
    /// `CASES = [(1, 2), (3, 4)]`.
    constants: BTreeMap<String, Node<'t>>,
}

impl<'t> Names<'t> {
    /// Reads the module-level assignments of named skip marks and
    /// constants, in order, so a name given another skip mark's name is one
    /// too.
    fn read_module(&mut self, root: Node<'t>, src: &[u8]) {
        let mut times: BTreeMap<String, usize> = BTreeMap::new();
        let mut values: BTreeMap<String, Node<'t>> = BTreeMap::new();
        for statement in children(root) {
            let Some((name, value)) = assigned(statement, src) else {
                continue;
            };
            if mark_name(value, src).is_some_and(|path| self.is_skip(&path)) {
                self.skip_marks.insert(name.clone());
            } else {
                self.skip_marks.remove(&name);
            }
            *times.entry(name.clone()).or_default() += 1;
            values.insert(name, value);
        }
        self.constants = values
            .into_iter()
            .filter(|(name, value)| {
                times.get(name) == Some(&1)
                    && matches!(value.kind(), "list" | "tuple" | "expression_list")
            })
            .collect();
    }

    /// True when `name` is the module `module` (`pytest`, or an alias of it).
    fn is_module(&self, name: &str, module: &str) -> bool {
        name == module
            || self
                .bound
                .get(name)
                .is_some_and(|(from, original)| from == module && original.is_empty())
    }

    /// The name `local` stands for when it is imported from `module`.
    fn imported_from(&self, local: &str, module: &str) -> Option<&str> {
        self.bound
            .get(local)
            .filter(|(from, original)| from == module && !original.is_empty())
            .map(|(_, original)| original.as_str())
    }

    /// True for `pytest.<one of names>` or a bare name imported from pytest
    /// as one of them.
    fn is_pytest(&self, path: &[String], names: &[&str]) -> bool {
        match path {
            [module, name] => self.is_module(module, "pytest") && names.contains(&name.as_str()),
            [local] => self
                .imported_from(local, "pytest")
                .is_some_and(|original| names.contains(&original)),
            _ => false,
        }
    }

    /// True for a mark or decorator that skips a test.
    fn is_skip(&self, path: &[String]) -> bool {
        let Some((last, before)) = path.split_last() else {
            return false;
        };
        let last = last.as_str();
        if before.iter().any(|part| part == "mark") && PYTEST_SKIPS.contains(&last) {
            return true;
        }
        if before
            .first()
            .is_some_and(|first| self.is_module(first, "unittest"))
            && UNITTEST_SKIPS.contains(&last)
        {
            return true;
        }
        before.is_empty()
            && (self.skip_marks.contains(last)
                || self
                    .imported_from(last, "unittest")
                    .is_some_and(|original| UNITTEST_SKIPS.contains(&original)))
    }

    /// True for a decorator pytest or unittest knows, that does not supply
    /// a test's body: a pytest mark, `pytest.fixture`, a unittest skip, a
    /// named skip mark. Any other (pytest-bdd's `@scenario(…)`) may.
    fn is_known_decorator(&self, decorator: Node<'_>, src: &[u8]) -> bool {
        mark_name(decorator, src).is_some_and(|path| {
            self.is_skip(&path)
                || self.is_pytest(&path, &["fixture"])
                || path
                    .split_last()
                    .is_some_and(|(_, before)| before.iter().any(|part| part == "mark"))
        })
    }

    /// True for an expression that is a skip mark, or a list or tuple
    /// holding one (`pytestmark = [...]`, `marks=(...)`).
    fn skips(&self, value: Node<'_>, src: &[u8]) -> bool {
        if matches!(value.kind(), "list" | "tuple" | "expression_list") {
            return children(value)
                .into_iter()
                .any(|item| mark_name(item, src).is_some_and(|path| self.is_skip(&path)));
        }
        mark_name(value, src).is_some_and(|path| self.is_skip(&path))
    }
}

/// The module part and the name part of every import, with the spec each
/// name stands for in [`TestFile::imports`].
fn imports<'t>(root: Node<'t>, src: &[u8]) -> (BTreeMap<String, String>, Names<'t>) {
    let mut specs = BTreeMap::new();
    let mut bound = BTreeMap::new();
    walk(root, |node| match node.kind() {
        "import_statement" => {
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                let (target, alias) = aliased(name, src);
                let Some(module) = target else { continue };
                let local = alias
                    .unwrap_or_else(|| module.split('.').next().unwrap_or_default().to_owned());
                bound.insert(local.clone(), (module.clone(), String::new()));
                specs.insert(local, module);
            }
            false
        }
        "import_from_statement" => {
            let Some(module) = node
                .child_by_field_name("module_name")
                .and_then(|module| from_module(module, src))
            else {
                return false;
            };
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                let (target, alias) = aliased(name, src);
                let Some(target) = target else { continue };
                let local = alias.unwrap_or_else(|| target.clone());
                // `from . import a`: `a` is a module of the package.
                let spec = if module.chars().all(|c| c == '.') {
                    format!("{module}{target}")
                } else {
                    module.clone()
                };
                bound.insert(local.clone(), (module.clone(), target));
                specs.insert(local, spec);
            }
            false
        }
        // `from __future__ import …` names no module of the repo.
        "future_import_statement" => false,
        _ => true,
    });
    (
        specs,
        Names {
            bound,
            skip_marks: BTreeSet::new(),
            constants: BTreeMap::new(),
        },
    )
}

/// A `dotted_name`, or an `aliased_import`'s name and alias.
fn aliased(node: Node<'_>, src: &[u8]) -> (Option<String>, Option<String>) {
    if node.kind() == "aliased_import" {
        let name = node
            .child_by_field_name("name")
            .and_then(|name| dotted_name(name, src));
        let alias = node
            .child_by_field_name("alias")
            .map(|alias| text(alias, src).to_owned());
        (name, alias)
    } else {
        (dotted_name(node, src), None)
    }
}

/// The text of a `dotted_name`, without white space or comments.
fn dotted_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "dotted_name" {
        return None;
    }
    let parts: Vec<&str> = children(node)
        .into_iter()
        .map(|part| text(part, src))
        .collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

/// The module of a `from` import: `a.b`, `.models`, `..`.
fn from_module(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "dotted_name" => dotted_name(node, src),
        "relative_import" => {
            let mut module = String::new();
            for child in children(node) {
                match child.kind() {
                    "import_prefix" => {
                        module.extend(text(child, src).chars().filter(|&c| c == '.'));
                    }
                    "dotted_name" => module.push_str(&dotted_name(child, src)?),
                    _ => {}
                }
            }
            (!module.is_empty()).then_some(module)
        }
        _ => None,
    }
}

/// The decorators written on a function or class definition.
fn decorators(definition: Node<'_>) -> Vec<Node<'_>> {
    definition
        .parent()
        .filter(|parent| parent.kind() == "decorated_definition")
        .map(|parent| {
            children(parent)
                .into_iter()
                .filter(|child| child.kind() == "decorator")
                .filter_map(|decorator| children(decorator).into_iter().next())
                .collect()
        })
        .unwrap_or_default()
}

/// The value of `pytestmark = …` among the statements of a module or class
/// body.
fn pytestmark<'t>(body: Node<'t>, src: &[u8]) -> Vec<Node<'t>> {
    children(body)
        .into_iter()
        .filter(|statement| statement.kind() == "expression_statement")
        .filter_map(|statement| children(statement).into_iter().next())
        .filter(|assignment| {
            assignment.kind() == "assignment"
                && assignment
                    .child_by_field_name("left")
                    .is_some_and(|left| text(left, src) == "pytestmark")
        })
        .filter_map(|assignment| assignment.child_by_field_name("right"))
        .collect()
}

/// The positional arguments and the keyword arguments of a call.
fn arguments<'t>(call: Node<'t>, src: &[u8]) -> (Vec<Node<'t>>, BTreeMap<String, Node<'t>>) {
    let mut positional = Vec::new();
    let mut keywords = BTreeMap::new();
    let Some(list) = call.child_by_field_name("arguments") else {
        return (positional, keywords);
    };
    if list.kind() != "argument_list" {
        // `f(x for x in y)`: one argument.
        positional.push(list);
        return (positional, keywords);
    }
    for argument in children(list) {
        match argument.kind() {
            "keyword_argument" => {
                if let (Some(name), Some(value)) = (
                    argument.child_by_field_name("name"),
                    argument.child_by_field_name("value"),
                ) {
                    keywords.insert(text(name, src).to_owned(), value);
                }
            }
            "dictionary_splat" => {}
            _ => positional.push(argument),
        }
    }
    (positional, keywords)
}

/// For a `@pytest.mark.parametrize(names, values)` decorator: its rows,
/// and how many of them are a `pytest.param(…, marks=<a skip>)`. `None` for
/// any other decorator.
fn parametrize(decorator: Node<'_>, src: &[u8], names: &Names<'_>) -> Option<(usize, usize)> {
    if decorator.kind() != "call" {
        return None;
    }
    let path = dotted(decorator.child_by_field_name("function")?, src)?;
    let (last, before) = path.split_last()?;
    if last != "parametrize" || !before.iter().any(|part| part == "mark") {
        return None;
    }
    let (positional, keywords) = arguments(decorator, src);
    let values = positional
        .get(1)
        .copied()
        .or_else(|| keywords.get("argvalues").copied());
    // A bare name: the list or tuple a module-level assignment gives it.
    let values = values.and_then(|values| {
        if values.kind() == "identifier" {
            names.constants.get(text(values, src)).copied()
        } else {
            Some(values)
        }
    });
    let Some(values) =
        values.filter(|values| matches!(values.kind(), "list" | "tuple" | "expression_list"))
    else {
        return Some((1, 0));
    };
    let rows = children(values);
    let skipped = rows
        .iter()
        .filter(|row| {
            row.kind() == "call"
                && row
                    .child_by_field_name("function")
                    .and_then(|function| dotted(function, src))
                    .is_some_and(|path| names.is_pytest(&path, &["param"]))
                && arguments(**row, src)
                    .1
                    .get("marks")
                    .is_some_and(|marks| names.skips(*marks, src))
        })
        .count();
    Some((rows.len().max(1), skipped))
}

/// True for a class whose methods named `test*` are tests: `Test*`, or a
/// subclass of something ending with `TestCase`.
fn is_test_class(class: Node<'_>, name: &str, src: &[u8]) -> bool {
    if name.starts_with("Test") {
        return true;
    }
    class
        .child_by_field_name("superclasses")
        .is_some_and(|superclasses| {
            children(superclasses).into_iter().any(|superclass| {
                dotted(superclass, src)
                    .and_then(|path| path.last().cloned())
                    .is_some_and(|last| last.ends_with("TestCase"))
            })
        })
}

/// Why a test function does not run when no marker says so. `own_body` is
/// false when a decorator pytest does not know may supply the body, so a
/// body that does nothing is not a reason.
fn not_run(
    function: Node<'_>,
    src: &[u8],
    names: &Names<'_>,
    dead: &reach::Dead,
    own_body: bool,
) -> Option<&'static str> {
    let body = function.child_by_field_name("body")?;
    if first_doing(body, src).is_some_and(|statement| calls_skip(statement, src, names)) {
        return Some("it calls skip first");
    }
    if own_body && reach::does_nothing(body, src, &GRAMMAR) {
        return Some("its body does nothing");
    }
    dead.why(function)
}

/// The first statement of a block that does something: not `pass`, `...`
/// or a docstring.
fn first_doing<'t>(body: Node<'t>, src: &[u8]) -> Option<Node<'t>> {
    children(body)
        .into_iter()
        .find(|statement| !noop(*statement, src))
}

/// True for a function whose first statement that does something skips.
fn skips_first(function: Node<'_>, src: &[u8], names: &Names<'_>) -> bool {
    function
        .child_by_field_name("body")
        .and_then(|body| first_doing(body, src))
        .is_some_and(|statement| calls_skip(statement, src, names))
}

/// True for a class body with a `setUp`, `setUpClass`, `setup_method` or
/// `setup_class` that skips first: none of the class's tests run.
fn setup_skips(body: Node<'_>, src: &[u8], names: &Names<'_>) -> bool {
    children(body).into_iter().any(|statement| {
        let function = if statement.kind() == "decorated_definition" {
            statement.child_by_field_name("definition")
        } else {
            Some(statement)
        };
        function
            .filter(|function| function.kind() == "function_definition")
            .filter(|function| {
                function
                    .child_by_field_name("name")
                    .is_some_and(|name| SETUPS.contains(&text(name, src)))
            })
            .is_some_and(|function| skips_first(function, src, names))
    })
}

/// True for a module or class body that says `__test__ = False`: pytest
/// collects none of its tests.
fn not_collected(body: Node<'_>, src: &[u8]) -> bool {
    children(body).into_iter().any(|statement| {
        assigned(statement, src)
            .is_some_and(|(name, value)| name == "__test__" && value.kind() == "false")
    })
}

/// True for a module that calls `pytest.importorskip(…)` at module level,
/// as a statement or in an assignment: its tests are skipped when the
/// module is missing.
fn imports_or_skips(root: Node<'_>, src: &[u8], names: &Names<'_>) -> bool {
    let is_importorskip = |node: Node<'_>| {
        node.kind() == "call"
            && node
                .child_by_field_name("function")
                .and_then(|function| dotted(function, src))
                .is_some_and(|path| names.is_pytest(&path, &["importorskip"]))
    };
    children(root).into_iter().any(|statement| {
        if statement.kind() != "expression_statement" {
            return false;
        }
        children(statement)
            .first()
            .is_some_and(|inner| is_importorskip(*inner))
            || assigned(statement, src).is_some_and(|(_, value)| is_importorskip(value))
    })
}

/// True for `@pytest.fixture(autouse=True)` (or `fixture` imported from
/// pytest) on a function that skips first: every test it reaches is
/// skipped.
fn autouse_skips(function: Node<'_>, src: &[u8], names: &Names<'_>) -> bool {
    decorators(function).into_iter().any(|decorator| {
        decorator.kind() == "call"
            && decorator
                .child_by_field_name("function")
                .and_then(|callee| dotted(callee, src))
                .is_some_and(|path| names.is_pytest(&path, &["fixture"]))
            && arguments(decorator, src)
                .1
                .get("autouse")
                .is_some_and(|value| value.kind() == "true")
    }) && skips_first(function, src, names)
}

/// The text of a string literal, or of literals written next to each
/// other, escapes as written; `None` for an f-string with a value in it or
/// anything else.
fn string_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "string" => {
            let mut value = String::new();
            for part in children(node) {
                match part.kind() {
                    "string_content" => value.push_str(text(part, src)),
                    "interpolation" => return None,
                    _ => {}
                }
            }
            Some(value)
        }
        "concatenated_string" => {
            let mut value = String::new();
            for part in children(node) {
                value.push_str(&string_value(part, src)?);
            }
            Some(value)
        }
        _ => None,
    }
}

/// The string entries a module-level statement adds to `collect_ignore`
/// or `collect_ignore_glob`: `= [...]`, `+= [...]`, `.append("…")`,
/// `.extend([...])`, `.insert(0, "…")`. Each with the variable's name.
fn ignored_entries<'t>(statement: Node<'t>, src: &[u8], found: &mut Vec<(&'static str, Node<'t>)>) {
    let (variable, mut stack) = match statement.kind() {
        "assignment" | "augmented_assignment" => {
            let (Some(left), Some(right)) = (
                statement.child_by_field_name("left"),
                statement.child_by_field_name("right"),
            ) else {
                return;
            };
            (text(left, src), vec![right])
        }
        "call" => {
            let Some(function) = statement
                .child_by_field_name("function")
                .filter(|function| function.kind() == "attribute")
            else {
                return;
            };
            let (Some(object), Some(method)) = (
                function.child_by_field_name("object"),
                function.child_by_field_name("attribute"),
            ) else {
                return;
            };
            if !matches!(text(method, src), "append" | "extend" | "insert") {
                return;
            }
            let mut positional = arguments(statement, src).0;
            positional.reverse();
            (text(object, src), positional)
        }
        _ => return,
    };
    let Some(variable) = IGNORES.iter().copied().find(|name| *name == variable) else {
        return;
    };
    while let Some(value) = stack.pop() {
        match value.kind() {
            "string" | "concatenated_string" => found.push((variable, value)),
            "list"
            | "tuple"
            | "set"
            | "expression_list"
            | "parenthesized_expression"
            | "binary_operator" => stack.extend(children(value).into_iter().rev()),
            _ => {}
        }
    }
}

/// Every string entry of `collect_ignore` and `collect_ignore_glob` at
/// module level (also inside a module-level `if` or `try`), that runs.
fn collect_ignored<'t>(
    root: Node<'t>,
    src: &[u8],
    dead: &reach::Dead,
) -> Vec<(&'static str, Node<'t>)> {
    let mut found = Vec::new();
    walk(root, |node| match node.kind() {
        "function_definition" | "class_definition" | "lambda" => false,
        "expression_statement" => {
            if dead.why(node).is_none() {
                if let Some(inner) = children(node).into_iter().next() {
                    ignored_entries(inner, src, &mut found);
                }
            }
            false
        }
        _ => true,
    });
    found
}

/// True for `pytest.skip(…)`, `pytest.xfail(…)`, `skip(…)` imported from
/// pytest, `self.skipTest(…)`, or `raise unittest.SkipTest(…)`.
fn calls_skip(statement: Node<'_>, src: &[u8], names: &Names<'_>) -> bool {
    if statement.kind() == "raise_statement" {
        return children(statement).first().is_some_and(|raised| {
            mark_name(*raised, src)
                .and_then(|path| path.last().cloned())
                .is_some_and(|last| last == "SkipTest")
        });
    }
    called(statement, src).is_some_and(|path| {
        names.is_pytest(&path, &["skip", "xfail"])
            || matches!(&path[..], [receiver, method] if receiver == "self" && method == "skipTest")
    })
}

/// Whether a call is an assertion, and whether it can fail. `None` for a
/// call that is not one.
fn call_assertion(call: Node<'_>, src: &[u8], names: &Names<'_>) -> Option<bool> {
    let function = call.child_by_field_name("function")?;
    if dotted(function, src).is_some_and(|path| {
        names.is_pytest(&path, &["raises", "warns", "fail"])
            || matches!(&path[..], [receiver, method] if receiver == "self" && method == "fail")
    }) {
        return Some(true);
    }
    let (last, receiver) = match function.kind() {
        "identifier" => (text(function, src), None),
        "attribute" => (
            text(function.child_by_field_name("attribute")?, src),
            function.child_by_field_name("object"),
        ),
        _ => return None,
    };
    if !last.starts_with("assert") {
        return None;
    }
    let own = receiver
        .is_none_or(|receiver| receiver.kind() == "identifier" && text(receiver, src) == "self");
    let (positional, _) = arguments(call, src);
    let constant = own
        && !positional.is_empty()
        && positional
            .iter()
            .all(|argument| reach::constant(*argument, &LITERALS));
    let passes = constant
        && match last {
            "assertTrue" => positional
                .first()
                .is_some_and(|value| truth(*value, src) == Some(true)),
            "assertFalse" => positional
                .first()
                .is_some_and(|value| truth(*value, src) == Some(false)),
            "assertEqual" | "assertEquals" | "assertIs" => match positional[..] {
                [a, b, ..] => same_literal(a, b, src, last == "assertIs"),
                _ => false,
            },
            _ => false,
        };
    Some(!passes)
}

/// True for an `assert` statement that can fail: its condition does not
/// certainly pass. The message after the comma is not looked at. `assert
/// False` can: it is how a test says "never reach here".
fn assert_can_fail(statement: Node<'_>, src: &[u8]) -> bool {
    children(statement)
        .first()
        .is_some_and(|condition| !certainly_passes(*condition, src))
}

/// The expression inside any parentheses around it, at most
/// [`MAX_CHAIN`] deep.
fn unwrap_parentheses(node: Node<'_>) -> Node<'_> {
    let mut at = node;
    for _ in 0..MAX_CHAIN {
        if at.kind() != "parenthesized_expression" {
            break;
        }
        match children(at)[..] {
            [inner] => at = inner,
            _ => break,
        }
    }
    at
}

/// True for an `assert` condition that passes on every run: a truthy
/// literal, a non-empty tuple of anything (`assert (x == 3, "message")`),
/// or `a == b` / `a is b` of two literals written the same.
fn certainly_passes(condition: Node<'_>, src: &[u8]) -> bool {
    if truth(condition, src) == Some(true) {
        return true;
    }
    let comparison = unwrap_parentheses(condition);
    if comparison.kind() != "comparison_operator" {
        return false;
    }
    let [a, b] = children(comparison)[..] else {
        return false;
    };
    let mut cursor = comparison.walk();
    let operators: Vec<&str> = comparison
        .children_by_field_name("operators", &mut cursor)
        .map(|operator| text(operator, src))
        .collect();
    match operators[..] {
        ["=="] => same_literal(a, b, src, false),
        ["is"] => same_literal(a, b, src, true),
        _ => false,
    }
}

/// True for two literal values written the same, white space aside: they
/// are equal. For `is`, not when a list literal is in them: two lists are
/// never the same object.
fn same_literal(a: Node<'_>, b: Node<'_>, src: &[u8], identity: bool) -> bool {
    let squeezed = |node: Node<'_>| -> String { text(node, src).split_whitespace().collect() };
    let has_list = |node: Node<'_>| {
        let mut found = false;
        walk(node, |inner| {
            found |= inner.kind() == "list";
            !found
        });
        found
    };
    reach::constant(a, &LITERALS)
        && reach::constant(b, &LITERALS)
        && squeezed(a) == squeezed(b)
        && !(identity && (has_list(a) || has_list(b)))
}

/// Whether a value is certainly truthy (`Some(true)`: `True`, a non-zero
/// number, a non-empty string, a non-empty list, tuple, set or dict of
/// anything), certainly falsy (`Some(false)`: `False`, `None`, zero, `""`,
/// `[]`, `()`, `{}`), through `not` and parentheses; `None` when it is not
/// known from the text.
fn truth(node: Node<'_>, src: &[u8]) -> Option<bool> {
    let mut at = node;
    let mut negated = false;
    for _ in 0..MAX_CHAIN {
        match at.kind() {
            "parenthesized_expression" => match children(at)[..] {
                [inner] => at = inner,
                _ => return None,
            },
            "not_operator" => {
                negated = !negated;
                at = at.child_by_field_name("argument")?;
            }
            _ => break,
        }
    }
    let value = match at.kind() {
        "true" => true,
        "false" | "none" => false,
        "integer" | "float" => !is_zero(text(at, src)),
        "unary_operator" => {
            let operator = at.child_by_field_name("operator").map(|op| text(op, src));
            let operand = at.child_by_field_name("argument")?;
            if !matches!(operator, Some("-" | "+"))
                || !matches!(operand.kind(), "integer" | "float")
            {
                return None;
            }
            !is_zero(text(operand, src))
        }
        "string" | "concatenated_string" => !string_value(at, src)?.is_empty(),
        "tuple" | "list" | "set" | "dictionary" => {
            let items = children(at);
            if items.is_empty() {
                false
            } else if items.iter().all(|item| SPLATS.contains(&item.kind())) {
                // `[*a]` is empty when `a` is.
                return None;
            } else {
                true
            }
        }
        _ => return None,
    };
    Some(value != negated)
}

/// True for a number literal that is zero: `0`, `0.0`, `.0`, `0x0`,
/// `0e5`, `0j`.
fn is_zero(literal: &str) -> bool {
    let lower: String = literal
        .chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let lower = lower.trim_end_matches(['j', 'l']);
    let digits = match lower.get(..2) {
        Some("0x" | "0o" | "0b") => lower.get(2..).unwrap_or_default(),
        _ => lower.split('e').next().unwrap_or(lower),
    };
    digits.chars().any(|c| c == '0') && digits.chars().all(|c| c == '0' || c == '.')
}

/// Reads the tests, skips and assertions of a Python test file.
#[must_use]
pub(super) fn read(source: &str) -> TestFile {
    let Some(tree) = parse(source, &tree_sitter_python::LANGUAGE.into()) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    let dead = reach::Dead::of(root, src, &GRAMMAR);
    let (specs, mut names) = imports(root, src);
    names.read_module(root, src);
    let mut file = TestFile {
        imports: specs,
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
    let mut module_skipped = pytestmark(root, src)
        .into_iter()
        .any(|value| names.skips(value, src))
        || imports_or_skips(root, src, &names);
    let mut module_why = None;
    if not_collected(root, src) {
        module_skipped = true;
        module_why = Some(TEST_FALSE);
    }

    // Each class and function open around the node being visited.
    enum Scope {
        Class {
            name: String,
            /// It and every class around it can hold tests.
            holds_tests: bool,
            skipped: bool,
            /// Why, when no marker says so.
            why: Option<&'static str>,
            /// The rows and skipped rows of its parametrize decorators and
            /// its classes'.
            rows: Vec<(usize, usize)>,
        },
        Function {
            test: Option<usize>,
        },
    }
    let mut open: Vec<(usize, Scope)> = Vec::new();
    walk(root, |node| {
        while open
            .last()
            .is_some_and(|(end, _)| node.start_byte() >= *end)
        {
            open.pop();
        }
        match node.kind() {
            "class_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map_or("", |name| text(name, src))
                    .to_owned();
                let outer = open.last().map(|(_, scope)| scope);
                let (outer_holds, outer_skipped, mut why, mut rows) = match outer {
                    None => (true, false, None, Vec::new()),
                    Some(Scope::Class {
                        holds_tests,
                        skipped,
                        why,
                        rows,
                        ..
                    }) => (*holds_tests, *skipped, *why, rows.clone()),
                    Some(Scope::Function { .. }) => (false, false, None, Vec::new()),
                };
                let mut skipped = outer_skipped;
                for decorator in decorators(node) {
                    if let Some(table) = parametrize(decorator, src, &names) {
                        rows.push(table);
                    } else if mark_name(decorator, src).is_some_and(|path| names.is_skip(&path)) {
                        skipped = true;
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    skipped |= pytestmark(body, src)
                        .into_iter()
                        .any(|value| names.skips(value, src));
                    if not_collected(body, src) {
                        skipped = true;
                        why = why.or(Some(TEST_FALSE));
                    }
                    if setup_skips(body, src, &names) {
                        skipped = true;
                        why = why.or(Some("its class's setup calls skip first"));
                    }
                }
                let holds_tests = outer_holds && is_test_class(node, &name, src);
                open.push((
                    node.end_byte(),
                    Scope::Class {
                        name,
                        holds_tests,
                        skipped,
                        why,
                        rows,
                    },
                ));
            }
            "function_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map_or("", |name| text(name, src));
                let (is_test, class_skipped, class_why, mut rows) =
                    match open.last().map(|(_, scope)| scope) {
                        None => (true, false, None, Vec::new()),
                        Some(Scope::Class {
                            holds_tests,
                            skipped,
                            why,
                            rows,
                            ..
                        }) => (*holds_tests, *skipped, *why, rows.clone()),
                        Some(Scope::Function { .. }) => (false, false, None, Vec::new()),
                    };
                let mut test = None;
                if is_test && name.starts_with("test") {
                    let mut skipped = module_skipped || class_skipped;
                    let mut own_body = true;
                    for decorator in decorators(node) {
                        if let Some(table) = parametrize(decorator, src, &names) {
                            rows.push(table);
                        } else if mark_name(decorator, src).is_some_and(|path| names.is_skip(&path))
                        {
                            skipped = true;
                        }
                        own_body &= names.is_known_decorator(decorator, src);
                    }
                    let why = if skipped {
                        class_why.or(module_why)
                    } else {
                        not_run(node, src, &names, &dead, own_body)
                    };
                    skipped |= why.is_some();
                    let weight = rows
                        .iter()
                        .fold(1usize, |product, (rows, _)| product.saturating_mul(*rows));
                    let running = rows.iter().fold(1usize, |product, (rows, skipped)| {
                        product.saturating_mul(rows.saturating_sub(*skipped))
                    });
                    let id = open
                        .iter()
                        .filter_map(|(_, scope)| match scope {
                            Scope::Class { name, .. } => Some(name.as_str()),
                            Scope::Function { .. } => None,
                        })
                        .chain(std::iter::once(name))
                        .collect::<Vec<_>>()
                        .join("::");
                    let whole = node
                        .parent()
                        .filter(|parent| parent.kind() == "decorated_definition")
                        .unwrap_or(node);
                    let mut uses = BTreeSet::new();
                    walk(whole, |inner| {
                        if inner.kind() == "identifier" {
                            uses.insert(text(inner, src).to_owned());
                        }
                        true
                    });
                    test = Some(file.tests.len());
                    file.tests.push(TestCase {
                        id,
                        line: node.start_position().row + 1,
                        skipped,
                        skipped_rows: if skipped {
                            0
                        } else {
                            weight.saturating_sub(running)
                        },
                        why,
                        marker: false,
                        focused: false,
                        uses,
                        weight,
                    });
                } else if dead.why(node).is_none() && autouse_skips(node, src, &names) {
                    file.tests.push(TestCase {
                        id: format!("autouse fixture {name}"),
                        line: node.start_position().row + 1,
                        skipped: true,
                        skipped_rows: 0,
                        why: Some("an autouse fixture skips every test it reaches"),
                        marker: true,
                        focused: false,
                        uses: BTreeSet::new(),
                        weight: 1,
                    });
                }
                // A function inside a test belongs to it.
                let test = test.or_else(|| {
                    open.iter().rev().find_map(|(_, scope)| match scope {
                        Scope::Function { test } => *test,
                        Scope::Class { .. } => None,
                    })
                });
                open.push((node.end_byte(), Scope::Function { test }));
            }
            "assert_statement" | "call" => {
                let can_fail = if node.kind() == "assert_statement" {
                    Some(assert_can_fail(node, src))
                } else {
                    call_assertion(node, src, &names)
                };
                if can_fail == Some(true) && dead.why(node).is_none() {
                    let test = open.iter().rev().find_map(|(_, scope)| match scope {
                        Scope::Function { test } => *test,
                        Scope::Class { .. } => None,
                    });
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
                }
            }
            _ => {}
        }
        true
    });
    // Each file a conftest.py keeps pytest from collecting: one skipped
    // pseudo-test per entry, so adding one is a skip and moving one is not.
    for (variable, entry) in collect_ignored(root, src, &dead) {
        let Some(value) = string_value(entry, src) else {
            continue;
        };
        file.tests.push(TestCase {
            id: format!("{variable}: {value}"),
            line: entry.start_position().row + 1,
            skipped: true,
            skipped_rows: 0,
            why: Some("collect_ignore in conftest.py"),
            marker: true,
            focused: false,
            uses: BTreeSet::new(),
            weight: 1,
        });
    }
    file
}

/// True for a path the guard reads as a test file: a `.py` file named
/// `test_*.py`, `*_test.py` or `conftest.py`, or in a folder named `test`
/// or `tests`.
#[must_use]
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let Some(stem) = name.strip_suffix(".py") else {
        return false;
    };
    stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem == "conftest"
        || folders
            .iter()
            .any(|folder| matches!(*folder, "test" | "tests"))
}

/// The repo file a module spec names in an import from `from`, among the
/// paths `exists` accepts: a relative `.models`, `..pkg.mod`, `.`, or an
/// absolute `app.models` looked for from the folder of `from` up to the repo
/// root, and in a `src` folder at each. `None` for the standard library, a
/// package, or anything else.
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let mut folder: Vec<&str> = from.split('/').filter(|part| !part.is_empty()).collect();
    folder.pop();
    let dots = spec.chars().take_while(|&c| c == '.').count();
    let rest = &spec[dots..];
    let parts: Vec<&str> = if rest.is_empty() {
        Vec::new()
    } else {
        rest.split('.').collect()
    };
    if parts
        .iter()
        .any(|part| part.is_empty() || part.contains(['/', '\\']))
    {
        return None;
    }
    let candidates = |base: &[&str]| -> [String; 2] {
        let module = base
            .iter()
            .chain(parts.iter())
            .copied()
            .collect::<Vec<_>>()
            .join("/");
        let package = if module.is_empty() {
            "__init__.py".to_owned()
        } else {
            format!("{module}/__init__.py")
        };
        [format!("{module}.py"), package]
    };
    if dots > 0 {
        for _ in 1..dots {
            folder.pop()?;
        }
        let [module, package] = candidates(&folder);
        if parts.is_empty() {
            return exists(&package).then_some(package);
        }
        return [module, package].into_iter().find(|path| exists(path));
    }
    if parts.is_empty() {
        return None;
    }
    loop {
        let mut in_src = folder.clone();
        in_src.push("src");
        for base in [&folder, &in_src] {
            if let Some(found) = candidates(base).into_iter().find(|path| exists(path)) {
                return Some(found);
            }
        }
        folder.pop()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(file: &TestFile) -> Vec<(&str, bool)> {
        file.tests
            .iter()
            .map(|test| (test.id.as_str(), test.skipped))
            .collect()
    }

    fn shapes(file: &TestFile) -> Vec<&str> {
        file.assertions
            .iter()
            .map(|assertion| assertion.shape.as_str())
            .collect()
    }

    #[test]
    fn module_functions_and_test_class_methods_are_tests() {
        let file = read(
            r#"
import unittest
from app.pricing import total

def test_add():
    assert total([1, 2]) == 3

async def test_async():
    assert await total([]) == 0

def check(x):
    assert x

class TestMath:
    def test_sub(self):
        assert 3 - 1 == total([2])

    def helper(self):
        pass

    class TestInner:
        def test_deep(self):
            assert total([1]) == 1

class Helpers:
    def test_not_collected(self):
        assert total([]) == 0

class Prices(unittest.TestCase):
    def test_eq(self):
        self.assertEqual(total([1]), 1)

def test_outer():
    def test_nested():
        assert total([]) == 0
    test_nested()
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(
            ids(&file),
            [
                ("test_add", false),
                ("test_async", false),
                ("TestMath::test_sub", false),
                ("TestMath::TestInner::test_deep", false),
                ("Prices::test_eq", false),
                ("test_outer", false),
            ]
        );
        assert_eq!(file.tests[0].line, 5);
        assert!(file.tests[0].uses.contains("total"));
        let tests: Vec<Option<usize>> = file.assertions.iter().map(|a| a.test).collect();
        // `check`'s assert, and the one in the non-test class, belong to no
        // test; the nested function's to `test_outer`.
        assert_eq!(
            tests,
            [
                Some(0),
                Some(1),
                None,
                Some(2),
                Some(3),
                None,
                Some(4),
                Some(5)
            ]
        );
        assert_eq!(
            file.imports.get("total").map(String::as_str),
            Some("app.pricing")
        );
    }

    #[test]
    fn the_line_is_the_def_and_uses_include_decorators() {
        let file = read("import pytest\n\n@pytest.mark.parametrize(\"n\", CASES)\ndef test_a(n, cart):\n    assert n\n");
        assert_eq!(file.tests[0].line, 4);
        for name in ["pytest", "CASES", "n", "cart"] {
            assert!(file.tests[0].uses.contains(name), "{name}");
        }
    }

    #[test]
    fn comments_and_strings_are_not_tests_or_assertions() {
        let file = read(
            "# def test_x():\n#     assert x == 1\nS = \"def test_y(): assert y\"\n\ndef test_z():\n    \"\"\"assert z\"\"\"\n    # assert z == 1\n    print(\"self.assertEqual(a, b)\")\n",
        );
        assert_eq!(ids(&file), [("test_z", false)]);
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn pytest_marks_skip() {
        let file = read(
            r#"
import pytest
from pytest import mark

@pytest.mark.skip(reason="later")
def test_a():
    assert f() == 1

@pytest.mark.skipif(sys.platform == "win32", reason="posix")
def test_b():
    assert f() == 1

@mark.xfail
def test_c():
    assert f() == 1

@pytest.mark.slow
def test_d():
    assert f() == 1

@pytest.mark.usefixtures("db")
def test_e():
    assert f() == 1

@pytest.mark.skip
class TestOff:
    def test_f(self):
        assert f() == 1
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("test_a", true),
                ("test_b", true),
                ("test_c", true),
                ("test_d", false),
                ("test_e", false),
                ("TestOff::test_f", true),
            ]
        );
        assert!(file.tests.iter().all(|test| test.why.is_none()));
        // A marker skips the test; its assertions are still read.
        assert_eq!(file.assertions.len(), 6);
    }

    #[test]
    fn unittest_skips() {
        let file = read(
            r#"
import unittest
import unittest as ut
from unittest import skip, expectedFailure as known
from other import skipIf

class T(unittest.TestCase):
    @unittest.skip("x")
    def test_a(self):
        self.assertEqual(f(), 1)

    @ut.skipUnless(HAVE, "x")
    def test_b(self):
        self.assertEqual(f(), 1)

    @skip("x")
    def test_c(self):
        self.assertEqual(f(), 1)

    @known
    def test_d(self):
        self.assertEqual(f(), 1)

    @skipIf(True, "not unittest's")
    def test_e(self):
        self.assertEqual(f(), 1)

    @unittest.mock.patch("x")
    def test_f(self, patched):
        self.assertEqual(f(), 1)

@unittest.skipIf(OLD, "x")
class U(unittest.IsolatedAsyncioTestCase):
    async def test_g(self):
        self.assertEqual(await f(), 1)
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("T::test_a", true),
                ("T::test_b", true),
                ("T::test_c", true),
                ("T::test_d", true),
                ("T::test_e", false),
                ("T::test_f", false),
                ("U::test_g", true),
            ]
        );
    }

    #[test]
    fn pytestmark_skips_the_module_or_the_class() {
        let module = read(
            "import pytest\npytestmark = [pytest.mark.slow, pytest.mark.skipif(OLD, reason=\"x\")]\n\ndef test_a():\n    assert f()\n",
        );
        assert_eq!(ids(&module), [("test_a", true)]);
        let class = read(
            "import pytest\n\nclass TestA:\n    pytestmark = pytest.mark.xfail\n    def test_a(self):\n        assert f()\n\nclass TestB:\n    pytestmark = (pytest.mark.slow,)\n    def test_b(self):\n        assert f()\n\ndef test_c():\n    assert f()\n",
        );
        assert_eq!(
            ids(&class),
            [
                ("TestA::test_a", true),
                ("TestB::test_b", false),
                ("test_c", false)
            ]
        );
        let not_a_skip =
            read("pytestmark = pytest.mark.django_db\n\ndef test_a():\n    assert f()\n");
        assert_eq!(ids(&not_a_skip), [("test_a", false)]);
    }

    #[test]
    fn a_skip_call_first_skips_the_test() {
        let file = read(
            r#"
import pytest
from pytest import skip as later

def test_a():
    """Docstring first."""
    pytest.skip("later")
    assert f() == 1

def test_b():
    later("x")

class T(unittest.TestCase):
    def test_c(self):
        self.skipTest("x")
        self.assertEqual(f(), 1)

def test_d():
    if OLD:
        pytest.skip("old")
    assert f() == 1

def test_e():
    raise unittest.SkipTest("x")
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("test_a", true),
                ("test_b", true),
                ("T::test_c", true),
                ("test_d", false),
                ("test_e", true),
            ]
        );
        assert_eq!(file.tests[0].why, Some("it calls skip first"));
        assert_eq!(file.tests[3].why, None);
        // What follows the skip never runs.
        assert_eq!(shapes(&file), ["assertf()==1"]);
        assert_eq!(file.assertions[0].test, Some(3));
    }

    #[test]
    fn a_body_that_does_nothing_is_skipped() {
        let file = read(
            "def test_pass():\n    pass\n\ndef test_dots():\n    ...\n\ndef test_doc():\n    \"\"\"Only a docstring.\"\"\"\n\ndef test_return():\n    # a comment\n    return\n    assert f() == 1\n\ndef test_real():\n    \"\"\"Docstring.\"\"\"\n    assert f() == 1\n",
        );
        assert_eq!(
            ids(&file),
            [
                ("test_pass", true),
                ("test_dots", true),
                ("test_doc", true),
                ("test_return", true),
                ("test_real", false),
            ]
        );
        assert_eq!(file.tests[0].why, Some("its body does nothing"));
        assert_eq!(file.tests[3].why, Some("its body does nothing"));
        assert_eq!(shapes(&file), ["assertf()==1"]);
        assert_eq!(file.assertions[0].test, Some(4));
    }

    #[test]
    fn code_that_never_runs_is_not_counted() {
        let file = read(
            r#"
def test_early_return():
    cart = make()
    return
    assert cart.total == 3
    cart.assert_paid()

def test_if_false():
    cart = make()
    if False:
        assert cart.total == 3
    if (False):
        assert cart.total == 4
    while False:
        assert cart.total == 5
    if True:
        assert cart.total == 6

def test_return_in_branch():
    cart = make()
    if cart.empty:
        return
    assert cart.total == 7

if False:
    def test_hidden():
        assert make().total == 8
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("test_early_return", false),
                ("test_if_false", false),
                ("test_return_in_branch", false),
                ("test_hidden", true),
            ]
        );
        assert_eq!(file.tests[3].why, Some("inside if (false)"));
        assert_eq!(
            shapes(&file),
            ["assertcart.total==6", "assertcart.total==7"]
        );
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"
def test_a(self, mock):
    assert True
    assert 1 == 1, "placeholder"
    assert "x"
    assert not None
    assert (1, 2)
    assert f"{x}"
    assert x
    self.assertTrue(True)
    self.assertEqual(1, 1)
    self.assertEqual(-1.5, 3 * 2, msg=reason)
    assertTrue(True)
    self.assertEqual(x, 3)
    self.assertTrue(True, x)
    mock.assert_called_once()
    mock.assert_called_once_with(1)
    np.testing.assert_allclose(a, b)
    assert_frame_equal(a, b)
    self.assertRaises(ValueError, f)
    self.fail_soon(1)
"#,
        );
        // `-1.5 == 3 * 2` is literals, but not written the same: it fails.
        assert_eq!(
            shapes(&file),
            [
                "assertf\"{x}\"",
                "assertx",
                "self.assertEqual(-1.5,3*2,msg=reason)",
                "self.assertEqual(x,3)",
                "self.assertTrue(True,x)",
                "mock.assert_called_once()",
                "mock.assert_called_once_with(1)",
                "np.testing.assert_allclose(a,b)",
                "assert_frame_equal(a,b)",
                "self.assertRaises(ValueError,f)",
            ]
        );
    }

    #[test]
    fn assertions_that_certainly_fail_count() {
        let file = read(
            r#"
import pytest
from pytest import fail

def test_raises():
    try:
        parse("x")
        assert False, "should raise"
    except ValueError:
        pass

class T(unittest.TestCase):
    def test_a(self):
        assert 0
        assert 0.0
        assert None
        assert ""
        assert []
        assert ()
        assert {}
        assert [*rest]
        assert not True
        assert 1 == 2
        assert 1 != 1
        assert 1 == 1 == 2
        assert [] is []
        assert 1 + 1 == 2
        self.assertTrue(False)
        self.assertTrue(0)
        self.assertFalse(True)
        self.assertFalse("x")
        self.assertEqual(1, 2)
        self.assertIs([], [])
        self.assertIn(1, [1])
        self.fail("unreachable")
        pytest.fail("unreachable")
        fail("unreachable")
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "assertFalse,\"shouldraise\"",
                "assert0",
                "assert0.0",
                "assertNone",
                "assert\"\"",
                "assert[]",
                "assert()",
                "assert{}",
                "assert[*rest]",
                "assertnotTrue",
                "assert1==2",
                "assert1!=1",
                "assert1==1==2",
                "assert[]is[]",
                "assert1+1==2",
                "self.assertTrue(False)",
                "self.assertTrue(0)",
                "self.assertFalse(True)",
                "self.assertFalse(\"x\")",
                "self.assertEqual(1,2)",
                "self.assertIs([],[])",
                "self.assertIn(1,[1])",
                "self.fail(\"unreachable\")",
                "pytest.fail(\"unreachable\")",
                "fail(\"unreachable\")",
            ]
        );
        assert_eq!(file.assertions[0].test, Some(0));
    }

    #[test]
    fn assertions_that_certainly_pass_are_not_counted() {
        let file = read(
            r#"
class T(unittest.TestCase):
    def test_a(self):
        assert 1
        assert -2.5
        assert 0x10
        assert 1e-3
        assert "x" "y"
        assert b"x"
        assert [x]
        assert {"a": 1}
        assert {x}
        assert (x == 3, "msg")
        assert (total(cart), 3)
        assert ((x, y))
        assert not 0
        assert not ""
        assert None is None
        assert "a" == "a"
        assert (1, 2) == (1,  2)
        assert [1] == [1]
        self.assertTrue(1)
        self.assertTrue("yes")
        self.assertFalse(None)
        self.assertFalse("")
        self.assertFalse([])
        self.assertEqual("a", "a")
        self.assertEquals(1, 1)
        self.assertIs(None, None)
        other.fail("x")
        self.fail_soon()
        failing("x")
"#,
        );
        assert_eq!(shapes(&file), Vec::<&str>::new());
        // A tuple whose items are only splats may be empty, so it counts.
        assert_eq!(
            shapes(&read("def test_a():\n    assert (*xs,)\n")),
            ["assert(*xs,)"]
        );
    }

    #[test]
    fn numbers_read_as_zero() {
        for zero in [
            "0", "00", "0.0", ".0", "0.", "0e5", "0E-3", "0x0", "0o0", "0b0", "0j", "0_0",
        ] {
            assert!(is_zero(zero), "{zero}");
        }
        for not_zero in ["1", "0.5", "1e-9", "0x10", "0xe", "0b1", "1j", "10", ""] {
            assert!(!is_zero(not_zero), "{not_zero}");
        }
    }

    #[test]
    fn a_named_skip_mark_skips() {
        let file = read(
            r#"
import pytest
from pytest import mark

needs_db = pytest.mark.skipif(True, reason="db")
later = pytest.mark.skip
flaky = mark.xfail(reason="flaky")
also_later = later
slow = pytest.mark.slow
renamed = pytest.mark.skip
renamed = pytest.mark.slow

@needs_db
def test_a():
    assert f() == 1

@later(reason="x")
def test_b():
    assert f() == 1

@flaky
class TestC:
    def test_c(self):
        assert f() == 1

@also_later
def test_d():
    assert f() == 1

@slow
def test_e():
    assert f() == 1

@renamed
def test_f():
    assert f() == 1

def helper():
    local = pytest.mark.skip

@local
def test_g():
    assert f() == 1

@pytest.mark.parametrize("a", [1, pytest.param(2, marks=needs_db)])
def test_h(a):
    assert a
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("test_a", true),
                ("test_b", true),
                ("TestC::test_c", true),
                ("test_d", true),
                ("test_e", false),
                ("test_f", false),
                ("test_g", false),
                ("test_h", false),
            ]
        );
        assert_eq!(file.tests[7].skipped_rows, 1);
        let module = read("import pytest\nwindows_only = pytest.mark.skipif(not WIN, reason=\"w\")\npytestmark = windows_only\n\ndef test_a():\n    assert f()\n");
        assert_eq!(ids(&module), [("test_a", true)]);
    }

    #[test]
    fn dunder_test_false_skips_the_module_or_the_class() {
        let module = read("__test__ = False\n\ndef test_a():\n    assert f()\n\nclass TestB:\n    def test_b(self):\n        assert f()\n");
        assert_eq!(ids(&module), [("test_a", true), ("TestB::test_b", true)]);
        assert!(module
            .tests
            .iter()
            .all(|test| test.why == Some("__test__ = False")));
        let class = read("class TestA:\n    __test__ = False\n    def test_a(self):\n        assert f()\n\nclass TestB:\n    __test__ = True\n    def test_b(self):\n        assert f()\n\ndef test_c():\n    __test__ = False\n    assert f()\n");
        assert_eq!(
            ids(&class),
            [
                ("TestA::test_a", true),
                ("TestB::test_b", false),
                ("test_c", false)
            ]
        );
        assert_eq!(class.tests[0].why, Some("__test__ = False"));
    }

    #[test]
    fn a_module_level_importorskip_skips_the_module() {
        for source in [
            "import pytest\nnp = pytest.importorskip(\"numpy\")\n\ndef test_a():\n    assert np.ones(1)[0] == 1\n",
            "import pytest\npytest.importorskip(\"numpy\", minversion=\"2\")\n\ndef test_a():\n    assert f()\n",
            "from pytest import importorskip\nnp = importorskip(\"numpy\")\n\ndef test_a():\n    assert f()\n",
        ] {
            let file = read(source);
            assert_eq!(ids(&file), [("test_a", true)], "{source}");
            assert_eq!(file.tests[0].why, None);
        }
        // In a test, it is that test's business; elsewhere not pytest's.
        for source in [
            "import pytest\n\ndef test_a():\n    np = pytest.importorskip(\"numpy\")\n    assert np.ones(1)[0] == 1\n\ndef test_b():\n    assert f()\n",
            "np = importorskip(\"numpy\")\n\ndef test_b():\n    assert f()\n",
        ] {
            let file = read(source);
            assert!(file.tests.iter().all(|test| !test.skipped), "{source}");
        }
    }

    #[test]
    fn a_setup_that_skips_first_skips_the_class() {
        let file = read(
            r#"
import unittest
import pytest

class A(unittest.TestCase):
    def setUp(self):
        """Not on CI."""
        self.skipTest("not on CI")
        self.db = connect()

    def test_a(self):
        self.assertEqual(f(), 1)

class B(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise unittest.SkipTest("no db")

    def test_b(self):
        self.assertEqual(f(), 1)

class TestC:
    def setup_method(self, method):
        pytest.skip("later")

    def test_c(self):
        assert f() == 1

class D(unittest.TestCase):
    def setUp(self):
        self.db = connect()
        if self.db is None:
            self.skipTest("no db")

    def test_d(self):
        self.assertEqual(f(), 1)

class TestE:
    def helper(self):
        pytest.skip("not setup")

    def test_e(self):
        assert f() == 1
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("A::test_a", true),
                ("B::test_b", true),
                ("TestC::test_c", true),
                ("D::test_d", false),
                ("TestE::test_e", false),
            ]
        );
        assert_eq!(
            file.tests[0].why,
            Some("its class's setup calls skip first")
        );
    }

    #[test]
    fn a_module_level_skip_skips_what_follows() {
        for source in [
            "import unittest\n\ndef test_before():\n    assert f()\n\nraise unittest.SkipTest(\"posix only\")\n\ndef test_after():\n    assert f()\n\nclass TestAfter:\n    def test_m(self):\n        assert f()\n",
            "from unittest import SkipTest\n\ndef test_before():\n    assert f()\n\nraise SkipTest\n\ndef test_after():\n    assert f()\n\nclass TestAfter:\n    def test_m(self):\n        assert f()\n",
            "import pytest\n\ndef test_before():\n    assert f()\n\npytest.skip(\"posix only\", allow_module_level=True)\n\n@pytest.mark.slow\ndef test_after():\n    assert f()\n\nclass TestAfter:\n    def test_m(self):\n        assert f()\n",
            "import pytest\n\ndef test_before():\n    assert f()\n\nraise pytest.skip.Exception(\"x\", allow_module_level=True)\n\ndef test_after():\n    assert f()\n\nclass TestAfter:\n    def test_m(self):\n        assert f()\n",
        ] {
            let file = read(source);
            assert_eq!(
                ids(&file),
                [
                    ("test_before", false),
                    ("test_after", true),
                    ("TestAfter::test_m", true)
                ],
                "{source}"
            );
            assert_eq!(file.tests[1].why, Some("after a skip"), "{source}");
            assert_eq!(file.tests[2].why, Some("after a skip"), "{source}");
            assert_eq!(file.assertions.len(), 1, "{source}");
        }
        // Only when the module can't be imported: the tests may run.
        let file = read("import unittest\ntry:\n    import numpy\nexcept ImportError:\n    raise unittest.SkipTest(\"numpy\")\n\ndef test_a():\n    assert f()\n\nraise ValueError(\"not a skip\")\n\ndef test_b():\n    assert f()\n");
        assert_eq!(ids(&file), [("test_a", false), ("test_b", false)]);
    }

    #[test]
    fn collect_ignore_entries_are_skipped_pseudo_tests() {
        let file = read(
            r#"
import sys

collect_ignore = ["setup.py", "legacy/test_old.py"]
collect_ignore_glob = ("*_slow.py",)
collect_ignore += ["build"]
collect_ignore.append("docs/conf.py")
collect_ignore.extend(["a.py", f"{x}.py"])
if sys.version_info < (3, 10):
    collect_ignore.append("tests/test_match.py")
if False:
    collect_ignore.append("never.py")

def pytest_configure(config):
    collect_ignore.append("inside.py")

ignored = ["not_this.py"]
others.append("nor_this.py")
"#,
        );
        let found: Vec<&str> = file.tests.iter().map(|test| test.id.as_str()).collect();
        assert_eq!(
            found,
            [
                "collect_ignore: setup.py",
                "collect_ignore: legacy/test_old.py",
                "collect_ignore_glob: *_slow.py",
                "collect_ignore: build",
                "collect_ignore: docs/conf.py",
                "collect_ignore: a.py",
                "collect_ignore: tests/test_match.py",
            ]
        );
        assert!(file.tests.iter().all(|test| test.skipped
            && test.weight == 1
            && test.why == Some("collect_ignore in conftest.py")));
        assert_eq!(file.tests[0].line, 4);
    }

    #[test]
    fn an_autouse_fixture_that_skips_is_a_skipped_pseudo_test() {
        let file = read(
            r#"
import pytest
from pytest import fixture

@pytest.fixture(autouse=True)
def no_network():
    """Network tests are off."""
    pytest.skip("no network")

@fixture(autouse=True, scope="session")
def no_db():
    raise unittest.SkipTest("no db")

@pytest.fixture(autouse=True)
def tmp_home(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    if not tmp_path.exists():
        pytest.skip("no tmp")

@pytest.fixture
def skipper():
    pytest.skip("asked for")

@pytest.fixture(autouse=False)
def off():
    pytest.skip("x")

@other.fixture(autouse=True)
def not_pytest():
    pytest.skip("x")
"#,
        );
        let found: Vec<(&str, bool)> = ids(&file);
        assert_eq!(
            found,
            [
                ("autouse fixture no_network", true),
                ("autouse fixture no_db", true)
            ]
        );
        assert_eq!(
            file.tests[0].why,
            Some("an autouse fixture skips every test it reaches")
        );
        assert_eq!(file.tests[0].weight, 1);
    }

    #[test]
    fn a_decorator_pytest_does_not_know_may_supply_the_body() {
        let file = read(
            r#"
import pytest
from pytest_bdd import scenario

@scenario("cart.feature", "Add")
def test_add():
    pass

@pytest.mark.slow
def test_slow():
    pass

@pytest.mark.parametrize("a", [1])
def test_rows(a):
    """Nothing."""

@pytest.mark.usefixtures("db")
@scenario("cart.feature", "Remove")
def test_remove():
    pass

@scenario("cart.feature", "Skip")
def test_skip_first():
    pytest.skip("later")
"#,
        );
        assert_eq!(
            ids(&file),
            [
                ("test_add", false),
                ("test_slow", true),
                ("test_rows", true),
                ("test_remove", false),
                ("test_skip_first", true),
            ]
        );
        assert_eq!(file.tests[0].why, None);
        assert_eq!(file.tests[1].why, Some("its body does nothing"));
    }

    #[test]
    fn parametrize_rows_from_a_module_constant() {
        let file = read(
            r#"
import pytest

CASES = [(1, 2), (3, 4)]
TUPLE = (1, 2, 3)
BARE = 1, 2, 3, 4
WITH_SKIP = [1, pytest.param(2, marks=pytest.mark.skip), 3]
TWICE = [1]
TWICE = [1, 2]
CALLED = list(range(5))

@pytest.mark.parametrize("a,b", CASES)
def test_a(a, b):
    assert a < b

@pytest.mark.parametrize("a", TUPLE)
def test_b(a):
    assert a

@pytest.mark.parametrize("a", argvalues=BARE)
def test_c(a):
    assert a

@pytest.mark.parametrize("a", WITH_SKIP)
def test_d(a):
    assert a

@pytest.mark.parametrize("a", TWICE)
def test_e(a):
    assert a

@pytest.mark.parametrize("a", CALLED)
def test_f(a):
    assert a

@pytest.mark.parametrize("a", UNKNOWN)
def test_g(a):
    assert a

def helper():
    LOCAL = [1, 2, 3]

@pytest.mark.parametrize("a", LOCAL)
def test_h(a):
    assert a
"#,
        );
        let weights: Vec<(usize, usize)> = file
            .tests
            .iter()
            .map(|test| (test.weight, test.skipped_rows))
            .collect();
        assert_eq!(
            weights,
            [
                (2, 0),
                (3, 0),
                (4, 0),
                (3, 1),
                (1, 0),
                (1, 0),
                (1, 0),
                (1, 0)
            ]
        );
        assert_eq!(file.assertions[0].weight, 2);
    }

    #[test]
    fn pytest_raises_and_warns_are_assertions() {
        let file = read(
            "import pytest\nfrom pytest import raises\n\ndef test_a():\n    with pytest.raises(ValueError, match=\"bad\"):\n        parse(\"x\")\n    with pytest.warns(UserWarning), raises(KeyError):\n        parse(\"y\")\n    with open(\"f\") as handle:\n        pass\n",
        );
        assert_eq!(
            shapes(&file),
            [
                "pytest.raises(ValueError,match=\"bad\")",
                "pytest.warns(UserWarning)",
                "raises(KeyError)",
            ]
        );
        assert_eq!(
            file.assertions[0].text,
            "pytest.raises(ValueError, match=\"bad\")"
        );
        assert_eq!(file.assertions[0].line, 5);
    }

    #[test]
    fn parametrize_weighs_tests_and_assertions() {
        let file = read(
            r#"
import pytest
from pytest import param

@pytest.mark.parametrize("a, b", [(1, 2), (2, 3), pytest.param(3, 4, marks=pytest.mark.skip)])
def test_rows(a, b):
    assert a + 1 == b

@pytest.mark.parametrize("a", (1, 2))
@pytest.mark.parametrize("b", argvalues=[1, param(2, marks=[pytest.mark.slow, pytest.mark.xfail]), 3])
def test_grid(a, b):
    assert a * b

@pytest.mark.parametrize("a", CASES)
def test_named(a):
    assert a

@pytest.mark.parametrize("a", [])
def test_empty(a):
    assert a

@pytest.mark.parametrize("x", [1, 2])
class TestClass:
    @pytest.mark.parametrize("y", [1, 2, 3])
    def test_xy(self, x, y):
        assert x < y

@pytest.mark.skip
@pytest.mark.parametrize("a", [1, pytest.param(2, marks=pytest.mark.skip)])
def test_off(a):
    assert a
"#,
        );
        let weights: Vec<(usize, usize)> = file
            .tests
            .iter()
            .map(|test| (test.weight, test.skipped_rows))
            .collect();
        assert_eq!(weights, [(3, 1), (6, 2), (1, 0), (1, 0), (6, 0), (2, 0)]);
        assert!(file.tests[..5].iter().all(|test| !test.skipped));
        let weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(weights, [3, 6, 1, 1, 6, 2]);
    }

    #[test]
    fn imports_name_their_modules() {
        let file = read(
            "from __future__ import annotations\nimport os\nimport app.models\nimport app.util as u\nfrom app.pricing import total, tax as vat\nfrom .models import Cart\nfrom ..pkg.mod import thing\nfrom . import helpers\nfrom .. import shared as sh\nfrom app import *\n",
        );
        let found: Vec<(&str, &str)> = file
            .imports
            .iter()
            .map(|(name, spec)| (name.as_str(), spec.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("Cart", ".models"),
                ("app", "app.models"),
                ("helpers", ".helpers"),
                ("os", "os"),
                ("sh", "..shared"),
                ("thing", "..pkg.mod"),
                ("total", "app.pricing"),
                ("u", "app.util"),
                ("vat", "app.pricing"),
            ]
        );
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read("def test_a():\n    assert f() == 1\n\ndef test_b(:\n    pass\n");
        assert!(file.has_errors);
        assert_eq!(file.error_line, Some(4));
        assert!(!read("def test_a():\n    assert f() == 1\n").has_errors);
    }

    #[test]
    fn odd_input_does_not_panic() {
        let deep = format!(
            "def test_a():\n    assert {}x{}\n",
            "(".repeat(5000),
            ")".repeat(5000)
        );
        let file = read(&deep);
        assert_eq!(file.tests.len(), 1);
        let chain = format!(
            "def test_a():\n    x{}.assert_called()\n",
            ".a".repeat(20_000)
        );
        assert_eq!(read(&chain).assertions.len(), 1);
        let nested: String = (0..300)
            .map(|depth| format!("{}if x:\n", "    ".repeat(depth)))
            .collect();
        let _ = read(&format!("{nested}{}assert y\n", "    ".repeat(300)));
        let nots = read(&format!(
            "def test_a():\n    assert {}True\n",
            "not ".repeat(5000)
        ));
        assert_eq!(nots.assertions.len(), 1);
        let lists = read(&format!(
            "collect_ignore = {}\"a.py\"{}\n",
            "[".repeat(3000),
            "]".repeat(3000)
        ));
        assert_eq!(lists.tests.len(), 1);
        let sums = read(&format!(
            "collect_ignore = [\"a\"]{}\n",
            " + [\"b\"]".repeat(5000)
        ));
        assert_eq!(sums.tests.len(), 5001);
        for sample in [
            "collect_ignore =",
            "collect_ignore.append(",
            "collect_ignore += 1",
            "__test__ = False\n@",
            "x = pytest.mark.skip\n@x(",
            "@pytest.fixture(autouse=True)\ndef f(",
            "assert",
            "assert (",
            "assert 0x",
            "",
            "\u{feff}",
            "@",
            "class",
            "def test_(",
            "pytestmark =",
            "\0\0",
        ] {
            let _ = read(sample);
        }
    }

    #[test]
    fn test_paths() {
        for path in [
            "test_pricing.py",
            "app/pricing_test.py",
            "tests/helpers.py",
            "pkg/test/x.py",
            "conftest.py",
            "tests/unit/conftest.py",
            "src/Test_Upper.PY",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "app/pricing.py",
            "testing/x.py",
            "app/contest.py",
            "tests/a.ts",
            "test_x.pyc",
            "app/latest.py",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn module_specs_resolve_to_repo_files() {
        let files = [
            "app/pricing.py",
            "app/__init__.py",
            "app/models/__init__.py",
            "tests/helpers.py",
            "tests/__init__.py",
            "src/lib/core.py",
            "pkg/sub/mod.py",
            "pkg/sub/tests/test_mod.py",
        ];
        let exists = |path: &str| files.contains(&path);
        let at = |from: &str, spec: &str| resolve(from, spec, &exists);
        assert_eq!(
            at("tests/test_p.py", "app.pricing").as_deref(),
            Some("app/pricing.py")
        );
        assert_eq!(
            at("tests/test_p.py", "app.models").as_deref(),
            Some("app/models/__init__.py")
        );
        assert_eq!(
            at("tests/test_p.py", "app").as_deref(),
            Some("app/__init__.py")
        );
        assert_eq!(
            at("tests/test_p.py", "tests.helpers").as_deref(),
            Some("tests/helpers.py")
        );
        assert_eq!(
            at("tests/test_p.py", "lib.core").as_deref(),
            Some("src/lib/core.py")
        );
        assert_eq!(
            at("tests/test_p.py", ".helpers").as_deref(),
            Some("tests/helpers.py")
        );
        assert_eq!(
            at("tests/test_p.py", ".").as_deref(),
            Some("tests/__init__.py")
        );
        assert_eq!(
            at("tests/test_p.py", "..app.pricing").as_deref(),
            Some("app/pricing.py")
        );
        assert_eq!(
            at("pkg/sub/tests/test_mod.py", "..mod").as_deref(),
            Some("pkg/sub/mod.py")
        );
        assert_eq!(
            at("test_root.py", ".app.pricing").as_deref(),
            Some("app/pricing.py")
        );
        assert_eq!(at("test_root.py", "..app"), None);
        assert_eq!(at("tests/test_p.py", "os.path"), None);
        assert_eq!(at("tests/test_p.py", "pytest"), None);
        assert_eq!(at("tests/test_p.py", "app..pricing"), None);
        assert_eq!(at("tests/test_p.py", ""), None);
    }
}
