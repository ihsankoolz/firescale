//! The Dart reader of the tamper guard (M32, ADR 0052): `package:test` and
//! `flutter_test`.
//!
//! - A test is a call of `test(…)`, `testWidgets(…)` or `testGoldens(…)`; a
//!   suite is a call of `group(…)`. Its id is the descriptions of the groups
//!   around it and its own, joined with ` > ` (a description that is not a
//!   plain string is its source text), as in JavaScript.
//! - It is skipped with a `skip:` argument other than `false` or `null` (on
//!   the test or a group around it: a string gives the reason, any other
//!   value skips it at least sometimes), with `@Skip(…)` on the library or
//!   on `main` (the whole file), when its body starts with
//!   `markTestSkipped(…)`, or when its body does nothing or never runs. A
//!   `solo: true` test or group is focused: the file's other tests do not
//!   run.
//! - An assertion is a call of `expect`, `expectLater`, `expectAsync…`,
//!   `fail`, or mockito's and mocktail's `verify`, `verifyNever`,
//!   `verifyInOrder`, `verifyNoMoreInteractions` and
//!   `verifyZeroInteractions` (with the chain after it:
//!   `verify(m.f()).called(1)`), unless it never runs (after a `return`,
//!   inside `if (false)`) or cannot fail (`expect(true, isTrue)`,
//!   `expect(1, 1)`, `expect(1, equals(1))`).
//! - Subjects: the names an import `show`s, the prefix of an import `as`,
//!   and, when a file has one other import of its own package, the names
//!   its tests use that it does not define; `package:<name>/x.dart` is
//!   looked up as `lib/x.dart` in the test's folder or a folder above it.
//! - Suppressions: `// ignore: rules` (`ignore:rule`) and
//!   `// ignore_for_file: rules` (`ignore-file:rule`); `type=lint` is every
//!   lint rule (`*`).

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::suppress::{keyed, shown};
use super::{Assertion, Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

const TEST_NAMES: [&str; 3] = ["test", "testWidgets", "testGoldens"];
const SUITE_NAMES: [&str; 1] = ["group"];

/// Calls that are assertions, besides `expectAsync0` … `expectAsyncUntil6`.
const ASSERTIONS: [&str; 8] = [
    "expect",
    "expectLater",
    "fail",
    "verify",
    "verifyNever",
    "verifyInOrder",
    "verifyNoMoreInteractions",
    "verifyZeroInteractions",
];

/// Packages that are never a test's subject: Dart's and Flutter's own, the
/// test frameworks and the mocking libraries.
const EXTERNAL_PACKAGES: [&str; 14] = [
    "test",
    "test_api",
    "flutter_test",
    "flutter",
    "integration_test",
    "matcher",
    "mockito",
    "mocktail",
    "async",
    "collection",
    "meta",
    "bloc_test",
    "golden_toolkit",
    "fake_async",
];

/// Names of the test frameworks and of Dart's core library: a test that
/// uses them does not test them.
const FRAMEWORK_NAMES: [&str; 64] = [
    "expect",
    "expectLater",
    "test",
    "group",
    "setUp",
    "tearDown",
    "setUpAll",
    "tearDownAll",
    "testWidgets",
    "testGoldens",
    "verify",
    "verifyNever",
    "when",
    "any",
    "anyNamed",
    "captureAny",
    "thenReturn",
    "thenAnswer",
    "equals",
    "isTrue",
    "isFalse",
    "isNull",
    "isNotNull",
    "isA",
    "isEmpty",
    "isNotEmpty",
    "throwsA",
    "throwsException",
    "throwsArgumentError",
    "throwsStateError",
    "completion",
    "completes",
    "emits",
    "emitsInOrder",
    "emitsDone",
    "emitsError",
    "contains",
    "hasLength",
    "greaterThan",
    "lessThan",
    "closeTo",
    "same",
    "predicate",
    "find",
    "findsOneWidget",
    "findsNothing",
    "findsWidgets",
    "findsNWidgets",
    "WidgetTester",
    "markTestSkipped",
    "fail",
    "print",
    "Future",
    "Stream",
    "List",
    "Map",
    "Set",
    "String",
    "Duration",
    "DateTime",
    "Exception",
    "Error",
    "Object",
    "Iterable",
];

/// How deep an assertion chain is followed.
const MAX_CHAIN: usize = 32;

/// What the Dart grammar calls the nodes [`reach`] looks at.
const GRAMMAR: Grammar = Grammar {
    blocks: &["block"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &["while_statement"],
    hoisted: &["function_declaration", "class_declaration"],
    stops: None,
    noop: |node, _| node.kind() == "empty_statement",
};

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 18] = [
    "decimal_integer_literal",
    "decimal_floating_point_literal",
    "hex_integer_literal",
    "true",
    "false",
    "null_literal",
    "string_literal",
    "string_literal_single_quotes",
    "string_literal_double_quotes",
    "string_literal_single_quotes_multiple",
    "string_literal_double_quotes_multiple",
    "raw_string_literal_single_quotes",
    "raw_string_literal_double_quotes",
    "template_chars_single_single",
    "template_chars_double_single",
    "template_chars_single",
    "template_chars_double",
    "escape_sequence",
];

/// The name a call calls when it is a plain identifier (`test`, `expect`).
fn callee<'s>(call: Node<'_>, src: &'s [u8]) -> Option<&'s str> {
    call.child_by_field_name("function")
        .filter(|function| function.kind() == "identifier")
        .map(|function| text(function, src))
}

/// The arguments of a call: the positional ones and the named ones
/// (`skip: true` as `("skip", value)`).
fn arguments<'t>(call: Node<'t>, src: &[u8]) -> (Vec<Node<'t>>, Vec<(String, Node<'t>)>) {
    let mut positional = Vec::new();
    let mut named = Vec::new();
    let Some(list) = call.child_by_field_name("arguments") else {
        return (positional, named);
    };
    let mut cursor = list.walk();
    for argument in list
        .named_children(&mut cursor)
        .filter(|n| !n.is_extra() && !n.kind().ends_with("comment"))
    {
        if argument.kind() == "named_argument" {
            let mut inner = argument.walk();
            let mut label = String::new();
            let mut value = None;
            for part in argument.named_children(&mut inner) {
                if part.kind() == "label" {
                    label = part
                        .named_child(0)
                        .map(|name| text(name, src).to_owned())
                        .unwrap_or_default();
                } else if !part.is_extra() && !part.kind().ends_with("comment") {
                    value = Some(part);
                }
            }
            if let Some(value) = value {
                named.push((label, value));
            }
        } else {
            positional.push(argument);
        }
    }
    (positional, named)
}

/// The value of a string literal when it has no interpolation.
fn string_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let mut value = String::new();
    let mut plain = true;
    walk(node, |inner| {
        match inner.kind() {
            "template_substitution" => plain = false,
            kind if kind.starts_with("template_chars") || kind == "escape_sequence" => {
                value.push_str(text(inner, src));
                return false;
            }
            _ => {}
        }
        plain
    });
    plain.then_some(value)
}

/// The description of a test or group call: its first argument, as a
/// string when it is one, else its source text.
fn title(first: Option<Node<'_>>, src: &[u8]) -> String {
    let Some(first) = first else {
        return String::new();
    };
    string_value(first, src).unwrap_or_else(|| {
        text(first, src)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// True for a call of an assertion.
fn is_assertion(call: Node<'_>, src: &[u8]) -> bool {
    callee(call, src)
        .is_some_and(|name| ASSERTIONS.contains(&name) || name.starts_with("expectAsync"))
}

/// The outermost chain that starts with `call`
/// (`verify(m.f())` → `verify(m.f()).called(1)`).
fn chain(call: Node<'_>) -> Node<'_> {
    let mut top = call;
    for _ in 0..MAX_CHAIN {
        let Some(parent) = top.parent() else {
            break;
        };
        let continues = match parent.kind() {
            "member_expression" => parent.child_by_field_name("object") == Some(top),
            "call_expression" => parent.child_by_field_name("function") == Some(top),
            _ => false,
        };
        if !continues {
            break;
        }
        top = parent;
    }
    top
}

/// The source text of a node without white space.
fn squeezed(node: Node<'_>, src: &[u8]) -> String {
    text(node, src).split_whitespace().collect()
}

/// True for an assertion that cannot fail: `expect` of a literal and the
/// same literal (`expect(1, 1)`, `expect('a', equals('a'))`), of `true`
/// and `isTrue`, `false` and `isFalse`, `null` and `isNull`. One that
/// certainly fails (`fail(…)`, `expect(true, isFalse)`) counts.
fn cannot_fail(call: Node<'_>, src: &[u8]) -> bool {
    if callee(call, src) != Some("expect") {
        return false;
    }
    let (positional, _) = arguments(call, src);
    let [actual, matcher] = positional.as_slice() else {
        return false;
    };
    let constant = |node: &Node<'_>| reach::constant(*node, &LITERALS);
    if !constant(actual) {
        return false;
    }
    let same = |expected: &Node<'_>| {
        constant(expected) && squeezed(*actual, src) == squeezed(*expected, src)
    };
    if same(matcher) {
        return true;
    }
    match matcher.kind() {
        "identifier" => matches!(
            (actual.kind(), text(*matcher, src)),
            ("true", "isTrue") | ("false", "isFalse") | ("null_literal", "isNull")
        ),
        "call_expression" => {
            callee(*matcher, src) == Some("equals")
                && matches!(arguments(*matcher, src).0.as_slice(), [expected] if same(expected))
        }
        _ => false,
    }
}

/// The function a test or group call runs: its last positional argument
/// when written inline.
fn body_function<'t>(positional: &[Node<'t>]) -> Option<Node<'t>> {
    positional
        .iter()
        .rev()
        .find(|argument| argument.kind() == "function_expression")
        .copied()
}

/// The block of a function expression, `None` for an `=>` body.
fn body_block(function: Node<'_>) -> Option<Node<'_>> {
    let body = function.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let block = body
        .named_children(&mut cursor)
        .find(|child| child.kind() == "block");
    block
}

/// True when a block's first statement is `markTestSkipped(…)`.
fn marked_skipped(block: Node<'_>, src: &[u8]) -> bool {
    let mut cursor = block.walk();
    let first = block
        .named_children(&mut cursor)
        .find(|child| !child.is_extra() && !child.kind().ends_with("comment"));
    first
        .filter(|statement| statement.kind() == "expression_statement")
        .and_then(|statement| statement.named_child(0))
        .filter(|call| call.kind() == "call_expression")
        .is_some_and(|call| callee(call, src) == Some("markTestSkipped"))
}

/// True for an `@Skip` annotation among the children of `node`.
fn has_skip_annotation(node: Node<'_>, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).any(|child| {
        child.kind() == "annotation"
            && child
                .child_by_field_name("name")
                .is_some_and(|name| text(name, src).rsplit('.').next() == Some("Skip"))
    });
    found
}

/// True when the whole file is skipped: `@Skip(…)` on its `library`
/// directive or on `main`.
fn file_skipped(root: Node<'_>, src: &[u8]) -> bool {
    let mut cursor = root.walk();
    let skipped = root
        .named_children(&mut cursor)
        .any(|child| match child.kind() {
            "library_name" => has_skip_annotation(child, src),
            "function_declaration" => {
                let is_main = child
                    .child_by_field_name("signature")
                    .and_then(|signature| signature.child_by_field_name("name"))
                    .is_some_and(|name| text(name, src) == "main");
                is_main && has_skip_annotation(child, src)
            }
            _ => false,
        });
    skipped
}

/// The identifiers under `node` that name something: not a member after a
/// `.`, not an argument's label.
fn names_used(node: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    walk(node, |inner| {
        match inner.kind() {
            "label" => return false,
            "identifier" => {
                let member = inner
                    .parent()
                    .is_some_and(|parent| parent.child_by_field_name("property") == Some(inner));
                if !member {
                    found.insert(text(inner, src).to_owned());
                }
            }
            _ => {}
        }
        true
    });
    found
}

/// The names a file declares: variables, parameters, functions, classes.
fn declared(root: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    walk(root, |node| {
        if node.kind() == "identifier" {
            let declares = node.parent().is_some_and(|parent| {
                parent.kind() == "formal_parameter"
                    || parent.child_by_field_name("name") == Some(node)
                        && parent.kind() != "annotation"
            });
            if declares {
                found.insert(text(node, src).to_owned());
            }
        }
        true
    });
    found
}

/// One import of a file.
struct Import {
    uri: String,
    /// `show a, b`.
    shown: Vec<String>,
    /// `as prefix`.
    alias: Option<String>,
}

/// The imports of a file.
fn imports_of(root: Node<'_>, src: &[u8]) -> Vec<Import> {
    let mut found = Vec::new();
    walk(root, |node| {
        if node.kind() != "import_specification" {
            return matches!(
                node.kind(),
                "source_file" | "import_or_export" | "library_import"
            );
        }
        let uri = node
            .child_by_field_name("uri")
            .and_then(|uri| {
                let mut string = None;
                walk(uri, |inner| {
                    if string.is_none() && inner.kind() == "string_literal" {
                        string = string_value(inner, src);
                    }
                    string.is_none()
                });
                string
            })
            .unwrap_or_default();
        let alias = node
            .child_by_field_name("alias")
            .map(|alias| text(alias, src).to_owned());
        let mut shown = Vec::new();
        let mut cursor = node.walk();
        for combinator in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "combinator")
        {
            if text(combinator, src).trim_start().starts_with("show") {
                let mut inner = combinator.walk();
                shown.extend(
                    combinator
                        .named_children(&mut inner)
                        .filter(|name| name.kind() == "identifier")
                        .map(|name| text(name, src).to_owned()),
                );
            }
        }
        found.push(Import { uri, shown, alias });
        false
    });
    found
}

/// True for an import that may hold a test's subject: a relative path, or
/// a `package:` URI of a package other than Dart's, Flutter's and the test
/// and mocking libraries.
fn may_be_subject(uri: &str) -> bool {
    if uri.starts_with("dart:") || uri.is_empty() {
        return false;
    }
    match uri.strip_prefix("package:") {
        Some(rest) => {
            let package = rest.split('/').next().unwrap_or_default();
            !EXTERNAL_PACKAGES.contains(&package)
        }
        None => !uri.contains(':'),
    }
}

/// The first line of a syntax error under `root`.
fn first_error(root: Node<'_>) -> Option<usize> {
    let mut line = None;
    walk(root, |node| {
        if line.is_none() && (node.is_error() || node.is_missing()) {
            line = Some(node.start_position().row + 1);
        }
        line.is_none() && node.has_error()
    });
    line
}

/// Reads the tests, skips and assertions of a Dart test file.
pub(super) fn read(source: &str) -> TestFile {
    let language: Language = tree_sitter_dart::LANGUAGE.into();
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
        file.error_line = first_error(root);
    }
    let whole_file_skipped = file_skipped(root, src);

    // Each group and test call, with the byte range it covers.
    struct Block {
        end: usize,
        title: String,
        skipped: bool,
        focused: bool,
        test: Option<usize>,
    }
    let mut open: Vec<Block> = Vec::new();
    let mut all_uses: BTreeSet<String> = BTreeSet::new();
    walk(root, |node| {
        while open
            .last()
            .is_some_and(|block| node.start_byte() >= block.end)
        {
            open.pop();
        }
        match node.kind() {
            "comment" | "documentation_comment" | "string_literal" => return false,
            "call_expression" => {}
            _ => return true,
        }
        if is_assertion(node, src) {
            if dead.why(node).is_some() || cannot_fail(node, src) {
                return true;
            }
            let whole = chain(node);
            let inside = open.iter().rev().find(|block| block.test.is_some());
            let test = inside.and_then(|block| block.test);
            file.assertions.push(Assertion {
                line: node.start_position().row + 1,
                shape: squeezed(whole, src),
                text: text(whole, src)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                weight: 1,
                test,
            });
            return true;
        }
        let Some(name) = callee(node, src) else {
            return true;
        };
        let is_test = TEST_NAMES.contains(&name);
        let is_suite = SUITE_NAMES.contains(&name);
        if !(is_test || is_suite) {
            return true;
        }
        let (positional, named) = arguments(node, src);
        if positional.is_empty() {
            return true;
        }
        let mut own_skip = false;
        let mut own_focus = false;
        for (label, value) in &named {
            match label.as_str() {
                // `skip: false` and `skip: null` run it; a reason or any
                // other value skips it, at least sometimes.
                "skip" => own_skip |= !matches!(value.kind(), "false" | "null_literal"),
                "solo" => own_focus |= value.kind() == "true",
                _ => {}
            }
        }
        let function = body_function(&positional);
        let block = function.and_then(body_block);
        if is_test {
            own_skip |= block.is_some_and(|block| marked_skipped(block, src));
        }
        let why = dead.why(node).or_else(|| {
            (is_test && !own_skip)
                .then(|| {
                    block
                        .filter(|block| reach::does_nothing(*block, src, &GRAMMAR))
                        .map(|_| "its body does nothing")
                })
                .flatten()
        });
        let skipped = whole_file_skipped
            || own_skip
            || why.is_some()
            || open.iter().any(|block| block.skipped);
        let focused = own_focus || open.iter().any(|block| block.focused);
        let own_title = title(positional.first().copied(), src);
        let mut test = None;
        if is_test {
            let id = open
                .iter()
                .map(|block| block.title.as_str())
                .chain(std::iter::once(own_title.as_str()))
                .collect::<Vec<_>>()
                .join(" > ");
            let uses = node
                .child_by_field_name("arguments")
                .map(|list| names_used(list, src))
                .unwrap_or_default();
            all_uses.extend(uses.iter().cloned());
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
                weight: 1,
            });
        }
        open.push(Block {
            end: node.end_byte(),
            title: own_title,
            skipped,
            focused,
            test,
        });
        true
    });
    file.imports = subjects(root, src, &all_uses);
    file
}

/// The names the tests use, with the import each comes from: those an
/// import `show`s, an import's `as` prefix, and, when the file has exactly
/// one other import that may hold its subject, every other name its tests
/// use that the file does not declare and that is not the test framework's.
fn subjects(root: Node<'_>, src: &[u8], uses: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut open: Vec<String> = Vec::new();
    for import in imports_of(root, src) {
        if !may_be_subject(&import.uri) {
            continue;
        }
        for name in &import.shown {
            found.insert(name.clone(), import.uri.clone());
        }
        if let Some(alias) = &import.alias {
            found.insert(alias.clone(), import.uri.clone());
        }
        if import.shown.is_empty() && import.alias.is_none() {
            open.push(import.uri);
        }
    }
    if let [uri] = open.as_slice() {
        let own = declared(root, src);
        for name in uses {
            if !own.contains(name)
                && !found.contains_key(name)
                && !FRAMEWORK_NAMES.contains(&name.as_str())
                && !name.starts_with("expectAsync")
            {
                found.insert(name.clone(), uri.clone());
            }
        }
    }
    found
}

/// The repo path an import of `spec` from `from` names, among `exists`:
/// `package:<name>/x.dart` is `lib/x.dart` in the folder of `from` or one
/// above it (the package's root); a relative path is joined to the folder
/// of `from`. `None` for `dart:` and anything not found.
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let mut folders: Vec<&str> = from.split('/').collect();
    folders.pop();
    if let Some(rest) = spec.strip_prefix("package:") {
        let (_, inside) = rest.split_once('/')?;
        if inside
            .split('/')
            .any(|part| part == ".." || part.is_empty())
        {
            return None;
        }
        loop {
            let base = folders.join("/");
            let candidate = if base.is_empty() {
                format!("lib/{inside}")
            } else {
                format!("{base}/lib/{inside}")
            };
            if exists(&candidate) {
                return Some(candidate);
            }
            folders.pop()?;
        }
    }
    if spec.contains(':') {
        return None;
    }
    for part in spec.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                folders.pop()?;
            }
            other => folders.push(other),
        }
    }
    let joined = folders.join("/");
    exists(&joined).then_some(joined)
}

/// True for a path that holds Dart tests by its name or folder alone: a
/// `.dart` file named `*_test.dart`, or in a folder named `test` or
/// `integration_test`.
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || !extension.eq_ignore_ascii_case("dart") {
        return false;
    }
    stem.ends_with("_test")
        || folders
            .iter()
            .any(|folder| matches!(*folder, "test" | "integration_test"))
}

/// True for Dart source that holds tests although its path does not say
/// so: it imports `package:test` or `package:flutter_test` and calls
/// `test`, `testWidgets` or `group`.
pub(super) fn holds_tests(source: &str) -> bool {
    (source.contains("package:test/") || source.contains("package:flutter_test/"))
        && ["test(", "testWidgets(", "group("]
            .iter()
            .any(|call| source.contains(call))
}

/// The suppressions a Dart comment holds: `// ignore: a, b` and
/// `// ignore_for_file: a`. A documentation comment (`///`) is not one.
fn comment_keys(raw: &str) -> Vec<String> {
    let Some(body) = raw.strip_prefix("//") else {
        return Vec::new();
    };
    if body.starts_with('/') {
        return Vec::new();
    }
    let body = body.trim_start();
    let (tool, list) = if let Some(list) = body.strip_prefix("ignore_for_file:") {
        ("ignore-file", list)
    } else if let Some(list) = body.strip_prefix("ignore:") {
        ("ignore", list)
    } else {
        return Vec::new();
    };
    let rules: Vec<String> = list
        .split(',')
        .filter_map(|item| item.split_whitespace().next())
        .map(|rule| if rule == "type=lint" { "*" } else { rule }.to_owned())
        .collect();
    if rules.is_empty() {
        // `// ignore:` with no rule ignores nothing.
        return Vec::new();
    }
    keyed(tool, rules)
}

/// The lint suppressions in a Dart source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let language: Language = tree_sitter_dart::LANGUAGE.into();
    let Some(tree) = parse(source, &language) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| {
        if node.kind() != "comment" {
            return !node.kind().contains("string_literal");
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

    fn ids(file: &TestFile) -> Vec<(&str, bool, bool)> {
        file.tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.focused))
            .collect()
    }

    fn shapes(file: &TestFile) -> Vec<&str> {
        file.assertions.iter().map(|a| a.shape.as_str()).collect()
    }

    fn keys(source: &str) -> Vec<String> {
        suppressions(source).into_iter().map(|s| s.key).collect()
    }

    #[test]
    fn tests_groups_skips_focus_and_assertions_are_read() {
        let file = read(
            r#"import 'package:test/test.dart';
import 'package:shop/src/cart.dart';

void main() {
  group('Cart', () {
    test('adds', () {
      final cart = Cart();
      expect(cart.total, equals(0));
      expect(total([1, 2]), 3, reason: 'sum');
    });
    test('skipped', () {
      expect(a, b);
    }, skip: true);
    test('skipped with a reason', () {
      expect(a, b);
    }, skip: 'flaky on CI');
    test('skipped sometimes', () {
      expect(a, b);
    }, skip: Platform.isWindows);
    test('runs', () {
      expect(a, b);
    }, skip: false);
    test('focused', () async {
      await expectLater(stream, emits(1));
    }, solo: true);
    group('off', () {
      test('inside', () => expect(a, b));
    }, skip: 'later');
    group('solo group', () {
      test('also focused', () => expect(a, b));
    }, solo: true);
  });
  testWidgets('renders', (WidgetTester tester) async {
    await tester.pumpWidget(App());
    expect(find.text('Cart'), findsOneWidget);
  });
  testGoldens('golden', (tester) async {
    expect(1 + 1, 2);
  });
}
"#,
        );
        assert!(!file.has_errors);
        assert_eq!(
            ids(&file),
            [
                ("Cart > adds", false, false),
                ("Cart > skipped", true, false),
                ("Cart > skipped with a reason", true, false),
                ("Cart > skipped sometimes", true, false),
                ("Cart > runs", false, false),
                ("Cart > focused", false, true),
                ("Cart > off > inside", true, false),
                ("Cart > solo group > also focused", false, true),
                ("renders", false, false),
                ("golden", false, false),
            ]
        );
        assert_eq!(file.tests[0].line, 6);
        assert!(file.tests[0].uses.contains("Cart"));
        assert_eq!(
            shapes(&file),
            [
                "expect(cart.total,equals(0))",
                "expect(total([1,2]),3,reason:'sum')",
                "expect(a,b)",
                "expect(a,b)",
                "expect(a,b)",
                "expect(a,b)",
                "expectLater(stream,emits(1))",
                "expect(a,b)",
                "expect(a,b)",
                "expect(find.text('Cart'),findsOneWidget)",
                "expect(1+1,2)",
            ]
        );
        assert_eq!(file.assertions[0].test, Some(0));
        assert_eq!(file.assertions[0].line, 8);
        assert_eq!(
            file.imports.get("Cart").map(String::as_str),
            Some("package:shop/src/cart.dart")
        );
        assert_eq!(
            file.imports.get("total").map(String::as_str),
            Some("package:shop/src/cart.dart")
        );
        // Framework names and the file's own variables are not subjects.
        for name in ["expect", "cart", "tester", "find", "equals"] {
            assert_ne!(
                file.imports.get(name).map(String::as_str),
                Some("package:shop/src/cart.dart"),
                "{name}"
            );
        }
    }

    #[test]
    fn interpolated_titles_are_their_source_text() {
        let file = read("void main() {\n  test('total of $n', () { expect(n, 1); });\n  test(\"double\", () { expect(n, 1); });\n}\n");
        let names: Vec<&str> = file.tests.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(names, ["'total of $n'", "double"]);
    }

    #[test]
    fn a_skip_annotation_skips_the_file() {
        let library = read("@Skip('not on this platform')\nlibrary;\n\nimport 'package:test/test.dart';\n\nvoid main() {\n  test('a', () { expect(a, 1); });\n}\n");
        assert_eq!(ids(&library), [("a", true, false)]);
        let on_main = read("import 'package:test/test.dart';\n\n@Skip()\nvoid main() {\n  test('a', () { expect(a, 1); });\n}\n");
        assert_eq!(ids(&on_main), [("a", true, false)]);
        let other = read("@TestOn('vm')\nlibrary;\n\nimport 'package:test/test.dart';\n\nvoid main() {\n  test('a', () { expect(a, 1); });\n}\n");
        assert_eq!(ids(&other), [("a", false, false)]);
    }

    #[test]
    fn mark_test_skipped_and_empty_bodies_skip_a_test() {
        let file = read(
            r#"void main() {
  test('marked', () {
    markTestSkipped('needs a device');
    expect(a, b);
  });
  test('marked later', () {
    expect(a, b);
    markTestSkipped('too late');
  });
  test('empty', () {});
  test('a comment', () {
    // TODO
  });
  test('returns', () {
    return;
  });
  test('arrow', () => run());
  test('a named function', runCase);
}
"#,
        );
        let tests: Vec<(&str, bool, Option<&str>)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.why))
            .collect();
        let why = Some("its body does nothing");
        assert_eq!(
            tests,
            [
                ("marked", true, None),
                ("marked later", false, None),
                ("empty", true, why),
                ("a comment", true, why),
                ("returns", true, why),
                ("arrow", false, None),
                ("a named function", false, None),
            ]
        );
    }

    #[test]
    fn every_assertion_form_is_read() {
        let file = read(
            r#"void main() {
  test('forms', () async {
    expect(a, 1);
    await expectLater(f(), completes);
    verify(mock.save(any)).called(1);
    verifyNever(mock.delete());
    verifyInOrder([mock.a(), mock.b()]);
    verifyNoMoreInteractions(mock);
    stream.listen(expectAsync1((value) => expect(value, 2)));
    fail('unreachable');
    expectSomething(a);
    cart..add(1)..add(2);
  });
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "expect(a,1)",
                "expectLater(f(),completes)",
                "verify(mock.save(any)).called(1)",
                "verifyNever(mock.delete())",
                "verifyInOrder([mock.a(),mock.b()])",
                "verifyNoMoreInteractions(mock)",
                "expectAsync1((value)=>expect(value,2))",
                "expect(value,2)",
                "fail('unreachable')",
            ]
        );
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = read(
            r#"void main() {
  test('placeholders', () {
    expect(true, isTrue);
    expect(false, isFalse);
    expect(null, isNull);
    expect(1, 1);
    expect('a', 'a');
    expect(1, equals(1));
    expect(1.5, equals(1.5));
    expect(total, 3);
    expect(1 + 1, 2);
    expect(true, isFalse);
    expect(1, 2);
    expect(1, equals(2));
    expect(1, isNot(1));
    expect('a', "a");
  });
}
"#,
        );
        assert_eq!(
            shapes(&file),
            [
                "expect(total,3)",
                "expect(1+1,2)",
                "expect(true,isFalse)",
                "expect(1,2)",
                "expect(1,equals(2))",
                "expect(1,isNot(1))",
                "expect('a',\"a\")",
            ]
        );
    }

    #[test]
    fn code_that_never_runs_is_not_counted() {
        let file = read(
            r#"void main() {
  test('returns early', () {
    final total = 3;
    return;
    expect(total, 3);
  });
  test('if false', () {
    if (false) {
      expect(total(), 3);
    }
    while (false) { expect(a, 1); }
    expect(other(), 1);
  });
  if (false) {
    test('hidden', () { expect(total(), 3); });
  }
  group('after return', () {
    return;
    test('never registered', () { expect(total(), 3); });
  });
  test('conditional return', () {
    if (ready()) { return; }
    expect(total(), 3);
  });
}
"#,
        );
        let tests: Vec<(&str, bool, Option<&str>)> = file
            .tests
            .iter()
            .map(|t| (t.id.as_str(), t.skipped, t.why))
            .collect();
        assert_eq!(
            tests,
            [
                ("returns early", false, None),
                ("if false", false, None),
                ("hidden", true, Some("inside if (false)")),
                (
                    "after return > never registered",
                    true,
                    Some("after a return")
                ),
                ("conditional return", false, None),
            ]
        );
        assert_eq!(shapes(&file), ["expect(other(),1)", "expect(total(),3)"]);
        assert_eq!(file.assertions[1].line, 23);
    }

    #[test]
    fn look_alikes_are_not_tests_or_assertions() {
        let file = read(
            r#"// test('commented', () { expect(a, 1); });
/// expect(doc, 1);
const s = "test('in a string', () { expect(a, 1); })";
const r = r'expect(raw, 1)';
void main() {
  regex.test('x');
  helper.expect(a, 1);
}
"#,
        );
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        assert!(file.assertions.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_says_so() {
        let file = read("void main() {\n  test('a', () {\n    expect(a, 1)\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        assert!(!read("").has_errors);
    }

    #[test]
    fn imports_name_the_subject() {
        let file = read(
            r#"import 'package:test/test.dart';
import 'package:mockito/mockito.dart';
import 'package:shop/models.dart' show Item;
import '../lib/pricing.dart' as pricing;
import 'dart:async';

void main() {
  test('a', () { expect(pricing.total([Item()]), 1); });
}
"#,
        );
        assert_eq!(
            file.imports.get("Item").map(String::as_str),
            Some("package:shop/models.dart")
        );
        assert_eq!(
            file.imports.get("pricing").map(String::as_str),
            Some("../lib/pricing.dart")
        );
        assert_eq!(file.imports.len(), 2);
        // Two imports that may hold the subject: which one a name comes
        // from is not known, so neither is named.
        let file = read("import 'package:shop/a.dart';\nimport 'package:shop/b.dart';\nvoid main() {\n  test('a', () { expect(A().b, B()); });\n}\n");
        assert!(file.imports.is_empty());
    }

    #[test]
    fn imports_resolve_to_repo_files() {
        let exists = |path: &str| {
            matches!(
                path,
                "lib/src/cart.dart" | "packages/shop/lib/models.dart" | "lib/pricing.dart"
            )
        };
        assert_eq!(
            resolve("test/cart_test.dart", "package:app/src/cart.dart", &exists).as_deref(),
            Some("lib/src/cart.dart")
        );
        assert_eq!(
            resolve(
                "packages/shop/test/unit/models_test.dart",
                "package:shop/models.dart",
                &exists
            )
            .as_deref(),
            Some("packages/shop/lib/models.dart")
        );
        assert_eq!(
            resolve("test/pricing_test.dart", "../lib/pricing.dart", &exists).as_deref(),
            Some("lib/pricing.dart")
        );
        assert_eq!(resolve("test/a_test.dart", "dart:async", &exists), None);
        assert_eq!(
            resolve("test/a_test.dart", "package:app/missing.dart", &exists),
            None
        );
        assert_eq!(resolve("a_test.dart", "../../x.dart", &exists), None);
    }

    #[test]
    fn test_paths() {
        for path in [
            "test/cart_test.dart",
            "test/helpers.dart",
            "integration_test/app_test.dart",
            "packages/shop/test/unit/a.dart",
            "lib/src/cart_test.dart",
            "test/A.DART",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "lib/src/cart.dart",
            "testing/a.dart",
            "test/a.py",
            "test/.dart",
            "lib/contest.dart",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_looks_for_a_framework_and_a_call() {
        assert!(holds_tests(
            "import 'package:test/test.dart';\nvoid main() { test('a', () {}); }"
        ));
        assert!(holds_tests(
            "import 'package:flutter_test/flutter_test.dart';\nvoid main() { testWidgets('a', (t) async {}); }"
        ));
        assert!(!holds_tests("void main() { test('a', () {}); }"));
        assert!(!holds_tests("import 'package:test/test.dart';\n"));
    }

    #[test]
    fn ignore_comments_turn_rules_off() {
        let source = r#"// ignore_for_file: unused_import, avoid_print
// ignore_for_file: type=lint
void main() {
  // ignore: avoid_print
  print('x'); // ignore: avoid_print, unnecessary_string_interpolations
  //ignore:dead_code
  // ignore: type=lint
}
"#;
        assert_eq!(
            keys(source),
            [
                "ignore-file:unused_import",
                "ignore-file:avoid_print",
                "ignore-file:*",
                "ignore:avoid_print",
                "ignore:avoid_print",
                "ignore:unnecessary_string_interpolations",
                "ignore:dead_code",
                "ignore:*",
            ]
        );
        let found = suppressions(source);
        assert_eq!(found[3].line, 4);
        assert_eq!(found[3].text, "// ignore: avoid_print");
    }

    #[test]
    fn dart_look_alikes_are_not_suppressions() {
        let source = r#"/// ignore: avoid_print
// Do not add ignore: comments here.
// ignored: avoid_print
// ignore:
/* ignore: avoid_print */
const s = '// ignore: avoid_print';
const t = """
// ignore_for_file: type=lint
""";
"#;
        assert!(keys(source).is_empty(), "{:?}", keys(source));
    }
}
