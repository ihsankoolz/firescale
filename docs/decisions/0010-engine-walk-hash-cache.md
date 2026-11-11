# 0010 — Engine: walker, hashing and the rule-result cache

Status: accepted (M3)

## Context

SPEC §9 gives the Rust core a parallel gitignore-aware walker, glob matching, content hashing, a rule-result cache keyed by rule version, input-file hashes and config, and parallel scheduling of rule checks.

## Decision

### Crates

- `ignore` 0.4.34 for the walker. It is the gitignore implementation ripgrep uses, with a parallel walker built in.
- `globset` 0.4.21 for a rule's `inputs` globs. `*` stays inside one path segment, `**` crosses segments.
- `blake3` 1.8.7 for content hashes and cache keys. Fast, and nothing here needs a specific standard hash.

All three are pinned. They are the only new runtime dependencies.

### What the cache key covers

A rule's key is a BLAKE3 hash of:

1. a cache format number and the firescale version (this stands in for "rule version": built-in rules ship with the binary);
2. the rule id and its `inputs` globs;
3. the rule's options from the config;
4. for every file matching `inputs`: its path, its content hash, and **whether git tracks it**.

Point 4 goes beyond "input-file hashes" on purpose. `core/no-committed-env` depends on the git index, not on file contents: `git add -f .env` changes the result without changing a byte on disk. Including the tracked flag makes that safe. The candidate files are the walker's result (everything git would not ignore) plus every tracked file, so a tracked-but-ignored file is covered too.

A rule's status (`evaluated` or `enforced`) is not in the key: it does not change the diagnostics, only how they are reported.

### Storage

`.firescale/cache/<pack>__<rule>.json`, one entry per rule, holding the key and the diagnostics. Writes go to a temporary file and are renamed into place. firescale writes `.firescale/.gitignore` containing `*`, so the folder never shows up in `git status` and is never committed. An unreadable or outdated entry is a miss, never an error.

### What is not cached

- **Custom TypeScript rules.** Their code can change without firescale knowing, and hashing their module graph is out of scope. They run every time, one after another through the single Node host ("batched" in the sense of one process, not one request).
- Rules with no `inputs`.
- Anything when `--no-cache` is passed. It neither reads nor writes the cache.

### Scheduling

The cache is consulted for every built-in rule first. The rules that missed run in parallel on scoped threads. Reports are assembled in rule order, so output does not depend on timing.

### Seeing what happened

`FSCALE_LOG=cache` prints one line per rule to stderr: `cache: hit <id>`, `cache: miss <id>` or `cache: skip <id>`. Stdout is unchanged, so `--json` output is the same with a warm or cold cache. Tests use this.

## Consequences

- **The cache costs a repo walk on every run.** To compute keys, firescale walks the repo and reads the git index. On the 50k-file fixture that is about 60 ms, while the only built-in rule today takes about 30 ms without the cache. So with one cheap rule, a warm run is slower than `--no-cache` (numbers in `bench/RESULTS.md`). The cache starts to pay off when rules read file contents (the secrets and env-usage rules in M5). Whether the cache should stay on by default until then is listed under "Needs your decision" in DECISIONS.md.
- Possible later improvements, not done: skip the walk when every enabled rule is cheap; store the fingerprint per file with mtime and size to avoid rehashing; cache custom rules by hashing their source.
- A rule's `inputs` must list everything its result depends on. A rule that reads a file outside its `inputs` would be served stale results. Rule tests cannot catch that automatically yet.
