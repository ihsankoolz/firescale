---
name: gitignore
description: Fixes violations of the firescale rule python/gitignore by ignoring Python's caches and virtual environments. Use when fscale check reports python/gitignore.
---

# python/gitignore

## What this rule enforces and why

`.gitignore` ignores `__pycache__/`, `.venv/`, `venv/`, `.pytest_cache/` and `.ruff_cache/`, which Python, pytest, ruff and `python -m venv` write in every project folder.

Committed caches and environments are large, machine-specific and noisy in every diff.

## Manual fix

1. Run `fscale sync`, or add the lines to `.gitignore`.
2. If any of them is tracked already, untrack it: `git rm -r --cached <path>`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It appends the missing lines.

### REPORT

- Caches or environments that are already tracked: report the `git rm -r --cached` command.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules python/gitignore
```
