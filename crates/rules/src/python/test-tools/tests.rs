//! Tests for `python/test-tools`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{TestTools, ID};
use crate::python::fixtures::{root_app, two_services};

#[test]
fn meta_names_the_rule_and_its_fix() {
    let meta = TestTools.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

#[test]
fn set_up_projects_pass() {
    for f in [root_app(), two_services(&[])] {
        assert!(f.check(&TestTools, &Options::new()).unwrap().is_empty());
        assert!(f
            .fix(&TestTools, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn an_empty_folder_is_a_starter_to_build() {
    let f = Fixture::new(&[], Track::None).unwrap();
    let found = f.check(&TestTools, &Options::new()).unwrap();
    if !found.is_empty() {
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(found[0].file, "requirements-dev.txt");
    }
}

#[test]
fn a_service_without_pytest_in_its_lists_gets_a_dev_list_or_an_agent_finding() {
    let f = two_services(&["backend/orders/requirements-dev.txt"]);
    let found = f.check(&TestTools, &Options::new()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file, "backend/orders/requirements-dev.txt");
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    assert_eq!(
        f.fix(&TestTools, &Options::new()).unwrap().changed_files,
        ["backend/orders/requirements-dev.txt"]
    );
    assert!(f.check(&TestTools, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&TestTools, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    // A list a person wrote is never edited: that is for an agent.
    let g = two_services(&["backend/orders/requirements-dev.txt"]);
    std::fs::write(
        g.root().join("backend/orders/requirements-dev.txt"),
        "-r requirements.txt\nblack\n",
    )
    .unwrap();
    let found = g.check(&TestTools, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Agent);
    assert!(g
        .fix(&TestTools, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
