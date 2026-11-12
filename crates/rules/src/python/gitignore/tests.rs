//! Tests for `python/gitignore`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{PythonGitignore, ID};
use crate::python::fixtures::{root_app, two_services};

#[test]
fn meta_names_the_rule_and_its_fix() {
    let meta = PythonGitignore.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

#[test]
fn set_up_projects_pass() {
    for f in [root_app(), two_services(&[])] {
        assert!(f
            .check(&PythonGitignore, &Options::new())
            .unwrap()
            .is_empty());
        assert!(f
            .fix(&PythonGitignore, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn an_empty_folder_is_a_starter_to_build() {
    let f = Fixture::new(&[], Track::None).unwrap();
    let found = f.check(&PythonGitignore, &Options::new()).unwrap();
    if !found.is_empty() {
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(found[0].file, ".gitignore");
    }
}

#[test]
fn missing_lines_are_appended_in_the_file_s_own_line_endings_once() {
    let f = two_services(&[".gitignore"]);
    std::fs::write(f.root().join(".gitignore"), "node_modules/\r\n.venv\r\n").unwrap();
    f.fix(&PythonGitignore, &Options::new()).unwrap();
    let text = f.read(".gitignore").unwrap();
    assert!(
        text.starts_with("node_modules/\r\n.venv\r\n# Python caches"),
        "{text:?}"
    );
    assert!(!text.replace("\r\n", "").contains('\n'));
    assert!(!text.contains(".venv/"), "already covered: {text:?}");
    assert!(f
        .fix(&PythonGitignore, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
