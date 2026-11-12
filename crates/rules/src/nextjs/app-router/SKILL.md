---
name: app-router
description: Fixes violations of the firescale rule nextjs/app-router by creating the App Router's root layout and first page. Use when fscale check reports nextjs/app-router.
---

# nextjs/app-router

## What this rule enforces and why

An `app/` (or `src/app/`) folder has a root `layout` file. A project with neither `app/` nor `pages/` gets `app/layout.tsx` and `app/page.tsx`. A Pages Router project (`pages/`) passes.

The App Router needs a root layout with `<html>` and `<body>`; without it `next build` fails.

## Manual fix

1. Run `fscale sync`, or create `app/layout.tsx` that renders `<html><body>{children}</body></html>`.
2. Add `app/page.tsx` for the home page.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It only creates files that are missing.

### REPORT

- Nothing. Pages and routes beyond the first are the project's own work.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules nextjs/app-router
```
