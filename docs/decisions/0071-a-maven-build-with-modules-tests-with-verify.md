# 0071 — A Maven build with modules is tested with `mvn verify`

Status: accepted (2026-11-18, 0.5.4 run). Amends ADR 0051 (ecosystems) and ADR 0054 (the Maven pack).

## Context

fscale's test command for every Maven project was `mvn -B test`. On google/gson (`docs/reviews/real-repos-0.5.3.md`, bug 4) that fails: the module `test-jpms` tests the packaged jar of the `gson` module, and `test` stops before `package`, so the jar is not there.

Run on the laptop on 2026-11-18, at gson's commit `e67037d`, with Maven 3.9.14 and JDK 17:

- `mvn -B test`: exit 1. `test-jpms` does not compile: `module-info.java:[19,22] module not found: com.google.gson`. The five modules after it are skipped.
- `mvn -B verify`: exit 0. All 8 modules succeed, in 1m 19s.

gson's README names `mvn clean verify`, and its CI runs `mvn verify`.

The owner asked for the simplest rule that is right for gson and does not change the other Maven repo of the review, apache/commons-cli.

## Decision

- **When the project's `pom.xml` lists modules, the test command is `mvn -B verify`** (or `./mvnw -B verify`). Without them it stays `mvn -B test`.
- "Lists modules" is read from the text, not from parsed XML: a `<modules>` tag whose first child is a `<module>`, outside `<!-- -->` comments, anywhere in the file. So modules inside a `<profile>` count, and a plugin's own `<modules>` setting does not (the EAR plugin's holds `<webModule>` and `<ejbModule>`).
- Nothing else changes: the build command is still `mvn -B package -DskipTests`, the lint commands are the same, and the modules are still not projects of their own (ADR 0051).

Why this rule: in a build of one module nothing can depend on another module's jar, so `test` is enough and is faster. In a build of several, one module can use what another packages, and only the build's later phases make it. `verify` is the phase Maven's own docs give for "run every check", and it includes `test`.

## Consequences

- **A Maven build with modules now runs `package` and `integration-test` in the test step.** That takes longer, and it runs integration tests (Failsafe) that `mvn test` left out. A repo whose integration tests need a service that is not there will now fail where it passed. It sets `verify.test` (or `verify.projects.<folder>.test`) to `mvn -B test` to go back.
- apache/commons-cli has no `<modules>`, so its command is unchanged. Its own CI runs the pom's default goal, which includes `verify`; that was not followed here, because the review found its tests right under `mvn -B test`.
- A single-module build that needs `verify` (its tests use its own jar) still gets `test`. Not seen in the review; the config can name the command.
- The command is in the step's cache key, so a build with modules runs its tests again once after the upgrade.
- The messages of the Maven pack's rules and the reason for the missing typecheck step still say `mvn test`. They are right for a build without modules, and the pack's starter is one.
- The files this reads are already cache inputs (`pom.xml`, ADR 0069).

## Tests

- `a_maven_build_with_modules_tests_with_verify` in `crates/core/src/ecosystems/tests.rs`, written first and seen to fail (`left: Some("mvn -B test")`, `right: Some("mvn -B verify")`): a pom shaped like gson's gives `verify`; with the wrapper and with the modules in a profile it is `./mvnw -B verify`; a pom with no modules, three with `<modules>` only in a comment, and one with the EAR plugin's `<modules>` keep `test`.
- The third real-repos run checks gson itself.
