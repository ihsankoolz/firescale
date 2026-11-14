//! Tests for `go/go-mod`: passing, failing and fixed repos, and the fix run
//! twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::go::fixtures::{fixture, module_without, with};
use crate::go::go_mod::{new_go_mod, GoMod};

#[test]
fn a_missing_go_mod_is_created_once() {
    assert!(module_without(|_| false)
        .check(&GoMod, &Options::new())
        .unwrap()
        .is_empty());
    let f = module_without(|p| p == "go.mod");
    assert_eq!(
        f.check(&GoMod, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&GoMod, &Options::new()).unwrap().changed_files,
        vec!["go.mod"]
    );
    assert_eq!(f.read("go.mod").as_deref(), Some(new_go_mod().as_str()));
    assert!(f
        .fix(&GoMod, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    assert!(f.check(&GoMod, &Options::new()).unwrap().is_empty());
}

#[test]
fn a_missing_go_line_is_added_after_the_module_line_in_the_files_line_endings() {
    let f = with(
        |_| false,
        &[(
            "go.mod",
            "module example.com/tool\r\n\r\nrequire golang.org/x/text v0.20.0\r\n",
        )],
    );
    assert_eq!(f.check(&GoMod, &Options::new()).unwrap().len(), 1);
    f.fix(&GoMod, &Options::new()).unwrap();
    assert_eq!(
        f.read("go.mod").as_deref(),
        Some(
            "module example.com/tool\r\n\r\ngo 1.26.0\r\n\r\nrequire golang.org/x/text v0.20.0\r\n"
        )
    );
    assert!(f
        .fix(&GoMod, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    // A module line with no newline after it.
    let f = with(|_| false, &[("go.mod", "module example.com/tool")]);
    f.fix(&GoMod, &Options::new()).unwrap();
    assert_eq!(
        f.read("go.mod").as_deref(),
        Some("module example.com/tool\n\ngo 1.26.0\n")
    );
}

#[test]
fn no_module_line_is_for_a_person() {
    let f = with(|_| false, &[("go.mod", "go 1.26.0\n")]);
    let found = f.check(&GoMod, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Manual);
}

#[test]
fn another_stack_without_go_mod_is_left_alone() {
    let f = fixture(&[("package.json", "{ \"name\": \"web\" }\n".to_owned())]);
    assert!(f.check(&GoMod, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&GoMod, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
