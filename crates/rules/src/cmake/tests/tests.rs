//! Tests for `cmake/tests`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::cmake::fixtures::{project_without, with};
use crate::cmake::lists::new_lists;
use crate::cmake::test_files::{with_testing, Tests};

#[test]
fn testing_turned_on_below_only_is_turned_on_at_the_root_once() {
    assert!(project_without(|_| false)
        .check(&Tests, &Options::new())
        .unwrap()
        .is_empty());
    let root = "cmake_minimum_required(VERSION 3.20)\r\nproject(x\r\n  LANGUAGES CXX)\r\nadd_subdirectory(tests)\r\n";
    let f = with(
        |_| false,
        &[
            ("CMakeLists.txt", root),
            (
                "tests/CMakeLists.txt",
                "enable_testing()\nadd_test(NAME t COMMAND t)\n",
            ),
        ],
    );
    let found = f.check(&Tests, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    f.fix(&Tests, &Options::new()).unwrap();
    assert_eq!(
        f.read("CMakeLists.txt").unwrap(),
        "cmake_minimum_required(VERSION 3.20)\r\nproject(x\r\n  LANGUAGES CXX)\r\nenable_testing()\r\nadd_subdirectory(tests)\r\n"
    );
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Tests, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn include_ctest_or_a_module_turns_it_on_and_no_test_is_for_an_agent() {
    let starter = new_lists();
    let ctest = starter.replace("enable_testing()", "include(CTest)");
    let f = with(|_| false, &[("CMakeLists.txt", &ctest)]);
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
    let moved = starter.replace("enable_testing()", "include(cmake/Testing.cmake)");
    let f = with(
        |_| false,
        &[
            ("CMakeLists.txt", &moved),
            ("cmake/Testing.cmake", "enable_testing()\n"),
        ],
    );
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
    let gtest = starter.replace(
        "add_test(NAME app_test COMMAND app_test)",
        "include(GoogleTest)\ngtest_discover_tests(app_test)",
    );
    let f = with(|_| false, &[("CMakeLists.txt", &gtest)]);
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
    let none = starter.replace(
        "add_test(NAME app_test COMMAND app_test)",
        "# add_test(NAME app_test COMMAND app_test)",
    );
    let f = with(|_| false, &[("CMakeLists.txt", &none)]);
    let found = f.check(&Tests, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Agent);
    assert!(f
        .fix(&Tests, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_line_goes_after_project_in_its_indentation() {
    assert_eq!(
        with_testing("project(x) # main\nadd_test(NAME t COMMAND t)").as_deref(),
        Some("project(x) # main\nenable_testing()\nadd_test(NAME t COMMAND t)")
    );
    assert_eq!(
        with_testing("if(TOP)\n  project(x)\nendif()\n").as_deref(),
        Some("if(TOP)\n  project(x)\n  enable_testing()\nendif()\n")
    );
    assert_eq!(
        with_testing("project(x)").as_deref(),
        Some("project(x)\nenable_testing()\n")
    );
    assert_eq!(with_testing("add_test(NAME t COMMAND t)\n"), None);
    assert_eq!(with_testing("project(x\n"), None);
}
