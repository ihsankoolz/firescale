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

pub const ID: &str = "supabase/local-smtp";

pub struct LocalSmtp;

/// The first CLI whose `init` writes `[local_smtp]` (Supabase's v3
/// migration guide): older ones may not read it.
const RENAMED_IN: (u64, u64) = (2, 108);

/// The 1-based lines of `[inbucket]` and `[inbucket.*]` headers.
fn inbucket_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            table_header(line)
                .is_some_and(|name| name == "inbucket" || name.starts_with("inbucket."))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// True when the repo pins a CLI older than the rename.
fn older_cli(ctx: &RuleContext) -> Result<bool> {
    Ok(match package_json(ctx)? {
        Ok(Some(json)) => declared_version(&json, "supabase").is_some_and(|v| v < RENAMED_IN),
        _ => false,
    })
}

/// The config with each `[inbucket…]` header renamed, everything else as it
/// was (the keys are the same: CLI 3.1.0 loads them under `[local_smtp]`).
fn renamed(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|piece| {
            if table_header(piece)
                .is_some_and(|name| name == "inbucket" || name.starts_with("inbucket."))
            {
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
            inputs: vec![CONFIG.to_owned(), "package.json".to_owned()],
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
        Ok(vec![finding(
            CONFIG,
            if older {
                format!("supabase/config.toml has an [inbucket] table (line {first}), which Supabase CLI v3 refuses to load. package.json pins a CLI older than 2.108, which may not read [local_smtp]: update the CLI (`npm install --save-dev supabase@latest`), then run `fscale sync` to rename it.")
            } else {
                format!("supabase/config.toml has an [inbucket] table (line {first}), which Supabase CLI v3 refuses to load (`supabase start` and the db commands fail). `fscale sync` renames it to [local_smtp]; its keys stay as they are.")
            },
            if older {
                Fixable::Manual
            } else {
                Fixable::Deterministic
            },
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? || older_cli(ctx)? {
            return Ok(result);
        }
        let Some(text) = ctx.read(CONFIG)? else {
            return Ok(result);
        };
        if inbucket_lines(&text).is_empty() {
            return Ok(result);
        }
        ctx.write(CONFIG, &renamed(&text))?;
        result.changed_files.push(CONFIG.to_owned());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{inbucket_lines, renamed};

    #[test]
    fn renames_only_the_headers() {
        let text = "project_id = \"x\"\r\n[inbucket]\r\nenabled = true # inbucket\r\n[ inbucket.extra ]\r\n[api]\r\n";
        assert_eq!(inbucket_lines(text), vec![2, 4]);
        let out = renamed(text);
        assert_eq!(
            out,
            "project_id = \"x\"\r\n[local_smtp]\r\nenabled = true # inbucket\r\n[ local_smtp.extra ]\r\n[api]\r\n"
        );
        assert!(inbucket_lines(&out).is_empty());
    }
}
