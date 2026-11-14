//! `cmake/lists`: the root has a `CMakeLists.txt` that CMake configures
//! without a warning about its form: `cmake_minimum_required` first, and a
//! `project()` call.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, create, finding, only_below, syntax, CXX_STANDARD, STARTER_MINIMUM};

#[cfg(test)]
mod tests;

pub const ID: &str = "cmake/lists";

pub struct Lists;

/// The `CMakeLists.txt` of a new project, as run with CMake 3.28 and 4.4:
/// a library with the code, the program and a test linking it, warnings
/// on, `compile_commands.json` for clangd and clang-tidy, and a `lint`
/// target that checks the formatting with clang-format (ADR 0055).
#[must_use]
pub fn new_lists() -> String {
    format!(
        r#"cmake_minimum_required(VERSION {STARTER_MINIMUM})
project(app VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD {CXX_STANDARD})
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
# compile_commands.json, for clangd and clang-tidy (Makefile and Ninja builds).
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# The code, as a library the program and the tests both link.
add_library(app_lib src/greeting.cpp)
target_include_directories(app_lib PUBLIC src)
if(MSVC)
  target_compile_options(app_lib PRIVATE /W4 /permissive-)
else()
  target_compile_options(app_lib PRIVATE -Wall -Wextra -Wpedantic)
endif()

add_executable(app src/main.cpp)
target_link_libraries(app PRIVATE app_lib)

# `ctest --test-dir build` runs every test added here.
enable_testing()
add_executable(app_test tests/greeting_test.cpp)
target_link_libraries(app_test PRIVATE app_lib)
add_test(NAME app_test COMMAND app_test)

# `cmake --build build --target lint` checks the formatting of every source
# file against .clang-format.
file(GLOB_RECURSE LINTED CONFIGURE_DEPENDS
  src/*.cpp src/*.hpp tests/*.cpp tests/*.hpp)
find_program(CLANG_FORMAT clang-format)
if(CLANG_FORMAT)
  add_custom_target(lint
    COMMAND "${{CLANG_FORMAT}}" --dry-run --Werror ${{LINTED}}
    WORKING_DIRECTORY "${{PROJECT_SOURCE_DIR}}"
    VERBATIM)
else()
  add_custom_target(lint
    COMMAND "${{CMAKE_COMMAND}}" -E echo "clang-format was not found on the PATH; install it to run the lint target"
    COMMAND "${{CMAKE_COMMAND}}" -E false
    VERBATIM)
endif()
"#
    )
}

/// What is wrong with the form of a root `CMakeLists.txt`, as messages;
/// `None` when it cannot be read.
#[must_use]
pub fn problems(text: &str) -> Option<Vec<String>> {
    let commands = syntax::commands(text)?;
    let mut found = Vec::new();
    let first_required = commands
        .iter()
        .position(|command| command.name == "cmake_minimum_required");
    let first_project = commands
        .iter()
        .position(|command| command.name == "project");
    match (first_required, first_project) {
        (None, _) => found.push(format!(
            "CMakeLists.txt does not call cmake_minimum_required, so CMake does not know which version's behaviour the project expects, and warns. Add `cmake_minimum_required(VERSION <oldest CMake you support>)` as its first line (the starter uses {STARTER_MINIMUM})."
        )),
        (Some(required), Some(project)) if project < required => found.push(
            "CMakeLists.txt calls project() before cmake_minimum_required, and CMake warns (\"cmake_minimum_required() should be called prior to this top-level project() call\"): the project's languages are set up before its policies. Move cmake_minimum_required to the top.".to_owned(),
        ),
        _ => {}
    }
    if first_project.is_none() {
        found.push(
            "CMakeLists.txt does not call project(), and CMake warns (\"No project() command is present\") and sets the languages up with defaults. Add `project(<name> LANGUAGES CXX)` (or C) after cmake_minimum_required.".to_owned(),
        );
    }
    Some(found)
}

impl BuiltinRule for Lists {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description: "The root has a CMakeLists.txt that calls cmake_minimum_required first, and project()."
                .to_owned(),
            inputs: super::inputs(&[]),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            if let Some(below) = only_below(ctx)? {
                return Ok(vec![finding(
                    &format!("{}/CMakeLists.txt", below[0]),
                    format!(
                        "The cmake pack checks a CMake project at the repo root, and this repo's CMakeLists.txt files are in {}. Nothing is checked or written; add a root CMakeLists.txt that adds them with add_subdirectory, or turn the pack off.",
                        below.join(", ")
                    ),
                    Fixable::Manual,
                )]);
            }
            return Ok(Vec::new());
        }
        let Some(text) = ctx.read("CMakeLists.txt")? else {
            return Ok(vec![finding(
                "CMakeLists.txt",
                format!("There is no CMakeLists.txt, so CMake builds nothing. `fscale sync` creates one for a C++{CXX_STANDARD} program named app, with a library, a test and a lint target; rename app to the project's name."),
                Fixable::Deterministic,
            )]);
        };
        let Some(problems) = problems(&text) else {
            return Ok(vec![finding(
                "CMakeLists.txt",
                "CMakeLists.txt is not CMake the pack can read (an unclosed parenthesis, quote, bracket argument or bracket comment), so nothing in it is checked or changed. Run `cmake -S . -B build` to see where CMake stops.".to_owned(),
                Fixable::Manual,
            )]);
        };
        Ok(problems
            .into_iter()
            .map(|message| finding("CMakeLists.txt", message, Fixable::Manual))
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if applies(ctx)? {
            create(ctx, "CMakeLists.txt", &new_lists(), &mut result)?;
        }
        Ok(result)
    }
}
