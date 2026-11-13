//! Code that never runs, and assertions that cannot fail (M24, ADR 0044).
//!
//! The counts of ADR 0042 are of code as written. These helpers let each
//! language's reader leave out what certainly never runs or never checks
//! anything, so that wrapping a test in `if (false)`, returning before its
//! assertions, or replacing `expect(total).toBe(3)` with
//! `expect(true).toBe(true)` reads as what it is. Only what is certain from
//! the syntax tree alone is used: a literal `false` condition, a `return`
//! earlier in the same block, an assertion with no name or call in it.

use tree_sitter::Node;

use crate::imports::text;

/// What one grammar calls the nodes these helpers look at.
pub(super) struct Grammar {
    /// Lists of statements, in which a `return` makes the statements after
    /// it unreachable: `statement_block`, Python's `block`, Rust's `block`.
    pub blocks: &'static [&'static str],
    /// A return: `return_statement`, Rust's `return_expression` (also as
    /// the only child of an `expression_statement`).
    pub returns: &'static [&'static str],
    /// `if` statements or expressions, with `condition` and `consequence`
    /// fields.
    pub ifs: &'static [&'static str],
    /// `while` loops, with `condition` and `body` fields.
    pub whiles: &'static [&'static str],
    /// Definitions that exist wherever they are written: a hoisted JS
    /// `function` declaration, a Rust item. Nothing above one is looked at.
    pub hoisted: &'static [&'static str],
    /// A statement after which the rest of its block does not run, besides
    /// a return: pytest's `pytest.skip(…)`. `None` for none.
    pub stops: Option<fn(Node<'_>, &[u8]) -> bool>,
    /// A statement that does nothing: `pass`, a docstring, `;`.
    pub noop: fn(Node<'_>, &[u8]) -> bool,
}

/// Why a node never runs, or `None` when it may.
pub(super) fn unreachable(node: Node<'_>, src: &[u8], grammar: &Grammar) -> Option<&'static str> {
    let mut child = node;
    while let Some(parent) = child.parent() {
        if grammar.hoisted.contains(&child.kind()) && child != node {
            return None;
        }
        let kind = parent.kind();
        if grammar.ifs.contains(&kind)
            && parent.child_by_field_name("consequence") == Some(child)
            && parent
                .child_by_field_name("condition")
                .is_some_and(|condition| is_false(condition, src))
        {
            return Some("inside if (false)");
        }
        if grammar.whiles.contains(&kind)
            && parent.child_by_field_name("body") == Some(child)
            && parent
                .child_by_field_name("condition")
                .is_some_and(|condition| is_false(condition, src))
        {
            return Some("inside while (false)");
        }
        if grammar.blocks.contains(&kind) {
            let mut earlier = child.prev_named_sibling();
            while let Some(statement) = earlier {
                if is_return(statement, grammar) {
                    return Some("after a return");
                }
                if grammar.stops.is_some_and(|stops| stops(statement, src)) {
                    return Some("after a skip");
                }
                earlier = statement.prev_named_sibling();
            }
        }
        child = parent;
    }
    None
}

/// True for a literal `false` condition, in parentheses or not.
fn is_false(condition: Node<'_>, src: &[u8]) -> bool {
    let mut at = condition;
    for _ in 0..8 {
        match at.kind() {
            "parenthesized_expression" => match at.named_child(0) {
                Some(inner) => at = inner,
                None => return false,
            },
            "false" => return true,
            "boolean_literal" => return text(at, src) == "false",
            _ => return false,
        }
    }
    false
}

/// True for a return statement, or Rust's `return …;`.
fn is_return(statement: Node<'_>, grammar: &Grammar) -> bool {
    grammar.returns.contains(&statement.kind())
        || (statement.kind() == "expression_statement"
            && statement
                .named_child(0)
                .is_some_and(|inner| grammar.returns.contains(&inner.kind())))
}

/// True when a function body runs nothing: it is empty, or holds only
/// statements that do nothing (and comments) before its first return.
pub(super) fn does_nothing(body: Node<'_>, src: &[u8], grammar: &Grammar) -> bool {
    let mut cursor = body.walk();
    for statement in body.named_children(&mut cursor) {
        if statement.is_extra() || statement.kind().ends_with("comment") {
            continue;
        }
        if is_return(statement, grammar) {
            return true;
        }
        if (grammar.noop)(statement, src) {
            continue;
        }
        return false;
    }
    true
}

/// True when every named node under `node` (included) is one of
/// `literals`, comments aside: a value that is the same on every run.
pub(super) fn constant(node: Node<'_>, literals: &[&str]) -> bool {
    let mut only_literals = true;
    crate::imports::walk(node, |inner| {
        if !only_literals {
            return false;
        }
        if inner.is_named() && !inner.is_extra() && !literals.contains(&inner.kind()) {
            only_literals = false;
        }
        only_literals
    });
    only_literals
}
