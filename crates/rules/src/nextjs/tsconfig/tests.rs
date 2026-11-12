//! Tests for `nextjs/tsconfig`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};

use super::{TsConfig, ID};
use crate::nextjs::fixtures::app_without;

#[test]
fn meta_names_the_rule_and_its_fix() {
    let meta = TsConfig.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

#[test]
fn a_set_up_app_passes() {
    let f = app_without(|_| false);
    assert!(f.check(&TsConfig, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&TsConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_missing_file_is_a_deterministic_finding_that_the_fix_repairs_once() {
    let f = app_without(|p| p == "tsconfig.json");
    let found = f.check(&TsConfig, &Options::new()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file, "tsconfig.json");
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    assert!(!f
        .fix(&TsConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    assert!(f.check(&TsConfig, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&TsConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
