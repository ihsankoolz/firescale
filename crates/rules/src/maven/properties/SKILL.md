---
name: properties
description: Fixes violations of the firescale rule maven/properties by setting the source encoding in pom.xml, and explains how to set the Java version. Use when fscale check reports maven/properties.
---

# maven/properties

## What this rule enforces and why

`pom.xml` says how to read the sources and which Java version to build for:

- `project.build.sourceEncoding` (or the compiler plugin's `<encoding>`). Without it Maven reads sources in each machine's own encoding and warns that "the build is platform dependent": a file that builds on Linux can fail on Windows.
- The Java version: `maven.compiler.release` (or `maven.compiler.source` and `target`, or the compiler plugin's own `<release>`). Without it the compiler plugin picks its own default, which changes between plugin versions.

A POM with a `<parent>` is not checked: the parent may set both.

## Manual fix

1. Run `fscale sync` to add `<project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>`.
2. Add `<maven.compiler.release>N</maven.compiler.release>` to `<properties>`, where N is the oldest Java version the project must run on (21 for a new project).

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It adds the encoding line to the project's `<properties>`, or a `<properties>` block when there is none, in the file's indentation and line endings. A POM it cannot read, a self-closing `<properties/>`, or a POM whose elements carry a namespace prefix is left to you (the finding then says "manual").

### REPORT

- The Java version: find which version the project's code and its CI use (the JDK in `.github/workflows`, `.java-version`, `.sdkmanrc`), set `maven.compiler.release` to it, and run `mvn -B test`. If nothing shows it, ask the user: building for a newer version can stop the code running where it runs today.
- If the sources are not UTF-8, report which files: do not convert them.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules maven/properties
```
