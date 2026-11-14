//! Tests for `go/go-sum`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::go::fixtures::with;
use crate::go::go_sum::{requires, GoSum};

#[test]
fn requirements_read_from_lines_and_blocks() {
    assert!(!requires("module a\n\ngo 1.26.0\n"));
    assert!(requires("module a\n\nrequire golang.org/x/text v0.20.0\n"));
    assert!(requires(
        "module a\n\nrequire (\n\tgolang.org/x/text v0.20.0 // indirect\n)\n"
    ));
    assert!(!requires("module a\n\nrequire (\n)\n"));
    assert!(!requires("module a\n// require x v1\n"));
}

#[test]
fn requirements_without_go_sum_are_for_a_person() {
    let go_mod = "module example.com/app\n\ngo 1.26.0\n\nrequire golang.org/x/text v0.20.0\n";
    let f = with(|_| false, &[("go.mod", go_mod)]);
    let found = f.check(&GoSum, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Manual);
    let f = with(
        |_| false,
        &[
            ("go.mod", go_mod),
            ("go.sum", "golang.org/x/text v0.20.0 h1:x=\n"),
        ],
    );
    assert!(f.check(&GoSum, &Options::new()).unwrap().is_empty());
}
