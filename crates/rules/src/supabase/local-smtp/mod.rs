//! `supabase/local-smtp`: `supabase/config.toml` has no `[inbucket]` table,
//! which Supabase CLI v3 refuses to load ("[inbucket] in
//! supabase/config.toml is no longer supported. Use [local_smtp] instead.",
//! seen with CLI 3.1.0). Projects made by `supabase init` before v2.108.0
//! have one.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, declared_version, finding, package_json, table_header, CONFIG};

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/local-smtp";

pub struct LocalSmtp;

/// The first CLI whose `init` writes `[local_smtp]` (Supabase's v3
/// migration guide): older ones may not read it.
const RENAMED_IN: (u64, u64) = (2, 108);

/// The table headers of a config, by 1-based line, outside multi-line
/// strings (`"""` and `\'\'\'`).
fn headers(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_string: Option<&str> = None;
    for (index, line) in text.lines().enumerate() {
        if in_string.is_none() {
            if let Some(name) = table_header(line) {
                out.push((index + 1, name));
                continue;
            }
        }
        for quote in [r#"""""#, "\'\'\'"] {
            let count = line.matches(quote).count();
            if count % 2 == 1 {
                in_string = match in_string {
                    Some(q) if q == quote => None,
                    None => Some(quote),
                    other => other,
                };
            }
        }
    }
    out
}

fn is_inbucket(name: &str) -> bool {
    name == "inbucket" || name.starts_with("inbucket.")
}

/// The 1-based lines of `[inbucket]` and `[inbucket.*]` headers.
fn inbucket_lines(text: &str) -> Vec<usize> {
    headers(text)
        .into_iter()
        .filter(|(_, name)| is_inbucket(name))
        .map(|(line, _)| line)
        .collect()
}

/// True when the config already has `[local_smtp]`: renaming would make a
/// second table of that name, which TOML forbids.
fn has_local_smtp(text: &str) -> bool {
    headers(text)
        .iter()
        .any(|(_, name)| name == "local_smtp" || name.starts_with("local_smtp."))
}

/// The command that updates the CLI with the repo's own package manager.
fn update_command(ctx: &RuleContext) -> &'static str {
    let root = ctx.root();
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm add -D supabase@latest"
    } else if root.join("yarn.lock").exists() {
        "yarn add -D supabase@latest"
    } else if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
        "bun add -d supabase@latest"
    } else {
        "npm install --save-dev supabase@latest"
    }
}

/// The CLI's version spec when package.json allows a CLI older than the
/// rename (its lowest version is older), or `None`.
fn older_cli(ctx: &RuleContext) -> Result<Option<String>> {
    Ok(match package_json(ctx)? {
        Ok(Some(json)) => declared_version(&json, "supabase")
            .filter(|v| *v < RENAMED_IN)
            .and_then(|_| {
                ["dependencies", "devDependencies"]
                    .iter()
                    .find_map(|f| json.get(f)?.get("supabase")?.as_str().map(str::to_owned))
            }),
        _ => None,
    })
}

/// The config with each `[inbucket…]` header renamed, everything else as it
/// was (the keys are the same: CLI 3.1.0 loads them under `[local_smtp]`).
fn renamed(text: &str) -> String {
    let lines: Vec<usize> = inbucket_lines(text);
    text.split_inclusive('\n')
        .enumerate()
        .map(|(index, piece)| {
            if lines.contains(&(index + 1)) {
                piece.replacen("inbucket", "local_smtp", 1)
            } else {
                piece.to_owned()
            }
        })
        .collect()
}

impl BuiltinRule for LocalSmtp {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "supabase/config.toml has no [inbucket] table, which Supabase CLI v3 refuses; it is [local_smtp] now.".to_owned(),
            inputs: super::inputs(&["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb"]),
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
        let Some(text) = ctx.read(CONFIG)? else {
            return Ok(Vec::new());
        };
        let lines = inbucket_lines(&text);
        let Some(first) = lines.first() else {
            return Ok(Vec::new());
        };
        let older = older_cli(ctx)?;
        let both = has_local_smtp(&text);
        let message = if both {
            format!("supabase/config.toml has an [inbucket] table (line {first}), which Supabase CLI v3 refuses to load, and a [local_smtp] table too. Move the settings you want into [local_smtp] and delete [inbucket] by hand (renaming would make two tables of one name).")
        } else if let Some(spec) = &older {
            format!("supabase/config.toml has an [inbucket] table (line {first}), which Supabase CLI v3 refuses to load. package.json allows a CLI older than 2.108 (\"supabase\": \"{spec}\"), which may not read [local_smtp]: update the CLI (`{}`), then run `fscale sync` to rename it.", update_command(ctx))
        } else {
            format!("supabase/config.toml has an [inbucket] table (line {first}), which Supabase CLI v3 refuses to load (`supabase start` and the db commands fail). `fscale sync` renames it to [local_smtp]; its keys stay as they are.")
        };
        Ok(vec![finding(
            CONFIG,
            message,
            if both || older.is_some() {
                Fixable::Manual
            } else {
                Fixable::Deterministic
            },
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? || older_cli(ctx)?.is_some() {
            return Ok(result);
        }
        let Some(text) = ctx.read(CONFIG)? else {
            return Ok(result);
        };
        if inbucket_lines(&text).is_empty() || has_local_smtp(&text) {
            return Ok(result);
        }
        ctx.write(CONFIG, &renamed(&text))?;
        result.changed_files.push(CONFIG.to_owned());
        Ok(result)
    }
}
