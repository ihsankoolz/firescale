---
name: next-config
description: Fixes violations of the firescale rule nextjs/next-config by creating next.config.ts. Use when fscale check reports nextjs/next-config.
---

# nextjs/next-config

## What this rule enforces and why

A Next.js config file is at the root: `next.config.ts`, `next.config.mjs`, `next.config.js` or `next.config.cjs`.

Settings then have one known place, typed with `NextConfig`.

## Manual fix

1. Run `fscale sync`, or create `next.config.ts` that exports a `NextConfig` object.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`.

### REPORT

- Which options the project needs: leave the new file empty unless the task asks for one.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules nextjs/next-config
```
