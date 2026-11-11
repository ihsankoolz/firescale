---
name: ci-exists
description: Fixes violations of the firescale rule core/ci-exists by adding a GitHub Actions workflow that runs on pull requests. Use when fscale check reports core/ci-exists.
---

# core/ci-exists

## What this rule enforces and why

A CI workflow exists and runs on pull requests. The rule looks for `.yml` or `.yaml` files in `.github/workflows/`, and at least one of them must mention a `pull_request` (or `pull_request_target`) trigger outside comments. If another CI system's config exists at the repo root (`.gitlab-ci.yml`, `.circleci/config.yml`, `azure-pipelines.yml`, `bitbucket-pipelines.yml`, `Jenkinsfile`), the rule reports nothing, because it only understands GitHub Actions triggers.

Without CI on pull requests, broken builds and failing tests reach the main branch unnoticed.

## Manual fix

1. Create `.github/workflows/ci.yml` (or add a trigger to an existing workflow).
2. Make it run on pull requests and on pushes to the main branch:

   ```yaml
   on:
     pull_request:
     push:
       branches: [main]
   ```

3. Add jobs that install dependencies, build and run the tests, using the commands this project already uses.
4. Commit the file.

## Agent fix instructions

Firescale cannot know this project's build and test commands, so there is no deterministic fix.

### DETERMINISTIC

None.

### REPORT

Work out the install, build and test commands from the project's manifests and README (for example `package.json` scripts, `Cargo.toml`, `pyproject.toml`, `Makefile`).

1. If there is no workflow, write `.github/workflows/ci.yml` that triggers on `pull_request` and on pushes to the default branch, and runs those commands. Pin action versions (for example `actions/checkout@v4`).
2. If workflows exist but none runs on pull requests, add `pull_request:` under `on:` in the most suitable one.
3. Do not add secrets, deploy steps or paths outside `.github/workflows/`.
4. Report which file you wrote, which commands it runs, and any command you could not determine.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/ci-exists
```
