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

// Found on a real repo (microservices-demo): its modules are in src/*.
#[test]
fn modules_in_subfolders_get_one_finding_and_nothing_written() {
    let f = fixture(&[
        (
            "src/frontend/go.mod",
            "module frontend\n\ngo 1.26.0\n".to_owned(),
        ),
        (
            "src/frontend/main.go",
            "package main\n\nfunc main() {}\n".to_owned(),
        ),
        (
            "src/checkout/go.mod",
            "module checkout\n\ngo 1.26.0\n".to_owned(),
        ),
    ]);
    let found = findings(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(
        found[0].message.contains("src/checkout, src/frontend"),
        "{}",
        found[0].message
    );
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

// Found on a real repo (gin): doc.go shows `package main` in a comment.
#[test]
fn a_package_main_in_a_doc_comment_is_not_a_program() {
    let f = fixture(&[
        (
            "go.mod",
            "module github.com/acme/web\n\ngo 1.26.0\n".to_owned(),
        ),
        (
            "doc.go",
            "/*\nPackage web serves.\n\n\tpackage main\n*/\npackage web\n".to_owned(),
        ),
        ("web_test.go", "package web\n".to_owned()),
    ]);
    fix_all(&f);
    assert!(!f.read(".gitignore").unwrap_or_default().contains("/web"));
}
