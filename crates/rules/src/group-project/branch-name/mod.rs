//! `group-project/branch-name`: the current branch's name matches the team's
//! patterns.

use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::{Error, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use super::branch_patterns;

#[cfg(test)]
mod tests;

pub const ID: &str = "group-project/branch-name";

/// The variable GitHub Actions sets to a pull request's source branch, on
/// `pull_request` and `pull_request_target` runs, where the checkout itself
/// has no branch.
pub const HEAD_REF_VAR: &str = "GITHUB_HEAD_REF";

pub struct BranchName;

fn matcher(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|error| {
            Error::user(format!(
                "{ID}: option \"branches\" has a pattern that is not valid, \"{pattern}\" ({error}). Fix it in your firescale config."
            ))
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|error| {
        Error::user(format!(
            "{ID}: option \"branches\" could not be read ({error}). Fix it in your firescale config."
        ))
    })
}

/// The branch to judge: the checked-out branch, else (detached HEAD, as in
/// CI) the pull request's branch from `head_ref`. `None` outside git, or
/// when neither says.
fn branch(ctx: &RuleContext, head_ref: Option<String>) -> Result<Option<String>> {
    if !ctx.git().is_repo()? {
        return Ok(None);
    }
    if let Some(name) = ctx.git().current_branch()? {
        return Ok(Some(name));
    }
    Ok(head_ref
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty()))
}

/// The check, with the pull request branch given rather than read from the
/// environment.
pub(crate) fn check_with(ctx: &RuleContext, head_ref: Option<String>) -> Result<Vec<Finding>> {
    let patterns = branch_patterns(ctx.options(), ID)?;
    let set = matcher(&patterns)?;
    let Some(name) = branch(ctx, head_ref)? else {
        return Ok(Vec::new());
    };
    if set.is_match(&name) {
        return Ok(Vec::new());
    }
    let shown: Vec<String> = patterns.iter().map(|p| format!("`{p}`")).collect();
    let example = patterns
        .iter()
        .find(|pattern| pattern.ends_with("/*"))
        .map(|pattern| {
            let tail = name.rsplit('/').next().unwrap_or(&name);
            format!(
                " For example: `git branch -m {}{tail}`.",
                pattern.trim_end_matches('*')
            )
        })
        .unwrap_or_default();
    Ok(vec![Finding {
        message: format!(
            "The branch \"{name}\" does not match the team's branch names: {}. Rename it with `git branch -m <new-name>` before you push.{example}",
            shown.join(", ")
        ),
        file: ".git/HEAD".to_owned(),
        range: None,
        project: None,
        fixable: Fixable::Manual,
        severity: None,
    }])
}

impl BuiltinRule for BranchName {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Consistency,
            description: "The current branch's name matches the team's branch patterns.".to_owned(),
            // No inputs: the branch is not a file, so the result is never cached.
            inputs: Vec::new(),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        check_with(ctx, std::env::var(HEAD_REF_VAR).ok())
    }
}
