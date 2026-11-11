//! Tests for `core/scripts-present`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};
use serde_json::json;

use super::{ScriptsPresent, ID};

const FULL: &str =
    "{\"scripts\":{\"lint\":\"eslint .\",\"test\":\"vitest run\",\"build\":\"tsc\"}}\n";

fn options(value: serde_json::Value) -> Options {
    value.as_object().cloned().unwrap_or_default()
}

fn missing(fixture: &Fixture, options: &Options) -> Vec<String> {
    fixture
        .check(&ScriptsPresent, options)
        .unwrap()
        .into_iter()
        .map(|d| d.message)
        .collect()
}

#[test]
fn meta_has_no_fix() {
    let meta = ScriptsPresent.meta();
    assert_eq!(meta.id, ID);
    assert!(!meta.has_fix);
}

mod passing_repo {
    use super::*;

    #[test]
    fn all_three_scripts_present() {
        let f = Fixture::new(&[("package.json", FULL)], Track::All).unwrap();
        assert!(missing(&f, &Options::new()).is_empty());
    }

    #[test]
    fn rust_only_repo_has_nothing_to_check() {
        let f = Fixture::new(&[("Cargo.toml", "[package]\n")], Track::All).unwrap();
        assert!(missing(&f, &Options::new()).is_empty());
    }

    #[test]
    fn required_option_replaces_the_default_list() {
        let f = Fixture::new(
            &[("package.json", "{\"scripts\":{\"test\":\"jest\"}}")],
            Track::All,
        )
        .unwrap();
        assert!(missing(&f, &options(json!({"required": ["test"]}))).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn one_agent_fixable_finding_per_missing_script() {
        let f = Fixture::new(
            &[("package.json", "{\"scripts\":{\"build\":\"tsc\"}}")],
            Track::All,
        )
        .unwrap();
        let found = f.check(&ScriptsPresent, &Options::new()).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|d| d.file == "package.json" && d.fixable == Fixable::Agent));
    }

    #[test]
    fn no_scripts_section_reports_all_three() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}")], Track::All).unwrap();
        assert_eq!(missing(&f, &Options::new()).len(), 3);
    }

    #[test]
    fn npm_init_test_placeholder_counts_as_missing() {
        let pkg = "{\"scripts\":{\"lint\":\"x\",\"build\":\"x\",\"test\":\"echo \\\"Error: no test specified\\\" && exit 1\"}}";
        let f = Fixture::new(&[("package.json", pkg)], Track::All).unwrap();
        let found = missing(&f, &Options::new());
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("\"test\""));
    }

    #[test]
    fn malformed_package_json_is_one_manual_finding() {
        let f = Fixture::new(&[("package.json", "{ nope")], Track::All).unwrap();
        let found = f.check(&ScriptsPresent, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(found[0].message.contains("not valid JSON"));
    }

    #[test]
    fn invalid_required_option_is_a_user_error_naming_the_option() {
        let f = Fixture::new(&[("package.json", FULL)], Track::All).unwrap();
        let error = f
            .check(&ScriptsPresent, &options(json!({"required": "lint"})))
            .unwrap_err();
        assert!(error.message().contains("\"required\""));
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn passes_once_the_scripts_are_added() {
        let f = Fixture::new(&[("package.json", "{}")], Track::All).unwrap();
        assert_eq!(missing(&f, &Options::new()).len(), 3);
        f.write("package.json", FULL).unwrap();
        assert!(missing(&f, &Options::new()).is_empty());
    }

    #[test]
    fn fix_does_nothing() {
        let f = Fixture::new(&[("package.json", "{}")], Track::All).unwrap();
        assert!(f
            .fix(&ScriptsPresent, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read("package.json").unwrap(), "{}");
    }
}
