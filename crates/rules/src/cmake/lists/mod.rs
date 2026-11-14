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
/// target that checks the formatting with clang-format through
/// [`LINT_SCRIPT`] (ADR 0055).
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

# `cmake --build build --target lint` checks the formatting of the C and
# C++ files under src/ and tests/ against .clang-format (cmake/lint.cmake).
add_custom_target(lint
  COMMAND "${{CMAKE_COMMAND}}" -P "${{PROJECT_SOURCE_DIR}}/cmake/lint.cmake"
  VERBATIM)
"#
    )
}

/// Where the starter's lint script is.
pub const LINT_PATH: &str = "cmake/lint.cmake";

/// The starter's lint script, run by its `lint` target with `cmake -P`: it
/// runs clang-format on every C and C++ file under `src/` and `tests/`, 50
/// at a time (a Windows command line holds about 8,000 characters), and
/// passes when there is none (clang-format given no file reads its input
/// and waits; found in review). Without clang-format it stops with a line
/// `fscale verify` reads as a setup need (ADR 0055).
pub const LINT_SCRIPT: &str = r#"# Checks the formatting of the C and C++ files under src/ and tests/
# against .clang-format: `cmake --build build --target lint` runs it, or
# `cmake -P cmake/lint.cmake`.
find_program(CLANG_FORMAT NAMES clang-format clang-format-22 clang-format-21
  clang-format-20 clang-format-19 clang-format-18 clang-format-17
  clang-format-16 clang-format-15 clang-format-14)
if(NOT CLANG_FORMAT)
  message(FATAL_ERROR "clang-format was not found on the PATH; install it to run the lint target")
endif()

get_filename_component(root "${CMAKE_CURRENT_LIST_DIR}" DIRECTORY)
set(patterns)
foreach(folder src tests)
  foreach(extension c h cc cpp cxx c++ hh hpp hxx h++ ipp ixx cppm)
    list(APPEND patterns "${root}/${folder}/*.${extension}")
  endforeach()
endforeach()
file(GLOB_RECURSE files LIST_DIRECTORIES false RELATIVE "${root}" ${patterns})
list(SORT files)
if(NOT files)
  message(STATUS "lint: no C or C++ files under src/ or tests/")
  return()
endif()

# In groups, to stay within the length of a command line on Windows.
set(failed FALSE)
list(LENGTH files total)
set(start 0)
while(start LESS total)
  list(SUBLIST files ${start} 50 group)
  execute_process(COMMAND "${CLANG_FORMAT}" --dry-run --Werror ${group}
    WORKING_DIRECTORY "${root}" RESULT_VARIABLE result)
  if(NOT result EQUAL 0)
    set(failed TRUE)
  endif()
  math(EXPR start "${start} + 50")
endwhile()
if(failed)
  message(FATAL_ERROR "lint: the files above are not formatted as .clang-format asks; `clang-format -i <file>` formats one")
endif()
"#;

/// True when the root `CMakeLists.txt` has the starter's `lint` target,
/// which runs [`LINT_PATH`].
fn runs_lint_script(text: &str) -> bool {
    syntax::commands(text).is_some_and(|commands| {
        commands.iter().any(|command| {
            command.name == "add_custom_target"
                && command.args.first().is_some_and(|name| name == "lint")
                && command.args.iter().any(|arg| arg.ends_with(LINT_PATH))
        })
    })
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
        let mut found: Vec<Finding> = problems
            .into_iter()
            .map(|message| finding("CMakeLists.txt", message, Fixable::Manual))
            .collect();
        if runs_lint_script(&text) && !ctx.root().join(LINT_PATH).exists() {
            found.push(finding(
                LINT_PATH,
                format!("CMakeLists.txt's lint target runs {LINT_PATH}, which is not there, so the lint step fails. `fscale sync` writes it: it checks the C and C++ files under src/ and tests/ with clang-format."),
                Fixable::Deterministic,
            ));
        }
        Ok(found)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        create(ctx, "CMakeLists.txt", &new_lists(), &mut result)?;
        if ctx
            .read("CMakeLists.txt")?
            .is_some_and(|text| runs_lint_script(&text))
        {
            create(ctx, LINT_PATH, LINT_SCRIPT, &mut result)?;
        }
        Ok(result)
    }
}
