---
name: gitignore
description: Fixes violations of the firescale rule expo/gitignore by ignoring Expo's local state and build output, local env files and app-signing keys. Use when fscale check reports expo/gitignore.
---

# expo/gitignore

## What this rule enforces and why

`.gitignore` ignores `.expo/` (Expo's local state, "not meant to be shared"), `dist/` (what `expo export` writes), `.env*.local` (local env files) and app-signing keys: `*.jks`, `*.p8`, `*.p12`, `*.key`, `*.mobileprovision`. Expo's templates write these lines, and `expo-doctor` warns when the first two are missing. A committed signing key lets anyone sign builds as you.

## Manual fix

1. Run `fscale sync`, or add the missing lines to `.gitignore`.
2. If a key or `.expo/` is tracked already, untrack it (`git rm --cached <file>`) and rotate the key.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It appends the missing lines in the file's own line endings.

### REPORT

- A signing key that is already tracked: report it, and that it must be rotated. Do not print it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/gitignore
```
