# 0012 — The core rule pack: what each rule treats as a violation

Status: accepted (M5)

## Context

SPEC §6 gives each core rule one line. Turning a line into a check means choosing heuristics. The guiding choice for all seven new rules: **a false positive is worse than a miss.** These rules run on repos whose owners did not write them, are report-only by default, and a rule that cries wolf gets switched off.

## Decision

All rules are Rust, in `crates/rules/src/core/<rule>/`, with a SKILL.md and fixture tests. Only the repo root is examined unless stated.

| Rule                   | Violation                                                                                                                                                                                                                                                                              | Fix                                                                  |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `gitignore-basics`     | In a git repo: no root `.gitignore`, or it lacks an entry for a detected language (`node_modules` for JS/TS, `target` for Rust, `__pycache__` and `.venv` for Python). `node_modules/`, `/node_modules` and `**/node_modules/` all count.                                              | Deterministic: append the missing entries.                           |
| `single-lockfile`      | A root `package.json` exists and: there is no JS lockfile; or there are several; or the one lockfile belongs to a different manager than `packageManager` declares.                                                                                                                    | None. Deleting a lockfile is not safe to automate.                   |
| `scripts-present`      | A root `package.json` lacks a `lint`, `test` or `build` script. npm's placeholder `test` script counts as missing. The `required` option replaces the list.                                                                                                                            | None. Marked agent-fixable: firescale cannot know the right command. |
| `ci-exists`            | No GitHub Actions workflow is tracked, or none mentions the `pull_request` trigger. If another CI system's config is present (GitLab, CircleCI, Azure, Bitbucket, Jenkins), the rule stays silent, because it only understands GitHub triggers.                                        | None. Agent-fixable.                                                 |
| `readme-setup`         | No root README, or it shows neither an install step nor a run step. A step is a heading such as "Install", "Setup", "Usage", "Getting started", or a known command inside a code block or backticks.                                                                                   | None. Agent-fixable.                                                 |
| `env-example`          | Tracked source reads an upper-case env var (`process.env.X`, `os.environ["X"]`, `env::var("X")` and similar in JS, TS, Python, Rust, Go, Ruby) that `.env.example` does not list. Common non-config names (`NODE_ENV`, `CI`, `PATH`, `HOME`, ...) and the `ignore` option are skipped. | Deterministic: append `NAME=` lines, never a value.                  |
| `no-secrets-in-source` | A tracked text file contains a token in a well-known format: AWS access key id, GitHub token, Slack token, Stripe live key, Google API key, npm token, or a private key block.                                                                                                         | None. The fix is rotating the secret, which only a person can do.    |

Further choices:

- **Stack-specific rules are JavaScript-only for now.** `single-lockfile` and `scripts-present` report nothing without a root `package.json`. SPEC says "per detected stack"; Rust and Python equivalents are not written.
- **`no-secrets-in-source` only matches fixed prefixes with the right length and character set.** No entropy scoring and no generic `password = "..."` patterns: both are noisy. Obvious placeholders are skipped (repeated characters, `EXAMPLE`, `xxxx`), so AWS's documented sample key is not flagged. A line containing `firescale:allow-secret` is skipped, and the `allowFiles` option skips whole files. Lockfiles, binaries, files over 1 MB and vendored folders are not scanned.
- **A finding never contains the secret.** The message names the kind and the line. Tests assert that no message contains the matched text, and that the rule's own source, tests and SKILL.md produce no match (test tokens are assembled at run time).
- **Severity and category** follow SPEC's intent: secrets is a blocker; a missing or wrong lockfile is high; the rest are medium or low.
- **Every rule stays `evaluated` by default** (SPEC §10), so adding seven rules cannot fail anyone's build.

## Consequences

- `no-secrets-in-source` declares `**/*` as its inputs, so its cache key hashes every tracked file. On a large repo a warm run costs about as much as the scan it saves. This sharpens decision 1 in DECISIONS.md about the cache.
- The heuristics will miss things: a secret in an unknown format, a README that explains setup in prose only, a CI system firescale has not heard of. That is the accepted price of few false positives.
- `env-example` and `readme-setup` are string scans, not parsers. Unusual syntax is not seen.
- Running `fscale check` on this repo reports nothing with all eight rules, which is M5's dogfooding criterion. A test keeps it that way.
