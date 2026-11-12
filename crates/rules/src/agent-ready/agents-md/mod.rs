//! `agent-ready/agents-md`: AGENTS.md exists at the repo root and holds
//! firescale's managed block, up to date: the repo's commands and the
//! definition of done (ADR 0027). A root CLAUDE.md imports it.
//!
//! Text outside the block belongs to the team and is never rewritten. The
//! fix creates AGENTS.md, appends the block to one that has none, or
//! replaces what is inside the block.

use firescale_core::facts::detect_facts_with;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::Value;

use super::managed::{
    agents_block, block_is_current, claude_block, find_block, fscale_command, imports_agents_md,
    with_block, Block, FACTS_INPUTS,
};
use super::{read_agents_md, AgentsMd, AGENTS_MD};

#[cfg(test)]
mod tests;

pub const ID: &str = "agent-ready/agents-md";

/// Claude Code's instructions file. When it exists, Claude Code reads it
/// and not AGENTS.md, so it must import AGENTS.md.
pub const CLAUDE_MD: &str = "CLAUDE.md";

pub struct AgentsMdExists;

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

/// The block AGENTS.md should hold in this repo.
fn wanted_block(ctx: &RuleContext) -> Result<String> {
    let facts = detect_facts_with(ctx.root(), None)?;
    let package_json: Option<Value> = std::fs::read_to_string(ctx.root().join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok());
    let bin = fscale_command(&facts, package_json.as_ref());
    Ok(agents_block(&facts, &bin))
}

/// Reads CLAUDE.md leniently: `None` when absent or unreadable.
fn read_claude(ctx: &RuleContext) -> Option<String> {
    std::fs::read(ctx.root().join(CLAUDE_MD))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

impl BuiltinRule for AgentsMdExists {
    fn meta(&self) -> RuleMeta {
        let mut inputs = vec![
            AGENTS_MD.to_owned(),
            // Changes the cache key when AGENTS.md is a folder with files in it.
            format!("{AGENTS_MD}/**"),
            CLAUDE_MD.to_owned(),
        ];
        inputs.extend(FACTS_INPUTS.iter().map(|input| (*input).to_owned()));
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "AGENTS.md exists at the repo root with firescale's managed block of commands, up to date; a root CLAUDE.md imports it.".to_owned(),
            inputs,
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        let block = wanted_block(ctx)?;
        match read_agents_md(ctx) {
            AgentsMd::Missing => findings.push(finding(
                AGENTS_MD,
                "No AGENTS.md at the repo root. `fscale sync` creates one with the repo's commands and the definition of done; then describe the project above the block.".to_owned(),
                Fixable::Deterministic,
            )),
            AgentsMd::Unreadable => findings.push(finding(
                AGENTS_MD,
                "AGENTS.md at the repo root cannot be read as a file (it may be a folder). Replace it with a text file, then run `fscale sync`.".to_owned(),
                Fixable::Manual,
            )),
            AgentsMd::Text(text) => match find_block(&text) {
                Block::Broken(why) => findings.push(finding(
                    AGENTS_MD,
                    format!("AGENTS.md has a damaged firescale block: {why}. Remove the stray marker lines by hand, then run `fscale sync` to write the block again."),
                    Fixable::Manual,
                )),
                Block::Absent if text.trim().is_empty() => findings.push(finding(
                    AGENTS_MD,
                    "AGENTS.md is blank. `fscale sync` writes the repo's commands and the definition of done into it.".to_owned(),
                    Fixable::Deterministic,
                )),
                Block::Absent => findings.push(finding(
                    AGENTS_MD,
                    "AGENTS.md has no firescale block. `fscale sync` adds one at the end with the repo's commands and the definition of done; your text stays as it is.".to_owned(),
                    Fixable::Deterministic,
                )),
                Block::At { .. } if !block_is_current(&text, &block) => findings.push(finding(
                    AGENTS_MD,
                    "The firescale block in AGENTS.md is out of date: it was edited by hand, or the repo's commands changed. `fscale sync` writes it again; put your own notes outside the block.".to_owned(),
                    Fixable::Deterministic,
                )),
                Block::At { .. } => {}
            },
        }
        if let Some(text) = read_claude(ctx) {
            match find_block(&text) {
                Block::Broken(why) => findings.push(finding(
                    CLAUDE_MD,
                    format!("CLAUDE.md has a damaged firescale block: {why}. Remove the stray marker lines by hand, then run `fscale sync`."),
                    Fixable::Manual,
                )),
                Block::At { .. } if block_is_current(&text, &claude_block()) => {}
                Block::Absent if imports_agents_md(&text) => {}
                _ => findings.push(finding(
                    CLAUDE_MD,
                    "CLAUDE.md does not import AGENTS.md, and Claude Code reads only CLAUDE.md when it exists, so it never sees the commands and the definition of done. `fscale sync` adds a small block with `@AGENTS.md` at the end; your text stays as it is.".to_owned(),
                    Fixable::Deterministic,
                )),
            }
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        let block = wanted_block(ctx)?;
        match read_agents_md(ctx) {
            AgentsMd::Missing => {
                let content = format!(
                    "# AGENTS.md\n\nInstructions for coding agents working in this repo. Describe the project, its layout and its conventions here, outside the firescale block.\n\n{block}"
                );
                ctx.create_tracked(AGENTS_MD, &content)?;
                result.changed_files.push(AGENTS_MD.to_owned());
                result.notes.push("AGENTS.md lists the repo's commands. Describe the project and its conventions above the firescale block.".to_owned());
            }
            AgentsMd::Text(text) if !block_is_current(&text, &block) => {
                if let Some(updated) = with_block(&text, &block) {
                    // Keep a byte order mark the file had.
                    let raw = ctx.read(AGENTS_MD)?.unwrap_or_default();
                    let updated = if raw.starts_with('\u{feff}') {
                        format!("\u{feff}{updated}")
                    } else {
                        updated
                    };
                    ctx.write(AGENTS_MD, &updated)?;
                    result.changed_files.push(AGENTS_MD.to_owned());
                }
            }
            _ => {}
        }
        if let Some(text) = read_claude(ctx) {
            let needs = !imports_agents_md(&text) || matches!(find_block(&text), Block::At { .. });
            if needs && !block_is_current(&text, &claude_block()) {
                if let Some(updated) = with_block(&text, &claude_block()) {
                    ctx.write(CLAUDE_MD, &updated)?;
                    result.changed_files.push(CLAUDE_MD.to_owned());
                    result.notes.push("CLAUDE.md now imports AGENTS.md through a small firescale block at its end, so Claude Code sees the commands too.".to_owned());
                }
            }
        }
        Ok(result)
    }
}
