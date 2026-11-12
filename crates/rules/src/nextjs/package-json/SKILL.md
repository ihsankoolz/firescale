---
name: package-json
description: Fixes violations of the firescale rule nextjs/package-json by giving package.json the Next.js dependencies and scripts. Use when fscale check reports nextjs/package-json.
---

# nextjs/package-json

## What this rule enforces and why

`package.json` depends on `next`, `react` and `react-dom`, and has the scripts `dev` (`next dev`), `build` (`next build`), `start` (`next start`) and, once ESLint for Next.js is a dependency, `lint` (`eslint`).

Without them the project does not build, and people and agents guess the commands.

## Manual fix

1. With no `package.json`, run `fscale sync`: it writes one with the versions create-next-app uses, then run `npm install` (or your package manager's install).
2. For a missing dependency, install it with your package manager, for example `npm install next react react-dom`, so the lockfile matches.
3. For a missing script, run `fscale sync`: it adds it and leaves the rest of the file as it is.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync` to create `package.json` or add missing scripts.

### REPORT

- A missing `next`, `react` or `react-dom` dependency: report the install command; do not pick versions by editing `package.json`.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules nextjs/package-json
```
