---
name: tsconfig
description: Fixes violations of the firescale rule nextjs/tsconfig by creating the tsconfig.json Next.js expects. Use when fscale check reports nextjs/tsconfig.
---

# nextjs/tsconfig

## What this rule enforces and why

A Next.js project that depends on TypeScript (or a new one) has a `tsconfig.json`, with the `next` plugin, `moduleResolution: bundler` and the `@/*` path, as create-next-app writes it.

`next build` type-checks with it; without it, editors and the build disagree.

## Manual fix

1. Run `fscale sync`, or copy the `tsconfig.json` create-next-app writes.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. An existing `tsconfig.json` is never changed.

### REPORT

- Changes to an existing `tsconfig.json`: report them, do not make them.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules nextjs/tsconfig
```
