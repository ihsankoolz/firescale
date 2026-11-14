//! `supabase/rls`: every table the migrations create in a schema the API
//! exposes has row level security on when the last migration has run.
//! Supabase's docs: "A table in an exposed schema without RLS is readable
//! and writable by any role with a grant on it. Enable RLS on every table
//! in an exposed schema." The database linter's `0013_rls_disabled_in_public`
//! checks the same in a running database; this reads the migrations.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, config_value, finding, migrations, sql, string_list, CONFIG, MIGRATIONS};

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/rls";

pub struct Rls;

/// The schemas the API exposes: `[api] schemas`, or the CLI's default;
/// none when `[api] enabled = false`.
fn exposed(ctx: &RuleContext) -> Result<Vec<String>> {
    let text = ctx.read(CONFIG)?.unwrap_or_default();
    if config_value(&text, Some("api"), "enabled").as_deref() == Some("false") {
        return Ok(Vec::new());
    }
    Ok(config_value(&text, Some("api"), "schemas").map_or_else(
        || vec!["public".to_owned(), "graphql_public".to_owned()],
        |value| string_list(&value),
    ))
}

impl BuiltinRule for Rls {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Security,
            description: "Every table the migrations create in a schema the API exposes has row level security enabled.".to_owned(),
            inputs: vec![format!("{MIGRATIONS}/*.sql"), CONFIG.to_owned()],
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let schemas = exposed(ctx)?;
        let migrations = migrations(ctx)?;
        Ok(sql::tables(&migrations)
            .into_iter()
            .filter(|table| {
                !table.rls
                    && table
                        .name
                        .split_once('.')
                        .is_some_and(|(schema, _)| schemas.iter().any(|s| s == schema))
            })
            .map(|table| {
                finding(
                    &format!("{MIGRATIONS}/{}", table.created_in),
                    format!(
                        "Table {} is in a schema the API exposes, and no migration enables row level security on it: any role with a grant can read and write it through the API. Add a migration with `alter table {} enable row level security;` and the policies its users need.",
                        table.name, table.name
                    ),
                    Fixable::Agent,
                )
            })
            .collect())
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
