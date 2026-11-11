---
name: no-secrets-in-source
description: Handles violations of the firescale rule core/no-secrets-in-source, which finds obvious keys and tokens in tracked files. Use when fscale check reports core/no-secrets-in-source.
---

# core/no-secrets-in-source

## What this rule enforces and why

No tracked file contains an obvious key or token. The rule looks for a few well-known formats: AWS access key IDs, GitHub tokens, Slack tokens, Stripe live keys, Google API keys, npm tokens and private key blocks. It reports the file, the line and the kind of secret, never the value.

Anyone who can read the repo can read a committed secret, and it stays in git history after the line is deleted.

A private key is reported only when there is evidence of key material: the header line (`BEGIN ... PRIVATE KEY`) must start the line (after optional whitespace), and either one of the next 3 lines is a base64 body line (at least 40 characters of letters, digits, `+`, `/`, `=`) or a matching `END ... PRIVATE KEY` line follows within 200 lines. A header inside a quoted string on a code line is not reported. Fixed-length formats (AWS access key IDs, Google API keys, npm tokens) must not be followed by a letter, digit or underscore.

What is skipped, exactly:

- Lockfiles: `pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `Cargo.lock`, `bun.lock`, `poetry.lock`, `uv.lock`, `go.sum`, `composer.lock`, `Gemfile.lock`.
- Binary files (containing a NUL byte), files that are not UTF-8, and files over 1 MB.
- Any file under a folder named `node_modules`, `dist`, `build`, `target` or `vendor`, at any depth.
- File suffixes `.min.js`, `.map`, `.svg`, `.png`, `.jpg`, `.jpeg`, `.gif`, `.ico`, `.woff`, `.woff2`, `.pdf` and `.snap`.
- Placeholder values: a value made of one repeated character, or one containing `EXAMPLE`, `example`, `xxxx`, `XXXX`, `0000000000` or `placeholder`.
- Any line containing the marker `firescale:allow-secret`.
- Files listed in the `allowFiles` option: a list of repo-relative paths with forward slashes, for example `["docs/sample-config.txt"]`.

## Manual fix

1. Rotate the secret first: revoke it at the provider and create a new one. Assume it is already leaked.
2. Remove it from the file. Read the new value from an environment variable or a secrets manager, and do not commit it.
3. Commit the change.
4. Decide whether to rewrite git history. After rotation the old value is useless, so rewriting is often not needed.

## Agent fix instructions

Never print, copy or summarise the value of a secret. Refer to it by file, line and kind only.

### DETERMINISTIC

None.

### REPORT

- Every finding, with its file, line and kind of secret. Never the value.
- That each secret must be rotated at the provider first, and that removing it from the file does not remove it from git history.
- That rewriting git history is the user's decision. Do not rewrite history.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/no-secrets-in-source
```
