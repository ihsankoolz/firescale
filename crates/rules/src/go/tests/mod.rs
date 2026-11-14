//! `go/tests`: a module with Go code has at least one test file, so
//! `go test ./...` checks something.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding, go_files};

#[cfg(test)]
mod tests;

pub const ID: &str = "go/tests";

pub struct Tests;

impl BuiltinRule for Tests {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "A module with Go code has a _test.go file.".to_owned(),
            inputs: super::inputs(&["**/*.go"]),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let files = go_files(ctx)?;
        // No Go code at all is `go/entry`'s.
        if files.is_empty() || files.iter().any(|file| file.ends_with("_test.go")) {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            &files[0],
            format!(
                "The module has {} Go file{} and no test: `go test ./...` passes without checking anything. Add a _test.go file beside the code it tests.",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            Fixable::Agent,
        )])
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
