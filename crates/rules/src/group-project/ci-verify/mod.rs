//! `group-project/ci-verify`: CI runs `fscale verify` on pull requests.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use crate::core::ci_exists::{
    candidate_files, create_starter, is_workflow, mentions_pull_request, starter_blocker,
    strip_comment, STARTER,
};

#[cfg(test)]
mod tests;

pub const ID: &str = "group-project/ci-verify";

pub struct CiVerify;

/// True when a line runs firescale's verify: a word that names the
/// command (`firescale`, `fscale`, `firescale@0.1.2`, a path ending in one
/// of them) followed later on the line by the word `verify`.
fn runs_verify(line: &str) -> bool {
    let words: Vec<&str> = strip_comment(line)
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '&' | '|' | '('))
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().enumerate().any(|(index, word)| {
        let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
        let name = name.split('@').next().unwrap_or(name);
        matches!(
            name,
            "firescale" | "fscale" | "fscale.exe" | "firescale.cmd" | "fscale.cmd"
        ) && words[index + 1..].contains(&"verify")
    })
}

/// True when the workflow file runs on pull requests and has a step that
/// runs `fscale verify`.
pub(crate) fn verifies_pull_requests(text: &str) -> bool {
    mentions_pull_request(text) && text.lines().any(runs_verify)
}

/// What is wrong, as `(message, file, fixable)`, or `None` when CI runs
/// verify on pull requests.
fn problem(ctx: &RuleContext) -> Result<Option<(String, String, Fixable)>> {
    let files = candidate_files(ctx)?;
    let workflows: Vec<&String> = files.iter().filter(|file| is_workflow(file)).collect();
    for file in &workflows {
        if ctx
            .read(file)?
            .is_some_and(|text| verifies_pull_requests(&text))
        {
            return Ok(None);
        }
    }
    if let Some(first) = workflows.first() {
        // A workflow a person wrote is never rewritten.
        return Ok(Some((
            format!(
                "No workflow in .github/workflows runs `fscale verify` on pull requests. Add a step `- run: npx firescale verify` to a job in \"{first}\" that runs on `pull_request` (after the install step), so every pull request is checked the same way as on a teammate's machine."
            ),
            (*first).clone(),
            Fixable::Agent,
        )));
    }
    if !files.is_empty() {
        return Ok(Some((
            format!(
                "This repo's CI is not GitHub Actions, so firescale cannot tell whether it runs `fscale verify`. Run `npx firescale verify` on every pull request in \"{}\", then turn this rule off in the firescale config.",
                files[0]
            ),
            files[0].clone(),
            Fixable::Manual,
        )));
    }
    if let Some((message, fixable)) = starter_blocker(ctx)? {
        return Ok(Some((message, STARTER.to_owned(), fixable)));
    }
    Ok(Some((
        format!(
            "No CI workflow runs `fscale verify` on pull requests. `fscale fix` can add {STARTER}, which does."
        ),
        STARTER.to_owned(),
        Fixable::Deterministic,
    )))
}

impl BuiltinRule for CiVerify {
    fn meta(&self) -> RuleMeta {
        let mut inputs: Vec<String> = [".github/workflows/*.yml", ".github/workflows/*.yaml"]
            .iter()
            .map(|glob| (*glob).to_owned())
            .collect();
        inputs.extend(crate::core::ci_exists::CiExists.meta().inputs);
        inputs.sort();
        inputs.dedup();
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "CI runs `fscale verify` on pull requests.".to_owned(),
            inputs,
            has_fix: true,
            // core/ci-exists writes the same starter workflow. When both are
            // on, its fix runs first and this one finds nothing left to do.
            after: vec![crate::core::ci_exists::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        Ok(problem(ctx)?
            .map(|(message, file, fixable)| Finding {
                message,
                file,
                range: None,
                project: None,
                fixable,
                severity: None,
            })
            .into_iter()
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        // Only ever creates the starter where no CI config exists at all.
        if !matches!(problem(ctx)?, Some((_, _, Fixable::Deterministic))) {
            return Ok(FixResult::default());
        }
        create_starter(ctx)
    }
}
