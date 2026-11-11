# firescale — Build Plan

One milestone per session and per pull request. A milestone is done only when every box under "Done when" is true.

Status key: ☐ not started · ◐ in progress · ☑ done

---

## M0 — Skeleton ☑

**Build**

- pnpm workspace + Cargo workspace, laid out as in SPEC §9.
- TypeScript strict config, ESLint, Prettier, Vitest.
- Rust: `cargo fmt`, `clippy` with warnings denied, `cargo test`.
- GitHub Actions CI on Windows, macOS and Linux: install, lint, typecheck, test, build.
- A `fscale --version` command that calls one trivial function in the Rust core through the Node binding. This proves the bridge works on all three OSes.
- README with a one-paragraph pitch and dev setup.
- `docs/decisions/` folder with an ADR for each real choice made (binding approach, test runner, and so on).

**Done when**

- ☑ CI is green on all three OSes.
- ☑ `pnpm build && pnpm test` passes from a clean clone.
- ☑ `fscale --version` prints a version that came through the Rust binding.

---

## M1 — Rule SDK and the first rule ☑

**Build**

- `packages/sdk`: `defineRule`, `Diagnostic`, `FixResult`, rule context, and a fixture-based test helper.
- Config loader for `firescale.config.ts` / `.json` (SPEC §10).
- `fscale check` with human output and `--json`.
- `fscale explain <rule>`.
- One real rule end to end: `core/no-committed-env`, with check, fix, SKILL.md and tests.
- A SKILL.md template and a test that validates every SKILL.md has the required sections.

**Done when**

- ☑ `fscale check` finds a tracked `.env` in a fixture repo and exits 1 when the rule is `enforced`.
- ☑ `fscale check --json` output matches a documented schema, with a snapshot test.
- ☑ The rule test covers: passing repo, failing repo, fixed repo, fix run twice (idempotent).

---

## M2 — Rust front door ☑

Design change, see ADR 0008: `fscale` is a native Rust binary, core rules are built in, and Node is started only for TypeScript rules, `.ts` config, generators and agent adapters.

**Build**

- `crates/cli`: the `fscale` binary with `check`, `explain` and `--version`. Same behaviour, same exit codes and the same `--json` schema as M1.
- Config loading for `firescale.config.json` in Rust. `firescale.config.ts` is loaded by starting Node.
- `core/no-committed-env` ported to Rust as a built-in rule, with the same tests.
- The Node bridge: one child process, JSON lines. Runs a custom rule defined with the M1 SDK.
- Remove the napi binding and the TypeScript CLI and rule pack it replaces.
- Small fixes: flag `.ENV` case variants; the SKILL.md validator ignores headings inside code fences; `--rules <id>` runs a rule that is `off`, with a note; Node 22 in the CI matrix; GitHub Actions pinned to commit SHAs.
- `bench/startup.mjs` and startup numbers before and after in `bench/RESULTS.md`.

**Done when**

- ☑ `fscale` is a Rust binary, and the M1 CLI snapshot tests for `check`, `explain` and `--version` pass against it unchanged.
- ☑ `fscale check --json` output validates against the unchanged JSON schema.
- ☑ A repo with only built-in rules runs with no Node process, proven by a test.
- ☑ `core/no-committed-env` is built in, with tests for: passing repo, failing repo, fixed repo, fix run twice.
- ☑ A sample custom TypeScript rule runs through the bridge, and a `.ts` config loads through Node, each proven by a test.
- ☑ Startup time for `fscale --version` and `fscale check`, before and after, is recorded in `bench/RESULTS.md`.
- ☑ CI is green on all three OSes, on Node 22 and Node 24.

---

## M3 — Engine: walk, hash, cache ☑

**Build** (all Rust, in `crates/core`)

- Parallel, gitignore-aware file walker.
- Glob matching and content hashing.
- Rule-result cache keyed by rule version + input-file hashes + config hash. Stored under `.firescale/cache/`.
- Parallel scheduling of rule checks. Custom TypeScript rules are batched through the one Node child.
- `bench/`: a generator for large fixture repos and a benchmark script.
- Loose ends from M2: an airtight "no Node process" test, tests for the automatic lookup of the Node host, the `core/no-committed-env` SKILL.md mentions case variants, and benchmarks recorded from Windows and macOS CI runners.

**Done when**

- ☑ A second `fscale check` with no file changes is served from cache, proven by a test.
- ☑ Changing a file inside a rule's `inputs` invalidates only that rule's result, proven by a test.
- ☑ Benchmarks for cold and warm runs on a 50k-file fixture are recorded in `bench/RESULTS.md`.
- ☑ All Rust code passes clippy with warnings denied.

---

## M4 — `fix`, SARIF and facts ☑

**Build**

- `fscale fix` and `fscale fix --dry-run` for built-in rules, and for custom rules through the bridge.
- SARIF output (`--sarif`).
- Stack detection (`fscale facts`, `--json`) from marker files, and the simple project graph.
- `--affected` support using git diff plus the project graph.

**Done when**

- ☑ `fscale fix` fixes `core/no-committed-env` in a fixture repo, and a second run changes nothing.
- ☑ `fscale fix --dry-run` prints a diff and changes nothing.
- ☑ `--sarif` output validates against the SARIF 2.1.0 schema, with a snapshot test.
- ☑ `fscale facts --json` has a documented schema and a snapshot test.

---

## M5 — Core rule pack ☑

**Build**

- The remaining seven `core` rules from SPEC §6, in Rust, each with check, SKILL.md, tests, and a deterministic fix where one is safe.

**Done when**

- ☑ All 8 core rules pass their fixture tests on all three OSes.
- ☑ Every fix is idempotent, proven by tests.
- ☑ Every SKILL.md passes validation.
- ☑ Running `fscale check` on this repo itself reports zero violations (dogfooding).

---

## M6 — npm packaging ☑

**Build**

- Per-platform binary packages and the `firescale` launcher package, as designed in ADR 0008 §6.
- A release workflow that builds the binary for each platform and assembles the packages. Publishing itself stays a manual, approved step.
- Install smoke tests: `npm`, `pnpm` and `yarn`, on all three OSes, from locally packed tarballs.

**Done when**

- ☑ `npx firescale --version` works from packed tarballs on all three OSes with no Rust toolchain.
- ☑ A custom TypeScript rule works from an npm install, proven by a smoke test.
- ☑ The launcher's overhead is measured and recorded in `bench/RESULTS.md`.

---

## M7 — `verify` and agent hooks ☑

**Build**

- `fscale verify` per SPEC §8, in Rust, with the compact agent-readable summary and `--json`.
- Runner detection: Nx, Turborepo, or plain package scripts.
- `fscale hooks install` for Claude Code, Codex and Cursor. Confirm each agent's current hook format from its official docs before writing it.
- Loop guard and retry cap.
- The `agent-ready` rule pack, in Rust.
- From this milestone on, this repo's own agent hooks call `fscale verify`.

**Done when**

- ☑ `fscale verify` exits non-zero with a clear summary when a fixture repo has a failing test.
- ☑ Hook files are generated for three agents, with snapshot tests.
- ☑ The loop guard is covered by a test.
- ☑ p50 and p99 verify latency are recorded in `bench/RESULTS.md`.

---

## Direction change after M7

On 2026-11-11 the direction was refined (ADR 0015): firescale stays a toolkit for small teams, the rules become the scaffold, every repo is born agent-ready, and a referee keeps agents honest. Everything below was rewritten then. Milestones are sized for one session each. Themes, in order:

| Theme                         | Milestones |
| ----------------------------- | ---------- |
| Rules are the scaffold        | M8, M9     |
| Group-project mode            | M10, M11   |
| Born agent-ready              | M12, M13   |
| Referee: blame-aware verify   | M14, M15   |
| Referee: tamper guard         | M16, M17   |
| Guarded agent runner, ledger  | M18, M19   |
| Stack packs                   | M20 to M23 |

---

## M8 — The fix loop ☐

The engine under `init`, `adopt` and `sync`: run fixes in order until nothing is left to fix.

**Build**

- A rule can declare the rules that must be fixed before it (`after`), for built-in rules and in the TypeScript SDK. A cycle or an unknown id is a config-time error that names the rules.
- A fixed-point loop in `crates/core`: check, run every enabled rule's deterministic fix in dependency order, repeat until no deterministic violation is left or a pass changes nothing.
- A pass limit. When it is reached, or when two fixes keep rewriting the same file, stop with an error that names the rules and files.
- `fscale fix` uses the loop. `fscale fix --dry-run` shows the combined diff of all passes.
- An ADR for the loop: ordering, the limit, and how a dry run sees files that earlier fixes would have written (a known limit from M4).

**Done when**

- ☐ Two fixture rules where B must run after A are fixed in one `fscale fix`, in that order, proven by a test.
- ☐ A dependency cycle and an unknown `after` id are each rejected with a clear error, proven by tests.
- ☐ Two fixture rules that undo each other stop the loop with an error naming both, and the exit code is not 0.
- ☐ Running `fscale fix` a second time changes nothing, for every built-in rule with a fix.

---

## M9 — `init`, `adopt` and `sync` ☐

**Build**

- `fscale init [--packs <list>]`: in an empty or new folder, run `git init` if needed, write `firescale.config.json`, then run the fix loop until `fscale check` passes. No templates.
- Fixes that create files for the core rules that lack one and can have one safely: a starter CI workflow that runs `fscale verify` (`ci-exists`), a README skeleton with install and run sections (`readme-setup`). Rules that need a real decision stay agent or manual.
- `fscale adopt`: the same loop on an existing repo. Shows the plan as a diff first (`--dry-run`), never overwrites a file it did not create, and lists what is left for a person or an agent.
- `fscale sync` and `fscale sync --check`: run the loop again to repair drift; `--check` changes nothing and exits 1 when a fix would change something.
- `docs/init.md`.

**Done when**

- ☐ `fscale init` in an empty folder yields a repo where `fscale check` passes with every core rule enforced except those listed as needing a person.
- ☐ `fscale adopt --dry-run` on a fixture repo prints a diff and changes nothing; `fscale adopt` then makes `fscale check` report no deterministic violations.
- ☐ `fscale sync --check` exits 1 after a generated file is deleted, and 0 after `fscale sync`.
- ☐ `init`, `adopt` and `sync` start no Node process in a repo with only built-in packs, proven by a test.

---

## M10 — The group-project pack ☐

**Build**

- A `group-project` pack, in Rust, opt-in. Rules, each with a fix that creates the file: a pull request template, a contributing guide, a CI workflow that runs `fscale verify` on pull requests, and a branch naming rule (checks the current branch against a pattern from the rule's options; no fix).
- Each rule has a SKILL.md and fixture tests: passing, failing, fixed, fixed twice.
- `fscale init --packs core,group-project` builds a group-project repo through the fix loop alone.

**Done when**

- ☐ All group-project rules pass their fixture tests, and every fix is idempotent.
- ☐ `fscale init --packs core,group-project` in an empty folder yields a repo where `fscale check` passes for the pack.
- ☐ The generated CI workflow is valid YAML that runs `fscale verify`, with a snapshot test.

---

## M11 — `fscale doctor` ☐

**Build**

- `fscale doctor [--json]`: checks the machine, not the repo. Tool versions against what the repo declares (`packageManager`, `engines`, `rust-toolchain.toml`, `.nvmrc`, `.python-version`), every variable named in `.env.example` (set in the environment or present in a local env file; names only, values are never read out or printed), and whether dependencies are installed and match the lockfile.
- Each problem comes with the command that fixes it. Exit 1 when something is wrong.
- A JSON schema for `--json`, and `docs/doctor.md`.

**Done when**

- ☐ `fscale doctor` reports a missing tool, a wrong version, a missing env variable and missing dependencies in fixture setups, each with a next step, proven by tests.
- ☐ `fscale doctor` exits 0 on this repo on a machine that can build it.
- ☐ No test and no output ever contains the value of an env variable.
- ☐ `--json` output validates against its schema, with a snapshot test.

---

## M12 — Born agent-ready: agent files from `init` ☐

**Build**

- Confirm the current Claude Code, Codex and Cursor formats for skills, hooks and permissions from their official docs first (Cursor's hook format is still unconfirmed from M7). Record them in an ADR.
- Fixes for the `agent-ready` rules: an AGENTS.md **managed block** (commands from `fscale facts`, the verify rule; text outside the block is never touched), the verify hooks, and safe default permissions (deny reading env files and secrets; allow the repo's own check commands).
- A new rule with a fix that installs the SKILL.md of every enabled rule where each agent looks for skills.
- `fscale init --packs core,agent-ready` writes all of it through the fix loop. `fscale sync` repairs a hand-edited managed block.

**Done when**

- ☐ `fscale init` with `agent-ready` on yields a repo where the whole `agent-ready` pack passes, for all three agents.
- ☐ Text outside the AGENTS.md managed block survives `fscale sync` unchanged, proven by a test.
- ☐ Generated agent files have snapshot tests, and an existing agent settings file keeps its other settings.
- ☐ `fscale sync --check` fails after a hand edit inside the managed block.

---

## M13 — Generators: `add` and `schema` ☐

**Build**

- `packages/generators`: the generator interface in TypeScript, called through the bridge.
- `fscale add <generator> --json <input> [--dry-run]` and `fscale schema <generator>`. Input is validated against the schema before anything is written.
- Two real generators to prove the interface: a workspace package and a rule (rule folder with check, SKILL.md and test).
- A skill, installed by M12's mechanism, telling agents to call `fscale add` instead of hand-writing boilerplate.

**Done when**

- ☐ `fscale schema <generator>` prints a valid JSON Schema, and `fscale add` rejects input that does not match it with an error naming the field.
- ☐ `fscale add --dry-run` prints a diff and changes nothing.
- ☐ Each generator has snapshot tests, and its output passes `fscale check`.

---

## M14 — Blame-aware verify: the base run ☐

**Build**

- Study trustbut, goodhart and skylos first. Write the referee ADR: what each does, what firescale does differently, and how the false-positive rate will be measured.
- Run verify on the base commit in a temporary git worktree. Never touch the user's working tree or index.
- Cache base results under `.firescale/` keyed by base SHA + toolchain versions + lockfile hash.
- Label at step level and for rule diagnostics: `new`, `pre-existing`, `fixed`. Only `new` makes `fscale verify` exit 1. The base is `--base <ref>`, or the merge base with the default branch.
- With no base that differs from the head (on the default branch itself, or outside git), verify behaves as it does today: every failure blocks.
- The summary and `--json` carry the labels (schema version bump).

**Done when**

- ☐ In a fixture repo whose test already fails on the base commit, `fscale verify` exits 0 and labels the failure `pre-existing`.
- ☐ A failure introduced on the head is labelled `new` and exits 1; one repaired on the head is labelled `fixed`.
- ☐ A second run with the same base makes no base run, proven by a test; changing the lockfile invalidates the cache.
- ☐ The user's working tree and index are byte-for-byte unchanged after a run, proven by a test.
- ☐ On the default branch, with no separate base, a failing test still exits 1.

---

## M15 — Blame-aware verify: per-test labels and reruns ☐

**Build**

- Failure identity below step level: read test results per test from the common runners' machine-readable output (start with Vitest or Jest, `cargo test` and pytest), so one new failing test is not hidden by ten old ones.
- Rerun a `new` failure once before blocking. A pass on the rerun is labelled `flaky` and does not block. No flaky history is stored.
- The stop hook (`fscale hooks run`) uses blame-aware verify, so an agent is only sent back for failures it introduced.
- p50 and p99 of blame-aware verify, with a cold and a warm base cache, in `bench/RESULTS.md`.

**Done when**

- ☐ With two failing tests, one old and one new, only the new one is labelled `new` and named in the summary.
- ☐ A test that fails once and then passes is labelled `flaky` and does not block, proven with a fixture that fails on its first run only.
- ☐ Nothing about past flaky results is written to disk, proven by a test.
- ☐ The stop hook lets an agent stop when every failure is `pre-existing`.

---

## M16 — Tamper guard: tests and assertions ☐

**Build**

- An ADR for tree-sitter: which grammars, binary size and build time before and after.
- Parse base and head with tree-sitter for JavaScript and TypeScript, Rust and Python. Count, per file and in total: tests, skipped tests, assertions.
- Compare by net counts, so a moved or renamed test is not a finding. Hard-fail: tests deleted or newly skipped, and a net loss of assertions.
- Policy (protected paths, allowed exceptions) is read from the base commit's firescale config.
- Runs as part of `fscale verify`, reported with file and line.

**Done when**

- ☐ Skipping a test, deleting a test and removing an assertion are each caught in fixture repos for all three languages.
- ☐ Moving a test to another file, renaming it, and adding tests are not findings.
- ☐ Loosening the policy in the same change has no effect, proven by a test.
- ☐ Binary size and build time before and after are in `bench/RESULTS.md`.

---

## M17 — Tamper guard: config, CI and approvals ☐

**Build**

- More detectors, same base-versus-head method: new lint-disable comments and `allow` attributes, lowered coverage thresholds, CI steps removed or made unable to fail (`continue-on-error`, `|| true`, a deleted verify step), and `--no-verify` in scripts, hooks or CI.
- Human approval for the ambiguous cases, changed expected values and changed snapshots: verify blocks until a person records an approval. An ADR decides where approvals live so that an agent cannot write its own.
- A labelled corpus of honest and cheating changes, and a script that measures the false-positive and miss rates. Results in `bench/RESULTS.md`.

**Done when**

- ☐ Each detector has a caught case and a not-a-finding case in fixture repos.
- ☐ A changed snapshot blocks until approved, and an approval written by the change under review does not count, proven by tests.
- ☐ The false-positive rate on the corpus is measured and recorded.

---

## M18 — Guarded agent runner ☐

**Build**

- The runner and every guardrail in SPEC §7, in Rust, each with its own test.
- A fake agent adapter, so tests and CI never call a real model.
- The re-check after a run is the referee (SPEC §8): blame-aware verify plus the tamper guard.

**Done when**

- ☐ With the fake agent: a good fix is committed, a bad fix is rolled back, a fix touching a protected path is rejected, and a fix that skips a test is rejected by the tamper guard.
- ☐ The runner refuses to start on a dirty tree or when secrets are tracked.
- ☐ Every guardrail in SPEC §7 has a test named after it.

---

## M19 — Agent adapters and the ledger ☐ (finish and test locally)

**Build**

- `packages/agents`: the adapter interface plus Claude Code and Codex adapters, in TypeScript, called through the bridge. Confirm each CLI's flags from its current docs and pin them in tests.
- The ledger: every fix run appends one line to `.firescale/ledger.jsonl` with rule, agent, pass or fail, diff size, duration and cost (when the agent reports it).
- `fscale ledger [--json]`: runs, pass rate and total cost, per rule and per agent.

**Done when**

- ☐ A fake-agent run appends one schema-valid ledger line, for a pass and for a fail.
- ☐ `fscale ledger` summarises a fixture ledger, with a snapshot test, and copes with a damaged line.
- ☐ Manual local test: a real agent fixes one real violation end to end. Result noted in the PR.

---

## M20 to M23 — Stack packs ☐

| Milestone | Pack       | Status |
| --------- | ---------- | ------ |
| M20       | `python`   | ☐      |
| M21       | `nextjs`   | ☐      |
| M22       | `supabase` | ☐      |
| M23       | `expo`     | ☐      |

One milestone and one pull request per pack, in this order (simplest toolchain first). Each follows the same shape.

**Build**

- Research the stack's current official setup docs first, and record what the pack assumes in an ADR.
- Five to eight rules whose fixes create or repair files, each with a SKILL.md and fixture tests.
- `fscale init --packs core,<stack>` builds a working starter through the fix loop.

**Done when**

- ☐ `fscale init --packs core,<stack>` in an empty folder yields a repo where `fscale check` passes and the stack's own build or test command succeeds (run by hand where CI cannot install the stack; noted in the PR).
- ☐ `fscale adopt` on a fixture repo of that stack reports only real gaps.
- ☐ Every fix is idempotent, proven by tests.

---

## Later (not ordered)

- Publish to npm; docs site. Publishing is a manual step for the owner (see DECISIONS.md), and group-project mode needs it before real teammates can use it.
- Ingest alint output when alint is installed, and report it as diagnostics.
- A public benchmark built from fix ledgers.
- A thin MCP server; Nx plugins for EAS and Supabase.
