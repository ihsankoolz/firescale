//! `supabase/gitignore`: the CLI's local state (`supabase/.branches`,
//! `supabase/.temp`, which holds the linked project's ref and pooler URL)
//! stays out of git.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, create, finding, DIR};
use crate::core::gitignore_basics::is_covered;

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/gitignore";

pub struct SupabaseGitignore;

pub const FILE: &str = "supabase/.gitignore";

/// What `supabase init` writes to `supabase/.gitignore` (CLI 3.1.0,
/// `INIT_GITIGNORE_TEMPLATE`), byte for byte.
pub const TEMPLATE: &str =
    "# Supabase\n.branches\n.temp\n\n# dotenvx\n.env.keys\n.env.local\n.env.*.local\n";

/// The folders the CLI writes, relative to `supabase/`.
const ENTRIES: [&str; 2] = [".branches", ".temp"];

fn missing(ctx: &RuleContext) -> Result<Vec<&'static str>> {
    let inner = ctx.read(FILE)?.unwrap_or_default();
    let root = ctx.read(".gitignore")?.unwrap_or_default();
    Ok(ENTRIES
        .into_iter()
        .filter(|entry| {
            // The root's line must reach `supabase/<entry>` as git reads
            // it: `/.temp` there is the root's own folder, not this one.
            !is_covered(&inner, entry) && !is_covered(&root, &format!("{DIR}/{entry}"))
        })
        .collect())
}

impl BuiltinRule for SupabaseGitignore {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Security,
            description: "supabase/.branches and supabase/.temp, the Supabase CLI's local state, are ignored by git.".to_owned(),
            inputs: super::inputs(&[FILE, ".gitignore"]),
            has_fix: true,
            after: vec![super::config::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let missing = missing(ctx)?;
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        let names: Vec<String> = missing.iter().map(|m| format!("{DIR}/{m}")).collect();
        Ok(vec![finding(
            FILE,
            format!(
                "git does not ignore {}, where the Supabase CLI keeps local state (the linked project's ref and connection details). `fscale sync` {}.",
                names.join(" and "),
                if ctx.root().join(FILE).exists() {
                    "adds the missing lines to supabase/.gitignore"
                } else {
                    "creates supabase/.gitignore as `supabase init` writes it"
                }
            ),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        let missing = missing(ctx)?;
        if missing.is_empty() {
            return Ok(result);
        }
        let Some(current) = ctx.read(FILE)? else {
            create(ctx, FILE, TEMPLATE, &mut result)?;
            return Ok(result);
        };
        let eol = if current.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let separator = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            eol
        };
        let mut block = vec!["# Supabase CLI local state".to_owned()];
        block.extend(missing.iter().map(|line| (*line).to_owned()));
        ctx.write(
            FILE,
            &format!("{current}{separator}{}{eol}", block.join(eol)),
        )?;
        result.changed_files.push(FILE.to_owned());
        Ok(result)
    }
}
