//! Fixture tests for the `go` pack as a whole: a set-up module, an empty
//! folder built by the fixes, and every fix run twice.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::Fixture;

use super::fixtures::{fixture, module_without, set_up};
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
fn the_pack_has_five_rules_with_skills() {
    let rules = rules();
    assert_eq!(rules.len(), 5);
    for rule in &rules {
        let meta = rule.meta();
        let name = meta.id.trim_start_matches("go/");
        assert!(
            rule.skill().contains(&format!("name: {name}")),
            "{}",
            meta.id
        );
    }
}

#[test]
fn a_set_up_module_has_no_findings_and_nothing_to_fix() {
    let f = module_without(|_| false);
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
    fix_all(&f);
    fix_all(&f);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
    for (file, content) in set_up() {
        if file == ".gitignore" {
            // core/gitignore-basics is not part of this pack: only the Go lines.
            assert!(f.read(file).unwrap().contains("# Go\n/app\n*.exe\n"));
        } else {
            assert_eq!(f.read(file).as_deref(), Some(content.as_str()), "{file}");
        }
    }
}

#[test]
fn another_stack_without_go_mod_gets_nothing() {
    let f = fixture(&[("pyproject.toml", "[project]\nname = \"svc\"\n".to_owned())]);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn every_rule_reads_what_decides_where_the_pack_applies() {
    for rule in rules() {
        let inputs = rule.meta().inputs;
        for shared in [
            "go.mod",
            "package.json",
            "pyproject.toml",
            "Cargo.toml",
            "CMakeLists.txt",
        ] {
            assert!(
                inputs.iter().any(|i| i == shared),
                "{} lacks {shared}",
                rule.meta().id
            );
        }
    }
}
