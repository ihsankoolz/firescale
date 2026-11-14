---
name: config
description: Fixes violations of the firescale rule supabase/config by creating supabase/config.toml or adding its project_id. Use when fscale check reports supabase/config.
---

# supabase/config

## What this rule enforces and why

`supabase/config.toml` exists and sets `project_id`. The Supabase CLI looks for this file to know it is in a Supabase project, and stops with "Missing required field in config: project_id" without the key. The id also names the local containers, so two projects on one machine need different ids.

## Manual fix

1. Run `npx supabase init` in the repo root, or create `supabase/config.toml` with one line: `project_id = "<your-project>"`.
2. If the file exists, add `project_id = "<your-project>"` above its first `[table]`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates the file with `project_id` only (every other setting takes the CLI's default), or adds `project_id` before the first table. The id is package.json's name, cleaned as the CLI cleans names, or `supabase-app`.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/config
```
