---
name: ci-verify
description: Fixes violations of the firescale rule group-project/ci-verify by making a GitHub Actions workflow run fscale verify on pull requests. Use when fscale check reports group-project/ci-verify.
---

# group-project/ci-verify

## What this rule enforces and why

A GitHub Actions workflow that runs on pull requests has a `run:` step that runs firescale's verify, such as `npx firescale verify`, `pnpm fscale verify`, `npx --yes firescale@0.1.2 verify` or `npm ci && npx firescale verify`. The workflow is a tracked `.yml` or `.yaml` file in `.github/workflows/`, and its `on:` key lists `pull_request` or `pull_request_target`. Only `run:` steps count (one line, or a `|` or `>` block), and the command itself must be firescale's verify: a step's name, a comment, an `echo` or a later command that merely mentions the words does not count.

A package script that runs verify (`npm run verify` with `"verify": "fscale verify"`) is not recognised. Call `npx firescale verify` in the workflow directly, or turn this rule off.

Then every pull request is checked by the same command a teammate runs on their machine, and "it passes for me" means the same thing for everyone.

## Manual fix

1. Open the workflow that runs on pull requests, or create `.github/workflows/ci.yml`.
2. After the step that installs dependencies, add a step:

   ```yaml
   - run: npx firescale verify
   ```

3. Commit the change.

## Agent fix instructions

### DETERMINISTIC

- When the repo has no CI config at all, run `fscale fix --rules group-project/ci-verify`. It creates the same starter workflow as `core/ci-exists`, which runs `npx firescale verify` on pull requests. It never changes a workflow that exists.

### REPORT

- A hand-written workflow without verify: add one step `- run: npx firescale verify` to the job that runs on pull requests, after its install step. Change nothing else in the file. Report what you added.
- CI that is not GitHub Actions (GitLab, CircleCI and others): report it. The rule cannot read those files; the user can add the command and turn the rule off.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules group-project/ci-verify
```
