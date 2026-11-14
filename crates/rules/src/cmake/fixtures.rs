//! Fixture repos the cmake rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::entry::FILES;
use super::lists::{new_lists, LINT_PATH, LINT_SCRIPT};

/// A CMake project as `fscale init --packs core,cmake` leaves it.
pub(crate) fn set_up() -> Vec<(&'static str, String)> {
    let mut files = vec![
        ("CMakeLists.txt", new_lists()),
        (LINT_PATH, LINT_SCRIPT.to_owned()),
    ];
    files.extend(
        FILES
            .iter()
            .map(|(path, content)| (*path, (*content).to_owned())),
    );
    files.push((
        ".gitignore",
        "# Generated files and dependencies\nbuild/\n".to_owned(),
    ));
    files
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

/// Files from `(path, content)` pairs.
pub(crate) fn files(pairs: &[(&str, &str)]) -> Fixture {
    fixture(
        &pairs
            .iter()
            .map(|(p, c)| (*p, (*c).to_owned()))
            .collect::<Vec<_>>(),
    )
}
