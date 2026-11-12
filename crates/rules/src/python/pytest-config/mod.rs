//! `python/pytest-config`: each Python project with tests configures pytest,
//! so `python -m pytest` finds the tests and imports the project's code.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::tests_present::has_tests;
use super::{create, finding, target_folders, targets, Target, INPUTS};

#[cfg(test)]
mod tests;

pub const ID: &str = "python/pytest-config";

pub struct PytestConfig;

/// pytest.ini for a project with a tests folder. `pythonpath = .` lets tests
/// import the project's own packages (pytest 7 and later).
pub const PYTEST_INI: &str = "[pytest]\ntestpaths = tests\npythonpath = .\naddopts = -ra\n";

/// pytest.ini for a project with a src/ layout.
pub const PYTEST_INI_SRC: &str = "[pytest]\ntestpaths = tests\npythonpath = src\naddopts = -ra\n";

/// pytest.ini for a project whose tests are beside its code.
pub const PYTEST_INI_FLAT: &str = "[pytest]\npythonpath = .\naddopts = -ra\n";

/// True when the project's folder, or a folder above it up to the root,
/// configures pytest: pytest looks upward for its config too. The climb
/// stops at another project's folder, whose config (with its own
/// `pythonpath`) is not this project's (review of M16).
fn configured(ctx: &RuleContext, target: &Target, projects: &[String]) -> bool {
    let mut parts: Vec<&str> = target.folder.split('/').filter(|p| *p != ".").collect();
    let mut first = true;
    loop {
        let here = if parts.is_empty() {
            ".".to_owned()
        } else {
            parts.join("/")
        };
        if !first && projects.contains(&here) {
            return false;
        }
        first = false;
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
        let projects = target_folders(ctx)?;
        for target in targets(ctx)? {
            // Configured, or nothing to test yet (python/tests reports that).
            if configured(ctx, &target, &projects) || !(target.is_new() || has_tests(ctx, &target))
            {
                continue;
            }
            if target.blocked(ctx, "pytest.ini") {
                findings.push(finding(
                    &target,
                    "pytest.ini",
                    "pytest.ini is there but is not a file (a folder or a link). Remove it or replace it with a file, then run `fscale sync`.".to_owned(),
                    Fixable::Manual,
                ));
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
        let projects = target_folders(ctx)?;
        for target in targets(ctx)? {
            if configured(ctx, &target, &projects)
                || !(target.is_new() || has_tests(ctx, &target))
                || target.blocked(ctx, "pytest.ini")
            {
                continue;
            }
            // A src/ layout imports from src/, not from the project's folder.
            let content = match (
                target.exists(ctx, "src"),
                target.is_new() || target.exists(ctx, "tests"),
            ) {
                (true, true) => PYTEST_INI_SRC.to_owned(),
                (true, false) => PYTEST_INI_SRC.replace("testpaths = tests\n", ""),
                (false, true) => PYTEST_INI.to_owned(),
                (false, false) => PYTEST_INI_FLAT.to_owned(),
            };
            let content = content.as_str();
            create(ctx, &target.path("pytest.ini"), content, &mut result)?;
        }
        Ok(result)
    }
}
