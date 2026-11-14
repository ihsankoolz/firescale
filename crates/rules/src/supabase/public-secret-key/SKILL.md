---
name: public-secret-key
description: Handles violations of the firescale rule supabase/public-secret-key, which finds a Supabase secret or service-role key under a name that ships it to the client. Use when fscale check reports supabase/public-secret-key.
---

# supabase/public-secret-key

## What this rule enforces and why

No Supabase secret key (`sb_secret_…`) or legacy `service_role` key is read through a variable whose prefix makes the framework inline it into the client bundle: `NEXT_PUBLIC_`, `VITE_`, `EXPO_PUBLIC_`, `PUBLIC_`, `NUXT_PUBLIC_`, `REACT_APP_`, `GATSBY_`. Such a key bypasses every row level security policy; Supabase's docs say to never put one in a browser or a shipped app. Clients use the publishable key (`…_SUPABASE_PUBLISHABLE_KEY`).

The rule reads tracked source files (JavaScript, TypeScript, Vue, Svelte, Astro), `.env*` files and `app.json`/`eas.json` for such names.

## Manual fix

1. If the key was ever deployed under that name, rotate it now in the Supabase dashboard (API keys).
2. Move every use to server-only code (a route handler, a server action, an Edge Function) and read it as `SUPABASE_SECRET_KEY`, with no public prefix.
3. In client code, use the publishable key instead.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

None.

### REPORT

- Report the file and line, and that the key must be rotated if it was ever shipped. Moving code from client to server changes the app's design: do it only if asked, and never print or copy the key's value.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules supabase/public-secret-key
```
