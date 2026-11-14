---
name: minimum-version
description: Explains how to fix violations of the firescale rule cmake/minimum-version, which needs every cmake_minimum_required to ask for CMake 3.10 or later. Use when fscale check reports cmake/minimum-version.
---

# cmake/minimum-version

## What this rule enforces and why

`cmake_minimum_required(VERSION <v>)` tells CMake which version's behaviour (its policies) the project expects. CMake 4.0 removed compatibility with versions before 3.5: such a project stops at configure with "Compatibility with CMake < 3.5 has been removed from CMake". CMake 3.31 deprecated versions before 3.10, and warns on every configure. With a range (`3.5...3.10`) the policies come from its upper end, so a range ending at 3.10 or later is accepted.

## Manual fix

1. Find the oldest CMake the project must build with (its CI, its README, the distributions it supports).
2. Set `cmake_minimum_required(VERSION <that version>)`, 3.10 or later. To keep an older minimum, use a range: `cmake_minimum_required(VERSION 3.5...3.10)`.
3. Configure and build with that CMake and the newest one, and read the policy warnings: a newer policy version can change behaviour.
4. For a subfolder from another project (a vendored library), update that library instead, or ask its authors to. Until then, `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` lets CMake 4 configure it.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

1. Read the project's CI and README for the CMake versions it supports, and report the line and the version you propose.
2. Do not change the version of a vendored library's `CMakeLists.txt` (under `third_party/`, `external/`, `extern/` or similar): report it.
3. With the user's agreement, change the line, then run `cmake -S . -B build` and `cmake --build build` and report any new policy warning.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules cmake/minimum-version
```
