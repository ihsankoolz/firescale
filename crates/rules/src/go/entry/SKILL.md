---
name: entry
description: Fixes violations of the firescale rule go/entry by writing a main package and its test for a module with no Go code. Use when fscale check reports go/entry.
---

# go/entry

## What this rule enforces and why

The module has Go code. With none, `go build ./...` and `go test ./...` match no packages and do nothing, so a check that passes proves nothing. A new module gets `main.go`, a program that prints a greeting, and `main_test.go`, a test of it, both as `gofmt` writes them.

Files in folders whose name starts with `.` or `_`, and in `testdata` or `vendor`, do not count: the go tool ignores them.

## Manual fix

1. Run `fscale sync`, or write your first package and a `_test.go` file for it.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It writes `main.go` and `main_test.go` when the module has no Go file, and never replaces one.

### REPORT

- Tell the user the starter is a placeholder program: replace `Greeting` and its test with the module's real code.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules go/entry
```
