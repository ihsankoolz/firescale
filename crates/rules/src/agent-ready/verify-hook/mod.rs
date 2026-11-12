//! `agent-ready/verify-hook`: an agent stop hook runs `fscale verify`.

use firescale_core::hooks::{hook_installed, Agent, AGENTS};
use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::{Error, Result};
use serde_json::Value;

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

fn finding(file: &str, message: String) -> Finding {
    Finding {
        message,
        file: file.to_owned(),
        range: None,
        project: None,
        fixable: Fixable::Manual,
        severity: None,
    }
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
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        match required(ctx)? {
            None => {
                if AGENTS.iter().any(|agent| hook_installed(ctx.root(), *agent)) {
                    return Ok(Vec::new());
                }
                Ok(vec![finding(
                    Agent::Claude.config_file(),
                    "No coding agent is set up to run fscale verify before it stops. Run `fscale hooks install --all` (or `--agent <claude|codex|cursor>`) and commit the files it writes."
                        .to_owned(),
                )])
            }
            Some(agents) => Ok(agents
                .into_iter()
                .filter(|agent| !hook_installed(ctx.root(), *agent))
                .map(|agent| {
                    finding(
                        agent.config_file(),
                        format!(
                            "{} has no stop hook that runs fscale verify. Run `fscale hooks install --agent {}` and commit the file it writes.",
                            agent.config_file(),
                            agent.name()
                        ),
                    )
                })
                .collect()),
        }
    }
}
