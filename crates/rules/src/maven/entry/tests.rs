//! Tests for `maven/entry`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::maven::entry::{Entry, APP, APP_PATH, APP_TEST, TEST_PATH};
use crate::maven::fixtures::{project_without, with};

#[test]
fn a_project_with_no_java_code_gets_a_class_and_its_test_once() {
    assert!(project_without(|_| false)
        .check(&Entry, &Options::new())
        .unwrap()
        .is_empty());
    let f = project_without(|p| p.ends_with(".java"));
    assert_eq!(
        f.check(&Entry, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&Entry, &Options::new()).unwrap().changed_files,
        vec![APP_PATH, TEST_PATH]
    );
    assert_eq!(f.read(APP_PATH).as_deref(), Some(APP));
    assert_eq!(f.read(TEST_PATH).as_deref(), Some(APP_TEST));
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn an_aggregator_or_code_in_its_own_package_is_left_alone() {
    let aggregator = with(
        |p| p.ends_with(".java"),
        &[("pom.xml", "<project>\n  <packaging>pom</packaging>\n  <modules>\n    <module>core</module>\n  </modules>\n</project>\n")],
    );
    assert!(aggregator
        .check(&Entry, &Options::new())
        .unwrap()
        .is_empty());
    assert!(aggregator
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    let own = with(
        |p| p.ends_with(".java"),
        &[("src/main/java/org/acme/Tool.java", "package org.acme;\n")],
    );
    assert!(own.check(&Entry, &Options::new()).unwrap().is_empty());
}
