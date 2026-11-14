# 0055 — The `cmake` stack pack

Status: accepted (2026-11-14, M35). Follows the stack-pack shape of PLAN.md and ADRs 0029, 0049, 0050, 0053 and 0054; builds on the CMake project support of ADR 0051.

## Context

The third pack for the new languages (PLAN.md "M33 to M39"): C and C++ built with CMake. CMake is the build most C and C++ projects share, and ADR 0051 already gives a CMake project its build and test commands (`cmake -S . -B build`, `cmake --build build --config Debug`, `ctest --test-dir build -C Debug`). CMake 3.28.3, GCC 13, Clang 18, Ninja and clang-format 18 are installed here; CMake 3.31.10 and 4.4.4 were installed from PyPI to check what newer versions do.

## What the pack assumes (read and run on 2026-11-14)

From CMake's documentation, and **checked by running** CMake 3.28.3, 3.31.10 and 4.4.4:

- **Old minimum versions.** CMake 4.0 removed compatibility with versions before 3.5: `cmake_minimum_required(VERSION 2.8)` or `3.4` stops at configure ("Compatibility with CMake < 3.5 has been removed from CMake"). CMake 3.31 deprecated versions before 3.10: 3.5 to 3.9 warn on every configure (3.28 does not). With a range (`2.8...3.10`) the policies come from the upper end, and CMake 4 accepts it without a warning.
- **The form of the root file.** Without `cmake_minimum_required`, with it after `project()`, or without `project()`, CMake 3.28 and 4.4 warn ("cmake_minimum_required() should be called prior to this top-level project() call", "No project() command is present").
- **Tests.** Without a registered test, without `enable_testing()`, or with `enable_testing()` only in a subfolder, `ctest --test-dir build` prints "No tests were found!!!" and **exits 0** (3.28 and 4.4): `fscale verify`'s test step passes having tested nothing. CMake's documentation says `enable_testing()` belongs in the top-level source folder, because ctest reads the test list at the top of the build folder.
- **The C++ standard.** Without one, each compiler uses its own default: C++17 for GCC 11 and later and Clang 16 and later, C++14 for MSVC.

## The starter

`fscale init --packs core,cmake` writes:

- `CMakeLists.txt`: `cmake_minimum_required(VERSION 3.20)` (3.20 brought `ctest --test-dir`, which `fscale verify` runs), `project(app VERSION 0.1.0 LANGUAGES CXX)`, C++20 required without extensions, `compile_commands.json` on (for clangd and clang-tidy), a library `app_lib` with warnings on (`-Wall -Wextra -Wpedantic`, `/W4 /permissive-` with MSVC), the program `app` and the test `app_test` linking it, `enable_testing()` and `add_test`, and a **`lint` target** that runs `clang-format --dry-run --Werror` on every file under `src/` and `tests/` (a `CONFIGURE_DEPENDS` glob, so new files are linted).
- `src/greeting.hpp`, `src/greeting.cpp`, `src/main.cpp`, `tests/greeting_test.cpp`, in Google's C++ style, and `.clang-format` (`BasedOnStyle: Google`).

**The test needs no framework**: it is a program that returns non-zero on failure. GoogleTest or Catch2 would be fetched at configure time (`FetchContent`), which needs the network on every fresh build and adds a dependency the owner did not choose; `cmake/tests`' skill tells an agent to use the framework a project already has.

**Lint is a CMake target**, because clang-format needs a list of files and `fscale verify` runs one command on every system. ADR 0051's CMake ecosystem now takes a root `add_custom_target(lint …)` as the lint step (`cmake --build build --config Debug --target lint`), and `fscale doctor` checks for clang-format when that target runs it. Without clang-format, the target prints "clang-format was not found on the PATH; install it to run the lint target" and fails; `fscale verify` reads that line as SETUP NEEDED for `clang-format`, not as a failure of the code.

Run here: in an empty folder `fscale init --packs core,cmake` leaves no violations; with CMake 3.28 and 4.4, configure and build give no warning, `ctest` passes 1 test, `build/app` prints `Hello, world!`, the lint target passes, and fails on one badly indented line; Clang 18 with Ninja builds it without a warning too; `fscale verify` is OK on every step (check, lint, test) and `fscale doctor` lists cmake, a compiler and clang-format.

## The rules

| Rule                           | Checks                                                                                                                                                           | Fix                                                                                   |
| ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `cmake/lists` (high)           | the root has a `CMakeLists.txt` whose first `cmake_minimum_required` comes before `project()`, and which calls `project()`                                       | creates the starter's; the order and a missing call are for a person                  |
| `cmake/minimum-version` (high) | no `CMakeLists.txt` asks for a policy version before 3.10 (below 3.5 at high severity, 3.5 to 3.9 at medium)                                                     | none: a newer policy version changes behaviour                                        |
| `cmake/standard` (medium)      | a build that compiles C++ (the root `project()` enables CXX, or `enable_language(CXX)`) sets a C++ standard, in its CMake files or `CMakePresets.json`           | none: the standard decides which compilers build the code                             |
| `cmake/entry` (high)           | a root `CMakeLists.txt` that builds `src/main.cpp`, in a repo with no C or C++ code, gets the starter's code                                                     | writes the starter's sources, test and `.clang-format`                                |
| `cmake/tests` (medium)         | a project with code registers a test (`add_test`, `*_discover_tests`, `gtest_add_tests`) and turns testing on at the root (`enable_testing()`, `include(CTest)`) | adds `enable_testing()` on the line after `project()`; a missing test is for an agent |

**The checks read commands, not text.** `cmake/syntax.rs` reads CMake's command syntax: comments (`#`, and bracket comments `#[[ … ]]`), quoted arguments with escapes, bracket arguments (`[=[ … ]=]`) and nested parentheses, and returns nothing for a file it cannot follow, so a commented-out `add_test` or a call inside a message string does not count, and nothing is edited on a guess (the lesson of ADR 0054's review). It evaluates nothing: variables, `if()` and `include()` are read as written.

**What the pack reads**: every `CMakeLists.txt` and `*.cmake` outside build folders (`build`, `out`, `cmake-build-*`) and folders no build looks in (`stackkit::set_aside`, which skips `_deps`). C and C++ sources by extension (`.c`, `.h`, `.cc`, `.cpp`, `.cxx`, `.c++`, `.hh`, `.hpp`, `.hxx`, `.h++`, `.ipp`, `.ixx`, `.cppm`); `.cxx`, `.hh` and `.hxx` now count as code for every pack's "is this repo another stack's?".

**Where it applies**: a repo with a root `CMakeLists.txt`, or one with no code at all, no other stack's manifest, no other C build at the root (a Makefile, Autotools, SCons, xmake, premake, Bazel's `BUILD`, Meson, a Visual Studio solution or project) and no `CMakeLists.txt` below. **C or C++ code built another way is never given a `CMakeLists.txt`.** A `CMakeLists.txt` only in a subfolder of a repo that is no other stack's gets one finding naming it.

**Nothing in an existing `CMakeLists.txt` is changed except adding `enable_testing()`**: the minimum version, the standard, a missing test and the order of the first commands each change how the project builds, and are for a person or an agent. `.gitignore` gets `build/` from `core/gitignore-basics` (ADR 0051).

## Real repos

Shallow clones, read-only, the pack alone, 23 projects. No finding: Catch2, GSL, SFML, benchmark, cxxopts, doctest, fmt, glog, googletest, imgui (not a CMake project: its `CMakeLists.txt` files are standalone examples), json, leveldb, spdlog, yaml-cpp, zlib. Findings, each checked:

- `cmake/minimum-version`: cJSON (3.5), libuv's `docs/code` (3.5), utfcpp's `bench` and `tests` (3.5), rapidjson (3.5, and `example/` at 2.8, where CMake 4.4 stops configuring the whole project, run here).
- `cmake/tests`: curl (its tests run through custom targets, so `ctest` finds none), glfw (its test programs are interactive and not registered), raylib and TheAlgorithms/C-Plus-Plus (no test registered).

The first pass also named zlib and curl under `cmake/standard`: both are C projects whose C++ files (zlib's `contrib/`, curl's examples and a separate test project) their build never compiles. The rule now asks only when the build compiles C++.

## Not checked, and limits

- The pack does not evaluate CMake: a test registered through a project's own function defined in a file the reader skips, or a standard set through a variable's value, is not seen.
- Projects in subfolders are not checked one by one; `cmake/minimum-version` reads every `CMakeLists.txt`, the other rules the build as a whole.
- C standards are not checked: C compilers' defaults differ less, and many C projects set flags by hand.
- Conan and vcpkg manifests are reported by `fscale facts` (ADR 0051) and not checked.
- Windows and macOS: the starter was built on Linux only (MSVC's flags are from its documentation); the rules are text, and CI runs the tests on all three.

## Consequences

- `fscale init --packs core,cmake` starts a C++ project that builds, lints and tests with CMake, and `fscale verify` and an agent's stop hook run all three.
- `fscale adopt` gives an existing CMake project a root `enable_testing()` where tests were registered without it, and names an old minimum version, a missing standard or a missing test for a person or an agent.
