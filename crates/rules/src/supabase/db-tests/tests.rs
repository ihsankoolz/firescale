//! Tests for `supabase/db-tests`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::rule::Options;

use crate::supabase::db_tests;
use crate::supabase::fixtures::project;

#[test]
fn any_test_file_counts_and_a_missing_one_is_created_once() {
    let other = project(
        |p| p == db_tests::FILE,
        &[("supabase/tests/x/y.pg", "select 1;\n")],
    );
    assert!(other
        .check(&db_tests::DbTests, &Options::new())
        .unwrap()
        .is_empty());
    let none = project(|p| p == db_tests::FILE, &[]);
    assert_eq!(
        none.check(&db_tests::DbTests, &Options::new())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        none.fix(&db_tests::DbTests, &Options::new())
            .unwrap()
            .changed_files,
        vec![db_tests::FILE]
    );
    assert!(none
        .fix(&db_tests::DbTests, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
