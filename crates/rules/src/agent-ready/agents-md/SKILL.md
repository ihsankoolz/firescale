---
name: agents-md
description: Fixes violations of the firescale rule agent-ready/agents-md by writing an AGENTS.md at the repo root that describes the real project. Use when fscale check reports agent-ready/agents-md.
---

# agent-ready/agents-md

## What this rule enforces and why

A file named `AGENTS.md` exists at the repo root and is not blank. A file that holds only whitespace counts as blank. A folder named `AGENTS.md` is reported too. Only the root file counts: an `AGENTS.md` in a subfolder does not replace it.

`AGENTS.md` is where coding agents look first for how to work in a repo. Without it, an agent has to guess the commands and the house rules, and guesses differ from run to run.

## Manual fix

1. Create `AGENTS.md` at the repo root.
2. Write what is true for this project: what it is, how to install, build, test and lint it, and the rules a contributor must follow.
3. Keep it short. Link to longer documents instead of copying them.

## Agent fix instructions

### DETERMINISTIC

None. The content must describe this project, so there is nothing safe to generate.

### REPORT

Read the repo before writing: the README, the package or build manifests, the CI config and `fscale facts`. Then write `AGENTS.md` with:

- One or two sentences on what the project is.
- The exact commands to install, build, test, lint and typecheck, taken from the manifests. Do not invent a command.
- Conventions you can see in the repo, such as the commit style or folder layout.

Report what you wrote and list anything you could not find, so the user can fill it in. Do not copy a template from another project.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules agent-ready/agents-md
```
