//! Tests for `agent-ready/single-test-command`.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{makefile_has_test, SingleTestCommand, ID};

fn check(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let fixture = Fixture::new(files, Track::All).unwrap();
    fixture.check(&SingleTestCommand, &Options::new()).unwrap()
}

#[test]
fn meta_has_no_fix() {
    let meta = SingleTestCommand.meta();
    assert_eq!(meta.id, ID);
    assert!(!meta.has_fix);
    assert!(meta.inputs.contains(&"package.json".to_owned()));
}

#[test]
fn makefile_targets_are_read_line_by_line() {
    assert!(makefile_has_test("all: build\n\ntest:\n\tpytest\n"));
    assert!(makefile_has_test("test : deps\r\n\tgo test\r\n"));
    assert!(!makefile_has_test("tests:\n\tpytest\n"));
    assert!(!makefile_has_test("test := 1\n"));
    assert!(!makefile_has_test(
        "\ttest: indented, so part of a recipe\n"
    ));
}

mod passing_repo {
    use super::*;

    #[test]
    fn a_package_json_test_script() {
        assert!(check(&[("package.json", "{\"scripts\":{\"test\":\"vitest run\"}}")]).is_empty());
    }

    #[test]
    fn a_rust_repo() {
        assert!(check(&[("Cargo.toml", "[package]\nname = \"x\"\n")]).is_empty());
    }

    #[test]
    fn python_with_pytest_configured() {
        assert!(check(&[(
            "pyproject.toml",
            "[project]\nname = \"x\"\n\n[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n"
        )])
        .is_empty());
        assert!(check(&[
            ("requirements.txt", "pytest\n"),
            ("pytest.ini", "[pytest]\n")
        ])
        .is_empty());
    }

    #[test]
    fn a_makefile_test_target() {
        assert!(check(&[
            ("requirements.txt", "flask\n"),
            ("Makefile", "test:\n\tpytest\n")
        ])
        .is_empty());
    }

    #[test]
    fn a_repo_with_no_detected_language_is_left_alone() {
        assert!(check(&[("README.md", "# notes\n")]).is_empty());
        assert!(check(&[]).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn javascript_without_a_test_script() {
        let found = check(&[("package.json", "{\"scripts\":{\"build\":\"tsc\"}}")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_id, ID);
        assert_eq!(found[0].file, "package.json");
        assert_eq!(found[0].fixable, Fixable::Agent);
        assert!(found[0].message.contains("No single command"));
    }

    #[test]
    fn python_without_pytest_config_points_at_its_manifest() {
        let found = check(&[("pyproject.toml", "[project]\nname = \"x\"\n")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "pyproject.toml");
        let found = check(&[("requirements.txt", "flask\n")]);
        assert_eq!(found[0].file, "requirements.txt");
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn passes_once_a_test_script_exists_and_fix_does_nothing() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}")], Track::All).unwrap();
        assert_eq!(
            f.check(&SingleTestCommand, &Options::new()).unwrap().len(),
            1
        );
        let result = f.fix(&SingleTestCommand, &Options::new()).unwrap();
        assert!(result.changed_files.is_empty());
        f.write(
            "package.json",
            "{\"name\":\"demo\",\"scripts\":{\"test\":\"node --test\"}}",
        )
        .unwrap();
        assert!(f
            .check(&SingleTestCommand, &Options::new())
            .unwrap()
            .is_empty());
    }
}
