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
