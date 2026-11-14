//! `maven/tests`: a project with Java code has a test under `src/test`, so
//! `mvn -B test` checks something.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding, java_files};

#[cfg(test)]
mod tests;

pub const ID: &str = "maven/tests";

pub struct Tests;

impl BuiltinRule for Tests {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "A project with Java code under src/main has a test under src/test."
                .to_owned(),
            inputs: super::inputs(&["src/**/*.java"]),
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
        let (main, test) = java_files(ctx)?;
        if main.is_empty() || !test.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            &main[0],
            format!(
                "The project has {} Java file{} under src/main and no test under src/test: `mvn -B test` passes without checking anything. Add a JUnit test beside the code it tests, under src/test/java.",
                main.len(),
                if main.len() == 1 { "" } else { "s" }
            ),
            Fixable::Agent,
        )])
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
