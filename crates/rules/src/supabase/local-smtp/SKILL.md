---
name: local-smtp
description: Fixes violations of the firescale rule supabase/local-smtp by renaming the [inbucket] table of supabase/config.toml to [local_smtp]. Use when fscale check reports supabase/local-smtp.
---

# supabase/local-smtp

## What this rule enforces and why

`supabase/config.toml` has no `[inbucket]` table. Supabase CLI v3 refuses to load a config with one ("[inbucket] in supabase/config.toml is no longer supported. Use [local_smtp] instead."), so `supabase start`, `stop`, `status` and the `db` commands fail. Projects created with `supabase init` before CLI 2.108 have that table; the keys inside it are the same under the new name.

## Manual fix

1. In `supabase/config.toml`, rename `[inbucket]` to `[local_smtp]` (and any `[inbucket.<x>]` to `[local_smtp.<x>]`).
2. If package.json pins a CLI older than 2.108, update it first: `npm install --save-dev supabase@latest`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It renames the table headers and leaves every key and comment as it was.

### REPORT

- package.json pins a CLI older than 2.108: report that the CLI must be updated first; do not change the version yourself.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/local-smtp
```
