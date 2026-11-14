//! Tests for `go/tests`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::go::fixtures::{module_without, with};
use crate::go::test_files::Tests;

#[test]
fn go_code_without_a_test_is_for_an_agent() {
    assert!(module_without(|_| false)
        .check(&Tests, &Options::new())
        .unwrap()
        .is_empty());
    let f = module_without(|p| p == "main_test.go");
    let found = f.check(&Tests, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Agent);
    assert!(f
        .fix(&Tests, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    // No Go code at all is go/entry's.
    assert!(module_without(|p| p.ends_with(".go"))
        .check(&Tests, &Options::new())
        .unwrap()
        .is_empty());
    // A test in a subfolder counts.
    let f = with(
        |p| p == "main_test.go",
        &[("internal/calc/calc_test.go", "package calc\n")],
    );
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
}
