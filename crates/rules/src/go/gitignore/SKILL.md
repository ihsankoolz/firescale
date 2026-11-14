---
name: gitignore
description: Fixes violations of the firescale rule go/gitignore by ignoring what the go tool writes. Use when fscale check reports go/gitignore.
---

# go/gitignore

## What this rule enforces and why

`.gitignore` ignores what the go tool writes, which does not belong in git:

- the program `go build ./...` writes at the root when the root package is `main`, named after the last element of the module path (`/app` for `example.com/app`); not when a folder of that name is at the root (the line would hide it), nor for a `//go:build ignore` generator;
- `*.exe` (programs built for Windows), `*.test` (test binaries from `go test -c`), `*.out` (coverage profiles such as `cover.out`);
- `go.work` and `go.work.sum`: a local workspace the Go module reference advises not to commit. A `go.work` that is tracked, or that a line such as `!go.work` un-ignores, is the team's choice and is left as it is.

## Manual fix

1. Run `fscale sync`, or add the missing lines to `.gitignore`.
2. If a built program is tracked already, untrack it: `git rm --cached <file>`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It appends the missing lines in the file's own line endings.

### REPORT

- A tracked `go.work` the team relies on: report it instead of untracking it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules go/gitignore
```
