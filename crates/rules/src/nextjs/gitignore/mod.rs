//! `nextjs/gitignore`: .gitignore keeps Next.js's build output out of git.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::finding;
use crate::core::gitignore_basics::is_covered;

pub const ID: &str = "nextjs/gitignore";

pub struct NextGitignore;

/// What a Next.js build writes: the build folder and the generated types.
pub const ENTRIES: [(&str, &str); 2] = [(".next", ".next/"), ("next-env.d.ts", "next-env.d.ts")];

fn missing(text: &str) -> Vec<&'static str> {
    ENTRIES
        .iter()
        .filter(|(name, _)| !is_covered(text, name))
        .map(|(_, line)| *line)
        .collect()
}

impl BuiltinRule for NextGitignore {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: ".gitignore ignores .next/ and next-env.d.ts.".to_owned(),
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
        Ok(vec![finding(
            ".gitignore",
            format!(
                ".gitignore does not ignore {}, which `next build` writes. `fscale sync` adds {}.",
                missing.join(" and "),
                if missing.len() == 1 { "it" } else { "them" }
            ),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let existing = ctx.read(".gitignore")?;
        let current = existing.clone().unwrap_or_default();
        let missing = missing(&current);
        if missing.is_empty() {
            return Ok(FixResult::default());
        }
        let eol = if current.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let separator = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            eol
        };
        let mut block = vec!["# Next.js build output".to_owned()];
        block.extend(missing.iter().map(|line| (*line).to_owned()));
        ctx.write(
            ".gitignore",
            &format!("{current}{separator}{}{eol}", block.join(eol)),
        )?;
        Ok(FixResult {
            changed_files: vec![".gitignore".to_owned()],
            notes: Vec::new(),
        })
    }
}
