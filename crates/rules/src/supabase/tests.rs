//! Fixture tests for the `supabase` pack: passing, failing and fixed repos,
//! and every fix run twice.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::Fixture;

use super::fixtures::{fixture, project};
use super::{config, db_tests, gitignore, pack, package_json};

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

#[test]
fn a_workspace_listing_the_cli_at_its_root_with_the_project_in_a_subfolder_gets_nothing_written() {
    // Review of M29: hoisted dependencies must not make a second project.
    let f = fixture(&[
        (
            "package.json",
            "{ \"name\": \"mono\", \"devDependencies\": { \"supabase\": \"3.1.0\" } }\n".to_owned(),
        ),
        (
            "apps/db/supabase/config.toml",
            "project_id = \"db\"\n".to_owned(),
        ),
    ]);
    assert_eq!(ids(&findings(&f)), vec![package_json::ID]);
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn a_supabase_project_in_another_stack_gets_no_package_json() {
    let f = fixture(&[
        ("pubspec.yaml", "name: app\n".to_owned()),
        ("supabase/config.toml", "project_id = \"x\"\n".to_owned()),
    ]);
    let found = f
        .check(&package_json::PackageJson, &Options::new())
        .unwrap();
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(found[0].message.contains("pubspec.yaml"));
    assert!(fix_all(&f).iter().all(|file| file != "package.json"));
}

#[test]
fn every_rule_reads_what_decides_where_the_pack_applies() {
    // Review of M29: the cache key of each rule covers package.json and
    // every Supabase project's config, or a cached run hides findings.
    for rule in rules() {
        let inputs = rule.meta().inputs;
        for shared in [
            "package.json",
            "supabase/config.toml",
            "**/supabase/config.toml",
        ] {
            assert!(
                inputs.iter().any(|i| i == shared),
                "{} lacks {shared}",
                rule.meta().id
            );
        }
    }
}
