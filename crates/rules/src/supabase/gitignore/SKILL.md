---
name: gitignore
description: Fixes violations of the firescale rule supabase/gitignore by ignoring the Supabase CLI's local state. Use when fscale check reports supabase/gitignore.
---

# supabase/gitignore

## What this rule enforces and why

git ignores `supabase/.branches` and `supabase/.temp`. The Supabase CLI writes its local state there; `supabase link` keeps the linked project's ref and connection details in `.temp`. `supabase init` writes a `supabase/.gitignore` for them, but only when it runs inside a git repo.

## Manual fix

1. Create `supabase/.gitignore` with `.branches` and `.temp` (the CLI's own file also lists `.env.keys`, `.env.local` and `.env.*.local`), or add `supabase/.branches` and `supabase/.temp` to the root `.gitignore`.
2. If they are tracked already, untrack them: `git rm -r --cached supabase/.branches supabase/.temp`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It creates `supabase/.gitignore` as `supabase init` writes it, or appends the missing lines.

### REPORT

- State that is already tracked: report the `git rm -r --cached` command.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/gitignore
```
