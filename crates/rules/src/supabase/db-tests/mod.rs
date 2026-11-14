//! `supabase/db-tests`: the project has a database test for `supabase test
//! db`, which runs pgTAP files from `supabase/tests` (`**/*.sql` and
//! `**/*.pg`, CLI 3.1.0).

use std::path::Path;

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, create, finding};

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/db-tests";

pub struct DbTests;

pub const TESTS: &str = "supabase/tests";
pub const FILE: &str = "supabase/tests/database/rls_test.sql";

/// The first test: every table in `public` has row level security on, in
/// the database the migrations built. pgTAP, in the layout of Supabase's
/// testing guide (`supabase/tests/database/*_test.sql`).
pub const TEST: &str = "-- Every table in the public schema has row level security on: the database\n-- the migrations build, checked by `supabase test db` (pgTAP).\nbegin;\nselect plan(1);\n\nselect is_empty(\n  $$ select tablename::text from pg_catalog.pg_tables\n     where schemaname = 'public' and not rowsecurity $$,\n  'every table in public has row level security enabled'\n);\n\nselect * from finish();\nrollback;\n";

/// True when `dir` holds a `.sql` or `.pg` file, at any depth.
fn has_test(dir: &Path, depth: usize) -> bool {
    if depth > 20 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(std::result::Result::ok).any(|entry| {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => has_test(&path, depth + 1),
            Ok(kind) if kind.is_file() => path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e == "sql" || e == "pg"),
            _ => false,
        }
    })
}

impl BuiltinRule for DbTests {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Reliability,
            description: "supabase/tests has at least one pgTAP test for `supabase test db`."
                .to_owned(),
            inputs: super::inputs(&[&format!("{TESTS}/**")]),
            has_fix: true,
            after: vec![super::config::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let dir = TESTS
            .split('/')
            .fold(ctx.root().to_path_buf(), |dir, part| dir.join(part));
        if !applies(ctx)? || has_test(&dir, 0) {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            FILE,
            "No database tests in supabase/tests, so `supabase test db` checks nothing. `fscale sync` adds a pgTAP test that every table in public has row level security on.".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        let dir = TESTS
            .split('/')
            .fold(ctx.root().to_path_buf(), |dir, part| dir.join(part));
        if applies(ctx)? && !has_test(&dir, 0) {
            create(ctx, FILE, TEST, &mut result)?;
        }
        Ok(result)
    }
}
