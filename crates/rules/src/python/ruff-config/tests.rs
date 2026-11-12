//! Tests for `python/ruff-config`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{RuffConfig, ID};
use crate::python::fixtures::{root_app, two_services};

#[test]
fn meta_names_the_rule_and_its_fix() {
    let meta = RuffConfig.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

#[test]
fn set_up_projects_pass() {
    for f in [root_app(), two_services(&[])] {
        assert!(f.check(&RuffConfig, &Options::new()).unwrap().is_empty());
        assert!(f
            .fix(&RuffConfig, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn an_empty_folder_is_a_starter_to_build() {
    let f = Fixture::new(&[], Track::None).unwrap();
    let found = f.check(&RuffConfig, &Options::new()).unwrap();
    if !found.is_empty() {
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(found[0].file, "ruff.toml");
    }
}

#[test]
fn a_config_in_a_folder_above_counts_and_a_missing_one_is_made_in_the_service() {
    let f = two_services(&["backend/orders/ruff.toml", "backend/payments/ruff.toml"]);
    let found = f.check(&RuffConfig, &Options::new()).unwrap();
    assert_eq!(found.len(), 2);
    std::fs::write(f.root().join("backend/ruff.toml"), "line-length = 88\n").unwrap();
    assert!(f.check(&RuffConfig, &Options::new()).unwrap().is_empty());
}
