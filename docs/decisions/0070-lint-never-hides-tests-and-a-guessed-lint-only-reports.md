# 0070 — A failing lint never hides the tests, and a lint command fscale guessed only reports

Status: accepted (2026-11-18, 0.5.4 run; the owner's decision). Amends ADR 0022 (projects are independent, and the first failure stops a project), ADR 0051 (ecosystems) and the verify JSON (schema version 8).

## Context

`fscale verify` ran on 15 public repos (`docs/reviews/real-repos-0.5.3.md`). In 4 of them the result was wrong for one cause, the review's bug 1:

- fscale picked a lint command the repo never adopted: `dotnet format --verify-no-changes` for ardalis/GuardClauses and serilog/serilog, `flutter analyze` for rrousselGit/provider, `swift format lint --strict` for apple/swift-argument-parser.
- That command failed on code the repo's own CI accepts.
- The failure stopped the project's steps, so tests that pass were `not run`, and the headline was `FAILED`.

These were first runs on a default branch, so there was no base commit and blame-aware verify (ADR 0037) could not label the failure `pre-existing`.

The owner decided both halves, on 2026-11-18.

## Decision

**1. A lint step never stops the steps after it.** In one project (or at the repo root), `typecheck` and `test` run whether `lint` passed, failed, needs setup or timed out, and each step is reported. A failing lint still fails the run as before, unless rule 2 makes it report-only. Nothing else changes: a failing `check` still stops everything, a failing `typecheck` still stops the project's `test`, and with a base commit every step ran already (ADR 0037).

**2. A lint command fscale guessed, that the repo does not declare itself, is report-only.** It runs and it is shown. It never fails the run: not when it fails, not when its program is missing, not when it times out.

"Declares itself" is one of three things:

- a package script or a build target named for lint,
- a config file of that linter in the repo,
- the command in the repo's CI files.

Most lint commands fscale runs are found through the first two, so they are declared by how they were found. Five are run for every project of their kind with nothing asked. Those five are the guesses, and for each the list says what makes it declared.

| Project kind      | Lint command                                                  | How fscale finds it                        | Declared when                                                                                                                                                                                                 |
| ----------------- | ------------------------------------------------------------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Node              | `<pm> run lint`                                               | the `lint` script of `package.json`        | always: a package script                                                                                                                                                                                      |
| Node              | `<exec> oxlint`, `<exec> eslint .`, `<exec> biome check .`    | the linter's config file                   | always: a config file                                                                                                                                                                                         |
| Nx, Turborepo     | `nx run-many -t lint`, `turbo run lint`                       | the workspace's own task                   | always: a build target                                                                                                                                                                                        |
| Python            | `python -m ruff check .`                                      | ruff in the project's own dependency lists | always (see "Decided on the owner's behalf")                                                                                                                                                                  |
| Maven             | `mvn -B spotless:check`, `checkstyle:check`, `pmd:check`      | the plugin in `pom.xml`                    | always: the build file configures the linter                                                                                                                                                                  |
| Gradle            | `spotlessCheck`, `ktlintCheck`, `detekt`, `checkstyleMain`    | the plugin in the build script             | always: the build file configures the linter                                                                                                                                                                  |
| Swift, Xcode      | `swiftlint lint --strict`                                     | `.swiftlint.yml` or `.swiftlint.yaml`      | always: a config file                                                                                                                                                                                         |
| Swift, Xcode      | `swift format lint --recursive --strict .`                    | `.swift-format`                            | always: a config file                                                                                                                                                                                         |
| CMake             | `cmake --build build --target lint`                           | a `lint` target in `CMakeLists.txt`        | always: a build target                                                                                                                                                                                        |
| Make              | `make lint`                                                   | a `lint` target in the `Makefile`          | always: a build target                                                                                                                                                                                        |
| **Rust**          | `cargo clippy [--workspace] --all-targets -- -D warnings`     | every Cargo project                        | `clippy.toml` or `.clippy.toml`; or a line starting with `[lints` or `[workspace.lints` in a `Cargo.toml`; or `cargo clippy` in a CI file                                                                     |
| **Go**            | `go vet ./...`                                                | every Go module                            | `go vet` in a CI file                                                                                                                                                                                         |
| **.NET**          | `dotnet format "<solution or project>" --verify-no-changes`   | every folder with one solution or project  | `dotnet format` (or `dotnet-format`) in a CI file                                                                                                                                                             |
| **Dart, Flutter** | `dart analyze`, `flutter analyze`                             | every `pubspec.yaml`                       | `analysis_options.yaml`; or `dart analyze` or `flutter analyze` in a CI file                                                                                                                                  |
| **Android**       | `./gradlew lint` (or `gradle lint`)                           | every Gradle build with the Android plugin | `lint.xml`; or a line starting with `lint {` or `lintOptions` in a `build.gradle` or `build.gradle.kts`; or a Gradle command with a task starting with `lint` (`lint`, `lintDebug`, `:app:lint`) in a CI file |
| any               | a command in `verify.lint` or `verify.projects.<folder>.lint` | the firescale config                       | always: the repo wrote it                                                                                                                                                                                     |

How the three kinds of sign are read (`crates/core/src/lintsource.rs`):

- **A config file, and a line in a build file,** count in the project's folder and in every folder above it up to the repo root. For Android the folders directly below the project count too, because a Gradle module keeps its `lint {}` block in its own folder. A file in a sibling folder says nothing.
- **CI files** are every `.yml` and `.yaml` file directly in `.github/workflows`, and at the repo root `.gitlab-ci.yml`, `.circleci/config.yml`, `azure-pipelines.yml`, `bitbucket-pipelines.yml`, `.travis.yml`, `Jenkinsfile`, `.buildkite/pipeline.yml`, `appveyor.yml` and `.appveyor.yml`. A line counts when it holds the command's words next to each other (`go vet`, not `go veto`). A line that is only a comment does not count. Which project the line is for is not looked at: one `go vet` anywhere in CI declares it for every Go module of the repo.
- **`.editorconfig` is not a sign for `dotnet format`.** Editors read it too, and both .NET repos of the review have one and run no formatter.

**3. The summary says it plainly.** A guessed lint that did not pass has the status `report-only` in JSON and reads `failed` (lower case) with `report-only:` before its detail in the summary. The headline keeps its result and adds `- report-only: lint in <project> failed (a lint command fscale guessed)`, so it never reads as a plain `OK`. Below the steps:

```
Report-only: lint failed, and that does not fail this run. fscale guessed this command, and nothing in this repo asks for it (looked for `dotnet format` in a CI file).
To make it block, add this to your firescale config: "verify": { "lint": "dotnet format \"GuardClauses.sln\" --verify-no-changes" }
```

For a project below the root the config line is `"verify": { "projects": { "<folder>": { "lint": "<command>" } } }`. The last lines of the command's output follow, as for a failure.

A guessed lint that **passes** reads as any passed step. In JSON every step of a guessed lint command has `reportOnly`, with the reason, whether it passed or not.

## Decided on the owner's behalf

- **Ruff found in a project's own dependency lists counts as declared.** The owner's list has three kinds of sign and a dependency is not one of them. But fscale only runs ruff when the project lists it (ADR 0022), which is the repo choosing the linter, and the course project's five services are found this way. Read strictly, their lint would stop blocking with no word from the owner. **Say if a dependency should not count**: then ruff needs `ruff.toml`, `.ruff.toml`, a `[tool.ruff]` table or `ruff` in a CI file.
- **A lint step stops nothing after it, whatever its status**: failed, setup needed or timed out. The owner's sentence says "failing". A linter that is not installed says nothing about whether the tests can run (SwiftyJSON in the review), so the tests run. When nothing is installed the test step then needs setup too, and the summary lists both.
- **`typecheck` also runs after a failing lint**, not only `test`.
- **A config file of the linter makes the lint blocking even where the repo's CI does not run it that strictly.** apple/swift-argument-parser has a `.swift-format` file and does not run `swift format lint --strict`; rrousselGit/provider has `analysis_options.yaml` and `flutter analyze` exits 1 on `info` lines. By the owner's rule both stay `FAILED`, now with their tests run and reported beside the lint. The command's flags are not changed here.
- **The status is a new one, `report-only`,** not `failed` with a flag: a reader of the JSON who knows only the old statuses cannot take it for a failure. Hence schema version 8.
- **`.editorconfig` does not count**, as above.

## Consequences

- **A Rust, Go, .NET, Dart, Flutter or Android project with none of the signs above no longer fails `fscale verify` on its lint.** The step still runs and its output is shown. To get the old behaviour, add the config line the summary prints.
- On a repo whose lint fails and whose tests fail, the headline now names both, where it named only the lint.
- A project where nothing is installed gets a setup-needed line for its tests as well as for its lint.
- The stop hook lets an agent stop when the only step that did not pass is a report-only lint: the run's exit code is 0.
- A report-only lint is not run on the base commit and gets no blame label: it blocks nothing, so there is nothing to excuse.
- A failing step is never cached (ADR 0036), so a report-only lint that fails runs again on every verify.
- **Verify's JSON is schema version 8**: the step status `report-only` and a step's `reportOnly`.
- Whether a guess is declared is read from the working tree when verify plans its steps. Adding `go vet` to a workflow file makes the next run's lint blocking.
- Not done: `fscale verify --dry-run` does not say which lint commands are guesses; `fscale facts` does not either. Doctor still looks only for SwiftLint among the linters (ADR 0051, #76).

## Tests

- `packages/cli/src/guessed-lint.test.ts`, written first and seen to fail (7 of 7): fixtures cut down from GuardClauses and serilog (`dotnet format` exits 2, `dotnet test` passes), provider (`flutter analyze` exits 1 on `info` lines) and swift-argument-parser (a `.swift-format` file, `swift format lint --strict` exits 1, `swift test` passes), with stand-ins for `dotnet`, `flutter`, `swift` and `go`.
- `crates/core/src/lintsource.rs`: each guessed command, each sign that declares it, what does not count, the config line, and `the_commands_facts_detects_are_the_ones_read_here`, which fails when `fscale facts` writes one of the five commands another way.
- Changed, because they pinned that a failing lint stops the project's tests: `one_projects_failure_does_not_stop_the_others` and `project_steps_run_in_the_projects_folder_and_a_failure_names_it` (`crates/core/src/verify.rs`), and two tests of `packages/cli/src/python-env.test.ts`. The first now also checks that a failing `typecheck` still stops the project's tests.
