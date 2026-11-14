---
name: public-env-secret
description: Handles violations of the firescale rule expo/public-env-secret, which finds EXPO_PUBLIC_ variables named like secrets. Use when fscale check reports expo/public-env-secret.
---

# expo/public-env-secret

## What this rule enforces and why

No `EXPO_PUBLIC_` variable is named like a secret: `SECRET`, `PRIVATE`, `PASSWORD`, `SERVICE_ROLE` or `SERVICE_KEY` in its name. Expo inlines every `EXPO_PUBLIC_` variable into the app bundle; its docs: "Do not store sensitive info, such as private keys, in `EXPO_PUBLIC_` variables. These variables will be visible in plain-text in your compiled application."

The rule reads tracked JavaScript and TypeScript files, `.env*` files, `app.json` and `eas.json` for such names. Public client keys (a maps token, Supabase's publishable key) are fine and not flagged.

## Manual fix

1. If the value was ever in a shipped build, rotate it now.
2. Move the code that needs it to a server (an Expo API route, an Edge Function) and read it there under a name without `EXPO_PUBLIC_`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

- Report the file and line, and that the value must be rotated if it was ever shipped. Moving code to a server changes the app's design: do it only if asked. Never print or copy the value.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/public-env-secret
```
