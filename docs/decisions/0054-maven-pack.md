# 0054 — The `maven` stack pack

Status: accepted (2026-11-14, M34). Follows the stack-pack shape of PLAN.md and ADRs 0029, 0049, 0050 and 0053; builds on the Maven project support of ADR 0051.

## Context

The second pack for the new languages (PLAN.md "M33 to M39"). The plan named Java "with Gradle". Gradle's starter, as `gradle init` writes it, runs through its wrapper, and the wrapper is a binary `gradle-wrapper.jar` that a fix, which writes text, cannot create; without it, a Gradle starter needs Gradle installed and gives no version to build with. Maven's starter is all text, Maven and a JDK are in the build environment, and Maven Central is reachable, so a Maven starter can be built and run here. **Java is served with Maven; a Gradle pack is not built** (Gradle projects keep the project support of ADR 0051, and Android's pack, M38, uses Gradle).

## What the pack assumes (read on 2026-11-14)

From Maven Central's metadata, Maven's and JUnit's documentation, and **checked by running** the starter with Maven 3.9.11 and JDK 21.0.12:

- **Versions** (newest stable): JUnit 6.1.3 through its bill of materials (JUnit 6 needs Java 17), maven-compiler-plugin 3.16.0 (4.0.0-beta-5 is newer but a beta), maven-surefire-plugin 3.6.1, maven-checkstyle-plugin 3.6.0 with Checkstyle 14.4.0. Maven 3.9 binds older compiler and Surefire plugins by default, so the starter pins them.
- **Java 21**: the starter builds for release 21, a long-term-support release that every newer JDK builds for (25, from 2025, is the newest long-term-support release), and the JDK here.
- **Checkstyle with Google's rules** (`google_checks.xml`, built into Checkstyle) and `violationSeverity` `warning`, so `mvn -B checkstyle:check` fails on any finding; ADR 0051 makes it the project's lint command, so `fscale verify` runs every step on the starter rather than skipping lint. The starter's code is in Google's style (two-space indents, Javadoc on public members). Run here: no finding on the starter, and a build failure ("You have 1 Checkstyle violation") on one line indented by four.
- **Without `project.build.sourceEncoding`** Maven reads sources in each machine's encoding and warns that the build "is platform dependent".
- **The Maven wrapper is not written.** Its `only-script` form is text (`mvnw`, `mvnw.cmd`, `.mvn/wrapper/maven-wrapper.properties`), but `mvnw` is a 300-line script of the Maven project's, which firescale would be copying, and it must be executable, which a fix cannot make so on every system. `maven/pom`'s skill gives the one command that adds it (`mvn -N wrapper:wrapper -Dtype=only-script`); the starter needs Maven on the PATH, which `fscale doctor` checks (ADR 0051).

## The rules

| Rule                        | Checks                                                                                                             | Fix                                                                                                                                                                                       |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `maven/pom` (high)          | the root `pom.xml` has `modelVersion`, `groupId`, `artifactId`, `version` (or a `<parent>` for the first and last) | creates the starter POM for `com.example:app:0.1.0-SNAPSHOT`; a missing coordinate is for a person                                                                                        |
| `maven/properties` (medium) | the POM sets the source encoding and a Java version (a POM with a `<parent>` is trusted)                           | adds `project.build.sourceEncoding` UTF-8 to the project's `<properties>` (not a profile's), or a new block, in the file's indentation and line endings; the Java version is for a person |
| `maven/entry` (high)        | the project has Java code under `src/main/java` or `src/test/java` (an aggregator POM is not checked)              | writes `com.example.app.App` and its JUnit test when there is no Java file                                                                                                                |
| `maven/tests` (medium)      | code under `src/main` has a test under `src/test`                                                                  | none: an agent writes one with the framework the POM already has                                                                                                                          |

**The Java version of an existing POM is never set by a fix**, for the reason ADR 0053 found for Go's `go` line: building for a version the code or its users do not run on changes what runs where. `.gitignore` gets `target/` from `core/gitignore-basics` (ADR 0051), so the pack has no rule of its own for it.

**Where it applies**: a repo with a root `pom.xml`, or one new to Java (no other stack's manifest or code anywhere, and no `pom.xml` below). A `pom.xml` only in a subfolder of a repo that is no other stack's gets one finding naming it. The question "is this repo another stack's?" is now one helper for every pack (`stackkit::other_stack`), which the go pack uses too.

## Proof

- **Run for real**: in an empty folder, `fscale init --packs core,maven` leaves no violations; `mvn -B test` (1 test, 0 failures), `mvn -B package -DskipTests` and `java -cp target/classes com.example.app.App` (`Hello, world!`) succeed without warnings; `target/` is ignored; `fscale verify` is OK on every step (check, `mvn -B checkstyle:check`, `mvn -B test`).
- **Tests**: each rule's `tests.rs` (passing, failing, fixed, the fix run twice, CRLF, profiles, comments, a parent, a self-closing `<properties/>`), the pack's tests (an empty folder built by the fixes, a second run changes nothing, other stacks left alone), and `packages/cli/src/maven-pack.test.ts` (`init` with snapshots, `sync` changes nothing, the starter's facts with its lint command, `adopt --dry-run` on a set-up project and on one missing its encoding).
- **Real repos**, shallow clones, read-only, the pack alone: apache/commons-lang (a parent), spring-projects/spring-petclinic (Spring Boot's parent), google/guava (an aggregator) and jhy/jsoup get no finding; square/okhttp, a Gradle repo, got a finding about the test fixture `maven-tests/pom.xml` in the first pass and gets none now (the go pack had the same flaw and is fixed with it).

## After the review

A fresh subagent tried the pack on 19 POM layouts (checking each result with `mvn -B validate` and the effective `project.build.sourceEncoding`) and on eight real projects (junit4, gson, jsoup, commons-cli, fastjson2, metrics, jackson-databind, maven-archetypes, all without a finding). It found, all fixed with tests:

- **The `maven/properties` fix broke valid POMs.** It read lines, not elements: with `<properties>` and its content on one line, or a POM on one line, it wrote outside the block or after `</project>`; with no project `<dependencies>`, a plugin's `<dependencies>` was its anchor, so a `<properties>` block landed inside a `<plugin>`; and a plugin's `<configuration><properties>` was taken for the project's. **A small reader of the POM's element structure** (`maven/xml.rs`: paths and byte ranges, skipping comments, CDATA, processing instructions and the doctype, and returning nothing for a document it cannot follow) now finds `project/properties` and the project-level anchors exactly; the fix inserts at element boundaries, one step deeper than the parent with the file's own step (tabs, four spaces); a POM it cannot read, a self-closing `<properties/>` or a prefixed `pom:project` (which Maven rejects, run here) is left to a person. All 19 layouts now give a POM Maven accepts with UTF-8 sources, and a second run changes nothing.
- **Values anywhere in the file counted**: an antrun `<target>`, a resources plugin's `<encoding>`, a profile's property, or flatten-maven-plugin's `<pomElements><parent>` setting hid a missing Java version, encoding or coordinates. They are read by path now: the project's own properties and coordinates, and the compiler plugin's own configuration (also in an execution's, as jsoup sets it).
- **Groovy-DSL Gradle, Ant and Bazel Java repos were given a Maven starter**: `settings.gradle`, `build.xml` and Bazel's files are other stacks now (for every pack).
- **A Kotlin, Scala or Groovy project was given a Java starter**: their sources are a project's code.
- A non-ASCII POM on one line could make the rule panic; the reader works on element boundaries only.

## Not checked, and limits

- Gradle projects: no pack (above).
- Modules of a multi-module build are not checked one by one; an aggregator's root is checked for its coordinates and properties only.
- The Java version is checked for presence, not against the JDKs the project's CI uses.
- `maven/tests` and `maven/entry` read Java, Kotlin, Scala and Groovy sources under `src/main` and `src/test`; a custom `<sourceDirectory>` elsewhere is not read.
- The POM reader is not a full XML parser: entities and a DTD's defaults are not expanded.
- Windows and macOS: the starter was built on Linux only; the rules are text, and CI runs the tests on all three.

## Consequences

- `fscale init --packs core,maven` starts a Java project that builds, lints and tests with Maven, and `fscale verify` and an agent's stop hook run all three.
- `fscale adopt` gives an existing Maven project its source encoding, and names a missing Java version or test for a person or an agent.
