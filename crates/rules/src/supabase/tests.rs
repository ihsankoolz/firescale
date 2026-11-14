//! Fixture tests for the `supabase` pack: passing, failing and fixed repos,
//! and every fix run twice.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::Fixture;

use super::fixtures::{fixture, project};
use super::{
    config, db_tests, gitignore, local_smtp, migration_names, pack, package_json,
    public_secret_key, rls,
};

fn rules() -> Vec<Box<dyn BuiltinRule>> {
    pack().rules
}

fn findings(f: &Fixture) -> Vec<Diagnostic> {
    rules()
        .iter()
        .flat_map(|rule| f.check(rule.as_ref(), &Options::new()).unwrap())
        .collect()
}

fn ids(found: &[Diagnostic]) -> Vec<&str> {
    found.iter().map(|d| d.rule_id.as_str()).collect()
}

/// Runs every fix in the pack's order once.
fn fix_all(f: &Fixture) -> Vec<String> {
    let mut changed = Vec::new();
    for rule in rules() {
        changed.extend(f.fix(rule.as_ref(), &Options::new()).unwrap().changed_files);
    }
    changed
}

#[test]
fn the_pack_has_eight_rules_with_skills() {
    let rules = rules();
    assert_eq!(rules.len(), 8);
    for rule in &rules {
        let meta = rule.meta();
        assert!(meta.id.starts_with("supabase/"), "{}", meta.id);
        let name = meta.id.trim_start_matches("supabase/");
        assert!(
            rule.skill().contains(&format!("name: {name}")),
            "{} has no SKILL.md of its own",
            meta.id
        );
    }
}

#[test]
fn a_set_up_project_has_no_findings_and_nothing_to_fix() {
    let f = project(|_| false, &[]);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn an_empty_folder_is_built_by_the_fixes_and_a_second_run_changes_nothing() {
    let f = fixture(&[]);
    let before = findings(&f);
    assert_eq!(
        ids(&before),
        vec![package_json::ID, config::ID, gitignore::ID, db_tests::ID]
    );
    assert!(before.iter().all(|d| d.fixable == Fixable::Deterministic));
    let changed = fix_all(&f);
    assert_eq!(
        changed,
        vec![
            "package.json",
            "supabase/config.toml",
            "supabase/.gitignore",
            db_tests::FILE
        ]
    );
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
    assert!(f
        .read("supabase/config.toml")
        .unwrap()
        .contains("project_id = \"supabase-app\""));
}

#[test]
fn a_workspace_with_supabase_in_a_subfolder_gets_nothing_written_at_the_root() {
    let f = fixture(&[
        (
            "package.json",
            "{ \"name\": \"web\", \"workspaces\": [\"apps/*\"] }\n".to_owned(),
        ),
        (
            "apps/db/supabase/config.toml",
            "project_id = \"db\"\n".to_owned(),
        ),
    ]);
    let found = findings(&f);
    assert_eq!(ids(&found), vec![package_json::ID]);
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(found[0].message.contains("apps/db/supabase"));
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn an_existing_package_json_of_another_stack_gets_the_cli_added() {
    let f = fixture(&[(
        "package.json",
        "{\n  \"name\": \"web\",\n  \"dependencies\": {\n    \"react\": \"19.0.0\"\n  }\n}\n"
            .to_owned(),
    )]);
    fix_all(&f);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert!(f
        .read("package.json")
        .unwrap()
        .contains("\"supabase\": \"3.1.0\""));
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

mod package_json_rule {
    use super::*;

    const WITHOUT_CLI: &str =
        "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"@supabase/supabase-js\": \"2.119.0\"\n  }\n}\n";

    #[test]
    fn the_cli_is_added_when_there_is_no_lockfile() {
        let f = project(|_| false, &[("package.json", WITHOUT_CLI)]);
        let found = f
            .check(&package_json::PackageJson, &Options::new())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(
            f.fix(&package_json::PackageJson, &Options::new())
                .unwrap()
                .changed_files,
            vec!["package.json"]
        );
        let text = f.read("package.json").unwrap();
        assert!(text.contains("\"supabase\": \"3.1.0\""), "{text}");
        // Every line that was there is still there.
        for line in WITHOUT_CLI
            .lines()
            .filter(|l| !l.trim().is_empty() && *l != "{" && *l != "}")
        {
            assert!(
                text.contains(line.trim_end_matches(',')),
                "{line} lost: {text}"
            );
        }
        assert!(f
            .check(&package_json::PackageJson, &Options::new())
            .unwrap()
            .is_empty());
        assert!(f
            .fix(&package_json::PackageJson, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }

    #[test]
    fn with_a_lockfile_installing_the_cli_is_for_the_package_manager() {
        let f = project(
            |_| false,
            &[("package.json", WITHOUT_CLI), ("package-lock.json", "{}\n")],
        );
        let found = f
            .check(&package_json::PackageJson, &Options::new())
            .unwrap();
        assert_eq!(found[0].fixable, Fixable::Agent);
        assert!(found[0].message.contains("npm install --save-dev supabase"));
        assert!(f
            .fix(&package_json::PackageJson, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

mod config_rule {
    use super::*;

    #[test]
    fn a_missing_project_id_is_added_before_the_first_table() {
        let f = project(
            |_| false,
            &[("supabase/config.toml", "[api]\nenabled = true\n")],
        );
        let found = f.check(&config::Config, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        f.fix(&config::Config, &Options::new()).unwrap();
        assert_eq!(
            f.read("supabase/config.toml").unwrap(),
            "project_id = \"supabase-app\"\n\n[api]\nenabled = true\n"
        );
        assert!(f
            .check(&config::Config, &Options::new())
            .unwrap()
            .is_empty());
        assert!(f
            .fix(&config::Config, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }

    #[test]
    fn the_project_id_comes_from_package_json_cleaned_as_the_cli_does() {
        let f = fixture(&[(
            "package.json",
            "{ \"name\": \"@acme/My App!\", \"devDependencies\": { \"supabase\": \"3.1.0\" } }\n"
                .to_owned(),
        )]);
        f.fix(&config::Config, &Options::new()).unwrap();
        assert!(f
            .read("supabase/config.toml")
            .unwrap()
            .contains("project_id = \"My_App_\""));
    }
}

mod local_smtp_rule {
    use super::*;

    const OLD: &str = "project_id = \"x\"\n\n[inbucket]\nenabled = true\nport = 54324\n";

    #[test]
    fn inbucket_is_renamed_to_local_smtp_once() {
        let f = project(|_| false, &[("supabase/config.toml", OLD)]);
        let found = f.check(&local_smtp::LocalSmtp, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("line 3"));
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        f.fix(&local_smtp::LocalSmtp, &Options::new()).unwrap();
        assert_eq!(
            f.read("supabase/config.toml").unwrap(),
            "project_id = \"x\"\n\n[local_smtp]\nenabled = true\nport = 54324\n"
        );
        assert!(f
            .check(&local_smtp::LocalSmtp, &Options::new())
            .unwrap()
            .is_empty());
        assert!(f
            .fix(&local_smtp::LocalSmtp, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }

    #[test]
    fn an_older_pinned_cli_is_for_a_person() {
        let f = project(
            |_| false,
            &[
                ("supabase/config.toml", OLD),
                (
                    "package.json",
                    "{ \"devDependencies\": { \"supabase\": \"^1.200.3\" } }\n",
                ),
            ],
        );
        let found = f.check(&local_smtp::LocalSmtp, &Options::new()).unwrap();
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(f
            .fix(&local_smtp::LocalSmtp, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

mod gitignore_rule {
    use super::*;

    #[test]
    fn missing_lines_are_appended_in_the_files_own_line_endings() {
        let f = project(|_| false, &[("supabase/.gitignore", "# mine\r\n.temp\r\n")]);
        let found = f
            .check(&gitignore::SupabaseGitignore, &Options::new())
            .unwrap();
        assert!(found[0].message.contains("supabase/.branches"));
        f.fix(&gitignore::SupabaseGitignore, &Options::new())
            .unwrap();
        assert_eq!(
            f.read("supabase/.gitignore").unwrap(),
            "# mine\r\n.temp\r\n# Supabase CLI local state\r\n.branches\r\n"
        );
        assert!(f
            .fix(&gitignore::SupabaseGitignore, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }

    #[test]
    fn the_root_gitignore_counts() {
        let f = project(
            |p| p == "supabase/.gitignore",
            &[(
                ".gitignore",
                "node_modules/\nsupabase/.branches\n**/.temp\n",
            )],
        );
        assert!(f
            .check(&gitignore::SupabaseGitignore, &Options::new())
            .unwrap()
            .is_empty());
    }
}

mod migration_names_rule {
    use super::*;

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
}

mod rls_rule {
    use super::*;

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
}

mod db_tests_rule {
    use super::*;

    #[test]
    fn any_test_file_counts_and_a_missing_one_is_created_once() {
        let other = project(
            |p| p == db_tests::FILE,
            &[("supabase/tests/x/y.pg", "select 1;\n")],
        );
        assert!(other
            .check(&db_tests::DbTests, &Options::new())
            .unwrap()
            .is_empty());
        let none = project(|p| p == db_tests::FILE, &[]);
        assert_eq!(
            none.check(&db_tests::DbTests, &Options::new())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            none.fix(&db_tests::DbTests, &Options::new())
                .unwrap()
                .changed_files,
            vec![db_tests::FILE]
        );
        assert!(none
            .fix(&db_tests::DbTests, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

mod public_secret_key_rule {
    use super::*;

    #[test]
    fn a_secret_key_under_a_public_prefix_is_found_with_its_line() {
        let f = project(
            |_| false,
            &[
                (
                    "src/client.ts",
                    "import { createClient } from '@supabase/supabase-js';\nexport const admin = createClient(url, process.env.NEXT_PUBLIC_SUPABASE_SERVICE_ROLE_KEY!);\n",
                ),
                (".env.example", "NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY=\nSUPABASE_SECRET_KEY=\n"),
            ],
        );
        let found = f
            .check(&public_secret_key::PublicSecretKey, &Options::new())
            .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].file, "src/client.ts");
        assert_eq!(found[0].range.map(|r| r.start_line), Some(2));
        assert_eq!(found[0].fixable, Fixable::Manual);
    }
}
