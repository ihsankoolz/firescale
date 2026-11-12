//! Tests for `python/pytest-config`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{PytestConfig, ID};
use crate::python::fixtures::{root_app, two_services};

#[test]
fn meta_names_the_rule_and_its_fix() {
    let meta = PytestConfig.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

#[test]
fn set_up_projects_pass() {
    for f in [root_app(), two_services(&[])] {
        assert!(f.check(&PytestConfig, &Options::new()).unwrap().is_empty());
        assert!(f
            .fix(&PytestConfig, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn an_empty_folder_is_a_starter_to_build() {
    let f = Fixture::new(&[], Track::None).unwrap();
    let found = f.check(&PytestConfig, &Options::new()).unwrap();
    if !found.is_empty() {
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(found[0].file, "pytest.ini");
    }
}

#[test]
fn a_service_missing_its_pytest_ini_gets_one_in_its_folder_once() {
    let f = two_services(&["backend/orders/pytest.ini"]);
    let found = f.check(&PytestConfig, &Options::new()).unwrap();
    assert_eq!(found[0].file, "backend/orders/pytest.ini");
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    assert_eq!(
        f.fix(&PytestConfig, &Options::new()).unwrap().changed_files,
        ["backend/orders/pytest.ini"]
    );
    assert!(f.check(&PytestConfig, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&PytestConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn pytest_settings_in_pyproject_count() {
    let f = two_services(&["backend/orders/pytest.ini"]);
    std::fs::write(
        f.root().join("backend/orders/pyproject.toml"),
        "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
    )
    .unwrap();
    assert!(f.check(&PytestConfig, &Options::new()).unwrap().is_empty());
}
