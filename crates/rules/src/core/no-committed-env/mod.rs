//! `core/no-committed-env`: no `.env` files are tracked by git.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, Options, RuleContext};
use firescale_core::{Error, Result};
use serde_json::Value;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/no-committed-env";

/// Example files that are meant to be committed. Override with the `allow` option.
const DEFAULT_ALLOW: [&str; 1] = [".env.example"];

const GITIGNORE: &str = ".gitignore";

/// Ignore every env file at any depth.
const IGNORE_LINES: [&str; 2] = [".env", ".env.*"];

pub struct NoCommittedEnv;

fn basename(file: &str) -> &str {
    file.rsplit('/').next().unwrap_or(file)
}

/// True for `.env` and `.env.<anything>` in any letter case, because Windows
/// and macOS treat `.ENV` and `.env` as the same file. Not for `.envrc`.
fn is_env_file(file: &str) -> bool {
    let name = basename(file).to_lowercase();
    name == ".env" || (name.starts_with(".env.") && name.len() > ".env.".len())
}

fn allow_list(options: &Options) -> Result<Vec<String>> {
    let Some(allow) = options.get("allow") else {
        return Ok(DEFAULT_ALLOW
            .iter()
            .map(|name| (*name).to_owned())
            .collect());
    };
    let names: Option<Vec<String>> = allow.as_array().and_then(|items| {
        items
            .iter()
            .map(|item| match item {
                Value::String(name) if !name.contains(['/', '\\']) && is_env_file(name) => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect()
    });
    names.ok_or_else(|| {
        Error::user(
            "core/no-committed-env: option \"allow\" must be a list of env file names without folders, for example [\".env.example\", \".env.template\"]. Fix it in your firescale config.",
        )
    })
}

fn is_allowed(file: &str, allow: &[String]) -> bool {
    let name = basename(file);
    allow
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// Works out which lines to append to `.gitignore` so that env files are
/// ignored and the allowed examples are not. In a gitignore the last matching
/// line wins, so each `!<example>` line must come after the ignore lines: one
/// that sits before them is added again at the end.
///
/// `.gitignore` matching is case-sensitive on Linux, so a flagged file whose
/// name is not lowercase (`.ENV`) gets its own exact line.
fn lines_to_append(gitignore: &str, allow: &[String], flagged: &[String]) -> Vec<String> {
    let lines: Vec<&str> = gitignore
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim_end)
        .collect();
    let position = |line: &str| lines.iter().rposition(|existing| *existing == line);

    let mut ignore: Vec<String> = IGNORE_LINES.iter().map(|line| (*line).to_owned()).collect();
    for file in flagged {
        let name = basename(file);
        let covered = name == ".env" || name.starts_with(".env.");
        if !covered && !ignore.iter().any(|line| line == name) {
            ignore.push(name.to_owned());
        }
    }

    let missing: Vec<String> = ignore
        .iter()
        .filter(|line| position(line).is_none())
        .cloned()
        .collect();
    // When an ignore line is about to be appended, every negation must follow it.
    let last_ignore = if missing.is_empty() {
        ignore.iter().filter_map(|line| position(line)).max()
    } else {
        Some(usize::MAX)
    };
    let negations = allow.iter().map(|name| format!("!{name}")).filter(|line| {
        match (position(line), last_ignore) {
            (Some(at), Some(last)) => at < last,
            (None, _) => true,
            (Some(_), None) => false,
        }
    });
    missing.iter().cloned().chain(negations).collect()
}

impl BuiltinRule for NoCommittedEnv {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Blocker,
            category: Category::Security,
            description:
                "No .env files are tracked by git, except example files such as .env.example."
                    .to_owned(),
            inputs: vec![
                "**/.env".to_owned(),
                "**/.env.*".to_owned(),
                GITIGNORE.to_owned(),
            ],
            has_fix: true,
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let allow = allow_list(ctx.options())?;
        Ok(ctx
            .git()
            .tracked_files()?
            .into_iter()
            .filter(|file| is_env_file(file) && !is_allowed(file, &allow))
            .map(|file| Finding {
                message: format!(
                    "\"{file}\" is tracked by git. Env files usually hold secrets and must not be committed."
                ),
                file,
                range: None,
                project: None,
                fixable: Fixable::Deterministic,
            })
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let allow = allow_list(ctx.options())?;
        let files: Vec<String> = diagnostics.iter().map(|d| d.file.clone()).collect();
        ctx.git().untrack(&files)?;
        let mut changed_files = files.clone();

        let current = ctx.read(GITIGNORE)?.unwrap_or_default();
        let eol = if current.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let missing = lines_to_append(&current, &allow, &files);
        if !missing.is_empty() {
            let separator = if current.is_empty() || current.ends_with('\n') {
                ""
            } else {
                eol
            };
            let mut block =
                vec!["# Env files hold secrets. Commit an example file instead.".to_owned()];
            block.extend(missing);
            ctx.write(
                GITIGNORE,
                &format!("{current}{separator}{}{eol}", block.join(eol)),
            )?;
            changed_files.push(GITIGNORE.to_owned());
        }

        let notes = if files.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "Untracked {} env file(s). They are still on disk and still in git history: rotate any secret they contained.",
                files.len()
            )]
        };
        Ok(FixResult {
            changed_files,
            notes,
        })
    }
}
