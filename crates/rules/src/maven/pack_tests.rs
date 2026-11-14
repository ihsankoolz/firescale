//! Fixture tests for the `maven` pack as a whole.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::Fixture;

use super::fixtures::{fixture, project_without, set_up};
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
fn the_pack_has_four_rules_with_skills() {
    let rules = rules();
    assert_eq!(rules.len(), 4);
    for rule in &rules {
        let meta = rule.meta();
        let name = meta.id.trim_start_matches("maven/");
        assert!(
            rule.skill().contains(&format!("name: {name}")),
            "{}",
            meta.id
        );
    }
}

#[test]
fn a_set_up_project_has_no_findings_and_nothing_to_fix() {
    let f = project_without(|_| false);
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
        if file != ".gitignore" {
            assert_eq!(f.read(file).as_deref(), Some(content.as_str()), "{file}");
        }
    }
}

#[test]
fn another_stack_gets_nothing() {
    for files in [
        vec![("build.gradle.kts", "plugins { java }\n")],
        vec![("README.md", "x\n"), ("src/app.py", "print(1)\n")],
        vec![("go.mod", "module x\n")],
        // Found in review: Groovy-DSL Gradle, Ant and Bazel Java repos.
        vec![
            ("settings.gradle", "include 'app'\n"),
            ("app/src/main/java/org/example/Main.java", "class Main {}\n"),
        ],
        vec![
            ("build.xml", "<project/>\n"),
            ("src/org/ex/A.java", "class A {}\n"),
        ],
        vec![
            ("MODULE.bazel", "module(name = \"x\")\n"),
            ("java/A.java", "class A {}\n"),
        ],
    ] {
        let f = fixture(
            &files
                .iter()
                .map(|(p, c)| (*p, (*c).to_owned()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(findings(&f), Vec::<Diagnostic>::new(), "{files:?}");
        assert_eq!(fix_all(&f), Vec::<String>::new(), "{files:?}");
    }
}

#[test]
fn every_rule_reads_what_decides_where_the_pack_applies() {
    for rule in rules() {
        let inputs = rule.meta().inputs;
        for shared in ["pom.xml", "**/pom.xml", "build.gradle.kts", "**/*.py"] {
            assert!(
                inputs.iter().any(|i| i == shared),
                "{} lacks {shared}",
                rule.meta().id
            );
        }
    }
}

// Found on okhttp: a Gradle repo with a test fixture's pom.xml below.
#[test]
fn a_pom_below_another_stack_is_that_stacks_business() {
    let f = fixture(&[
        ("build.gradle.kts", "plugins { java }\n".to_owned()),
        ("maven-tests/pom.xml", "<project/>\n".to_owned()),
    ]);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
}

// Found in review: a Kotlin project was given a Java starter.
#[test]
fn a_kotlin_project_has_code() {
    let f = fixture(&[
        ("pom.xml", super::pom::new_pom()),
        ("src/main/kotlin/ex/Main.kt", "fun main() {}\n".to_owned()),
        (
            "src/test/kotlin/ex/MainTest.kt",
            "class MainTest\n".to_owned(),
        ),
    ]);
    let ids: Vec<String> = findings(&f).into_iter().map(|d| d.rule_id).collect();
    assert!(
        !ids.iter()
            .any(|id| id == "maven/entry" || id == "maven/tests"),
        "{ids:?}"
    );
    assert!(fix_all(&f).is_empty());
}
