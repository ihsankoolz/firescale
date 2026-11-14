---
name: tsconfig
description: Fixes violations of the firescale rule expo/tsconfig by creating the Expo template's tsconfig.json. Use when fscale check reports expo/tsconfig.
---

# expo/tsconfig

## What this rule enforces and why

A TypeScript Expo app (one that depends on `typescript`) has `tsconfig.json`. Expo's templates extend `expo/tsconfig.base`, which sets the module resolution, JSX and `noEmit` that Metro and `tsc --noEmit` need; the app adds `strict`.

## Manual fix

1. Create `tsconfig.json`: `{ "extends": "expo/tsconfig.base", "compilerOptions": { "strict": true } }`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates the template's tsconfig.json. It never edits one that exists.

### REPORT

None.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/tsconfig
```
