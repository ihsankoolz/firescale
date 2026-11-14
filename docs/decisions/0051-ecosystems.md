# 0051 — Ecosystems found by a marker file: Java, Kotlin, Android, Swift, Xcode, C, C++, .NET, Dart

Status: accepted (2026-11-14, M31). Extends ADR 0022 (projects found in subfolders) and the verify plan of ADR 0024.

## Context

The owner asked for refactoring, and for firescale to work with other languages: Java, Rust, C, Swift and Xcode, mobile development, "and all that". Asked how deep, they chose project support (find the projects, know their build, lint and test commands, check the machine, gitignore and graph), with no new parser dependencies and no new stack packs, merged on green.

Before this, project kinds were `node`, `python`, `rust` and `go`. Each kind's knowledge was spread over `facts` (twice: once for the repo root and once for subfolders), `verify` (why a step has no command), `doctor` (what to install), `setup` (programs that mean "setup needed"), the step cache (version probes) and `core/gitignore-basics`. Adding a language meant touching six files in a dozen places.

## Decision

### One table of ecosystems

`crates/core/src/ecosystems.rs` holds one `Ecosystem` per kind found by a marker file. An entry says:

- how to recognise the folder (`marks`), and which kinds take the folder instead (`yields_to`);
- whether the same kind's subfolders are members of its build (`contains_nested`, `manifests`);
- the project's name and its `Found`: languages, package manager, lockfiles, tools, commands, and subfolders it owns (`claims`);
- why verify runs nothing for a step with no command (`Missing::NotNeeded` or `Missing::Skipped`, with the reason shown);
- the programs that mean "setup needed", what `fscale doctor` checks for (`Tool`: any of these programs, how to ask its version, what to install), and the build output `core/gitignore-basics` asks to ignore at the root.

`facts` asks the table for every folder, at the root and below, so the root and subfolder code paths are one. `verify`, `doctor`, `setup` and `core/gitignore-basics` read the table; nothing outside it names these kinds. Go moved into the table unchanged. Node, Python and Rust stay where they were: workspaces, interpreters and lockfile linking are theirs alone.

### The ecosystems

| Kind     | Marker                                                      | Test / lint / typecheck                                                                                                                                                                                                                                                                                                   |
| -------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `maven`  | `pom.xml`                                                   | `mvn -B test` (or `./mvnw`); lint from the Spotless, Checkstyle or PMD plugin; typecheck not needed (the test step compiles)                                                                                                                                                                                              |
| `gradle` | `build.gradle(.kts)`, `settings.gradle(.kts)`               | `gradle test` (or `./gradlew`); lint `lint` for Android (by plugin id, version-catalog alias or buildscript classpath), else Spotless, ktlint, detekt, Checkstyle                                                                                                                                                         |
| `swift`  | `Package.swift`                                             | `swift test`; SwiftLint or swift-format                                                                                                                                                                                                                                                                                   |
| `xcode`  | a `*.xcodeproj` or `*.xcworkspace` folder                   | typecheck `xcodebuild build -scheme S -destination "generic/platform=…" CODE_SIGNING_ALLOWED=NO` with the one shared scheme and the platform from `SDKROOT`, needing no simulator, device or certificate; tests only for a macOS app, otherwise skipped with the `verify.test` to set, because a simulator must be chosen |
| `cmake`  | `CMakeLists.txt`                                            | `cmake -S . -B build && cmake --build build && ctest --test-dir build --output-on-failure`; languages from `project(… LANGUAGES …)`, else the sources                                                                                                                                                                     |
| `meson`  | `meson.build`                                               | `meson setup build && meson test -C build` (Meson 1.12 re-runs `setup` on a configured folder without error)                                                                                                                                                                                                              |
| `dotnet` | `*.sln`, `*.slnx`, `*.csproj`, `*.fsproj`, `*.vbproj`       | `dotnet test X`, `dotnet format X --verify-no-changes` with the one solution, else the one project; several in one folder: skipped                                                                                                                                                                                        |
| `dart`   | `pubspec.yaml`                                              | `flutter test` and `flutter analyze` for a Flutter app, else `dart test` and `dart analyze`                                                                                                                                                                                                                               |
| `make`   | a Makefile with C-family sources or a `test`/`check` target | `make test`/`make check`, `make lint`, `make typecheck` when the target exists; never beside another kind (a Makefile that wraps another tool is that tool's)                                                                                                                                                             |

Wrappers: `./gradlew` and `./mvnw` when they are in the folder, as `gradlew` and `mvnw` on Windows (verify runs commands with `cmd /C` there, which finds `gradlew.bat` in the folder). The step cache asks `java --version` for them, so a JDK upgrade invalidates cached results.

### Which folder is which project

- **Members, not projects**: a Maven, Gradle, CMake, Meson or .NET build at the root takes the same kind's subfolders as members, and so does any folder whose manifest has the same manifest in a folder above it (a nested `CMakeLists.txt` is always part of a bigger build). Go modules, Swift packages and Dart packages nest as separate projects, as their tools treat them.
- **A Flutter app owns its platform folders** (`android`, `ios`, `macos`, `linux`, `windows`, `web`): they are runners `flutter` builds. A React Native app's `android/` and `ios/` are projects of their own, as they are built with Gradle and Xcode.
- **Swift and CMake in one folder**: the sources decide. C or C++ code (three levels into `src`, `include`, `lib`, `source`, `Sources`) makes it a CMake project that also ships a Swift package (nlohmann/json); otherwise a Swift package that also builds with CMake (swift-argument-parser). Meson beside CMake yields to CMake; Xcode beside `Package.swift` yields to the package.

### gitignore lines are read as git reads them

`core/gitignore-basics` and the stack packs' gitignore rules compared names. The real repos showed why that fails: `[Bb]in/` (Visual Studio's standard file) and `/build*/` were not seen, and `/.temp` at the root was wrongly taken to cover `supabase/.temp`. `firescale_core::walk::gitignore_covers` uses the `ignore` crate's gitignore matcher, already a dependency, with git's globs, anchors, directory-only lines and negations. The ecosystems' build output is required only for a project at the root: a bare `build` line there would ignore every `build` folder.

### Refactoring beyond the table

The stack packs (nextjs, supabase, expo) each had their own copies of the same helpers (`finding`, `package_json`, `depends_on`, `declared_version`, `create`, the JSON line insert). They are one module, `crates/rules/src/stackkit.rs`; 230 lines went.

## Proof

- **Run for real in the build environment** with `fscale verify`: a CMake project with a CTest test, a Make project, a Meson project (Meson 1.12), a Maven project with JUnit 5, and a Gradle project with JUnit 5. All pass. The Gradle one fails on a planted wrong assertion (`CalcTest > adds() FAILED`), and passes once it is put right. `fscale doctor` finds Java behind a `JAVA_TOOL_OPTIONS` line, and Gradle.
- **Real repos**, read-only shallow clones, `fscale facts` and `core/gitignore-basics`, nothing of theirs run: `android/nowinandroid` (one Gradle project, Android lint through a version-catalog alias), `apple/swift-argument-parser` (one Swift package; its CMake files are members), `nlohmann/json` (one CMake project in C++, not the Swift package or Meson build it also ships), `dotnet/eShop` (the solution, `dotnet test "eShop.slnx"`), `flutter/gallery` (one Flutter app; its five platform runners are its own). Each of these was wrong in the first pass and is a test now.
- Tests: `crates/core/src/ecosystems/tests.rs` (every kind, the yields, members, claims, the real-repo layouts), the gitignore matcher's tests, and the facts, graph and verify schemas.

## Not checked, and limits

- **Swift, Xcode, .NET and Flutter were not run**: their tools are not in the build environment. Their commands come from their tools' documented usage and are checked by tests on fixtures only.
- **Xcode tests need a simulator** chosen by a person, except for macOS apps; schemes that are not shared are not seen.
- **No parsing of Gradle, Maven or CMake dependencies for the project graph**, so `--affected` treats each of these projects as depending on nothing else in the repo; a Makefile's targets are read line by line.
- **The tamper guard** still reads JavaScript, TypeScript, Python and Rust test files only (the owner chose project support without new parsers).
- **The generated CI workflow** sets up Node and Python; GitHub's Ubuntu runners already have Java, Maven, Gradle, CMake, .NET and Go, and its macOS runners Swift and Xcode, but no workflow step is generated for them.
- Lint for C and C++ is never guessed: clang-tidy and clang-format need a file list. `verify.lint` names it.

## Consequences

- `fscale facts`, `verify`, `doctor` and the gitignore rule work in Java, Kotlin, Android, Swift, Xcode, C, C++, .NET and Dart/Flutter repos, and in monorepos that mix them with Node, Python, Rust and Go.
- A new ecosystem is one table entry and its tests.
