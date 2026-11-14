//! Tests for `go/entry`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::go::entry::{Entry, MAIN, MAIN_TEST};
use crate::go::fixtures::{module_without, with};

#[test]
fn a_module_with_no_go_code_gets_a_main_package_and_its_test_once() {
    assert!(module_without(|_| false)
        .check(&Entry, &Options::new())
        .unwrap()
        .is_empty());
    let f = module_without(|p| p.ends_with(".go"));
    assert_eq!(
        f.check(&Entry, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&Entry, &Options::new()).unwrap().changed_files,
        vec!["main.go", "main_test.go"]
    );
    assert_eq!(f.read("main.go").as_deref(), Some(MAIN));
    assert_eq!(f.read("main_test.go").as_deref(), Some(MAIN_TEST));
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn go_code_anywhere_in_the_module_counts_but_not_where_the_go_tool_looks_away() {
    let lib = with(
        |p| p.ends_with(".go"),
        &[("internal/calc/calc.go", "package calc\n")],
    );
    assert!(lib.check(&Entry, &Options::new()).unwrap().is_empty());
    for ignored in ["testdata/x.go", "_old/x.go", ".cache/x.go", "vendor/a/x.go"] {
        let f = with(|p| p.ends_with(".go"), &[(ignored, "package x\n")]);
        assert_eq!(
            f.check(&Entry, &Options::new()).unwrap().len(),
            1,
            "{ignored}"
        );
    }
}
