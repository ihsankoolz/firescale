---
name: standard
description: Explains how to fix violations of the firescale rule cmake/standard, which needs a CMake project with C++ code to set its C++ standard. Use when fscale check reports cmake/standard.
---

# cmake/standard

## What this rule enforces and why

A CMake project with C++ code sets the C++ standard it is written in. Without one, each compiler builds with its own default: C++17 for GCC 11 and later and for Clang 16 and later, C++14 for MSVC. Code that builds on one machine then fails on another, or on a newer compiler. The rule accepts `set(CMAKE_CXX_STANDARD <n>)`, a target's `CXX_STANDARD` property, `target_compile_features(<target> … cxx_std_<n>)`, a `-std=c++<n>` or `/std:c++<n>` flag, or `CMAKE_CXX_STANDARD` in `CMakePresets.json`.

## Manual fix

1. Find the newest standard the code needs (its use of `std::span`, concepts, `std::format` and so on) and the oldest compilers it must build with.
2. After `project()`, add:

   ```cmake
   set(CMAKE_CXX_STANDARD 20)
   set(CMAKE_CXX_STANDARD_REQUIRED ON)
   set(CMAKE_CXX_EXTENSIONS OFF)
   ```

   or, for a library others build against, `target_compile_features(<target> PUBLIC cxx_std_20)`.

3. Build with each compiler the project supports.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

1. Choosing the standard changes which compilers can build the code: report the standard you propose and why (the newest feature the code uses, the compilers in CI).
2. With the user's agreement, add the lines above after `project()` in the root `CMakeLists.txt`, then run `cmake -S . -B build` and `cmake --build build`.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules cmake/standard
```
