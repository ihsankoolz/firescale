---
name: package-json
description: Fixes violations of the firescale rule expo/package-json by giving an Expo app its dependencies and scripts. Use when fscale check reports expo/package-json.
---

# expo/package-json

## What this rule enforces and why

`package.json` depends on `expo`, `react` and `react-native`, and has the `start`, `android` and `ios` scripts of Expo's templates (`web` too when it depends on `react-native-web`, and `lint`, `expo lint`, once `eslint-config-expo` is a dependency). React and React Native must be the versions the Expo SDK was built with, so they are installed with `npx expo install`, never by hand.

## Manual fix

1. Install missing packages with `npx expo install expo react react-native`: it picks the versions the SDK expects.
2. Add the missing scripts: `"start": "expo start"`, `"android": "expo start --android"`, `"ios": "expo start --ios"`, `"web": "expo start --web"`, `"lint": "expo lint"`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. With no package.json it creates one (SDK 58, TypeScript, ESLint); otherwise it adds the missing scripts as new lines.
2. For missing react or react-native, run `npx expo install react react-native`. Do not type versions by hand.

### REPORT

- package.json is not valid JSON, or the root is a workspace whose app is in a subfolder: report it; do not rewrite package.json.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules expo/package-json
```
