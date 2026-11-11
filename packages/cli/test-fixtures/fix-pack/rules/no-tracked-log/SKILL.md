---
name: no-tracked-log
description: Fixes violations of the sample rule fixable/no-tracked-log by removing tracked log files from the git index and ignoring them. Use when fscale check reports fixable/no-tracked-log.
---

# fixable/no-tracked-log

## What this rule enforces and why

No `.log` file is tracked by git. Logs are build or run output: they change on every run and bloat the history.

## Manual fix

1. Stop tracking each reported file, keeping your local copy: `git rm --cached <file>`.
2. Add `*.log` to the root `.gitignore`.

## Agent fix instructions

Fix only the files named in this rule's diagnostics.

### DETERMINISTIC

1. For each reported file, run `git rm --cached --quiet -- <file>`. Do not delete the file from disk.
2. Add the line `*.log` to the root `.gitignore` if it is missing.

### REPORT

None.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules fixable/no-tracked-log
```
