---
name: pr-template
description: Fixes violations of the firescale rule group-project/pr-template by adding a pull request template that asks what a change does and how to test it. Use when fscale check reports group-project/pr-template.
---

# group-project/pr-template

## What this rule enforces and why

A pull request template exists, so every pull request starts with the same questions: what it changes, how to test it, and a short checklist. GitHub fills a new pull request's description from a file named `pull_request_template` (with an extension such as `.md` or `.txt`, in any letter case) in the repo root, `docs/` or `.github/`. A `PULL_REQUEST_TEMPLATE` folder with templates in it counts too. In a git repo the file must be tracked, because teammates only get it through a commit, and GitHub uses it only once it is on the default branch.

In a group project, reviewers otherwise get pull requests with no description, and nobody can tell how to try the change.

## Manual fix

1. Create `.github/pull_request_template.md`.
2. Write the headings your team wants every pull request to answer, for example "What this changes" and "How to test it", and a short checklist.
3. Run `git add` on it and commit it.

## Agent fix instructions

### DETERMINISTIC

- When the repo has no template at all, run `fscale fix --rules group-project/pr-template`. It creates `.github/pull_request_template.md` and stages it. It never overwrites a file.

### REPORT

- A template that exists but is not tracked: report its path and the `git add` command. Do not commit for the user.
- A template that is ignored by `.gitignore`, or a path that is blocked by a file or link: report it. Do not change `.gitignore`.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules group-project/pr-template
```
