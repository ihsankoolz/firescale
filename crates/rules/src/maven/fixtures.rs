//! Fixture repos the maven rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::entry::{APP, APP_PATH, APP_TEST, TEST_PATH};
use super::pom::new_pom;

/// A Maven project as `fscale init --packs core,maven` leaves it.
pub(crate) fn set_up() -> Vec<(&'static str, String)> {
    vec![
        ("pom.xml", new_pom()),
        (APP_PATH, APP.to_owned()),
        (TEST_PATH, APP_TEST.to_owned()),
        (
            ".gitignore",
            "# Generated files and dependencies\ntarget/\n".to_owned(),
        ),
    ]
}

pub(crate) fn fixture(files: &[(&str, String)]) -> Fixture {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}

/// The set-up project without the files for which `drop` is true.
pub(crate) fn project_without(drop: impl Fn(&str) -> bool) -> Fixture {
    with(drop, &[])
}

/// The set-up project without the files for which `drop` is true, and with
/// `extra` files added (or replaced).
pub(crate) fn with(drop: impl Fn(&str) -> bool, extra: &[(&'static str, &str)]) -> Fixture {
    let mut files: Vec<(&str, String)> = set_up()
        .into_iter()
        .filter(|(path, _)| !drop(path) && !extra.iter().any(|(p, _)| p == path))
        .collect();
    files.extend(extra.iter().map(|(p, c)| (*p, (*c).to_owned())));
    fixture(&files)
}
