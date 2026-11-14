//! The C and C++ reader of the tamper guard (M32, ADR 0052).
//!
//! Test frameworks in C and C++ are macros, which tree-sitter reads as
//! whatever they look like: `TEST(Suite, Name) { … }` as a function
//! definition, `TEST_CASE("name") { … }` as a call missing its `;`
//! followed by a block. Both shapes are read, and a missing `;` after a
//! macro call (an all-caps name) is not a syntax error.
//!
//! - **GoogleTest**: `TEST`, `TEST_F`, `TEST_P`, `TYPED_TEST`,
//!   `TYPED_TEST_P` (and Unity Fixture's `TEST`, `IGNORE_TEST`); id
//!   `Suite::Name`. Skipped: a `DISABLED_` suite or name, `GTEST_SKIP()` as
//!   a statement of its body or of its fixture's `SetUp`. A `TEST_P` weighs
//!   the rows of its suite's `INSTANTIATE_TEST_SUITE_P(…, Values(a, b, c))`
//!   (or `Bool()`) when every instantiation in the file is that literal.
//! - **Catch2 and doctest**: `TEST_CASE`, `SCENARIO`, `TEST_CASE_METHOD`,
//!   `TEMPLATE_TEST_CASE` (weighed by its types) and their forms; id its
//!   name, inside doctest's `TEST_SUITE("s")` `s::name`. `SECTION`,
//!   `GIVEN`, `WHEN`, `THEN` are part of the test. Skipped: a hidden tag
//!   (`[.]`, `[.slow]`, `[!hide]`), `[!shouldfail]`, a name starting with
//!   `./`, `* doctest::skip()`, `SKIP()` as a statement of its body.
//! - **C**: Unity (`void test…(void)` functions in a file that includes
//!   `unity.h`, or a function `RUN_TEST` names; `TEST_IGNORE()` skips),
//!   CMocka (the functions `cmocka_unit_test(…)` lists, one test per entry;
//!   `skip()` skips), Check (`START_TEST(name) { … } END_TEST`).
//! - A test is also skipped when its body does nothing, inside `#if 0`
//!   (never compiled) or under any other `#if`/`#ifdef` than a header's
//!   include guard (compiled only sometimes, like Rust's `cfg`).
//! - **Assertions**: `EXPECT_*`, `ASSERT_*`, `FAIL()`, `ADD_FAILURE()`;
//!   `REQUIRE*`, `CHECK*` (not `CHECK_NOFAIL`, `CHECKED_IF`), `FAIL_CHECK`;
//!   `TEST_ASSERT*`, `TEST_FAIL*`; `assert_*`, `fail()`, `fail_msg()`;
//!   `ck_assert*`, `ck_abort*`; plain `assert(…)`. `WARN` and `SUCCEED`
//!   cannot fail and are not assertions. Not counted: after a `return` or a
//!   skip, inside `if (false)`, `if (0)` or `#if 0`, and those that cannot
//!   fail (`EXPECT_TRUE(true)`, `ASSERT_EQ(1, 1)`, `REQUIRE(1 == 1)`,
//!   `assert(1)`).
//! - **Subjects**: each quoted include (`#include "pricing.h"`) is a
//!   module every test of the file uses, named by its stem (`pricing`), so
//!   a test deleted with the header it includes is not a finding.
//!   `<…>` includes are not subjects.
//! - **Suppressions**: `// NOLINT(…)`, `// NOLINTNEXTLINE(…)`
//!   (`nolint:check`), `// NOLINTBEGIN(…)` (`nolint-range:check`);
//!   `#pragma GCC|clang diagnostic ignored "-Wfoo"`
//!   (`pragma-ignored:-Wfoo`); `#pragma warning(disable: 4996)`
//!   (`msvc-disable:4996`); `// cppcheck-suppress id` (`cppcheck:id`).
//!
//! Catch2's template tests take types as arguments, which no C++ grammar
//! reads as an expression: the types are blanked out (same length, so
//! lines and offsets stay) before the file is parsed, as is Check's
//! `END_TEST`. A file that still has syntax errors is not compared.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tree_sitter::{Language, Node};

use super::reach::{self, Grammar};
use super::recover;
use super::suppress::{keyed, shown};
use super::{Assertion, Suppression, TestCase, TestFile};
use crate::imports::{parse, text, walk};

/// What the C and C++ grammars call the nodes [`reach`] looks at.
const GRAMMAR: Grammar = Grammar {
    blocks: &["compound_statement"],
    returns: &["return_statement"],
    ifs: &["if_statement"],
    whiles: &["while_statement"],
    hoisted: &["function_definition"],
    stops: Some(is_skip_statement),
    noop: is_noop,
};

/// [`GRAMMAR`] for a file that includes `cmocka.h`, whose `skip()` stops
/// a test.
const GRAMMAR_CMOCKA: Grammar = Grammar {
    stops: Some(is_skip_statement_cmocka),
    ..GRAMMAR
};

/// The nodes of a value that is the same on every run.
const LITERALS: [&str; 14] = [
    "number_literal",
    "string_literal",
    "string_content",
    "raw_string_literal",
    "char_literal",
    "character",
    "escape_sequence",
    "concatenated_string",
    "true",
    "false",
    "null",
    "nullptr",
    "parenthesized_expression",
    "unary_expression",
];

/// Why `why` says a test inside `#if 0` does not run.
const NEVER_COMPILED: &str = "inside #if 0, which is never compiled";

/// Tests named by two identifiers, `Suite, Name`.
const SUITE_TESTS: [&str; 7] = [
    "TEST",
    "TEST_F",
    "TEST_P",
    "TYPED_TEST",
    "TYPED_TEST_P",
    "GTEST_TEST",
    "IGNORE_TEST",
];

/// Tests named by a string: Catch2's and doctest's.
const NAMED_TESTS: [&str; 20] = [
    "TEST_CASE",
    "SCENARIO",
    "TEST_CASE_METHOD",
    "SCENARIO_METHOD",
    "TEST_CASE_FIXTURE",
    "TEST_CASE_TEMPLATE",
    "TEST_CASE_TEMPLATE_DEFINE",
    "TEMPLATE_TEST_CASE",
    "TEMPLATE_TEST_CASE_METHOD",
    "TEMPLATE_SCENARIO",
    "TEMPLATE_SCENARIO_METHOD",
    "TEMPLATE_PRODUCT_TEST_CASE",
    "TEMPLATE_PRODUCT_TEST_CASE_METHOD",
    "TEMPLATE_TEST_CASE_SIG",
    "TEMPLATE_TEST_CASE_METHOD_SIG",
    "TEMPLATE_PRODUCT_TEST_CASE_SIG",
    "TEMPLATE_PRODUCT_TEST_CASE_METHOD_SIG",
    "TEMPLATE_LIST_TEST_CASE",
    "TEMPLATE_LIST_TEST_CASE_METHOD",
    "SCENARIO_TEMPLATE",
];

/// Macros whose type arguments are blanked out before parsing: how many
/// arguments to keep, and whether the rest are types each making a test.
const TEMPLATED: [(&str, usize, bool); 17] = [
    ("TEMPLATE_TEST_CASE", 2, true),
    ("TEMPLATE_TEST_CASE_METHOD", 3, true),
    ("TEMPLATE_SCENARIO", 2, true),
    ("TEMPLATE_SCENARIO_METHOD", 3, true),
    ("TEMPLATE_PRODUCT_TEST_CASE", 2, false),
    ("TEMPLATE_PRODUCT_TEST_CASE_METHOD", 3, false),
    ("TEMPLATE_TEST_CASE_SIG", 2, false),
    ("TEMPLATE_TEST_CASE_METHOD_SIG", 3, false),
    ("TEMPLATE_PRODUCT_TEST_CASE_SIG", 2, false),
    ("TEMPLATE_PRODUCT_TEST_CASE_METHOD_SIG", 3, false),
    ("TEMPLATE_LIST_TEST_CASE", 2, false),
    ("TEMPLATE_LIST_TEST_CASE_METHOD", 3, false),
    // doctest: `TEST_CASE_TEMPLATE("name", T, int, char)`: `T` is kept
    // out of the count below.
    ("TEST_CASE_TEMPLATE", 1, true),
    ("TEST_CASE_TEMPLATE_DEFINE", 1, false),
    ("SCENARIO_TEMPLATE", 1, true),
    ("TEST_CASE_TEMPLATE_INVOKE", 1, false),
    ("TEST_CASE_TEMPLATE_APPLY", 1, false),
];

/// Assertion macros that check one condition is true.
const TRUTHY: [&str; 21] = [
    "EXPECT_TRUE",
    "ASSERT_TRUE",
    "GTEST_EXPECT_TRUE",
    "GTEST_ASSERT_TRUE",
    "REQUIRE",
    "CHECK",
    "STATIC_REQUIRE",
    "STATIC_CHECK",
    "CHECK_UNARY",
    "REQUIRE_UNARY",
    "CHECK_MESSAGE",
    "REQUIRE_MESSAGE",
    "TEST_ASSERT",
    "TEST_ASSERT_TRUE",
    "TEST_ASSERT_MESSAGE",
    "TEST_ASSERT_TRUE_MESSAGE",
    "assert",
    "assert_true",
    "ck_assert",
    "ck_assert_msg",
    "fail_unless",
];

/// Assertion macros that check one condition is false.
const FALSY: [&str; 14] = [
    "EXPECT_FALSE",
    "ASSERT_FALSE",
    "GTEST_EXPECT_FALSE",
    "GTEST_ASSERT_FALSE",
    "REQUIRE_FALSE",
    "CHECK_FALSE",
    "STATIC_REQUIRE_FALSE",
    "STATIC_CHECK_FALSE",
    "CHECK_UNARY_FALSE",
    "REQUIRE_UNARY_FALSE",
    "TEST_ASSERT_FALSE",
    "TEST_ASSERT_FALSE_MESSAGE",
    "assert_false",
    "fail_if",
];

/// A macro name without the prefix Catch2 (`CATCH_CONFIG_PREFIX_ALL`) or
/// doctest (`DOCTEST_`) can give it.
fn bare(name: &str) -> &str {
    name.strip_prefix("CATCH_")
        .or_else(|| name.strip_prefix("DOCTEST_"))
        .unwrap_or(name)
}

/// True for an all-caps macro name: `TEST_CASE`, `EXPECT_EQ`.
fn is_macro_case(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// True for a character of an identifier.
fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// White space removed.
fn squeezed(raw: &str) -> String {
    raw.split_whitespace().collect()
}

/// The name of the function a call calls, when it is a plain identifier.
fn called<'s>(call: Node<'_>, src: &'s [u8]) -> Option<&'s str> {
    if call.kind() != "call_expression" {
        return None;
    }
    let function = call.child_by_field_name("function")?;
    (function.kind() == "identifier").then(|| text(function, src))
}

/// The first named child that is not a comment.
fn first_named(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let first = node
        .named_children(&mut cursor)
        .find(|child| !child.is_extra() && child.kind() != "comment");
    first
}

/// The named children of a call's argument list, comments left out.
fn arguments(call: Node<'_>) -> Vec<Node<'_>> {
    let Some(list) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    let found = list
        .named_children(&mut cursor)
        .filter(|child| !child.is_extra() && child.kind() != "comment")
        .collect();
    found
}

/// The call a statement makes: `f(…);`, or `GTEST_SKIP() << "why";`.
fn statement_call(statement: Node<'_>) -> Option<Node<'_>> {
    if statement.kind() != "expression_statement" {
        return None;
    }
    let mut at = first_named(statement)?;
    for _ in 0..8 {
        match at.kind() {
            "call_expression" => return Some(at),
            "binary_expression" => at = at.child_by_field_name("left")?,
            _ => return None,
        }
    }
    None
}

/// True for a macro that skips the rest of a test.
fn is_skip_name(name: &str, cmocka: bool) -> bool {
    matches!(
        bare(name),
        "GTEST_SKIP" | "SKIP" | "TEST_IGNORE" | "TEST_IGNORE_MESSAGE"
    ) || (cmocka && name == "skip")
}

/// True for a statement that skips the rest of a test (`GTEST_SKIP();`,
/// Catch2's `SKIP();`, Unity's `TEST_IGNORE();`).
fn is_skip_statement(statement: Node<'_>, src: &[u8]) -> bool {
    statement_call(statement)
        .and_then(|call| called(call, src))
        .is_some_and(|name| is_skip_name(name, false))
}

/// [`is_skip_statement`], with CMocka's `skip();`.
fn is_skip_statement_cmocka(statement: Node<'_>, src: &[u8]) -> bool {
    statement_call(statement)
        .and_then(|call| called(call, src))
        .is_some_and(|name| is_skip_name(name, true))
}

/// True for a statement that does nothing: `;`, or `(void)state;`.
fn is_noop(statement: Node<'_>, src: &[u8]) -> bool {
    if statement.kind() != "expression_statement" {
        return false;
    }
    match first_named(statement) {
        None => true,
        Some(inner) => {
            inner.kind() == "cast_expression"
                && inner
                    .child_by_field_name("type")
                    .is_some_and(|ty| squeezed(text(ty, src)) == "void")
                && inner
                    .child_by_field_name("value")
                    .is_some_and(|value| value.kind() == "identifier")
        }
    }
}

// ------------------------------------------------------------ masking

/// The source as it is parsed: Check's `END_TEST` and the type arguments
/// of template tests blanked out with spaces (line breaks kept), and how
/// many types each template test was given, by the byte its macro starts
/// at.
struct Masked {
    text: String,
    rows: HashMap<usize, usize>,
}

/// Where a scan of C source is: in code, a comment or a literal.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lex {
    Code,
    LineComment,
    BlockComment,
    Quoted(u8),
}

/// Steps over one byte of C source, from `state`. Returns the state after
/// it and how many bytes it took.
fn lex_step(bytes: &[u8], at: usize, state: Lex) -> (Lex, usize) {
    let byte = bytes[at];
    let next = bytes.get(at + 1).copied();
    match state {
        Lex::Code => match (byte, next) {
            (b'/', Some(b'/')) => (Lex::LineComment, 2),
            (b'/', Some(b'*')) => (Lex::BlockComment, 2),
            (b'"' | b'\'', _) => (Lex::Quoted(byte), 1),
            _ => (Lex::Code, 1),
        },
        Lex::LineComment => match (byte, next) {
            (b'\\', Some(b'\n')) => (Lex::LineComment, 2),
            (b'\n', _) => (Lex::Code, 1),
            _ => (Lex::LineComment, 1),
        },
        Lex::BlockComment => match (byte, next) {
            (b'*', Some(b'/')) => (Lex::Code, 2),
            _ => (Lex::BlockComment, 1),
        },
        Lex::Quoted(quote) => match byte {
            b'\\' => (state, 2),
            b'\n' => (Lex::Code, 1),
            b if b == quote => (Lex::Code, 1),
            _ => (state, 1),
        },
    }
}

/// From the `(` at `open`, the top-level commas of the argument list and
/// its closing `)`, or `None` when it is not closed.
fn argument_commas(bytes: &[u8], open: usize) -> Option<(Vec<usize>, usize)> {
    let mut depth = 0usize;
    let mut commas = Vec::new();
    let mut state = Lex::Code;
    let mut at = open;
    while at < bytes.len() {
        if state == Lex::Code {
            match bytes[at] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((commas, at));
                    }
                }
                b',' if depth == 1 => commas.push(at),
                _ => {}
            }
        }
        let (next, step) = lex_step(bytes, at, state);
        state = next;
        at += step;
    }
    None
}

/// Blanks out what tree-sitter cannot read as code (see [`Masked`]).
fn mask(source: &str) -> Masked {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut rows = HashMap::new();
    let mut state = Lex::Code;
    let mut depth = 0usize;
    let mut at = 0;
    while at < bytes.len() {
        let starts_word = state == Lex::Code
            && (bytes[at].is_ascii_alphabetic() || bytes[at] == b'_')
            && (at == 0 || !is_word(bytes[at - 1]));
        if starts_word {
            let end = (at..bytes.len())
                .find(|&i| !is_word(bytes[i]))
                .unwrap_or(bytes.len());
            let word = &source[at..end];
            if word == "END_TEST" {
                out[at..end].fill(b' ');
            } else if let Some(macro_end) = Some(depth)
                .filter(|d| *d == 0)
                .and_then(|_| bare_macro_line(bytes, at, end))
            {
                out[at..macro_end].fill(b' ');
                at = macro_end;
                continue;
            } else if let Some(&(_, keep, weighed)) =
                TEMPLATED.iter().find(|(name, _, _)| *name == bare(word))
            {
                let open = (end..bytes.len()).find(|&i| !bytes[i].is_ascii_whitespace());
                if let Some(open) = open.filter(|&i| bytes[i] == b'(') {
                    if let Some((commas, close)) = argument_commas(bytes, open) {
                        if commas.len() >= keep {
                            let from = commas[keep - 1];
                            for byte in &mut out[from..close] {
                                if *byte != b'\n' {
                                    *byte = b' ';
                                }
                            }
                            if weighed {
                                // doctest's own forms name the type
                                // parameter first.
                                let given = commas.len() + 1 - keep;
                                let types = if keep == 1 {
                                    given.saturating_sub(1)
                                } else {
                                    given
                                };
                                rows.insert(at, types.max(1));
                            }
                        }
                    }
                }
            }
            at = end;
            continue;
        }
        if state == Lex::Code {
            match bytes[at] {
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let (next, step) = lex_step(bytes, at, state);
        state = next;
        at += step;
    }
    let text = String::from_utf8(out).unwrap_or_else(|_| source.to_owned());
    Masked { text, rows }
}

/// Where a macro alone on its line ends, when the word at `at..end` starts
/// one, outside any parentheses: `FMT_BEGIN_NAMESPACE`, `G_BEGIN_DECLS`,
/// `FMT_PRAGMA_GCC(diagnostic ignored "-Wfoo")`. tree-sitter does not expand
/// macros and reads one as the start of a declaration, which swallows the
/// next one into an error; such a line is blanked before parsing (M32,
/// ADR 0052). The name is upper-case letters, digits and `_`, with one `_`
/// at least, and names no test or assertion; its arguments, if any, close
/// on the same line; nothing but a `//` comment follows; the line does not
/// continue a macro, and the next line does not open a block with `{`.
fn bare_macro_line(bytes: &[u8], at: usize, end: usize) -> Option<usize> {
    let word = &bytes[at..end];
    if !word.contains(&b'_')
        || !word
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
    {
        return None;
    }
    let line_start = bytes[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    if !bytes[line_start..at]
        .iter()
        .all(|b| *b == b' ' || *b == b'\t')
    {
        return None;
    }
    let line_end = bytes[end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |i| end + i);
    let after_name = (end..line_end)
        .find(|&i| !bytes[i].is_ascii_whitespace())
        .unwrap_or(line_end);
    let macro_end = if bytes.get(after_name) == Some(&b'(') {
        let (_, close) = argument_commas(bytes, after_name)?;
        if close >= line_end {
            return None;
        }
        close + 1
    } else {
        end
    };
    let rest = bytes[macro_end..line_end].trim_ascii();
    if !(rest.is_empty() || rest.starts_with(b"//")) {
        return None;
    }
    // The line before ends with `\`: this is a macro's body.
    let before = bytes[..line_start.saturating_sub(1)].trim_ascii_end();
    if before.ends_with(b"\\") {
        return None;
    }
    // A block opened on the next line belongs to the macro.
    if bytes[line_end..].trim_ascii_start().starts_with(b"{") {
        return None;
    }
    let name = std::str::from_utf8(word).ok()?;
    (!recover::could_name(name)).then_some(macro_end)
}

/// The grammar for C or C++.
fn grammar(cpp: bool) -> Language {
    if cpp {
        tree_sitter_cpp::LANGUAGE.into()
    } else {
        tree_sitter_c::LANGUAGE.into()
    }
}

// ------------------------------------------------------------ the preprocessor

/// Byte ranges the preprocessor keeps out: never compiled (`#if 0`, the
/// `#else` of `#if 1`) or compiled only sometimes (any other `#if`,
/// `#ifdef`, `#ifndef` but a header's include guard).
#[derive(Default)]
struct Regions {
    never: Vec<(usize, usize)>,
    sometimes: Vec<(usize, usize)>,
}

impl Regions {
    /// Finds the regions of a file.
    fn of(root: Node<'_>, src: &[u8]) -> Self {
        let mut found = Self::default();
        walk(root, |node| {
            match node.kind() {
                "preproc_if" | "preproc_elif" => {
                    let alternative = node.child_by_field_name("alternative");
                    let body_end = alternative.map_or(node.end_byte(), |alt| alt.start_byte());
                    let body_start = node
                        .child_by_field_name("condition")
                        .map_or(node.start_byte(), |c| c.end_byte());
                    match node
                        .child_by_field_name("condition")
                        .and_then(|c| literal_condition(c, src))
                    {
                        Some(false) => found.never.push((body_start, body_end)),
                        Some(true) => {
                            if let Some(alt) = alternative {
                                found.never.push((alt.start_byte(), alt.end_byte()));
                            }
                        }
                        None => found.sometimes.push((node.start_byte(), node.end_byte())),
                    }
                }
                "preproc_ifdef" if !is_include_guard(node, src) => {
                    found.sometimes.push((node.start_byte(), node.end_byte()));
                }
                "comment" | "string_literal" => return false,
                _ => {}
            }
            true
        });
        found
    }

    /// True when `node` is never compiled.
    fn never(&self, node: Node<'_>) -> bool {
        let at = node.start_byte();
        self.never.iter().any(|&(from, to)| from <= at && at < to)
    }

    /// True when `node` is compiled only sometimes.
    fn sometimes(&self, node: Node<'_>) -> bool {
        let at = node.start_byte();
        self.sometimes
            .iter()
            .any(|&(from, to)| from <= at && at < to)
    }
}

/// `Some(false)` for a literal `0` or `false` condition, `Some(true)` for
/// `1` or `true`, `None` for anything else.
fn literal_condition(condition: Node<'_>, src: &[u8]) -> Option<bool> {
    let mut at = condition;
    for _ in 0..8 {
        match at.kind() {
            "parenthesized_expression" => at = at.named_child(0)?,
            "false" => return Some(false),
            "true" => return Some(true),
            "number_literal" => {
                return match text(at, src) {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                }
            }
            _ => return None,
        }
    }
    None
}

/// True for `#ifndef X` followed by `#define X`: a header's include guard.
fn is_include_guard(node: Node<'_>, src: &[u8]) -> bool {
    if !text(node, src).trim_start().starts_with("#ifndef") {
        return false;
    }
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    let mut cursor = node.walk();
    let defined = node
        .named_children(&mut cursor)
        .filter(|child| child.id() != name.id() && child.kind() != "comment")
        .find(|_| true);
    defined.is_some_and(|define| {
        define.kind() == "preproc_def"
            && define
                .child_by_field_name("name")
                .is_some_and(|defined| text(defined, src) == text(name, src))
    })
}

// ------------------------------------------------------------ what a file holds

/// What one pass over a file finds before its tests are read.
#[derive(Default)]
struct Facts {
    /// Quoted includes, as written.
    includes: Vec<String>,
    /// It includes `unity.h`.
    unity: bool,
    /// It includes `cmocka.h`.
    cmocka: bool,
    /// CMocka: each function `cmocka_unit_test(…)` lists, how many times
    /// and the line of the first entry.
    cmocka_tests: BTreeMap<String, (usize, usize)>,
    /// Unity: the functions `RUN_TEST(…)` names.
    run_tests: BTreeSet<String>,
    /// GoogleTest: the rows of each value-parameterized suite, `None` when
    /// an instantiation is not a literal list.
    rows: HashMap<String, Option<usize>>,
    /// GoogleTest fixtures whose `SetUp` skips.
    skipping_fixtures: HashSet<String>,
}

/// The last part of an include's path.
fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// True for a function body with a statement of its own that skips.
fn body_skips(body: Node<'_>, src: &[u8], cmocka: bool) -> bool {
    let mut cursor = body.walk();
    let skips = body.named_children(&mut cursor).any(|statement| {
        statement_call(statement)
            .and_then(|call| called(call, src))
            .is_some_and(|name| is_skip_name(name, cmocka))
    });
    skips
}

/// The rows a GoogleTest parameter generator makes, when it is a literal
/// list: `Values(a, b, c)` makes 3, `Bool()` 2.
fn generator_rows(generator: Node<'_>, src: &[u8]) -> Option<usize> {
    if generator.kind() != "call_expression" {
        return None;
    }
    let function = squeezed(text(generator.child_by_field_name("function")?, src));
    let function = function.trim_start_matches("::");
    match function.strip_prefix("testing::").unwrap_or(function) {
        "Values" => Some(arguments(generator).len()).filter(|rows| *rows > 0),
        "Bool" => Some(2),
        _ => None,
    }
}

impl Facts {
    /// Reads the facts of a file.
    fn of(root: Node<'_>, src: &[u8]) -> Self {
        let mut found = Self::default();
        walk(root, |node| match node.kind() {
            "preproc_include" => {
                if let Some(path) = node.child_by_field_name("path") {
                    let raw = text(path, src);
                    let inner = raw.trim_matches(['"', '<', '>']).replace('\\', "/");
                    match base_name(&inner) {
                        "unity.h" => found.unity = true,
                        "cmocka.h" => found.cmocka = true,
                        _ => {}
                    }
                    if path.kind() == "string_literal" && !inner.is_empty() {
                        found.includes.push(inner);
                    }
                }
                false
            }
            "call_expression" => {
                let Some(name) = called(node, src) else {
                    return true;
                };
                let args = arguments(node);
                let first = args
                    .first()
                    .filter(|arg| arg.kind() == "identifier")
                    .map(|arg| text(*arg, src).to_owned());
                if name.starts_with("cmocka_unit_test") || name == "unit_test" {
                    if let Some(first) = first {
                        let entry = found
                            .cmocka_tests
                            .entry(first)
                            .or_insert((0, node.start_position().row + 1));
                        entry.0 += 1;
                    }
                } else if name == "RUN_TEST" {
                    if let Some(first) = first {
                        found.run_tests.insert(first);
                    }
                } else if matches!(name, "INSTANTIATE_TEST_SUITE_P" | "INSTANTIATE_TEST_CASE_P")
                    && args.len() >= 3
                {
                    let suite = squeezed(text(args[1], src));
                    let rows = generator_rows(args[2], src);
                    let entry = found.rows.entry(suite).or_insert(Some(0));
                    *entry = match (*entry, rows) {
                        (Some(had), Some(more)) => Some(had + more),
                        _ => None,
                    };
                }
                true
            }
            "function_definition" => {
                // `void Fixture::SetUp() { GTEST_SKIP(); }`, in a class or
                // outside it.
                let declarator = node
                    .child_by_field_name("declarator")
                    .and_then(|d| d.child_by_field_name("declarator"));
                if let (Some(declarator), Some(body)) =
                    (declarator, node.child_by_field_name("body"))
                {
                    let name = squeezed(text(declarator, src));
                    let (fixture, method) = match name.rsplit_once("::") {
                        Some((fixture, method)) => (Some(fixture.to_owned()), method.to_owned()),
                        None => (enclosing_class(node, src), name.clone()),
                    };
                    if matches!(
                        method.as_str(),
                        "SetUp" | "SetUpTestSuite" | "SetUpTestCase"
                    ) && body_skips(body, src, false)
                    {
                        if let Some(fixture) = fixture {
                            found.skipping_fixtures.insert(fixture);
                        }
                    }
                }
                true
            }
            "comment" | "string_literal" | "raw_string_literal" | "char_literal" => false,
            _ => true,
        });
        found
    }
}

/// The name of the class or struct a method is written in.
fn enclosing_class(method: Node<'_>, src: &[u8]) -> Option<String> {
    let list = method.parent()?;
    if list.kind() != "field_declaration_list" {
        return None;
    }
    let class = list.parent()?;
    class
        .child_by_field_name("name")
        .map(|name| squeezed(text(name, src)))
}

/// The identifiers under some nodes: what a test uses.
fn identifiers(nodes: &[Node<'_>], src: &[u8], into: &mut BTreeSet<String>) {
    for node in nodes {
        walk(*node, |inner| match inner.kind() {
            "comment" | "string_literal" | "raw_string_literal" | "char_literal" => false,
            "identifier" | "type_identifier" | "field_identifier" | "namespace_identifier" => {
                into.insert(text(inner, src).to_owned());
                false
            }
            _ => true,
        });
    }
}

/// The text of a string argument without its quotes.
fn string_value(node: Node<'_>, src: &[u8]) -> String {
    let raw = text(node, src);
    if node.kind() == "string_literal" {
        raw.strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(raw)
            .to_owned()
    } else {
        raw.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// The string literals in the arguments of a test macro, in order, each
/// searched for in its argument (doctest's `"name" * doctest::skip()`).
fn string_arguments<'t>(args: &[Node<'t>]) -> Vec<Node<'t>> {
    let mut found = Vec::new();
    for arg in args {
        let mut first = None;
        walk(*arg, |inner| {
            if first.is_some() {
                return false;
            }
            match inner.kind() {
                "string_literal" | "raw_string_literal" | "concatenated_string" => {
                    first = Some(inner);
                    false
                }
                "call_expression" | "argument_list" => false,
                _ => true,
            }
        });
        found.extend(first);
    }
    found
}

/// True for a doctest decorator that keeps a test from failing a run:
/// `* doctest::skip()`, `* doctest::skip(true)`, `* doctest::should_fail()`.
fn doctest_skips(args: &[Node<'_>], src: &[u8]) -> bool {
    let mut skips = false;
    for arg in args {
        walk(*arg, |inner| {
            if inner.kind() == "call_expression" {
                let function = inner
                    .child_by_field_name("function")
                    .map(|f| squeezed(text(f, src)))
                    .unwrap_or_default();
                let function = function.trim_start_matches("::");
                let function = function.strip_prefix("doctest::").unwrap_or(function);
                let given = inner
                    .child_by_field_name("arguments")
                    .map(|a| squeezed(text(a, src)))
                    .unwrap_or_default();
                if matches!(function, "skip" | "should_fail")
                    && matches!(given.as_str(), "()" | "(true)")
                {
                    skips = true;
                }
                return false;
            }
            true
        });
    }
    skips
}

/// True for Catch2 tags that keep a test out of a default run: a hidden
/// tag (`[.]`, `[.slow]`, `[!hide]`) or `[!shouldfail]`.
fn hidden_tags(tags: &str) -> bool {
    tags.split('[').skip(1).any(|tag| {
        let tag = tag.split(']').next().unwrap_or_default();
        tag.starts_with('.') || tag == "!hide" || tag == "!shouldfail"
    })
}

/// A test macro's head: `TEST(Suite, Name)` or `TEST_CASE("name")`.
struct Head<'t> {
    /// The macro, without a `CATCH_` or `DOCTEST_` prefix.
    name: String,
    /// Its arguments: identifiers or parameter declarations for
    /// `TEST(…)`, expressions for `TEST_CASE(…)`.
    args: Vec<Node<'t>>,
    /// The byte the macro's name starts at.
    start: usize,
}

/// A test's head as a function definition: `TEST(Suite, Name) { … }`, read
/// by the C++ grammar as a function with two parameters, or Check's
/// `START_TEST(name) { … }`, read as a function named `name` of type
/// `START_TEST`.
fn function_head<'t>(function: Node<'t>, src: &[u8]) -> Option<Head<'t>> {
    let declarator = function.child_by_field_name("declarator")?;
    if let Some(ty) = function.child_by_field_name("type") {
        if text(ty, src) == "START_TEST" && declarator.kind() == "parenthesized_declarator" {
            let name = first_named(declarator).filter(|n| n.kind() == "identifier")?;
            return Some(Head {
                name: "START_TEST".to_owned(),
                args: vec![name],
                start: ty.start_byte(),
            });
        }
        return None;
    }
    if declarator.kind() != "function_declarator" {
        return None;
    }
    let macro_name = declarator.child_by_field_name("declarator")?;
    if macro_name.kind() != "identifier" {
        return None;
    }
    let name = bare(text(macro_name, src));
    if !SUITE_TESTS.contains(&name) {
        return None;
    }
    let parameters = declarator.child_by_field_name("parameters")?;
    let mut cursor = parameters.walk();
    let args: Vec<Node<'t>> = parameters
        .named_children(&mut cursor)
        .filter(|p| p.kind() != "comment")
        .collect();
    // Each parameter must be one identifier, as a suite or a test name is.
    let plain = args.iter().all(|arg| {
        let raw = text(*arg, src);
        !raw.is_empty() && raw.bytes().all(is_word)
    });
    plain.then(|| Head {
        name: name.to_owned(),
        args,
        start: macro_name.start_byte(),
    })
}

/// A test's head as a macro call left without its `;` and followed by a
/// block: `TEST_CASE("name") { … }`, and `TEST(Group, Name) { … }` as the C
/// grammar reads it. The head and its block.
fn call_head<'t>(statement: Node<'t>, src: &[u8]) -> Option<(Head<'t>, Node<'t>)> {
    let call = first_named(statement)?;
    let name = bare(called(call, src)?);
    let mut cursor = statement.walk();
    let missing = statement
        .children(&mut cursor)
        .last()
        .is_some_and(|last| last.is_missing());
    if !missing {
        return None;
    }
    let mut next = statement.next_named_sibling();
    while let Some(sibling) = next.filter(|n| n.kind() == "comment") {
        next = sibling.next_named_sibling();
    }
    let body = next.filter(|n| n.kind() == "compound_statement")?;
    let wanted = SUITE_TESTS.contains(&name) || NAMED_TESTS.contains(&name) || name == "TEST_SUITE";
    wanted.then(|| {
        (
            Head {
                name: name.to_owned(),
                args: arguments(call),
                start: call.start_byte(),
            },
            body,
        )
    })
}

/// The scopes around the node being visited.
struct Scope {
    end: usize,
    /// A function or a test's block, not a suite or the file.
    function: bool,
    /// For a test: its index.
    test: Option<usize>,
    /// A doctest `TEST_SUITE`'s name.
    suite: Option<String>,
    /// The suite skips its tests.
    skipped: bool,
}

/// What [`read`] knows while it reads one file.
struct Reader<'a> {
    src: &'a [u8],
    facts: &'a Facts,
    masked: &'a Masked,
    grammar: &'a Grammar,
    dead: &'a reach::Dead,
    regions: &'a Regions,
    /// The stems of the file's quoted includes: every test uses them.
    subjects: Vec<String>,
    file: TestFile,
    open: Vec<Scope>,
    /// Test and suite blocks, by node id, to open when the walk gets there.
    pending: HashMap<usize, Scope>,
    /// CMocka tests whose function the file defines.
    defined: BTreeSet<String>,
}

impl Reader<'_> {
    /// Adds a test, from its head and body.
    fn add_test<'t>(
        &mut self,
        id: String,
        at: Node<'t>,
        marked: bool,
        body: Option<Node<'t>>,
        used: &[Node<'t>],
        weight: usize,
    ) -> usize {
        let src = self.src;
        let why = if marked {
            None
        } else if self.regions.never(at) {
            Some(NEVER_COMPILED)
        } else if let Some(why) = self.dead.why(at) {
            Some(why)
        } else if body.is_some_and(|body| reach::does_nothing(body, src, self.grammar)) {
            Some("its body does nothing")
        } else {
            None
        };
        let skips = body.is_some_and(|body| body_skips(body, src, self.facts.cmocka));
        let gated = self.regions.sometimes(at);
        let mut uses = BTreeSet::new();
        identifiers(used, src, &mut uses);
        if let Some(body) = body {
            identifiers(&[body], src, &mut uses);
        }
        uses.extend(self.subjects.iter().cloned());
        self.file.tests.push(TestCase {
            id,
            line: at.start_position().row + 1,
            skipped: marked || gated || skips || why.is_some(),
            skipped_rows: 0,
            why,
            marker: false,
            focused: false,
            uses,
            weight: weight.max(1),
        });
        self.file.tests.len() - 1
    }

    /// The test a suite-style head (`TEST(Suite, Name)`) makes.
    fn suite_test<'t>(
        &mut self,
        head: &Head<'t>,
        at: Node<'t>,
        body: Option<Node<'t>>,
    ) -> Option<usize> {
        let src = self.src;
        let [suite, name] = head.args.as_slice() else {
            return None;
        };
        let suite = text(*suite, src).trim();
        let name = text(*name, src).trim();
        let disabled = suite.starts_with("DISABLED_") || name.starts_with("DISABLED_");
        let marked = disabled
            || head.name == "IGNORE_TEST"
            || (matches!(head.name.as_str(), "TEST_F" | "TEST_P")
                && self.facts.skipping_fixtures.contains(suite));
        let weight = if head.name == "TEST_P" {
            self.facts.rows.get(suite).copied().flatten().unwrap_or(1)
        } else {
            1
        };
        let id = format!(
            "{}::{}",
            suite.trim_start_matches("DISABLED_"),
            name.trim_start_matches("DISABLED_")
        );
        Some(self.add_test(id, at, marked, body, &head.args, weight))
    }

    /// The test a named head (`TEST_CASE("name", "[tags]")`) makes.
    fn named_test<'t>(&mut self, head: &Head<'t>, at: Node<'t>, body: Node<'t>) -> usize {
        let src = self.src;
        let strings = string_arguments(&head.args);
        let name = strings
            .first()
            .map(|n| string_value(*n, src))
            .unwrap_or_else(|| format!("(unnamed, line {})", at.start_position().row + 1));
        let tags = strings
            .get(1)
            .map(|n| string_value(*n, src))
            .unwrap_or_default();
        let in_skipped_suite = self.open.iter().any(|scope| scope.skipped);
        let marked = in_skipped_suite
            || hidden_tags(&tags)
            || name.starts_with("./")
            || doctest_skips(&head.args, src);
        let suites: Vec<&str> = self
            .open
            .iter()
            .filter_map(|scope| scope.suite.as_deref())
            .collect();
        let id = if suites.is_empty() {
            name
        } else {
            format!("{}::{name}", suites.join("::"))
        };
        let weight = self.masked.rows.get(&head.start).copied().unwrap_or(1);
        self.add_test(id, at, marked, Some(body), &head.args, weight)
    }

    /// True for a function a C framework runs as a test: listed by CMocka
    /// or `RUN_TEST`, or `void test…(void)` in a Unity file.
    fn c_test_function(&self, function: Node<'_>, name: &str) -> bool {
        let src = self.src;
        if self.facts.cmocka_tests.contains_key(name) || self.facts.run_tests.contains(name) {
            return true;
        }
        if !self.facts.unity || !name.starts_with("test") {
            return false;
        }
        let returns_void = function
            .child_by_field_name("type")
            .is_some_and(|ty| text(ty, src) == "void");
        let no_parameters = function
            .child_by_field_name("declarator")
            .and_then(|d| d.child_by_field_name("parameters"))
            .is_some_and(|p| matches!(squeezed(text(p, src)).as_str(), "()" | "(void)"));
        returns_void && no_parameters
    }

    /// Visits a function definition: a test or not.
    fn function(&mut self, node: Node<'_>) {
        let src = self.src;
        let body = node.child_by_field_name("body");
        let mut test = None;
        if let Some(head) = function_head(node, src) {
            test = if head.name == "START_TEST" {
                let name = text(head.args[0], src).to_owned();
                Some(self.add_test(name, node, false, body, &[], 1))
            } else {
                self.suite_test(&head, node, body)
            };
        } else if let Some(name) = node
            .child_by_field_name("declarator")
            .filter(|d| d.kind() == "function_declarator")
            .and_then(|d| d.child_by_field_name("declarator"))
            .filter(|d| d.kind() == "identifier")
            .map(|d| text(d, src).to_owned())
        {
            if self.c_test_function(node, &name) {
                let weight = self
                    .facts
                    .cmocka_tests
                    .get(&name)
                    .map_or(1, |(count, _)| *count);
                if self.facts.cmocka_tests.contains_key(&name) {
                    self.defined.insert(name.clone());
                }
                test = Some(self.add_test(name, node, false, body, &[], weight));
            }
        }
        self.open.push(Scope {
            end: node.end_byte(),
            function: true,
            test,
            suite: None,
            skipped: false,
        });
    }

    /// Visits an assertion call, if it is one.
    fn assertion(&mut self, node: Node<'_>, name: &str) {
        if self.dead.why(node).is_some()
            || self.regions.never(node)
            || cannot_fail(node, name, self.src)
        {
            return;
        }
        let test = self
            .open
            .iter()
            .rev()
            .find(|scope| scope.function)
            .and_then(|scope| scope.test);
        let whole = text(node, self.src);
        self.file.assertions.push(Assertion {
            line: node.start_position().row + 1,
            shape: squeezed(whole),
            text: whole.split_whitespace().collect::<Vec<_>>().join(" "),
            weight: test
                .and_then(|index| self.file.tests.get(index))
                .map_or(1, |test| test.weight),
            test,
        });
    }
}

/// True for the name of an assertion macro or function.
fn is_assertion(name: &str) -> bool {
    let macro_name = bare(name);
    if is_macro_case(macro_name) {
        if matches!(macro_name, "CHECKED_IF" | "CHECKED_ELSE" | "CHECK_NOFAIL") {
            return false;
        }
        return matches!(
            macro_name,
            "FAIL"
                | "FAIL_CHECK"
                | "ADD_FAILURE"
                | "ADD_FAILURE_AT"
                | "GTEST_FAIL"
                | "GTEST_FAIL_AT"
                | "CHECK"
                | "REQUIRE"
                | "STATIC_REQUIRE"
                | "STATIC_CHECK"
                | "TEST_ASSERT"
                | "TEST_FAIL"
        ) || [
            "EXPECT_",
            "ASSERT_",
            "GTEST_EXPECT_",
            "GTEST_ASSERT_",
            "CHECK_",
            "REQUIRE_",
            "STATIC_REQUIRE_",
            "STATIC_CHECK_",
            "FAST_CHECK_",
            "FAST_REQUIRE_",
            "TEST_ASSERT_",
            "TEST_FAIL_",
        ]
        .iter()
        .any(|prefix| macro_name.starts_with(prefix));
    }
    matches!(
        name,
        "assert" | "fail" | "fail_msg" | "fail_unless" | "fail_if"
    ) || name.starts_with("assert_")
        || name.starts_with("ck_assert")
        || name.starts_with("ck_abort")
}

/// True for a literal that is certainly true: `true`, a non-zero decimal.
fn is_true_literal(raw: &str) -> bool {
    let raw = raw.trim_start_matches('(').trim_end_matches(')');
    raw == "true"
        || (!raw.is_empty()
            && raw.bytes().all(|b| b.is_ascii_digit())
            && raw.bytes().any(|b| b != b'0'))
}

/// True for a literal that is certainly false: `false`, `0`.
fn is_false_literal(raw: &str) -> bool {
    let raw = raw.trim_start_matches('(').trim_end_matches(')');
    raw == "false" || (!raw.is_empty() && raw.bytes().all(|b| b == b'0'))
}

/// True for a condition that certainly holds: a true literal, or `==` of
/// the same literal on both sides.
fn holds(condition: Node<'_>, src: &[u8]) -> bool {
    if condition.kind() == "binary_expression" {
        let operator = condition
            .child_by_field_name("operator")
            .map_or("", |op| text(op, src));
        let (Some(left), Some(right)) = (
            condition.child_by_field_name("left"),
            condition.child_by_field_name("right"),
        ) else {
            return false;
        };
        return operator == "=="
            && reach::constant(left, &LITERALS)
            && reach::constant(right, &LITERALS)
            && squeezed(text(left, src)) == squeezed(text(right, src));
    }
    reach::constant(condition, &LITERALS) && is_true_literal(&squeezed(text(condition, src)))
}

/// True for an equality assertion: `EXPECT_EQ`, `TEST_ASSERT_EQUAL_INT`,
/// `ck_assert_int_eq`, `assert_int_equal`, `CHECK_EQ`.
fn is_equality(name: &str) -> bool {
    let lower = bare(name).to_ascii_lowercase();
    (lower.ends_with("_eq")
        || lower.ends_with("streq")
        || lower.ends_with("strcaseeq")
        || lower.ends_with("_equal")
        || lower.contains("_equal_"))
        && !lower.contains("not")
        && !lower.contains("within")
}

/// True for an assertion that cannot fail: it checks only literals, and
/// they certainly pass (`EXPECT_TRUE(true)`, `REQUIRE(1 == 1)`,
/// `ASSERT_EQ(1, 1)`, `assert(1)`). One that certainly fails
/// (`ASSERT_TRUE(false)`, `FAIL()`) counts like any other.
fn cannot_fail(call: Node<'_>, name: &str, src: &[u8]) -> bool {
    let macro_name = bare(name);
    let args = arguments(call);
    if TRUTHY.contains(&macro_name) {
        return args.first().is_some_and(|first| holds(*first, src));
    }
    if FALSY.contains(&macro_name) {
        return args.first().is_some_and(|first| {
            reach::constant(*first, &LITERALS) && is_false_literal(&squeezed(text(*first, src)))
        });
    }
    if is_equality(macro_name) {
        return match args.as_slice() {
            [left, right, ..] => {
                reach::constant(*left, &LITERALS)
                    && reach::constant(*right, &LITERALS)
                    && squeezed(text(*left, src)) == squeezed(text(*right, src))
            }
            _ => false,
        };
    }
    false
}

/// Reads the tests, skips and assertions of a C or C++ file.
#[must_use]
pub(super) fn read(source: &str, cpp: bool) -> TestFile {
    let masked = mask(source);
    let Some(tree) = parse(&masked.text, &grammar(cpp)) else {
        return TestFile {
            has_errors: true,
            ..TestFile::default()
        };
    };
    let src = masked.text.as_bytes();
    let root = tree.root_node();
    let facts = Facts::of(root, src);
    let grammar = if facts.cmocka {
        &GRAMMAR_CMOCKA
    } else {
        &GRAMMAR
    };
    let dead = reach::Dead::of(root, src, grammar);
    let regions = Regions::of(root, src);
    let error_line = recover::first_harmful(root, src);
    let mut imports = BTreeMap::new();
    let mut subjects = Vec::new();
    for include in &facts.includes {
        let stem = base_name(include)
            .split('.')
            .next()
            .unwrap_or_default()
            .to_owned();
        if !stem.is_empty() && stem.bytes().all(is_word) {
            imports.insert(stem.clone(), include.clone());
            subjects.push(stem);
        }
    }
    let mut reader = Reader {
        src,
        facts: &facts,
        masked: &masked,
        grammar,
        dead: &dead,
        regions: &regions,
        subjects,
        file: TestFile {
            imports,
            has_errors: error_line.is_some(),
            error_line,
            ..TestFile::default()
        },
        open: vec![Scope {
            end: usize::MAX,
            function: false,
            test: None,
            suite: None,
            skipped: false,
        }],
        pending: HashMap::new(),
        defined: BTreeSet::new(),
    };
    walk(root, |node| {
        while reader.open.len() > 1
            && reader
                .open
                .last()
                .is_some_and(|scope| node.start_byte() >= scope.end)
        {
            reader.open.pop();
        }
        if let Some(scope) = reader.pending.remove(&node.id()) {
            reader.open.push(scope);
        }
        match node.kind() {
            "comment"
            | "string_literal"
            | "raw_string_literal"
            | "char_literal"
            | "concatenated_string"
            | "system_lib_string"
            | "preproc_arg" => false,
            "function_definition" => {
                reader.function(node);
                true
            }
            "expression_statement" => {
                if let Some((head, body)) = call_head(node, src) {
                    if head.name == "TEST_SUITE" {
                        let strings = string_arguments(&head.args);
                        let skipped = doctest_skips(&head.args, src);
                        let name = strings.first().map(|n| string_value(*n, src));
                        reader.pending.insert(
                            body.id(),
                            Scope {
                                end: body.end_byte(),
                                function: false,
                                test: None,
                                suite: name,
                                skipped,
                            },
                        );
                    } else {
                        let test = if SUITE_TESTS.contains(&head.name.as_str()) {
                            reader.suite_test(&head, node, Some(body))
                        } else {
                            Some(reader.named_test(&head, node, body))
                        };
                        reader.pending.insert(
                            body.id(),
                            Scope {
                                end: body.end_byte(),
                                function: true,
                                test,
                                suite: None,
                                skipped: false,
                            },
                        );
                    }
                }
                true
            }
            "call_expression" => match called(node, src) {
                Some(name) if is_assertion(name) => {
                    reader.assertion(node, name);
                    // An assertion's arguments hold no more assertions.
                    false
                }
                _ => true,
            },
            _ => true,
        }
    });
    // CMocka entries whose function is in another file.
    for (name, (count, line)) in &facts.cmocka_tests {
        if reader.defined.contains(name) {
            continue;
        }
        reader.file.tests.push(TestCase {
            id: name.clone(),
            line: *line,
            skipped: false,
            skipped_rows: 0,
            why: None,
            marker: false,
            focused: false,
            uses: reader.subjects.iter().cloned().collect(),
            weight: *count,
        });
    }
    reader.file
}

/// The repo path a quoted include names, among the paths `exists`
/// accepts: next to the including file, then in each folder above it, each
/// also under `include/` and `src/` (where `-I` points).
pub(super) fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if spec.starts_with('/') || spec.contains(':') {
        return None;
    }
    let spec = spec.replace('\\', "/");
    let mut folder: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    folder.pop();
    for depth in (0..=folder.len()).rev() {
        for extra in [&[][..], &["include"][..], &["src"][..]] {
            let mut parts: Vec<&str> = folder[..depth].to_vec();
            parts.extend_from_slice(extra);
            let mut escaped = false;
            for part in spec.split('/') {
                match part {
                    "" | "." => {}
                    ".." => escaped |= parts.pop().is_none(),
                    other => parts.push(other),
                }
            }
            if escaped {
                continue;
            }
            let candidate = parts.join("/");
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// True for a path that holds C or C++ tests by its name or folder alone:
/// any C or C++ file in a `test`, `tests`, `unittest` or `unittests` folder,
/// and a source file named `*_test.c`, `*_tests.cc`, `*_unittest.cc`,
/// `test_*.c`, `*Test.cpp` or `*Tests.cpp`.
pub(super) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((name, folders)) = parts.split_last() else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    let source = matches!(extension.as_str(), "c" | "cc" | "cpp" | "cxx" | "c++");
    let header = matches!(extension.as_str(), "h" | "hh" | "hpp" | "hxx");
    if stem.is_empty() || !(source || header) {
        return false;
    }
    let in_folder = folders.iter().any(|folder| {
        ["test", "tests", "unittest", "unittests"]
            .iter()
            .any(|name| folder.eq_ignore_ascii_case(name))
    });
    let lower = stem.to_ascii_lowercase();
    let by_name = source
        && (["_test", "_tests", "_unittest", "_unittests"]
            .iter()
            .any(|suffix| lower.len() > suffix.len() && lower.ends_with(suffix))
            || (lower.starts_with("test_") && lower.len() > 5)
            || ["Test", "Tests"]
                .iter()
                .any(|suffix| stem.len() > suffix.len() && stem.ends_with(suffix)));
    in_folder || by_name
}

/// True for an include of a test framework's header.
fn includes_framework(source: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('#') else {
            return false;
        };
        let Some(path) = rest.trim_start().strip_prefix("include") else {
            return false;
        };
        let path = path.trim().trim_matches(['"', '<', '>']).replace('\\', "/");
        let path = path.split(['"', '>']).next().unwrap_or_default();
        ["gtest/", "gmock/", "catch2/", "doctest"]
            .iter()
            .any(|needle| path.contains(needle))
            || matches!(
                base_name(path),
                "gtest.h"
                    | "gmock.h"
                    | "catch.hpp"
                    | "catch_amalgamated.hpp"
                    | "unity.h"
                    | "unity_fixture.h"
                    | "cmocka.h"
                    | "check.h"
            )
    })
}

/// True for C or C++ source that holds tests although its path does not
/// say so: it includes a test framework's header and defines a test.
pub(super) fn holds_tests(source: &str, cpp: bool) -> bool {
    includes_framework(source) && !read(source, cpp).tests.is_empty()
}

/// The rules in `(a, b)` after a directive, or none.
fn listed(rest: &str) -> Vec<String> {
    rest.trim_start()
        .strip_prefix('(')
        .and_then(|list| list.split(')').next())
        .map(|list| list.split(',').map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `rest` when `body` starts with `word` and the word ends there.
fn word_after<'a>(body: &'a str, word: &str) -> Option<&'a str> {
    let rest = body.strip_prefix(word)?;
    match rest.bytes().next() {
        Some(b) if is_word(b) || b == b'-' => None,
        _ => Some(rest),
    }
}

/// The suppressions a comment holds: clang-tidy's (and cpplint's)
/// `NOLINT…`, cppcheck's `cppcheck-suppress…`. The directive must open the
/// comment.
fn comment_keys(raw: &str) -> Vec<String> {
    let body = raw
        .strip_prefix("//")
        .or_else(|| {
            raw.strip_prefix("/*")
                .map(|inner| inner.strip_suffix("*/").unwrap_or(inner))
        })
        .unwrap_or(raw);
    let body = body.trim_start_matches(['/', '*', '!']).trim();
    for (form, tool) in [
        ("NOLINTNEXTLINE", "nolint"),
        ("NOLINTBEGIN", "nolint-range"),
        ("NOLINT", "nolint"),
    ] {
        if let Some(rest) = word_after(body, form) {
            return keyed(tool, listed(rest));
        }
    }
    if word_after(body, "NOLINTEND").is_some() {
        return Vec::new();
    }
    let Some(rest) = body.strip_prefix("cppcheck-suppress") else {
        return Vec::new();
    };
    let (tool, rest) = if let Some(rest) = rest
        .strip_prefix("-file")
        .or_else(|| rest.strip_prefix("-begin"))
    {
        ("cppcheck-range", rest)
    } else if let Some(rest) = rest.strip_prefix("-macro") {
        ("cppcheck", rest)
    } else if rest.starts_with("-end") {
        return Vec::new();
    } else {
        ("cppcheck", rest)
    };
    if let Some(list) = rest.strip_prefix('[') {
        let list = list.split(']').next().unwrap_or_default();
        return keyed(tool, list.split(',').map(str::to_owned));
    }
    if !rest.starts_with(char::is_whitespace) {
        return Vec::new();
    }
    let id = rest
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(';');
    if id.is_empty() {
        return Vec::new();
    }
    keyed(tool, [id.to_owned()])
}

/// The suppressions a `#pragma` holds: `GCC diagnostic ignored "-Wfoo"`
/// (or `clang …`), MSVC's `warning(disable: 4996 4100)` and
/// `warning(suppress: 4996)`. `push` and `pop` do not count.
fn pragma_keys(argument: &str) -> Vec<String> {
    let code = argument.split("//").next().unwrap_or_default();
    let words: Vec<&str> = code.split_whitespace().collect();
    if let [compiler, "diagnostic", "ignored", flag, ..] = words.as_slice() {
        if matches!(*compiler, "GCC" | "clang") {
            let flag = flag.trim_matches('"');
            if !flag.is_empty() {
                return vec![format!("pragma-ignored:{flag}")];
            }
        }
        return Vec::new();
    }
    let Some(list) = code
        .trim()
        .strip_prefix("warning")
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('('))
    else {
        return Vec::new();
    };
    let list = list.split(')').next().unwrap_or_default();
    let mut keys = Vec::new();
    for specifier in list.split(';') {
        let Some((name, numbers)) = specifier.split_once(':') else {
            continue;
        };
        if matches!(name.trim(), "disable" | "suppress") {
            keys.extend(
                numbers
                    .split_whitespace()
                    .map(|number| format!("msvc-disable:{number}")),
            );
        }
    }
    keys
}

/// The lint suppressions in a C or C++ source file.
pub(super) fn suppressions(source: &str, cpp: bool) -> Vec<Suppression> {
    let masked = mask(source);
    let Some(tree) = parse(&masked.text, &grammar(cpp)) else {
        return Vec::new();
    };
    let src = masked.text.as_bytes();
    let mut found = Vec::new();
    walk(tree.root_node(), |node| {
        let line = node.start_position().row + 1;
        let (keys, shown_text) = match node.kind() {
            "comment" => (comment_keys(text(node, src)), text(node, src)),
            "preproc_call" => {
                let directive = node
                    .child_by_field_name("directive")
                    .map(|d| squeezed(text(d, src)))
                    .unwrap_or_default();
                if directive != "#pragma" {
                    return false;
                }
                let argument = node
                    .child_by_field_name("argument")
                    .map_or("", |a| text(a, src));
                let mut keys = pragma_keys(argument);
                // A comment after a directive is part of its argument.
                if let Some((_, comment)) = argument.split_once("//") {
                    keys.extend(comment_keys(&format!("//{comment}")));
                }
                (keys, text(node, src))
            }
            "string_literal"
            | "raw_string_literal"
            | "char_literal"
            | "concatenated_string"
            | "system_lib_string" => return false,
            _ => return true,
        };
        for key in keys {
            found.push(Suppression {
                key,
                line,
                text: shown(shown_text),
            });
        }
        false
    });
    found
}

#[cfg(test)]
mod tests {
    use super::super::{read_test_file, Language as Lang};
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

    fn cpp(source: &str) -> TestFile {
        read(source, true)
    }

    fn c(source: &str) -> TestFile {
        read(source, false)
    }

    fn keys(source: &str, cpp: bool) -> Vec<String> {
        suppressions(source, cpp)
            .into_iter()
            .map(|s| s.key)
            .collect()
    }

    const DOES_NOTHING: Option<&str> = Some("its body does nothing");

    #[test]
    fn googletest_tests_and_assertions_are_read() {
        let file = cpp(r#"#include <gtest/gtest.h>
#include "pricing.h"

TEST(Cart, Adds) {
  EXPECT_EQ(total(1, 2), 3);
  ASSERT_TRUE(ok()) << "message";
}

class CartTest : public ::testing::Test {
 protected:
  void SetUp() override { cart.clear(); }
  Cart cart;
};

TEST_F(CartTest, Empty) {
  EXPECT_THAT(cart.items(), ::testing::IsEmpty());
  ASSERT_THROW(cart.pop(), std::out_of_range);
}

namespace {
TYPED_TEST(Typed, Works) { EXPECT_NE(TypeParam(), TypeParam(1)); }
}  // namespace
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("Cart::Adds", false, None, 1),
                ("CartTest::Empty", false, None, 1),
                ("Typed::Works", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 4);
        assert!(file.tests[0].uses.contains("total"));
        assert!(file.tests[0].uses.contains("pricing"));
        assert_eq!(
            shapes(&file),
            [
                "EXPECT_EQ(total(1,2),3)",
                "ASSERT_TRUE(ok())",
                "EXPECT_THAT(cart.items(),::testing::IsEmpty())",
                "ASSERT_THROW(cart.pop(),std::out_of_range)",
                "EXPECT_NE(TypeParam(),TypeParam(1))",
            ]
        );
        let tests: Vec<Option<usize>> = file.assertions.iter().map(|a| a.test).collect();
        assert_eq!(tests, [Some(0), Some(0), Some(1), Some(1), Some(2)]);
        assert_eq!(file.assertions[0].line, 5);
        assert_eq!(
            file.imports,
            BTreeMap::from([("pricing".to_owned(), "pricing.h".to_owned())])
        );
    }

    #[test]
    fn friend_test_and_look_alikes_are_not_tests() {
        let file = cpp(r#"#include <gtest/gtest_prod.h>
class Cart {
  FRIEND_TEST(CartTest, Adds);
};
// TEST(Cart, Commented) { EXPECT_TRUE(x); }
const char* s = "TEST(Cart, InAString) { EXPECT_TRUE(x); }";
int TEST(int a, int b) { return a + b; }
void helper() { EXPECT_TRUE(ready()); }
"#);
        assert!(!file.has_errors);
        assert!(file.tests.is_empty());
        // An assertion in a helper counts once, in no test.
        assert_eq!(shapes(&file), ["EXPECT_TRUE(ready())"]);
        assert_eq!(file.assertions[0].test, None);
    }

    #[test]
    fn googletest_skips() {
        let file = cpp(r#"
TEST(Cart, DISABLED_Adds) { EXPECT_EQ(add(1, 2), 3); }
TEST(DISABLED_Cart, Subtracts) { EXPECT_EQ(sub(3, 1), 2); }
TEST(Cart, Later) {
  GTEST_SKIP() << "not yet";
  EXPECT_EQ(mul(2, 2), 4);
}
TEST(Cart, Empty) {}
TEST(Cart, Returns) { return; EXPECT_EQ(div(4, 2), 2); }
TEST(Cart, SkipsWhenAsked) {
  if (no_network()) { GTEST_SKIP(); }
  EXPECT_TRUE(fetch());
}
class Slow : public ::testing::Test {
  void SetUp() override { GTEST_SKIP() << "slow"; }
};
TEST_F(Slow, Fetches) { EXPECT_TRUE(fetch()); }
TEST_F(Other, Fetches) { EXPECT_TRUE(fetch()); }
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("Cart::Adds", true, None, 1),
                ("Cart::Subtracts", true, None, 1),
                ("Cart::Later", true, None, 1),
                ("Cart::Empty", true, DOES_NOTHING, 1),
                ("Cart::Returns", true, DOES_NOTHING, 1),
                ("Cart::SkipsWhenAsked", false, None, 1),
                ("Slow::Fetches", true, None, 1),
                ("Other::Fetches", false, None, 1),
            ]
        );
        // After the skip and the return, nothing runs.
        assert_eq!(
            shapes(&file),
            [
                "EXPECT_EQ(add(1,2),3)",
                "EXPECT_EQ(sub(3,1),2)",
                "EXPECT_TRUE(fetch())",
                "EXPECT_TRUE(fetch())",
                "EXPECT_TRUE(fetch())",
            ]
        );
    }

    #[test]
    fn value_parameterized_tests_weigh_their_literal_rows() {
        let file = cpp(r#"
TEST_P(Prices, Positive) { EXPECT_GT(GetParam(), 0); }
INSTANTIATE_TEST_SUITE_P(Small, Prices, ::testing::Values(1, 2, 3));
INSTANTIATE_TEST_SUITE_P(Big, Prices, testing::Values(100));
TEST_P(Flags, Work) { EXPECT_TRUE(run(GetParam())); }
INSTANTIATE_TEST_SUITE_P(All, Flags, ::testing::Bool());
TEST_P(Ranges, Work) { EXPECT_TRUE(run(GetParam())); }
INSTANTIATE_TEST_SUITE_P(All, Ranges, ::testing::Values(1, 2));
INSTANTIATE_TEST_SUITE_P(More, Ranges, ::testing::Range(1, 10));
TEST_P(Elsewhere, Work) { EXPECT_TRUE(run(GetParam())); }
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("Prices::Positive", false, None, 4),
                ("Flags::Work", false, None, 2),
                ("Ranges::Work", false, None, 1),
                ("Elsewhere::Work", false, None, 1),
            ]
        );
        let weights: Vec<usize> = file.assertions.iter().map(|a| a.weight).collect();
        assert_eq!(weights, [4, 2, 1, 1]);
    }

    #[test]
    fn catch2_tests_sections_and_assertions_are_read() {
        let file = cpp(r#"#include <catch2/catch_test_macros.hpp>
#include "cart.hpp"

TEST_CASE("cart adds items", "[cart]") {
  Cart cart;
  REQUIRE(cart.empty());
  SECTION("one item") {
    cart.add(1);
    CHECK(cart.size() == 1);
    REQUIRE_THROWS_AS(cart.pop(2), std::out_of_range);
  }
  SECTION("two items") {
    CHECK_THAT(cart.name(), Catch::Matchers::StartsWith("c"));
  }
}

SCENARIO("checkout", "[cart]") {
  GIVEN("a cart") {
    WHEN("paid") {
      THEN("it is closed") { REQUIRE_FALSE(cart.open()); }
    }
  }
}

TEST_CASE_METHOD(CartFixture, "fixture test", "[cart]") {
  CHECKED_IF(ready) { CHECK_NOFAIL(maybe()); }
  WARN(soft());
  FAIL_CHECK("not done");
}
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("cart adds items", false, None, 1),
                ("checkout", false, None, 1),
                ("fixture test", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 4);
        assert!(file.tests[0].uses.contains("cart"));
        assert_eq!(
            shapes(&file),
            [
                "REQUIRE(cart.empty())",
                "CHECK(cart.size()==1)",
                "REQUIRE_THROWS_AS(cart.pop(2),std::out_of_range)",
                "CHECK_THAT(cart.name(),Catch::Matchers::StartsWith(\"c\"))",
                "REQUIRE_FALSE(cart.open())",
                "FAIL_CHECK(\"notdone\")",
            ]
        );
        let tests: Vec<Option<usize>> = file.assertions.iter().map(|a| a.test).collect();
        assert_eq!(
            tests,
            [Some(0), Some(0), Some(0), Some(0), Some(1), Some(2)]
        );
    }

    #[test]
    fn catch2_skips() {
        let file = cpp(r#"
TEST_CASE("hidden", "[.]") { REQUIRE(a()); }
TEST_CASE("hidden with a name", "[.integration][db]") { REQUIRE(a()); }
TEST_CASE("hidden old style", "[!hide]") { REQUIRE(a()); }
TEST_CASE("should fail", "[!shouldfail]") { REQUIRE(a()); }
TEST_CASE("may fail", "[!mayfail]") { REQUIRE(a()); }
TEST_CASE("./dot name") { REQUIRE(a()); }
TEST_CASE("skips", "[x]") {
  SKIP("later");
  REQUIRE(b());
}
TEST_CASE("empty") {}
TEST_CASE("tagged", "[fast]") { REQUIRE(a()); }
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("hidden", true, None, 1),
                ("hidden with a name", true, None, 1),
                ("hidden old style", true, None, 1),
                ("should fail", true, None, 1),
                ("may fail", false, None, 1),
                ("./dot name", true, None, 1),
                ("skips", true, None, 1),
                ("empty", true, DOES_NOTHING, 1),
                ("tagged", false, None, 1),
            ]
        );
        assert!(!shapes(&file).contains(&"REQUIRE(b())"));
    }

    #[test]
    fn catch2_template_tests_are_read_and_weighed() {
        let file = cpp(r#"
TEMPLATE_TEST_CASE("vectors", "[vec]", int, float, (std::map<int, int>)) {
  REQUIRE(std::vector<TestType>().empty());
}
TEMPLATE_PRODUCT_TEST_CASE("products", "[p]", (std::vector, std::list), (int, float)) {
  REQUIRE(TestType().empty());
}
TEST_CASE("after") { REQUIRE(ok()); }
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("vectors", false, None, 3),
                ("products", false, None, 1),
                ("after", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 2);
        assert_eq!(file.assertions.len(), 3);
        // Masking keeps lines.
        assert_eq!(file.assertions[2].line, 8);
    }

    #[test]
    fn doctest_tests_suites_and_decorators() {
        let file = cpp(r#"#include <doctest/doctest.h>
TEST_SUITE("math") {
  TEST_CASE("adds") { CHECK(add(1, 2) == 3); WARN(add(0, 0) == 0); }
  TEST_CASE("skipped" * doctest::skip()) { CHECK_EQ(sub(3, 1), 2); }
  TEST_CASE("not skipped" * doctest::skip(false)) { REQUIRE_UNARY(ok()); }
  TEST_CASE("must fail" * doctest::should_fail()) { CHECK(bad()); }
  TEST_CASE("may fail" * doctest::may_fail()) { CHECK(bad()); }
}
TEST_SUITE("off" * doctest::skip()) {
  TEST_CASE("inside") { CHECK(a()); }
}
TEST_CASE_TEMPLATE("typed", T, int, char) { CHECK(T() == T()); }
TEST_CASE_FIXTURE(Fixture, "fixtured") { CHECK(value == 1); }
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("math::adds", false, None, 1),
                ("math::skipped", true, None, 1),
                ("math::not skipped", false, None, 1),
                ("math::must fail", true, None, 1),
                ("math::may fail", false, None, 1),
                ("off::inside", true, None, 1),
                ("typed", false, None, 2),
                ("fixtured", false, None, 1),
            ]
        );
        // `WARN` does not fail a run.
        assert_eq!(shapes(&file)[0], "CHECK(add(1,2)==3)");
        assert!(!shapes(&file).iter().any(|s| s.starts_with("WARN")));
    }

    #[test]
    fn unity_tests_are_void_test_functions_in_a_file_that_includes_unity() {
        let file = c(r#"#include "unity.h"
#include "pricing.h"

void setUp(void) {}
void tearDown(void) {}

void test_total_adds(void) {
  TEST_ASSERT_EQUAL_INT(3, total(1, 2));
  TEST_ASSERT_TRUE_MESSAGE(total(0, 0) == 0, "zero");
}

static void test_total_ignored(void) {
  TEST_IGNORE_MESSAGE("later");
  TEST_ASSERT_EQUAL(4, total(2, 2));
}

void test_empty(void) {}

int test_helper(int x) { return x; }

void runs_by_name(void) { TEST_FAIL_MESSAGE("never"); }

int main(void) {
  UNITY_BEGIN();
  RUN_TEST(test_total_adds);
  RUN_TEST(runs_by_name);
  return UNITY_END();
}
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("test_total_adds", false, None, 1),
                ("test_total_ignored", true, None, 1),
                ("test_empty", true, DOES_NOTHING, 1),
                ("runs_by_name", false, None, 1),
            ]
        );
        assert_eq!(
            shapes(&file),
            [
                "TEST_ASSERT_EQUAL_INT(3,total(1,2))",
                "TEST_ASSERT_TRUE_MESSAGE(total(0,0)==0,\"zero\")",
                "TEST_FAIL_MESSAGE(\"never\")",
            ]
        );
        // Without `unity.h`, a function named `test_…` is not a test.
        let plain = c("void test_total(void) { assert(total(1) == 1); }\n");
        assert!(plain.tests.is_empty());
        assert_eq!(shapes(&plain), ["assert(total(1)==1)"]);
    }

    #[test]
    fn unity_fixture_tests() {
        let file = c(r#"#include "unity_fixture.h"
TEST_GROUP(Cart);
TEST_SETUP(Cart) {}
TEST(Cart, Adds) { TEST_ASSERT_EQUAL(3, add(1, 2)); }
IGNORE_TEST(Cart, Later) { TEST_ASSERT_EQUAL(4, add(2, 2)); }
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("Cart::Adds", false, None, 1),
                ("Cart::Later", true, None, 1)
            ]
        );
        assert_eq!(file.assertions[0].test, Some(0));
    }

    #[test]
    fn cmocka_tests_are_the_listed_functions() {
        let file = c(r#"#include <stdarg.h>
#include <stddef.h>
#include <setjmp.h>
#include <cmocka.h>

static void adds(void **state) {
  (void)state;
  assert_int_equal(total(1, 2), 3);
  assert_non_null(cart());
}

static void later(void **state) {
  skip();
  assert_true(ready());
}

static void empty(void **state) { (void)state; }

static void helper(void **state) { assert_true(1); }

int main(void) {
  const struct CMUnitTest tests[] = {
    cmocka_unit_test(adds),
    cmocka_unit_test(later),
    cmocka_unit_test_setup_teardown(empty, setup, teardown),
    cmocka_unit_test(defined_elsewhere),
  };
  return cmocka_run_group_tests(tests, NULL, NULL);
}
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("adds", false, None, 1),
                ("later", true, None, 1),
                ("empty", true, DOES_NOTHING, 1),
                ("defined_elsewhere", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 6);
        assert_eq!(file.tests[3].line, 26);
        assert_eq!(
            shapes(&file),
            ["assert_int_equal(total(1,2),3)", "assert_non_null(cart())"]
        );
    }

    #[test]
    fn check_tests_between_start_test_and_end_test() {
        let file = c(r#"#include <check.h>
#include "money.h"

START_TEST(test_money_create)
{
  ck_assert_int_eq(money_amount(m), 5);
  ck_assert_str_eq(money_currency(m), "USD");
}
END_TEST

START_TEST(test_money_empty) { } END_TEST

START_TEST(test_two)
{
  fail_unless(money_amount(m) == 5, "wrong");
  ck_abort_msg("unreachable");
}
END_TEST
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("test_money_create", false, None, 1),
                ("test_money_empty", true, DOES_NOTHING, 1),
                ("test_two", false, None, 1),
            ]
        );
        assert_eq!(file.tests[0].line, 4);
        assert_eq!(
            shapes(&file),
            [
                "ck_assert_int_eq(money_amount(m),5)",
                "ck_assert_str_eq(money_currency(m),\"USD\")",
                "fail_unless(money_amount(m)==5,\"wrong\")",
                "ck_abort_msg(\"unreachable\")",
            ]
        );
    }

    #[test]
    fn assertions_that_cannot_fail_are_not_counted() {
        let file = cpp(r#"
TEST(A, B) {
  EXPECT_TRUE(true);
  ASSERT_TRUE(1) << "always";
  EXPECT_FALSE(false);
  ASSERT_EQ(1, 1);
  EXPECT_STREQ("a", "a");
  REQUIRE(true);
  CHECK(1 == 1);
  REQUIRE_FALSE(0);
  CHECK_EQ(-1, -1);
  EXPECT_TRUE(x);
  EXPECT_EQ(x, 1);
  EXPECT_EQ(1, 2);
  ASSERT_TRUE(false);
  EXPECT_NE(1, 1);
  CHECK(2 > 1);
  REQUIRE(1 == 1 + y);
  FAIL() << "must not be reached";
  ADD_FAILURE();
  SUCCEED();
}
"#);
        assert_eq!(
            shapes(&file),
            [
                "EXPECT_TRUE(x)",
                "EXPECT_EQ(x,1)",
                "EXPECT_EQ(1,2)",
                "ASSERT_TRUE(false)",
                "EXPECT_NE(1,1)",
                "CHECK(2>1)",
                "REQUIRE(1==1+y)",
                "FAIL()",
                "ADD_FAILURE()",
            ]
        );
        let file = c(r#"
void t(void) {
  assert(1);
  assert(true);
  TEST_ASSERT_TRUE(1);
  TEST_ASSERT_EQUAL_INT(3, 3);
  TEST_ASSERT_FALSE(0);
  assert_true(1);
  assert_int_equal(2, 2);
  ck_assert_int_eq(4, 4);
  ck_assert(1);
  assert(0);
  assert(ptr);
  TEST_ASSERT_FLOAT_WITHIN(0.1, 0.1, value);
  TEST_ASSERT_NOT_EQUAL(1, 1);
}
"#);
        assert_eq!(
            shapes(&file),
            [
                "assert(0)",
                "assert(ptr)",
                "TEST_ASSERT_FLOAT_WITHIN(0.1,0.1,value)",
                "TEST_ASSERT_NOT_EQUAL(1,1)",
            ]
        );
    }

    #[test]
    fn code_that_never_runs_is_not_counted() {
        let file = cpp(r#"
TEST(A, AfterReturn) {
  int total = 3;
  return;
  EXPECT_EQ(total, 3);
}
TEST(A, InsideIfFalse) {
  if (false) { EXPECT_EQ(total(), 3); }
  if (0) { EXPECT_TRUE(a()); }
  while (false) { EXPECT_TRUE(b()); }
  if (true) { EXPECT_TRUE(c()); }
  auto f = [] { return; };
  EXPECT_TRUE(d());
}
#if 0
TEST(A, NeverCompiled) { EXPECT_TRUE(e()); }
#endif
#ifdef _WIN32
TEST(A, OnWindows) { EXPECT_TRUE(f()); }
#else
TEST(A, Elsewhere) { EXPECT_TRUE(g()); }
#endif
#if 1
TEST(A, Always) { EXPECT_TRUE(h()); }
#else
TEST(A, Never) { EXPECT_TRUE(i()); }
#endif
"#);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(
            ids(&file),
            [
                ("A::AfterReturn", false, None, 1),
                ("A::InsideIfFalse", false, None, 1),
                ("A::NeverCompiled", true, Some(NEVER_COMPILED), 1),
                ("A::OnWindows", true, None, 1),
                ("A::Elsewhere", true, None, 1),
                ("A::Always", false, None, 1),
                ("A::Never", true, Some(NEVER_COMPILED), 1),
            ]
        );
        assert_eq!(
            shapes(&file),
            [
                "EXPECT_TRUE(c())",
                "EXPECT_TRUE(d())",
                "EXPECT_TRUE(f())",
                "EXPECT_TRUE(g())",
                "EXPECT_TRUE(h())",
            ]
        );
        let file = c("#include \"unity.h\"\nvoid test_a(void) {\n  if (0) { TEST_ASSERT_TRUE(x); }\n  TEST_ASSERT_TRUE(y);\n}\n");
        assert_eq!(shapes(&file), ["TEST_ASSERT_TRUE(y)"]);
    }

    #[test]
    fn an_include_guard_is_not_a_condition() {
        let file = cpp("#ifndef CART_TEST_H\n#define CART_TEST_H\n#include <gtest/gtest.h>\nTEST(A, B) { EXPECT_TRUE(x); }\n#endif\n");
        assert_eq!(ids(&file), [("A::B", false, None, 1)]);
    }

    #[test]
    fn macro_heads_without_a_semicolon_are_not_errors_but_real_errors_are() {
        let file = cpp("DEFINE_FLAG(verbose)\nTEST_CASE(\"a\") { REQUIRE(x); }\n");
        assert!(!file.has_errors);
        assert_eq!(ids(&file), [("a", false, None, 1)]);
        let file = cpp("TEST(A, B) {\n  EXPECT_EQ(1, x);\n");
        assert!(file.has_errors);
        assert!(file.error_line.is_some());
        // An error that could hide an assertion is one (M32, ADR 0052)...
        assert!(cpp("TEST(ostream_test, template) {\n  EXPECT_EQ(1, x);\n}\n").has_errors);
        assert!(!cpp("").has_errors);
        // ...and one that cannot, or a missing `;`, is not: the tree around
        // it is whole.
        assert!(!c("void f(void) { int x = }\n").has_errors);
        assert!(!cpp("void f() { go() }\n").has_errors);
    }

    // fmt's test files open a namespace with a macro alone on its line.
    #[test]
    fn a_namespace_macro_alone_on_its_line_is_read_past() {
        let source = "struct test_struct {};\n\nFMT_BEGIN_NAMESPACE\ntemplate <typename Char> struct formatter<test_struct, Char> {\n  auto parse(int ctx) -> int { return ctx; }\n};\nFMT_END_NAMESPACE  // fmt\n\nTEST(core_test, format) {\n  EXPECT_EQ(fmt::format(\"{}\", test_struct()), \"test\");\n}\n";
        let file = cpp(source);
        assert!(!file.has_errors, "{:?}", file.error_line);
        assert_eq!(ids(&file), [("core_test::format", false, None, 1)]);
        assert_eq!(file.assertions.len(), 1);
        // Not inside parentheses, a macro's body, or beside other code.
        assert!(mask("f(\n  MY_CONST\n);\n").text.contains("MY_CONST"));
        assert!(!mask("G_BEGIN_DECLS\n").text.contains("G_BEGIN_DECLS"));
        assert_eq!(bare_macro_line(b"#define X \\\n  MY_CONST\n", 15, 23), None);
        assert_eq!(bare_macro_line(b"MY_CONST x;\n", 0, 8), None);
        assert_eq!(bare_macro_line(b"GTEST_SKIP_IF\n", 0, 13), None);
        assert_eq!(bare_macro_line(b"Mixed_Case\n", 0, 10), None);
        assert_eq!(bare_macro_line(b"MY_SUITE_BEGIN\n{\n", 0, 14), None);
        assert_eq!(bare_macro_line(b"MY_DEF(a,\n b)\n", 0, 6), None);
        assert_eq!(bare_macro_line(b"G_BEGIN_DECLS\n", 0, 13), Some(13));
        // fmt's pragma macros: the call goes too.
        let pragma = b"FMT_PRAGMA_GCC(diagnostic ignored \"-Wx\")  // why\n";
        assert_eq!(bare_macro_line(pragma, 0, 14), Some(40));
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let source = format!(
            "TEST(A, B) {{ {} EXPECT_TRUE(x); {} }}\n",
            "{ ".repeat(1_000),
            "} ".repeat(1_000)
        );
        let file = cpp(&source);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.assertions.len(), 1);
    }

    #[test]
    fn many_tests_read_in_linear_time() {
        let source: String = (0..4_000)
            .map(|i| format!("TEST(A, T{i}) {{ EXPECT_EQ(f({i}), {i}); }}\nTEST_CASE(\"c{i}\") {{ CHECK(g()); }}\n"))
            .collect();
        let started = std::time::Instant::now();
        let file = cpp(&source);
        assert_eq!(file.tests.len(), 8_000);
        assert_eq!(file.assertions.len(), 8_000);
        assert!(started.elapsed().as_secs() < 20);
    }

    #[test]
    fn masking_keeps_lines_and_skips_comments_and_strings() {
        let source = "// TEMPLATE_TEST_CASE(\"x\", \"y\", int, ( unclosed\nconst char* s = \"END_TEST\";\nEND_TEST\nTEMPLATE_TEST_CASE(\"v\", \"[v]\",\n  int,\n  float) {}\n";
        let masked = mask(source);
        assert_eq!(masked.text.len(), source.len());
        assert_eq!(masked.text.lines().count(), source.lines().count());
        assert!(masked
            .text
            .starts_with("// TEMPLATE_TEST_CASE(\"x\", \"y\", int"));
        assert!(masked.text.contains("\"END_TEST\""));
        assert!(!masked.text.contains("\nEND_TEST\n"));
        assert!(masked.text.contains("TEMPLATE_TEST_CASE(\"v\", \"[v]\""));
        assert!(!masked.text.contains("float"));
        assert_eq!(masked.rows.values().copied().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn headers_are_read_as_cpp() {
        assert_eq!(Lang::of("tests/helpers.h"), Some(Lang::Cpp));
        assert_eq!(Lang::of("tests/cart_test.hpp"), Some(Lang::Cpp));
        assert_eq!(Lang::of("src/cart.c"), Some(Lang::C));
        let file = read_test_file(
            "#include <gtest/gtest.h>\nTEST(A, B) { EXPECT_TRUE(x); }\n",
            Lang::Cpp,
        );
        assert_eq!(ids(&file), [("A::B", false, None, 1)]);
        // A C header with Unity tests reads the same with the C++ grammar.
        let file = read_test_file(
            "#include \"unity.h\"\nvoid test_a(void) { TEST_ASSERT_TRUE(x); }\n",
            Lang::Cpp,
        );
        assert_eq!(ids(&file), [("test_a", false, None, 1)]);
    }

    #[test]
    fn test_paths() {
        for path in [
            "tests/cart.c",
            "test/cart.cpp",
            "unittests/helpers.h",
            "src/unittest/a.cc",
            "Tests/a.cpp",
            "src/cart_test.cc",
            "src/cart_tests.cpp",
            "src/cart_unittest.cc",
            "src/test_cart.c",
            "src/CartTest.cpp",
            "src/CartTests.cpp",
            "src/cart_test.CPP",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "src/cart.c",
            "src/contest.cpp",
            "src/latest.cc",
            "src/Test.cpp",
            "src/test_.c",
            "src/_test.c",
            "src/cart_test.h",
            "src/CartTest.hpp",
            "testing/cart.cc",
            "tests/cart.py",
            "tests/.c",
            "tests",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn holds_tests_needs_a_framework_and_a_test() {
        for (source, cpp) in [
            ("#include <gtest/gtest.h>\nTEST(A, B) { EXPECT_TRUE(x); }\n", true),
            ("#include \"gmock/gmock.h\"\nTEST_F(A, B) { EXPECT_TRUE(x); }\n", true),
            ("#include <catch2/catch_test_macros.hpp>\nTEST_CASE(\"a\") { REQUIRE(x); }\n", true),
            ("#include \"catch.hpp\"\nSCENARIO(\"a\") { REQUIRE(x); }\n", true),
            ("#include <doctest/doctest.h>\nTEST_CASE(\"a\") { CHECK(x); }\n", true),
            ("#include \"unity.h\"\nvoid test_a(void) { TEST_ASSERT(x); }\n", false),
            ("#include <cmocka.h>\nstatic void a(void **s) { assert_true(x); }\nconst struct CMUnitTest t[] = { cmocka_unit_test(a) };\n", false),
            ("#include <check.h>\nSTART_TEST(a) { ck_assert(x); }\nEND_TEST\n", false),
        ] {
            assert!(holds_tests(source, cpp), "{source}");
        }
        for (source, cpp) in [
            (
                "#include <gtest/gtest_prod.h>\nclass A { FRIEND_TEST(A, B); };\n",
                true,
            ),
            (
                "#include <gtest/gtest.h>\nint main() { return RUN_ALL_TESTS(); }\n",
                true,
            ),
            ("TEST(A, B) { EXPECT_TRUE(x); }\n", true),
            ("// #include <gtest/gtest.h>\nint x;\n", true),
            ("#include <assert.h>\nvoid f(void) { assert(x); }\n", false),
        ] {
            assert!(!holds_tests(source, cpp), "{source}");
        }
    }

    #[test]
    fn quoted_includes_resolve_to_repo_files() {
        let files = [
            "src/pricing.h",
            "include/lib/cart.h",
            "lib/foo/include/foo/money.h",
            "tests/helpers.h",
        ];
        let exists = |path: &str| files.contains(&path);
        let found = |from: &str, spec: &str| resolve(from, spec, &exists);
        assert_eq!(
            found("tests/pricing_test.cc", "../src/pricing.h").as_deref(),
            Some("src/pricing.h")
        );
        assert_eq!(
            found("tests/pricing_test.cc", "pricing.h").as_deref(),
            Some("src/pricing.h")
        );
        assert_eq!(
            found("tests/cart_test.cc", "lib/cart.h").as_deref(),
            Some("include/lib/cart.h")
        );
        assert_eq!(
            found("lib/foo/tests/money_test.cc", "foo/money.h").as_deref(),
            Some("lib/foo/include/foo/money.h")
        );
        assert_eq!(
            found("tests/a_test.cc", "helpers.h").as_deref(),
            Some("tests/helpers.h")
        );
        assert_eq!(found("tests/a_test.cc", "../../outside.h"), None);
        assert_eq!(found("tests/a_test.cc", "/usr/include/x.h"), None);
        assert_eq!(found("tests/a_test.cc", "missing.h"), None);
    }

    #[test]
    fn suppressions_in_comments_and_pragmas() {
        let source = r#"
#include "legacy.h"  // NOLINT(build/include)
int x = f(); // NOLINT
int y = g(); // NOLINT(bugprone-narrowing, cert-err34-c)
// NOLINTNEXTLINE(readability-magic-numbers)
int z = 42;
/* NOLINTBEGIN(google-runtime-int) */
long a;
// NOLINTEND(google-runtime-int)
// NOLINTBEGIN
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-variable"
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
#pragma GCC diagnostic pop
#pragma warning(disable: 4996 4100)
#pragma warning( push )
#pragma warning(disable : 4200; error : 4201)
#pragma warning(suppress: 6011)
#pragma once
// cppcheck-suppress nullPointer
int* p = 0;
// cppcheck-suppress[uninitvar, unusedVariable] why
// cppcheck-suppress-file missingInclude
void f() {
  #pragma GCC diagnostic ignored "-Wshadow"
  int q; // NOLINT(misc-unused)
}
"#;
        assert_eq!(
            keys(source, true),
            [
                "nolint:build/include",
                "nolint:*",
                "nolint:bugprone-narrowing",
                "nolint:cert-err34-c",
                "nolint:readability-magic-numbers",
                "nolint-range:google-runtime-int",
                "nolint-range:*",
                "pragma-ignored:-Wunused-variable",
                "pragma-ignored:-Wdeprecated-declarations",
                "msvc-disable:4996",
                "msvc-disable:4100",
                "msvc-disable:4200",
                "msvc-disable:6011",
                "cppcheck:nullPointer",
                "cppcheck:uninitvar",
                "cppcheck:unusedVariable",
                "cppcheck-range:missingInclude",
                "pragma-ignored:-Wshadow",
                "nolint:misc-unused",
            ]
        );
        let found = suppressions(source, true);
        assert_eq!(found[1].line, 3);
        assert_eq!(found[1].text, "// NOLINT");
        // The C grammar reads the same.
        assert_eq!(
            keys(
                "int x; // NOLINT(a)\n#pragma GCC diagnostic ignored \"-Wformat\"\n",
                false
            ),
            ["nolint:a", "pragma-ignored:-Wformat"]
        );
    }

    #[test]
    fn look_alikes_are_not_suppressions() {
        let source = r#"
const char* a = "// NOLINT";
const char* b = R"(// NOLINT(x) #pragma GCC diagnostic ignored "-Wall")";
// We do not use NOLINT here.
// NOLINTED is not a directive
// see cppcheck-suppress docs
// cppcheck-suppressions are documented
#pragma GCC diagnostic warning "-Wall"
#pragma GCC diagnostic error "-Wall"
#pragma message("NOLINT")
#define NOLINT_ME 1
"#;
        assert!(keys(source, true).is_empty(), "{:?}", keys(source, true));
        assert!(keys("const char *s = \"// NOLINT\";\n", false).is_empty());
    }
}
