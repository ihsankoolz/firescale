//! `agent-ready/single-test-command`: the tests run with one command.

use firescale_core::facts::detect_facts;
use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

#[cfg(test)]
mod tests;

pub const ID: &str = "agent-ready/single-test-command";

/// Files that can show a test command, in the order a finding is anchored to them.
const MANIFESTS: [&str; 3] = ["package.json", "pyproject.toml", "requirements.txt"];

pub struct SingleTestCommand;

/// Reads a root file leniently: anything unreadable is treated as absent.
fn text(ctx: &RuleContext, file: &str) -> Option<String> {
    std::fs::read(ctx.root().join(file))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// True when a Makefile has a `test` target.
fn makefile_has_test(makefile: &str) -> bool {
    makefile.lines().any(|line| {
        line.strip_prefix("test")
            .is_some_and(|rest| rest.trim_start().starts_with(':') && !rest.contains(":="))
    })
}

/// True when the repo configures pytest, so `pytest` alone runs the tests.
fn has_pytest_config(ctx: &RuleContext) -> bool {
    ctx.root().join("pytest.ini").is_file()
        || text(ctx, "pyproject.toml").is_some_and(|toml| toml.contains("[tool.pytest"))
}

impl BuiltinRule for SingleTestCommand {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "The tests run with one command.".to_owned(),
            inputs: [
                "package.json",
                "Cargo.toml",
                "pyproject.toml",
                "requirements.txt",
                "pytest.ini",
                "Makefile",
                "tsconfig.json",
            ]
            .iter()
            .map(|input| (*input).to_owned())
            .collect(),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let facts = detect_facts(ctx.root())?;
        let has_test_command = facts.commands.test.is_some()
            || ctx.root().join("Cargo.toml").is_file()
            || has_pytest_config(ctx)
            || text(ctx, "Makefile").is_some_and(|makefile| makefile_has_test(&makefile));
        // With no detected language there is nothing to say what "tests" would mean.
        if has_test_command || facts.languages.is_empty() {
            return Ok(Vec::new());
        }
        let file = MANIFESTS
            .iter()
            .find(|file| ctx.root().join(file).is_file())
            .copied()
            .unwrap_or(MANIFESTS[0]);
        Ok(vec![Finding {
            message: "No single command runs this repo's tests. Add a \"test\" script to package.json, a pytest section to pyproject.toml, or a `test` target to a Makefile, so that people, CI and agents all run the tests the same way."
                .to_owned(),
            file: file.to_owned(),
            range: None,
            project: None,
            fixable: Fixable::Agent,
        }])
    }
}
