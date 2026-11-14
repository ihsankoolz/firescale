---
name: go-mod
description: Fixes violations of the firescale rule go/go-mod by creating go.mod or adding its go line. Use when fscale check reports go/go-mod.
---

# go/go-mod

## What this rule enforces and why

The repo root has a `go.mod` with a `module` line (the module's import path) and a `go` line (the Go version it needs). Outside a module the go tool builds nothing, and with no `go` line it assumes Go 1.16 and turns off newer language features.

A new module gets `module example.com/app` and `go 1.26.0`: Go supports its two newest releases (1.27 and 1.26 on 2026-11-14), and an older local Go downloads the 1.26.0 toolchain by itself.

## Manual fix

1. Run `fscale sync`, or `go mod init <path>` in the repo root.
2. Change `example.com/app` to where the code will live (`github.com/you/project`), in `go.mod` and in any import of the module's own packages.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `go.mod`, or adds `go 1.26.0` after the module line.

### MANUAL

- No module line: add `module <path>` as the first line. Ask the user for the path if the repo's remote does not show it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules go/go-mod
```
