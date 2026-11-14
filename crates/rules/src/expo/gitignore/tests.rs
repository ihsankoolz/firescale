//! Tests for `expo/gitignore`: passing, failing and fixed repos, and the fix
//! run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::fixtures::{app_without, with};
use crate::expo::gitignore::ExpoGitignore;

#[test]
fn missing_lines_are_appended_once_in_the_files_line_endings() {
    assert!(app_without(|_| false)
        .check(&ExpoGitignore, &Options::new())
        .unwrap()
        .is_empty());
    let f = with(
        |_| false,
        &[(".gitignore", "node_modules/\r\n.env.*\r\n*.jks\r\n")],
    );
    let found = f.check(&ExpoGitignore, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    // `.env.*` covers `.env*.local`.
    assert!(
        !found[0].message.contains(".env*.local"),
        "{}",
        found[0].message
    );
    f.fix(&ExpoGitignore, &Options::new()).unwrap();
    assert_eq!(
        f.read(".gitignore").unwrap(),
        "node_modules/\r\n.env.*\r\n*.jks\r\n# Expo\r\n.expo/\r\ndist/\r\n*.p8\r\n*.p12\r\n*.key\r\n*.mobileprovision\r\n"
    );
    assert!(f.check(&ExpoGitignore, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&ExpoGitignore, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_env_local_pair_covers_env_star_local() {
    let f = with(
        |_| false,
        &[(".gitignore", ".expo/\ndist/\n.env.local\n.env.*.local\n*.jks\n*.p8\n*.p12\n*.key\n*.mobileprovision\n")],
    );
    assert!(f.check(&ExpoGitignore, &Options::new()).unwrap().is_empty());
}
