//! The Python reader of the tamper guard (M24, ADR 0044): pytest and
//! unittest.
//!
//! - Tests: `def test*` at module level, and `def test*` methods of a class
//!   named `Test*` or one whose superclass ends with `TestCase`.
//! - Skips: `@pytest.mark.skip`, `skipif`, `xfail`, `unittest.skip…` on the
//!   test or its classes, `pytestmark`, a `pytest.skip(…)` or
//!   `self.skipTest(…)` first, a body that does nothing, a definition that
//!   never runs.
//! - Assertions: `assert`, any `…assert…(…)` call, `pytest.raises` and
//!   `pytest.warns`; not those that never run or cannot fail.

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

/// Values that are the same on every run: an `assert` of one of these, or
/// `self.assertTrue(True)`, cannot fail.
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
/// `pytest.skip(…)`, `pytest.xfail(…)`, `self.skipTest(…)`.
fn stops(statement: Node<'_>, src: &[u8]) -> bool {
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

/// What the file's imports bind: for `from X import a as b`, `b` → (`X`,
/// `a`); for `import X as Y`, `Y` → (`X`, `""`).
struct Names {
    bound: BTreeMap<String, (String, String)>,
}

impl Names {
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
            && self
                .imported_from(last, "unittest")
                .is_some_and(|original| UNITTEST_SKIPS.contains(&original))
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
fn imports(root: Node<'_>, src: &[u8]) -> (BTreeMap<String, String>, Names) {
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
    (specs, Names { bound })
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
fn parametrize(decorator: Node<'_>, src: &[u8], names: &Names) -> Option<(usize, usize)> {
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
    let Some(values) = values.filter(|values| matches!(values.kind(), "list" | "tuple")) else {
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

/// Why a test function does not run when no marker says so.
fn not_run(
    function: Node<'_>,
    src: &[u8],
    names: &Names,
    dead: &reach::Dead,
) -> Option<&'static str> {
    let body = function.child_by_field_name("body")?;
    let first = children(body)
        .into_iter()
        .find(|statement| !noop(*statement, src));
    if first.is_some_and(|statement| calls_skip(statement, src, names)) {
        return Some("it calls skip first");
    }
    if reach::does_nothing(body, src, &GRAMMAR) {
        return Some("its body does nothing");
    }
    dead.why(function)
}

/// True for `pytest.skip(…)`, `pytest.xfail(…)`, `skip(…)` imported from
/// pytest, `self.skipTest(…)`, or `raise unittest.SkipTest(…)`.
fn calls_skip(statement: Node<'_>, src: &[u8], names: &Names) -> bool {
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
fn call_assertion(call: Node<'_>, src: &[u8], names: &Names) -> Option<bool> {
    let function = call.child_by_field_name("function")?;
    if dotted(function, src).is_some_and(|path| names.is_pytest(&path, &["raises", "warns"])) {
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
    Some(!constant)
}

/// True for an `assert` statement that can fail: its condition is not a
/// literal. The message after the comma is not looked at.
fn assert_can_fail(statement: Node<'_>) -> bool {
    children(statement)
        .first()
        .is_some_and(|condition| !reach::constant(*condition, &LITERALS))
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
    let (specs, names) = imports(root, src);
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
    let module_skipped = pytestmark(root, src)
        .into_iter()
        .any(|value| names.skips(value, src));

    // Each class and function open around the node being visited.
    enum Scope {
        Class {
            name: String,
            /// It and every class around it can hold tests.
            holds_tests: bool,
            skipped: bool,
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
                let (outer_holds, outer_skipped, mut rows) = match outer {
                    None => (true, false, Vec::new()),
                    Some(Scope::Class {
                        holds_tests,
                        skipped,
                        rows,
                        ..
                    }) => (*holds_tests, *skipped, rows.clone()),
                    Some(Scope::Function { .. }) => (false, false, Vec::new()),
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
                }
                let holds_tests = outer_holds && is_test_class(node, &name, src);
                open.push((
                    node.end_byte(),
                    Scope::Class {
                        name,
                        holds_tests,
                        skipped,
                        rows,
                    },
                ));
            }
            "function_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map_or("", |name| text(name, src));
                let (is_test, class_skipped, mut rows) = match open.last().map(|(_, scope)| scope) {
                    None => (true, false, Vec::new()),
                    Some(Scope::Class {
                        holds_tests,
                        skipped,
                        rows,
                        ..
                    }) => (*holds_tests, *skipped, rows.clone()),
                    Some(Scope::Function { .. }) => (false, false, Vec::new()),
                };
                let mut test = None;
                if is_test && name.starts_with("test") {
                    let mut skipped = module_skipped || class_skipped;
                    for decorator in decorators(node) {
                        if let Some(table) = parametrize(decorator, src, &names) {
                            rows.push(table);
                        } else if mark_name(decorator, src).is_some_and(|path| names.is_skip(&path))
                        {
                            skipped = true;
                        }
                    }
                    let why = if skipped {
                        None
                    } else {
                        not_run(node, src, &names, &dead)
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
                        focused: false,
                        uses,
                        weight,
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
                    Some(assert_can_fail(node))
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
                    });
                }
            }
            _ => {}
        }
        true
    });
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
        assert_eq!(
            shapes(&file),
            [
                "assertf\"{x}\"",
                "assertx",
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
        for sample in [
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
