---
name: lists
description: Fixes violations of the firescale rule cmake/lists by creating CMakeLists.txt, and explains how to fix the order of cmake_minimum_required and project(). Use when fscale check reports cmake/lists.
---

# cmake/lists

## What this rule enforces and why

The repo root has a `CMakeLists.txt` whose first command is `cmake_minimum_required`, and which calls `project()`. Without `cmake_minimum_required` CMake does not know which version's behaviour the project expects; called after `project()`, the languages are set up before the policies; without `project()`, the languages are set up with defaults. CMake warns in each case.

A new project gets a `CMakeLists.txt` for CMake 3.20 or later: a C++20 library `app_lib` (`src/greeting.cpp`), the program `app` (`src/main.cpp`) and the test `app_test` (`tests/greeting_test.cpp`) linking it, warnings on (`-Wall -Wextra -Wpedantic`, or `/W4` with MSVC), `compile_commands.json` for clangd and clang-tidy, and a `lint` target that runs `cmake/lint.cmake`: clang-format checks every C and C++ file under `src/` and `tests/` against `.clang-format`. Without clang-format the target says so, and `fscale verify` reports it as a setup need. `fscale verify` builds, lints and runs the tests through it.

## Manual fix

1. Run `fscale sync`, or write `CMakeLists.txt` by hand.
2. Rename `app` to the project's name in `project()` and the targets.
3. Configure, build and test: `cmake -S . -B build`, `cmake --build build`, `ctest --test-dir build`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `CMakeLists.txt` when there is none, and `cmake/lint.cmake` when the `lint` target runs it and it is missing. It never edits either.

### REPORT

- No `cmake_minimum_required`, or one after `project()`: move or add it as the first command, with the oldest CMake version the project supports. Ask the user for that version if CI or the README does not show it.
- No `project()`: add `project(<name> LANGUAGES <C, CXX or both>)` after `cmake_minimum_required`, with the languages of the code.
- A `CMakeLists.txt` the pack cannot read: run `cmake -S . -B build`, and fix the line CMake names.
- A `CMakeLists.txt` only in a subfolder: report where it is. Do not create one at the root.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules cmake/lists
```
