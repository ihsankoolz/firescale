//! `python/test-tools`: each Python project declares pytest among its
//! dependencies, so `python -m pytest` works once they are installed.

use firescale_core::facts::python_declared;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::requirements::dev_requirements;
use super::{create, finding, targets, Target, INPUTS};

#[cfg(test)]
mod tests;

pub const ID: &str = "python/test-tools";

pub struct TestTools;

fn declares_pytest(ctx: &RuleContext, target: &Target) -> bool {
    python_declared(ctx.root(), &target.folder).contains("pytest")
}

/// True when a new requirements-dev.txt can be made: requirements.txt is
/// there and requirements-dev.txt is not (nothing a person wrote is edited).
fn can_create(ctx: &RuleContext, target: &Target) -> bool {
    target.exists(ctx, "requirements.txt")
        && !target.exists(ctx, "requirements-dev.txt")
        && !target.exists(ctx, "pyproject.toml")
}

impl BuiltinRule for TestTools {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "Each Python project declares pytest among its dependencies.".to_owned(),
            inputs: INPUTS.iter().map(|input| (*input).to_owned()).collect(),
            has_fix: true,
            after: vec![super::requirements::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for target in targets(ctx)? {
            if target.is_new() || declares_pytest(ctx, &target) {
                continue;
            }
            findings.push(if can_create(ctx, &target) {
                finding(
                    &target,
                    "requirements-dev.txt",
                    "pytest is not declared. `fscale sync` creates requirements-dev.txt with `-r requirements.txt`, pytest and ruff.".to_owned(),
                    Fixable::Deterministic,
                )
            } else {
                finding(
                    &target,
                    "requirements-dev.txt",
                    format!("pytest is not declared in {}. Add it to its development dependencies (requirements-dev.txt, or a dev group in pyproject.toml).", target.folder),
                    Fixable::Agent,
                )
            });
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        for target in targets(ctx)? {
            if !target.is_new() && !declares_pytest(ctx, &target) && can_create(ctx, &target) {
                create(
                    ctx,
                    &target.path("requirements-dev.txt"),
                    &dev_requirements(),
                    &mut result,
                )?;
            }
        }
        Ok(result)
    }
}
