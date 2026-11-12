---
name: eslint
description: Fixes violations of the firescale rule nextjs/eslint by creating eslint.config.mjs with Next.js's rules. Use when fscale check reports nextjs/eslint.
---

# nextjs/eslint

## What this rule enforces and why

An ESLint config is at the root (`eslint.config.mjs` or another `eslint.config.*` or `.eslintrc*`). A new one uses `eslint-config-next`'s core-web-vitals and TypeScript rules.

Next.js 16 removed `next lint`; the `lint` script runs `eslint` with this config.

## Manual fix

1. Install the dev dependencies: `npm install -D eslint eslint-config-next`.
2. Run `fscale sync` to write `eslint.config.mjs`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. When `eslint` and `eslint-config-next` are dependencies, run `fscale sync`.

### REPORT

- When they are not: report `npm install -D eslint eslint-config-next`; do not edit `package.json` by hand.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules nextjs/eslint
```
