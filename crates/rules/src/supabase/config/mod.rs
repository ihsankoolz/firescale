//! `supabase/config`: `supabase/config.toml` exists and names the project.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, config_value, create, finding, package_json, table_header, CONFIG};

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/config";

pub struct Config;

/// The project id a new config gets: package.json's name cleaned the way
/// the CLI cleans a folder name (`sanitizeProjectId`, CLI 3.1.0), or
/// `supabase-app`. Never the folder's name, so a dry run in a scratch copy
/// writes the same file.
pub(crate) fn project_id(ctx: &RuleContext) -> Result<String> {
    let name = match package_json(ctx)? {
        Ok(Some(json)) => json
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(|name| name.rsplit('/').next().unwrap_or(name).to_owned()),
        _ => None,
    };
    let mut cleaned = String::new();
    for c in name.unwrap_or_default().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            cleaned.push(c);
        } else if !cleaned.ends_with('_') {
            cleaned.push('_');
        }
    }
    let cleaned: String = cleaned
        .trim_start_matches(['_', '.', '-'])
        .chars()
        .take(40)
        .collect();
    Ok(if cleaned.is_empty() {
        "supabase-app".to_owned()
    } else {
        cleaned
    })
}

/// A new `supabase/config.toml`: the project id and nothing else, so every
/// other setting takes the CLI's own default (CLI 3.1.0 loads it).
#[must_use]
pub fn new_config(project_id: &str) -> String {
    format!(
        "#:schema https://supabase.com/schemas/config/v1.json\n# Supabase CLI settings: https://supabase.com/docs/guides/local-development/cli/config\n# A setting not written here takes the CLI's default.\n\n# Names the local containers: keep it unique among your projects.\nproject_id = \"{project_id}\"\n"
    )
}

/// The config with `project_id` added before its first table, in the
/// file's own line endings.
fn with_project_id(text: &str, project_id: &str) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let line = format!("project_id = \"{project_id}\"{eol}");
    // A byte order mark stays first.
    let mut offset = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    for piece in text[offset..].split_inclusive('\n') {
        if table_header(piece).is_some() {
            return format!("{}{line}{eol}{}", &text[..offset], &text[offset..]);
        }
        offset += piece.len();
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        eol
    };
    format!("{text}{separator}{line}")
}

impl BuiltinRule for Config {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description:
                "supabase/config.toml exists and sets project_id, which the Supabase CLI requires."
                    .to_owned(),
            inputs: super::inputs(&[]),
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        match ctx.read(CONFIG)? {
            None => Ok(vec![finding(
                CONFIG,
                "No supabase/config.toml, so the Supabase CLI has no project here. `fscale sync` creates one that sets project_id and leaves every other setting at the CLI's default.".to_owned(),
                Fixable::Deterministic,
            )]),
            Some(text) if config_value(&text, None, "project_id").is_none() => Ok(vec![finding(
                CONFIG,
                "supabase/config.toml has no project_id, which the Supabase CLI requires (it names the local containers). `fscale sync` adds it before the first table.".to_owned(),
                Fixable::Deterministic,
            )]),
            Some(_) => Ok(Vec::new()),
        }
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        let id = project_id(ctx)?;
        match ctx.read(CONFIG)? {
            None => create(ctx, CONFIG, &new_config(&id), &mut result)?,
            Some(text) if config_value(&text, None, "project_id").is_none() => {
                // Written only when the key is then read back at the top
                // level: never a change the next run would repeat.
                let updated = with_project_id(&text, &id);
                if config_value(&updated, None, "project_id").is_some() {
                    ctx.write(CONFIG, &updated)?;
                    result.changed_files.push(CONFIG.to_owned());
                }
            }
            Some(_) => {}
        }
        Ok(result)
    }
}
