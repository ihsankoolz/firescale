//! `expo/gitignore`: .gitignore keeps Expo's local state, local env files
//! and app-signing keys out of git, as Expo's templates do. `expo-doctor`
//! checks the first two.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding};
use crate::core::gitignore_basics::is_covered;

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/gitignore";

pub struct ExpoGitignore;

/// What must be ignored, each with the line the template writes and other
/// lines that cover it.
pub const ENTRIES: [(&str, &[&str]); 8] = [
    (".expo/", &[".expo"]),
    // What `expo export` writes.
    ("dist/", &["dist"]),
    (
        ".env*.local",
        &[".env*.local", ".env*", ".env.*", "*.local"],
    ),
    ("*.jks", &["*.jks"]),
    ("*.p8", &["*.p8"]),
    ("*.p12", &["*.p12"]),
    ("*.key", &["*.key"]),
    ("*.mobileprovision", &["*.mobileprovision"]),
];

fn missing(text: &str) -> Vec<&'static str> {
    ENTRIES
        .iter()
        .filter(|(_, covers)| !covers.iter().any(|name| is_covered(text, name)))
        .map(|(line, _)| *line)
        .collect()
}

impl BuiltinRule for ExpoGitignore {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Security,
            description: ".gitignore ignores .expo/, dist/, .env*.local and app-signing keys (*.jks, *.p8, *.p12, *.key, *.mobileprovision).".to_owned(),
            inputs: vec![".gitignore".to_owned(), "package.json".to_owned()],
            has_fix: true,
            after: vec!["core/gitignore-basics".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let text = ctx.read(".gitignore")?.unwrap_or_default();
        let missing = missing(&text);
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            ".gitignore",
            format!(
                ".gitignore does not ignore {}: Expo's local state, local env files or app-signing keys, which Expo's templates keep out of git. `fscale sync` adds {}.",
                missing.join(", "),
                if missing.len() == 1 { "it" } else { "them" }
            ),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        if !applies(ctx)? {
            return Ok(FixResult::default());
        }
        let current = ctx.read(".gitignore")?.unwrap_or_default();
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
        let mut block = vec!["# Expo".to_owned()];
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
