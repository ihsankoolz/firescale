//! `group-project/pr-template`: a pull request template exists.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{
    community_files, community_inputs, counted, create_blocker, entries, in_folder, on_disk,
    COMMUNITY_FOLDERS,
};

#[cfg(test)]
mod tests;

pub const ID: &str = "group-project/pr-template";

/// The only file the fix creates.
pub const TEMPLATE: &str = ".github/pull_request_template.md";

/// The template the fix writes. LF line endings.
pub const TEMPLATE_TEXT: &str = "\
<!-- Created by firescale (group-project/pr-template). It is your team's to edit. -->

## What this changes

<!-- One or two sentences. Link the issue it closes, for example: Closes #12 -->

## How to test it

<!-- The steps a reviewer can follow to see it work. -->

## Checklist

- [ ] `npx firescale verify` passes on my machine.
- [ ] The branch is named as CONTRIBUTING.md says.
- [ ] Tests cover the change, or I wrote above why they do not.
- [ ] A teammate is asked to review.
";

pub struct PrTemplate;

/// `pull_request_template` with any extension, in any letter case.
fn is_template_name(lower: &str) -> bool {
    lower == "pull_request_template"
        || lower
            .strip_prefix("pull_request_template.")
            .is_some_and(|extension| !extension.is_empty() && !extension.contains('.'))
}

/// Templates on disk: a single template in a community folder, or any file
/// in a `PULL_REQUEST_TEMPLATE` folder there (several templates, chosen with
/// the `template` query parameter).
fn templates_on_disk(ctx: &RuleContext) -> Vec<String> {
    let root = ctx.root();
    let mut found = community_files(root, is_template_name);
    for dir in COMMUNITY_FOLDERS {
        for name in entries(root, dir) {
            if name.to_lowercase() != "pull_request_template" {
                continue;
            }
            let folder = in_folder(dir, &name);
            if !on_disk(root, &folder).is_dir() {
                continue;
            }
            found.extend(
                entries(root, &folder)
                    .into_iter()
                    .map(|file| format!("{folder}/{file}"))
                    .filter(|file| on_disk(root, file).is_file()),
            );
        }
    }
    found
}

/// What is wrong, as `(message, file, fixable)`, or `None` when a template
/// counts.
fn problem(ctx: &RuleContext) -> Result<Option<(String, String, Fixable)>> {
    let found = templates_on_disk(ctx);
    if !counted(ctx, &found)?.is_empty() {
        return Ok(None);
    }
    if let Some(untracked) = found.first() {
        return Ok(Some((
            format!(
                "\"{untracked}\" exists but is not tracked by git, so teammates do not get it. Run `git add {untracked}` and commit it."
            ),
            untracked.clone(),
            Fixable::Manual,
        )));
    }
    if let Some((message, fixable)) = create_blocker(ctx, TEMPLATE, "pull request template")? {
        return Ok(Some((message, TEMPLATE.to_owned(), fixable)));
    }
    Ok(Some((
        format!(
            "No pull request template. Add {TEMPLATE} so every pull request says what it changes and how to test it; `fscale fix` can create one."
        ),
        TEMPLATE.to_owned(),
        Fixable::Deterministic,
    )))
}

impl BuiltinRule for PrTemplate {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Consistency,
            description: "A pull request template exists.".to_owned(),
            // Every place GitHub reads a template from, in any letter case.
            inputs: community_inputs("pull_request_template"),
            has_fix: true,
            after: Vec::new(),
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
        // Look at the disk again: only create the file where nothing is.
        if !matches!(problem(ctx)?, Some((_, _, Fixable::Deterministic))) {
            return Ok(FixResult::default());
        }
        ctx.create_tracked(TEMPLATE, TEMPLATE_TEXT)?;
        Ok(FixResult {
            changed_files: vec![TEMPLATE.to_owned()],
            notes: vec![format!(
                "Review {TEMPLATE}: it is the text every new pull request starts with. Change the checklist to what your team agreed."
            )],
        })
    }
}
