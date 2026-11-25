//! An assertion made conditional or a tautology (`assertion-weakened`, ADR
//! 0088; verification idea 6, items ii and iii).
//!
//! The readers of Python, JavaScript/TypeScript and Rust ask [`why`] of each
//! assertion they count. It names how the assertion can pass without
//! checking anything:
//!
//! - **Conditional or swallowed**: inside its test it runs under an `if`
//!   (`if let`, `elif`, `else`, a ternary, `&&`), inside the body of a `try`
//!   whose handler catches the assertion's error (Python: `AssertionError`,
//!   `Exception`, `BaseException` or a bare `except`; JavaScript: any
//!   `catch`), under `with suppress(AssertionError)`, or in a closure given
//!   to `catch_unwind`. An assertion in a `catch` or `except` handler is not:
//!   that is how a test checks an error.
//! - **A tautology**: what it checks always holds: a value compared with
//!   itself, a length that is `>= 0`, or a condition that ends in
//!   `or True` / `|| true`.
//!
//! Only the shape is read: whether a condition can be false is not.

use tree_sitter::Node;

/// Nodes that end the walk up from an assertion: a function, a closure or
/// anything wider. What lies beyond is not "inside its test".
const BOUNDARIES: [&str; 18] = [
    "function_definition",
    "function_declaration",
    "function_item",
    "function_expression",
    "function",
    "arrow_function",
    "generator_function",
    "generator_function_declaration",
    "method_definition",
    "closure_expression",
    "lambda",
    "class_definition",
    "class_declaration",
    "decorated_definition",
    "mod_item",
    "module",
    "program",
    "source_file",
];

/// At most this many parents are looked at.
const MAX_DEPTH: usize = 40;

/// How the assertion at `node` can pass without checking anything, or
/// `None`. `text` is the assertion as the reader shows it, with white
/// space squeezed to single spaces.
#[must_use]
pub(super) fn why(node: Node<'_>, src: &[u8], text: &str) -> Option<&'static str> {
    guarded(node, src).or_else(|| tautology(text))
}

fn source(node: Node<'_>, src: &[u8]) -> String {
    String::from_utf8_lossy(&src[node.start_byte()..node.end_byte()]).into_owned()
}

fn inside(outer: Node<'_>, node: Node<'_>) -> bool {
    outer.start_byte() <= node.start_byte() && node.end_byte() <= outer.end_byte()
}

/// The assertion runs only sometimes, or its failure is caught.
fn guarded(node: Node<'_>, src: &[u8]) -> Option<&'static str> {
    let mut child = node;
    for _ in 0..MAX_DEPTH {
        let parent = child.parent()?;
        let kind = parent.kind();
        if BOUNDARIES.contains(&kind) {
            // A closure handed to `catch_unwind` swallows its panics.
            let caught = parent.kind() == "closure_expression"
                && parent
                    .parent()
                    .filter(|arguments| arguments.kind() == "arguments")
                    .and_then(|arguments| arguments.parent())
                    .and_then(|call| call.child_by_field_name("function"))
                    .is_some_and(|function| source(function, src).ends_with("catch_unwind"));
            return caught.then_some("inside catch_unwind, which swallows its panic");
        }
        match kind {
            "if_statement" | "if_expression" => {
                let in_condition = parent
                    .child_by_field_name("condition")
                    .is_some_and(|condition| inside(condition, child));
                if !in_condition {
                    let is_let = parent
                        .child_by_field_name("condition")
                        .is_some_and(|c| matches!(c.kind(), "let_condition" | "let_chain"));
                    return Some(if is_let {
                        "now under an `if let`: it runs only when the pattern matches"
                    } else {
                        "now under an `if`: it runs only when the condition holds"
                    });
                }
            }
            "ternary_expression" | "conditional_expression" => {
                let in_condition = parent
                    .child_by_field_name("condition")
                    .or_else(|| parent.named_child(1))
                    .is_some_and(|condition| inside(condition, child));
                if !in_condition {
                    return Some("now in a conditional expression: it runs only on one branch");
                }
            }
            "binary_expression" | "boolean_operator" => {
                let operator = parent
                    .child_by_field_name("operator")
                    .map(|operator| source(operator, src));
                let right = parent
                    .child_by_field_name("right")
                    .is_some_and(|right| inside(right, child));
                if right && matches!(operator.as_deref(), Some("&&" | "||" | "and" | "or")) {
                    return Some("now behind `&&` or `||`: it runs only sometimes");
                }
            }
            "try_statement" => {
                let in_body = parent
                    .child_by_field_name("body")
                    .is_some_and(|body| inside(body, child));
                if in_body && swallows(parent, src) {
                    return Some("now inside a `try` whose handler catches its failure");
                }
            }
            "with_statement" => {
                let mut cursor = parent.walk();
                let header: String = parent
                    .named_children(&mut cursor)
                    .filter(|part| part.kind() == "with_clause")
                    .map(|part| source(part, src))
                    .collect();
                if header.contains("suppress(") && catches_assertion(&header) {
                    return Some(
                        "now under `suppress(AssertionError)`, which swallows its failure",
                    );
                }
            }
            _ => {}
        }
        child = parent;
    }
    None
}

/// A Python `except` header that catches an assertion's failure, or a
/// JavaScript `try` with a `catch`.
fn swallows(statement: Node<'_>, src: &[u8]) -> bool {
    if statement.child_by_field_name("handler").is_some() {
        return true;
    }
    let mut cursor = statement.walk();
    let found = statement.named_children(&mut cursor).any(|part| {
        if part.kind() != "except_clause" {
            return false;
        }
        let whole = source(part, src);
        let header = whole.split(':').next().unwrap_or("").trim();
        header == "except" || catches_assertion(header)
    });
    found
}

fn catches_assertion(header: &str) -> bool {
    header
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| matches!(word, "AssertionError" | "Exception" | "BaseException"))
}

// ------------------------------------------------------------ tautologies

/// One call of an assertion's text: its dotted name and its arguments.
#[derive(Debug, PartialEq, Eq)]
struct Call {
    name: String,
    args: Vec<String>,
}

/// The calls of a chain (`expect(a).not.toBe(b)`, `self.assertEqual(a, b)`,
/// `assert_eq!(a, b)`), from the left. `None` when the text is not a chain
/// of calls.
fn calls(text: &str) -> Option<Vec<Call>> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let mut out = Vec::new();
    while at < chars.len() {
        let start = at;
        while at < chars.len() && chars[at] != '(' {
            at += 1;
        }
        if at >= chars.len() {
            // A trailing name with no call (`.resolves`): nothing to check.
            break;
        }
        let name: String = chars[start..at].iter().collect::<String>();
        let name = name.trim().trim_start_matches('.').trim().to_owned();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '!' | '$' | ':'))
        {
            return None;
        }
        let (args, end) = arguments(&chars, at)?;
        out.push(Call { name, args });
        at = end;
    }
    (!out.is_empty()).then_some(out)
}

/// The arguments of the parenthesised list that opens at `open`, split at
/// its top-level commas, and the index after its `)`.
fn arguments(chars: &[char], open: usize) -> Option<(Vec<String>, usize)> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut current = String::new();
    let mut args = Vec::new();
    let mut at = open;
    while at < chars.len() {
        let c = chars[at];
        if let Some(q) = quote {
            current.push(c);
            if c == '\\' {
                if let Some(next) = chars.get(at + 1) {
                    current.push(*next);
                    at += 1;
                }
            } else if c == q {
                quote = None;
            }
            at += 1;
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                quote = Some(c);
                current.push(c);
            }
            '(' | '[' | '{' => {
                if depth > 0 {
                    current.push(c);
                }
                depth += 1;
            }
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let last = current.trim().to_owned();
                    if !last.is_empty() || !args.is_empty() {
                        args.push(last);
                    }
                    return Some((args, at + 1));
                }
                current.push(c);
            }
            ',' if depth == 1 => {
                args.push(current.trim().to_owned());
                current.clear();
            }
            _ => current.push(c),
        }
        at += 1;
    }
    None
}

/// Splits `text` at the first top-level occurrence of one of `operators`,
/// with spaces around word operators. Brackets and strings are skipped.
fn split_top<'a>(
    text: &'a str,
    operators: &[&'static str],
) -> Option<(&'a str, &'static str, &'a str)> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut at = 0;
    while at < bytes.len() {
        let b = bytes[at];
        if let Some(q) = quote {
            if b == b'\\' {
                at += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
            at += 1;
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => quote = Some(b),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ if depth == 0 => {
                for operator in operators {
                    if text.is_char_boundary(at) && text[at..].starts_with(operator) {
                        let left = &text[..at];
                        let right = &text[at + operator.len()..];
                        // `<=` is not `<` then `=`; `==` is not `===`.
                        let clean =
                            !right.starts_with('=') && !left.ends_with(['=', '!', '<', '>']);
                        if clean {
                            return Some((left.trim(), operator, right.trim()));
                        }
                    }
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// `text` without the parentheses around all of it.
fn unwrap(text: &str) -> &str {
    let mut at = text.trim();
    while at.starts_with('(') && at.ends_with(')') {
        let chars: Vec<char> = at.chars().collect();
        match arguments(&chars, 0) {
            Some((_, end)) if end == chars.len() => at = at[1..at.len() - 1].trim(),
            _ => break,
        }
    }
    at
}

/// A length, a count or a size, which is never below 0.
fn is_length(text: &str) -> bool {
    let text = unwrap(text);
    (text.starts_with("len(") && text.ends_with(')'))
        || [".len()", ".length", ".size", ".size()", ".count()"]
            .iter()
            .any(|end| text.ends_with(end))
}

const SELF: &str = "a tautology: it compares a value with itself";
const LENGTH: &str = "a tautology: a length is never below 0";
const OR_TRUE: &str = "a tautology: its condition ends in `or True`";

/// A condition that always holds.
fn condition_always(condition: &str) -> Option<&'static str> {
    let condition = unwrap(condition);
    if let Some((_, _, right)) = split_top(condition, &[" or ", "||"]) {
        let last = right.rsplit([' ', '|']).next().unwrap_or(right);
        if matches!(unwrap(last), "True" | "true") || matches!(unwrap(right), "True" | "true") {
            return Some(OR_TRUE);
        }
    }
    let (left, operator, right) = split_top(condition, &["===", "==", ">=", "<=", " is "])?;
    let (left, right) = (unwrap(left), unwrap(right));
    if left == right && !left.is_empty() {
        return Some(SELF);
    }
    match operator {
        ">=" if right == "0" && is_length(left) => Some(LENGTH),
        "<=" if left == "0" && is_length(right) => Some(LENGTH),
        _ => None,
    }
}

/// An equality whose two sides are the same.
fn pair_always(actual: &str, expected: &str) -> Option<&'static str> {
    let (actual, expected) = (unwrap(actual), unwrap(expected));
    (!actual.is_empty() && actual == expected).then_some(SELF)
}

/// What the assertion's text checks always holds.
fn tautology(text: &str) -> Option<&'static str> {
    let text = text.trim().trim_end_matches(';');
    // Python's `assert` statement: its condition, without the message.
    if let Some(rest) = text.strip_prefix("assert ") {
        let condition = split_top(rest, &[","]).map_or(rest, |(condition, _, _)| condition);
        return condition_always(condition);
    }
    let chain = calls(text)?;
    let first = chain.first()?;
    let base = first.name.rsplit('.').next().unwrap_or(&first.name);
    // `expect(actual)…matcher(expected)`.
    if first.name == "expect" || first.name == "expect.soft" {
        if chain
            .iter()
            .any(|call| call.name.split('.').any(|part| part == "not"))
        {
            return None;
        }
        let actual = first.args.first()?;
        let matcher = chain.last()?;
        if chain.len() < 2 {
            return None;
        }
        let name = matcher.name.rsplit('.').next().unwrap_or(&matcher.name);
        let expected = matcher.args.first().map(String::as_str);
        return match (name, expected) {
            ("toBe", Some("true")) | ("toBeTruthy", None) => condition_always(actual),
            ("toBe" | "toEqual" | "toStrictEqual", Some(expected)) => pair_always(actual, expected),
            ("toBeGreaterThanOrEqual", Some("0")) if is_length(actual) => Some(LENGTH),
            _ => None,
        };
    }
    let args = &first.args;
    match base {
        "assert!" | "debug_assert!" | "assertTrue" | "assert" | "ok" | "isTrue" => {
            condition_always(args.first()?)
        }
        "assert_eq!" | "debug_assert_eq!" | "assertEqual" | "assertEquals" | "assertIs"
        | "equal" | "strictEqual" | "deepEqual" | "deepStrictEqual" => {
            pair_always(args.first()?, args.get(1)?)
        }
        "assertGreaterEqual" if args.get(1).map(String::as_str) == Some("0") => {
            is_length(args.first()?).then_some(LENGTH)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tautologies_by_their_text() {
        let cases = [
            ("assert len(items) >= 0", Some(LENGTH)),
            ("assert 0 <= len(items), \"why\"", Some(LENGTH)),
            ("assert total([4, 6]) == 10 or True", Some(OR_TRUE)),
            ("assert x == x", Some(SELF)),
            ("assert x == y", None),
            ("assert len(items) >= 1", None),
            ("assert len(items) > 0", None),
            (
                "expect(items(3).length).toBeGreaterThanOrEqual(0);",
                Some(LENGTH),
            ),
            ("expect(a).toEqual(a)", Some(SELF)),
            ("expect(a).not.toEqual(a)", None),
            ("expect(a).toEqual(b)", None),
            ("expect(a.length >= 0).toBe(true)", Some(LENGTH)),
            ("expect(ok || true).toBeTruthy()", Some(OR_TRUE)),
            ("assert!(got.len() >= 0)", Some(LENGTH)),
            ("assert!(got.len() >= 0, \"a, b\")", Some(LENGTH)),
            ("assert_eq!(got, got)", Some(SELF)),
            ("assert_eq!(got, vec![\"a\", \"b\"])", None),
            ("assert!(a <= b)", None),
            ("self.assertEqual(x, x)", Some(SELF)),
            ("self.assertGreaterEqual(len(x), 0)", Some(LENGTH)),
            ("self.assertTrue(x == y or True)", Some(OR_TRUE)),
            ("assert.strictEqual(a, a)", Some(SELF)),
            ("assert.equal(a, b)", None),
            ("assert x >= y", None),
            ("assert x is not None", None),
        ];
        for (text, want) in cases {
            assert_eq!(tautology(text), want, "{text}");
        }
    }

    fn first_assertion_why(
        source: &str,
        language: tree_sitter::Language,
        kinds: &[&str],
    ) -> Option<&'static str> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut found = None;
        crate::imports::walk(tree.root_node(), |node| {
            if found.is_none() && kinds.contains(&node.kind()) {
                let text =
                    String::from_utf8_lossy(&source.as_bytes()[node.start_byte()..node.end_byte()])
                        .into_owned();
                let shown = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if shown.starts_with("assert") || shown.starts_with("expect") {
                    found = Some(why(node, source.as_bytes(), &shown));
                }
            }
            found.is_none()
        });
        found.flatten()
    }

    #[test]
    fn python_guards() {
        let py = tree_sitter_python::LANGUAGE.into();
        let kinds = ["assert_statement"];
        let under_if = "def test_a():\n    if x:\n        assert f() == 1\n";
        assert!(first_assertion_why(under_if, py, &kinds)
            .unwrap()
            .contains("`if`"));
        let in_else =
            "def test_a():\n    if x:\n        pass\n    else:\n        assert f() == 1\n";
        assert!(
            first_assertion_why(in_else, tree_sitter_python::LANGUAGE.into(), &kinds).is_some()
        );
        let swallowed = "def test_a():\n    try:\n        assert f() == 1\n    except AssertionError:\n        pass\n";
        assert!(
            first_assertion_why(swallowed, tree_sitter_python::LANGUAGE.into(), &kinds)
                .unwrap()
                .contains("try")
        );
        let other_error = "def test_a():\n    try:\n        assert f() == 1\n    except ValueError:\n        pass\n";
        assert_eq!(
            first_assertion_why(other_error, tree_sitter_python::LANGUAGE.into(), &kinds),
            None
        );
        let in_handler = "def test_a():\n    try:\n        f()\n    except Exception as e:\n        assert str(e) == 'x'\n";
        assert_eq!(
            first_assertion_why(in_handler, tree_sitter_python::LANGUAGE.into(), &kinds),
            None
        );
        let suppressed = "def test_a():\n    with contextlib.suppress(AssertionError):\n        assert f() == 1\n";
        assert!(
            first_assertion_why(suppressed, tree_sitter_python::LANGUAGE.into(), &kinds).is_some()
        );
        let plain = "def test_a():\n    for x in xs:\n        assert f(x) == 1\n";
        assert_eq!(
            first_assertion_why(plain, tree_sitter_python::LANGUAGE.into(), &kinds),
            None
        );
        // An `if` outside the test function is not inside the test.
        let outside = "if sys.platform == 'linux':\n    def test_a():\n        assert f() == 1\n";
        assert_eq!(
            first_assertion_why(outside, tree_sitter_python::LANGUAGE.into(), &kinds),
            None
        );
    }

    #[test]
    fn javascript_and_rust_guards() {
        let ts = || -> tree_sitter::Language { tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into() };
        let kinds = ["call_expression"];
        let try_catch = "it('a', () => {\n  try {\n    expect(f()).toBe(1);\n  } catch {}\n});\n";
        assert!(first_assertion_why(try_catch, ts(), &kinds)
            .unwrap()
            .contains("try"));
        let finally_only =
            "it('a', () => {\n  try {\n    expect(f()).toBe(1);\n  } finally { done(); }\n});\n";
        assert_eq!(first_assertion_why(finally_only, ts(), &kinds), None);
        let and = "it('a', () => {\n  ok && expect(f()).toBe(1);\n});\n";
        assert!(first_assertion_why(and, ts(), &kinds).is_some());
        let rs = || -> tree_sitter::Language { tree_sitter_rust::LANGUAGE.into() };
        let kinds = ["macro_invocation"];
        let if_let =
            "#[test]\nfn a() {\n    if let Some(v) = f() {\n        assert_eq!(v, 1);\n    }\n}\n";
        assert!(first_assertion_why(if_let, rs(), &kinds)
            .unwrap()
            .contains("if let"));
        let unwind = "#[test]\nfn a() {\n    let _ = std::panic::catch_unwind(|| {\n        assert_eq!(f(), 1);\n    });\n}\n";
        assert!(first_assertion_why(unwind, rs(), &kinds)
            .unwrap()
            .contains("catch_unwind"));
        let plain = "#[test]\nfn a() {\n    let v = f();\n    assert_eq!(v, 1);\n}\n";
        assert_eq!(first_assertion_why(plain, rs(), &kinds), None);
    }
}
