---
name: branch-name
description: Reports when the current git branch's name does not match the team's branch patterns, and says how to rename it. Use when fscale check reports group-project/branch-name.
---

# group-project/branch-name

## What this rule enforces and why

The name of the checked-out git branch matches one of the patterns in the rule's `branches` option. In a pull request run on GitHub Actions, where the checkout has no branch, the pull request's branch from `GITHUB_HEAD_REF` is checked instead. Outside a git repo, or with a detached HEAD and no pull request, there is nothing to check.

The repo's own main branch is always allowed: the branch `git init` names new repos after (`init.defaultBranch`), and the remote's default branch (`origin/HEAD`). So a repo that starts on `trunk` passes.

The default patterns are `main`, `master`, `develop`, and `feat/*`, `fix/*`, `docs/*`, `test/*`, `refactor/*`, `chore/*`, `ci/*`, `build/*`, `perf/*`, `style/*`, `release/*`, `hotfix/*`, `revert/*`. `*` matches any text.

Branch names that say what kind of change they hold make a team's list of branches and pull requests readable at a glance.

## Manual fix

1. Pick a name that matches a pattern, for example `feat/login-page`.
2. Rename the branch: `git branch -m feat/login-page`.
3. If the old name was pushed already, push the new one and delete the old one on the remote: `git push -u origin feat/login-page`, then `git push origin --delete <old-name>`.

## Agent fix instructions

### DETERMINISTIC

None. Renaming a branch changes what the user and their teammates see, so it is the user's choice.

### REPORT

- Report the current name, the patterns, and a suggested new name with the `git branch -m` command. Do not rename the branch yourself.
- In a pull request run, the branch is the pull request's. Renaming it in CI does nothing: the work must be pushed to a branch with a matching name, or a pattern added to `branches`.

## Options

- `branches`: a list of branch name patterns, for example `["main", "feat/*", "fix/*"]`. Set `group-project/contributing`'s `branches` to the same list, so the guide says the same.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules group-project/branch-name
```
