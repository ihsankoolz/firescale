//! `agent-ready/agent-permissions`: each agent's project settings keep env
//! files and keys out of its reach and allow the repo's own check commands
//! (SPEC §16, ADR 0027).
//!
//! - Claude Code: `permissions.deny` and `permissions.allow` in
//!   `.claude/settings.json`.
//! - Codex: a permission profile in `.codex/config.toml` that denies the
//!   same files in its sandbox.
//! - Cursor: `permissions.deny` in `.cursor/cli.json` (format unconfirmed:
//!   cursor.com could not be read from the build session).
//!
//! Existing settings are kept: entries are added as new lines and nothing
//! else in the file changes. A file that cannot take them that way is left
//! for a person.

use firescale_core::facts::detect_facts_with;
use firescale_core::jsonedit::{holds, insert_merge, merged};
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::{json, Value};

use super::managed::{fscale_command, stable_command};

#[cfg(test)]
mod tests;

pub const ID: &str = "agent-ready/agent-permissions";

pub const CLAUDE_SETTINGS: &str = ".claude/settings.json";
pub const CODEX_CONFIG: &str = ".codex/config.toml";
pub const CURSOR_CLI: &str = ".cursor/cli.json";

/// Files no agent should read: env files with real values, keys and a
/// secrets folder. `.env.example` is not among them: agents keep it up to
/// date (core/env-example).
pub const SECRET_GLOBS: [&str; 10] = [
    "**/.env",
    "**/.env.local",
    "**/.env.*.local",
    "**/.env.development",
    "**/.env.production",
    "**/.env.staging",
    "**/.env.test",
    "**/*.pem",
    "**/*.key",
    "**/secrets/**",
];

/// The line that marks firescale's profile in a Codex config.
const CODEX_MARKER: &str = "[permissions.firescale]";

pub struct AgentPermissions;

/// The commands an agent may run without asking: fscale's own, and the
/// repo's build, lint, typecheck and test scripts.
fn allowed_commands(ctx: &RuleContext) -> Result<Vec<String>> {
    let facts = detect_facts_with(ctx.root(), None)?;
    let package_json: Option<Value> = std::fs::read_to_string(ctx.root().join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok());
    let bin = fscale_command(&facts, package_json.as_ref());
    let mut commands: Vec<String> = ["verify", "check", "facts", "doctor", "explain"]
        .iter()
        .map(|command| format!("{bin} {command}"))
        .collect();
    for command in [
        &facts.commands.build,
        &facts.commands.lint,
        &facts.commands.typecheck,
        &facts.commands.test,
    ]
    .into_iter()
    .flatten()
    {
        if !commands.contains(command) {
            commands.push(command.clone());
        }
    }
    // A project of another kind at the root, such as a Python project.
    for project in facts
        .projects
        .iter()
        .filter(|project| project.root == "." && project.kind != "node")
    {
        for command in [
            &project.commands.build,
            &project.commands.lint,
            &project.commands.typecheck,
            &project.commands.test,
        ]
        .into_iter()
        .flatten()
        {
            let stable = stable_command(command, Some(project), false);
            if !commands.contains(&stable) {
                commands.push(stable);
            }
        }
    }
    Ok(commands)
}

/// What Claude Code's settings must contain.
fn claude_wanted(ctx: &RuleContext) -> Result<Value> {
    let allow: Vec<String> = allowed_commands(ctx)?
        .iter()
        .map(|command| format!("Bash({command} *)"))
        .collect();
    let deny: Vec<String> = SECRET_GLOBS
        .iter()
        .map(|glob| format!("Read({glob})"))
        .collect();
    Ok(json!({ "permissions": { "allow": allow, "deny": deny } }))
}

/// What Cursor's CLI settings must contain (unconfirmed format).
fn cursor_wanted() -> Value {
    let deny: Vec<String> = SECRET_GLOBS
        .iter()
        .map(|glob| format!("Read({glob})"))
        .collect();
    json!({ "permissions": { "deny": deny } })
}

/// The Codex profile, as the fix writes it into a new `.codex/config.toml`.
#[must_use]
pub fn codex_config() -> String {
    let mut out = String::from(
        "# Written by firescale (agent-ready/agent-permissions). Codex reads this file only\n# in a project you have marked as trusted. The profile keeps env files and keys out\n# of the agent's sandbox.\ndefault_permissions = \"firescale\"\n\n[permissions.firescale]\nextends = \":workspace\"\n\n[permissions.firescale.filesystem.\":project_roots\"]\n",
    );
    for glob in SECRET_GLOBS {
        out.push_str(&format!("\"{glob}\" = \"deny\"\n"));
    }
    out
}

/// One JSON settings file: the new content when entries are missing and
/// can be added as new lines, `Ok(None)` when nothing is missing, and
/// `Err(())` when a person has to add them.
fn json_update(
    ctx: &RuleContext,
    file: &str,
    wanted: &Value,
) -> Result<std::result::Result<Option<String>, ()>> {
    let Some(text) = ctx.read(file)? else {
        let pretty = serde_json::to_string_pretty(wanted).unwrap_or_default();
        return Ok(Ok(Some(format!("{pretty}\n"))));
    };
    if text.trim().is_empty() {
        let pretty = serde_json::to_string_pretty(wanted).unwrap_or_default();
        return Ok(Ok(Some(format!("{pretty}\n"))));
    }
    let Ok(have) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return Ok(Err(()));
    };
    let target = merged(&have, wanted);
    // A value of another type where the entries go cannot be merged into.
    if !holds(&target, wanted) {
        return Ok(Err(()));
    }
    match insert_merge(&text, &target) {
        Some(updated) if updated == text => Ok(Ok(None)),
        Some(updated) => Ok(Ok(Some(updated))),
        None => Ok(Err(())),
    }
}

fn finding(file: &str, message: String, fixable: Fixable) -> Finding {
    Finding {
        message,
        file: file.to_owned(),
        range: None,
        project: None,
        fixable,
        severity: None,
    }
}

impl BuiltinRule for AgentPermissions {
    fn meta(&self) -> RuleMeta {
        let mut inputs = vec![
            CLAUDE_SETTINGS.to_owned(),
            CODEX_CONFIG.to_owned(),
            CURSOR_CLI.to_owned(),
        ];
        inputs.extend(
            super::managed::FACTS_INPUTS
                .iter()
                .chain(firescale_core::ecosystems::MARKER_GLOBS.iter())
                .map(|input| (*input).to_owned()),
        );
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Security,
            description: "Each agent's project settings deny reading env files and keys, and allow the repo's own check commands.".to_owned(),
            inputs,
            has_fix: true,
            after: vec![super::verify_hook::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for (file, wanted, agent) in [
            (CLAUDE_SETTINGS, claude_wanted(ctx)?, "Claude Code"),
            (CURSOR_CLI, cursor_wanted(), "Cursor"),
        ] {
            match json_update(ctx, file, &wanted)? {
                Ok(None) => {}
                Ok(Some(_)) => findings.push(finding(
                    file,
                    format!("{file} does not keep {agent} away from env files and keys, or does not allow the repo's check commands. `fscale sync` adds the missing entries and keeps everything else."),
                    Fixable::Deterministic,
                )),
                Err(()) => findings.push(finding(
                    file,
                    format!("{file} lacks some of firescale's permission entries, and they cannot be added without changing a line you wrote (the file is not valid JSON, or a list is written on one line). Add them by hand: {}", serde_json::to_string(&wanted).unwrap_or_default()),
                    Fixable::Manual,
                )),
            }
        }
        match ctx.read(CODEX_CONFIG)? {
            None => findings.push(finding(
                CODEX_CONFIG,
                format!("{CODEX_CONFIG} is missing, so Codex may read env files and keys. `fscale sync` creates it with a permission profile that denies them."),
                Fixable::Deterministic,
            )),
            Some(text) if !text.lines().any(|line| line.trim() == CODEX_MARKER) => findings.push(finding(
                CODEX_CONFIG,
                format!("{CODEX_CONFIG} has no firescale permission profile, so Codex may read env files and keys. firescale does not edit a Codex config you wrote; add the profile by hand (`fscale explain {ID}` shows it)."),
                Fixable::Manual,
            )),
            Some(_) => {}
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        for (file, wanted) in [
            (CLAUDE_SETTINGS, claude_wanted(ctx)?),
            (CURSOR_CLI, cursor_wanted()),
        ] {
            let exists = ctx.read(file)?.is_some();
            match json_update(ctx, file, &wanted)? {
                Ok(Some(content)) if exists => {
                    ctx.write(file, &content)?;
                    result.changed_files.push(file.to_owned());
                }
                Ok(Some(content)) => {
                    ctx.create_tracked(file, &content)?;
                    result.changed_files.push(file.to_owned());
                }
                Ok(None) => {}
                Err(()) => result.notes.push(format!(
                    "{file} was left as it is: add firescale's permission entries by hand (`fscale check --rules {ID}` lists them)."
                )),
            }
        }
        if ctx.read(CODEX_CONFIG)?.is_none() {
            ctx.create_tracked(CODEX_CONFIG, &codex_config())?;
            result.changed_files.push(CODEX_CONFIG.to_owned());
        }
        if result.changed_files.iter().any(|file| file == CURSOR_CLI) {
            result.notes.push(format!(
                "{CURSOR_CLI} follows Cursor's CLI permission format as far as it could be checked; Cursor's own docs could not be read when it was written. Check it once in Cursor."
            ));
        }
        Ok(result)
    }
}
