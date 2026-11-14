---
name: go-sum
description: Explains how to fix violations of the firescale rule go/go-sum, which needs a go.sum when go.mod requires other modules. Use when fscale check reports go/go-sum.
---

# go/go-sum

## What this rule enforces and why

When `go.mod` requires other modules, `go.sum` is committed beside it (a requirement a `replace` points to a folder needs none). It holds the checksums of every module the build uses: without it the go tool stops with "missing go.sum entry", and nothing checks that what it downloads on another machine is what was tested.

## Manual fix

1. Run `go mod tidy` in the repo root. It needs the network (or a module proxy).
2. Commit `go.sum`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

1. Run `go mod tidy`. If it cannot reach the network, report that and stop: never write `go.sum` by hand.
2. Check that `go build ./...` and `go test ./...` pass.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules go/go-sum
```
