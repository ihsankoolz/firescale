# 0069 — The project-type table, step 1: each ecosystem lists the files it reads, and the cache inputs are built from that

Status: proposed (2026-11-18, 0.5.3 run). **Not merged: it waits for the owner.** Amends ADR 0051 (ecosystems). First step of part C of `docs/reviews/ponytail-audit.md`.

## Context

The audit's part C found that "what kind of project is this folder, and which files decide that?" is answered in at least seven places, with lists kept by hand. The cost is not the copies but the cache: a rule that writes the repo's commands into a file (AGENTS.md, the agents' permissions, `.gitignore`) keeps its result until one of its inputs changes, and its inputs are those hand-kept lists. A file that facts reads and no list names gives a stale pass.

One such list was `ecosystems::MARKER_GLOBS`: 22 globs written beside the table of ecosystems, which had to track every entry's `marks` function. It held the marker files only. The files an entry's `found` reads to choose a command were in it by luck or not at all: `.swiftlint.yml`, `.swiftlint.yaml` and `.swift-format` (a Swift package's lint command), `cmake/lint.cmake`, `conanfile.txt`, `vcpkg.json`, the shared Xcode schemes. `mvnw` and `gradlew` were in a second hand-kept list, `FACTS_INPUTS`, added after a bug.

A test shows it: with a cached pass of `agent-ready/agents-md` for a Swift package, adding `.swiftlint.yml` changes the lint command that `fscale facts` reports, and the cached pass still stood.

Part C as a whole (Node, Python and Rust as entries of the table; the packs' "another stack" lists, the cmake pack's lists and the step cache's probes built from it) changes cache keys and reverses the module's rule that those three kinds "stay in facts". It is too large for one change, and this run had no local build: every line here was compiled and tested by GitHub CI only.

## Decision

Step 1 only:

- **Each entry of `ECOSYSTEMS` has an `inputs` list**: every file the entry reads to recognise the folder, to name the project and to choose its commands and tools, by its path in the project's folder (`*` for any name).
- **`ecosystems::input_globs()` builds the globs from the entries** (`**/<input>`, sorted, no repeats). It takes the place of `MARKER_GLOBS`, which is removed. `agent-ready/agents-md`, `agent-ready/agent-permissions` and `core/gitignore-basics` use it where they used the constant.
- Every glob of the old list is still among the inputs (a test names all 22), so no cached result that was invalidated before stays valid now.
- `FACTS_INPUTS` (Node, Python, Rust and Go files, and the wrappers) stays as it is, by hand. It goes when those kinds are entries of the table.

Left for later steps, each with its own ADR or an amendment of this one:

1. Node, Python and Rust as entries, so `FACTS_INPUTS` and `DISCOVERY_SKIPPED_DIRS` can be built from the table.
2. Code extensions and vendored folder names per entry, so `stackkit::MANIFESTS` and `CODE` and the cmake pack's five lists are built from the table.
3. The step cache's probes from each entry's `programs`.

## Consequences

- A Swift linter's config, `cmake/lint.cmake`, the C package managers' files, a shared Xcode scheme, `go.sum`, `Package.resolved`, `pubspec.lock`, `gradle.lockfile` and `packages.lock.json` are now cache inputs of the three rules. The first run after the upgrade checks those rules again once: their inputs changed.
- **The list is still kept by hand, but in one place, beside the code that reads the files.** Nothing checks that an entry's `inputs` names every file its functions read. A new read with no new input is the same bug as before, one screen away from its fix instead of in another crate.
- **What a list of file names cannot say is not covered:** an entry that looks for a folder (`src/main/kotlin`, a Dart package's `test/`, a Swift package's `Sources/`) or for source files by extension (`c_family`) is not invalidated when one appears. That was so before.
- More inputs mean more files hashed for those three rules' keys. Not measured.
- Tests: three new cases in `packages/cli/src/facts-inputs-cache.test.ts` (written first), and `the_cache_inputs_are_built_from_the_table` in `crates/core/src/ecosystems/tests.rs`.
