---
name: tests
description: Handles violations of the firescale rule go/tests, which needs at least one _test.go file in a module with Go code. Use when fscale check reports go/tests.
---

# go/tests

## What this rule enforces and why

A module with Go code has at least one `_test.go` file. Without one, `go test ./...` reports `[no test files]` and passes, so `fscale verify` and CI prove nothing about the code.

## Manual fix

1. Add `<file>_test.go` beside the code it tests, in the same package, with a `func TestXxx(t *testing.T)`.
2. Run `go test ./...`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

1. Pick the package with the most important exported function.
2. Write one table-driven test for it in a `_test.go` file beside it, in the same package, using only the standard library's `testing` package.
3. Run `go test ./...` and make it pass without changing the code under test.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules go/tests
```
