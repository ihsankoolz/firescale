//! Import statements read from Python and JavaScript/TypeScript source with
//! tree-sitter (ADR 0033).
//!
//! The project graph needs to know which module a file imports. A regular
//! expression would count `import x` written inside a string, a comment or a
//! docstring, so each file is parsed with a real grammar and only syntax
//! nodes are read:
//!
//! - Python: `import_statement` and `import_from_statement` anywhere in the
//!   tree (`from __future__ import x` is its own node kind and is skipped),
//!   plus `call` nodes that import a module chosen at run time.
//! - JS/TS: `import_statement` (with an `import_require_clause` child for TS
//!   `import x = require("x")`), `export_statement` with a `source` field,
//!   and `call_expression` whose function is `import` or the identifier
//!   `require`. The TSX grammar also reads `.js`, `.jsx`, `.mjs` and `.cjs`.
//!
//! A file with syntax errors still gives what tree-sitter could read, with
//! `has_errors` set. Nothing here panics on any input.

use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

/// The longest `text` kept for an import, in characters, `…` included.
const MAX_TEXT_CHARS: usize = 120;

/// One static Python import. `import a.b as c, d` gives two entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyImport {
    /// The module as written, with leading dots for a relative import:
    /// `shared.db`, `.models`, `..`, `..shared.db`.
    pub module: String,
    /// For `from X import a, b as c`: the imported names `["a", "b"]` (not aliases);
    /// `["*"]` for a wildcard; empty for a plain `import X`.
    pub names: Vec<String>,
    /// 1-based line of the statement's first line.
    pub line: usize,
    /// The statement's first source line, trimmed, at most 120 characters (cut on a char boundary, append `…` when cut).
    pub text: String,
}

/// A call that imports a module chosen at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyDynamic {
    /// `importlib.import_module`, `__import__`, `importlib.__import__`, or `import_module` (when imported from importlib), written as the call appears after resolving aliases: always one of those four strings.
    pub call: String,
    /// 1-based line where the call starts.
    pub line: usize,
    /// The call's first source line, trimmed, at most 120 characters (cut on a char boundary, append `…` when cut).
    pub text: String,
}

/// Everything [`python_imports`] read from one Python file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PythonImports {
    /// Static imports, sorted by line, then in source order.
    pub imports: Vec<PyImport>,
    /// Calls that import a module chosen at run time, sorted by line, then in source order.
    pub dynamic: Vec<PyDynamic>,
    /// True when tree-sitter reported syntax errors; what could be read is still returned.
    pub has_errors: bool,
}

/// The tree-sitter grammar a JS/TS file is parsed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsDialect {
    /// The TypeScript grammar: `.ts`, `.mts`, `.cts` (and `.d.ts`).
    TypeScript,
    /// The TSX grammar, which also reads plain JavaScript and JSX.
    Tsx,
}

/// One static or literal-argument JS/TS import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsImport {
    /// The module specifier string, unquoted: `../shared/x`, `react`.
    pub specifier: String,
    /// `import`, `export` (re-export `export ... from`), `require`, `dynamic-import` (`import("x")`), or `import-equals` (TS `import x = require("x")`).
    pub kind: &'static str,
    /// 1-based line where the statement or call starts.
    pub line: usize,
    /// The statement's first source line, trimmed, at most 120 characters (cut on a char boundary, append `…` when cut).
    pub text: String,
}

/// Everything [`js_imports`] read from one JS/TS file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JsImports {
    /// Imports, sorted by line, then in source order.
    pub imports: Vec<JsImport>,
    /// True when tree-sitter reported syntax errors; what could be read is still returned.
    pub has_errors: bool,
}

/// Picks the grammar for a JS/TS file from its extension, ASCII
/// case-insensitive.
///
/// `.ts`, `.mts`, `.cts` (so `.d.ts` too) give [`JsDialect::TypeScript`];
/// `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` give [`JsDialect::Tsx`]; anything
/// else gives `None`.
#[must_use]
pub fn js_dialect(file_name: &str) -> Option<JsDialect> {
    let ext = Path::new(file_name).extension()?.to_str()?;
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "mts" | "cts" => Some(JsDialect::TypeScript),
        "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(JsDialect::Tsx),
        _ => None,
    }
}

/// Reads the static imports and dynamic import calls of a Python file.
#[must_use]
pub fn python_imports(source: &str) -> PythonImports {
    let Some(tree) = parse(source, &tree_sitter_python::LANGUAGE.into()) else {
        return PythonImports {
            has_errors: true,
            ..PythonImports::default()
        };
    };
    let root = tree.root_node();
    let src = source.as_bytes();
    let lines = Lines::new(source);
    let mut out = PythonImports {
        has_errors: root.has_error(),
        ..PythonImports::default()
    };
    let mut aliases = PyAliases::default();

    walk(root, |node| match node.kind() {
        "import_statement" => {
            let (line, text) = lines.locate(node.start_byte());
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                let (target, alias) = py_aliased(name, src);
                let Some(module) = target else { continue };
                aliases.note_module(&module, alias.as_deref());
                out.imports.push(PyImport {
                    module,
                    names: Vec::new(),
                    line,
                    text: text.clone(),
                });
            }
            false
        }
        "import_from_statement" => {
            if let Some(import) = py_from_import(node, src, &lines, &mut aliases) {
                out.imports.push(import);
            }
            false
        }
        _ => true,
    });

    walk(root, |node| {
        if node.kind() == "call" {
            if let Some(call) = node
                .child_by_field_name("function")
                .and_then(|f| aliases.dynamic_call(f, src))
            {
                let (line, text) = lines.locate(node.start_byte());
                out.dynamic.push(PyDynamic { call, line, text });
            }
        }
        true
    });

    out.imports.sort_by_key(|i| i.line);
    out.dynamic.sort_by_key(|d| d.line);
    out
}

/// Reads the imports, re-exports, `require` calls and `import()` calls of a
/// JS/TS file.
#[must_use]
pub fn js_imports(source: &str, dialect: JsDialect) -> JsImports {
    let language: Language = match dialect {
        JsDialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        JsDialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    };
    let Some(tree) = parse(source, &language) else {
        return JsImports {
            imports: Vec::new(),
            has_errors: true,
        };
    };
    let root = tree.root_node();
    let src = source.as_bytes();
    let lines = Lines::new(source);
    let mut imports = Vec::new();

    walk(root, |node| {
        let found = match node.kind() {
            "import_statement" => js_import_statement(node, src),
            "export_statement" => node
                .child_by_field_name("source")
                .and_then(|s| js_string(s, src))
                .map(|spec| (spec, "export")),
            "call_expression" => js_call(node, src),
            _ => None,
        };
        if let Some((specifier, kind)) = found {
            let (line, text) = lines.locate(node.start_byte());
            imports.push(JsImport {
                specifier,
                kind,
                line,
                text,
            });
        }
        true
    });

    imports.sort_by_key(|i| i.line);
    JsImports {
        imports,
        has_errors: root.has_error(),
    }
}

/// Parses `source`, or `None` when the grammar cannot be loaded (a language
/// version mismatch) or the parser gives up.
pub(crate) fn parse(source: &str, language: &Language) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(source, None)
}

/// Visits every node under `root` (included) in source order, without
/// recursion so deeply nested input cannot overflow the stack. `visit`
/// returns whether to descend into the node's children.
pub(crate) fn walk<'t>(root: Node<'t>, mut visit: impl FnMut(Node<'t>) -> bool) {
    let mut cursor = root.walk();
    loop {
        if visit(cursor.node()) && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// The source text of a node, or `""` if it is not valid UTF-8 (it always
/// is for `&str` input).
pub(crate) fn text<'s>(node: Node<'_>, src: &'s [u8]) -> &'s str {
    node.utf8_text(src).unwrap_or("")
}

/// Byte offsets where each line starts, to turn a node's start into a line
/// number and that line's text.
struct Lines<'a> {
    source: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(source: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { source, starts }
    }

    /// The 1-based line holding `byte`, and that line trimmed and cut to
    /// [`MAX_TEXT_CHARS`].
    fn locate(&self, byte: usize) -> (usize, String) {
        let row = self.starts.partition_point(|&s| s <= byte).max(1) - 1;
        let start = self.starts.get(row).copied().unwrap_or(0);
        let end = self
            .starts
            .get(row + 1)
            .map_or(self.source.len(), |next| next - 1);
        let line = self.source.get(start..end).unwrap_or("");
        (row + 1, shorten(line.trim()))
    }
}

/// Cuts `line` to at most [`MAX_TEXT_CHARS`] characters, ending with `…`
/// when anything was cut.
fn shorten(line: &str) -> String {
    if line.char_indices().nth(MAX_TEXT_CHARS).is_none() {
        return line.to_string();
    }
    let cut = line
        .char_indices()
        .nth(MAX_TEXT_CHARS - 1)
        .map_or(line.len(), |(i, _)| i);
    let mut kept = line.get(..cut).unwrap_or("").to_string();
    kept.push('…');
    kept
}

// ---------------------------------------------------------------- Python

/// A `dotted_name` as written, without any spaces or comments between its
/// parts: `a . b` gives `a.b`.
fn py_dotted(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "dotted_name" {
        return None;
    }
    let mut cursor = node.walk();
    let parts: Vec<&str> = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .map(|c| text(c, src))
        .collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

/// For an import's `name` field (`dotted_name` or `aliased_import`): the
/// dotted name and its alias, if any.
fn py_aliased(node: Node<'_>, src: &[u8]) -> (Option<String>, Option<String>) {
    if node.kind() == "aliased_import" {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| py_dotted(n, src));
        let alias = node
            .child_by_field_name("alias")
            .map(|a| text(a, src).to_string());
        (name, alias)
    } else {
        (py_dotted(node, src), None)
    }
}

/// The module of a `from` import: a `dotted_name`, or a `relative_import`
/// made of an `import_prefix` (the dots) and an optional `dotted_name`.
fn py_from_module(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "dotted_name" => py_dotted(node, src),
        "relative_import" => {
            let mut module = String::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    "import_prefix" => {
                        let dots = text(child, src).chars().filter(|&c| c == '.').count();
                        module.push_str(&".".repeat(dots));
                    }
                    "dotted_name" => module.push_str(&py_dotted(child, src)?),
                    _ => {}
                }
            }
            (!module.is_empty()).then_some(module)
        }
        _ => None,
    }
}

/// One `from X import ...` statement, noting `importlib` aliases on the way.
fn py_from_import(
    node: Node<'_>,
    src: &[u8],
    lines: &Lines<'_>,
    aliases: &mut PyAliases,
) -> Option<PyImport> {
    let module = py_from_module(node.child_by_field_name("module_name")?, src)?;
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "wildcard_import" {
            names.push("*".to_string());
        }
    }
    for name in node.children_by_field_name("name", &mut cursor) {
        let (target, alias) = py_aliased(name, src);
        if let Some(target) = target {
            if module == "importlib" {
                aliases.note_function(&target, alias.as_deref());
            }
            names.push(target);
        }
    }
    let (line, text) = lines.locate(node.start_byte());
    Some(PyImport {
        module,
        names,
        line,
        text,
    })
}

/// Names that refer to `importlib` or to its import functions in one file.
#[derive(Default)]
struct PyAliases {
    /// Names bound to the `importlib` module besides `importlib` itself
    /// (`import importlib as il`).
    modules: Vec<String>,
    /// Names bound by `from importlib import ...`, with the call they stand
    /// for.
    functions: Vec<(String, &'static str)>,
}

impl PyAliases {
    /// Records `import <module> [as <alias>]`.
    fn note_module(&mut self, module: &str, alias: Option<&str>) {
        if module == "importlib" {
            if let Some(alias) = alias {
                self.modules.push(alias.to_string());
            }
        }
    }

    /// Records `from importlib import <name> [as <alias>]`.
    fn note_function(&mut self, name: &str, alias: Option<&str>) {
        let call = match name {
            "import_module" => "import_module",
            "__import__" if alias.is_some() => "importlib.__import__",
            _ => return,
        };
        self.functions
            .push((alias.unwrap_or(name).to_string(), call));
    }

    fn is_importlib(&self, name: &str) -> bool {
        name == "importlib" || self.modules.iter().any(|m| m == name)
    }

    /// The dynamic-import call a call's `function` node stands for, if any.
    fn dynamic_call(&self, function: Node<'_>, src: &[u8]) -> Option<String> {
        match function.kind() {
            "identifier" => {
                let name = text(function, src);
                if let Some((_, call)) = self.functions.iter().find(|(n, _)| n == name) {
                    return Some((*call).to_string());
                }
                (name == "__import__").then(|| "__import__".to_string())
            }
            "attribute" => {
                let object = function.child_by_field_name("object")?;
                let attribute = function.child_by_field_name("attribute")?;
                if object.kind() != "identifier" || !self.is_importlib(text(object, src)) {
                    return None;
                }
                match text(attribute, src) {
                    "import_module" => Some("importlib.import_module".to_string()),
                    "__import__" => Some("importlib.__import__".to_string()),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

// ----------------------------------------------------------------- JS/TS

/// An `import_statement`: `import ... from "x"`, `import "x"`, or TS
/// `import x = require("x")` (an `import_require_clause` child).
fn js_import_statement(node: Node<'_>, src: &[u8]) -> Option<(String, &'static str)> {
    if let Some(source) = node.child_by_field_name("source") {
        return js_string(source, src).map(|s| (s, "import"));
    }
    let mut cursor = node.walk();
    let clause = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "import_require_clause")?;
    let source = clause.child_by_field_name("source")?;
    js_string(source, src).map(|s| (s, "import-equals"))
}

/// A `call_expression` that is `require("x")` or `import("x")` with a
/// literal module specifier.
fn js_call(node: Node<'_>, src: &[u8]) -> Option<(String, &'static str)> {
    let function = node.child_by_field_name("function")?;
    let arguments = node.child_by_field_name("arguments")?;
    if arguments.kind() != "arguments" {
        // A tagged template such as require`x` is not a call with arguments.
        return None;
    }
    let mut cursor = arguments.walk();
    let args: Vec<Node<'_>> = arguments
        .named_children(&mut cursor)
        .filter(|a| !a.is_extra())
        .collect();
    let kind = match function.kind() {
        // `import("x")`, optionally with an options object (import attributes).
        "import" if (1..=2).contains(&args.len()) => "dynamic-import",
        "identifier" if text(function, src) == "require" && args.len() == 1 => "require",
        _ => return None,
    };
    let first = args.first()?;
    js_string(*first, src).map(|s| (s, kind))
}

/// The value of a string literal, or of a template string with no
/// substitutions; `None` for anything else.
pub(crate) fn js_string(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "string" && node.kind() != "template_string" {
        return None;
    }
    let mut value = String::new();
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        match part.kind() {
            "string_fragment" => value.push_str(text(part, src)),
            "escape_sequence" => value.push_str(&unescape(text(part, src))),
            "template_substitution" => return None,
            // Comments cannot appear inside a string; anything else
            // (an HTML character reference in JSX) is kept as written.
            _ => value.push_str(text(part, src)),
        }
    }
    Some(value)
}

/// Decodes one JS escape sequence (`\n`, `\x41`, `\u0041`, `\u{41}`, a line
/// continuation, or `\` before any other character). Anything it cannot
/// decode is kept as written.
fn unescape(seq: &str) -> String {
    let Some(rest) = seq.strip_prefix('\\') else {
        return seq.to_string();
    };
    let hex = |digits: &str| {
        u32::from_str_radix(digits, 16)
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
    };
    let decoded = match rest {
        "n" => Some("\n".to_string()),
        "t" => Some("\t".to_string()),
        "r" => Some("\r".to_string()),
        "b" => Some("\u{8}".to_string()),
        "f" => Some("\u{c}".to_string()),
        "v" => Some("\u{b}".to_string()),
        "0" => Some("\0".to_string()),
        "\n" | "\r\n" | "\r" | "\u{2028}" | "\u{2029}" => Some(String::new()),
        _ if rest.starts_with("u{") && rest.ends_with('}') => {
            rest.get(2..rest.len() - 1).and_then(hex)
        }
        _ if rest.starts_with('u') || rest.starts_with('x') => rest.get(1..).and_then(hex),
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c.to_string()),
                _ => None,
            }
        }
    };
    decoded.unwrap_or_else(|| seq.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py(source: &str) -> Vec<(String, Vec<String>, usize)> {
        python_imports(source)
            .imports
            .into_iter()
            .map(|i| (i.module, i.names, i.line))
            .collect()
    }

    fn py_modules(source: &str) -> Vec<String> {
        python_imports(source)
            .imports
            .into_iter()
            .map(|i| i.module)
            .collect()
    }

    fn calls(source: &str) -> Vec<(String, usize)> {
        python_imports(source)
            .dynamic
            .into_iter()
            .map(|d| (d.call, d.line))
            .collect()
    }

    fn js(source: &str, dialect: JsDialect) -> Vec<(String, &'static str, usize)> {
        js_imports(source, dialect)
            .imports
            .into_iter()
            .map(|i| (i.specifier, i.kind, i.line))
            .collect()
    }

    fn ts(source: &str) -> Vec<(String, &'static str, usize)> {
        js(source, JsDialect::TypeScript)
    }

    fn tsx(source: &str) -> Vec<(String, &'static str, usize)> {
        js(source, JsDialect::Tsx)
    }

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    fn entry(spec: &str, kind: &'static str, line: usize) -> (String, &'static str, usize) {
        (spec.to_string(), kind, line)
    }

    // ------------------------------------------------------------ loading

    #[test]
    fn both_grammars_load_with_this_tree_sitter_version() {
        let mut parser = Parser::new();
        assert!(parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .is_ok());
        assert!(parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .is_ok());
        assert!(parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
            .is_ok());
        assert!(!python_imports("import os\n").has_errors);
        assert!(!js_imports("import x from 'y';\n", JsDialect::TypeScript).has_errors);
        assert!(!js_imports("import x from 'y';\n", JsDialect::Tsx).has_errors);
    }

    #[test]
    fn empty_input_gives_nothing_and_no_errors() {
        assert_eq!(python_imports(""), PythonImports::default());
        assert_eq!(js_imports("", JsDialect::Tsx), JsImports::default());
        assert_eq!(js_imports("", JsDialect::TypeScript), JsImports::default());
    }

    // ----------------------------------------------------- Python imports

    #[test]
    fn a_plain_import_gives_its_module_and_no_names() {
        assert_eq!(py("import os\n"), vec![("os".to_string(), vec![], 1)]);
    }

    #[test]
    fn a_dotted_import_keeps_the_whole_dotted_name() {
        assert_eq!(
            py_modules("import shared.db.models\n"),
            owned(&["shared.db.models"])
        );
    }

    #[test]
    fn an_aliased_import_gives_the_module_not_the_alias() {
        assert_eq!(py_modules("import numpy as np\n"), owned(&["numpy"]));
    }

    #[test]
    fn an_import_of_several_modules_gives_one_entry_each() {
        let found = python_imports("import a.b as c, d\n").imports;
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].module, "a.b");
        assert_eq!(found[1].module, "d");
        assert!(found.iter().all(|i| i.line == 1 && i.names.is_empty()));
        assert!(found.iter().all(|i| i.text == "import a.b as c, d"));
    }

    #[test]
    fn a_from_import_gives_the_names_not_the_aliases() {
        assert_eq!(
            py("from shared.db import a, b as c\n"),
            vec![("shared.db".to_string(), owned(&["a", "b"]), 1)]
        );
    }

    #[test]
    fn relative_imports_keep_their_dots() {
        let source = "from . import x\nfrom .models import User\nfrom .. import y\nfrom ..shared.db import z\nfrom ... import w\n";
        assert_eq!(
            py(source),
            vec![
                (".".to_string(), owned(&["x"]), 1),
                (".models".to_string(), owned(&["User"]), 2),
                ("..".to_string(), owned(&["y"]), 3),
                ("..shared.db".to_string(), owned(&["z"]), 4),
                ("...".to_string(), owned(&["w"]), 5),
            ]
        );
    }

    #[test]
    fn a_wildcard_import_gives_a_star() {
        assert_eq!(
            py("from pkg.mod import *\nfrom . import *\n"),
            vec![
                ("pkg.mod".to_string(), owned(&["*"]), 1),
                (".".to_string(), owned(&["*"]), 2),
            ]
        );
    }

    #[test]
    fn a_parenthesised_multi_line_from_import_is_one_entry_on_its_first_line() {
        let source = "x = 1\nfrom shared.db import (\n    a,\n    b as c,  # why\n    d,\n)\n";
        let found = python_imports(source).imports;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].module, "shared.db");
        assert_eq!(found[0].names, owned(&["a", "b", "d"]));
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].text, "from shared.db import (");
    }

    #[test]
    fn future_imports_are_skipped() {
        assert_eq!(
            py_modules("from __future__ import annotations\nimport os\n"),
            owned(&["os"])
        );
    }

    #[test]
    fn imports_inside_try_functions_and_type_checking_blocks_count() {
        let source = "\
from typing import TYPE_CHECKING
try:
    import ujson as json
except ImportError:
    import json

if TYPE_CHECKING:
    from shared.models import User

def load():
    import yaml
    class Inner:
        from .local import thing
    return yaml
";
        assert_eq!(
            py(source),
            vec![
                ("typing".to_string(), owned(&["TYPE_CHECKING"]), 1),
                ("ujson".to_string(), vec![], 3),
                ("json".to_string(), vec![], 5),
                ("shared.models".to_string(), owned(&["User"]), 8),
                ("yaml".to_string(), vec![], 11),
                (".local".to_string(), owned(&["thing"]), 13),
            ]
        );
    }

    #[test]
    fn an_import_inside_a_docstring_does_not_count() {
        let source =
            "\"\"\"Use it like this:\n\nimport secret\nfrom x import y\n\"\"\"\nimport real\n";
        assert_eq!(py(source), vec![("real".to_string(), vec![], 6)]);
    }

    #[test]
    fn imports_in_strings_and_comments_do_not_count() {
        let source = "\
# import commented
s = 'import quoted'
t = \"from a import b\"
u = f\"import {name}\"
v = '''
import triple
'''
import real  # import trailing
";
        assert_eq!(py_modules(source), owned(&["real"]));
        assert!(calls(source).is_empty());
    }

    #[test]
    fn a_file_with_a_syntax_error_still_gives_its_valid_imports() {
        let source = "import os\n\ndef broken(:\n    pass\n\nfrom shared import db\n";
        let found = python_imports(source);
        assert!(found.has_errors);
        let modules: Vec<&str> = found.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(modules.contains(&"os"), "{modules:?}");
        assert!(modules.contains(&"shared"), "{modules:?}");
    }

    #[test]
    fn crlf_line_endings_give_the_right_line_numbers_and_text() {
        let source = "\"\"\"Doc.\r\n\r\nimport no\r\n\"\"\"\r\nimport os\r\nfrom a import (\r\n    b,\r\n)\r\nimport c\r\n";
        let found = python_imports(source).imports;
        let got: Vec<(&str, usize, &str)> = found
            .iter()
            .map(|i| (i.module.as_str(), i.line, i.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("os", 5, "import os"),
                ("a", 6, "from a import ("),
                ("c", 9, "import c"),
            ]
        );
    }

    #[test]
    fn the_text_is_the_trimmed_first_line_of_the_statement() {
        let found = python_imports("if True:\n    import os   \n").imports;
        assert_eq!(found[0].text, "import os");
        assert_eq!(found[0].line, 2);
    }

    // ----------------------------------------------------- Python dynamic

    #[test]
    fn each_dynamic_import_form_is_found() {
        let source = "\
import importlib
a = importlib.import_module('a')
b = __import__('b')
c = importlib.__import__('c')
";
        assert_eq!(
            calls(source),
            vec![
                ("importlib.import_module".to_string(), 2),
                ("__import__".to_string(), 3),
                ("importlib.__import__".to_string(), 4),
            ]
        );
    }

    #[test]
    fn an_aliased_importlib_module_resolves_to_importlib() {
        let source =
            "import importlib as il\nm = il.import_module(name)\nn = il.__import__(name)\n";
        assert_eq!(
            calls(source),
            vec![
                ("importlib.import_module".to_string(), 2),
                ("importlib.__import__".to_string(), 3),
            ]
        );
    }

    #[test]
    fn import_module_imported_from_importlib_is_found_with_or_without_alias() {
        let source = "\
from importlib import import_module
from importlib import import_module as im
def f(name):
    return import_module(name), im(name)
";
        assert_eq!(
            calls(source),
            vec![
                ("import_module".to_string(), 4),
                ("import_module".to_string(), 4)
            ]
        );
    }

    #[test]
    fn import_module_not_from_importlib_does_not_count() {
        let source =
            "from mylib import import_module\nimport_module('x')\nother.import_module('y')\n";
        assert!(calls(source).is_empty());
    }

    #[test]
    fn dynamic_import_mentions_that_are_not_calls_do_not_count() {
        let source = "\
import importlib
f = importlib.import_module
g = __import__
# importlib.import_module('x')
s = \"__import__('y')\"
\"\"\"importlib.import_module('z')\"\"\"
";
        assert!(calls(source).is_empty());
    }

    #[test]
    fn the_dynamic_call_text_is_its_line() {
        let found = python_imports("x = 1\n    \nmod = __import__(name)  \n").dynamic;
        assert_eq!(
            found,
            vec![PyDynamic {
                call: "__import__".to_string(),
                line: 3,
                text: "mod = __import__(name)".to_string(),
            }]
        );
    }

    // -------------------------------------------------------------- JS/TS

    #[test]
    fn dialects_follow_the_extension_case_insensitively() {
        for name in ["a.ts", "b.MTS", "c.cts", "types.d.ts", "src/x/y.Ts"] {
            assert_eq!(js_dialect(name), Some(JsDialect::TypeScript), "{name}");
        }
        for name in ["a.tsx", "b.js", "c.JSX", "d.mjs", "e.cjs"] {
            assert_eq!(js_dialect(name), Some(JsDialect::Tsx), "{name}");
        }
        for name in ["a.py", "b.json", "README", "ts", "c.ts.bak"] {
            assert_eq!(js_dialect(name), None, "{name}");
        }
    }

    #[test]
    fn each_static_import_form_is_found() {
        let source = "\
import React from 'react';
import { a, b as c } from \"../shared/x\";
import * as ns from './ns';
import './side-effect.css';
import type { T } from './types';
import def, { named } from '@scope/pkg/sub';
";
        assert_eq!(
            ts(source),
            vec![
                entry("react", "import", 1),
                entry("../shared/x", "import", 2),
                entry("./ns", "import", 3),
                entry("./side-effect.css", "import", 4),
                entry("./types", "import", 5),
                entry("@scope/pkg/sub", "import", 6),
            ]
        );
    }

    #[test]
    fn re_exports_are_found_and_plain_exports_are_not() {
        let source = "\
export * from './all';
export * as ns from './ns';
export { a, b as c } from './some';
export type { T } from './types';
export const x = 1;
export { x as y };
export default function f() {}
";
        assert_eq!(
            ts(source),
            vec![
                entry("./all", "export", 1),
                entry("./ns", "export", 2),
                entry("./some", "export", 3),
                entry("./types", "export", 4),
            ]
        );
    }

    #[test]
    fn require_with_a_string_or_plain_template_is_found() {
        let source = "\
const a = require('a');
const b = require(\"./b\");
const c = require(`c`);
require(/* why */ 'd');
";
        assert_eq!(
            tsx(source),
            vec![
                entry("a", "require", 1),
                entry("./b", "require", 2),
                entry("c", "require", 3),
                entry("d", "require", 4),
            ]
        );
    }

    #[test]
    fn require_with_a_non_literal_argument_is_ignored() {
        let source = "\
const name = 'x';
require(name);
require(`./${name}`);
require('a' + name);
require('a', 'b');
require();
obj.require('z');
require`tagged`;
";
        assert!(tsx(source).is_empty());
    }

    #[test]
    fn dynamic_import_with_a_literal_is_found_and_otherwise_ignored() {
        let source = "\
const a = await import('./a');
const b = import(`./b`);
const c = import('./c', { with: { type: 'json' } });
const d = import(name);
const e = import(`./${name}`);
";
        assert_eq!(
            ts(source),
            vec![
                entry("./a", "dynamic-import", 1),
                entry("./b", "dynamic-import", 2),
                entry("./c", "dynamic-import", 3),
            ]
        );
    }

    #[test]
    fn typescript_import_equals_require_is_found() {
        let source = "import fs = require('fs');\nimport type T = require('./t');\nimport Alias = NS.Inner;\n";
        assert_eq!(
            ts(source),
            vec![
                entry("fs", "import-equals", 1),
                entry("./t", "import-equals", 2),
            ]
        );
    }

    #[test]
    fn a_declaration_file_is_read_with_the_typescript_grammar() {
        let source = "\
import { Base } from './base';
declare module 'virtual' {
  export function f(): void;
}
export type { Base } from './base';
";
        let found = js_imports(source, JsDialect::TypeScript);
        assert!(!found.has_errors);
        assert_eq!(
            found
                .imports
                .into_iter()
                .map(|i| (i.specifier, i.kind, i.line))
                .collect::<Vec<_>>(),
            vec![entry("./base", "import", 1), entry("./base", "export", 5)]
        );
    }

    #[test]
    fn a_tsx_file_with_jsx_is_read() {
        let source = "\
import React from 'react';
import { Button } from './button';
export function App(props: { n: number }) {
  const Lazy = React.lazy(() => import('./lazy'));
  return <div title=\"import('fake')\">{props.n} require('no') <Button /></div>;
}
";
        let found = js_imports(source, JsDialect::Tsx);
        assert!(!found.has_errors);
        assert_eq!(
            found
                .imports
                .into_iter()
                .map(|i| (i.specifier, i.kind, i.line))
                .collect::<Vec<_>>(),
            vec![
                entry("react", "import", 1),
                entry("./button", "import", 2),
                entry("./lazy", "dynamic-import", 4),
            ]
        );
    }

    #[test]
    fn a_js_file_with_jsx_is_read_with_the_tsx_grammar() {
        let source = "\
const React = require('react');
module.exports = function App() {
  return <main>{items.map((i) => <li key={i}>{i}</li>)}</main>;
};
";
        let dialect = js_dialect("app.js");
        assert_eq!(dialect, Some(JsDialect::Tsx));
        let found = js_imports(source, JsDialect::Tsx);
        assert!(!found.has_errors);
        assert_eq!(found.imports.len(), 1);
        assert_eq!(found.imports[0].specifier, "react");
        assert_eq!(found.imports[0].kind, "require");
    }

    #[test]
    fn comments_strings_templates_and_regexes_do_not_count() {
        let source = "\
// import x from 'line-comment';
/* require('block-comment') */
/** import('doc-comment') */
const s = \"import a from 'string'\";
const t = 'require(\"string2\")';
const u = `import('template') ${require('real')} require('template2')`;
const r = /import\\(\"x\"\\)|require\\('y'\\)/g;
const r2 = /import x from 'regex'/;
";
        assert_eq!(tsx(source), vec![entry("real", "require", 6)]);
        assert_eq!(ts(source), vec![entry("real", "require", 6)]);
    }

    #[test]
    fn escapes_in_a_specifier_are_decoded() {
        assert_eq!(
            tsx("import a from './a\\u002db';\nrequire('c\\'d');\n"),
            vec![entry("./a-b", "import", 1), entry("c'd", "require", 2)]
        );
    }

    #[test]
    fn js_results_are_sorted_by_line_then_source_order() {
        let source = "const a = require('a'), b = require('b');\nimport c from 'c';\nexport * from 'd'; import('e');\n";
        assert_eq!(
            ts(source),
            vec![
                entry("a", "require", 1),
                entry("b", "require", 1),
                entry("c", "import", 2),
                entry("d", "export", 3),
                entry("e", "dynamic-import", 3),
            ]
        );
    }

    #[test]
    fn a_js_file_with_a_syntax_error_still_gives_its_valid_imports() {
        let source = "import a from 'a';\nconst = ;\nfunction (\nimport b from 'b';\n";
        let found = js_imports(source, JsDialect::TypeScript);
        assert!(found.has_errors);
        assert!(found.imports.iter().any(|i| i.specifier == "a"));
    }

    #[test]
    fn js_crlf_line_endings_give_the_right_line_numbers() {
        let source =
            "/*\r\n import('no')\r\n*/\r\nimport a from 'a';\r\nconst b = require('b');\r\n";
        let found = js_imports(source, JsDialect::Tsx).imports;
        let got: Vec<(&str, usize, &str)> = found
            .iter()
            .map(|i| (i.specifier.as_str(), i.line, i.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a", 4, "import a from 'a';"),
                ("b", 5, "const b = require('b');"),
            ]
        );
    }

    // ------------------------------------------------------ text and abuse

    #[test]
    fn text_longer_than_120_characters_is_cut_with_an_ellipsis() {
        let names: Vec<String> = (0..60).map(|i| format!("n{i}")).collect();
        let source = format!("from pkg import {}\n", names.join(", "));
        let found = python_imports(&source).imports;
        assert_eq!(found[0].names.len(), 60);
        let text = &found[0].text;
        assert_eq!(text.chars().count(), 120);
        assert!(text.ends_with('…'));
        assert!(text.starts_with("from pkg import n0, n1"));
    }

    #[test]
    fn text_is_cut_on_a_char_boundary() {
        let long = "é".repeat(200);
        let source = format!("import x  # {long}\n");
        let text = &python_imports(&source).imports[0].text;
        assert_eq!(text.chars().count(), 120);
        assert!(text.ends_with("éé…"));

        let source = format!("const x = require('ü');  // {long}\n");
        let text = &js_imports(&source, JsDialect::Tsx).imports[0].text;
        assert_eq!(text.chars().count(), 120);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn text_of_exactly_120_characters_is_kept_whole() {
        let line = format!("import {}", "a".repeat(113));
        assert_eq!(line.chars().count(), 120);
        assert_eq!(shorten(&line), line);
        assert_eq!(shorten(&format!("{line}b")).chars().count(), 120);
    }

    #[test]
    fn many_imports_on_one_huge_line_are_all_found() {
        let line: String = (0..2000).map(|i| format!("require('m{i}');")).collect();
        let found = js_imports(&line, JsDialect::Tsx).imports;
        assert_eq!(found.len(), 2000);
        assert_eq!(found[1999].specifier, "m1999");
        assert!(found.iter().all(|i| i.line == 1 && i.text.ends_with('…')));
    }

    #[test]
    fn odd_input_never_panics() {
        let deep = format!("{}{}", "(".repeat(20_000), ")".repeat(20_000));
        let samples = [
            "\0\0\0",
            "\u{feff}import os\n",
            "\r\r\r\n\n",
            "from",
            "from . import",
            "import",
            "import(",
            "require(",
            "export * from",
            "`${`${`",
            "\"\"\"never closed",
            "é\u{1F600}\u{200B}",
            deep.as_str(),
        ];
        for sample in samples {
            let _ = python_imports(sample);
            let _ = js_imports(sample, JsDialect::TypeScript);
            let _ = js_imports(sample, JsDialect::Tsx);
        }
    }
}
