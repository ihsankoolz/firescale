//! Fixture repos the go rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::entry::{MAIN, MAIN_TEST};
use super::go_mod::new_go_mod;

/// A Go module as `fscale init --packs core,go` leaves it.
pub(crate) fn set_up() -> Vec<(&'static str, String)> {
    vec![
        ("go.mod", new_go_mod()),
        ("main.go", MAIN.to_owned()),
        ("main_test.go", MAIN_TEST.to_owned()),
        (
            ".gitignore",
            ".env\n\n# Go\n/app\n*.exe\n*.test\n*.out\ngo.work\ngo.work.sum\n".to_owned(),
        ),
    ]
}

pub(crate) fn fixture(files: &[(&str, String)]) -> Fixture {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}

/// The set-up module without the files for which `drop` is true.
pub(crate) fn module_without(drop: impl Fn(&str) -> bool) -> Fixture {
    with(drop, &[])
}

/// The set-up module without the files for which `drop` is true, and with
/// `extra` files added (or replaced).
pub(crate) fn with(drop: impl Fn(&str) -> bool, extra: &[(&'static str, &str)]) -> Fixture {
    let mut files: Vec<(&str, String)> = set_up()
        .into_iter()
        .filter(|(path, _)| !drop(path) && !extra.iter().any(|(p, _)| p == path))
        .collect();
    files.extend(extra.iter().map(|(p, c)| (*p, (*c).to_owned())));
    fixture(&files)
}
