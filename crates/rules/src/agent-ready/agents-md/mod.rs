//! `agent-ready/agents-md`: AGENTS.md exists at the repo root and is not blank.

use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{read_agents_md, AgentsMd, AGENTS_MD};

#[cfg(test)]
mod tests;

pub const ID: &str = "agent-ready/agents-md";

pub struct AgentsMdExists;

impl BuiltinRule for AgentsMdExists {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "AGENTS.md exists at the repo root and is not blank.".to_owned(),
            // The second glob changes the cache key when AGENTS.md is a folder with files in it.
            inputs: vec![AGENTS_MD.to_owned(), format!("{AGENTS_MD}/**")],
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let (message, fixable) = match read_agents_md(ctx) {
            AgentsMd::Text(text) if !text.trim().is_empty() => return Ok(Vec::new()),
            AgentsMd::Text(_) => (
                "AGENTS.md is blank. Describe this project in it: what it is, the commands to build, test and lint it, and the rules a coding agent must follow.",
                Fixable::Agent,
            ),
            AgentsMd::Missing => (
                "No AGENTS.md at the repo root. Create one that describes this project: what it is, the commands to build, test and lint it, and the rules a coding agent must follow.",
                Fixable::Agent,
            ),
            AgentsMd::Unreadable => (
                "AGENTS.md at the repo root cannot be read as a file (it may be a folder). Replace it with a text file that describes this project for coding agents.",
                Fixable::Manual,
            ),
        };
        Ok(vec![Finding {
            message: message.to_owned(),
            file: AGENTS_MD.to_owned(),
            range: None,
            project: None,
            fixable,
        }])
    }
}
