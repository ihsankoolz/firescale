---
name: entry
description: Fixes violations of the firescale rule cmake/entry by writing the starter's C++ code, its test and .clang-format. Use when fscale check reports cmake/entry.
---

# cmake/entry

## What this rule enforces and why

The starter's `CMakeLists.txt` builds `src/main.cpp`. When the repo has no C or C++ code at all, CMake stops at configure ("Cannot find source file"). The rule only looks at a `CMakeLists.txt` that builds `src/main.cpp`: one a person wrote without code (a superbuild) is theirs.

A new project gets a greeting library (`src/greeting.hpp`, `src/greeting.cpp`), the program (`src/main.cpp`), its test (`tests/greeting_test.cpp`, a program that returns non-zero on failure, so it needs no test framework) and `.clang-format` (`BasedOnStyle: Google`), which the `lint` target checks.

## Manual fix

1. Run `fscale sync`, or write the files by hand.
2. Build and test: `cmake -S . -B build`, `cmake --build build`, `ctest --test-dir build`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It writes the files that are missing and never changes one that exists.

### REPORT

- Tell the user the starter is a placeholder: replace `greeting` and its test with the project's real code, and `app` with its name.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules cmake/entry
```
