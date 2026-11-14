//! Tests for `cmake/minimum-version`.

use firescale_core::model::{Fixable, Severity};
use firescale_core::rule::Options;

use crate::cmake::fixtures::{project_without, with};
use crate::cmake::minimum_version::{outcome, MinimumVersion};

#[test]
fn versions_as_cmake_4_reads_them() {
    assert_eq!(outcome((2, 8)), Some(true));
    assert_eq!(outcome((3, 4)), Some(true));
    assert_eq!(outcome((3, 5)), Some(false));
    assert_eq!(outcome((3, 9)), Some(false));
    assert_eq!(outcome((3, 10)), None);
    assert_eq!(outcome((4, 0)), None);
}

#[test]
fn old_versions_are_named_in_every_lists_file_and_ranges_count_by_their_end() {
    assert!(project_without(|_| false)
        .check(&MinimumVersion, &Options::new())
        .unwrap()
        .is_empty());
    let f = with(
        |_| false,
        &[
            (
                "CMakeLists.txt",
                "cmake_minimum_required(VERSION 3.5)\nproject(x LANGUAGES CXX)\n",
            ),
            (
                "lib/CMakeLists.txt",
                "cmake_minimum_required(VERSION 2.8.12)\n",
            ),
            (
                "ok/CMakeLists.txt",
                "cmake_minimum_required(VERSION 2.8...3.10)\n",
            ),
            ("old.cmake", "cmake_minimum_required(VERSION 2.6)\n"),
            (
                "doc/CMakeLists.txt",
                "# cmake_minimum_required(VERSION 2.6)\n",
            ),
        ],
    );
    let found = f.check(&MinimumVersion, &Options::new()).unwrap();
    let summary: Vec<(&str, Option<Severity>)> = found
        .iter()
        .map(|d| (d.file.as_str(), d.severity))
        .collect();
    assert_eq!(
        summary,
        [
            ("CMakeLists.txt", Some(Severity::Medium)),
            // The rule's own severity, high.
            ("lib/CMakeLists.txt", None),
        ]
    );
    assert!(found.iter().all(|d| d.fixable == Fixable::Manual));
    assert!(
        found[0].message.contains("VERSION 3.5...3.10"),
        "{}",
        found[0].message
    );
    assert!(found[1].message.contains("CMake 4 stops"));
}

// Found in review: a version check picks the call by the CMake running.
#[test]
fn a_call_in_a_version_check_is_not_the_one_cmake_4_takes() {
    let f = with(
        |_| false,
        &[(
            "CMakeLists.txt",
            "if(CMAKE_VERSION VERSION_LESS 3.12)\n  cmake_minimum_required(VERSION 3.1)\nelse()\n  cmake_minimum_required(VERSION 3.12)\nendif()\nproject(x LANGUAGES CXX)\n",
        )],
    );
    assert!(f
        .check(&MinimumVersion, &Options::new())
        .unwrap()
        .is_empty());
}
