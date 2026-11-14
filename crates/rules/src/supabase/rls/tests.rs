//! Tests for `supabase/rls`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::supabase::fixtures::project;
use crate::supabase::rls;

#[test]
fn a_table_in_public_without_rls_is_found_in_the_migration_that_made_it() {
    let f = project(
        |_| false,
        &[(
            "supabase/migrations/20261114010000_todos.sql",
            "create table todos (id int);\ncreate table private.audit (id int);\n",
        )],
    );
    let found = f.check(&rls::Rls, &Options::new()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].file,
        "supabase/migrations/20261114010000_todos.sql"
    );
    assert!(found[0].message.contains("public.todos"));
    assert_eq!(found[0].fixable, Fixable::Agent);
}

#[test]
fn a_later_migration_that_enables_rls_passes_and_one_that_disables_it_fails() {
    let on = project(
        |_| false,
        &[
            (
                "supabase/migrations/20261114010000_todos.sql",
                "create table todos (id int);\n",
            ),
            (
                "supabase/migrations/20261114020000_rls.sql",
                "ALTER TABLE \"public\".todos ENABLE ROW LEVEL SECURITY;\n",
            ),
        ],
    );
    assert!(on.check(&rls::Rls, &Options::new()).unwrap().is_empty());
    let off = project(
        |_| false,
        &[(
            "supabase/migrations/20261114020000_off.sql",
            "alter table public.notes disable row level security;\n",
        )],
    );
    assert_eq!(off.check(&rls::Rls, &Options::new()).unwrap().len(), 1);
}

#[test]
fn the_exposed_schemas_come_from_the_config() {
    let f = project(
        |_| false,
        &[
            (
                "supabase/config.toml",
                "project_id = \"x\"\n[api]\nschemas = [\"api\"]\n",
            ),
            (
                "supabase/migrations/20261114010000_t.sql",
                "create table api.things (id int);\ncreate table public.local_only (id int);\n",
            ),
        ],
    );
    let found = f.check(&rls::Rls, &Options::new()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].message.contains("api.things"));
}

#[test]
fn a_schemas_array_over_several_lines_is_read() {
    let f = project(
        |_| false,
        &[
            (
                "supabase/config.toml",
                "project_id = \"x\"\n[api]\nschemas = [\n  \"public\", # main\n  \"api\",\n]\n",
            ),
            (
                "supabase/migrations/20261114010000_t.sql",
                "create table notes2 (id int);\n",
            ),
        ],
    );
    assert_eq!(f.check(&rls::Rls, &Options::new()).unwrap().len(), 1);
}

#[test]
fn a_migration_in_another_encoding_is_read_not_fatal() {
    let f = project(|_| false, &[]);
    std::fs::write(
        f.root()
            .join("supabase")
            .join("migrations")
            .join("20261114010000_latin.sql"),
        b"-- caf\xe9\ncreate table \"Todos\" (id int);\n",
    )
    .unwrap();
    let found = f.check(&rls::Rls, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    // A mixed-case name is quoted in the suggested SQL.
    assert!(
        found[0]
            .message
            .contains("alter table public.\"Todos\" enable"),
        "{}",
        found[0].message
    );
}
