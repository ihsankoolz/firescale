---
name: requirements
description: Fixes violations of the firescale rule python/requirements by giving a Python project its dependency files. Use when fscale check reports python/requirements.
---

# python/requirements

## What this rule enforces and why

Each Python project (each service, in a repo with several) declares its dependencies: `requirements.txt`, `pyproject.toml`, a `Pipfile`, `setup.py` or `setup.cfg`. A repo with no Python project yet gets `requirements.txt` for the app and `requirements-dev.txt` with `-r requirements.txt`, pytest and ruff, pinned.

Without a declared list, every machine installs something different, and `fscale verify` cannot tell setup problems from code problems.

## Manual fix

1. For a new project, run `fscale sync`.
2. For an existing project, write `requirements.txt` listing what its code imports, pinned with `==`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. For a new project, run `fscale sync`.

### REPORT

- An existing project with no dependency file: report it. Choosing packages and versions needs a person or a careful reading of the imports.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules python/requirements
```
