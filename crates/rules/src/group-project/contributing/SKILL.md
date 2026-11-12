---
name: contributing
description: Fixes violations of the firescale rule group-project/contributing by adding a CONTRIBUTING.md that says how to set up, check and hand in work in this repo. Use when fscale check reports group-project/contributing.
---

# group-project/contributing

## What this rule enforces and why

A contributing guide exists: a file named `CONTRIBUTING` (any one extension, any letter case) in the repo root, `docs/` or `.github/`. GitHub links it when someone opens a pull request or an issue. In a git repo the file must be tracked.

A team with mixed experience needs one place that says how to install the project, how to check work before asking for a review, how to name branches, and what a pull request needs. Without it every teammate learns this by asking.

## Manual fix

1. Create `CONTRIBUTING.md` at the repo root.
2. Write the install command of each part of the repo, the check to run before a pull request (`npx firescale verify`), the branch naming rule, and what a pull request needs before it is merged.
3. Run `git add` on it and commit it.

## Agent fix instructions

### DETERMINISTIC

- When the repo has no guide at all, run `fscale fix --rules group-project/contributing`. It writes `CONTRIBUTING.md` from what `fscale facts` finds (each project's install, lint, typecheck and test commands) and from the rule's `branches` option, and stages it. It never overwrites a file.

### REPORT

- A guide that exists but is not tracked: report its path and the `git add` command.
- What the team agreed that firescale cannot know (review rules, meeting times, who owns what): report it as something for the user to add. Do not invent it.

## Options

- `branches`: the branch name patterns to write into the guide. The default is the same list as `group-project/branch-name`'s. When you set one, set both to the same list.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules group-project/contributing
```
