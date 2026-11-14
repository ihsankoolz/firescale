---
name: package-json
description: Fixes violations of the firescale rule supabase/package-json by adding the Supabase CLI as a dev dependency. Use when fscale check reports supabase/package-json.
---

# supabase/package-json

## What this rule enforces and why

`package.json` lists the Supabase CLI (`supabase`) as a dev dependency. Supabase's docs install the CLI this way (`npm install supabase --save-dev`) and run it with `npx supabase`, so every person and every CI run uses the version the repo pins, not whatever is installed globally.

## Manual fix

1. Install the CLI with your package manager: `npm install --save-dev supabase` (or `pnpm add -D supabase`, `yarn add -D supabase`, `bun add -D supabase`).
2. Commit `package.json` and the lockfile.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. With no package.json it creates one; with a package.json and no lockfile it adds `supabase` to `devDependencies`.

### AGENT

1. The repo has a lockfile: run the package manager the lockfile belongs to, to add `supabase` as a dev dependency. Do not edit the lockfile by hand.

### REPORT

- package.json is not valid JSON: report it; do not rewrite it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/package-json
```
