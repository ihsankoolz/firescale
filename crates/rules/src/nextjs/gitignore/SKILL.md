---
name: gitignore
description: Fixes violations of the firescale rule nextjs/gitignore by ignoring Next.js's build output. Use when fscale check reports nextjs/gitignore.
---

# nextjs/gitignore

## What this rule enforces and why

`.gitignore` ignores `.next/` and `next-env.d.ts`, which `next build` and `next dev` write.

Committed build output makes every diff noisy and can leak build-time values.

## Manual fix

1. Run `fscale sync`, or add `.next/` and `next-env.d.ts` to `.gitignore`.
2. If they are tracked already, untrack them: `git rm -r --cached .next next-env.d.ts`.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

1. Run `fscale sync`. It appends the missing lines.

### REPORT

- Build output that is already tracked: report the `git rm --cached` command.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules nextjs/gitignore
```
