---
name: eslint
description: Fixes violations of the firescale rule expo/eslint by setting up ESLint with Expo's config. Use when fscale check reports expo/eslint.
---

# expo/eslint

## What this rule enforces and why

An Expo app has an ESLint config (`eslint.config.*`, `.eslintrc.*` or `eslintConfig` in package.json). Without one, `npx expo lint` installs `eslint` and `eslint-config-expo`, writes `eslint.config.js` and adds a script before it lints: it changes the repo and needs the network, which a CI run or an agent's check should not.

## Manual fix

1. `npx expo install eslint eslint-config-expo -- --save-dev` (the versions the SDK expects).
2. Create `eslint.config.js` as `npx expo lint` writes it (or run `npx expo lint` once and commit what it writes).

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. With eslint and eslint-config-expo installed, run `fscale sync`. It creates eslint.config.js exactly as `npx expo lint` does.
2. Without them, install them with `npx expo install eslint eslint-config-expo -- --save-dev`, then run `fscale sync`.

### REPORT

None.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/eslint
```
