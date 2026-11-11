---
name: readme-setup
description: Fixes violations of the firescale rule core/readme-setup by adding install and run steps to the README. Use when fscale check reports core/readme-setup.
---

# core/readme-setup

## What this rule enforces and why

The root README (`README.md`, `README`, `README.rst` or `README.txt`, in any letter case) shows how to install and how to run the project. The check is a heuristic. "Install" is satisfied by a heading such as Install, Setup, Getting started, Quick start or Development, or by a known install command (`npm install`, `pnpm install`, `pip install`, `cargo build`, ...) in a code block or inline code. "Run" is satisfied by a heading such as Usage, Run, Start, Getting started or Development, or by a known run command (`npm run`, `pnpm dev`, `cargo run`, `python`, `make`, ...).

New contributors and agents cannot work on a project if they cannot tell how to set it up and start it.

## Manual fix

1. Create or open the root README.
2. Add an "Install" section with the exact command that installs dependencies.
3. Add a "Usage" (or "Run") section with the exact command that starts the project, and the command that runs the tests.
4. Check that each command works from a fresh clone.

## Agent fix instructions

Firescale cannot know this project's commands, so there is no deterministic fix.

### DETERMINISTIC

None.

### REPORT

Take the commands from the project's manifests and scripts (`package.json`, `Cargo.toml`, `pyproject.toml`, `Makefile`), not from memory.

1. If there is no README, create `README.md` with the project name, a one-line description, an "Install" section and a "Usage" section.
2. If the README exists, add only the missing section. Keep the existing text and its line endings.
3. Write real commands that you confirmed exist. Do not invent scripts.
4. Report which sections you added and any command you could not confirm.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/readme-setup
```
