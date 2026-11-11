---
name: ci-exists
description: Fixes violations of the firescale rule core/ci-exists by adding a GitHub Actions workflow that runs on pull requests, or a pull request trigger to an existing one. Use when fscale check reports core/ci-exists.
---

# core/ci-exists

## What this rule enforces and why

A CI workflow exists and runs on pull requests. The rule looks for `.yml` or `.yaml` files in `.github/workflows/`, and at least one of them must list `pull_request` or `pull_request_target` as a trigger under its top-level `on:` key. The rule reads `on:` as a scalar (`on: pull_request`), a flow list (`on: [push, pull_request]`), a block list, or a mapping with trigger keys. Mentions elsewhere (a `run:` line, a job name, a comment) do not count. Workflows triggered only by `workflow_call`, `workflow_dispatch` or `schedule` do not count either, but they never cause a finding on their own when another workflow runs on pull requests. If another CI system's config exists at the repo root (`.gitlab-ci.yml`, `.circleci/config.yml`, `azure-pipelines.yml`, `bitbucket-pipelines.yml`, `Jenkinsfile`), the rule reports nothing, because it only understands GitHub Actions triggers. In a git repo a file counts only when it is tracked by git and present on disk.

Without CI on pull requests, broken builds and failing tests reach the main branch unnoticed.

## Manual fix

1. Create `.github/workflows/ci.yml` (or add a trigger to an existing workflow). `fscale fix` creates a starter file when the repo has no CI at all.
2. Make it run on pull requests and on pushes to the main branch:

   ```yaml
   on:
     pull_request:
     push:
       branches: [main]
   ```

3. Add jobs that install dependencies, build and run the tests, using the commands this project already uses.
4. Run `git add` on the file and commit it. An untracked workflow does not count.

## Agent fix instructions

`fscale fix` only ever creates a file that is absent. It never overwrites, edits or renames a file that exists on disk, tracked or not.

### DETERMINISTIC

Applies only when the repo has no CI at all: `.github/workflows/` holds no `.yml` or `.yaml` file on disk, `.github/workflows/ci.yml` does not exist, and no config of another CI system exists.

Create `.github/workflows/ci.yml` and stage it with `git add`. The starter workflow runs on pull requests and on pushes to `main` and `master`, with read-only permissions. Its one job checks out the code, sets up Node, installs dependencies with the detected package manager (`npm ci` or `npm install` for npm; pnpm installed and then `pnpm install --frozen-lockfile`; a comment to fill in for any other package manager; no install step when there is none), then runs `npx firescale verify`. Line endings are LF. Once created, the file belongs to the team and is not rewritten.

### REPORT

- After the starter is created: tell the user to review `.github/workflows/ci.yml` and add what the project needs before the verify step (services, secrets, other toolchains). If the install step is a comment, say so.
- A workflow or another CI config that exists but is not tracked by git: tell the user to `git add` it. Do not replace it.
- Workflows exist but none runs on pull requests: add `pull_request:` under `on:` in the most suitable one, change nothing else in it, and report which file you changed. `fscale fix` never does this.
- Do not add secrets, deploy steps or paths outside `.github/workflows/`.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/ci-exists
```
