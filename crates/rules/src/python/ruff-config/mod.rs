//! `python/ruff-config`: a Python project that uses ruff has its config, in
//! its folder or a folder above.

use firescale_core::facts::python_declared;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{create, finding, targets, Target, INPUTS};

#[cfg(test)]
mod tests;

pub const ID: &str = "python/ruff-config";

pub struct RuffConfig;

pub const RUFF_TOML: &str = "line-length = 100\n\n[lint]\nselect = [\"E\", \"F\", \"W\", \"I\"]\n";

fn uses_ruff(ctx: &RuleContext, target: &Target) -> bool {
    target.is_new() || python_declared(ctx.root(), &target.folder).contains("ruff")
}

/// True when the folder or one above it, up to the root, configures ruff.
fn configured(ctx: &RuleContext, target: &Target) -> bool {
    let mut parts: Vec<&str> = target.folder.split('/').filter(|p| *p != ".").collect();
    loop {
        let dir = parts
            .iter()
            .fold(ctx.root().to_path_buf(), |path, part| path.join(part));
        if dir.join("ruff.toml").is_file()
            || dir.join(".ruff.toml").is_file()
            || std::fs::read_to_string(dir.join("pyproject.toml"))
                .is_ok_and(|t| t.contains("[tool.ruff"))
        {
            return true;
        }
        if parts.pop().is_none() {
            return false;
        }
    }
}

impl BuiltinRule for RuffConfig {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Consistency,
            description:
                "A Python project that uses ruff has a ruff config in its folder or a folder above."
                    .to_owned(),
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
            if !uses_ruff(ctx, &target) || configured(ctx, &target) {
                continue;
            }
            if target.blocked(ctx, "ruff.toml") {
                findings.push(finding(
                    &target,
                    "ruff.toml",
                    "ruff.toml is there but is not a file (a folder or a link). Remove it or replace it with a file, then run `fscale sync`.".to_owned(),
                    Fixable::Manual,
                ));
                continue;
            }
            findings.push(finding(
                &target,
                "ruff.toml",
                "ruff is used but not configured, so every machine lints with its own defaults. `fscale sync` creates ruff.toml with the line length and the rule sets.".to_owned(),
                Fixable::Deterministic,
            ));
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        for target in targets(ctx)? {
            if uses_ruff(ctx, &target)
                && !configured(ctx, &target)
                && !target.blocked(ctx, "ruff.toml")
            {
                create(ctx, &target.path("ruff.toml"), RUFF_TOML, &mut result)?;
            }
        }
        Ok(result)
    }
}
