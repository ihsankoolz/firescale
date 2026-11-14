---
name: entry
description: Fixes violations of the firescale rule expo/entry by creating the Expo app's entry files. Use when fscale check reports expo/entry.
---

# expo/entry

## What this rule enforces and why

The app's entry point exists, or Metro has nothing to bundle:

- `main` in package.json names a file that exists (Expo's blank template: `"main": "index.ts"`, which calls `registerRootComponent(App)`);
- `"main": "expo-router/entry"` needs a root layout, `app/_layout.tsx` or `src/app/_layout.tsx`;
- with no `main`, Expo loads `App` from the repo root.

## Manual fix

1. Create the file `main` names, or point `main` at the real entry.
2. For a new app, create `App.tsx` and `index.ts` as Expo's blank TypeScript template has them.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. For a new app it creates `App.tsx` (or `App.js`) and `index.ts`; for a missing `index.ts` with an `App`, it creates `index.ts`.
2. For Expo Router without a root layout, create `app/_layout.tsx` (or under `src/app/` if the routes are there) exporting a `Stack` from `expo-router`.

### REPORT

- `main` names a file that is gone and you cannot tell which file is the entry: report it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/entry
```
