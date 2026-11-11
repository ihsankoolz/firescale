# Decisions log

A running log, newest milestone first. For each milestone: what was built, decisions made on your behalf (with the ADR), known limits, and what needs your decision. Anything you could reasonably disagree with is under "Needs your decision".

## Needs your decision (all milestones, most important first)

0. **Are the core rules' heuristics right for you?** (M5, ADR 0012) Seven rules were written quickly, tuned to avoid false alarms. The table in ADR 0012 says exactly what each one flags. The ones most worth your eye: `no-secrets-in-source` (which formats, what is skipped), `readme-setup` (what counts as install and run steps), and whether `single-lockfile` and `scripts-present` should cover Rust and Python.
1. **Should the result cache be on by default right now?** (M3, ADR 0010) With the one cheap rule that exists today, a warm cached `check` on a 50k-file repo is slower than `--no-cache` on every OS (p50: 64 ms against 33 ms locally; on CI runners 109 against 27 ms on Linux, 102 against 49 ms on macOS, 370 against 144 ms on Windows), because proving "nothing changed" means walking the repo. The cache will pay off for content-scanning rules in M5. Options: (a) keep it on, as now; (b) make it opt-in until M5; (c) let each rule declare whether it is worth caching. I kept (a) because PLAN.md's "Done when" describes a cache that is simply on.
2. **Removing the napi binding outright.** (M2, ADR 0008 §3) Nothing calls from Node into Rust any more. If you want the TypeScript SDK to reuse the Rust walker or hasher in-process, a thin binding would have to come back.
3. **"No Node process" under an npm install.** (M2, ADR 0008 §6) The usual npm layout puts a small Node launcher in front of the binary. The design adds an optional hard-link step that removes it where install scripts are allowed. Decide whether that is acceptable when M6 (npm packaging) is built.
4. **Custom rule packs are listed by path only.** (M2, ADR 0008 §5) Loading a pack by npm package name is not supported.
5. **`fscale fix` also fixes rules that are only `evaluated`, and exits 0 when violations remain.** (M4, ADR 0011) If you would rather fix only `enforced` rules, or fail when something is left, say so.
6. **`--affected` has no default base.** (M4, ADR 0011) Without `--base` it only looks at uncommitted and untracked files. A default such as `origin/main` could be added.
7. **No TOML or YAML parser.** (M4, ADR 0011) `fscale facts` reads `Cargo.toml` and `pnpm-workspace.yaml` with small hand-written readers. Adding the `toml` crate would be more robust at the cost of a dependency.
8. **Custom TypeScript rules are never cached.** (M3, ADR 0010) They run on every `check`. Caching them needs a way to hash their source.
9. **`.envrc` is not flagged** by `core/no-committed-env`; `.ENV`-style names are. (M1/M2, ADR 0007 point 10)
10. **TypeScript is pinned to 6.0.3**, not 7, until `typescript-eslint` supports 7. (M0, ADR 0002)

## M5 — Core rule pack

**Built**

- The seven remaining `core` rules, in Rust, each with a check, a SKILL.md and fixture tests: `gitignore-basics`, `single-lockfile`, `scripts-present`, `ci-exists`, `readme-setup`, `env-example`, `no-secrets-in-source`.
- Deterministic fixes for `gitignore-basics` and `env-example`.
- `fscale check` on this repo reports nothing with all eight rules, and a test keeps it that way.

**Decisions made for you** (all ADR 0012)

- Every heuristic prefers missing a problem to raising a false alarm.
- `single-lockfile` and `scripts-present` only look at JavaScript projects for now.
- `no-secrets-in-source` matches well-known token formats only; no entropy scoring, no generic password patterns. Findings never contain the secret.
- `ci-exists` understands GitHub Actions only and stays silent when another CI system's config is present.
- Only `gitignore-basics` and `env-example` have fixes. The others are marked for an agent or a person.

**Known limits**

- These rules were written and reviewed in the last half hour of the session. They have tests and passed a short review, but less scrutiny than M2 to M4. Treat the heuristics as a first version.
- No Rust or Python equivalents of the lockfile and scripts rules.
- `no-secrets-in-source` hashes every tracked file for its cache key, so the cache gives it little on large repos.
- The secrets rule misses tokens in formats it does not know.

## M4 — `fix`, SARIF and facts

**Built**

- `fscale fix` and `fscale fix --dry-run`, for built-in rules and, through the bridge, for custom TypeScript rules.
- `fscale check --sarif` (SARIF 2.1.0, validated against the official schema in tests).
- `fscale facts` and `fscale facts --json`, with a JSON schema: package manager, lockfiles, languages, tools, commands and a project graph from workspace files.
- `fscale check --affected [--base <ref>]`.

**Decisions made for you** (all ADR 0011)

- A dry run is the real fix running in record-only mode, not a separate plan.
- `fscale fix` exits 0 even when violations remain, and fixes rules that are only `evaluated`.
- SARIF levels: report-only rules are `note`; enforced rules are `error` (blocker, high), `warning` (medium) or `note` (low).
- `facts` uses small hand-written readers for `Cargo.toml` and `pnpm-workspace.yaml`, with no TOML or YAML dependency.
- `--affected` filters what is reported and does not skip rules. It has no default base ref.
- The help text was reorganised around `[options]`, which changed the help snapshot on purpose.

**Known limits**

- Nx's project graph and uv workspaces are not read. `nx.json` is only reported as a tool.
- A custom rule that writes files without using its context cannot be previewed by `--dry-run`.
- The diff in a dry run has one hunk per file, shows CRLF as LF, and falls back to "remove all, add all" for very large rewrites.
- In a dry run, a fix that reads back a file it has just written sees the old content, so its preview can differ from the real run. No built-in fix does this.
- `fscale facts` skips a workspace member whose folder does not exist without a warning, and does not handle two projects with the same name.
- `--affected` does not make `check` faster.
- The facts readers handle common `Cargo.toml` and workspace forms only; see `docs/facts.md`.

## M3 — Engine: walk, hash, cache

**Built**

- A parallel, gitignore-aware file walker, glob matching for rule inputs, and BLAKE3 content hashing, in `crates/core`.
- A rule-result cache in `.firescale/cache/`. `fscale check --no-cache` bypasses it. `FSCALE_LOG=cache` shows hits and misses.
- Built-in rules that miss the cache run in parallel.
- `bench/generate-fixture.mjs` and `bench/check-bench.mjs`, plus a CI job that runs the 50k-file benchmark on Linux, macOS and Windows.
- The four loose ends from M2: an airtight "no Node process" test (a stand-in `node` binary records whether it was started), tests for the automatic lookup of the Node host, the `core/no-committed-env` SKILL.md now covers case variants (its `explain` snapshot was updated on purpose), and per-OS benchmark numbers from CI.
- PLAN.md reordered: npm packaging is now M6.

**Decisions made for you**

- Walker, globs and hashing use the `ignore`, `globset` and `blake3` crates (ADR 0010).
- The cache key includes whether git tracks each input file, not only file contents, because the one existing rule depends on the git index (ADR 0010).
- "Rule version" in the key is the firescale version, since built-in rules ship inside the binary (ADR 0010).
- firescale writes `.firescale/.gitignore` so the cache is never committed (ADR 0010).
- Custom TypeScript rules are not cached and run one at a time through the single Node host (ADR 0010).
- A new test-only crate, `crates/testkit`, holds the stand-in `node` binary. It is not shipped.
- The help text gained a `--no-cache` line, which changed the help snapshot on purpose.

**Known limits**

- The cache costs a repo walk on every run (see decision 1).
- A rule that reads a file outside its declared `inputs` would be served stale results. Nothing checks this automatically.
- If an input file changes while a check is running, the result can be stored under the key of the old content. The next run recomputes the key and misses, so the stale entry is not served unless the file is changed back.
- The "no Node process" test puts a stand-in `node` first on `PATH`. It would not notice Node started by an absolute path.
- The benchmark CI job records numbers but has no time budget, so a slowdown does not fail it.
- Benchmarks on macOS and Windows come from shared CI runners, so they vary from run to run.
- The host lookup next to the binary searches five parent folders, so in an unusual layout it could find a `firescale` package from a different checkout.

## M2 — Rust front door

**Built:** `fscale` as a native Rust binary with `check`, `explain` and `--version`; JSON config in Rust; `core/no-committed-env` in Rust; the Node bridge for `.ts` config and custom rules; startup benchmarks.

**Decisions made for you:** napi binding removed; TypeScript CLI and TypeScript rule pack removed; custom packs listed by path; `--rules <id>` runs an off rule as report-only with a note on stderr; `serde_json` keeps key order (ADR 0008). CI runs Node 22 and 24 with actions pinned to commit SHAs (ADR 0009).

**Known limits:** nothing is installable from npm yet; the bridge runs a custom rule's `check` but not its `fix`; usage error wording differs from the old CLI.

## M1 — Rule SDK and the first rule

**Built:** the TypeScript rule SDK, config loader, `check`, `explain`, `core/no-committed-env`, the JSON schema for `check --json`.

**Decisions made for you:** config format and strict validation (ADR 0005); fixture repos built at test time (ADR 0006); the rule API (ADR 0007).

## M0 — Skeleton

**Built:** pnpm and Cargo workspaces, CI on three OSes, lint and test tooling.

**Decisions made for you:** toolchain versions (ADR 0002); planning files renamed to `SPEC.md` and `PLAN.md` (ADR 0004).

## Left for you to do by hand

- Decide the items under "Needs your decision" at the top of this file.
- Commits made by the build sessions use your name and email but are not signed, so GitHub shows them as "Unverified". If you want verified history, sign going forward; re-signing old commits means rewriting history.
- The branches `m0-skeleton`, `m1-rule-sdk`, `m2-rust-front-door`, `m3-engine-cache` and `m4-fix-sarif-facts` are merged and can be deleted.
- The M8 manual test with a real agent is not reachable yet.

## Publishing to npm

Not possible yet: packaging is M6 and has not been built. The exact publish steps will be written here when it is.
