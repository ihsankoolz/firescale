//! Fixture tests for the `expo` pack as a whole: a set-up app, an empty
//! folder built by the fixes, and every fix run twice.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::Fixture;

use super::fixtures::{app_without, fixture};
use super::pack;

fn rules() -> Vec<Box<dyn BuiltinRule>> {
    pack().rules
}

fn findings(f: &Fixture) -> Vec<Diagnostic> {
    rules()
        .iter()
        .flat_map(|rule| f.check(rule.as_ref(), &Options::new()).unwrap())
        .collect()
}

fn fix_all(f: &Fixture) -> Vec<String> {
    let mut changed = Vec::new();
    for rule in rules() {
        changed.extend(f.fix(rule.as_ref(), &Options::new()).unwrap().changed_files);
    }
    changed
}

#[test]
fn the_pack_has_seven_rules_with_skills() {
    let rules = rules();
    assert_eq!(rules.len(), 7);
    for rule in &rules {
        let meta = rule.meta();
        let name = meta.id.trim_start_matches("expo/");
        assert!(
            rule.skill().contains(&format!("name: {name}")),
            "{}",
            meta.id
        );
    }
}

#[test]
fn a_set_up_app_has_no_findings_and_nothing_to_fix() {
    let f = app_without(|_| false);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn an_empty_folder_is_built_by_the_fixes_and_a_second_run_changes_nothing() {
    let f = fixture(&[]);
    let before = findings(&f);
    assert!(
        before.iter().all(|d| d.fixable == Fixable::Deterministic),
        "{before:?}"
    );
    // The fix loop runs until nothing changes; two passes reach it here.
    fix_all(&f);
    fix_all(&f);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
    for file in [
        "package.json",
        "app.json",
        "App.tsx",
        "index.ts",
        "tsconfig.json",
        "eslint.config.js",
        ".gitignore",
    ] {
        assert!(f.read(file).is_some(), "{file} was not created");
    }
}

#[test]
fn a_workspace_listing_expo_at_its_root_gets_nothing_written() {
    // Review of M30: the app is in apps/mobile.
    let f = fixture(&[
        (
            "package.json",
            "{ \"name\": \"mono\", \"workspaces\": [\"apps/*\"], \"devDependencies\": { \"expo\": \"~58.0.25\" } }\n".to_owned(),
        ),
        ("apps/mobile/app.json", "{ \"expo\": { \"name\": \"m\", \"slug\": \"m\" } }\n".to_owned()),
    ]);
    let found = findings(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn every_rule_reads_what_decides_where_the_pack_applies() {
    for rule in rules() {
        let inputs = rule.meta().inputs;
        for shared in [
            "package.json",
            "pnpm-workspace.yaml",
            "app.json",
            "index.ts",
            "App.tsx",
        ] {
            assert!(
                inputs.iter().any(|i| i == shared),
                "{} lacks {shared}",
                rule.meta().id
            );
        }
    }
}
