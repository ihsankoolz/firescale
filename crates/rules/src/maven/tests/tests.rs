//! Tests for `maven/tests`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::maven::fixtures::{project_without, with};
use crate::maven::test_files::Tests;

#[test]
fn code_without_a_test_is_for_an_agent() {
    assert!(project_without(|_| false)
        .check(&Tests, &Options::new())
        .unwrap()
        .is_empty());
    let f = project_without(|p| p.starts_with("src/test/"));
    let found = f.check(&Tests, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Agent);
    // A file under src/test that is not Java is not a test.
    let other = with(
        |p| p.starts_with("src/test/"),
        &[("src/test/resources/data.txt", "x\n")],
    );
    assert_eq!(other.check(&Tests, &Options::new()).unwrap().len(), 1);
}
