//! `core/gitignore-basics`: .gitignore exists and covers the detected stack.

use firescale_core::facts::detect_facts;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/gitignore-basics";

const GITIGNORE: &str = ".gitignore";

pub struct GitignoreBasics;

/// Entries every repo of a detected language should ignore.
fn required_entries(languages: &[String]) -> Vec<&'static str> {
    let has = |name: &str| languages.iter().any(|language| language == name);
    let mut entries = Vec::new();
    if has("javascript") || has("typescript") {
        entries.push("node_modules");
    }
    if has("rust") {
        entries.push("target");
    }
    if has("python") {
        entries.push("__pycache__");
        entries.push(".venv");
    }
    entries
}

/// Reduces a `.gitignore` line to a bare name: `/node_modules/`, `**/node_modules/**` -> `node_modules`.
fn normalise(line: &str) -> String {
    let mut text = line.trim();
    if let Some(rest) = text.strip_prefix("**/") {
        text = rest;
    }
    if let Some(rest) = text.strip_prefix('/') {
        text = rest;
    }
    for suffix in ["/**", "/*", "/"] {
        if let Some(rest) = text.strip_suffix(suffix) {
            text = rest;
            break;
        }
    }
    text.to_owned()
}

/// Covered when the LAST line matching the entry is not a negation (`!entry`).
pub(crate) fn is_covered(gitignore: &str, entry: &str) -> bool {
    let mut covered = false;
    for line in gitignore
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        match line.strip_prefix('!') {
            Some(rest) if normalise(rest) == entry => covered = false,
            Some(_) => {}
            None if normalise(line) == entry => covered = true,
            None => {}
        }
    }
    covered
}

fn missing_entries(ctx: &RuleContext, gitignore: &str) -> Result<Vec<&'static str>> {
    let facts = detect_facts(ctx.root())?;
    Ok(required_entries(&facts.languages)
        .into_iter()
        .filter(|entry| !is_covered(gitignore, entry))
        .collect())
}

impl BuiltinRule for GitignoreBasics {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: ".gitignore exists and covers the detected stack.".to_owned(),
            // The stack comes from every project `fscale facts` finds, so the
            // marker files count wherever they are.
            inputs: [
                ".gitignore",
                "package.json",
                "Cargo.toml",
                "pyproject.toml",
                "requirements.txt",
                "tsconfig.json",
                "**/package.json",
                "**/Cargo.toml",
                "**/pyproject.toml",
                "**/requirements.txt",
                "**/tsconfig.json",
                "**/setup.py",
                "**/Pipfile",
                "**/go.mod",
                "go.mod",
                "setup.py",
                "Pipfile",
                // What decides whether a folder is searched, and whether a
                // requirements.txt in it makes a Python project.
                "**/.gitignore",
                "pnpm-workspace.yaml",
                "**/*.py",
                "**/pytest.ini",
                "**/tox.ini",
                "**/setup.cfg",
                "**/ruff.toml",
                "**/.ruff.toml",
                "**/mypy.ini",
                "**/alembic.ini",
            ]
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !ctx.git().is_repo()? {
            return Ok(Vec::new());
        }
        let finding = |message: String| Finding {
            message,
            file: GITIGNORE.to_owned(),
            range: None,
            project: None,
            fixable: Fixable::Deterministic,
            severity: None,
        };
        let Some(current) = ctx.read(GITIGNORE)? else {
            return Ok(vec![finding(
                "There is no .gitignore at the repo root. Create one that ignores build output and dependencies; `fscale fix` can do it."
                    .to_owned(),
            )]);
        };
        Ok(missing_entries(ctx, &current)?
            .into_iter()
            .map(|entry| {
                finding(format!(
                    ".gitignore does not ignore \"{entry}\". Add \"{entry}/\" to .gitignore so generated files are not committed."
                ))
            })
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let existing = ctx.read(GITIGNORE)?;
        let current = existing.clone().unwrap_or_default();
        let missing = missing_entries(ctx, &current)?;
        if existing.is_some() && missing.is_empty() {
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
        // The header once: a first pass with nothing to list (no package.json
        // yet) already wrote it.
        let header = "# Generated files and dependencies";
        let mut block = Vec::new();
        if !current.lines().any(|line| line.trim() == header) {
            block.push(header.to_owned());
        }
        block.extend(missing.iter().map(|entry| format!("{entry}/")));
        ctx.write(
            GITIGNORE,
            &format!("{current}{separator}{}{eol}", block.join(eol)),
        )?;
        Ok(FixResult {
            changed_files: vec![GITIGNORE.to_owned()],
            notes: Vec::new(),
        })
    }
}
