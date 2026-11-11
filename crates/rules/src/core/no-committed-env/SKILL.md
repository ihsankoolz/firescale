---
name: no-committed-env
description: Fixes violations of the firescale rule core/no-committed-env by removing tracked .env files from the git index and ignoring them. Use when fscale check reports core/no-committed-env.
---

# core/no-committed-env

## What this rule enforces and why

No `.env` file is tracked by git. That means `.env` itself and any `.env.<name>` file such as `.env.local` or `.env.production`, at any depth. Names match in any letter case, so `.ENV` and `.Env.Local` count too: Windows and macOS ignore filename case, so such a file is the same secret file there. Example files are allowed: `.env.example` by default, or the names listed in the rule's `allow` option.

Env files usually hold API keys, database passwords and tokens. Once one is committed, everyone with access to the repo can read those secrets, and they stay in git history even after the file is deleted.

## Manual fix

1. Stop tracking each reported file, keeping your local copy: `git rm --cached <file>`.
2. Add these lines to the root `.gitignore`, in this order (the last matching line wins):

   ```
   .env
   .env.*
   !.env.example
   ```

   `.gitignore` is case-sensitive on Linux, so for a file whose name is not in lower case (for example `.ENV`), also add an exact ignore line with that name.

3. If there is no `.env.example`, create one that lists the variable names with empty or fake values, and commit it.
4. Commit the change.
5. Treat every secret that was in a committed env file as leaked. Rotate it at the provider. Removing the file from the index does not remove it from history.

## Agent fix instructions

Fix only the files named in this rule's diagnostics. Never print, copy or summarise the contents of an env file.

### DETERMINISTIC

Apply these steps exactly:

1. For each reported file, run `git rm --cached --quiet -- <file>`. Do not delete the file from disk.
2. Make sure the root `.gitignore` contains the lines `.env`, `.env.*` and `!.env.example`. Add only the lines that are missing. `!.env.example` must come after `.env` and `.env.*`, because the last matching line wins; if it sits before them, add it again at the end. Create the file if it does not exist.
3. For a reported file whose name is not in lower case (for example `.ENV` or `.Env.Local`), also add an exact ignore line with its path, because `.gitignore` is case-sensitive on Linux. Add it only if that line is missing.
4. Do not edit, move or rename the env files themselves.

### REPORT

- Every file that was untracked. Tell the user that any secret in it must be rotated, because it remains in git history.
- That rewriting git history to remove the files is a separate decision for the user. Do not rewrite history.
- A missing `.env.example`, if there is none. Do not create it from real values.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/no-committed-env
```
