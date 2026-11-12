---
name: ruff-config
description: Fixes violations of the firescale rule python/ruff-config by creating ruff.toml for a Python project that uses ruff. Use when fscale check reports python/ruff-config.
---

# python/ruff-config

## What this rule enforces and why

A Python project that declares ruff (or a new one) has a ruff config: `ruff.toml`, `.ruff.toml` or `[tool.ruff]` in `pyproject.toml`, in its folder or a folder above. The file the fix writes sets `line-length = 100` and the rule sets E, F, W and I.

Without it, ruff's defaults decide, and an upgrade can change what passes.

## Manual fix

1. Run `fscale sync`, or create `ruff.toml` in the project's folder.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`.

### REPORT

- Nothing.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules python/ruff-config
```
