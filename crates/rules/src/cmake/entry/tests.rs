//! Tests for `cmake/entry`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::cmake::entry::{Entry, FILES};
use crate::cmake::fixtures::{files, project_without};

#[test]
fn the_starter_without_code_gets_its_code_once() {
    assert!(project_without(|_| false)
        .check(&Entry, &Options::new())
        .unwrap()
        .is_empty());
    let f = project_without(|p| p.starts_with("src/") || p.starts_with("tests/"));
    let found = f.check(&Entry, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    // .clang-format is there already, and is not changed.
    assert_eq!(
        f.fix(&Entry, &Options::new()).unwrap().changed_files,
        vec![
            "src/greeting.cpp",
            "src/greeting.hpp",
            "src/main.cpp",
            "tests/greeting_test.cpp"
        ]
    );
    for (path, content) in FILES {
        assert_eq!(f.read(path).as_deref(), Some(content), "{path}");
    }
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn a_lists_file_a_person_wrote_or_any_code_is_left_alone() {
    // A superbuild with no code of its own.
    let f = files(&[(
        "CMakeLists.txt",
        "cmake_minimum_required(VERSION 3.20)\nproject(s NONE)\ninclude(ExternalProject)\n",
    )]);
    assert!(f.check(&Entry, &Options::new()).unwrap().is_empty());
    // The starter's file with code of its own elsewhere.
    let f = project_without(|p| p.starts_with("src/") || p.starts_with("tests/"));
    std::fs::create_dir_all(f.root().join("lib")).unwrap();
    std::fs::write(f.root().join("lib/x.c"), "int x;\n").unwrap();
    assert!(f.check(&Entry, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
