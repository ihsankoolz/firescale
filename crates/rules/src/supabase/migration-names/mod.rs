//! `supabase/migration-names`: every SQL file in `supabase/migrations` is
//! named `<timestamp>_<name>.sql`, and no two share a timestamp. The CLI
//! skips any other file with only a warning (`MIGRATE_FILE_PATTERN`,
//! `^([0-9]+)_(.*)\.sql$`, CLI 3.1.0), so a misnamed migration silently
//! never runs; two with one version clash in the migration history table.

use std::collections::BTreeMap;

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding, migration_entries, MIGRATIONS};

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/migration-names";

pub struct MigrationNames;

/// The version (the leading digits) of a migration file name the CLI
/// applies, or `None`.
#[must_use]
pub fn version(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".sql")?;
    let (digits, _) = stem.split_once('_')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

impl BuiltinRule for MigrationNames {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "Every SQL file in supabase/migrations is named <timestamp>_<name>.sql, with no timestamp used twice; the CLI skips any other.".to_owned(),
            inputs: vec![format!("{MIGRATIONS}/*")],
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
        let mut findings = Vec::new();
        let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, is_file) in migration_entries(ctx) {
            if !is_file || !name.to_ascii_lowercase().ends_with(".sql") {
                continue;
            }
            match version(&name) {
                Some(v) => versions.entry(v.to_owned()).or_default().push(name),
                None => findings.push(finding(
                    &format!("{MIGRATIONS}/{name}"),
                    format!("{name} is not named <timestamp>_<name>.sql, so the Supabase CLI skips it and it never runs. Rename it with a UTC timestamp (YYYYMMDDHHMMSS) that puts it where it belongs in the order, as `supabase migration new` would."),
                    Fixable::Agent,
                )),
            }
        }
        for (v, names) in versions {
            if names.len() > 1 {
                findings.push(finding(
                    &format!("{MIGRATIONS}/{}", names[1]),
                    format!("{} share the version {v}; the migration history keeps one row per version, so they clash when applied. Give one a later timestamp, keeping the order they must run in.", names.join(" and ")),
                    Fixable::Agent,
                ));
            }
        }
        Ok(findings)
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
