---
name: tests
description: Fixes violations of the firescale rule python/tests by giving a Python project tests. Use when fscale check reports python/tests.
---

# python/tests

## What this rule enforces and why

Each Python project has tests: a `tests/` (or `test/`) folder, or `test_*.py` files. A repo with no Python project yet gets `app/__init__.py`, `app/main.py` and `tests/test_main.py`.

A project with no tests passes `fscale verify` without proving anything.

## Manual fix

1. For a new project, run `fscale sync`.
2. For an existing project, add a `tests/` folder with tests for its code.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. For a new project, run `fscale sync`.

### REPORT

- An existing project with no tests: report it, and offer to write tests for its main functions.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules python/tests
```
