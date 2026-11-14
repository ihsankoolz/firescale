//! Lint suppressions the tamper guard counts (M24, ADR 0044): comments that
//! turn a linter or type checker off (`// eslint-disable-next-line`,
//! `// @ts-ignore`, `# noqa`, `# type: ignore`, `# pylint: disable=…`) and
//! Rust's `#[allow(…)]` and `#[expect(…)]` attributes.
//!
//! They are read from syntax trees: a directive written in a string, or in
//! prose in the middle of a comment, is not one. Each suppression has a key
//! made of its tool, its reach and its rule (`eslint:no-console`,
//! `noqa:E501`, `allow:dead_code`), and the guard compares how many of each
//! key the change's files hold on the base and now. Coverage and formatting
//! pragmas (`istanbul ignore`, `pragma: no cover`, `prettier-ignore`,
//! `fmt: off`) are not lint suppressions and are not counted.

use tree_sitter::Node;

use super::Language;
use crate::imports::{parse, text, walk, JsDialect};

/// One suppression in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    /// Tool, reach and rule: `eslint:no-console`, `eslint-range:*`,
    /// `ts:ignore`, `noqa:E501`, `type-ignore:*`, `allow:dead_code`,
    /// `allow-module:clippy::pedantic`. `*` is every rule.
    pub key: String,
    /// 1-based line.
    pub line: usize,
    /// The comment or attribute, white space squeezed, at most 120
    /// characters.
    pub text: String,
}

/// The suppressions in a source file.
#[must_use]
pub fn suppressions(source: &str, language: Language) -> Vec<Suppression> {
    // The languages of M32 read their own (ADR 0052).
    match language {
        Language::Java => return super::java::suppressions(source),
        Language::Kotlin => return super::kotlin::suppressions(source),
        Language::Swift => return super::swift::suppressions(source),
        Language::C => return super::cfamily::suppressions(source, false),
        Language::Cpp => return super::cfamily::suppressions(source, true),
        Language::CSharp => return super::csharp::suppressions(source),
        Language::Go => return super::go::suppressions(source),
        Language::Dart => return super::dart::suppressions(source),
        Language::Js(_) | Language::Python | Language::Rust => {}
    }
    let grammar: tree_sitter::Language = match language {
        Language::Js(JsDialect::TypeScript) => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Js(JsDialect::Tsx) => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        _ => return Vec::new(),
    };
    let Some(tree) = parse(source, &grammar) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    // Python: a comment before the module's first statement (docstrings and
    // imports are statements) reaches the whole module.
    let first_statement = {
        let mut cursor = root.walk();
        let first = root
            .named_children(&mut cursor)
            .find(|child| !child.is_extra() && child.kind() != "comment")
            .map_or(usize::MAX, |child| child.start_byte());
        first
    };
    // Rust: the item each outer attribute applies to, found once per list
    // of items: `#[allow(…)]` on a `mod` reaches the whole module.
    let mut applies_to_module: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut found = Vec::new();
    walk(root, |node| {
        if language == Language::Rust {
            let mut pending: Vec<usize> = Vec::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    "attribute_item" => pending.push(child.id()),
                    "line_comment" | "block_comment" => {}
                    kind => {
                        if kind == "mod_item" {
                            applies_to_module.extend(pending.iter().copied());
                        }
                        pending.clear();
                    }
                }
            }
        }
        let line = node.start_position().row + 1;
        let keys = match (language, node.kind()) {
            (Language::Js(_), "comment") => js_comment(text(node, src)),
            (Language::Python, "comment") => {
                python_comment(text(node, src), node.start_byte() < first_statement)
            }
            (Language::Rust, "attribute_item") => {
                let tool = if applies_to_module.contains(&node.id()) {
                    "allow-module"
                } else {
                    "allow"
                };
                rust_attribute(node, src, tool)
            }
            (Language::Rust, "inner_attribute_item") => rust_attribute(node, src, "allow-module"),
            _ => return true,
        };
        for key in keys {
            found.push(Suppression {
                key,
                line,
                text: shown(text(node, src)),
            });
        }
        false
    });
    found
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

/// `rest` when `body` starts with `word` and the word ends there.
fn directive<'a>(body: &'a str, word: &str) -> Option<&'a str> {
    let rest = body.strip_prefix(word)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_alphanumeric() || c == '-' || c == '_' => None,
        Some(_) => Some(rest),
    }
}

/// `rest` when `body` starts with `word`, in any letter case, and the word
/// ends there.
fn directive_nocase<'a>(body: &'a str, word: &str) -> Option<&'a str> {
    let head = body.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    directive(&body[word.len()..], "")
}

/// Keys of `tool:rule` for each rule in a list, or `tool:*` for none.
fn keyed(tool: &str, rules: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut keys: Vec<String> = rules
        .into_iter()
        .map(|rule| rule.trim().to_owned())
        .filter(|rule| !rule.is_empty())
        .map(|rule| format!("{tool}:{rule}"))
        .collect();
    if keys.is_empty() {
        keys.push(format!("{tool}:*"));
    }
    keys
}

/// ESLint-style rule lists: `a, b -- why`.
fn comma_rules(rest: &str) -> Vec<String> {
    let rules = rest.split(" --").next().unwrap_or_default();
    rules.split(',').map(str::to_owned).collect()
}

/// The suppressions a JS/TS comment holds: ESLint and oxlint
/// `*-disable…`, TypeScript's `@ts-ignore`, `@ts-expect-error` and
/// `@ts-nocheck`, Biome's `biome-ignore…`, Deno's `deno-lint-ignore…`. The
/// directive must open the comment, as each tool requires.
fn js_comment(raw: &str) -> Vec<String> {
    let body = raw
        .strip_prefix("//")
        .or_else(|| {
            raw.strip_prefix("/*")
                .map(|inner| inner.strip_suffix("*/").unwrap_or(inner))
        })
        .unwrap_or(raw);
    let body = body.trim_start_matches(['*', '!']).trim();
    for tool in ["eslint", "oxlint"] {
        for (form, reach) in [
            ("-disable-next-line", ""),
            ("-disable-line", ""),
            ("-disable", "-range"),
        ] {
            if let Some(rest) = directive(body, &format!("{tool}{form}")) {
                return keyed(&format!("{tool}{reach}"), comma_rules(rest));
            }
        }
    }
    // `/* eslint no-console: 0, "no-alert": "off" */`: rules configured
    // in the file; those set to off are turned off for the rest of it.
    if let Some(rest) = directive(body, "eslint") {
        return eslint_settings(rest);
    }
    if directive(body, "@ts-ignore").is_some() || directive(body, "@ts-expect-error").is_some() {
        return vec!["ts:ignore".to_owned()];
    }
    if directive(body, "@ts-nocheck").is_some() {
        return vec!["ts:nocheck".to_owned()];
    }
    for (form, tool) in [
        ("biome-ignore-all", "biome-range"),
        ("biome-ignore-start", "biome-range"),
        ("biome-ignore", "biome"),
    ] {
        if let Some(rest) = directive(body, form) {
            let rule = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_end_matches(':');
            return keyed(tool, [rule.to_owned()]);
        }
    }
    for (form, tool) in [
        ("deno-lint-ignore-file", "deno-range"),
        ("deno-lint-ignore", "deno"),
    ] {
        if let Some(rest) = directive(body, form) {
            return keyed(tool, rest.split_whitespace().map(str::to_owned));
        }
    }
    Vec::new()
}

/// The rules an inline ESLint configuration turns off: `rule: 0`,
/// `rule: "off"` or `rule: ["off", …]`, split at the commas outside
/// brackets and strings.
fn eslint_settings(rest: &str) -> Vec<String> {
    let mut settings: Vec<String> = vec![String::new()];
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in rest.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{' | '(') => depth += 1,
            (None, ']' | '}' | ')') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                settings.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(setting) = settings.last_mut() {
            setting.push(c);
        }
    }
    settings
        .iter()
        .filter_map(|setting| setting.split_once(':'))
        .filter(|(_, value)| {
            let value = value.trim().trim_start_matches('[').trim_start();
            value.starts_with('0') || value.starts_with("\"off\"") || value.starts_with("'off'")
        })
        .map(|(rule, _)| format!("eslint-range:{}", rule.trim().trim_matches(['"', '\''])))
        .collect()
}

/// Codes after `noqa:` or in `[…]`: split at commas and white space.
fn codes(list: &str) -> Vec<String> {
    list.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::to_owned)
        .collect()
}

/// The suppressions a Python comment holds. A comment may hold several,
/// each opening one `#` part: `# type: ignore[attr-defined]  # noqa: E501`.
fn python_comment(raw: &str, module_top: bool) -> Vec<String> {
    let mut keys = Vec::new();
    for part in raw.split('#').map(str::trim) {
        if let Some(rest) = directive_nocase(part, "noqa") {
            keys.extend(noqa("noqa", rest));
        } else if let Some(rest) = ["ruff:", "flake8:"]
            .iter()
            .find_map(|tool| part.strip_prefix(tool))
            .and_then(|rest| directive_nocase(rest.trim_start(), "noqa"))
        {
            keys.extend(noqa("noqa-file", rest));
        } else if let Some(rest) = part
            .strip_prefix("type:")
            .and_then(|rest| directive(rest.trim_start(), "ignore"))
        {
            // At the top of a module, mypy ignores the whole module.
            keys.extend(bracketed(
                if module_top {
                    "type-ignore-file"
                } else {
                    "type-ignore"
                },
                rest,
            ));
        } else if let Some(rest) = part.strip_prefix("pyright:").map(str::trim_start) {
            if let Some(rest) = directive(rest, "ignore") {
                keys.extend(bracketed("pyright", rest));
            } else {
                // `# pyright: reportOptionalMemberAccess=false`: a rule
                // turned off for the file.
                keys.extend(
                    rest.split(',')
                        .filter_map(|setting| setting.split_once('='))
                        .filter(|(_, value)| matches!(value.trim(), "false" | "none" | "\"none\""))
                        .map(|(rule, _)| format!("pyright-file:{}", rule.trim())),
                );
            }
        } else if let Some(rest) = part.strip_prefix("mypy:").map(str::trim_start) {
            if directive(rest, "ignore-errors").is_some() {
                keys.push("mypy-file:*".to_owned());
            } else if let Some(list) = rest.strip_prefix("disable-error-code") {
                let list = list.trim_start().trim_start_matches('=').trim();
                keys.extend(keyed("mypy-file", codes(list.trim_matches(['"', '\'']))));
            }
        } else if let Some(rest) = part.strip_prefix("pylint:").map(str::trim_start) {
            if directive(rest, "skip-file").is_some() {
                keys.push("pylint-file:*".to_owned());
                continue;
            }
            let list = ["disable-next=", "disable="]
                .iter()
                .find_map(|form| rest.strip_prefix(form));
            if let Some(list) = list {
                keys.extend(keyed("pylint", codes(list)));
            }
        } else if let Some(rest) = directive(part, "nosec") {
            keys.extend(keyed("nosec", codes(rest)));
        }
    }
    keys
}

/// `noqa`, `noqa: E501, F401` or `noqa:E501`.
fn noqa(tool: &str, rest: &str) -> Vec<String> {
    match rest.trim_start().strip_prefix(':') {
        Some(list) => keyed(tool, codes(list)),
        None => keyed(tool, Vec::new()),
    }
}

/// `ignore` or `ignore[code, other]`.
fn bracketed(tool: &str, rest: &str) -> Vec<String> {
    let inside = rest
        .trim_start()
        .strip_prefix('[')
        .and_then(|list| list.split(']').next());
    keyed(tool, inside.map(codes).unwrap_or_default())
}

/// The lints a Rust attribute allows: `#[allow(a, clippy::b)]`,
/// `#[expect(a)]`, and the same inside `#[cfg_attr(…, allow(…))]`.
/// `reason = "…"` is not a lint.
fn rust_attribute(item: Node<'_>, src: &[u8], tool: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut cursor = item.walk();
    for attribute in item
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "attribute")
    {
        let mut pairs: Vec<(Node<'_>, Node<'_>)> = Vec::new();
        // The attribute's own path and arguments, then each `allow(…)`
        // inside a `cfg_attr`'s arguments.
        if let (Some(path), Some(arguments)) = (
            attribute.named_child(0),
            attribute.child_by_field_name("arguments"),
        ) {
            if text(path, src) == "cfg_attr" {
                let mut inner = arguments.walk();
                let tokens: Vec<Node<'_>> = arguments.children(&mut inner).collect();
                for window in tokens.windows(2) {
                    if window[0].kind() == "identifier" && window[1].kind() == "token_tree" {
                        pairs.push((window[0], window[1]));
                    }
                }
            } else {
                pairs.push((path, arguments));
            }
        }
        for (name, arguments) in pairs {
            if !matches!(text(name, src), "allow" | "expect") {
                continue;
            }
            let list = text(arguments, src);
            let list = list
                .strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .unwrap_or(list);
            let lints: Vec<String> = list
                .split(',')
                .filter(|lint| !lint.contains('='))
                .map(|lint| lint.split_whitespace().collect::<String>())
                .collect();
            keys.extend(keyed(tool, lints));
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(source: &str, language: Language) -> Vec<String> {
        suppressions(source, language)
            .into_iter()
            .map(|s| s.key)
            .collect()
    }

    const TS: Language = Language::Js(JsDialect::TypeScript);

    #[test]
    fn javascript_comments_that_turn_a_check_off() {
        let source = r##"
// eslint-disable-next-line no-console, no-alert -- debugging
console.log(1); // eslint-disable-line no-console
/* eslint-disable */
/* eslint-disable no-undef */
// @ts-ignore
// @ts-expect-error: wrong type on purpose
// @ts-nocheck
// biome-ignore lint/suspicious/noExplicitAny: legacy
// deno-lint-ignore no-explicit-any ban-types
// oxlint-disable-next-line no-debugger
"##;
        assert_eq!(
            keys(source, TS),
            [
                "eslint:no-console",
                "eslint:no-alert",
                "eslint:no-console",
                "eslint-range:*",
                "eslint-range:no-undef",
                "ts:ignore",
                "ts:ignore",
                "ts:nocheck",
                "biome:lint/suspicious/noExplicitAny",
                "deno:no-explicit-any",
                "deno:ban-types",
                "oxlint:no-debugger",
            ]
        );
    }

    #[test]
    fn javascript_look_alikes_are_not_suppressions() {
        let source = r##"
// We do not use eslint-disable-next-line here.
// eslint-disabled is not a directive
// eslint-enable no-console
const s = "// eslint-disable-next-line no-console";
/* istanbul ignore next */
// prettier-ignore
// c8 ignore next
"##;
        assert!(keys(source, TS).is_empty());
    }

    #[test]
    fn python_comments_that_turn_a_check_off() {
        let source = r##"
import os  # noqa: F401, E501
x = 1  # NOQA
y: int = "a"  # type: ignore[assignment]  # noqa
z = f()  # type: ignore
w = g()  # pyright: ignore[reportGeneralTypeIssues]
# pylint: disable=unused-import,too-many-locals
# pylint: disable-next=broad-except
# ruff: noqa: E501
# flake8: noqa
# mypy: ignore-errors
# mypy: disable-error-code="attr-defined"
subprocess.call(cmd, shell=True)  # nosec B602
"##;
        assert_eq!(
            keys(source, Language::Python),
            [
                "noqa:F401",
                "noqa:E501",
                "noqa:*",
                "type-ignore:assignment",
                "noqa:*",
                "type-ignore:*",
                "pyright:reportGeneralTypeIssues",
                "pylint:unused-import",
                "pylint:too-many-locals",
                "pylint:broad-except",
                "noqa-file:E501",
                "noqa-file:*",
                "mypy-file:*",
                "mypy-file:attr-defined",
                "nosec:B602",
            ]
        );
    }

    #[test]
    fn python_look_alikes_are_not_suppressions() {
        let source = r##"
# Do not add noqa here: fix the import.
# noqanything else
s = "# noqa: E501"
# pragma: no cover
# fmt: off
# type: int
"##;
        assert!(keys(source, Language::Python).is_empty());
    }

    #[test]
    fn rust_allow_and_expect_attributes() {
        let source = r##"
#![allow(clippy::pedantic)]
#[allow(dead_code, clippy::too_many_lines)]
fn a() {}
#[expect(unused_variables, reason = "kept for the docs")]
fn b() {}
#[cfg_attr(test, allow(clippy::unwrap_used))]
fn c() {}
#[deny(warnings)]
#[rustfmt::skip]
#[derive(Debug)]
struct S;
// #[allow(dead_code)]
const T: &str = "#[allow(dead_code)]";
"##;
        assert_eq!(
            keys(source, Language::Rust),
            [
                "allow-module:clippy::pedantic",
                "allow:dead_code",
                "allow:clippy::too_many_lines",
                "allow:unused_variables",
                "allow:clippy::unwrap_used",
            ]
        );
    }

    #[test]
    fn wider_forms_have_their_own_keys() {
        // A `# type: ignore` before the first statement reaches the module.
        let python = "# type: ignore\n\"\"\"Docs.\"\"\"\nx = f()  # type: ignore\n# pylint: skip-file\n# pyright: reportOptionalMemberAccess=false, strict=true\n";
        assert_eq!(
            keys(python, Language::Python),
            [
                "type-ignore-file:*",
                "type-ignore:*",
                "pylint-file:*",
                "pyright-file:reportOptionalMemberAccess",
            ]
        );
        // `#[allow]` on a `mod` reaches the module; on a fn, the fn.
        let rust = "#[allow(clippy::unwrap_used)]\n// big\npub mod big;\n#[allow(dead_code)]\n#[inline]\nfn f() {}\n";
        assert_eq!(
            keys(rust, Language::Rust),
            ["allow-module:clippy::unwrap_used", "allow:dead_code"]
        );
        // An inline ESLint configuration that turns rules off.
        let js = r#"/* eslint no-console: 0, "no-alert": "off", eqeqeq: ["off", "smart"], semi: ["error", "always"], curly: 2 */
// eslint-env node
// eslint is how we lint: see the docs
"#;
        assert_eq!(
            keys(js, TS),
            [
                "eslint-range:no-console",
                "eslint-range:no-alert",
                "eslint-range:eqeqeq",
            ]
        );
    }

    #[test]
    fn a_long_comment_is_shown_cut() {
        let long = format!(
            "// eslint-disable-next-line no-console -- {}",
            "x".repeat(300)
        );
        let found = suppressions(&long, TS);
        assert_eq!(found[0].text.chars().count(), 120);
        assert!(found[0].text.ends_with('…'));
    }
}
