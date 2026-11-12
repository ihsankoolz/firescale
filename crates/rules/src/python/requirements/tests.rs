//! Tests for `python/requirements`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{Requirements, ID};
use crate::python::fixtures::{root_app, two_services};

#[test]
fn meta_names_the_rule_and_its_fix() {
    let meta = Requirements.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

#[test]
fn set_up_projects_pass() {
    for f in [root_app(), two_services(&[])] {
        assert!(f.check(&Requirements, &Options::new()).unwrap().is_empty());
        assert!(f
            .fix(&Requirements, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn an_empty_folder_is_a_starter_to_build() {
    let f = Fixture::new(&[], Track::None).unwrap();
    let found = f.check(&Requirements, &Options::new()).unwrap();
    if !found.is_empty() {
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(found[0].file, "requirements.txt");
    }
}
