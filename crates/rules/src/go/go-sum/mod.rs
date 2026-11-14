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

/// True when `go.mod` requires a module: a `require` line or block with an
/// entry.
#[must_use]
pub fn requires(go_mod: &str) -> bool {
    let mut in_block = false;
    for line in go_mod.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        if in_block {
            if line == ")" {
                in_block = false;
            } else if !line.is_empty() {
                return true;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("require") {
            let rest = rest.trim();
            if rest == "(" {
                in_block = true;
            } else if !rest
                .trim_matches(|c| c == '(' || c == ')')
                .trim()
                .is_empty()
            {
                // `require x v1`, or a block on one line.
                return true;
            }
        }
    }
    false
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
