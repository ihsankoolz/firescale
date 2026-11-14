//! Tests for `cmake/lists`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::cmake::fixtures::{files, project_without, with};
use crate::cmake::lists::{new_lists, problems, Lists};

#[test]
fn a_set_up_project_passes_and_a_missing_file_is_created_once() {
    assert!(project_without(|_| false)
        .check(&Lists, &Options::new())
        .unwrap()
        .is_empty());
    let f = files(&[]);
    let found = f.check(&Lists, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    f.fix(&Lists, &Options::new()).unwrap();
    assert_eq!(f.read("CMakeLists.txt"), Some(new_lists()));
    assert!(f
        .fix(&Lists, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_form_of_the_file_is_for_a_person() {
    assert_eq!(problems(&new_lists()).unwrap(), Vec::<String>::new());
    let order = problems("project(x)\ncmake_minimum_required(VERSION 3.20)\n").unwrap();
    assert_eq!(order.len(), 1);
    assert!(
        order[0].contains("before cmake_minimum_required"),
        "{order:?}"
    );
    let neither = problems("add_executable(x m.c)\n").unwrap();
    assert_eq!(neither.len(), 2, "{neither:?}");
    // Comments do not count, and case does not matter.
    let commented = problems("# cmake_minimum_required(VERSION 3.20)\nPROJECT(x)\n").unwrap();
    assert_eq!(commented.len(), 1);
    assert!(commented[0].contains("does not call cmake_minimum_required"));
    assert_eq!(
        problems("CMAKE_MINIMUM_REQUIRED(VERSION 3.20)\r\nProject(x)\r\n").unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(problems("project(x"), None);
}

#[test]
fn a_file_a_person_wrote_is_never_replaced() {
    let mine = "project(x)\n";
    let f = with(|_| false, &[("CMakeLists.txt", mine)]);
    let found = f.check(&Lists, &Options::new()).unwrap();
    assert!(
        found.iter().all(|d| d.fixable == Fixable::Manual),
        "{found:?}"
    );
    assert!(f
        .fix(&Lists, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    assert_eq!(f.read("CMakeLists.txt").as_deref(), Some(mine));
    let unreadable = with(|_| false, &[("CMakeLists.txt", "project(x\n")]);
    let found = unreadable.check(&Lists, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains("not CMake the pack can read"));
}
