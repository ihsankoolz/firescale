//! `go/go-sum`: a module that requires other modules has a `go.sum`, which
//! the go tool needs to build them and to check what it downloads.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, applies, finding};

#[cfg(test)]
mod tests;

pub const ID: &str = "go/go-sum";

pub struct GoSum;

/// The directives of one kind in `go.mod`, from lines and blocks: each
/// entry without the keyword and comments (`golang.org/x/text v0.20.0`).
fn directives<'t>(go_mod: &'t str, keyword: &str) -> Vec<&'t str> {
    let mut found = Vec::new();
    let mut in_block = false;
    for line in go_mod.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        if in_block {
            if line == ")" {
                in_block = false;
            } else if !line.is_empty() {
                found.push(line);
            }
            continue;
        }
        let Some(rest) = line.strip_prefix(keyword) else {
            continue;
        };
        if !rest.starts_with([' ', '\t', '(']) {
            continue;
        }
        let rest = rest.trim();
        if rest == "(" {
            in_block = true;
        } else {
            let inline = rest.trim_matches(|c| c == '(' || c == ')').trim();
            if !inline.is_empty() {
                found.push(inline);
            }
        }
    }
    found
}

/// True when `go.mod` requires a module that needs a checksum: one no
/// `replace` points to a folder (`replace example.com/lib => ./lib` needs
/// none, and `go mod tidy` writes no `go.sum` for it; found in review).
#[must_use]
pub fn requires(go_mod: &str) -> bool {
    let local: Vec<&str> = directives(go_mod, "replace")
        .into_iter()
        .filter_map(|entry| {
            let (from, to) = entry.split_once("=>")?;
            let to = to.trim();
            let is_folder = to.starts_with("./")
                || to.starts_with("../")
                || to.starts_with('/')
                || to.starts_with(".\\")
                || to.starts_with("..\\")
                || to.get(1..3) == Some(":\\");
            is_folder.then(|| from.split_whitespace().next().unwrap_or_default())
        })
        .collect();
    directives(go_mod, "require").into_iter().any(|entry| {
        let module = entry.split_whitespace().next().unwrap_or_default();
        !local.contains(&module)
    })
}

impl BuiltinRule for GoSum {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Security,
            description: "A module that requires other modules has a go.sum.".to_owned(),
            inputs: super::inputs(&["go.sum"]),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? || any_exists(ctx, &["go.sum"]) {
            return Ok(Vec::new());
        }
        let Some(text) = ctx.read("go.mod")? else {
            return Ok(Vec::new());
        };
        if !requires(&text) {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "go.sum",
            "go.mod requires other modules and there is no go.sum: the go tool will not build them (\"missing go.sum entry\"), and nothing checks that what it downloads is what was tested. Run `go mod tidy` and commit go.sum.".to_owned(),
            Fixable::Manual,
        )])
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
