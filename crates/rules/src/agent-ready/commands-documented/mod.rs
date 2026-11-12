//! `agent-ready/commands-documented`: AGENTS.md mentions each command the repo has.

use firescale_core::facts::detect_facts;
use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{read_agents_md, AgentsMd, AGENTS_MD};

#[cfg(test)]
mod tests;

pub const ID: &str = "agent-ready/commands-documented";

pub struct CommandsDocumented;

/// The ways a detected command may be written. Facts report package scripts
/// as `<pm> run <script>`; `<pm> <script>` runs the same script with pnpm,
/// yarn and bun, and with npm only for `test`.
fn forms(command: &str) -> Vec<String> {
    let mut forms = vec![command.to_owned()];
    let parts: Vec<&str> = command.split(' ').collect();
    if let [pm, "run", script] = parts[..] {
        if pm != "npm" || script == "test" {
            forms.push(format!("{pm} {script}"));
        }
    }
    forms
}

/// True when `text` contains `needle` as a whole command: not as the start of
/// a longer script name such as `pnpm test:unit`, and not as the tail of
/// another word.
fn mentions(text: &str, needle: &str) -> bool {
    let continues_name = |c: char| c.is_alphanumeric() || matches!(c, ':' | '-' | '_');
    text.match_indices(needle).any(|(start, found)| {
        let before = text[..start].chars().next_back();
        let after = text[start + found.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_')
            && !after.is_some_and(continues_name)
    })
}

impl BuiltinRule for CommandsDocumented {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Consistency,
            description: "AGENTS.md mentions the repo's build, test, lint and typecheck commands."
                .to_owned(),
            // AGENTS.md, plus what `fscale facts` reads to work out the commands.
            inputs: [
                AGENTS_MD,
                "package.json",
                "pnpm-lock.yaml",
                "package-lock.json",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            ]
            .iter()
            .map(|input| (*input).to_owned())
            .collect(),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        // A missing or unreadable AGENTS.md is agent-ready/agents-md's finding.
        let AgentsMd::Text(text) = read_agents_md(ctx) else {
            return Ok(Vec::new());
        };
        let commands = detect_facts(ctx.root())?.commands;
        let detected = [
            ("build", commands.build),
            ("test", commands.test),
            ("lint", commands.lint),
            ("typecheck", commands.typecheck),
        ];
        Ok(detected
            .into_iter()
            .filter_map(|(kind, command)| command.map(|command| (kind, command)))
            .filter(|(_, command)| !forms(command).iter().any(|form| mentions(&text, form)))
            .map(|(kind, command)| Finding {
                message: format!(
                    "AGENTS.md does not mention the {kind} command `{command}`. Add it, so an agent does not have to guess how to {kind} this repo."
                ),
                file: AGENTS_MD.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Agent,
                severity: None,
            })
            .collect())
    }
}
