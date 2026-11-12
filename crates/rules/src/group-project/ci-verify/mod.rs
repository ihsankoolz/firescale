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

/// Words that only start another program: `npx`, `pnpm exec`, `yarn dlx`.
const RUNNERS: [&str; 10] = [
    "npx", "pnpm", "yarn", "bunx", "bun", "npm", "exec", "dlx", "x", "run",
];

/// True when one shell command (no `&&`, `;` or `|` in it) starts firescale's
/// verify: optional variable assignments and a runner, then `firescale` or
/// `fscale` (also `firescale@0.1.2` or a path ending in one), then, after
/// any options, the word `verify`.
fn command_runs_verify(command: &str) -> bool {
    let mut words = command.split_whitespace().peekable();
    // FOO=1 npx ...
    while words
        .peek()
        .is_some_and(|word| word.contains('=') && !word.starts_with('-'))
    {
        words.next();
    }
    let mut named = false;
    for word in words.by_ref() {
        if word.starts_with('-') || RUNNERS.contains(&word) {
            continue;
        }
        let name = word.trim_matches(['"', '\'']);
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let name = name.split('@').next().unwrap_or(name);
        named = matches!(
            name,
            "firescale" | "fscale" | "fscale.exe" | "firescale.cmd" | "fscale.cmd"
        );
        break;
    }
    if !named {
        return false;
    }
    let mut takes_value = false;
    for word in words {
        if takes_value {
            takes_value = false;
            continue;
        }
        if word == "--cwd" {
            takes_value = true;
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        return word == "verify";
    }
    false
}

/// True when a line of shell runs firescale's verify in one of its commands.
fn runs_verify(line: &str) -> bool {
    strip_comment(line)
        .split([';', '&', '|'])
        .any(command_runs_verify)
}

/// The shell lines of the `run:` steps of a workflow: the value of a
/// one-line `run:`, and every line of a `run: |` or `run: >` block.
fn run_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let trimmed = line.trim_start();
        let entry = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim_start();
        let Some(value) = entry.strip_prefix("run:") else {
            continue;
        };
        let value = value.trim();
        if value.starts_with('|') || value.starts_with('>') {
            let key_indent = line.len() - entry.len();
            let mut block = Vec::new();
            while index < lines.len() {
                let next = lines[index];
                let indent = next.len() - next.trim_start().len();
                if !next.trim().is_empty() && indent <= key_indent {
                    break;
                }
                block.push(next.trim().to_owned());
                index += 1;
            }
            // `>` folds the block's lines into one; `|` keeps them apart.
            if value.starts_with('>') {
                out.push(block.join(" "));
            } else {
                out.extend(block);
            }
        } else {
            let unquoted = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            out.push(unquoted.to_owned());
        }
    }
    out
}

/// True when the workflow file runs on pull requests and one of its `run:`
/// steps runs `fscale verify`.
pub(crate) fn verifies_pull_requests(text: &str) -> bool {
    mentions_pull_request(text) && run_lines(text).iter().any(|line| runs_verify(line))
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
