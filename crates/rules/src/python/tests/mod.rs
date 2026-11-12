//! `python/tests`: each Python project has tests; a new one gets a small
//! app package and a test for it.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{create, finding, targets, Target, INPUTS};

#[cfg(test)]
mod tests;

pub const ID: &str = "python/tests";

pub struct TestsPresent;

pub const APP_INIT: &str = "\"\"\"The app package.\"\"\"\n";
pub const APP_MAIN: &str = "\"\"\"The app's entry point. Replace it with the real code.\"\"\"\n\n\ndef greet(name: str) -> str:\n    \"\"\"Return a greeting for name.\"\"\"\n    return f\"Hello, {name}!\"\n\n\nif __name__ == \"__main__\":\n    print(greet(\"world\"))\n";
pub const TEST_MAIN: &str = "from app.main import greet\n\n\ndef test_greet() -> None:\n    assert greet(\"world\") == \"Hello, world!\"\n";

/// True when the project has a tests folder or a test file.
fn has_tests(ctx: &RuleContext, target: &Target) -> bool {
    if target.exists(ctx, "tests") || target.exists(ctx, "test") {
        return true;
    }
    std::fs::read_dir(
        ctx.root()
            .join(target.folder.replace('/', std::path::MAIN_SEPARATOR_STR)),
    )
    .is_ok_and(|entries| {
        entries.filter_map(std::result::Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
        })
    })
}

impl BuiltinRule for TestsPresent {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description:
                "Each Python project has tests; a new one gets app/ and tests/test_main.py."
                    .to_owned(),
            inputs: INPUTS.iter().map(|input| (*input).to_owned()).collect(),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for target in targets(ctx)? {
            if has_tests(ctx, &target) {
                continue;
            }
            findings.push(if target.is_new() {
                finding(&target, "tests/test_main.py", "No Python code or tests yet. `fscale sync` creates app/main.py and tests/test_main.py, which `python -m pytest` runs.".to_owned(), Fixable::Deterministic)
            } else {
                finding(&target, "tests/", format!("The Python project in {} has no tests (no tests/ folder and no test_*.py). Add tests for its code.", target.folder), Fixable::Agent)
            });
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        for target in targets(ctx)? {
            if target.is_new() && !has_tests(ctx, &target) {
                if !target.exists(ctx, "app") {
                    create(ctx, &target.path("app/__init__.py"), APP_INIT, &mut result)?;
                    create(ctx, &target.path("app/main.py"), APP_MAIN, &mut result)?;
                }
                create(
                    ctx,
                    &target.path("tests/test_main.py"),
                    TEST_MAIN,
                    &mut result,
                )?;
            }
        }
        Ok(result)
    }
}
