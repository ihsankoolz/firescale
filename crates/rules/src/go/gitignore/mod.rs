//! `go/gitignore`: .gitignore keeps what the go tool writes out of git: the
//! root program `go build` writes, Windows programs, test binaries, coverage
//! profiles and local workspace files.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding, go_files, module_path, package_name, program_name, MODULE};
use crate::core::gitignore_basics::is_covered;

#[cfg(test)]
mod tests;

pub const ID: &str = "go/gitignore";

pub struct GoGitignore;

/// What must be ignored besides the root program: programs built for
/// Windows, test binaries (`go test -c`), coverage profiles, and the local
/// workspace files the Go team advises not to commit.
pub const ENTRIES: [&str; 5] = ["*.exe", "*.test", "*.out", "go.work", "go.work.sum"];

/// The lines missing from `text`: the root program first, when the root
/// package is `main` (`go build ./...` then writes it at the root).
fn missing(ctx: &RuleContext, text: &str) -> Result<Vec<String>> {
    let mut wanted = Vec::new();
    if let Some(program) = root_program(ctx)? {
        wanted.push(format!("/{program}"));
    }
    wanted.extend(ENTRIES.iter().map(|entry| (*entry).to_owned()));
    Ok(wanted
        .into_iter()
        .filter(|line| !is_covered(text, line.trim_start_matches('/')))
        .collect())
}

/// The program `go build ./...` writes at the root, when the root package
/// is `main` (or the module has no Go code yet: `go/entry` writes one).
fn root_program(ctx: &RuleContext) -> Result<Option<String>> {
    let files = go_files(ctx)?;
    let main_at_root = files.is_empty()
        || files
            .iter()
            .filter(|file| !file.contains('/') && !file.ends_with("_test.go"))
            .any(|file| {
                ctx.read(file)
                    .ok()
                    .flatten()
                    .is_some_and(|text| package_name(&text) == Some("main"))
            });
    if !main_at_root {
        return Ok(None);
    }
    let go_mod = ctx.read("go.mod")?;
    let module = go_mod.as_deref().map_or(Some(MODULE), module_path);
    Ok(module.map(|module| program_name(module).to_owned()))
}

impl BuiltinRule for GoGitignore {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: ".gitignore ignores the root program go build writes, *.exe, *.test, *.out, go.work and go.work.sum.".to_owned(),
            inputs: super::inputs(&[".gitignore", "*.go", "**/*.go"]),
            has_fix: true,
            after: vec!["core/gitignore-basics".to_owned(), "go/entry".to_owned()],
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
        let missing = missing(ctx, &text)?;
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            ".gitignore",
            format!(
                ".gitignore does not ignore {}: files the go tool writes, which do not belong in git. `fscale sync` adds {}.",
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
        let missing = missing(ctx, &current)?;
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
        let mut block = vec!["# Go".to_owned()];
        block.extend(missing);
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
