---
name: entry
description: Fixes violations of the firescale rule maven/entry by writing a class with a main method and its JUnit test for a project with no Java code. Use when fscale check reports maven/entry.
---

# maven/entry

## What this rule enforces and why

The project has Java code under `src/main/java` or `src/test/java`, where Maven reads it. With none, `mvn -B test` compiles and runs nothing, so a check that passes proves nothing. A new project gets `com.example.app.App`, a program that prints a greeting, and `AppTest`, a JUnit test of it.

A POM that only aggregates other projects (`<packaging>pom</packaging>` or `<modules>`) has no code of its own and is not checked.

## Manual fix

1. Run `fscale sync`, or write your first class under `src/main/java` and a test under `src/test/java`.
2. Move the package `com.example.app` to the project's own (its `groupId` and `artifactId`).

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It writes `src/main/java/com/example/app/App.java` and `src/test/java/com/example/app/AppTest.java` when the project has no Java file, and never replaces one.

### REPORT

- Tell the user the starter is a placeholder: replace `App` and its test with the project's real code, in its own package.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules maven/entry
```
