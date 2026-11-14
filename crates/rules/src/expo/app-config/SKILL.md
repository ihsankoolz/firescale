---
name: app-config
description: Fixes violations of the firescale rule expo/app-config by creating app.json or naming the app in it. Use when fscale check reports expo/app-config.
---

# expo/app-config

## What this rule enforces and why

The app has an Expo app config: `app.json`, `app.config.json`, or a dynamic `app.config.ts`/`.js`. A static config sets `expo.name` and `expo.slug`, the minimum Expo's docs show, and does not set `expo.sdkVersion`: the SDK comes from the installed `expo` package, and `expo-doctor` warns about a stale value.

## Manual fix

1. With no config, create `app.json`: `{ "expo": { "name": "my-app", "slug": "my-app" } }`.
2. Add a missing `name` or `slug` inside `expo`.
3. Remove `expo.sdkVersion`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `app.json` (the template's settings, without image files), or adds the missing name and slug from package.json's name.
2. For `sdkVersion`, delete that one key from the config.

### REPORT

- The config is not valid JSON: report it; do not rewrite it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/app-config
```
