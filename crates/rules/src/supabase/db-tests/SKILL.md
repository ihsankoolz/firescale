---
name: db-tests
description: Fixes violations of the firescale rule supabase/db-tests by adding a pgTAP test for supabase test db. Use when fscale check reports supabase/db-tests.
---

# supabase/db-tests

## What this rule enforces and why

`supabase/tests` holds at least one database test (`.sql` or `.pg`, at any depth). `supabase test db` runs these pgTAP files against the local database the migrations build. Without one, the database has no test at all, and nothing in CI notices a migration that drops a policy or turns row level security off.

## Manual fix

1. `npx supabase test new <name>` creates `supabase/tests/<name>_test.sql` from pgTAP's template; Supabase's guide keeps them in `supabase/tests/database/`.
2. Write at least one assertion that matters to the app, then run `npx supabase test db` (it needs `supabase start`, so Docker).

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It adds `supabase/tests/database/rls_test.sql`, a pgTAP test that every table in `public` has row level security on.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/db-tests
```
