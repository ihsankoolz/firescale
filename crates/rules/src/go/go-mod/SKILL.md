---
name: go-mod
description: Fixes violations of the firescale rule go/go-mod by creating go.mod or adding its go line. Use when fscale check reports go/go-mod.
---

# go/go-mod

## What this rule enforces and why

The repo root has a `go.mod` with a `module` line (the module's import path) and a `go` line (the Go version it needs). Outside a module the go tool builds nothing, and with no `go` line it assumes Go 1.16 and turns off newer language features.

A new module gets `module example.com/app` and `go 1.26.0`: Go supports its two newest releases (1.27 and 1.26 on 2026-11-14), and an older local Go downloads the 1.26.0 toolchain by itself. An existing `go.mod` with no `go` line gets `go 1.16`, which the go tool already assumes, so the module builds as it does today: a newer `go` line changes how its requirements are read and some language rules, which can stop it building. A `go.mod` that starts with a byte order mark is reported: the go tool rejects it.

## Manual fix

1. Run `fscale sync`, or `go mod init <path>` in the repo root.
2. Change `example.com/app` to where the code will live (`github.com/you/project`), in `go.mod` and in any import of the module's own packages.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `go.mod` (with `go 1.26.0`), or adds `go 1.16` after the module line of an existing one.
2. To move an existing module to newer Go, run `go mod tidy -go=1.26.0` on purpose, then `go build ./...` and `go test ./...`: that is a change to review, not a fix.

### REPORT

- No module line: add `module <path>` as the first line. Ask the user for the path if the repo's remote does not show it.
- A byte order mark: save `go.mod` as UTF-8 without one.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules go/go-mod
```
