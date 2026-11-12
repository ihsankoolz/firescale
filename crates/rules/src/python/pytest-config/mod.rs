//! `python/pytest-config`: each Python project with tests configures pytest,
//! so `python -m pytest` finds the tests and imports the project's code.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::tests_present::has_tests;
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

/// True when the project's folder, or a folder above it up to the root,
/// configures pytest: pytest looks upward for its config too.
fn configured(ctx: &RuleContext, target: &Target) -> bool {
    let mut parts: Vec<&str> = target.folder.split('/').filter(|p| *p != ".").collect();
    loop {
        let dir = parts
            .iter()
            .fold(ctx.root().to_path_buf(), |path, part| path.join(part));
        let has = |file: &str, needle: &str| {
            std::fs::read_to_string(dir.join(file)).is_ok_and(|text| text.contains(needle))
        };
        if dir.join("pytest.ini").is_file()
            || dir.join("pytest.toml").is_file()
            || dir.join(".pytest.toml").is_file()
            || has("pyproject.toml", "[tool.pytest")
            || has("setup.cfg", "[tool:pytest]")
            || has("tox.ini", "[pytest]")
        {
            return true;
        }
        if parts.pop().is_none() {
            return false;
        }
    }
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
            // Configured, or nothing to test yet (python/tests reports that).
            if configured(ctx, &target) || !(target.is_new() || has_tests(ctx, &target)) {
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
            // Configured, or nothing to test yet (python/tests reports that).
            if configured(ctx, &target) || !(target.is_new() || has_tests(ctx, &target)) {
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
