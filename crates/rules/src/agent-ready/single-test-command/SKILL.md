---
name: single-test-command
description: Fixes violations of the firescale rule agent-ready/single-test-command by giving the repo one command that runs its tests. Use when fscale check reports agent-ready/single-test-command.
---

# agent-ready/single-test-command

## What this rule enforces and why

The repo has one command that runs its tests, and firescale can find it. Any one of these at the repo root is enough:

- a `test` script in `package.json` (npm's `no test specified` placeholder is still a script, so the rule `core/scripts-present` reports that case, not this rule),
- a `Cargo.toml` (`cargo test`),
- pytest configuration: a `pytest.ini`, or a `[tool.pytest...]` table in `pyproject.toml` (`pytest`),
- a `Makefile` with a `test` target (`make test`).

The rule only reports when a language is detected (JavaScript, TypeScript, Python or Rust) and none of these is present. It does not check that the command passes; `fscale verify` does that.

An agent can only prove its work if it can run the tests, and it will only do so reliably when there is exactly one obvious way.

## Manual fix

1. Find how the tests are run today: the README, the CI workflow, or a teammate.
2. Put that command behind a standard entry point: a `"test"` script in `package.json`, a `[tool.pytest.ini_options]` table in `pyproject.toml`, or a `test:` target in a `Makefile`.
3. If the repo has no tests yet, add a test runner and one real test first.

## Agent fix instructions

### DETERMINISTIC

None.

### REPORT

Look for an existing way to run the tests (CI config, README, test folders, a test runner in the dependencies). If you find one, add the matching entry point from the list above and nothing else. Do not install a test runner and do not write placeholder tests or a script that always succeeds. Report:

- The entry point you added and the command it runs.
- If the repo has no tests or no test runner, say so and stop, so the user can choose one.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules agent-ready/single-test-command
```
