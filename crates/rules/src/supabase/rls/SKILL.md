---
name: rls
description: Fixes violations of the firescale rule supabase/rls by adding a migration that enables row level security, with policies, on tables the API exposes. Use when fscale check reports supabase/rls.
---

# supabase/rls

## What this rule enforces and why

Every table the migrations in `supabase/migrations` create in a schema the API exposes (`[api] schemas` in `supabase/config.toml`, by default `public` and `graphql_public`) has row level security enabled once the last migration has run. Supabase's docs: a table in an exposed schema without RLS is readable and writable by any role with a grant on it, through the API, with the publishable key every client holds.

The rule reads the migrations: `create table`, `alter table … enable/disable row level security`, `rename to`, `set schema` and `drop table`. Tables made another way (a function, `supabase/schemas/`, the dashboard) are not seen; `supabase db lint` checks a running database.

## Manual fix

1. Create a migration: `npx supabase migration new enable_rls_<table>`.
2. In it, enable RLS: `alter table <schema>.<table> enable row level security;`
3. Add a policy for each role that should reach the table, for example `create policy "Owners read their rows" on <schema>.<table> for select to authenticated using ((select auth.uid()) = user_id);`. With RLS on and no policy, the API returns no rows.
4. Never edit a migration that has already been applied.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Read the migration that creates the table, and the app code that queries it, to see who should read and write it.
2. Add one new migration file `supabase/migrations/<YYYYMMDDHHMMSS>_enable_rls_<table>.sql` with a UTC timestamp later than every existing migration.
3. In it, `alter table <schema>.<table> enable row level security;` and the policies that the app's queries need, as narrow as they allow.
4. Do not edit existing migrations.

### REPORT

- If you cannot tell who should reach the table, add only the `enable row level security` statement and report that it needs policies before the app can use the table.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/rls
```
