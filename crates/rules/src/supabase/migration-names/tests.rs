//! Tests for `supabase/migration-names`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use super::*;
use crate::supabase::fixtures::project;
use crate::supabase::migration_names;

#[test]
fn misnamed_and_clashing_migrations_are_for_an_agent() {
    let f = project(
        |_| false,
        &[
            ("supabase/migrations/init.sql", "select 1;\n"),
            (
                "supabase/migrations/20261114000000_other.sql",
                "select 1;\n",
            ),
            ("supabase/migrations/README.md", "notes\n"),
        ],
    );
    let found = f
        .check(&migration_names::MigrationNames, &Options::new())
        .unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found
        .iter()
        .any(|d| d.file == "supabase/migrations/init.sql"));
    assert!(found
        .iter()
        .any(|d| d.message.contains("share the version 20261114000000")));
    assert!(found.iter().all(|d| d.fixable == Fixable::Agent));
}

#[test]
fn reads_versions_as_the_cli_does() {
    assert_eq!(version("20241001120000_init.sql"), Some("20241001120000"));
    assert_eq!(version("1_x.sql"), Some("1"));
    assert_eq!(version("init.sql"), None);
    assert_eq!(version("2024-10-01_init.sql"), None);
    assert_eq!(version("20241001_init.SQL"), None);
}
