---
name: test-tools
description: Fixes violations of the firescale rule python/test-tools by declaring pytest for a Python project. Use when fscale check reports python/test-tools.
---

# python/test-tools

## What this rule enforces and why

Each Python project declares pytest among its dependencies (in a requirements file it installs, or a dev group in `pyproject.toml`), so `python -m pytest` works once they are installed.

A test tool that only some machines have makes tests pass on one laptop and fail on the next.

## Manual fix

1. If the project has `requirements.txt` and no `requirements-dev.txt`, run `fscale sync`: it creates `requirements-dev.txt` with `-r requirements.txt`, pytest and ruff.
2. Otherwise add pytest to the development list the project already has.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync` when it reports a deterministic fix.

### REPORT

- A project whose development list exists already: report that pytest should be added to it. Do not edit a requirements file or `pyproject.toml` without the user.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules python/test-tools
```
