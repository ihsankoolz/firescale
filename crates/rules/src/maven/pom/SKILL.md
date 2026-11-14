---
name: pom
description: Fixes violations of the firescale rule maven/pom by creating pom.xml, and explains how to add a coordinate it lacks. Use when fscale check reports maven/pom.
---

# maven/pom

## What this rule enforces and why

The repo root has a `pom.xml` with `modelVersion` 4.0.0 and the project's coordinates: `groupId`, `artifactId` and `version` (`groupId` and `version` may come from a `<parent>`). Without them Maven stops before it builds anything.

A new project gets a POM for `com.example:app:0.1.0-SNAPSHOT`, built for Java 21 (`maven.compiler.release`), with UTF-8 sources, JUnit 6 through its bill of materials, the compiler (3.16.0) and Surefire (3.6.1) plugins pinned, since Maven 3.9 binds older ones by default, and Checkstyle 14.4.0 with Google's rules, which fails `mvn checkstyle:check` (`fscale verify`'s lint step) on any warning.

## Manual fix

1. Run `fscale sync`, or write `pom.xml` by hand.
2. Change `com.example` and `app` to the project's own `groupId` and `artifactId`, and move the code's package to match.
3. To build without Maven installed, add the Maven wrapper: `mvn -N wrapper:wrapper -Dtype=only-script`, and commit `mvnw`, `mvnw.cmd` and `.mvn/wrapper/maven-wrapper.properties`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `pom.xml` when there is none, and never edits one.

### REPORT

- A missing coordinate: add it as a child of `<project>`. Ask the user for the `groupId` (usually the organisation's reversed domain) and `artifactId` if the repo does not show them.
- A `pom.xml` only in a subfolder: report where it is. Do not create one at the root.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules maven/pom
```
