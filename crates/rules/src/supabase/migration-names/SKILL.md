---
name: migration-names
description: Handles violations of the firescale rule supabase/migration-names, which finds migrations the Supabase CLI would skip or apply twice under one version. Use when fscale check reports supabase/migration-names.
---

# supabase/migration-names

## What this rule enforces and why

Every SQL file in `supabase/migrations` is named `<timestamp>_<name>.sql` (digits, an underscore, a name), and no two files share the timestamp. The Supabase CLI applies only files that match this pattern and skips any other with a warning, so a misnamed migration silently never runs. The timestamp is the migration's version in the database's migration history, so two files with one version clash.

## Manual fix

1. Rename a misnamed file to `<YYYYMMDDHHMMSS>_<name>.sql` (UTC), choosing a timestamp that puts it where it must run among the others.
2. For two files with one version, give the later one a later timestamp.
3. If the migration was already applied to a hosted project under another name, fix the history with `supabase migration repair` instead of renaming blindly.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### AGENT

1. Read the migrations around the file to see where it belongs in the order.
2. Rename it (`git mv`) to a 14-digit UTC timestamp between its neighbours, keeping its name part. Do not change the file's contents.

### REPORT

- If the migration may already be applied to a hosted database, report that `supabase migration repair` is needed; do not rename it.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/migration-names
```
