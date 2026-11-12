//! `python/pytest-config`: each Python project with tests configures pytest,
//! so `python -m pytest` finds the tests and imports the project's code.

use firescale_core::model::{Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{create, finding, targets, Target, INPUTS};

#[cfg(test)]
mod tests;

pub const ID: &str = "python/pytest-config";

pub struct PytestConfig;

/// pytest.ini for a project with a tests folder. `pythonpath = .` lets tests
/// import the project's own packages (pytest 7 and later).
pub const PYTEST_INI: &str = "[pytest]\ntestpaths = tests\npythonpath = .\naddopts = -ra\n";

/// pytest.ini for a project whose tests are beside its code.
pub const PYTEST_INI_FLAT: &str = "[pytest]\npythonpath = .\naddopts = -ra\n";

fn configured(ctx: &RuleContext, target: &Target) -> bool {
    target.exists(ctx, "pytest.ini")
        || target.read(ctx, "pyproject.toml").is_some_and(|t| t.contains("[tool.pytest"))
        || target.read(ctx, "setup.cfg").is_some_and(|t| t.contains("[tool:pytest]"))
        || target.read(ctx, "tox.ini").is_some_and(|t| t.contains("[pytest]"))
}

impl BuiltinRule for PytestConfig {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "Each Python project configures pytest (pytest.ini, or pytest settings in pyproject.toml, setup.cfg or tox.ini).".to_owned(),
            inputs: INPUTS.iter().map(|input| (*input).to_owned()).collect(),
            has_fix: true,
            after: vec![super::tests_present::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for target in targets(ctx)? {
            if configured(ctx, &target) {
                continue;
            }
            findings.push(finding(
                &target,
                "pytest.ini",
                "pytest is not configured, so it searches from wherever it is started and the tests may not import the project's code. `fscale sync` creates pytest.ini with `pythonpath = .`.".to_owned(),
                Fixable::Deterministic,
            ));
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        for target in targets(ctx)? {
            if configured(ctx, &target) {
                continue;
            }
            let content = if target.is_new() || target.exists(ctx, "tests") {
                PYTEST_INI
            } else {
                PYTEST_INI_FLAT
            };
            create(ctx, &target.path("pytest.ini"), content, &mut result)?;
        }
        Ok(result)
    }
}
