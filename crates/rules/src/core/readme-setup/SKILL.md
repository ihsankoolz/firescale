---
name: readme-setup
description: Fixes violations of the firescale rule core/readme-setup by creating a README skeleton or adding install and run steps to the README. Use when fscale check reports core/readme-setup.
---

# core/readme-setup

## What this rule enforces and why

The root README (`README.md`, `README`, `README.rst` or `README.txt`, in any letter case) shows how to install and how to run the project. In a git repo the README counts only when it is tracked by git and present on disk. The check is a heuristic. Headings match whole words only, ignoring case, so "Reinstallation" or "Restart policy" do not count. "Install" is satisfied by a heading with one of the words Install, Installation, Installing, Setup, Set up, Getting started, Quick start, Development, Dev setup, Prerequisites, Build or Building, or by a known install command (`npm install`, `pnpm install`, `pip install`, `cargo build`, ...) in a code block or inline code. "Run" is satisfied by a heading with one of the words Usage, Run, Running, Start, Starting, Getting started, Quick start, Development, Example(s), Commands, Test or Testing, or by a known run command (`npm run`, `pnpm dev`, `cargo run`, `python`, `make`, ...).

New contributors and agents cannot work on a project if they cannot tell how to set it up and start it.

## Manual fix

1. Create or open the root README. `fscale fix` creates a skeleton `README.md` when the repo root has no README of any kind. If the README exists but is not tracked, run `git add` on it.
2. Add an "Install" section with the exact command that installs dependencies.
3. Add a "Usage" (or "Run") section with the exact command that starts the project, and the command that runs the tests.
4. Check that each command works from a fresh clone.

## Agent fix instructions

`fscale fix` only ever creates a file that is absent. It never overwrites or edits a README that exists on disk, tracked or not. It also creates nothing when git still tracks a README that is missing from disk, or when a `.gitignore` rule ignores `README.md`.

### DETERMINISTIC

Applies only when nothing at the repo root has a name that starts with "readme" in any letter case (so `README.adoc`, `readme.markdown` or a `README` folder all prevent it).

Create `README.md` and stage it with `git add`. The skeleton has the root project's name as its title (or "Project name" when none is detected), an "Install" section with the detected package manager's install command (`cargo build` for a Rust-only repo), and a "Usage" section with the detected build and test commands. Where a command is not known, the section holds a sentence to replace. Line endings are LF. Once created, the file belongs to the team and is not rewritten.

### REPORT

Take the commands from the project's manifests and scripts (`package.json`, `Cargo.toml`, `pyproject.toml`, `Makefile`), not from memory.

- After the skeleton is created: tell the user it is a skeleton, and that the project description and the Install and Usage sections still need to be completed.
- A README that exists but is not tracked by git: tell the user to `git add` it. Do not replace it.
- A README that git tracks but that is missing from disk: tell the user to restore it with `git restore <file>` or to commit its deletion. Do not write a new one.
- A `.gitignore` rule that ignores `README.md`: tell the user. Do not force-add the file.
- A README under another name (for example `README.adoc`): do not add a second README beside it. Report it, and ask whether to rename it or add the steps there.
- The README exists but lacks a section: add only the missing section, keep the existing text and its line endings, and report which sections you added. `fscale fix` never does this.
- Write real commands that you confirmed exist. Do not invent scripts. Report any command you could not confirm.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/readme-setup
```
