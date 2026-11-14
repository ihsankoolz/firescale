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
fn the_line_goes_after_project_outside_any_comment() {
    assert_eq!(
        with_testing("project(x) # main\nadd_test(NAME t COMMAND t)").as_deref(),
        Some("project(x) # main\nenable_testing()\nadd_test(NAME t COMMAND t)")
    );
    assert_eq!(
        with_testing("project(x)").as_deref(),
        Some("project(x)\nenable_testing()\n")
    );
    // Found in review: a bracket comment opening on project()'s line took
    // the new line in, and every sync added another.
    assert_eq!(
        with_testing("project(x CXX) #[[ a\nmultiline comment ]]\nadd_test(NAME a COMMAND a)\n").as_deref(),
        Some("project(x CXX)\nenable_testing()\n #[[ a\nmultiline comment ]]\nadd_test(NAME a COMMAND a)\n")
    );
    // A project() in a block may not run: for a person.
    assert_eq!(with_testing("if(TOP)\n  project(x)\nendif()\n"), None);
    assert_eq!(
        with_testing("if(ALT)\n  project(alt)\nelse()\n  project(x)\nendif()\n"),
        None
    );
    assert_eq!(with_testing("add_test(NAME t COMMAND t)\n"), None);
    assert_eq!(with_testing("project(x\n"), None);
}

// Found in review: testing turned on after the tests are added, vendored
// frameworks' add_test, and helpers of ROS 2 and KDE.
#[test]
fn too_late_vendored_and_helpers() {
    let late = "cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\nadd_subdirectory(tests)\nenable_testing()\n";
    let f = with(
        |_| false,
        &[
            ("CMakeLists.txt", late),
            ("tests/CMakeLists.txt", "add_test(NAME t COMMAND t)\n"),
        ],
    );
    assert_eq!(
        f.check(&Tests, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    f.fix(&Tests, &Options::new()).unwrap();
    assert!(f.read("CMakeLists.txt").unwrap().starts_with("cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\nenable_testing()\nadd_subdirectory(tests)"));
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());

    let vendored = "cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\nadd_subdirectory(third_party/googletest)\nadd_executable(a src/main.cpp)\n";
    for module in [
        "third_party/googletest/cmake/internal_utils.cmake",
        "libs/gtest/CMakeLists.txt",
    ] {
        let f = with(
            |_| false,
            &[
                ("CMakeLists.txt", vendored),
                (
                    ".gitmodules",
                    "[submodule \"gtest\"]\n\tpath = libs/gtest\n\turl = https://example.com/g\n",
                ),
                (module, "enable_testing()\nadd_test(NAME g COMMAND g)\n"),
            ],
        );
        let found = f.check(&Tests, &Options::new()).unwrap();
        assert_eq!(found.len(), 1, "{module}");
        assert_eq!(found[0].fixable, Fixable::Agent, "{module}");
    }

    let guarded = new_lists().replace(
        "enable_testing()",
        "if(BUILD_TESTING)\n  include(CTest)\nendif()",
    );
    let f = with(|_| false, &[("CMakeLists.txt", &guarded)]);
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
    for helper in [
        "ament_add_gtest(t t.cpp)",
        "ecm_add_test(t.cpp)",
        "ecm_add_tests(t.cpp)",
        "my_add_test(t)",
    ] {
        let text = new_lists().replace("add_test(NAME app_test COMMAND app_test)", helper);
        let f = with(|_| false, &[("CMakeLists.txt", &text)]);
        assert!(
            f.check(&Tests, &Options::new()).unwrap().is_empty(),
            "{helper}"
        );
    }
}

// Found in review: ROS 2's ament turns testing on through find_package.
#[test]
fn a_systems_own_helpers_are_trusted_to_turn_testing_on() {
    let ros = "cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\nfind_package(ament_cmake REQUIRED)\nadd_library(x src/main.cpp)\nif(BUILD_TESTING)\n  find_package(ament_cmake_gtest REQUIRED)\n  ament_add_gtest(t tests/t.cpp)\nendif()\nament_package()\n";
    let f = with(|_| false, &[("CMakeLists.txt", ros)]);
    assert!(f.check(&Tests, &Options::new()).unwrap().is_empty());
}

// Found on GSL, spdlog, benchmark and others: adding folders without tests
// before turning testing on is fine; rapidjson adds its tests first.
#[test]
fn only_folders_with_tests_must_come_after_testing_is_on() {
    let fine = "cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\nadd_subdirectory(include)\nadd_subdirectory(src)\nif(X_TESTS)\n  enable_testing()\n  add_subdirectory(tests)\nendif()\n";
    let late = "cmake_minimum_required(VERSION 3.20)\nproject(x CXX)\nadd_subdirectory(./test)\ninclude(CTest)\n";
    for (root, expected) in [(fine, 0), (late, 1)] {
        let f = with(
            |_| false,
            &[
                ("CMakeLists.txt", root),
                (
                    "include/CMakeLists.txt",
                    "install(FILES x.hpp DESTINATION include)\n",
                ),
                ("tests/CMakeLists.txt", "add_test(NAME t COMMAND t)\n"),
                ("test/unit/CMakeLists.txt", "add_test(NAME u COMMAND u)\n"),
            ],
        );
        assert_eq!(
            f.check(&Tests, &Options::new()).unwrap().len(),
            expected,
            "{root}"
        );
    }
}

// Found on curl: its own pytest target is no CTest test.
#[test]
fn a_custom_target_helper_is_not_a_test() {
    let text = new_lists().replace(
        "add_test(NAME app_test COMMAND app_test)",
        "curl_add_pytests(test-pytest \"\")",
    );
    let f = with(|_| false, &[("CMakeLists.txt", &text)]);
    assert_eq!(
        f.check(&Tests, &Options::new()).unwrap()[0].fixable,
        Fixable::Agent
    );
}
