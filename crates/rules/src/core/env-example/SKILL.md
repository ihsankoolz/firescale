---
name: env-example
description: Fixes violations of the firescale rule core/env-example by listing every env var the code reads in .env.example. Use when fscale check reports core/env-example.
---

# core/env-example

## What this rule enforces and why

If tracked source code reads environment variables, the root `.env.example` lists each of them. The rule scans `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, `.py`, `.rs`, `.go` and `.rb` files (not `node_modules`, `dist`, `build`, `target`, `vendor` or `.git`) for reads such as `process.env.NAME`, `import.meta.env.NAME`, `os.environ["NAME"]`, `os.getenv("NAME")`, `env::var("NAME")`, `os.Getenv("NAME")` and `ENV["NAME"]`. Only upper-case names count. Common system names (`NODE_ENV`, `CI`, `PATH`, `HOME`, ...) and names starting with `npm_`, `GITHUB_`, `RUNNER_` or `FSCALE_` are ignored.

Options:

- `ignore`: a list of extra variable names to skip, for example `["EXTRA_VAR"]`.
- `excludePaths`: a list of repo-relative paths (forward slashes) that are not scanned, for example `["tests/fixtures"]`. Matching is by whole path segment: an entry, with or without a trailing `/`, excludes the path equal to it and everything under it. `["src/a"]` excludes `src/a/x.ts` but not `src/ab/y.ts`.

Limits: lines whose trimmed text starts with `#`, `//`, `*` or `/*` are skipped as comments. This filter is cheap, so reads inside multi-line strings or docstrings can still be picked up. Destructuring (`const { FOO } = process.env`) and computed names (`process.env[name]`) are not detected.

Without an example file, a new developer finds out which variables are needed only when the app crashes.

## Manual fix

1. Create `.env.example` in the repo root if it does not exist.
2. Add one line per missing variable, for example `EXAMPLE_API_URL=`. Leave the value empty or use an obvious placeholder. Never put a real value in this file.
3. Commit `.env.example`. Do not commit a real `.env` file.

## Agent fix instructions

Never read, print or copy the contents of a real `.env` file. Only `.env.example` may be read or edited.

### DETERMINISTIC

Run `fscale fix --rules core/env-example`, or do the same by hand:

1. Create or open `.env.example`.
2. Append one `NAME=` line for each variable named in the diagnostics that the file does not list yet, sorted by name, with no values. Put them under one comment line when the file already has content.
3. Keep existing lines and their line endings. Do not add a name twice.

### REPORT

- The names that were added.
- Any variable that probably needs a documented default or format; tell the user, and do not invent values.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/env-example
```
