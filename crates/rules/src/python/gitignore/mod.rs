//! `python/gitignore`: .gitignore keeps Python's caches and virtual
//! environments out of git.

use firescale_core::model::{Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use crate::core::gitignore_basics::is_covered;

#[cfg(test)]
mod tests;

pub const ID: &str = "python/gitignore";

pub struct PythonGitignore;

/// What Python, pytest, ruff and venv write.
pub const ENTRIES: [(&str, &str); 5] = [
    ("__pycache__", "__pycache__/"),
    (".venv", ".venv/"),
    ("venv", "venv/"),
    (".pytest_cache", ".pytest_cache/"),
    (".ruff_cache", ".ruff_cache/"),
];

fn missing(text: &str) -> Vec<&'static str> {
    ENTRIES
        .iter()
        .filter(|(name, _)| !is_covered(text, name))
        .map(|(_, line)| *line)
        .collect()
}

fn finding(message: String) -> Finding {
    Finding {
        message,
        file: ".gitignore".to_owned(),
        range: None,
        project: None,
        fixable: Fixable::Deterministic,
        severity: None,
    }
}

impl BuiltinRule for PythonGitignore {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: ".gitignore ignores __pycache__/, .venv/, venv/, .pytest_cache/ and .ruff_cache/.".to_owned(),
            inputs: vec![".gitignore".to_owned()],
            has_fix: true,
            after: vec!["core/gitignore-basics".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let text = ctx.read(".gitignore")?.unwrap_or_default();
        let missing = missing(&text);
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![finding(format!(
            ".gitignore does not ignore {}. `fscale sync` adds {}.",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" }
        ))])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let current = ctx.read(".gitignore")?.unwrap_or_default();
        let missing = missing(&current);
        if missing.is_empty() {
            return Ok(FixResult::default());
        }
        let eol = if current.contains("\r\n") { "\r\n" } else { "\n" };
        let separator = if current.is_empty() || current.ends_with('\n') { "" } else { eol };
        let mut block = vec!["# Python caches and virtual environments".to_owned()];
        block.extend(missing.iter().map(|line| (*line).to_owned()));
        ctx.write(".gitignore", &format!("{current}{separator}{}{eol}", block.join(eol)))?;
        Ok(FixResult {
            changed_files: vec![".gitignore".to_owned()],
            notes: Vec::new(),
        })
    }
}
