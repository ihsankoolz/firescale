---
name: tests
description: Handles violations of the firescale rule maven/tests, which needs a test under src/test in a Maven project with code. Use when fscale check reports maven/tests.
---

# maven/tests

## What this rule enforces and why

A Maven project with Java code under `src/main` has at least one test under `src/test`. Without one, `mvn -B test` reports no tests and passes, so `fscale verify` and CI prove nothing about the code.

## Manual fix

1. Add a test class under `src/test/java`, in the package of the class it tests, named `<Class>Test`, with a JUnit `@Test` method.
2. Run `mvn -B test`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

1. Check which test framework the POM already has (JUnit 5 or 6 `junit-jupiter`, JUnit 4 `junit`, TestNG). If none, report that a test dependency must be added and stop: adding one changes the build.
2. Pick the class with the most important public method, and write one test for it under `src/test/java`, in the same package, with that framework.
3. Run `mvn -B test` and make it pass without changing the code under test.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules maven/tests
```
