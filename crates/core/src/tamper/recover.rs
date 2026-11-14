//! Parse errors that cannot hide a test (M32, ADR 0052).
//!
//! tree-sitter reads code as written: it does not expand C macros
//! (`int CJSON_CDECL main(void)`, `FMT_BEGIN_NAMESPACE`), and it cannot read
//! every `#if` placed inside an expression. Where it cannot read something it
//! recovers: it wraps what it could not read in a small error node, or puts
//! in a token that is missing, and reads the rest of the file as usual.
//!
//! A file with an error was "not checked" (ADR 0042). In real C, C++, Swift
//! and C# test files that made most changes to some files unreadable (fmt:
//! 66 of its 68 findings). An error is now harmless, and the file is read,
//! when it cannot hide a test, a skip or an assertion:
//!
//! - an error node none of whose words could name one (see [`could_name`]);
//! - a missing token other than a closing `}`, `)` or `]`: a `;` after a
//!   macro call, Swift's `!` in some `!=`. The tree around it is whole. A
//!   missing closing bracket is code cut off or not balanced, which macros
//!   do not cause: the file is not read.

use tree_sitter::Node;

use crate::imports::walk;

/// Parts of words, in any letter case, that a test, a skip or an assertion
/// is named by in some framework: `TEST_F`, `@Test`, `testFoo`, `[Fact]`,
/// `[Theory]`, `XCTAssertEqual`, `#expect`, `REQUIRE`, `CHECK`, `ck_assert`,
/// `shouldBe`, `t.Errorf`, `t.Fatal`, `GTEST_SKIP`, `[Ignore]`, `@Disabled`,
/// Catch2's `SECTION` and `SCENARIO`, doctest's `SUBCASE`, `verify`.
/// Generous on purpose: a word found here only keeps a file "not checked",
/// as before.
const PARTS: &[&str] = &[
    "test", "assert", "expect", "check", "require", "verify", "should", "must", "ensure", "skip",
    "ignore", "disable", "pending", "fail", "fatal", "error", "fact", "theory", "suite", "section",
    "subcase", "scenario", "xct",
];

/// Whole words that do the same: `it(…)`, `describe`, Go's `t.Run`,
/// Catch2's `GIVEN`, `WHEN` and `THEN`.
const WORDS: &[&str] = &[
    "it", "describe", "context", "run", "t", "s", "given", "when", "then",
];

/// Whether a word could name a test, a skip or an assertion.
pub(super) fn could_name(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    WORDS.contains(&lower.as_str()) || PARTS.iter().any(|part| lower.contains(part))
}

/// Whether any word of `text` could name a test, a skip or an assertion.
fn names_any(text: &[u8]) -> bool {
    text.split(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .filter(|word| !word.is_empty())
        .any(|word| std::str::from_utf8(word).is_ok_and(could_name))
}

/// The line of the first parse error that could hide a test, a skip or an
/// assertion, or `None` when every error is harmless (or there is none).
pub(super) fn first_harmful(root: Node<'_>, src: &[u8]) -> Option<usize> {
    if !root.has_error() {
        return None;
    }
    let mut found = None;
    walk(root, |node| {
        if found.is_some() {
            return false;
        }
        if node.is_missing() {
            if matches!(node.kind(), "}" | ")" | "]") {
                found = Some(node.start_position().row + 1);
            }
            return false;
        }
        if node.is_error() {
            let text = src
                .get(node.start_byte()..node.end_byte())
                .unwrap_or_default();
            if names_any(text) {
                found = Some(node.start_position().row + 1);
            }
            return false;
        }
        node.has_error()
    });
    found
}

/// The source with only the first branch of each `#if` kept: every
/// `#elif`, `#elseif` and `#else` branch blanked with spaces, line breaks
/// kept so lines do not move. For a file tree-sitter cannot read because an
/// `#if` gives one block two heads (C#'s `#if NET5_0` … `#else` around two
/// `foreach` lines), as a compiler with one set of symbols would read it.
/// `None` when there is no such branch, or when a branch left out names a
/// test, a skip or an assertion: then the file stays "not checked".
pub(super) fn first_branches(source: &str) -> Option<String> {
    let mut out = String::with_capacity(source.len());
    let mut left_out = String::new();
    // For each open `#if`: whether its current branch is kept.
    let mut open: Vec<bool> = Vec::new();
    for line in source.split_inclusive('\n') {
        let directive = line
            .trim_start()
            .strip_prefix('#')
            .map(|rest| {
                rest.trim_start()
                    .split(|c: char| !c.is_ascii_alphabetic())
                    .next()
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let blank = match directive {
            "if" | "ifdef" | "ifndef" => {
                let blank = open.contains(&false);
                open.push(true);
                blank
            }
            "elif" | "elifdef" | "elifndef" | "elseif" | "else" => {
                if let Some(kept) = open.last_mut() {
                    *kept = false;
                }
                true
            }
            "endif" => {
                open.pop();
                open.contains(&false)
            }
            _ => open.contains(&false),
        };
        if blank {
            left_out.push_str(line);
            out.extend(line.chars().map(|c| if c == '\n' { '\n' } else { ' ' }));
        } else {
            out.push_str(line);
        }
    }
    let names_nothing = !names_any(
        left_out
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    );
    (!left_out.is_empty() && names_nothing).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imports::parse;
    use tree_sitter::Language;

    fn harmful(source: &str, language: Language) -> Option<usize> {
        let tree = parse(source, &language).expect("parses");
        first_harmful(tree.root_node(), source.as_bytes())
    }

    fn c(source: &str) -> Option<usize> {
        harmful(source, tree_sitter_c::LANGUAGE.into())
    }

    fn cpp(source: &str) -> Option<usize> {
        harmful(source, tree_sitter_cpp::LANGUAGE.into())
    }

    #[test]
    fn words_that_could_name_a_test() {
        for word in [
            "TEST_F",
            "Test",
            "testAdds",
            "Fact",
            "Theory",
            "XCTAssertEqual",
            "expect",
            "REQUIRE",
            "CHECK",
            "ck_assert_int_eq",
            "shouldBe",
            "Errorf",
            "Fatal",
            "GTEST_SKIP",
            "Ignore",
            "Disabled",
            "SECTION",
            "it",
            "Run",
            "t",
        ] {
            assert!(could_name(word), "{word}");
        }
        for word in [
            "CJSON_CDECL",
            "FMT_BEGIN_NAMESPACE",
            "template",
            "size_t",
            "UNITY_INT8",
            "signed",
            "char",
            "NET48",
            "eu",
            "SPECIALIZE_MAKE_SIGNED",
        ] {
            assert!(!could_name(word), "{word}");
        }
    }

    // cJSON's calling-convention macro: one error node around the macro.
    #[test]
    fn a_macro_before_a_function_name_is_harmless() {
        let source = "int CJSON_CDECL main(void)\n{\n    UNITY_BEGIN();\n    RUN_TEST(t);\n    return UNITY_END();\n}\n";
        let tree = parse(source, &tree_sitter_c::LANGUAGE.into()).expect("parses");
        assert!(tree.root_node().has_error());
        assert_eq!(c(source), None);
    }

    // fmt's `SPECIALIZE_MAKE_SIGNED(char, signed char);` at file level.
    #[test]
    fn a_macro_with_type_arguments_is_harmless() {
        let source = "SPECIALIZE_MAKE_SIGNED(char, signed char);\nSPECIALIZE_MAKE_SIGNED(unsigned, int);\n\nTEST(printf_test, length) {\n  EXPECT_EQ(1, 1);\n}\n";
        let tree = parse(source, &tree_sitter_cpp::LANGUAGE.into()).expect("parses");
        assert!(tree.root_node().has_error());
        assert_eq!(cpp(source), None);
    }

    // A foreach macro: `cJSON_ArrayForEach(x, list) { … }` misses a `;`.
    #[test]
    fn a_missing_semicolon_is_harmless() {
        let source = "void test_each(void)\n{\n    cJSON_ArrayForEach(item, list)\n    {\n        TEST_ASSERT_TRUE(item != NULL);\n    }\n}\n";
        assert_eq!(c(source), None);
    }

    // fmt's `TEST(ostream_test, template)`: the error is around the test.
    #[test]
    fn an_error_around_a_test_is_harmful() {
        let source =
            "TEST(ostream_test, template) {\n  EXPECT_EQ(\"2\", fmt::format(\"{}\", 2));\n}\n";
        assert_eq!(cpp(source), Some(1));
    }

    #[test]
    fn a_missing_closing_brace_is_harmful() {
        let source = "void test_open(void) {\n    TEST_ASSERT_TRUE(1);\n";
        assert!(c(source).is_some());
    }

    #[test]
    fn first_branches_keeps_lines_and_leaves_out_the_others() {
        let source = "a\n#if NET5_0\nb\n#else\nc\n#endif\nd\n";
        assert_eq!(
            first_branches(source).as_deref(),
            Some("a\n#if NET5_0\nb\n     \n \n#endif\nd\n")
        );
        // Nested, and Swift's `#elseif`.
        let source = "#if os(iOS)\n#if A\nx\n#else\ny\n#endif\n#elseif os(macOS)\nz\n#endif\n";
        let kept = first_branches(source).expect("has branches");
        assert_eq!(
            kept.lines().map(str::trim).collect::<Vec<_>>(),
            [
                "#if os(iOS)",
                "#if A",
                "x",
                "",
                "",
                "#endif",
                "",
                "",
                "#endif"
            ]
        );
        // No other branch, or one that could hold a test: none.
        assert_eq!(first_branches("#if A\nx\n#endif\n"), None);
        assert_eq!(
            first_branches("#if A\nx\n#else\nTEST(a, b) {}\n#endif\n"),
            None
        );
    }

    #[test]
    fn a_file_with_no_error_has_none() {
        assert_eq!(c("void test_a(void) { TEST_ASSERT_TRUE(1); }\n"), None);
        assert_eq!(c(""), None);
    }
}
