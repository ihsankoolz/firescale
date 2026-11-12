---
name: pytest-config
description: Fixes violations of the firescale rule python/pytest-config by creating pytest.ini for a Python project. Use when fscale check reports python/pytest-config.
---

# python/pytest-config

## What this rule enforces and why

Each Python project configures pytest: `pytest.ini`, `[tool.pytest.ini_options]` in `pyproject.toml`, `[tool:pytest]` in `setup.cfg`, or `[pytest]` in `tox.ini`. The file the fix writes sets `pythonpath = .` (so tests import the project's own packages), `testpaths = tests` when there is a `tests/` folder, and `addopts = -ra`.

Without a config, pytest's root and import path depend on where it is started.

## Manual fix

1. Run `fscale sync`, or create `pytest.ini` in the project's folder with `[pytest]` and `pythonpath = .`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `pytest.ini` only where no pytest config exists.

### REPORT

- Nothing.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules python/pytest-config
```
