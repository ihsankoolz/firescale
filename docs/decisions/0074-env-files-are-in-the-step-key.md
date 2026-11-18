# 0074 — The env files git ignores are in the step key

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Amends ADR 0036 (what a step's key holds).

## Context

`docs/research/research-build-tools.md`, idea 1, first step. A step's key held the files git tracks or does not ignore. `.env` and `.env.local` are ignored in nearly every repo, and tests read them. So after an edit to `DATABASE_URL` or a feature flag in `.env`, verify gave a cached pass for a configuration that was never tested. `docs/verify.md` said so and told the reader to use `--no-cache`.

The base run already hashed these files into a kept base result's key (ADR 0043).

## Decision

**The bytes of `.env` and `.env.local`, at the repo root and in the unit's folder, are part of every step's key**, each as its repo-relative path and a hash of its bytes.

- A file that appears, changes or goes makes a miss. The old bytes again find the pass kept for them.
- A project's steps are keyed on the root's two files and its own folder's two. Another project's env file does not reach it. The repo root's own steps are keyed on the root's two.
- The files are read by one function, `stepcache::env_file`, which the base run's digest now uses too. It gives a hash and nothing else.
- **No value is printed, logged or stored.** The hash goes into the key's hash; the key is all that is kept. The cache log and the "Not cached" line name the file, never what it holds.
- **A link is not followed, and a step is not cached while one of the four paths is a link** or cannot be read: what the tests would read through it is not in the key. The summary says `Not cached: … .env is a link, which is not followed, so what it holds is not in the key`. Before this change such a step was cached with the link's target outside the key.
- A pass is not stored when one of the files changed while the step ran, as for the unit's other files (ADR 0039).
- A folder named `.env` (a virtual environment) is not an env file and is ignored here, as before.

This adds an input. It removes none, and no pass is easier to get.

## Consequences

- Every kept pass made before this change misses once: the key's shape changed.
- More misses for a repo whose `.env` is rewritten on each run (a token refreshed by a script). The "why a step ran again" note of ADR 0075 is where that will show.
- The shared cache of CI is keyed the same way. A runner with no env file and a laptop with one do not share a pass, which is right: they did not test the same thing.

## Not done

- Other ignored files a test reads (`.env.test`, `.env.development`, `config/local.yaml`, a generated file) are still outside the key. `verify.projects.<folder>.inputs` (ADR 0076) is the way to name them.
- An env file in a folder above the unit that is not the repo root (`backend/.env` for `backend/auth_service`) is not in the key.
- The repo root's own steps are not keyed on the projects' env files.

## Later

- Item 2 of the idea: the value of every variable that an `.env.example` in scope names.
- Item 3: the framework prefixes (`NEXT_PUBLIC_*`, `VITE_*` and others) for the frameworks `fscale facts` detects.
- A "not cached" or "ran again" line that names a volatile variable.

## Tests

- `packages/cli/src/step-cache-env.test.ts`, written first and seen failing in WSL before the change: 4 of 5 failed (`expected 3 to be 6`: the steps did not run again; `expected [] to deeply equal ['audit_service']`; the "Not cached" line for a link was missing). The fifth, that no value of an env file is in any output or stored file, passed before and after: it is the guard for the rule that values never appear.
- `the_env_files_line_holds_a_hash_per_file_and_never_a_value` in `crates/core/src/stepcache.rs`, with the change.
