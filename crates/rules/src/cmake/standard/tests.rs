//! Tests for `cmake/standard`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::cmake::fixtures::{files, project_without, with};
use crate::cmake::standard::Standard;

const BARE: &str = "cmake_minimum_required(VERSION 3.20)\nproject(x LANGUAGES CXX)\nadd_executable(x src/main.cpp)\n";

#[test]
fn every_way_to_set_the_standard_counts() {
    assert!(project_without(|_| false)
        .check(&Standard, &Options::new())
        .unwrap()
        .is_empty());
    let f = with(|_| false, &[("CMakeLists.txt", BARE)]);
    let found = f.check(&Standard, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Manual);
    for (file, text) in [
        ("CMakeLists.txt", format!("{BARE}target_compile_features(x PUBLIC cxx_std_17)\n")),
        ("CMakeLists.txt", format!("{BARE}set_target_properties(x PROPERTIES CXX_STANDARD 17)\n")),
        ("CMakeLists.txt", format!("{BARE}set(CMAKE_CXX_FLAGS \"${{CMAKE_CXX_FLAGS}} -std=c++17\")\n")),
        ("CMakeLists.txt", format!("{BARE}target_compile_options(x PRIVATE /std:c++latest)\n")),
        ("cmake/Standard.cmake", "set(CMAKE_CXX_STANDARD 17)\n".to_owned()),
        ("CMakePresets.json", "{\"configurePresets\":[{\"name\":\"d\",\"cacheVariables\":{\"CMAKE_CXX_STANDARD\":\"20\"}}]}\n".to_owned()),
    ] {
        let mut extra = vec![("CMakeLists.txt", BARE)];
        if file == "CMakeLists.txt" {
            extra[0].1 = &text;
        } else {
            extra.push((file, &text));
        }
        let f = with(|_| false, &extra);
        assert!(f.check(&Standard, &Options::new()).unwrap().is_empty(), "{file}: {text}");
    }
}

#[test]
fn a_comment_does_not_set_it_and_c_code_needs_none() {
    let commented = format!("{BARE}# set(CMAKE_CXX_STANDARD 17)\n");
    let f = with(|_| false, &[("CMakeLists.txt", &commented)]);
    assert_eq!(f.check(&Standard, &Options::new()).unwrap().len(), 1);
    let c = files(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(x LANGUAGES C)\nadd_executable(x main.c)\n"),
        ("main.c", "int main(void) { return 0; }\n"),
        ("x.h", "int x(void);\n"),
    ]);
    assert!(c.check(&Standard, &Options::new()).unwrap().is_empty());
}

// Found on zlib and curl: C projects with C++ files their build does not
// compile.
#[test]
fn a_c_project_with_cpp_files_it_does_not_build_needs_no_standard() {
    let zlib = files(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(\n    zlib\n    LANGUAGES C\n    VERSION 1.3)\nadd_library(z zlib.c)\n"),
        ("zlib.c", "int z;\n"),
        ("contrib/iostream/test.cpp", "int main() {}\n"),
    ]);
    assert!(zlib.check(&Standard, &Options::new()).unwrap().is_empty());
    let enabled = files(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(x LANGUAGES C)\nenable_language(CXX)\nadd_library(x x.cpp)\n"),
        ("x.cpp", "int x;\n"),
    ]);
    assert_eq!(enabled.check(&Standard, &Options::new()).unwrap().len(), 1);
}
