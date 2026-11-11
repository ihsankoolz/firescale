# Decisions log

A running log, newest milestone first. For each milestone: what was built, decisions made on your behalf (with the ADR), known limits, and what needs your decision. Anything you could reasonably disagree with is under "Needs your decision".

## CI owed

GitHub Actions stopped starting jobs on 2026-11-11 (the account's monthly limit), and you asked for work to continue without it. From that point everything is verified on Linux only, on the build machine. M6 pushes carried `[skip ci]`; the M7 pull request did not, to find out whether Actions was back. It was not. When Actions is available again, start the CI workflow by hand on `main` (Actions tab, "CI", "Run workflow"); that runs the checks, the benchmarks and the npm install tests on all three OSes.

Not yet proven on macOS or Windows:

- **M7 (not merged):** the pull request's CI run on 2026-11-11 did not execute. All four jobs ended as failed within four seconds, with no runner assigned and no logs, which is what the Actions limit looks like. M7 is verified on Linux only (`pnpm fscale verify` passes). It was left unmerged because the instruction was to merge on green CI on all three OSes. The review judged the Windows and macOS risk low after the fixes, but two things have never run on Windows: the raw `cmd` command line in `crates/core/src/verify.rs`, and the Vitest stdin handling for the stop hook. To finish: re-run the failed jobs on the pull request once Actions works, then merge.
- **M6:** the reworked npm install test (`scripts/smoke-npm.mjs`) on Windows. The earlier version passed on Linux and macOS; on Windows the install, `--version` and `check` passed and the script then failed on its own quoting bug, which is fixed but unconfirmed. The launcher changes from the review are also Linux-only so far.

## Needs your decision (all milestones, most important first)

- **(Direction) Read the new PLAN.md from M8 onward before anything is built.** It is in its own pull request, which is deliberately not merged. The points most worth your eye are listed under "Direction change" below.
- **(Direction) Publishing to npm now gates real use.** `doctor` and the group-project pack are for teammates, and teammates can only install firescale once it is on npm. Publishing is yours to do (see "Publishing to npm"). The CI workflow that `init` will generate (M9) also runs `npx firescale verify`. Consider publishing before M9.
- **(Direction) Blame-aware verify becomes the default behaviour of `fscale verify`** whenever there is a base commit to compare with. A failure that already exists on the base will no longer fail verify or hold an agent back. On the default branch itself every failure still blocks. Say if you would rather have it behind a flag.

- **(M7) The Cursor hook format is unconfirmed.** cursor.com was blocked from the build session, twice. `fscale hooks install` writes `.cursor/hooks.json` from a best reading and prints a note. Someone with access to Cursor's docs should check it (ADR 0014), or allow `cursor.com` in the cloud environment's network settings so a session can.
- **(M7) The `agent-ready` pack is opt-in**, not on by default. Say if you want it on for every repo.
- **(M7) A verify step with no command is skipped, not failed.** A repo with no tests passes `fscale verify`. The `agent-ready/single-test-command` rule is how you make that an error.
- **(M7) The hook lets an agent stop after 3 failed attempts in a row.** `FSCALE_HOOK_MAX_BLOCKS` changes it.

- **(M6) The npm name `fscale` is taken by an unrelated package.** `npx firescale` works; `npx fscale` would run that other package. After `npm install firescale` the command `fscale` is ours. If that risk bothers you, the options are to ask npm about the name or to make `firescale` the only command.
- **(M6) No postinstall step, so one Node start sits in front of every `fscale` run from npm.** ADR 0013 explains why. Agent hooks can call the binary directly. Say if you want the postinstall optimisation anyway.

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

## Direction change (after M7, docs only)

**Written**

- ADR 0015: firescale stays a toolkit for small teams and gains five changes: rules are the scaffold, born agent-ready, a built-in referee (blame-aware verify and a tamper guard), group-project mode, and a ledger.
- SPEC.md updated to match. Sections 1 to 14 keep their numbers, because ADRs and code comments cite them; the new sections are 15 (rules as the scaffold), 16 (born agent-ready) and 17 (group-project mode). The referee is in section 8, with verify.
- PLAN.md rewritten from M8 onward: nineteen milestones, M8 to M26, each sized for one session.
- README.md's first paragraph now gives the new pitch.

**Decisions made for you** (all ADR 0015)

- **The docs describe the product in its own words.** Your brief described it by comparison with another product, by name. AGENTS.md's originality rule says that name must never be used in this repo, so the ADR, SPEC and PLAN say "a toolkit for small teams" instead. The meaning is unchanged.
- **Milestone order: yours, with adjustments.** (1) The fix loop (ordering by declared dependencies, the convergence error) is its own milestone, M8, before `init`, because `init`, `adopt` and `sync` all stand on it. (2) The study of trustbut, goodhart and skylos opens the tamper guard (M17) instead of being a milestone of its own. (3) The ledger file is written by the runner (M21), since that is one of its guardrails; `fscale ledger` and cost come with the adapters (M22).
- **Themes are split so a milestone fits a session:** scaffold (M8 loop, M9 commands), group project (M10 pack, M11 doctor), agent-ready (M12 agent files, M13 generators), blame-aware verify (M14 base run, M15 per-test labels, M16 reruns and the stop hook), tamper guard (M17 design and JavaScript, M18 Rust, Python and policy, M19 config and CI detectors, M20 approvals and measured rates), runner (M21 guardrails and ledger file, M22 adapters and `fscale ledger`).
- **`fscale init` turns on `agent-ready` by default**, because you asked that `init` also writes the agent files. For `fscale check` in an existing repo the pack stays opt-in (ADR 0014).
- **Verify with a base runs every step** instead of stopping at the first failure, and `--base` works without `--affected`. Both change what M7 built, and are needed so that an old failure cannot hide a new one.
- **"Head" means the working tree**, uncommitted changes included, because that is what a stop hook judges.
- **Stack packs: one milestone each, Python first, then Next.js, Supabase, Expo** (simplest toolchain first).
- **`init` no longer needs Node.** It is the fix loop, in Rust. Only `add` and `schema` use the TypeScript generators.
- **A fix never replaces content a person wrote.** It creates, merges, appends, or rewrites a managed block.
- **Gemini CLI support is under "Later".** SPEC names Gemini, but nothing in your brief asks for it.
- **A docs review before the pull request** found milestones that were too large (the tamper guard and blame-aware verify were split further), contradictions with M7's behaviour, and missing test boxes. All were addressed.
- **Reading alint output is under "Later"**, not a numbered milestone, since you called it optional.
- **This pull request is stacked on the M7 branch**, because M7 could not be merged (its CI did not start; see "CI owed"). Its base is `m7-verify-hooks`, so it shows only the docs. Merge M7 first; GitHub then points this one at `main`.

**Left open on purpose, each for its milestone's ADR**

- The exact `after` syntax and the loop's pass limit (M8).
- Which agent permission settings count as "safe" (M12).
- How verify gets per-test results from a runner without changing the user's command (M15).
- Which tree-sitter grammars ship, and what they cost in binary size (M17).
- Where human approvals for changed snapshots are stored, so that an agent cannot write its own (M20).
- How the base worktree gets its dependencies installed, and what that costs (M14).

## M7 — `verify` and agent hooks

**Built**

- `fscale verify` (`--affected`, `--json`): check, lint, typecheck, test, with one compact summary. Commands come from the `verify` config key, Nx, Turborepo, package scripts or Cargo.
- `fscale hooks install` for Claude Code, Codex and Cursor, and `fscale hooks run`, the stop hook with a loop guard.
- The `agent-ready` rule pack: `agents-md`, `commands-documented`, `single-test-command`, `verify-hook`.
- This repo runs its own hooks: `.claude/`, `.codex/` and `.cursor/` call `fscale verify`.
- `bench/verify-bench.mjs` and verify latency in `bench/RESULTS.md` (firescale's share: p50 8.8 ms, p99 18.0 ms).
- Docs: `docs/verify.md`, `docs/hooks.md`, and the JSON schema for `verify --json`.

**Decisions made for you** (all ADR 0014)

- Steps stop at the first failure. A step with no command is skipped and does not fail.
- The hook command is `fscale hooks run --agent <name>`, so the loop guard and each agent's reply format live in the binary.
- Hooks merge into existing agent settings and never touch user-level settings. `--bin` chooses how the hook starts fscale; for npm installs use `npx firescale`.
- The loop guard allows 3 blocks in a row per session, then lets the agent stop with a note.
- `agent-ready` is opt-in and has no deterministic fixes yet.
- `fscale explain` now says which pack to turn on when a rule's pack is off.
- Two test expectations were updated because the text they pin changed on purpose: the config error's list of allowed keys (now includes `verify`; this was the known failing test), and the help snapshot.

**Known limits**

- Cursor's hook format is unconfirmed (see above).
- Verify latency was measured on Linux only.
- No time limit on verify steps; output is captured, not streamed.
- No `hooks uninstall`. Gemini CLI has no hook support here.
- `agent-ready/verify-hook` can serve a stale cached result when the hook file is untracked and git-ignored.
- An independent review before the pull request found eight issues; all were fixed with tests (listed in ADR 0014, "Consequences"). The most important: the loop guard did not cap an agent that omits `stop_hook_active`, and quoted commands were mangled on Windows.
- A subagent started the `agent-ready` pack and stopped at a usage limit after the first rule; the other three were written in the main session.

## M6 — npm packaging

**Built**

- The `fscale` launcher in the `firescale` package, and `scripts/pack-npm.mjs`, which builds the `firescale`, `@firescale/sdk` and per-platform tarballs.
- `scripts/smoke-npm.mjs`: installs the tarballs into an empty project with npm, pnpm or Yarn Classic and runs `fscale --version`, `check`, `explain` and a custom TypeScript rule. A CI job runs it on Linux, macOS and Windows.
- A Release workflow that builds six platforms and uploads tarballs. It does not publish.
- Launcher overhead measured in `bench/RESULTS.md`.

**Decisions made for you** (all ADR 0013)

- Eight packages at one version: `firescale`, `@firescale/sdk`, and six `@firescale/cli-<os>-<arch>` binary packages.
- **No postinstall step.** ADR 0008 proposed one to remove the Node launcher from the path. It is not built: package managers block install scripts too often for it to be reliable.
- The launcher passes its own Node and host script to the binary, so TypeScript rules use the same Node.
- `@firescale/sdk` is now publishable (it was private).

**CI was trimmed at your request** (ADR 0009 amendment): each push now runs 4 check jobs instead of 12 jobs. The benchmark and npm install jobs run only when you start the CI workflow by hand from the Actions tab, or on a pull request labelled `ci:bench` or `ci:package`. Run them before a release.

**Known limits**

- No musl (Alpine) build. Yarn Berry with Plug'n'Play is untested.
- The arm64 targets and the two cross-compiled targets of the Release workflow have never run.
- "No Rust toolchain" is shown by installing only from tarballs into an empty project; CI runners do have Rust installed.
- `npx fscale` runs someone else's package. The command to document is `npx firescale`.

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
- This repo now has a `firescale.config.json`. It excludes the `env-example` rule's own test fixtures from that rule, because they contain env reads on purpose. Dogfooding found this in CI.

**Known limits**

- These rules were written and reviewed in the last half hour of the session. They have tests and passed a short review, but less scrutiny than M2 to M4. Treat the heuristics as a first version.
- No Rust or Python equivalents of the lockfile and scripts rules.
- `no-secrets-in-source` hashes every tracked file for its cache key, so the cache gives it little on large repos.
- The secrets rule misses tokens in formats it does not know.
- Raised by the review and not fixed:
  - `no-secrets-in-source` does not scan `dist/`, `build/` or `vendor/` folders, so a secret committed there is missed.
  - `env-example` scans test files too, so variables only tests use are reported.
  - `env-example` can still pick up an env read written inside a multi-line string, and misses destructuring and computed names.
  - `single-lockfile` looks at parent folders up to the repo root, which are not in its cache inputs when firescale runs inside a workspace member.
  - `gitignore-basics`' fix writes only its comment line when no language is detected.

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
- The manual test with a real agent (now M22) is not reachable yet.

## Publishing to npm

Nothing has been published. The packaging is built and tested (M6); publishing is yours to do. Exact steps:

**Once, before the first publish**

1. Create the npm organisation `firescale` (the `@firescale` scope) under your npm account: npmjs.com, "Add Organization". The name was free on 2026-11-11, as was the package name `firescale`.
2. Turn on two-factor authentication for publishing on your npm account.
3. Decide the version. Everything is `0.1.0`. To change it, edit `version` under `[workspace.package]` in `Cargo.toml` and the `version` in every `package.json` (a test fails if they differ), and commit.

**For each release**

1. Tag the commit and push the tag, or run the "Release" workflow by hand from the Actions tab:

   ```bash
   git tag v0.1.0
   ```

   ```bash
   git push origin v0.1.0
   ```

2. Wait for the Release workflow. It builds all six platforms and uploads the tarballs as artifacts. Download and unpack them into one folder. You should have eight `.tgz` files: six `firescale-cli-<platform>-0.1.0.tgz`, one `firescale-sdk-0.1.0.tgz`, one `firescale-0.1.0.tgz`. (Each platform's artifact also carries its own copy of the sdk and firescale tarballs. Use the ones from the `linux-x64` artifact and discard the rest.)
3. Check one before publishing anything:

   ```bash
   tar -tzf firescale-0.1.0.tgz
   ```

4. Log in. With two-factor authentication on, npm asks for a one-time code for each of the eight publishes; add `--otp <code>` to a command to pass it up front:

   ```bash
   npm login
   ```

5. Publish the platform packages first, then the SDK, then `firescale` last, so `firescale` never points at packages that are not there yet:

   ```bash
   for f in firescale-cli-*-0.1.0.tgz; do npm publish "$f" --access public; done
   ```

   In PowerShell, the same step is:

   ```powershell
   Get-ChildItem firescale-cli-*-0.1.0.tgz | ForEach-Object { npm publish $_.FullName --access public }
   ```

   ```bash
   npm publish firescale-sdk-0.1.0.tgz --access public
   ```

   ```bash
   npm publish firescale-0.1.0.tgz
   ```

6. Check from an empty folder on a machine without this repo:

   ```bash
   npx firescale@0.1.0 --version
   ```

A published version cannot be replaced, only deprecated. If a publish fails halfway, publish the missing packages; do not bump the version to retry.
