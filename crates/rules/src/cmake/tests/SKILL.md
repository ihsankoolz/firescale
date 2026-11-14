---
name: tests
description: Fixes violations of the firescale rule cmake/tests by turning testing on in the root CMakeLists.txt, and explains how to add a test. Use when fscale check reports cmake/tests.
---

# cmake/tests

## What this rule enforces and why

A CMake project with C or C++ code registers at least one test with CTest (`add_test`, or a framework's `gtest_discover_tests`, `catch_discover_tests`, `doctest_discover_tests`), and turns testing on in its root `CMakeLists.txt` (`enable_testing()`, or `include(CTest)`). `enable_testing()` in a subfolder only is not enough: ctest reads the test list at the top of the build folder. Without a test, or without testing on at the root, `ctest --test-dir build` prints "No tests were found!!!" and exits 0, so `fscale verify` and CI pass having tested nothing.

## Manual fix

1. With tests registered: add `enable_testing()` after `project()` in the root `CMakeLists.txt` (`fscale sync` does this).
2. With no test: add a test program and register it:

   ```cmake
   add_executable(app_test tests/app_test.cpp)
   target_link_libraries(app_test PRIVATE <the library under test>)
   add_test(NAME app_test COMMAND app_test)
   ```

3. Run `cmake --build build` and `ctest --test-dir build --output-on-failure`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. When tests are registered and the root does not turn testing on, it adds `enable_testing()` on the line after `project()`.

### REPORT

1. No test: check which test framework the project already uses (GoogleTest, Catch2, doctest, its own). If it uses one, write one test of the most important function with it and register it the way the framework documents. If it uses none, write a test program that returns non-zero on failure and register it with `add_test`; do not add a framework without asking, since fetching one changes the build.
2. Run `ctest --test-dir build --output-on-failure` and make it pass without changing the code under test.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules cmake/tests
```
