//! Fixture tests for the whole `python` pack.

use firescale_core::model::Diagnostic;
use firescale_core::rule::Options;
use firescale_core::testing::{Fixture, Track};

use super::fixtures::{root_app, two_services};
use super::pack;

fn findings(f: &Fixture) -> Vec<Diagnostic> {
    pack()
        .rules
        .iter()
        .flat_map(|rule| f.check(rule.as_ref(), &Options::new()).unwrap())
        .collect()
}

fn fix_all(f: &Fixture) -> Vec<String> {
    let mut changed = Vec::new();
    for rule in pack().rules {
        changed.extend(f.fix(rule.as_ref(), &Options::new()).unwrap().changed_files);
    }
    changed
}

#[test]
fn every_rule_has_a_fix_and_a_skill() {
    let rules = pack().rules;
    assert_eq!(rules.len(), 6);
    for rule in &rules {
        let meta = rule.meta();
        assert!(meta.id.starts_with("python/"));
        assert!(meta.has_fix);
        assert!(rule
            .skill()
            .contains(&format!("fscale check --rules {}", meta.id)));
    }
}

#[test]
fn an_empty_folder_becomes_a_starter_that_passes_and_a_second_run_changes_nothing() {
    let f = Fixture::new(&[], Track::None).unwrap();
    let mut changed = Vec::new();
    // As the fix loop does: until nothing changes.
    for _ in 0..3 {
        changed.extend(fix_all(&f));
    }
    for file in [
        "requirements.txt",
        "requirements-dev.txt",
        "pytest.ini",
        "ruff.toml",
        "app/main.py",
        "tests/test_main.py",
        ".gitignore",
    ] {
        assert!(changed.contains(&file.to_owned()), "{file}: {changed:?}");
    }
    assert_eq!(findings(&f), []);
    assert!(fix_all(&f).is_empty());
    assert!(f.read("pyproject.toml").is_none());
}

#[test]
fn a_set_up_app_at_the_root_passes() {
    let f = root_app();
    assert_eq!(findings(&f), []);
    assert!(fix_all(&f).is_empty());
}

#[test]
fn services_each_with_their_own_files_pass_and_nothing_is_written_at_the_root() {
    // The course project's shape: no pyproject.toml anywhere.
    let f = two_services(&[]);
    assert_eq!(findings(&f), []);
    assert!(fix_all(&f).is_empty());
    for file in [
        "pyproject.toml",
        "requirements.txt",
        "pytest.ini",
        "ruff.toml",
        "tests/test_main.py",
    ] {
        assert!(f.read(file).is_none(), "{file} at the root");
    }
}

#[test]
fn a_service_with_a_gap_gets_exactly_that_file_in_its_own_folder() {
    let f = two_services(&["backend/payments/pytest.ini"]);
    let found = findings(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file, "backend/payments/pytest.ini");
    assert_eq!(found[0].project.as_deref(), Some("backend/payments"));
    assert_eq!(fix_all(&f), ["backend/payments/pytest.ini"]);
    assert_eq!(findings(&f), []);
}

/// Found on GoogleCloudPlatform/microservices-demo: services with code and
/// no tests got a pytest.ini and a requirements-dev.txt, only adding files.
#[test]
fn a_service_with_no_tests_is_only_told_to_add_tests() {
    let f = two_services(&[
        "backend/orders/tests/test_main.py",
        "backend/orders/pytest.ini",
        "backend/orders/requirements-dev.txt",
    ]);
    let found = findings(&f);
    let ids: Vec<&str> = found.iter().map(|d| d.rule_id.as_str()).collect();
    assert_eq!(ids, ["python/tests"], "{found:?}");
    assert!(fix_all(&f).is_empty());
}

/// pytest looks upward for its config, so a config in a folder above counts.
#[test]
fn a_pytest_config_in_a_folder_above_counts() {
    let f = two_services(&["backend/orders/pytest.ini", "backend/payments/pytest.ini"]);
    std::fs::write(f.root().join("backend/pytest.ini"), "[pytest]\n").unwrap();
    assert_eq!(findings(&f), []);
}

/// Regressions from the review of M16.
mod review {
    use super::*;
    use firescale_core::model::Fixable;

    #[test]
    fn services_that_facts_cannot_see_are_checked_and_get_no_root_starter() {
        // No pytest.ini and no .py file directly in the service folders.
        let f = two_services(&[
            "backend/orders/pytest.ini",
            "backend/payments/pytest.ini",
            "backend/orders/ruff.toml",
        ]);
        let found = findings(&f);
        let files: Vec<&str> = found.iter().map(|d| d.file.as_str()).collect();
        assert!(files.contains(&"backend/orders/pytest.ini"), "{found:?}");
        assert!(files.contains(&"backend/payments/pytest.ini"), "{found:?}");
        assert!(
            !files.iter().any(|file| !file.starts_with("backend/")),
            "{found:?}"
        );
        let changed = fix_all(&f);
        assert!(
            changed.iter().all(|file| file.starts_with("backend/")),
            "{changed:?}"
        );
        assert!(f.read("requirements.txt").is_none() && f.read("app/main.py").is_none());
    }

    #[test]
    fn a_src_layout_project_is_not_a_starter() {
        let f = Fixture::new(
            &[
                ("pyproject.toml", "[project]\nname = \"pkg\"\ndependencies = []\n\n[build-system]\nrequires = [\"hatchling\"]\n"),
                ("src/pkg/__init__.py", ""),
            ],
            Track::None,
        )
        .unwrap();
        let found = findings(&f);
        assert!(
            found
                .iter()
                .all(|d| d.fixable != Fixable::Deterministic || d.file == ".gitignore"),
            "{found:?}"
        );
        let changed = fix_all(&f);
        assert!(
            !changed
                .iter()
                .any(|file| file.starts_with("app/") || file.starts_with("tests/")),
            "{changed:?}"
        );
    }

    #[test]
    fn a_service_below_another_project_needs_its_own_pytest_config() {
        let f = root_app();
        for (path, content) in [
            ("svc/requirements.txt", "pytest==9.2.0\n"),
            ("svc/app/__init__.py", ""),
            ("svc/app/main.py", "def g() -> int:\n    return 1\n"),
            ("svc/tests/test_g.py", "from app.main import g\n"),
        ] {
            std::fs::create_dir_all(f.root().join(path).parent().unwrap()).unwrap();
            std::fs::write(f.root().join(path), content).unwrap();
        }
        let found: Vec<Diagnostic> = findings(&f)
            .into_iter()
            .filter(|d| d.rule_id == "python/pytest-config")
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].file, "svc/pytest.ini");
    }

    #[test]
    fn a_folder_where_pytest_ini_should_be_is_left_for_a_person() {
        let f = two_services(&["backend/orders/pytest.ini"]);
        std::fs::create_dir_all(f.root().join("backend/orders/pytest.ini")).unwrap();
        let found: Vec<Diagnostic> = findings(&f)
            .into_iter()
            .filter(|d| d.rule_id == "python/pytest-config")
            .collect();
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(fix_all(&f).is_empty());
    }
}
