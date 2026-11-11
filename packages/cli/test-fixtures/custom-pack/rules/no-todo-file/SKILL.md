---
name: no-todo-file
description: Fixes violations of the sample rule sample/no-todo-file by moving TODO items to the issue tracker. Use when fscale check reports sample/no-todo-file.
---

# sample/no-todo-file

## What this rule enforces and why

No `TODO.md` file is tracked by git. Work items kept in a file go stale and nobody is assigned to them.

## Manual fix

1. Move each item in the file to the issue tracker.
2. Delete the file and commit.

## Agent fix instructions

Fix only the files named in this rule's diagnostics.

### DETERMINISTIC

None.

### REPORT

- Every reported file, with the number of items in it. Do not delete the file.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules sample/no-todo-file
```
