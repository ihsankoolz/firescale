//! `agent-ready/verify-hook`: an agent stop hook runs `fscale verify`.

use firescale_core::facts::detect_facts_with;
use firescale_core::hooks::{hook_installed, merge_hook, Agent, AGENTS};
use firescale_core::jsonedit::{insert_merge, merged};
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::{Error, Result};
use serde_json::Value;

use super::managed::fscale_command;

#[cfg(test)]
mod tests;

pub const ID: &str = "agent-ready/verify-hook";

pub struct VerifyHook;

/// The agents named by the `agents` option. `None` means the default: any one agent.
fn required(ctx: &RuleContext) -> Result<Option<Vec<Agent>>> {
    let Some(option) = ctx.options().get("agents") else {
        return Ok(None);
    };
    let invalid = || {
        Error::user(
            "agent-ready/verify-hook: option \"agents\" must be a list of agent names, for example [\"claude\", \"codex\"]. Supported agents: claude, codex, cursor. Fix it in your firescale config.",
        )
    };
    let items = option
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(invalid)?;
    let mut agents = Vec::new();
    for item in items {
        let agent = match item {
            Value::String(name) => Agent::parse(name).map_err(|_| invalid())?,
            _ => return Err(invalid()),
        };
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    Ok(Some(agents))
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

/// How this repo starts fscale, for the hook command.
fn bin(ctx: &RuleContext) -> Result<String> {
    let facts = detect_facts_with(ctx.root(), None)?;
    let package_json: Option<Value> = std::fs::read_to_string(ctx.root().join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok());
    Ok(fscale_command(&facts, package_json.as_ref()))
}

/// What the agent's config file becomes with the hook in it, or `None`
/// when that needs a person: the file is not valid JSON, or the hook cannot
/// go in without changing one of its lines (ADR 0027).
fn with_hook(ctx: &RuleContext, agent: Agent, bin: &str) -> Result<Option<String>> {
    let file = agent.config_file();
    let Some(text) = ctx.read(file)? else {
        return merge_hook(None, agent, bin);
    };
    if text.trim().is_empty() {
        return merge_hook(None, agent, bin);
    }
    let Ok(have) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return Ok(None);
    };
    let Some(add) =
        merge_hook(None, agent, bin)?.and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Ok(None);
    };
    Ok(insert_merge(&text, &merged(&have, &add)))
}

/// The agents the fix sets up: the `agents` option, or all of them.
fn targets(ctx: &RuleContext) -> Result<Vec<Agent>> {
    Ok(required(ctx)?.unwrap_or_else(|| AGENTS.to_vec()))
}

impl BuiltinRule for VerifyHook {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "An agent stop hook runs fscale verify before the agent may stop."
                .to_owned(),
            inputs: AGENTS
                .iter()
                .map(|agent| agent.config_file().to_owned())
                .collect(),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        match required(ctx)? {
            None => {
                if AGENTS
                    .iter()
                    .any(|agent| hook_installed(ctx.root(), *agent))
                {
                    return Ok(Vec::new());
                }
                let bin = bin(ctx)?;
                let mut fixable = Fixable::Deterministic;
                for agent in AGENTS {
                    if with_hook(ctx, agent, &bin)?.is_none() {
                        fixable = Fixable::Manual;
                    }
                }
                Ok(vec![finding(
                    Agent::Claude.config_file(),
                    "No coding agent is set up to run fscale verify before it stops. `fscale sync` adds the stop hook for Claude Code, Codex and Cursor (or run `fscale hooks install --all`), then commit the files."
                        .to_owned(),
                    fixable,
                )])
            }
            Some(agents) => {
                let bin = bin(ctx)?;
                let mut findings = Vec::new();
                for agent in agents
                    .into_iter()
                    .filter(|agent| !hook_installed(ctx.root(), *agent))
                {
                    let (fixable, how) = if with_hook(ctx, agent, &bin)?.is_some() {
                        (Fixable::Deterministic, "`fscale sync` adds it".to_owned())
                    } else {
                        (Fixable::Manual, format!("The file is not valid JSON or is written on one line, so add it by hand: run `fscale hooks install --agent {}`", agent.name()))
                    };
                    findings.push(finding(
                        agent.config_file(),
                        format!(
                            "{} has no stop hook that runs fscale verify. {how}, then commit the file.",
                            agent.config_file(),
                        ),
                        fixable,
                    ));
                }
                Ok(findings)
            }
        }
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        // With no `agents` option one hook is enough to pass; the fix sets
        // up every agent once none is set up.
        if required(ctx)?.is_none()
            && AGENTS
                .iter()
                .any(|agent| hook_installed(ctx.root(), *agent))
        {
            return Ok(result);
        }
        let bin = bin(ctx)?;
        for agent in targets(ctx)? {
            if hook_installed(ctx.root(), agent) {
                continue;
            }
            let file = agent.config_file();
            let Some(content) = with_hook(ctx, agent, &bin)? else {
                result.notes.push(format!(
                    "{file} was left as it is: it is not valid JSON or the hook cannot go in without changing one of its lines. Run `fscale hooks install --agent {}`.",
                    agent.name()
                ));
                continue;
            };
            if ctx.read(file)?.is_some() {
                ctx.write(file, &content)?;
            } else {
                ctx.create_tracked(file, &content)?;
            }
            result.changed_files.push(file.to_owned());
        }
        if result
            .changed_files
            .iter()
            .any(|file| file.starts_with(".codex/"))
        {
            result.notes.push("Codex runs a project's hooks only in a trusted project, and only after you approve the hook once in Codex (and again after it changes).".to_owned());
        }
        Ok(result)
    }
}
