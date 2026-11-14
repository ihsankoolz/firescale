//! Code that never runs, and assertions that cannot fail (M24, ADR 0044).
//!
//! The counts of ADR 0042 are of code as written. These helpers let each
//! language's reader leave out what certainly never runs or never checks
//! anything, so that wrapping a test in `if (false)`, returning before its
//! assertions, or replacing `expect(total).toBe(3)` with
//! `expect(true).toBe(true)` reads as what it is. Only what is certain from
//! the syntax tree alone is used: a literal `false` condition, a `return`
//! earlier in the same block, an assertion with no name or call in it.

use std::collections::HashMap;

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

/// The nodes of one tree that never run, with why, found in one pass from
/// the root. Looking a node up is then constant time: walking up from each
/// node with `parent()` would be quadratic, since tree-sitter finds a
/// node's parent by descending from the root.
pub(super) struct Dead {
    nodes: HashMap<usize, &'static str>,
}

/// What the children of one node inherit, on the walk.
struct Frame {
    /// Why the children never run, when they don't.
    state: Option<&'static str>,
    /// The node is a block: a return among its children makes the later
    /// ones unreachable.
    block: bool,
    /// Set once a return or a stop was seen among this block's children.
    stopped: Option<&'static str>,
    /// The one child that never runs: the consequence of `if (false)`, the
    /// body of `while (false)`.
    dead_child: Option<(usize, &'static str)>,
}

impl Dead {
    /// Finds every node under `root` that never runs: inside `if (false)`
    /// or `while (false)`, or after a return (or a stop) earlier in its
    /// block or a block around it. A hoisted definition (a JS `function`
    /// declaration, a Rust item) exists wherever it is written, so what is
    /// around it does not reach inside it.
    pub(super) fn of(root: Node<'_>, src: &[u8], grammar: &Grammar) -> Self {
        let mut nodes = HashMap::new();
        let mut stack: Vec<Frame> = Vec::new();
        let mut cursor = root.walk();
        loop {
            let node = cursor.node();
            let mut own = None;
            if let Some(frame) = stack.last_mut() {
                own = frame.state.or(frame.stopped);
                if let Some((id, why)) = frame.dead_child {
                    if id == node.id() {
                        own = own.or(Some(why));
                    }
                }
                if frame.block && node.is_named() && frame.stopped.is_none() {
                    if is_return(node, grammar) {
                        frame.stopped = Some("after a return");
                    } else if grammar.stops.is_some_and(|stops| stops(node, src)) {
                        frame.stopped = Some("after a skip");
                    }
                }
            }
            if let Some(why) = own {
                nodes.insert(node.id(), why);
            }
            if cursor.goto_first_child() {
                let kind = node.kind();
                let false_condition = || {
                    node.child_by_field_name("condition")
                        .is_some_and(|condition| is_false(condition, src))
                };
                let dead_child = if grammar.ifs.contains(&kind) && false_condition() {
                    node.child_by_field_name("consequence")
                        .map(|child| (child.id(), "inside if (false)"))
                } else if grammar.whiles.contains(&kind) && false_condition() {
                    node.child_by_field_name("body")
                        .map(|child| (child.id(), "inside while (false)"))
                } else {
                    None
                };
                stack.push(Frame {
                    state: if grammar.hoisted.contains(&kind) {
                        None
                    } else {
                        own
                    },
                    block: grammar.blocks.contains(&kind),
                    stopped: None,
                    dead_child,
                });
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return Self { nodes };
                }
                stack.pop();
            }
        }
    }

    /// Why `node` never runs, or `None` when it may.
    pub(super) fn why(&self, node: Node<'_>) -> Option<&'static str> {
        self.nodes.get(&node.id()).copied()
    }
}

/// True for a literal `false` condition, in parentheses or not; in C and
/// C++ also a literal `0` (`if (0)`), and C++'s `condition_clause`.
fn is_false(condition: Node<'_>, src: &[u8]) -> bool {
    let mut at = condition;
    for _ in 0..8 {
        match at.kind() {
            "parenthesized_expression" => match at.named_child(0) {
                Some(inner) => at = inner,
                None => return false,
            },
            // C++: `if (false)` holds its condition in a clause, with an
            // optional initializer (`if (int x = f(); false)`).
            "condition_clause" => match at.child_by_field_name("value") {
                Some(inner) => at = inner,
                None => return false,
            },
            "false" => return true,
            "number_literal" => return text(at, src) == "0",
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
