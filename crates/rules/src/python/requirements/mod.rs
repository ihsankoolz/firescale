//! `python/requirements`: each Python project declares its dependencies; a
//! new project gets `requirements.txt` and `requirements-dev.txt`.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{create, finding, targets, Target, INPUTS, PYTEST, RUFF};

#[cfg(test)]
mod tests;

pub const ID: &str = "python/requirements";

pub struct Requirements;

pub const REQUIREMENTS: &str =
    "# What the app needs at run time, one package per line, pinned with ==.\n";

/// The development list: the app's own, plus the test and lint tools.
#[must_use]
pub fn dev_requirements() -> String {
    format!("-r requirements.txt\n{PYTEST}\n{RUFF}\n")
}

const DECLARATIONS: [&str; 5] = [
    "requirements.txt",
    "pyproject.toml",
    "Pipfile",
    "setup.py",
    "setup.cfg",
];

fn declares_anything(ctx: &RuleContext, target: &Target) -> bool {
    DECLARATIONS.iter().any(|file| target.exists(ctx, file))
        || std::fs::read_dir(
            ctx.root()
                .join(target.folder.replace('/', std::path::MAIN_SEPARATOR_STR)),
        )
        .is_ok_and(|entries| {
            entries.filter_map(std::result::Result::ok).any(|entry| {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                name.starts_with("requirements") && name.ends_with(".txt")
            })
        })
}

impl BuiltinRule for Requirements {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "Each Python project declares its dependencies; a new one gets requirements.txt and requirements-dev.txt.".to_owned(),
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
            if declares_anything(ctx, &target) {
                continue;
            }
            let (message, fixable) = if target.is_new() {
                (
                    "No Python dependency file. `fscale sync` creates requirements.txt for the app and requirements-dev.txt with pytest and ruff.".to_owned(),
                    Fixable::Deterministic,
                )
            } else {
                (
                    format!("The Python project in {} declares no dependencies. Add a requirements.txt (or a pyproject.toml) that lists what it imports.", target.folder),
                    Fixable::Agent,
                )
            };
            findings.push(finding(&target, "requirements.txt", message, fixable));
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        for target in targets(ctx)? {
            if target.is_new() && !declares_anything(ctx, &target) {
                create(
                    ctx,
                    &target.path("requirements.txt"),
                    REQUIREMENTS,
                    &mut result,
                )?;
                create(
                    ctx,
                    &target.path("requirements-dev.txt"),
                    &dev_requirements(),
                    &mut result,
                )?;
                result.notes.push("Make a virtual environment and install: `python -m venv .venv`, then `.venv/bin/python -m pip install -r requirements-dev.txt` (on Windows `.venv\\Scripts\\python.exe`).".to_owned());
            }
        }
        Ok(result)
    }
}
